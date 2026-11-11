//! Photoshop and OpenRaster imports. Like upstream Compositor's conversion sheet
//! (`UI/PSDConversionSheet.swift`, `EditorSession.finishPSDReading`), what the import changes is
//! shown before anything is applied, and Cancel leaves the open documents untouched. Files Xuan
//! represents completely open without asking.
//!
//! A Photoshop file with artboards opens one tab per artboard, the first reusing an untouched
//! empty tab. Imported as a layer, each artboard becomes a folder named after it, laid out as on
//! the Photoshop canvas, inside the folder named after the file.
use super::theme::PaletteExt as _;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

use egui::RichText;
use xuan::{
    document::{Document, Layer, MAX_LAYERS},
    i18n::tr,
    io::{
        ImportReport, ImportSource, ora,
        psd::{self, Artboards, Imported, PixelBudget},
    },
};

use super::{EditorApp, Session, widgets};

/// A read Photoshop or OpenRaster file waiting for the user to accept its conversion report.
pub(super) struct PendingImport {
    path: PathBuf,
    /// The whole file, or one document per artboard.
    documents: Vec<Imported>,
    report: ImportReport,
    as_layer: bool,
}

/// Photoshop and OpenRaster files read but not applied yet, first in line shown first.
pub(super) type PendingImports = VecDeque<PendingImport>;

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

impl EditorApp {
    /// Read a `.psd`, `.psb` or `.ora` file, then apply it at once or after the conversion
    /// report.
    pub(super) fn open_photoshop(&mut self, path: &Path, as_layer: bool) {
        let as_layer = as_layer && !self.sessions.is_empty();
        // Layers added to a document share its pixel budget; beyond it they are cropped.
        let budget = match self.session().filter(|_| as_layer) {
            Some(session) => PixelBudget::remaining(&session.document),
            None => PixelBudget::default(),
        };
        let read = if ora::is_openraster(path) {
            ora::load(path, budget).map(|(document, report)| {
                let whole = Imported {
                    artboard: None,
                    origin: (0, 0),
                    document,
                };
                (vec![whole], report)
            })
        } else {
            let artboards = if as_layer {
                Artboards::Folders
            } else {
                Artboards::Documents
            };
            psd::load_documents(path, budget, artboards)
        };
        match read {
            Ok((documents, report)) => {
                let pending = PendingImport {
                    path: path.to_path_buf(),
                    documents,
                    report,
                    as_layer,
                };
                if pending.report.is_empty() {
                    self.apply_photoshop(pending);
                } else {
                    self.photoshop_imports.push_back(pending);
                }
            }
            Err(error) => {
                self.error = Some(format!(
                    "{} {}\n\n{error:#}",
                    tr("Could not open"),
                    path.display()
                ))
            }
        }
    }

    fn apply_photoshop(&mut self, pending: PendingImport) {
        let name = file_stem(&pending.path);
        let changed = !pending.report.is_empty();
        let artboards = pending.documents.iter().any(|d| d.artboard.is_some());
        if pending.as_layer && !self.sessions.is_empty() {
            let (layers, size) = lay_out(pending.documents);
            let action = match pending.report.source() {
                ImportSource::OpenRaster => tr("Import OpenRaster File"),
                _ => tr("Import Photoshop File"),
            };
            self.edit(action, |document| {
                insert_as_folder(document, layers, size, name)
            });
        } else {
            // The first artboard takes the place of a new document nobody has touched.
            let mut reuse =
                artboards && self.sessions.len() == 1 && self.sessions[0].is_untouched();
            let mut first = None;
            for imported in pending.documents {
                let title = imported.artboard.unwrap_or_else(|| name.clone());
                let session = Session::new(imported.document, title, None);
                if reuse {
                    self.sessions[0] = session;
                    reuse = false;
                    first.get_or_insert(0);
                } else {
                    self.sessions.push(session);
                    first.get_or_insert(self.sessions.len() - 1);
                }
            }
            if let Some(first) = first {
                self.current = first;
            }
            self.mask_target = false;
            self.dialog = None;
        }
        if changed && self.error.is_none() {
            self.status = tr("Imported with changes").into();
        }
    }

    /// The conversion report of the first pending import, with Cancel and Import.
    pub(super) fn photoshop_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.photoshop_imports.front() else {
            return;
        };
        let mut choice = None;
        widgets::Window::new(
            tr("Import “{}”?").replace(
                "{}",
                &pending
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            ),
        )
        .id(egui::Id::new("photoshop_import"))
        .default_width(480.0)
        .show_with_footer(ctx, |ui| {
            ui.label(tr(match pending.report.source() {
                ImportSource::OpenRaster => {
                    "Xuan will convert these OpenRaster features. Nothing is applied until you continue."
                }
                _ => {
                    "Xuan will convert these Photoshop features. Nothing is applied until you continue."
                }
            }));
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for line in pending.report.lines() {
                        ui.label(RichText::new(format!("• {line}")).color(ui.palette().text));
                    }
                });
        }, |ui, ()| {
            let response =
                widgets::dialog_footer(ui, widgets::FooterButtons::commit(tr("Import")), |_| {});
            if response.cancel {
                choice = Some(false);
            } else if response.commit {
                choice = Some(true);
            }
        });
        if choice.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                choice = Some(false);
            } else if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                choice = Some(true);
            }
        }
        if let Some(accept) = choice {
            self.finish_photoshop(accept);
        }
    }

    /// Apply (or drop) the first pending import, as its dialog's buttons do.
    pub(super) fn finish_photoshop(&mut self, accept: bool) {
        if let Some(pending) = self.photoshop_imports.pop_front()
            && accept
        {
            self.apply_photoshop(pending);
        }
    }
}

/// The layers of an import as one stack, and the size of the area they were laid out on. A file
/// without artboards keeps its layers and canvas; artboards become folders named after them, at
/// their place on the Photoshop canvas within the smallest area holding them all.
fn lay_out(documents: Vec<Imported>) -> (Vec<Layer>, (f32, f32)) {
    if let [whole] = documents.as_slice()
        && whole.artboard.is_none()
    {
        let document = documents.into_iter().next().unwrap().document;
        return (
            document.layers,
            (document.width as f32, document.height as f32),
        );
    }
    let left = documents.iter().map(|d| d.origin.0).min().unwrap_or(0);
    let top = documents.iter().map(|d| d.origin.1).min().unwrap_or(0);
    let right = documents
        .iter()
        .map(|d| d.origin.0 + i64::from(d.document.width))
        .max()
        .unwrap_or(0);
    let bottom = documents
        .iter()
        .map(|d| d.origin.1 + i64::from(d.document.height))
        .max()
        .unwrap_or(0);
    let mut layers = Vec::new();
    // The documents come top first; layers are stored bottom first.
    for imported in documents.into_iter().rev() {
        let document = imported.document;
        let mut folder = Layer::blank(
            imported.artboard.unwrap_or_default(),
            document.width,
            document.height,
        );
        folder.group = true;
        let (dx, dy) = (
            (imported.origin.0 - left) as f32,
            (imported.origin.1 - top) as f32,
        );
        for mut layer in document.layers {
            shift(&mut layer, dx, dy);
            layer.parent.get_or_insert(folder.id);
            layers.push(layer);
        }
        // Folders follow their contents.
        layers.push(folder);
    }
    (layers, ((right - left) as f32, (bottom - top) as f32))
}

fn shift(layer: &mut Layer, dx: f32, dy: f32) {
    layer.transform.x += dx;
    layer.transform.y += dy;
    if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
        placement.x += dx;
        placement.y += dy;
    }
}

/// Add an import's layers, laid out on an area of `size`, to `document` inside a new folder named
/// after the file, centered on the canvas and placed like any new layer.
fn insert_as_folder(
    document: &mut Document,
    mut layers: Vec<Layer>,
    (width, height): (f32, f32),
    name: String,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        document.layers.len() + layers.len() < MAX_LAYERS,
        tr("Too many layers")
    );
    let dx = ((document.width as f32 - width) * 0.5).round();
    let dy = ((document.height as f32 - height) * 0.5).round();
    let mut folder = Layer::blank(name, document.width, document.height);
    folder.group = true;
    for layer in &mut layers {
        shift(layer, dx, dy);
        if layer.parent.is_none() {
            layer.parent = Some(folder.id);
        }
    }
    // Inserting the folder places it, and its parent, as for any new layer.
    document.insert(folder);
    let index = document
        .layers
        .iter()
        .position(|l| Some(l.id) == document.active)
        .expect("the folder was just inserted");
    // Folders follow their contents.
    document.layers.splice(index..index, layers);
    document.validate()
}

#[cfg(test)]
impl EditorApp {
    pub(super) fn pending_photoshop_lines(&self) -> Option<Vec<String>> {
        self.photoshop_imports.front().map(|p| p.report.lines())
    }
}

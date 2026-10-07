//! Photoshop imports. Like upstream Compositor's conversion sheet (`UI/PSDConversionSheet.swift`,
//! `EditorSession.finishPSDReading`), what the import changes is shown before anything is
//! applied, and Cancel leaves the open documents untouched. Files Xuan represents completely
//! open without asking.
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
        ImportReport,
        psd::{self, PixelBudget},
    },
};

use super::{EditorApp, Session, widgets};

/// A read Photoshop file waiting for the user to accept its conversion report.
pub(super) struct PendingImport {
    path: PathBuf,
    document: Document,
    report: ImportReport,
    as_layer: bool,
}

/// Photoshop files read but not applied yet, first in line shown first.
pub(super) type PendingImports = VecDeque<PendingImport>;

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

impl EditorApp {
    /// Read a `.psd` or `.psb` file, then apply it at once or after the conversion report.
    pub(super) fn open_photoshop(&mut self, path: &Path, as_layer: bool) {
        let as_layer = as_layer && !self.sessions.is_empty();
        // Layers added to a document share its pixel budget; beyond it they are cropped.
        let budget = match self.session().filter(|_| as_layer) {
            Some(session) => PixelBudget::remaining(&session.document),
            None => PixelBudget::default(),
        };
        match psd::load(path, budget) {
            Ok((document, report)) => {
                let pending = PendingImport {
                    path: path.to_path_buf(),
                    document,
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
        if pending.as_layer && !self.sessions.is_empty() {
            let imported = pending.document;
            self.edit(tr("Import Photoshop File"), |document| {
                insert_as_folder(document, imported, name)
            });
        } else {
            self.sessions
                .push(Session::new(pending.document, name, None));
            self.current = self.sessions.len() - 1;
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
            ui.label(tr(
                "Xuan will convert these Photoshop features. Nothing is applied until you continue.",
            ));
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

/// Add a Photoshop document's layers to `document` inside a new folder named after the file,
/// centered on the canvas and placed like any new layer.
fn insert_as_folder(
    document: &mut Document,
    imported: Document,
    name: String,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        document.layers.len() + imported.layers.len() < MAX_LAYERS,
        tr("Too many layers")
    );
    let dx = ((document.width as f32 - imported.width as f32) * 0.5).round();
    let dy = ((document.height as f32 - imported.height as f32) * 0.5).round();
    let mut folder = Layer::blank(name, document.width, document.height);
    folder.group = true;
    let mut layers = imported.layers;
    for layer in &mut layers {
        layer.transform.x += dx;
        layer.transform.y += dy;
        if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            placement.x += dx;
            placement.y += dy;
        }
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

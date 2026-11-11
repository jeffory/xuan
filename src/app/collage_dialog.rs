//! File → New Collage… and Image → Collage Layout…: photos laid out in a grid of cells with
//! borders, after PhotoScape X's Collage. The collage is ordinary layers ([`xuan::collage`]);
//! these dialogs choose its layout and make it, or lay an existing one out again.
use std::path::{Path, PathBuf};

use egui::{Color32, RichText, Sense, vec2};
use uuid::Uuid;
use xuan::{
    collage::{self, Layout, MAX_TRACKS, Template},
    document::MAX_SIDE,
    i18n::tr,
    io,
};

use super::{Dialog, EditorApp, Session, theme::PaletteExt as _, widgets};

/// Raster formats a collage's photos are read from; portal filters may be case-sensitive.
const PHOTO_EXTENSIONS: [&str; 11] = [
    "png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif", "heic", "heif", "hif",
];

/// The open dialog.
pub(super) struct CollageEdit {
    pub layout: Layout,
    /// The new collage's canvas size; a layout change keeps the canvas.
    pub size: [u32; 2],
    /// Photos for the new collage's cells, in order.
    pub photos: Vec<PathBuf>,
    /// Laying out the current document's collage again rather than making a new one.
    pub relayout: bool,
}

/// The dialog's choices that are remembered for the next collage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CollageSettings {
    pub layout: Layout,
    pub size: [u32; 2],
}

impl Default for CollageSettings {
    fn default() -> Self {
        Self {
            layout: Layout::default(),
            size: [2400, 1800],
        }
    }
}

fn pick_photos() -> Option<Vec<PathBuf>> {
    let extensions: Vec<String> = PHOTO_EXTENSIONS
        .iter()
        .flat_map(|e| [e.to_string(), e.to_ascii_uppercase()])
        .collect();
    rfd::FileDialog::new()
        .add_filter(tr("Images"), &extensions)
        .pick_files()
}

fn file_name(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

impl EditorApp {
    /// File → New Collage…, with the choices of the last collage.
    pub(super) fn open_new_collage(&mut self) {
        self.collage = Some(CollageEdit {
            layout: self.collage_settings.layout,
            size: self.collage_settings.size,
            photos: Vec::new(),
            relayout: false,
        });
        self.dialog = Some(Dialog::Collage);
    }

    /// Image → Collage Layout…, with the current document's layout.
    pub(super) fn open_collage_layout(&mut self) {
        let Some(session) = self.session() else {
            return;
        };
        let Some(record) = &session.document.collage else {
            return;
        };
        self.collage = Some(CollageEdit {
            layout: record.layout,
            size: [session.document.width, session.document.height],
            photos: Vec::new(),
            relayout: true,
        });
        self.dialog = Some(Dialog::Collage);
    }

    /// Makes the collage `edit` describes as a new document, its photos filling the cells in
    /// order. Photos that cannot be read are reported and their cells left empty; photos beyond
    /// the cells, or beyond the pixels a project may bring in, are left out.
    pub(super) fn create_collage(&mut self, edit: &CollageEdit) {
        let [width, height] = edit.size;
        let mut document = match collage::new_document(width, height, edit.layout) {
            Ok(document) => document,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        document.resolution = self.resolution;
        let cells = collage::cells(&document);
        let budget = xuan::limits::get().project_pixels;
        let mut used = 0_u64;
        let mut problems = Vec::new();
        for (path, cell) in edit.photos.iter().zip(&cells) {
            let image = match io::import_image(path) {
                Ok(image) => image,
                Err(error) => {
                    problems.push(format!("{}: {error:#}", path.display()));
                    continue;
                }
            };
            used += u64::from(image.width()) * u64::from(image.height());
            if used > budget {
                problems.push(
                    tr("The photos add up to more than {}, the most this computer opens at once")
                        .replace("{}", &xuan::limits::megapixels(budget)),
                );
                break;
            }
            let photo = xuan::document::Layer::image(file_name(path), image);
            if let Err(error) = collage::place(&mut document, *cell, photo) {
                problems.push(format!("{}: {error:#}", path.display()));
            }
        }
        if let Some(first) = cells.first() {
            document.select(*first, false);
        }
        self.collage_settings = CollageSettings {
            layout: edit.layout,
            size: edit.size,
        };
        self.sessions
            .push(Session::new(document, tr("Collage").into(), None));
        self.current = self.sessions.len() - 1;
        self.mask_target = false;
        self.dialog = None;
        self.status = tr("New Collage").into();
        if !problems.is_empty() {
            self.error = Some(format!(
                "{}\n\n{}",
                tr("Some photos could not be added to the collage."),
                problems.join("\n")
            ));
        }
    }

    /// Lays the current document's collage out again as one undo step.
    pub(super) fn apply_collage_layout(&mut self, layout: Layout) {
        self.collage_settings.layout = layout;
        self.edit(tr("Collage Layout"), |doc| collage::relayout(doc, layout));
        self.dialog = None;
    }

    /// The cell an image inserted as a layer goes into, if any: while files are inserted
    /// together, the next empty cell after the last one filled; otherwise the active cell.
    pub(super) fn import_cell(&self) -> Option<Uuid> {
        let document = &self.session()?.document;
        match self.collage_filled {
            Some(last) => collage::next_empty_cell(document, last),
            None => collage::active_cell(document),
        }
    }

    /// Inserts files as layers, one after another; in a collage, images fill the active cell
    /// and then the empty cells after it.
    pub(super) fn insert_files(&mut self, paths: &[PathBuf]) {
        self.collage_filled = None;
        for path in paths {
            self.open_path(path, true);
        }
        self.collage_filled = None;
    }

    pub(super) fn collage_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.collage.take() else {
            self.dialog = None;
            return;
        };
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let relayout = edit.relayout;
        let title = if relayout {
            tr("Collage layout")
        } else {
            tr("New collage")
        };
        widgets::Window::new(title)
            .id("collage")
            .open(&mut open)
            .default_width(380.0)
            .show_with_footer(
                ctx,
                |ui| collage_controls(ui, &mut edit),
                |ui, valid| {
                    let commit = if relayout {
                        tr("Apply")
                    } else {
                        tr("Create collage")
                    };
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(commit).enabled(valid),
                        |_| {},
                    );
                    apply = response.commit && valid;
                    cancel = response.cancel;
                },
            );
        if apply {
            self.collage = None;
            if edit.relayout {
                self.apply_collage_layout(edit.layout);
            } else {
                self.create_collage(&edit);
            }
        } else if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.collage = None;
            self.dialog = None;
        } else {
            self.collage = Some(edit);
        }
    }
}

/// The dialog's controls; whether the layout fits the canvas.
fn collage_controls(ui: &mut egui::Ui, edit: &mut CollageEdit) -> bool {
    let muted = ui.palette().muted;
    ui.add_space(4.0);
    ui.label(
        RichText::new(tr(
            "Photos in a grid of cells. Each cell is a folder with its photo clipped to a frame, so everything stays editable.",
        ))
        .color(muted),
    );
    ui.add_space(10.0);
    let layout = &mut edit.layout;
    ui.horizontal(|ui| {
        ui.label(tr("Layout"));
        widgets::PopUp::from_id_salt("collage_template")
            .selected_text(tr(layout.template.name()))
            .width(200.0)
            .show_ui(ui, |ui| {
                for template in Template::ALL {
                    widgets::menu_choice(ui, &mut layout.template, template, tr(template.name()));
                }
            });
    });
    if layout.template == Template::Grid {
        ui.horizontal(|ui| {
            ui.label(tr("Columns"));
            ui.add(widgets::Number::new(&mut layout.columns).range(1..=MAX_TRACKS));
            ui.add_space(12.0);
            ui.label(tr("Rows"));
            ui.add(widgets::Number::new(&mut layout.rows).range(1..=MAX_TRACKS));
        });
    }
    if !edit.relayout {
        ui.horizontal(|ui| {
            ui.label(tr("Width"));
            ui.add(
                widgets::Number::new(&mut edit.size[0])
                    .range(1..=MAX_SIDE)
                    .suffix(" px")
                    .speed(1.0),
            );
            ui.add_space(12.0);
            ui.label(tr("Height"));
            ui.add(
                widgets::Number::new(&mut edit.size[1])
                    .range(1..=MAX_SIDE)
                    .suffix(" px")
                    .speed(1.0),
            );
        });
    }
    ui.add_space(6.0);
    egui::Grid::new("collage_borders")
        .num_columns(2)
        .spacing(vec2(12.0, 6.0))
        .show(ui, |ui| {
            ui.label(tr("Spacing"));
            ui.add(
                widgets::Number::new(&mut layout.spacing)
                    .range(0..=MAX_SIDE)
                    .suffix(" px"),
            );
            ui.end_row();
            ui.label(tr("Border"));
            ui.add(
                widgets::Number::new(&mut layout.border)
                    .range(0..=MAX_SIDE)
                    .suffix(" px"),
            );
            ui.end_row();
            ui.label(tr("Corner radius"));
            ui.add(
                widgets::Number::new(&mut layout.corner_radius)
                    .range(0.0..=MAX_SIDE as f32)
                    .max_decimals(0)
                    .suffix(" px"),
            );
            ui.end_row();
            ui.label(tr("Border colour"));
            widgets::color_well(ui, &mut layout.color);
            ui.end_row();
        });
    let [width, height] = edit.size;
    let valid = xuan::document::validate_size(width, height)
        .and_then(|()| edit.layout.cells(width, height));
    ui.add_space(10.0);
    match &valid {
        Ok(cells) => preview(ui, &edit.layout, edit.size, cells),
        Err(error) => {
            ui.colored_label(ui.palette().error, error.to_string());
        }
    }
    if !edit.relayout {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if widgets::button(ui, tr("Choose photos…")).clicked()
                && let Some(paths) = pick_photos()
            {
                edit.photos = paths;
            }
            if !edit.photos.is_empty() && widgets::button(ui, tr("Clear photos")).clicked() {
                edit.photos.clear();
            }
        });
        let cells = edit.layout.cell_count();
        let note = match edit.photos.len() {
            0 => tr("No photos yet: drop or import images into the cells later.").to_owned(),
            n if n > cells => tr("{} photos; the first {} fill the cells")
                .replacen("{}", &n.to_string(), 1)
                .replacen("{}", &cells.to_string(), 1),
            n => tr("Photos: {}").replace("{}", &n.to_string()),
        };
        ui.label(RichText::new(note).color(muted));
    } else {
        ui.label(
            RichText::new(tr(
                "Photos follow their cells. Cells the new layout does not have are removed with their photos.",
            ))
            .color(muted),
        );
    }
    valid.is_ok()
}

/// A small picture of the layout: the canvas in the border colour with the cells on it.
fn preview(
    ui: &mut egui::Ui,
    layout: &Layout,
    [width, height]: [u32; 2],
    cells: &[collage::CellRect],
) {
    let room = vec2(ui.available_width().min(300.0), 120.0);
    let scale = (room.x / width as f32).min(room.y / height as f32);
    let size = vec2(width as f32 * scale, height as f32 * scale).max(vec2(1.0, 1.0));
    let (rect, _) = ui.allocate_exact_size(vec2(room.x, size.y), Sense::hover());
    let canvas = egui::Rect::from_min_size(rect.min + vec2((room.x - size.x) * 0.5, 0.0), size);
    let p = ui.palette();
    let [r, g, b, a] = layout.color;
    let painter = ui.painter();
    painter.rect_filled(canvas, 0.0, Color32::from_rgba_unmultiplied(r, g, b, a));
    painter.rect_stroke(
        canvas,
        0.0,
        egui::Stroke::new(1.0_f32, p.widget_stroke),
        egui::StrokeKind::Outside,
    );
    for cell in cells {
        let cell_rect = egui::Rect::from_min_size(
            canvas.min + vec2(cell.x as f32, cell.y as f32) * scale,
            vec2(cell.width as f32, cell.height as f32) * scale,
        );
        let radius = (layout.corner_radius * scale).min(cell_rect.size().min_elem() * 0.5);
        painter.rect_filled(cell_rect, radius, p.muted.gamma_multiply(0.6));
    }
}

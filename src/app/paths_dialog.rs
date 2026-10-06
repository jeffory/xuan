//! Select → Paths…: the paths kept with the document, as in Photoshop's Paths panel. A path
//! is added from SVG path data, and can be filled with the foreground colour, stroked with the
//! current brush, made into a selection (feathered, and combined by the selection mode) or
//! made into an editable shape layer. Each action is one undo step.
use anyhow::{Context as _, Result};
use uuid::Uuid;
use xuan::{
    document::{Document, Point},
    i18n::tr,
    paint::{self, Brush},
    selection::SelectionMode,
    vector::{FillRule, NamedPath, VectorPath},
};

use super::{Dialog, EditorApp, widgets};

/// The dialog's state, kept while it is open.
#[derive(Clone, Debug, Default)]
pub(super) struct PathsEdit {
    /// The path the actions use.
    pub selected: Option<Uuid>,
    /// SVG path data for a new path.
    pub data: String,
    /// The selected path's name as edited.
    pub name: String,
    pub feather: f32,
    pub mode: SelectionMode,
    pub evenodd: bool,
    /// Why the last action failed.
    pub error: Option<String>,
}

/// An action on the selected path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PathAction {
    Fill,
    Stroke,
    Select,
    ShapeLayer,
    Rename,
    Delete,
}

/// Paint `brush` along each subpath of `path` on the active layer, as one stroke per subpath.
pub(super) fn stroke_path(document: &mut Document, path: &VectorPath, brush: &Brush) -> Result<()> {
    let lines = path.flatten(0.2);
    anyhow::ensure!(!lines.is_empty(), "The path has no points");
    for line in lines {
        let mut points = line.points.clone();
        if line.closed {
            points.push(points[0]);
        }
        let samples: Vec<(Point, Brush)> = points.into_iter().map(|p| (p, brush.clone())).collect();
        paint::Stroke::default().path(
            document,
            &samples,
            paint::StrokeOptions {
                mode: paint::PaintMode::Paint,
                mask_target: false,
                source: None,
                clone_offset: Point::default(),
            },
        )?;
    }
    Ok(())
}

impl EditorApp {
    pub(super) fn open_paths(&mut self) {
        let Some(session) = self.session() else {
            return;
        };
        // The path the Pen shows, or else the first.
        let shown = match self.pen_target() {
            Some(super::pen_tool::PenTarget::Path(id)) => {
                session.document.paths.iter().find(|p| p.id == id)
            }
            _ => None,
        };
        let first = shown.or(session.document.paths.first());
        let selected = first.map(|p| p.id);
        self.paths_edit = Some(PathsEdit {
            selected: first.map(|p| p.id),
            name: first.map(|p| p.name.clone()).unwrap_or_default(),
            ..PathsEdit::default()
        });
        self.dialog = Some(Dialog::Paths);
        if selected.is_some() {
            self.pen_select(selected.map(super::pen_tool::PenTarget::Path));
        }
    }

    /// Add `data` to the document's paths as a new path; its id.
    pub(super) fn add_path(&mut self, data: &str) -> Result<Uuid> {
        let d = VectorPath::parse(data)?;
        let mut id = None;
        let mut failure = None;
        self.edit(tr("New Path"), |document| {
            let path = NamedPath::new(xuan::vector::next_path_name(&document.paths), d);
            id = Some(path.id);
            document.paths.push(path);
            xuan::vector::validate_paths(&document.paths)
        });
        if let Some(error) = self.error.take() {
            failure = Some(error);
        }
        match (id, failure) {
            (_, Some(error)) => Err(anyhow::anyhow!(error)),
            (Some(id), None) => Ok(id),
            (None, None) => anyhow::bail!("Open a document first"),
        }
    }

    /// Run `action` on the path `id`, as one undo step.
    pub(super) fn path_action(
        &mut self,
        id: Uuid,
        action: PathAction,
        edit: &PathsEdit,
    ) -> Result<()> {
        let path = self
            .session()
            .and_then(|s| s.document.paths.iter().find(|p| p.id == id))
            .map(|p| p.d.clone())
            .context("The path is gone")?;
        let rule = if edit.evenodd {
            FillRule::Evenodd
        } else {
            FillRule::Nonzero
        };
        let color = self.brush.color;
        let brush = self.brush.clone();
        let (feather, mode) = (edit.feather, edit.mode);
        match action {
            PathAction::Fill => self.edit(tr("Fill Path"), |document| {
                paint::fill_path(document, &path, rule, color)
            }),
            PathAction::Stroke => self.edit(tr("Stroke Path"), |document| {
                stroke_path(document, &path, &brush)
            }),
            PathAction::Select => self.edit_selection(tr("Make Selection"), |document| {
                let mut mask = path.mask(rule, document.width, document.height);
                if feather > 0.0 {
                    mask = xuan::gpu::blur_gray(&mask, feather);
                }
                xuan::selection::combine(document, mask, mode);
            }),
            PathAction::ShapeLayer => self.edit(tr("Shape Layer from Path"), |document| {
                let layer = paint::path_shape(&path, rule, color)?;
                document.insert(layer);
                Ok(())
            }),
            PathAction::Rename => {
                let name = edit.name.trim().to_owned();
                anyhow::ensure!(
                    !name.is_empty() && name.len() <= 256,
                    "A path name has 1 to 256 bytes"
                );
                self.edit(tr("Rename Path"), |document| {
                    if let Some(path) = document.paths.iter_mut().find(|p| p.id == id) {
                        path.name = name;
                    }
                    Ok(())
                });
            }
            PathAction::Delete => self.edit(tr("Delete Path"), |document| {
                document.paths.retain(|p| p.id != id);
                Ok(())
            }),
        }
        match self.error.take() {
            Some(error) => Err(anyhow::anyhow!(error)),
            None => Ok(()),
        }
    }

    pub(super) fn paths_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.paths_edit.take() else {
            self.dialog = None;
            return;
        };
        let paths: Vec<(Uuid, String)> = self
            .session()
            .map(|s| {
                (s.document.paths.iter())
                    .map(|p| (p.id, p.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if edit
            .selected
            .is_some_and(|id| paths.iter().all(|p| p.0 != id))
        {
            edit.selected = paths.first().map(|p| p.0);
            edit.name = paths.first().map(|p| p.1.clone()).unwrap_or_default();
        }
        let mut open = true;
        let mut action = None;
        let mut add = false;
        let mut edit_on_canvas = false;
        let before = edit.selected;
        widgets::Window::new(tr("Paths"))
            .id("paths")
            .open(&mut open)
            .default_width(380.0)
            .show(ctx, |ui| {
                if paths.is_empty() {
                    ui.label(tr("This document has no paths yet."));
                }
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (id, name) in &paths {
                            if ui
                                .selectable_label(edit.selected == Some(*id), name)
                                .clicked()
                            {
                                edit.selected = Some(*id);
                                edit.name = name.clone();
                            }
                        }
                    });
                ui.add_enabled_ui(edit.selected.is_some(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr("Name"));
                        ui.add(egui::TextEdit::singleline(&mut edit.name).desired_width(160.0));
                        if widgets::button(ui, tr("Rename")).clicked() {
                            action = Some(PathAction::Rename);
                        }
                        if widgets::button(ui, tr("Delete")).clicked() {
                            action = Some(PathAction::Delete);
                        }
                        if widgets::button(ui, tr("Edit with Pen")).clicked() {
                            edit_on_canvas = true;
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if widgets::button(ui, tr("Fill Path")).clicked() {
                            action = Some(PathAction::Fill);
                        }
                        if widgets::button(ui, tr("Stroke Path")).clicked() {
                            action = Some(PathAction::Stroke);
                        }
                        if widgets::button(ui, tr("Shape Layer")).clicked() {
                            action = Some(PathAction::ShapeLayer);
                        }
                    });
                    widgets::checkbox(
                        ui,
                        &mut edit.evenodd,
                        tr("Even-odd fill (inner subpaths cut holes)"),
                    );
                    ui.horizontal(|ui| {
                        if widgets::button(ui, tr("Make Selection")).clicked() {
                            action = Some(PathAction::Select);
                        }
                        ui.label(tr("Feather"));
                        ui.add(
                            widgets::Number::new(&mut edit.feather)
                                .range(0.0..=256.0)
                                .speed(0.5)
                                .suffix(" px"),
                        );
                    });
                    widgets::segmented(
                        ui,
                        &mut edit.mode,
                        &[
                            (SelectionMode::Replace, tr("New")),
                            (SelectionMode::Add, tr("Add")),
                            (SelectionMode::Subtract, tr("Subtract")),
                            (SelectionMode::Intersect, tr("Intersect")),
                        ],
                    );
                });
                ui.separator();
                ui.label(tr(
                    "New path from SVG path data, e.g. M 0 70 C 12 64 38 64 51 70 Z",
                ));
                ui.add(
                    egui::TextEdit::multiline(&mut edit.data)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if widgets::button(ui, tr("Add Path")).clicked() {
                    add = true;
                }
                if let Some(error) = &edit.error {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
                }
            });
        if add {
            match self.add_path(&edit.data) {
                Ok(id) => {
                    edit.selected = Some(id);
                    edit.data.clear();
                    edit.error = None;
                    edit.name = self
                        .session()
                        .and_then(|s| s.document.paths.iter().find(|p| p.id == id))
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                }
                Err(error) => edit.error = Some(format!("{error:#}")),
            }
        }
        if let (Some(action), Some(id)) = (action, edit.selected) {
            edit.error = self
                .path_action(id, action, &edit)
                .err()
                .map(|e| format!("{e:#}"));
        }
        // The selected path is shown on the canvas, and the Pen edits it.
        if edit.selected != before || edit_on_canvas {
            self.pen_select(edit.selected.map(super::pen_tool::PenTarget::Path));
        }
        if edit_on_canvas && edit.selected.is_some() {
            self.set_tool(super::Tool::Pen);
            open = false;
        }
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dialog = None;
        } else {
            self.paths_edit = Some(edit);
        }
    }
}

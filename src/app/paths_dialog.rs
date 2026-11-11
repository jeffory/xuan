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

use super::theme::PaletteExt as _;
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
    /// Stroke Path paints with the Pencil's hard pixels instead of the Brush.
    pub pencil: bool,
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

/// Paint `brush` along each subpath of `path` on the active layer, as one stroke per subpath,
/// with the Brush or (`pencil`) the Pencil.
pub(super) fn stroke_path(
    document: &mut Document,
    path: &VectorPath,
    brush: &Brush,
    pencil: bool,
) -> Result<()> {
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
                mode: if pencil {
                    paint::PaintMode::Pencil
                } else {
                    paint::PaintMode::Paint
                },
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
        self.add_vector_path(tr("New Path"), d)
    }

    /// Select → Paths… → Make Path from Selection: the selection's outline (where it is at
    /// least half selected) as a new path, within a pixel of the pixel edges; its id.
    pub(super) fn path_from_selection(&mut self) -> Result<Uuid> {
        let selection = self
            .session()
            .and_then(|s| s.document.selection.clone())
            .context("Make a selection first")?;
        let d = xuan::path_edit::trace_mask(&selection, 1.0).context("Nothing is selected")?;
        self.add_vector_path(tr("Make Path from Selection"), d)
    }

    fn add_vector_path(&mut self, name: &str, d: VectorPath) -> Result<Uuid> {
        let mut id = None;
        let mut failure = None;
        self.edit(name, |document| {
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
        let (feather, mode, pencil) = (edit.feather, edit.mode, edit.pencil);
        match action {
            PathAction::Fill => self.edit(tr("Fill Path"), |document| {
                paint::fill_path(document, &path, rule, color)
            }),
            PathAction::Stroke => self.edit(tr("Stroke Path"), |document| {
                stroke_path(document, &path, &brush, pencil)
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
        let mut from_selection = false;
        let mut draw_with_pen = false;
        let has_selection = self
            .session()
            .is_some_and(|s| s.document.selection.is_some());
        let before = edit.selected;
        widgets::Window::new(tr("Paths"))
            .id("paths")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                if paths.is_empty() {
                    // Nothing to act on yet: only the ways to make a path.
                    ui.vertical_centered(|ui| {
                        ui.add_space(12.0);
                        widgets::subheading(ui, tr("No paths yet"));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(tr(
                                "Draw one with the Pen, or trace the selection's outline.",
                            ))
                            .color(ui.palette().muted),
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            // Centre the pair of buttons by the width they took last frame.
                            let id = ui.id().with("empty_actions");
                            let width: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
                            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                            let pen = widgets::primary_button(ui, tr("Draw with Pen"));
                            let trace = ui
                                .add_enabled_ui(has_selection, |ui| {
                                    widgets::button(ui, tr("From selection"))
                                })
                                .inner;
                            draw_with_pen = pen.clicked();
                            from_selection = trace.clicked();
                            let used = trace.rect.max.x - pen.rect.min.x;
                            ui.data_mut(|d| d.insert_temp(id, used));
                        });
                        ui.add_space(12.0);
                    });
                } else {
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
                        path_controls(ui, &mut edit, &mut action, &mut edit_on_canvas);
                    });
                    ui.separator();
                    if ui
                        .add_enabled_ui(has_selection, |ui| {
                            widgets::button(ui, tr("Make Path from Selection"))
                        })
                        .inner
                        .clicked()
                    {
                        from_selection = true;
                    }
                }
                egui::CollapsingHeader::new(tr("Advanced"))
                    .id_salt("paths_advanced")
                    .show(ui, |ui| {
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
                    });
                if let Some(error) = &edit.error {
                    ui.colored_label(ui.palette().error, error);
                }
            });
        if add || from_selection {
            let added = if from_selection {
                self.path_from_selection()
            } else {
                self.add_path(&edit.data)
            };
            match added {
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
        // Draw with Pen: a new path is drawn on the canvas, so nothing is shown to edit.
        if draw_with_pen {
            self.pen_select(None);
            self.set_tool(super::Tool::Pen);
            open = false;
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

/// The selected path's name, its actions and their options.
fn path_controls(
    ui: &mut egui::Ui,
    edit: &mut PathsEdit,
    action: &mut Option<PathAction>,
    edit_on_canvas: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label(tr("Name"));
        ui.add(egui::TextEdit::singleline(&mut edit.name).desired_width(f32::INFINITY));
    });
    // The buttons get their own row, so they never widen the window.
    ui.horizontal_wrapped(|ui| {
        if widgets::button(ui, tr("Rename")).clicked() {
            *action = Some(PathAction::Rename);
        }
        if widgets::button(ui, tr("Delete")).clicked() {
            *action = Some(PathAction::Delete);
        }
        if widgets::button(ui, tr("Edit with Pen")).clicked() {
            *edit_on_canvas = true;
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        if widgets::button(ui, tr("Fill Path")).clicked() {
            *action = Some(PathAction::Fill);
        }
        if widgets::button(ui, tr("Stroke Path")).clicked() {
            *action = Some(PathAction::Stroke);
        }
        if widgets::button(ui, tr("Shape Layer")).clicked() {
            *action = Some(PathAction::ShapeLayer);
        }
    });
    ui.horizontal(|ui| {
        ui.label(tr("Stroke with"));
        widgets::segmented(
            ui,
            &mut edit.pencil,
            &[(false, tr("Brush")), (true, tr("Pencil"))],
        );
    });
    widgets::checkbox(
        ui,
        &mut edit.evenodd,
        tr("Even-odd fill (inner subpaths cut holes)"),
    );
    ui.horizontal(|ui| {
        if widgets::button(ui, tr("Make Selection")).clicked() {
            *action = Some(PathAction::Select);
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
}

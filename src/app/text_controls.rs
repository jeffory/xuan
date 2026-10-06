use super::theme::PaletteExt as _;
use egui::RichText;
use uuid::Uuid;
use xuan::i18n::tr;
use xuan::{
    document::{Layer, Point},
    render,
    text::{self, PathAlign, PathSide, TextPath, TextRenderer, TextStyle},
};

/// The Text window's path section: attach one of the document's paths for the text to
/// follow, or detach it, and how the text follows it. Text on a path is edited here; the
/// canvas shows it along the path.
/// Returns the path picked from the menu, `Some(None)` for none.
fn path_options(
    ui: &mut egui::Ui,
    edit: &mut TextEdit,
    paths: &[String],
) -> Option<Option<usize>> {
    const DETACH: usize = usize::MAX;
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.label(tr("Path"));
        let selected = if edit.style.path.is_some() {
            tr("Text on a path")
        } else {
            tr("None (text box)")
        };
        widgets::PopUp::from_id_salt("text_path")
            .selected_text(selected)
            .width(220.0)
            .show_ui(ui, |ui| {
                widgets::menu_choice(ui, &mut pick, Some(DETACH), tr("None (text box)"));
                for (index, name) in paths.iter().enumerate() {
                    widgets::menu_choice(ui, &mut pick, Some(index), name);
                }
            });
    });
    let pick = pick.map(|index| (index != DETACH).then_some(index));
    let Some(path) = &mut edit.style.path else {
        if paths.is_empty() {
            ui.label(
                RichText::new(tr(
                    "Save a path with Select → Paths… to set text along it.",
                ))
                .small()
                .color(ui.palette().muted),
            );
        }
        return pick;
    };
    let options = &mut path.options;
    ui.horizontal(|ui| {
        ui.label(tr("Start"));
        ui.add(
            widgets::Number::new(&mut options.start_offset)
                .range(-100.0..=100.0)
                .suffix(" %")
                .max_decimals(1),
        );
        ui.add_space(12.0);
        widgets::segmented(
            ui,
            &mut options.align,
            &[
                (PathAlign::Start, tr("Start")),
                (PathAlign::Center, tr("Center")),
                (PathAlign::End, tr("End")),
            ],
        );
    });
    ui.horizontal(|ui| {
        ui.label(tr("Letter spacing"));
        ui.add(
            widgets::Number::new(&mut options.letter_spacing)
                .range(-1000.0..=1000.0)
                .suffix(" px")
                .max_decimals(1),
        );
        ui.add_space(12.0);
        ui.label(tr("Baseline shift"));
        ui.add(
            widgets::Number::new(&mut options.baseline_shift)
                .range(-10_000.0..=10_000.0)
                .suffix(" px")
                .max_decimals(1),
        );
    });
    ui.horizontal(|ui| {
        let mut flip = options.side == PathSide::Right;
        if widgets::checkbox(ui, &mut flip, tr("Flip to the other side")).changed() {
            options.side = if flip {
                PathSide::Right
            } else {
                PathSide::Left
            };
        }
        widgets::checkbox(ui, &mut options.rotate, tr("Turn letters with the path"));
    });
    ui.horizontal(|ui| {
        let mut ramp = options.size_end.is_some();
        if widgets::checkbox(ui, &mut ramp, tr("End size")).changed() {
            options.size_end = ramp.then_some(edit.style.size);
        }
        if let Some(size) = &mut options.size_end {
            ui.add(
                widgets::Number::new(size)
                    .range(1.0..=1024.0)
                    .suffix(" px")
                    .max_decimals(1),
            );
        }
    });
    ui.horizontal(|ui| {
        ui.label(tr("Opacity"));
        for (index, value) in [&mut options.opacity_start, &mut options.opacity_end]
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                ui.label("→");
            }
            let mut percent = *value * 100.0;
            if ui
                .add(
                    widgets::Number::new(&mut percent)
                        .range(0.0..=100.0)
                        .suffix(" %")
                        .max_decimals(0),
                )
                .changed()
            {
                *value = (percent / 100.0).clamp(0.0, 1.0);
            }
        }
    });
    ui.label(
        RichText::new(tr(
            "Letters past the end of an open path are hidden; on a closed path the text wraps around.",
        ))
        .small()
        .color(ui.palette().muted),
    );
    pick
}

use super::{Dialog, EditorApp, Tool, font_picker::FontPicker, widgets};

pub(super) struct TextEdit {
    pub(super) target: Uuid,
    original: Layer,
    pub(super) style: TextStyle,
    fonts: FontPicker,
    focus: bool,
    changed: bool,
    pub(super) error: Option<String>,
}

impl EditorApp {
    pub(super) fn text_options(&mut self, ui: &mut egui::Ui) {
        let active = self
            .session()
            .and_then(|s| s.document.active())
            .filter(|layer| layer.text.is_some() && !layer.locked)
            .map(|layer| layer.id);
        if ui
            .add_enabled(active.is_some(), widgets::Button::new(tr("Edit text…")))
            .clicked()
        {
            self.start_text(active, Point::default());
        }
        if ui
            .add_enabled(
                self.session().is_some(),
                widgets::Button::new(tr("Add text…")),
            )
            .clicked()
            && let Some(session) = self.session()
        {
            let point = Point::new(
                session.document.width as f32 * 0.25,
                session.document.height as f32 * 0.25,
            );
            self.start_text(None, point);
        }
        ui.label(RichText::new(tr("Click the canvas to place text")).color(ui.palette().muted));
    }

    pub(super) fn text_click(&mut self, point: Point) {
        let Some(session) = self.session() else {
            return;
        };
        let document = &session.document;
        if !(0.0..document.width as f32).contains(&point.x)
            || !(0.0..document.height as f32).contains(&point.y)
        {
            return;
        }
        let hit = render::hit_test(document, point);
        let target = hit
            .and_then(|id| {
                document
                    .layers
                    .iter()
                    .find(|layer| layer.id == id && layer.text.is_some())
            })
            .or_else(|| {
                document.active().filter(|layer| {
                    let unit = layer.transform.inverse(point);
                    hit.is_none()
                        && layer.visible
                        && layer.text.is_some()
                        && (0.0..=1.0).contains(&unit.x)
                        && (0.0..=1.0).contains(&unit.y)
                })
            })
            .map(|layer| layer.id);
        self.start_text(target, point);
    }

    pub(super) fn start_text(&mut self, target: Option<Uuid>, point: Point) {
        if self.dialog.is_some() || self.job.is_some() || self.session().is_none() {
            return;
        }
        self.cancel_gesture();
        let (layer, is_new) = if let Some(id) = target {
            let Some(layer) = self
                .session()
                .unwrap()
                .document
                .layers
                .iter()
                .find(|layer| layer.id == id && layer.text.is_some())
                .cloned()
            else {
                return;
            };
            if layer.locked {
                self.status = tr("Unlock the text layer to edit it").into();
                return;
            }
            (layer, false)
        } else {
            let mut style = self.text_style.clone();
            style.content = tr("Text").into();
            style.color = self.brush.color;
            let renderer = self.text_renderer.get_or_insert_with(TextRenderer::default);
            let pixels = match renderer.render(&style) {
                Ok(pixels) => pixels,
                Err(error) => {
                    self.error = Some(error.to_string());
                    return;
                }
            };
            let mut layer = Layer::image(style.layer_name(), pixels);
            layer.transform.x = point.x;
            layer.transform.y = point.y;
            layer.text = Some(style);
            (layer, true)
        };
        self.text_renderer.get_or_insert_with(TextRenderer::default);
        let session = self.session_mut().unwrap();
        session.history.commit();
        session.history.begin(
            if is_new {
                tr("Add Text")
            } else {
                tr("Edit Text")
            },
            &session.document,
        );
        if is_new {
            session.document.insert(layer.clone());
        } else {
            session.document.select(layer.id, false);
        }
        if let Err(error) = session.document.validate() {
            session.history.cancel(&mut session.document);
            self.error = Some(error.to_string());
            return;
        }
        let layer = session.document.active().unwrap().clone();
        session.invalidate();
        self.text_edit = Some(TextEdit {
            target: layer.id,
            style: layer.text.clone().unwrap(),
            original: layer,
            fonts: FontPicker::default(),
            focus: true,
            changed: is_new,
            error: None,
        });
        self.tool = Tool::Text;
        self.mask_target = false;
        self.dialog = Some(Dialog::Text);
    }

    pub(super) fn preview_text(&mut self) {
        let Some(edit) = &self.text_edit else { return };
        let mut layer = edit.original.clone();
        let style = edit.style.clone();
        let result = text::restyle_layer(self.text_renderer.as_mut().unwrap(), &mut layer, style);
        let result = result.and_then(|()| {
            let session = self.session_mut().unwrap();
            let index = session
                .document
                .layers
                .iter()
                .position(|item| item.id == layer.id)
                .unwrap();
            let old = std::mem::replace(&mut session.document.layers[index], layer);
            if let Err(error) = session.document.validate() {
                session.document.layers[index] = old;
                return Err(error);
            }
            session.invalidate();
            Ok(())
        });
        let edit = self.text_edit.as_mut().unwrap();
        edit.changed = true;
        edit.error = result.err().map(|error| error.to_string());
    }

    pub(super) fn finish_text(&mut self, apply: bool) {
        if apply
            && self
                .text_edit
                .as_ref()
                .is_some_and(|edit| edit.error.is_some())
        {
            return;
        }
        let Some(edit) = self.text_edit.take() else {
            return;
        };
        if apply && edit.changed {
            self.text_style = edit.style;
            // New text starts in a box; a path belongs to its own layer.
            self.text_style.path = None;
            self.brush.color = self.text_style.color;
            let session = self.session_mut().unwrap();
            session.document.select(edit.target, false);
            session.history.commit();
            self.status = tr("Text applied").into();
        } else if let Some(session) = self.session_mut() {
            session.history.cancel(&mut session.document);
            session.invalidate();
        }
        self.dialog = None;
    }

    pub(super) fn text_dialog(&mut self, ctx: &egui::Context) {
        let apply_shortcut =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Enter));
        let paths: Vec<String> = self
            .session()
            .map(|s| {
                s.document
                    .paths
                    .iter()
                    .map(|p| p.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let Some(edit) = &mut self.text_edit else {
            return;
        };
        let renderer = self.text_renderer.as_mut().unwrap();
        let before = edit.style.clone();
        edit.fonts
            .handle_keys(ctx, renderer.families(), &mut edit.style.family);
        let mut open = true;
        let mut apply = false;
        let mut picked = None;
        let mut cancel = false;
        widgets::Window::new(tr("Text"))
            .default_width(440.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                let response = egui::ScrollArea::vertical()
                    .id_salt("text_content_scroll")
                    .max_height(140.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut edit.style.content)
                                .id_salt("text_content")
                                .desired_width(f32::INFINITY)
                                .desired_rows(4)
                                .char_limit(text::MAX_TEXT_BYTES),
                        )
                    })
                    .inner;
                if edit.focus {
                    response.request_focus();
                    if let Some(mut state) = egui::TextEdit::load_state(ctx, response.id) {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(edit.style.content.chars().count()),
                            )));
                        state.store(ctx, response.id);
                    }
                    edit.focus = false;
                }
                ui.horizontal(|ui| {
                    ui.label(tr("Font"));
                    edit.fonts.show(ui, renderer, &mut edit.style.family);
                });
                if !renderer.has_family(&edit.style.family) {
                    ui.label(
                        RichText::new(tr(
                            "This font is unavailable. Editing uses a fallback font.",
                        ))
                        .color(ui.palette().muted)
                        .small(),
                    );
                }
                ui.horizontal(|ui| {
                    ui.label(tr("Size"));
                    ui.add(
                        widgets::Number::new(&mut edit.style.size)
                            .range(1.0..=1024.0)
                            .suffix(" px")
                            .max_decimals(1),
                    );
                    ui.add_space(12.0);
                    ui.label(tr("Color"));
                    widgets::color_well(ui, &mut edit.style.color);
                });
                ui.horizontal(|ui| {
                    widgets::checkbox(ui, &mut edit.style.bold, tr("Bold"));
                    widgets::checkbox(ui, &mut edit.style.italic, tr("Italic"));
                    widgets::checkbox(ui, &mut edit.style.underline, tr("Underline"));
                    widgets::checkbox(ui, &mut edit.style.strikethrough, tr("Strikethrough"));
                });
                ui.separator();
                picked = path_options(ui, edit, &paths);
                if let Some(error) = &edit.error {
                    ui.colored_label(ui.palette().error, error);
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tr("Live preview · Ctrl+Enter to apply"))
                            .small()
                            .color(ui.palette().muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        apply = ui
                            .add_enabled(
                                edit.error.is_none(),
                                widgets::Button::new(tr("Apply")).primary(),
                            )
                            .clicked();
                        cancel = widgets::button(ui, tr("Cancel")).clicked();
                    });
                });
            });
        if cancel || !open || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.finish_text(false);
            return;
        }
        if let Some(pick) = picked {
            self.attach_text_path(pick);
        }
        if self
            .text_edit
            .as_ref()
            .is_some_and(|edit| edit.style != before)
        {
            self.preview_text();
        }
        if apply || apply_shortcut {
            self.finish_text(true);
        }
    }

    /// Set the edited text along the document path `index`, keeping its path options, or
    /// with `None` back in a box. The path is kept in the layer's own box, so it moves with
    /// the layer from now on.
    pub(super) fn attach_text_path(&mut self, index: Option<usize>) {
        let path = index.and_then(|index| {
            self.session()
                .and_then(|s| s.document.paths.get(index))
                .map(|p| p.d.clone())
        });
        let Some(edit) = &mut self.text_edit else {
            return;
        };
        let Some(path) = path else {
            edit.style.path = None;
            return;
        };
        let options = edit
            .style
            .path
            .as_ref()
            .map(|p| p.options.clone())
            .unwrap_or_default();
        let size = edit
            .original
            .pixels
            .as_ref()
            .map_or((1, 1), |p| p.dimensions());
        match TextPath::from_document(&path, edit.original.transform, size, options) {
            Ok(path) => edit.style.path = Some(path),
            Err(error) => edit.error = Some(error.to_string()),
        }
    }
}

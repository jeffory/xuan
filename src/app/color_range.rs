//! Select → Color Range…, after Compositor's `ColorRangeSheet`: every pixel near the
//! colours clicked on the canvas, anywhere in the image. The selection updates on the
//! canvas as the colours, Fuzziness and Invert change; OK keeps it as one undo step and
//! Cancel puts back the selection there was.
use super::theme::PaletteExt as _;
use std::sync::Arc;

use egui::{Color32, RichText};
use image::{GrayImage, RgbaImage};
use uuid::Uuid;
use xuan::{document::Point, i18n::tr, render, selection_ops::ColorRange};

use super::{EditorApp, Tool, widgets};

/// The preview fits in this, in points.
const PREVIEW: egui::Vec2 = egui::vec2(292.0, 200.0);

/// What a click on the canvas does with the colour under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SampleMode {
    /// Start over from that colour.
    #[default]
    Replace,
    Add,
    Remove,
}

pub(super) struct ColorRangeEdit {
    pub document: Uuid,
    /// The image as shown when the dialog opened: what colours are matched against.
    pub image: Arc<RgbaImage>,
    /// The selection before the dialog opened, shown while no colour is picked.
    original: Option<Arc<GrayImage>>,
    pub range: ColorRange,
    pub mode: SampleMode,
    /// The range the document's selection and the preview show.
    applied: Option<ColorRange>,
    preview: Option<egui::TextureHandle>,
    previous_tool: Tool,
}

impl EditorApp {
    pub(super) fn open_color_range(&mut self) {
        if self.color_range.is_some() || self.job.is_some() {
            return;
        }
        let previous_tool = self.tool;
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.commit();
        session.history.begin(tr("Color Range"), &session.document);
        let image = Arc::new(render::render(&session.document));
        let document = session.document.id;
        let original = session.document.selection.clone();
        self.color_range = Some(ColorRangeEdit {
            document,
            image,
            original,
            range: ColorRange {
                fuzziness: self.color_range_fuzziness,
                ..ColorRange::default()
            },
            mode: SampleMode::Replace,
            applied: None,
            preview: None,
            previous_tool,
        });
        // Clicks on the canvas pick colours, with the eyedropper's cursor and bubble.
        self.set_tool(Tool::Dropper);
    }

    /// A click on the canvas while Color Range is open: Shift adds the colour and Alt
    /// takes it away, whichever eyedropper is chosen.
    pub(super) fn sample_color_range(&mut self, point: Point, modifiers: egui::Modifiers) {
        let Some(edit) = &mut self.color_range else {
            return;
        };
        let Some(color) = xuan::selection_ops::sample_color(&edit.image, point.x, point.y) else {
            return;
        };
        let mode = if modifiers.alt {
            SampleMode::Remove
        } else if modifiers.shift {
            SampleMode::Add
        } else {
            edit.mode
        };
        match mode {
            SampleMode::Replace => {
                edit.range.include = vec![color];
                edit.range.exclude.clear();
            }
            SampleMode::Add => edit.range.include.push(color),
            SampleMode::Remove => edit.range.exclude.push(color),
        }
    }

    /// Shows the edit's selection on the canvas and in the preview when it changed.
    fn update_color_range(&mut self, ctx: &egui::Context) {
        let Some(edit) = &mut self.color_range else {
            return;
        };
        if edit.applied.as_ref() == Some(&edit.range) {
            return;
        }
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|s| s.document.id == edit.document)
        else {
            return;
        };
        let mask = (!edit.range.include.is_empty()).then(|| edit.range.mask(&edit.image));
        session.document.selection = match &mask {
            Some(mask) => Some(Arc::new(mask.clone())),
            None => edit.original.clone(),
        };
        session.invalidate();
        edit.preview = mask.map(|mask| {
            ctx.load_texture(
                "color_range_preview",
                preview_image(&mask),
                egui::TextureOptions::LINEAR,
            )
        });
        edit.applied = Some(edit.range.clone());
    }

    /// Closes the dialog: OK keeps a selection made from picked colours as one undo
    /// step; Cancel, or OK before any colour was picked, puts back the old one.
    pub(super) fn close_color_range(&mut self, keep: bool) {
        let Some(edit) = self.color_range.take() else {
            return;
        };
        self.color_range_fuzziness = edit.range.fuzziness;
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|s| s.document.id == edit.document)
        {
            if keep && !edit.range.include.is_empty() {
                session.document.selection = Some(Arc::new(edit.range.mask(&edit.image)));
                session.history.commit();
                self.status = tr("Color Range").into();
            } else {
                session.history.cancel(&mut session.document);
            }
            session.invalidate();
        }
        self.set_tool(edit.previous_tool);
    }

    pub(super) fn color_range_dialog(&mut self, ctx: &egui::Context) {
        let Some(edit) = &self.color_range else {
            return;
        };
        // Switching to another tab or closing this one cancels the edit.
        if self.session().map(|s| s.document.id) != Some(edit.document) {
            self.close_color_range(false);
            return;
        }
        self.update_color_range(ctx);
        let Some(edit) = &mut self.color_range else {
            return;
        };
        let held = ctx.input(|i| {
            if i.modifiers.alt {
                Some(SampleMode::Remove)
            } else if i.modifiers.shift {
                Some(SampleMode::Add)
            } else {
                None
            }
        });
        let mut open = true;
        let mut ok = false;
        let mut cancel = false;
        let aspect = edit.image.height() as f32 / edit.image.width().max(1) as f32;
        widgets::Window::new(tr("Color Range"))
            .id("color_range")
            .open(&mut open)
            .default_width(340.0)
            // Beside the canvas, over the sidebar, so the image stays free to click.
            .default_place(
                egui::Align2::RIGHT_TOP,
                ctx.content_rect().right_top() + egui::vec2(-16.0, 90.0),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Holding Shift or Alt shows the eyedropper a click will use.
                    let mut shown = held.unwrap_or(edit.mode);
                    widgets::segmented(
                        ui,
                        &mut shown,
                        &[
                            (SampleMode::Replace, tr("Pick")),
                            (SampleMode::Add, tr("Add")),
                            (SampleMode::Remove, tr("Remove")),
                        ],
                    )
                    .on_hover_text(tr("Shift-click adds a colour, Alt-click takes one away."));
                    if held.is_none() {
                        edit.mode = shown;
                    }
                });
                ui.add_space(8.0);
                let size = if aspect > PREVIEW.y / PREVIEW.x {
                    egui::vec2(PREVIEW.y / aspect, PREVIEW.y)
                } else {
                    egui::vec2(PREVIEW.x, PREVIEW.x * aspect)
                };
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
                    if let Some(texture) = &edit.preview {
                        ui.painter().image(
                            texture.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        egui::Stroke::new(1.0_f32, Color32::from_white_alpha(50)),
                        egui::StrokeKind::Inside,
                    );
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(if edit.range.include.is_empty() {
                        tr("Click the image to pick the colour to select.")
                    } else {
                        tr("Shift-click adds a colour, Alt-click takes one away.")
                    })
                    .color(ui.palette().muted),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(tr("Fuzziness"));
                    ui.add(widgets::Slider::new(
                        &mut edit.range.fuzziness,
                        0..=ColorRange::MAX_FUZZINESS,
                    ))
                    .on_hover_text(tr(
                        "How far a colour may be from the picked ones and still be selected",
                    ));
                });
                widgets::checkbox(ui, &mut edit.range.invert, tr("Invert")).on_hover_text(tr(
                    "Select everything except those colours, such as all but a green screen",
                ));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    cancel = widgets::button(ui, tr("Cancel")).clicked();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ok = ui.add(widgets::Button::new(tr("OK")).primary()).clicked();
                    });
                });
            });
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.wants_keyboard_input();
        if ok || enter {
            self.close_color_range(true);
        } else if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_color_range(false);
        }
    }
}

/// The selection in black and white, shrunk to the preview's size at twice its
/// resolution.
fn preview_image(mask: &GrayImage) -> egui::ColorImage {
    let scale = (PREVIEW.x * 2.0 / mask.width() as f32)
        .min(PREVIEW.y * 2.0 / mask.height() as f32)
        .min(1.0);
    let width = ((mask.width() as f32 * scale).round() as usize).max(1);
    let height = ((mask.height() as f32 * scale).round() as usize).max(1);
    let small = image::imageops::resize(
        mask,
        width as u32,
        height as u32,
        image::imageops::FilterType::Triangle,
    );
    egui::ColorImage::from_gray([width, height], small.as_raw())
}

//! Navigator pane: a thumbnail of the whole document with a box showing the
//! visible canvas region, plus zoom controls.
//!
//! The view state is the session's own `zoom` and `pan`; image/screen mapping
//! comes from [`canvas::image_origin`] and zoom-about-centre from
//! [`canvas::zoom_about`]. The thumbnail is a small cached texture rebuilt a
//! moment after the document stops changing, never a per-frame render.
//!
//! It is a sidebar pane ([`xuan::panes::NAVIGATOR`]), above Layers by default;
//! the sidebar draws its header and remembers whether it is collapsed, hidden
//! or resized. Develop mode shows its own workspace without the sidebar, so
//! the pane is hidden there.
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use image::RgbaImage;
use uuid::Uuid;
use xuan::{i18n::tr, render};

use super::{EditorApp, canvas, theme};

/// Longest side, in pixels, of the cached thumbnail texture.
const THUMBNAIL_SIDE: u32 = 320;
/// Seconds without a new revision before the thumbnail is rebuilt.
const DEBOUNCE: f64 = 0.25;
/// Room the zoom controls below the thumbnail take, in points.
const CONTROLS_HEIGHT: f32 = 40.0;
/// Smallest height the thumbnail is drawn at when the pane is short.
const MIN_THUMBNAIL_HEIGHT: f32 = 24.0;
/// Zoom step of the zoom in/out buttons (matches View → Zoom In/Out).
const ZOOM_STEP: f32 = 1.25;

/// The part of the document visible in a viewport, in document pixels.
/// Not clipped to the image.
pub(super) fn visible_doc_rect(viewport: Vec2, image: Vec2, zoom: f32, pan: Vec2) -> Rect {
    // Same relation as `canvas::image_origin`, relative to the viewport centre.
    let origin = pan - image * zoom * 0.5;
    Rect::from_min_max(
        ((-viewport * 0.5 - origin) / zoom).to_pos2(),
        ((viewport * 0.5 - origin) / zoom).to_pos2(),
    )
}

/// The viewport box inside `thumb`, or `None` when no part of the image is visible.
pub(super) fn viewport_box(
    thumb: Rect,
    viewport: Vec2,
    image: Vec2,
    zoom: f32,
    pan: Vec2,
) -> Option<Rect> {
    let visible = visible_doc_rect(viewport, image, zoom, pan)
        .intersect(Rect::from_min_size(Pos2::ZERO, image));
    if visible.width() <= 0.0 || visible.height() <= 0.0 {
        return None;
    }
    let scale = thumb.size() / image;
    Some(Rect::from_min_max(
        thumb.min + visible.min.to_vec2() * scale,
        thumb.min + visible.max.to_vec2() * scale,
    ))
}

/// Document point under a thumbnail position, clamped to the image.
pub(super) fn thumb_to_doc(thumb: Rect, image: Vec2, point: Pos2) -> Pos2 {
    let doc = (point - thumb.min) * image / thumb.size();
    pos2(doc.x.clamp(0.0, image.x), doc.y.clamp(0.0, image.y))
}

/// The pan that puts document point `doc` at the viewport centre.
pub(super) fn pan_to_centre(doc: Pos2, image: Vec2, zoom: f32) -> Vec2 {
    (image * 0.5 - doc.to_vec2()) * zoom
}

/// Slider position (0..=1) of a zoom factor on a logarithmic scale.
pub(super) fn zoom_to_slider(zoom: f32) -> f32 {
    let (min, max) = (*canvas::ZOOM_LIMITS.start(), *canvas::ZOOM_LIMITS.end());
    ((zoom.clamp(min, max) / min).ln() / (max / min).ln()).clamp(0.0, 1.0)
}

/// Zoom factor of a slider position; the inverse of [`zoom_to_slider`].
pub(super) fn slider_to_zoom(position: f32) -> f32 {
    let (min, max) = (*canvas::ZOOM_LIMITS.start(), *canvas::ZOOM_LIMITS.end());
    (min * (max / min).powf(position.clamp(0.0, 1.0))).clamp(min, max)
}

/// Zoom factor typed in the percent field ("150", "12.5%"), clamped to the limits.
pub(super) fn parse_zoom_percent(text: &str) -> Option<f32> {
    let percent: f32 = text.trim().trim_end_matches('%').trim().parse().ok()?;
    percent
        .is_finite()
        .then(|| (percent / 100.0).clamp(*canvas::ZOOM_LIMITS.start(), *canvas::ZOOM_LIMITS.end()))
}

pub(super) fn format_zoom_percent(zoom: f32) -> String {
    let text = format!("{:.1}", zoom * 100.0);
    format!("{}%", text.trim_end_matches('0').trim_end_matches('.'))
}

/// Size that fits `image` into `max` keeping its aspect ratio.
pub(super) fn fit_size(image: Vec2, max: Vec2) -> Vec2 {
    let scale = (max.x / image.x).min(max.y / image.y);
    vec2((image.x * scale).max(1.0), (image.y * scale).max(1.0))
}

/// Cached thumbnail texture of one document, rebuilt (debounced) after edits.
#[derive(Default)]
pub(super) struct ThumbnailCache {
    texture: Option<egui::TextureHandle>,
    /// Document and history revision the texture shows.
    shown: Option<(Uuid, u64)>,
    /// The newest key seen, and when it was first seen.
    pending: Option<((Uuid, u64), f64)>,
    /// Thumbnails rendered so far; lets tests check the cache.
    pub renders: usize,
}

impl ThumbnailCache {
    /// Bring the texture up to date with `key`, calling `render` only when needed.
    ///
    /// The first thumbnail is made at once; later revisions wait until `key`
    /// has been stable for [`DEBOUNCE`] seconds. Returns how long to wait before
    /// the next call is useful while a rebuild is pending.
    pub fn refresh(
        &mut self,
        ctx: &egui::Context,
        key: (Uuid, u64),
        now: f64,
        render: impl FnOnce() -> RgbaImage,
    ) -> Option<f64> {
        if self.shown == Some(key) {
            self.pending = None;
            return None;
        }
        let since = match self.pending {
            Some((pending, since)) if pending == key => since,
            _ => {
                self.pending = Some((key, now));
                now
            }
        };
        let remaining = since + DEBOUNCE - now;
        if self.texture.is_some() && remaining > 0.0 {
            return Some(remaining);
        }
        let image = render();
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        );
        match &mut self.texture {
            Some(texture) => texture.set(color, egui::TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture(
                    format!("navigator-{}", key.0),
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.shown = Some(key);
        self.pending = None;
        self.renders += 1;
        None
    }
}

impl EditorApp {
    /// The body of the Navigator pane. Like the other panes it takes no input
    /// while a dialog is open.
    pub(super) fn navigator_pane(&mut self, ui: &mut egui::Ui, enabled: bool) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                if self.sessions.is_empty() {
                    ui.label(egui::RichText::new(tr("No document open")).color(theme::MUTED));
                    return;
                }
                ui.add_enabled_ui(enabled, |ui| self.navigator_body(ui));
            });
    }

    fn navigator_body(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let viewport = self.canvas_viewport;
        let session = &mut self.sessions[self.current];
        let image = vec2(
            session.document.width as f32,
            session.document.height as f32,
        );
        let key = (session.document.id, session.history.revision);
        let pixels = fit_size(image, Vec2::splat(THUMBNAIL_SIDE as f32)).round();
        let document = &session.document;
        if let Some(wait) = session
            .navigator
            .refresh(&ctx, key, ctx.input(|i| i.time), || {
                render::render_thumbnail(document, pixels.x as u32, pixels.y as u32)
            })
        {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }

        let width = ui.available_width();
        let height = (ui.available_height() - CONTROLS_HEIGHT).max(MIN_THUMBNAIL_HEIGHT);
        let size = fit_size(image, vec2(width, height));
        ui.vertical_centered(|ui| {
            let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            ui.painter().rect_filled(rect, 0.0, theme::CANVAS);
            if let Some(texture) = &session.navigator.texture {
                ui.painter().image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let Some(viewport) = viewport else { return };
            let box_rect = viewport_box(rect, viewport.size(), image, session.zoom, session.pan);
            if let Some(box_rect) = box_rect {
                ui.painter().with_clip_rect(rect).rect_stroke(
                    box_rect,
                    0.0,
                    Stroke::new(1.5_f32, theme::ACCENT),
                    StrokeKind::Inside,
                );
            }
            // Dragging the box keeps the grab point; anything else centres on the pointer.
            let grab = response.id.with("grab");
            let target = if response.drag_started()
                && let Some(press) = ui.input(|i| i.pointer.press_origin())
            {
                let offset = box_rect
                    .filter(|b| b.contains(press))
                    .map_or(Vec2::ZERO, |b| press - b.center());
                ui.data_mut(|d| d.insert_temp(grab, offset));
                response.interact_pointer_pos().map(|p| p - offset)
            } else if response.dragged() {
                let offset = ui.data(|d| d.get_temp::<Vec2>(grab)).unwrap_or(Vec2::ZERO);
                response.interact_pointer_pos().map(|p| p - offset)
            } else if response.clicked() {
                response.interact_pointer_pos()
            } else {
                None
            };
            if let Some(point) = target {
                let doc = thumb_to_doc(rect, image, point);
                session.pan = pan_to_centre(doc, image, session.zoom);
                session.fit = false;
            }
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(if response.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
        });

        ui.add_space(8.0);
        let mut zoom = session.zoom;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let id = ui.make_persistent_id("navigator_zoom_field");
            let focused = ui.memory(|m| m.has_focus(id));
            let mut text = focused
                .then(|| ui.data(|d| d.get_temp::<String>(id)))
                .flatten()
                .unwrap_or_else(|| format_zoom_percent(zoom));
            let field = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .id(id)
                    .desired_width(48.0),
            );
            if field.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, text.clone()));
            }
            if field.lost_focus() {
                ui.data_mut(|d| d.remove_temp::<String>(id));
                if !ui.input(|i| i.key_pressed(egui::Key::Escape))
                    && let Some(typed) = parse_zoom_percent(&text)
                {
                    zoom = typed;
                }
            }
            if ui.small_button("−").on_hover_text(tr("Zoom out")).clicked() {
                zoom = (zoom / ZOOM_STEP).max(*canvas::ZOOM_LIMITS.start());
            }
            let mut position = zoom_to_slider(zoom);
            ui.spacing_mut().slider_width = (ui.available_width() - 34.0).max(30.0);
            if ui
                .add(egui::Slider::new(&mut position, 0.0..=1.0).show_value(false))
                .changed()
            {
                zoom = slider_to_zoom(position);
            }
            if ui.small_button("+").on_hover_text(tr("Zoom in")).clicked() {
                zoom = (zoom * ZOOM_STEP).min(*canvas::ZOOM_LIMITS.end());
            }
        });
        if zoom != session.zoom {
            canvas::zoom_about(&mut session.zoom, &mut session.pan, zoom, Vec2::ZERO);
            session.fit = false;
        }
        self.navigator_view = viewport.map(|v| (v, session.zoom, session.pan));
    }
}

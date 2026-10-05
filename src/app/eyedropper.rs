//! Eyedropper: live colour bubble, click-and-drag sampling and the sampling cache.
//!
//! Dragging updates the brush colour live (as Photoshop does) and remembers the
//! colour from before the gesture, so Esc can restore it. Layers with filters
//! need a full render to sample; that render is cached per session and reused
//! until the document changes, so a drag costs one render, not one per frame.
use std::sync::Arc;

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};
use image::RgbaImage;
use xuan::{document::Point, render};

use super::EditorApp;

/// Bubble edge length in points.
pub(super) const BUBBLE_SIZE: Vec2 = vec2(44.0, 44.0);
/// Gap between the pointer and the bubble, so the sampled pixel stays visible.
const BUBBLE_GAP: f32 = 14.0;

/// How many pixels around the pointer are averaged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SampleSize {
    #[default]
    Point,
    Three,
    Five,
}

impl SampleSize {
    fn radius(self) -> i32 {
        match self {
            Self::Point => 0,
            Self::Three => 1,
            Self::Five => 2,
        }
    }
}

/// Which layers the eyedropper reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SampleSource {
    CurrentLayer,
    AllLayers,
}

/// An eyedropper press that has not been released yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DropperGesture {
    /// Brush colour before the gesture; restored by Esc.
    pub previous: [u8; 4],
}

/// A full render of a session, valid for one history revision and source.
pub(super) struct SampleCache {
    revision: u64,
    source: SampleSource,
    image: Arc<RgbaImage>,
}

/// Bubble rectangle for a pointer at `pointer`, kept inside `bounds`.
///
/// Prefers up and to the right of the pointer; flips to the left or below when
/// that would leave `bounds`, and is finally clamped if it still does not fit.
pub(super) fn bubble_rect(pointer: Pos2, size: Vec2, bounds: Rect) -> Rect {
    let mut x = pointer.x + BUBBLE_GAP;
    if x + size.x > bounds.right() {
        x = pointer.x - BUBBLE_GAP - size.x;
    }
    let mut y = pointer.y - BUBBLE_GAP - size.y;
    if y < bounds.top() {
        y = pointer.y + BUBBLE_GAP;
    }
    let max = pos2(
        (bounds.right() - size.x).max(bounds.left()),
        (bounds.bottom() - size.y).max(bounds.top()),
    );
    Rect::from_min_size(
        pos2(x.clamp(bounds.left(), max.x), y.clamp(bounds.top(), max.y)),
        size,
    )
}

/// Alpha-weighted mean of straight-alpha RGBA samples.
fn average(samples: &[[f32; 4]]) -> [f32; 4] {
    let mut sum = [0.0_f32; 4];
    for s in samples {
        sum[0] += s[0] * s[3];
        sum[1] += s[1] * s[3];
        sum[2] += s[2] * s[3];
        sum[3] += s[3];
    }
    if sum[3] <= 0.0 {
        return [0.0; 4];
    }
    let n = samples.len() as f32;
    [
        sum[0] / sum[3],
        sum[1] / sum[3],
        sum[2] / sum[3],
        sum[3] / n,
    ]
}

/// Paint the bubble: new colour on the top half, current colour below it, with a
/// dark ring inside a light one so it reads on light and dark images.
pub(super) fn paint_bubble(painter: &Painter, rect: Rect, new: [u8; 4], current: [u8; 4]) {
    let radius = 8_u8;
    let split = rect.center().y;
    let top = Rect::from_min_max(rect.min, pos2(rect.right(), split));
    let bottom = Rect::from_min_max(pos2(rect.left(), split), rect.max);
    let half = |painter: &Painter, r: Rect, c: [u8; 4], round: CornerRadius| {
        // Opaque colour, so the bubble shows what the brush will become.
        painter.rect_filled(r, round, Color32::from_rgb(c[0], c[1], c[2]));
    };
    half(
        painter,
        top,
        new,
        CornerRadius {
            nw: radius,
            ne: radius,
            sw: 0,
            se: 0,
        },
    );
    half(
        painter,
        bottom,
        current,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: radius,
            se: radius,
        },
    );
    painter.rect_stroke(
        rect.expand(1.0),
        radius,
        Stroke::new(2.0_f32, Color32::from_black_alpha(220)),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.5_f32, Color32::WHITE),
        StrokeKind::Inside,
    );
    painter.line_segment(
        [pos2(rect.left(), split), pos2(rect.right(), split)],
        Stroke::new(1.0_f32, Color32::from_gray(128)),
    );
}

impl EditorApp {
    /// Colour under `point` for the current sample size and source, or `None`
    /// outside the document.
    pub(super) fn sample_color(&mut self, point: Point) -> Option<[u8; 4]> {
        let radius = self.dropper_size.radius();
        let source = self.dropper_source;
        let session = self.sessions.get_mut(self.current)?;
        let (width, height) = (session.document.width, session.document.height);
        let (cx, cy) = (point.x.floor() as i32, point.y.floor() as i32);
        if cx < 0 || cy < 0 || cx >= width as i32 || cy >= height as i32 {
            return None;
        }
        let needs_render = source == SampleSource::CurrentLayer
            || session.document.layers.iter().any(|l| l.filter.is_some());
        let image = needs_render.then(|| session.sample_image(source));
        let mut samples = Vec::new();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                    continue;
                }
                samples.push(match &image {
                    Some(image) => image
                        .get_pixel(x as u32, y as u32)
                        .0
                        .map(|v| v as f32 / 255.0),
                    None => render::pixel_at(
                        &session.document,
                        Point::new(x as f32 + 0.5, y as f32 + 0.5),
                    ),
                });
            }
        }
        Some(average(&samples).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
    }

    /// Free cached renders that cannot be used soon: all of them unless the
    /// Eyedropper is active, and always those of background tabs.
    pub(super) fn release_sample_caches(&mut self) {
        let active = self.tool == super::Tool::Dropper;
        for (index, session) in self.sessions.iter_mut().enumerate() {
            if !active || index != self.current {
                session.sample_cache = None;
            }
        }
    }

    /// Pointer pressed: remember the brush colour and sample.
    pub(super) fn dropper_press(&mut self, point: Point) {
        self.dropper = Some(DropperGesture {
            previous: self.brush.color,
        });
        self.dropper_update(point);
    }

    /// Pointer dragged: apply the colour live. Outside the document keeps the last one.
    pub(super) fn dropper_update(&mut self, point: Point) {
        if self.dropper.is_some()
            && let Some(color) = self.sample_color(point)
        {
            self.brush.color = color;
        }
    }

    /// Pointer released: the live colour becomes final.
    pub(super) fn dropper_release(&mut self) {
        self.dropper = None;
    }

    /// Esc: put back the colour from before the press.
    pub(super) fn dropper_cancel(&mut self) {
        if let Some(gesture) = self.dropper.take() {
            self.brush.color = gesture.previous;
        }
    }
}

impl super::Session {
    /// Full render for the given source, reused until the document changes.
    fn sample_image(&mut self, source: SampleSource) -> Arc<RgbaImage> {
        let revision = self.history.revision;
        if let Some(cache) = &self.sample_cache
            && cache.revision == revision
            && cache.source == source
        {
            return cache.image.clone();
        }
        let image = Arc::new(match source {
            SampleSource::AllLayers => render::render(&self.document),
            SampleSource::CurrentLayer => {
                let mut isolated = self.document.clone();
                let id = isolated.active;
                for layer in &mut isolated.layers {
                    layer.visible = Some(layer.id) == id || layer.group;
                }
                render::render(&isolated)
            }
        });
        self.sample_renders += 1;
        self.sample_cache = Some(SampleCache {
            revision,
            source,
            image: image.clone(),
        });
        image
    }
}

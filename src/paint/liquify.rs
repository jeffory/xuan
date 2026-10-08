//! Liquify (Forward Warp), a mode of the Blur / Smudge tool: the pixels under
//! the brush move with it, most at the centre and fading to none at the rim.
//!
//! Each step of the brush from `from` to `to` resamples the layer through a
//! displacement field centred on `to`: a pixel there takes the colour from
//! `strength × falloff × (to − from)` behind it, so at full strength the
//! pixel under the brush's start lands under its end. Long steps are cut
//! into short ones, so the field never folds over itself and the warp can't
//! tear or leave holes. Samples beyond the layer's edge take the nearest
//! edge pixel.

use std::sync::Arc;

use image::{GrayImage, RgbaImage};
use rayon::prelude::*;

use super::{Brush, brush_distance};
use crate::{
    document::{Layer, Point, Transform},
    selection,
};

/// The hardest edge the warp uses. A harder brush would need a step too
/// fine to keep the field from folding at the rim.
pub const MAX_HARDNESS: f32 = 0.8;

/// The most sub-steps one segment is cut into, bounding the work for a
/// very long jump of the pointer with a large brush.
pub const MAX_STEPS: usize = 32;

/// How much of the brush's displacement reaches a point at `distance` from
/// the centre, measured in radii: 1 inside `hardness`, easing smoothly to 0
/// at the rim and beyond.
pub fn falloff(distance: f32, hardness: f32) -> f32 {
    let hardness = hardness.clamp(0.0, MAX_HARDNESS);
    if !distance.is_finite() || distance >= 1.0 {
        return 0.0;
    }
    if distance <= hardness {
        return 1.0;
    }
    let t = (distance - hardness) / (1.0 - hardness);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// One dab of the warp: the brush centred at `center` has just moved by
/// `delta`, with `radius`, `hardness`, `strength` and pen `tilt`.
#[derive(Clone, Copy, Debug)]
pub struct Dab {
    pub center: Point,
    pub delta: Point,
    pub radius: f32,
    pub hardness: f32,
    pub strength: f32,
    pub tilt: [f32; 2],
}

impl Dab {
    /// How far back the colour that lands on `point` comes from, in
    /// document units.
    pub fn displacement(&self, point: Point) -> Point {
        let offset = Point::new(point.x - self.center.x, point.y - self.center.y);
        let weight = falloff(
            brush_distance(offset, self.radius, self.tilt),
            self.hardness,
        ) * self.strength.clamp(0.0, 1.0);
        Point::new(self.delta.x * weight, self.delta.y * weight)
    }
}

/// The longest step the brush may move in one dab without the field
/// folding: the falloff's steepest slope is 1.5 / (radius × (1 − hardness)),
/// and the step keeps the displacement's slope at a half or less.
pub fn max_step(radius: f32, hardness: f32, strength: f32) -> f32 {
    let hardness = hardness.clamp(0.0, MAX_HARDNESS);
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.0 {
        return f32::INFINITY;
    }
    (radius.max(0.5) * (1.0 - hardness) / (3.0 * strength)).max(0.5)
}

/// The dabs a segment from `from` to `to` is cut into, the brush's size,
/// hardness, strength (opacity) and tilt eased from `from_brush` to `brush`.
pub fn dabs(from: Point, to: Point, from_brush: &Brush, brush: &Brush) -> Vec<Dab> {
    let values = [from.x, from.y, to.x, to.y];
    if values.iter().any(|v| !v.is_finite()) {
        return Vec::new();
    }
    let length = from.distance(to);
    let radius = |b: &Brush| (b.diameter * 0.5).clamp(0.5, 100_000.0);
    let strength = |b: &Brush| {
        if b.opacity.is_finite() {
            b.opacity.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    if length <= 0.0 || strength(from_brush).max(strength(brush)) <= 0.0 {
        return Vec::new();
    }
    let step = max_step(
        radius(from_brush).min(radius(brush)),
        from_brush.hardness.max(brush.hardness),
        strength(from_brush).max(strength(brush)),
    );
    let count = ((length / step).ceil() as usize).clamp(1, MAX_STEPS);
    let delta = Point::new(
        (to.x - from.x) / count as f32,
        (to.y - from.y) / count as f32,
    );
    (1..=count)
        .map(|i| {
            let t = i as f32 / count as f32;
            let mix = |a: f32, b: f32| a + (b - a) * t;
            Dab {
                center: Point::new(mix(from.x, to.x), mix(from.y, to.y)),
                delta,
                radius: mix(radius(from_brush), radius(brush)),
                hardness: mix(from_brush.hardness, brush.hardness),
                strength: mix(strength(from_brush), strength(brush)),
                tilt: [0, 1].map(|axis| mix(from_brush.tilt[axis], brush.tilt[axis])),
            }
        })
        .collect()
}

/// Warp the layer's pixels by `dabs` in turn, inside the selection.
pub(super) fn segment(layer: &mut Layer, selection: Option<&GrayImage>, dabs: &[Dab]) {
    let transform = layer.transform;
    let Some(pixels) = layer.pixels.as_mut() else {
        return;
    };
    for dab in dabs {
        warp(pixels, transform, selection, dab);
    }
}

/// The layer pixels `[left, top, right, bottom)` that the document-space box
/// around `center` reaches, `reach` document units each way.
fn local_bounds(
    transform: Transform,
    (width, height): (u32, u32),
    center: Point,
    reach: f32,
) -> Option<[u32; 4]> {
    let min = Point::new(center.x - reach, center.y - reach);
    let max = Point::new(center.x + reach, center.y + reach);
    let local = [min, Point::new(max.x, min.y), max, Point::new(min.x, max.y)]
        .map(|p| transform.inverse(p));
    let fold = |pick: fn(&Point) -> f32, scale: u32, f: fn(f32, f32) -> f32, start: f32| {
        local.iter().map(pick).fold(start, f) * scale as f32
    };
    let left = fold(|p| p.x, width, f32::min, f32::MAX).floor() - 1.0;
    let right = fold(|p| p.x, width, f32::max, f32::MIN).ceil() + 1.0;
    let top = fold(|p| p.y, height, f32::min, f32::MAX).floor() - 1.0;
    let bottom = fold(|p| p.y, height, f32::max, f32::MIN).ceil() + 1.0;
    if ![left, right, top, bottom].iter().all(|v| v.is_finite()) {
        return None;
    }
    let bounds = [
        left.clamp(0.0, width as f32) as u32,
        top.clamp(0.0, height as f32) as u32,
        right.clamp(0.0, width as f32) as u32,
        bottom.clamp(0.0, height as f32) as u32,
    ];
    (bounds[2] > bounds[0] && bounds[3] > bounds[1]).then_some(bounds)
}

/// A copy of part of the layer, sampled with bilinear filtering on
/// premultiplied colour, clamped to the layer's edge.
struct Snapshot {
    bounds: [u32; 4],
    size: (u32, u32),
    pixels: Vec<u8>,
}

impl Snapshot {
    fn new(image: &RgbaImage, bounds: [u32; 4]) -> Self {
        let [left, top, right, bottom] = bounds;
        let row = (right - left) as usize * 4;
        let stride = image.width() as usize * 4;
        let mut pixels = Vec::with_capacity(row * (bottom - top) as usize);
        for y in top as usize..bottom as usize {
            let start = y * stride + left as usize * 4;
            pixels.extend_from_slice(&image.as_raw()[start..start + row]);
        }
        Self {
            bounds,
            size: image.dimensions(),
            pixels,
        }
    }

    /// The premultiplied colour at layer pixel coordinates `x`, `y`, where
    /// pixel centres are at half units.
    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let [left, top, right, bottom] = self.bounds;
        let clamp = |v: f32, size: u32, low: u32, high: u32| {
            (v - 0.5)
                .clamp(0.0, (size - 1) as f32)
                .clamp(low as f32, (high - 1) as f32)
        };
        let x = clamp(x, self.size.0, left, right);
        let y = clamp(y, self.size.1, top, bottom);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(right - 1), (y0 + 1).min(bottom - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let stride = (right - left) as usize;
        let at = |x: u32, y: u32| {
            let index = ((y - top) as usize * stride + (x - left) as usize) * 4;
            let p: [f32; 4] = std::array::from_fn(|i| f32::from(self.pixels[index + i]) / 255.0);
            [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|i| {
            let upper = a[i] + (b[i] - a[i]) * fx;
            let lower = c[i] + (d[i] - c[i]) * fx;
            upper + (lower - upper) * fy
        })
    }
}

/// Apply one dab to the layer's pixels.
fn warp(
    pixels: &mut Arc<RgbaImage>,
    transform: Transform,
    selection: Option<&GrayImage>,
    dab: &Dab,
) {
    let size = pixels.dimensions();
    let (width, height) = size;
    if width == 0 || height == 0 || dab.strength <= 0.0 {
        return;
    }
    // Pen tilt stretches the brush along its axis.
    let reach = dab.radius / super::tilt_shape(dab.tilt).1 + 1.0;
    let Some([left, top, right, bottom]) = local_bounds(transform, size, dab.center, reach) else {
        return;
    };
    let shift = dab.delta.x.hypot(dab.delta.y) * dab.strength;
    let Some(source) = local_bounds(transform, size, dab.center, reach + shift + 1.0) else {
        return;
    };
    let snapshot = Snapshot::new(pixels, source);
    let image = Arc::make_mut(pixels);
    let row_bytes = width as usize * 4;
    let rows = &mut image.as_mut()[top as usize * row_bytes..bottom as usize * row_bytes];
    rows.par_chunks_mut(row_bytes)
        .enumerate()
        .for_each(|(index, row)| {
            let y = top + index as u32;
            for x in left..right {
                let point = transform.point(Point::new(
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                ));
                let shift = dab.displacement(point);
                if shift.x == 0.0 && shift.y == 0.0 {
                    continue;
                }
                let amount = selection::coverage(selection, point);
                if amount <= 0.0 {
                    continue;
                }
                let local = transform.inverse(Point::new(point.x - shift.x, point.y - shift.y));
                if !local.x.is_finite() || !local.y.is_finite() {
                    continue;
                }
                let moved = snapshot.sample(local.x * width as f32, local.y * height as f32);
                let pixel = &mut row[x as usize * 4..x as usize * 4 + 4];
                let old = snapshot.sample(x as f32 + 0.5, y as f32 + 0.5);
                let mixed: [f32; 4] =
                    std::array::from_fn(|i| old[i] + (moved[i] - old[i]) * amount);
                let alpha = mixed[3];
                if alpha < 0.5 / 255.0 {
                    // Keep the colour of pixels that were invisible already.
                    if pixel[3] != 0 {
                        pixel.copy_from_slice(&[0; 4]);
                    }
                    continue;
                }
                for i in 0..3 {
                    pixel[i] = ((mixed[i] / alpha).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
                pixel[3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
}

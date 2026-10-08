//! Edit → Stroke…: a line of colour along the selection's edge, painted on the active pixel
//! layer, after Photoshop's Stroke dialog. [`selection_ops::stroke_coverage`] makes the line
//! as coverage on the canvas; [`paint`] lays it on the layer through the layer's transform,
//! as Fill lays the selection.

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context as _, Result};
use image::GrayImage;
use rayon::prelude::*;

use super::{ensure_pixels, expand_stroke_bounds};
use crate::{
    blend::{BlendMode, composite},
    document::{Document, Point},
    selection,
    selection_ops::{self, StrokeLocation},
};

/// Edit → Stroke…'s settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    /// 1–[`selection_ops::MAX_STROKE_WIDTH`] canvas pixels.
    pub width: u32,
    pub location: StrokeLocation,
    pub color: [u8; 3],
    /// 0–1.
    pub opacity: f32,
    /// Paint only over what the layer already shows and keep its alpha, as Photoshop's
    /// Preserve Transparency does.
    pub preserve_transparency: bool,
}

/// Coverage below half a level would round away, so it leaves a pixel untouched (and a
/// transparent pixel's colour as it was).
const VISIBLE: f32 = 0.5 / 255.0;

/// Strokes the document's selection on its active layer. Fails without a selection or a
/// layer that can be painted, and when cancelled.
pub fn apply(document: &mut Document, outline: &Outline, cancel: &AtomicBool) -> Result<()> {
    let selection = document.selection.clone().context("Select an area first")?;
    let coverage =
        selection_ops::stroke_coverage(&selection, outline.width, outline.location, cancel)
            .context("Cancelled")?;
    paint(document, &coverage, outline)
}

/// Paints `coverage` (canvas-sized, from [`selection_ops::stroke_coverage`]) on the active
/// layer in the outline's colour and opacity. The layer's pixels are mapped to the canvas by
/// its transform and the coverage sampled between pixels, so a moved, scaled or rotated
/// layer takes the line where it shows on the canvas. Without Preserve Transparency the
/// layer grows to hold all of the line, as a brush stroke past its edge does.
pub fn paint(document: &mut Document, coverage: &GrayImage, outline: &Outline) -> Result<()> {
    let layer = document.active_mut().context("Select a layer first")?;
    ensure_pixels(layer)?;
    let opacity = outline.opacity.clamp(0.0, 1.0);
    let Some((left, top, right, bottom)) = selection::bounds(coverage).filter(|_| opacity > 0.0)
    else {
        return Ok(());
    };
    if !outline.preserve_transparency {
        expand_stroke_bounds(
            layer,
            Point::new(left as f32, top as f32),
            Point::new(right as f32, bottom as f32),
            0.0,
        )?;
    }
    let transform = layer.transform;
    let pixels = Arc::make_mut(layer.pixels.as_mut().unwrap());
    let (width, height) = pixels.dimensions();
    let color = outline.color.map(|v| f32::from(v) / 255.0);
    let preserve = outline.preserve_transparency;
    pixels
        .as_mut()
        .par_chunks_exact_mut(width as usize * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let point = transform.point(Point::new(
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                ));
                let amount = sample(coverage, point) * opacity;
                if amount < VISIBLE || (preserve && pixel[3] == 0) {
                    continue;
                }
                let old = pixel.map(|v| f32::from(v) / 255.0);
                let new = if preserve {
                    let mix = |c: usize| old[c] + (color[c] - old[c]) * amount;
                    [mix(0), mix(1), mix(2), old[3]]
                } else {
                    composite(
                        old,
                        [color[0], color[1], color[2], amount],
                        BlendMode::Normal,
                    )
                };
                for (target, value) in pixel.iter_mut().zip(new) {
                    *target = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        });
    Ok(())
}

/// `coverage` at a canvas point, from 0 to 1, between the four nearest pixel centres;
/// nothing outside the canvas.
fn sample(coverage: &GrayImage, point: Point) -> f32 {
    let (width, height) = (coverage.width() as i64, coverage.height() as i64);
    let (x, y) = (point.x - 0.5, point.y - 0.5);
    let (left, top) = (x.floor(), y.floor());
    let (fx, fy) = (x - left, y - top);
    let at = |px: i64, py: i64| {
        if px < 0 || py < 0 || px >= width || py >= height {
            0.0
        } else {
            f32::from(coverage.as_raw()[(py * width + px) as usize]) / 255.0
        }
    };
    let (left, top) = (left as i64, top as i64);
    let upper = at(left, top) * (1.0 - fx) + at(left + 1, top) * fx;
    let lower = at(left, top + 1) * (1.0 - fx) + at(left + 1, top + 1) * fx;
    upper * (1.0 - fy) + lower * fy
}

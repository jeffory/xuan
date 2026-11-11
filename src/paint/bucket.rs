//! The Paint Bucket: fills the area around a clicked pixel whose colour is within a
//! tolerance of it. The area is the Magic Wand's ([`selection::wand`]); anti-aliasing
//! then reaches one pixel past its edge, partly covering the blended pixels of a line
//! so no ring of the old colour is left between the fill and the line.

use std::sync::Arc;

use anyhow::Result;
use image::{GrayImage, RgbaImage};

use super::{ensure_pixels, prepare_mask, selection};
use crate::{
    blend::{BlendMode, composite},
    document::{Document, Point, Transform},
    render,
};

/// How the Paint Bucket fills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BucketOptions {
    /// The fill colour, the foreground colour in the editor.
    pub color: [u8; 4],
    pub opacity: f32,
    /// How far, per channel, a pixel's colour may be from the clicked one.
    pub tolerance: u8,
    /// Fill only the area connected to the clicked pixel; off, every matching pixel.
    pub contiguous: bool,
    /// Partly fill the pixels along the area's edge.
    pub anti_alias: bool,
    /// Compare colours in the visible composition instead of the target layer.
    pub all_layers: bool,
    /// Paint the active layer's mask instead of its pixels.
    pub mask_target: bool,
}

impl Default for BucketOptions {
    fn default() -> Self {
        Self {
            color: [0, 0, 0, 255],
            opacity: 1.0,
            tolerance: 32,
            contiguous: true,
            anti_alias: true,
            all_layers: false,
            mask_target: false,
        }
    }
}

/// How far `pixel` is from the clicked colour `seed`: the largest channel difference.
/// A transparent pixel's colour channels carry nothing, so against a transparent seed
/// only alpha counts.
fn distance(seed: [u8; 4], pixel: [u8; 4]) -> u8 {
    if seed[3] == 0 {
        return pixel[3];
    }
    (0..4)
        .map(|i| seed[i].abs_diff(pixel[i]))
        .max()
        .unwrap_or(0)
}

/// How much of each pixel of `image` the bucket fills when clicked at pixel `seed`
/// (0 none, 255 fully). The area is [`selection::wand`]'s. With `anti_alias`, pixels
/// outside it that touch it (also diagonally) are covered by how close their colour
/// is: fully at `tolerance`, fading to nothing for the opposite colour.
pub fn bucket_coverage(
    image: &RgbaImage,
    seed: Point,
    tolerance: u8,
    contiguous: bool,
    anti_alias: bool,
) -> GrayImage {
    let mut coverage = selection::wand(image, seed, tolerance, contiguous);
    if !anti_alias || !coverage.as_raw().contains(&255) {
        return coverage;
    }
    let color = image.get_pixel(seed.x as u32, seed.y as u32).0;
    let (width, height) = image.dimensions();
    let region = coverage.clone();
    let inside = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && x < i64::from(width)
            && y < i64::from(height)
            && region.get_pixel(x as u32, y as u32)[0] == 255
    };
    let range = 255.0 - f32::from(tolerance);
    for (x, y, pixel) in coverage.enumerate_pixels_mut() {
        if pixel[0] == 255 {
            continue;
        }
        let (x, y) = (i64::from(x), i64::from(y));
        let touches = (-1..=1).any(|dy| (-1..=1).any(|dx| inside(x + dx, y + dy)));
        if !touches {
            continue;
        }
        let far = f32::from(distance(color, image.get_pixel(x as u32, y as u32).0));
        let amount = if range <= 0.0 {
            1.0
        } else {
            ((255.0 - far) / range).clamp(0.0, 1.0)
        };
        pixel[0] = (amount * 255.0).round() as u8;
    }
    coverage
}

/// `source` (canvas sized) as seen through `transform` on a `width` × `height` grid:
/// each grid pixel takes the canvas pixel under its centre, transparent off the canvas.
fn resample(source: &RgbaImage, transform: Transform, width: u32, height: u32) -> RgbaImage {
    let (sw, sh) = source.dimensions();
    RgbaImage::from_fn(width, height, |x, y| {
        let point = transform.point(Point::new(
            (x as f32 + 0.5) / width as f32,
            (y as f32 + 0.5) / height as f32,
        ));
        if point.x < 0.0 || point.y < 0.0 || point.x >= sw as f32 || point.y >= sh as f32 {
            image::Rgba([0; 4])
        } else {
            *source.get_pixel(point.x as u32, point.y as u32)
        }
    })
}

/// Fills the active layer, or its mask, around the canvas `point` (see
/// [`BucketOptions`]), inside the selection. The colours compared are the target's
/// own (a mask's greys when painting the mask) or, with `all_layers`, the visible
/// composition's. Returns whether anything was filled: a click off the sampled
/// pixels fills nothing. Refuses locked layers like the other painting tools.
pub fn bucket(document: &mut Document, point: Point, options: BucketOptions) -> Result<bool> {
    let BucketOptions {
        color,
        opacity,
        tolerance,
        contiguous,
        anti_alias,
        all_layers,
        mask_target,
    } = options;
    let composition = all_layers.then(|| render::render(document));
    let selection = document.selection.clone();
    let layer = document
        .active_mut()
        .ok_or_else(|| anyhow::anyhow!("Select a layer first"))?;
    let (transform, (width, height)) = if mask_target {
        prepare_mask(layer)?;
        let mask = layer.mask.as_ref().unwrap();
        (
            mask.placement.unwrap_or(layer.transform),
            mask.pixels.dimensions(),
        )
    } else {
        ensure_pixels(layer)?;
        (layer.transform, layer.pixels.as_ref().unwrap().dimensions())
    };
    let sample = match &composition {
        Some(composition) => resample(composition, transform, width, height),
        None if mask_target => {
            let mask = &layer.mask.as_ref().unwrap().pixels;
            RgbaImage::from_fn(width, height, |x, y| {
                let value = mask.get_pixel(x, y)[0];
                image::Rgba([value, value, value, 255])
            })
        }
        None => layer.pixels.as_deref().unwrap().clone(),
    };
    let unit = transform.inverse(point);
    let seed = Point::new(unit.x * width as f32, unit.y * height as f32);
    if !(seed.x >= 0.0 && seed.y >= 0.0 && seed.x < width as f32 && seed.y < height as f32) {
        return Ok(false);
    }
    let coverage = bucket_coverage(&sample, seed, tolerance, contiguous, anti_alias);
    let amount_at = |x: u32, y: u32| {
        let covered = coverage.get_pixel(x, y)[0];
        if covered == 0 {
            return 0.0;
        }
        let point = transform.point(Point::new(
            (x as f32 + 0.5) / width as f32,
            (y as f32 + 0.5) / height as f32,
        ));
        f32::from(covered) / 255.0 * opacity * selection::coverage(selection.as_deref(), point)
    };
    let mut filled = false;
    if mask_target {
        let target =
            f32::from(color[0]) * 0.3 + f32::from(color[1]) * 0.59 + f32::from(color[2]) * 0.11;
        let pixels = Arc::make_mut(&mut layer.mask.as_mut().unwrap().pixels);
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            let amount = amount_at(x, y);
            if amount > 0.0 {
                filled = true;
                pixel[0] = (f32::from(pixel[0]) * (1.0 - amount) + target * amount).round() as u8;
            }
        }
    } else {
        let pixels = Arc::make_mut(layer.pixels.as_mut().unwrap());
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            let amount = amount_at(x, y);
            if amount > 0.0 {
                filled = true;
                let mut source = color.map(|v| f32::from(v) / 255.0);
                source[3] *= amount;
                pixel.0 = composite(
                    pixel.0.map(|v| f32::from(v) / 255.0),
                    source,
                    BlendMode::Normal,
                )
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    Ok(filled)
}

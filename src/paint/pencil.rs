//! The pencil: hard-edged dabs that cover exactly the pixels whose centres fall inside
//! the tip. Segments are rasterized with Bresenham's line so no pixel is skipped, and
//! the per-stroke coverage mask keeps overlapping dabs from applying twice.

use std::sync::Arc;

use image::GrayImage;

use super::{Brush, Layer, Point, Stroke, selection};
use crate::{
    blend::{BlendMode, composite},
    document::Transform,
};

pub(super) struct Segment<'a> {
    pub mask_target: bool,
    /// Layer pixel region `[left, top, right, bottom]` the segment may touch.
    pub bounds: [u32; 4],
    pub transform: Transform,
    pub endpoints: [Point; 2],
    pub brushes: [&'a Brush; 2],
}

/// Pencil size in whole pixels.
fn size_of(diameter: f32) -> u32 {
    diameter.round().max(1.0) as u32
}

/// The point on the lattice a pencil of this diameter snaps it to: a pixel
/// centre for odd sizes, a pixel corner for even ones. Snapping a snapped
/// point again keeps it, so symmetric copies snap before they are mirrored
/// and mirror onto whole pixels exactly.
pub(super) fn snapped(point: Point, diameter: f32) -> Point {
    if size_of(diameter) % 2 == 1 {
        Point::new(point.x.floor() + 0.5, point.y.floor() + 0.5)
    } else {
        Point::new(point.x.round(), point.y.round())
    }
}

/// Whether a pixel whose centre is `offset` from the dab centre lies inside the tip.
fn in_tip(offset: Point, size: u32, square: bool) -> bool {
    let half = size as f32 * 0.5;
    if square {
        offset.x.abs() < half && offset.y.abs() < half
    } else {
        offset.x * offset.x + offset.y * offset.y < half * half
    }
}

/// Pixel offsets `(dx, dy)` covered by one dab, relative to its anchor pixel. Odd sizes
/// centre on the anchor pixel; even sizes centre on the anchor's top-left corner.
pub fn tip_offsets(size: u32, square: bool) -> Vec<(i32, i32)> {
    let size = size.max(1);
    let shift = if size % 2 == 1 { 0.5 } else { 0.0 };
    let reach = size as i32 / 2 + 1;
    let mut offsets = Vec::new();
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let centre = Point::new(dx as f32 + 0.5 - shift, dy as f32 + 0.5 - shift);
            if in_tip(centre, size, square) {
                offsets.push((dx, dy));
            }
        }
    }
    offsets
}

/// Integer points of the Bresenham line between two lattice points, both included.
fn bresenham(from: (i32, i32), to: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut y) = from;
    let dx = (to.0 - x).abs();
    let dy = -(to.1 - y).abs();
    let sx = if x < to.0 { 1 } else { -1 };
    let sy = if y < to.1 { 1 } else { -1 };
    let mut error = dx + dy;
    let mut points = Vec::with_capacity(dx.max(-dy) as usize + 1);
    loop {
        points.push((x, y));
        if (x, y) == to {
            return points;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
}

pub(super) fn segment(
    layer: &mut Layer,
    selection: Option<&GrayImage>,
    coverage: &mut Stroke,
    segment: Segment<'_>,
) {
    let Segment {
        mask_target,
        bounds: [left, top, right, bottom],
        transform,
        endpoints: [from, to],
        brushes: [from_brush, brush],
    } = segment;
    let (width, height) = if mask_target {
        layer.mask.as_ref().unwrap().pixels.dimensions()
    } else {
        layer.pixels.as_ref().unwrap().dimensions()
    };
    // Odd sizes snap to pixel centres and even sizes to pixel corners.
    let odd_lattice = size_of(brush.diameter) % 2 == 1;
    let snap = |p: Point| {
        if odd_lattice {
            (p.x.floor() as i32, p.y.floor() as i32)
        } else {
            (p.x.round() as i32, p.y.round() as i32)
        }
    };
    let line = bresenham(snap(from), snap(to));
    let last = (line.len() - 1).max(1) as f32;
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    for (index, (ax, ay)) in line.into_iter().enumerate() {
        let t = index as f32 / last;
        let size = size_of(lerp(from_brush.diameter, brush.diameter, t));
        let opacity = lerp(from_brush.opacity, brush.opacity, t);
        let shift = if size % 2 == 1 { 0.5 } else { 0.0 };
        // A mirrored symmetric copy puts a dab whose size parity differs
        // from the lattice's on the mirrored side of its anchor, so it
        // lands where the drawn dab's mirror image does.
        let place = |anchor: i32, flipped: bool| {
            if flipped {
                anchor as f32 + if odd_lattice { 1.0 } else { 0.0 } - shift
            } else {
                anchor as f32 + shift
            }
        };
        let flip = coverage.flip;
        let centre = Point::new(place(ax, flip[0]), place(ay, flip[1]));
        let half = size as f32 * 0.5;
        let corners = [(-half, -half), (half, -half), (half, half), (-half, half)]
            .map(|(x, y)| transform.inverse(Point::new(centre.x + x, centre.y + y)));
        let range = |axis: fn(&Point) -> f32, extent: u32, low: u32, high: u32| {
            let min = corners.iter().map(axis).fold(f32::MAX, f32::min) * extent as f32;
            let max = corners.iter().map(axis).fold(f32::MIN, f32::max) * extent as f32;
            (
                (min.floor().max(low as f32) as u32).min(high),
                (max.ceil().max(low as f32) as u32).min(high),
            )
        };
        let (x0, x1) = range(|p| p.x, width, left, right);
        let (y0, y1) = range(|p| p.y, height, top, bottom);
        for y in y0..y1 {
            for x in x0..x1 {
                let point = transform.point(Point::new(
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                ));
                let offset = Point::new(point.x - centre.x, point.y - centre.y);
                if !in_tip(offset, size, brush.square) {
                    continue;
                }
                let amount = opacity * selection::coverage(selection, point);
                if amount <= 0.0 {
                    continue;
                }
                let sample = coverage.pixel_mut(x, y);
                if amount <= sample.amount {
                    continue;
                }
                sample.amount = amount;
                let original = sample.original;
                if mask_target {
                    let pixels = Arc::make_mut(&mut layer.mask.as_mut().unwrap().pixels);
                    let target = f32::from(brush.color[0]) * 0.3
                        + f32::from(brush.color[1]) * 0.59
                        + f32::from(brush.color[2]) * 0.11;
                    pixels.get_pixel_mut(x, y)[0] =
                        (f32::from(original[0]) * (1.0 - amount) + target * amount).round() as u8;
                } else {
                    let pixels = Arc::make_mut(layer.pixels.as_mut().unwrap());
                    let mut color = brush.color.map(|v| v as f32 / 255.0);
                    color[3] *= amount;
                    pixels.get_pixel_mut(x, y).0 =
                        composite(original.map(|v| v as f32 / 255.0), color, BlendMode::Normal)
                            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                }
            }
        }
    }
}

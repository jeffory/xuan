//! What `document/edit` says about the edits that paint inside the selection
//! (`fill`, `fill_path`, `gradient`, `stroke`, and `apply_filter`,
//! `apply_adjustment` and `auto_adjust` on a layer's pixels): the selection's
//! bounds, and whether the edit changed any pixel at all, so that a plugin
//! learns when a selection it forgot about kept a stroke from painting.
//!
//! Such an edit can change a pixel only inside the selection, and a stroke
//! only within its brush's reach of its points, so only that part of the
//! layer is compared, never the whole layer. Keeping the pixels from before
//! costs nothing while the document the request started from still shares
//! them (the first edit of each layer in a request, which copies them anyway
//! when it paints); after that, only that part is copied. A stroke's check
//! therefore costs about as much as the stroke, however large the selection.
use std::sync::Arc;

use image::{GrayImage, RgbaImage};
use serde::Serialize;
use uuid::Uuid;

use super::Reader;
use crate::{
    document::{Document, Layer, Point, Transform},
    paint::Brush,
};

/// The answer's note on one edit that ran while a selection was active.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Note {
    /// The edit, counted from 1 as errors count them.
    pub edit: usize,
    pub op: &'static str,
    /// The selection's bounds in document pixels, all 0 when it is empty.
    pub clipped_to_selection: Bounds,
    /// Whether any of the layer's pixels (or, for a `mask` edit, its mask)
    /// changed. Pixels that stay fully transparent count as unchanged.
    pub changed: bool,
}

/// A rectangle in document pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Bounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The notes of one request.
#[derive(Default)]
pub(super) struct Notes {
    list: Vec<Note>,
    /// The last selection seen and its bounds, worked out once for all the
    /// edits it clips.
    last: Option<(Arc<GrayImage>, Option<[u32; 4]>)>,
    /// Pixels copied and compared so far, for the tests that keep the cost
    /// of the check in sight.
    #[cfg(test)]
    work: usize,
}

impl Notes {
    fn count(&mut self, _pixels: usize) {
        #[cfg(test)]
        {
            self.work += _pixels;
        }
    }
}

/// An edit about to run, with what to compare it with afterwards.
pub(super) struct Watch {
    layer: Uuid,
    mask: bool,
    /// The selection's bounds, `[x0, y0, x1, y1]`; `None` when it is empty.
    selection: Option<[u32; 4]>,
    /// Where in the document the edit can change pixels: the selection's
    /// bounds, cut to a stroke's reach. `None` when that is nowhere.
    area: Option<[u32; 4]>,
    /// The layer's pixels (or mask) there before; `None` when it had none yet.
    before: Option<Snapshot>,
}

impl Reader {
    /// Keep notes on the edits that paint inside the selection, for
    /// [`Reader::take_notes`].
    pub fn with_notes(mut self) -> Self {
        self.notes = Some(Notes::default());
        self
    }

    /// The notes on the edits applied so far, when kept.
    pub fn take_notes(&mut self) -> Vec<Note> {
        (self.notes.as_mut())
            .map(|notes| std::mem::take(&mut notes.list))
            .unwrap_or_default()
    }

    /// Before an edit paints the active layer's pixels (or its `mask`): what
    /// to compare it with, when notes are kept and a selection is active.
    /// A stroke gives its samples, so that only what it can reach is kept
    /// and compared.
    pub(super) fn watch(
        &mut self,
        document: &Document,
        mask: bool,
        stroke: Option<&[(Point, Brush)]>,
    ) -> Option<Watch> {
        let notes = self.notes.as_mut()?;
        let selection = document.selection.as_ref()?;
        let bounds = match &notes.last {
            Some((last, bounds)) if Arc::ptr_eq(last, selection) => *bounds,
            _ => {
                let bounds =
                    crate::selection::bounds(selection).map(|(x0, y0, x1, y1)| [x0, y0, x1, y1]);
                notes.last = Some((selection.clone(), bounds));
                bounds
            }
        };
        let reach = stroke.map(|samples| stroke_reach(samples, document.width, document.height));
        let area = bounds.and_then(|bounds| within(bounds, reach));
        let layer = document.active()?;
        let before = area.and_then(|area| Snapshot::take(layer, mask, area));
        notes.count(before.as_ref().map_or(0, Snapshot::copied));
        Some(Watch {
            layer: layer.id,
            mask,
            selection: bounds,
            area,
            before,
        })
    }

    /// After edit `index` (from 0) ran: note what it did.
    pub(super) fn note(
        &mut self,
        document: &Document,
        index: usize,
        op: &'static str,
        watch: Option<Watch>,
    ) {
        let (Some(notes), Some(watch)) = (self.notes.as_mut(), watch) else {
            return;
        };
        let [x0, y0, x1, y1] = watch.selection.unwrap_or_default();
        let mut compared = 0;
        let changed = watch.changed(document, &mut compared);
        notes.count(compared);
        notes.list.push(Note {
            edit: index + 1,
            op,
            clipped_to_selection: Bounds {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
            },
            changed,
        });
    }
}

/// The document rectangle a stroke through `samples` can paint, `[x0, y0,
/// x1, y1]`, with two pixels to spare. Its dabs lie on the lines between the
/// samples, each at most the largest sample's diameter across (taper, jitter
/// and pressure only make them smaller) and scattered at most `scatter`
/// diameters off the line, and each symmetric copy is that turned or
/// mirrored about the centre.
fn stroke_reach(samples: &[(Point, Brush)], width: u32, height: u32) -> [f32; 4] {
    let Some((_, first)) = samples.first() else {
        return [0.0; 4];
    };
    let largest = |value: fn(&Brush) -> f32| {
        (samples.iter())
            .map(|(_, brush)| value(brush))
            .fold(0.0, f32::max)
    };
    let diameter = largest(|brush| brush.diameter);
    let reach =
        (diameter * 0.5).max(0.5) + largest(|brush| brush.dynamics.scatter) * diameter + 2.0;
    let (low, high) = (samples.iter()).fold(
        (
            Point::new(f32::MAX, f32::MAX),
            Point::new(f32::MIN, f32::MIN),
        ),
        |(low, high), (point, _)| {
            (
                Point::new(low.x.min(point.x), low.y.min(point.y)),
                Point::new(high.x.max(point.x), high.y.max(point.y)),
            )
        },
    );
    let corners = [
        Point::new(low.x - reach, low.y - reach),
        Point::new(high.x + reach, low.y - reach),
        Point::new(high.x + reach, high.y + reach),
        Point::new(low.x - reach, high.y + reach),
    ];
    let copies = if first.symmetry.active() {
        first.symmetry.copies(width, height)
    } else {
        Vec::new()
    };
    let mut rect = [low.x - reach, low.y - reach, high.x + reach, high.y + reach];
    for copy in &copies {
        for corner in corners {
            let point = copy.point(corner);
            rect = [
                rect[0].min(point.x),
                rect[1].min(point.y),
                rect[2].max(point.x),
                rect[3].max(point.y),
            ];
        }
    }
    rect
}

/// The selection's `bounds` cut to a stroke's `reach` (whole pixels, rounded
/// outwards), or `None` when they do not meet.
fn within(bounds: [u32; 4], reach: Option<[f32; 4]>) -> Option<[u32; 4]> {
    let Some(reach) = reach.filter(|reach| reach.iter().all(|v| v.is_finite())) else {
        return Some(bounds);
    };
    let [x0, y0, x1, y1] = bounds.map(|v| v as f32);
    let area = [
        reach[0].floor().clamp(x0, x1),
        reach[1].floor().clamp(y0, y1),
        reach[2].ceil().clamp(x0, x1),
        reach[3].ceil().clamp(y0, y1),
    ]
    .map(|v| v as u32);
    (area[0] < area[2] && area[1] < area[3]).then_some(area)
}

impl Watch {
    /// Whether the edit changed a pixel where it could, adding to `compared`
    /// how many it looked at.
    fn changed(&self, document: &Document, compared: &mut usize) -> bool {
        // An empty selection, or one the stroke does not reach, lets nothing change.
        let Some(area) = self.area else {
            return false;
        };
        let Some(layer) = document.layers.iter().find(|layer| layer.id == self.layer) else {
            return true;
        };
        let Some((after, transform)) = Pixels::of(layer, self.mask) else {
            return false;
        };
        let size = after.size();
        let channels = after.channels();
        let [x0, y0, x1, y1] = region(area, transform, size);
        *compared += (x1 - x0) as usize * (y1 - y0) as usize;
        let row = |y| span(after.raw(), size.0, channels, y, x0, x1);
        let Some(before) = &self.before else {
            // The layer had no pixels (or no mask): they started empty (or white).
            return (y0..y1).any(|y| !blank(row(y), channels));
        };
        if let Kept::Shared(old) = &before.kept
            && old.same(&after)
        {
            return false;
        }
        let Some((ox, oy)) = before.offset(transform, size) else {
            // Resampled or turned: there is nothing to compare it with.
            return true;
        };
        let (old_width, old_height) = (i64::from(before.size.0), i64::from(before.size.1));
        let [kx0, ky0, kx1, ky1] = before.area.map(i64::from);
        let (x0, x1) = (i64::from(x0), i64::from(x1));
        let at = |x: i64| (x - x0) as usize * channels;
        for y in y0..y1 {
            let line = row(y);
            let old_y = i64::from(y) - oy;
            // The columns that held the old pixels; the rest is where the
            // layer grew, which started empty.
            let (inner0, inner1) = if (0..old_height).contains(&old_y) {
                (ox.clamp(x0, x1), (ox + old_width).clamp(x0, x1))
            } else {
                (x1, x1)
            };
            if !blank(&line[..at(inner0)], channels) || !blank(&line[at(inner1)..], channels) {
                return true;
            }
            // The snapshot keeps all the old pixels the edit can reach.
            let (c0, c1) = ((inner0 - ox).max(kx0), (inner1 - ox).min(kx1));
            if (ky0..ky1).contains(&old_y) && c0 < c1 {
                let old = before.span(old_y as u32, c0 as u32, c1 as u32, channels);
                if !same(old, &line[at(c0 + ox)..at(c1 + ox)], channels) {
                    return true;
                }
            }
        }
        false
    }
}

/// A layer's pixels or mask, shared.
enum Pixels {
    Rgba(Arc<RgbaImage>),
    Gray(Arc<GrayImage>),
}

impl Pixels {
    /// The buffer an edit paints, and where it sits in the document.
    fn of(layer: &Layer, mask: bool) -> Option<(Self, Transform)> {
        if mask {
            let mask = layer.mask.as_ref()?;
            let transform = mask.placement.unwrap_or(layer.transform);
            Some((Self::Gray(mask.pixels.clone()), transform))
        } else {
            Some((Self::Rgba(layer.pixels.clone()?), layer.transform))
        }
    }

    fn raw(&self) -> &[u8] {
        match self {
            Self::Rgba(image) => image.as_raw(),
            Self::Gray(image) => image.as_raw(),
        }
    }

    fn size(&self) -> (u32, u32) {
        match self {
            Self::Rgba(image) => image.dimensions(),
            Self::Gray(image) => image.dimensions(),
        }
    }

    fn channels(&self) -> usize {
        match self {
            Self::Rgba(_) => 4,
            Self::Gray(_) => 1,
        }
    }

    /// Whether more than the layer and this hold the buffer, so that painting
    /// copies it anyway and keeping it costs nothing.
    fn shared(&self) -> bool {
        match self {
            Self::Rgba(image) => Arc::strong_count(image) > 2,
            Self::Gray(image) => Arc::strong_count(image) > 2,
        }
    }

    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Rgba(a), Self::Rgba(b)) => Arc::ptr_eq(a, b),
            (Self::Gray(a), Self::Gray(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// A buffer before an edit, as much of it as the edit can change.
struct Snapshot {
    size: (u32, u32),
    transform: Transform,
    /// The columns `area[0]..area[2]` and rows `area[1]..area[3]` it keeps.
    area: [u32; 4],
    kept: Kept,
}

enum Kept {
    /// All of it, still shared with the document the request started from.
    Shared(Pixels),
    /// A copy of the area, row after row.
    Copy(Vec<u8>),
}

impl Snapshot {
    /// The buffer's pixels whose centres can fall inside `area` (document
    /// pixels), or all of it while it is shared.
    fn take(layer: &Layer, mask: bool, area: [u32; 4]) -> Option<Self> {
        let (pixels, transform) = Pixels::of(layer, mask)?;
        let size = pixels.size();
        if pixels.shared() {
            return Some(Self {
                size,
                transform,
                area: [0, 0, size.0, size.1],
                kept: Kept::Shared(pixels),
            });
        }
        let area = region(area, transform, size);
        let channels = pixels.channels();
        let mut copy = Vec::with_capacity(
            (area[2] - area[0]) as usize * (area[3] - area[1]) as usize * channels,
        );
        for y in area[1]..area[3] {
            copy.extend_from_slice(span(pixels.raw(), size.0, channels, y, area[0], area[2]));
        }
        Some(Self {
            size,
            transform,
            area,
            kept: Kept::Copy(copy),
        })
    }

    /// How many pixels were copied to keep it.
    fn copied(&self) -> usize {
        match self.kept {
            Kept::Shared(_) => 0,
            Kept::Copy(_) => {
                (self.area[2] - self.area[0]) as usize * (self.area[3] - self.area[1]) as usize
            }
        }
    }

    /// Row `y`, columns `x0..x1`, inside the area kept.
    fn span(&self, y: u32, x0: u32, x1: u32, channels: usize) -> &[u8] {
        match &self.kept {
            Kept::Shared(pixels) => span(pixels.raw(), self.size.0, channels, y, x0, x1),
            Kept::Copy(copy) => {
                let width = (self.area[2] - self.area[0]) as usize;
                let start =
                    ((y - self.area[1]) as usize * width + (x0 - self.area[0]) as usize) * channels;
                &copy[start..start + (x1 - x0) as usize * channels]
            }
        }
    }

    /// Where this buffer's first pixel lies in one of `size` placed by
    /// `transform`: the same place, or further in when a stroke or a filter
    /// grew the layer. `None` when the buffer was resampled or turned.
    fn offset(&self, transform: Transform, size: (u32, u32)) -> Option<(i64, i64)> {
        if (self.size, self.transform) == (size, transform) {
            return Some((0, 0));
        }
        let at = |unit: Point| {
            let point = transform.inverse(self.transform.point(unit));
            (point.x * size.0 as f32, point.y * size.1 as f32)
        };
        let (x0, y0) = at(Point::new(0.0, 0.0));
        let (x1, y1) = at(Point::new(1.0, 1.0));
        let (ox, oy) = (x0.round(), y0.round());
        let near = |a: f32, b: f32| (a - b).abs() <= 0.25;
        (near(x0, ox)
            && near(y0, oy)
            && near(x1, ox + self.size.0 as f32)
            && near(y1, oy + self.size.1 as f32))
        .then_some((ox as i64, oy as i64))
    }
}

/// The pixels of a `size` buffer placed by `transform` whose centres can fall
/// inside `bounds` (document pixels), with one to spare on each side, as
/// `[x0, y0, x1, y1]`.
fn region(
    [x0, y0, x1, y1]: [u32; 4],
    transform: Transform,
    (width, height): (u32, u32),
) -> [u32; 4] {
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .map(|(x, y)| transform.inverse(Point::new(x as f32, y as f32)));
    if corners.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return [0, 0, width, height];
    }
    let (w, h) = (width as f32, height as f32);
    let low = |axis: fn(&Point) -> f32| corners.iter().map(axis).fold(f32::MAX, f32::min);
    let high = |axis: fn(&Point) -> f32| corners.iter().map(axis).fold(f32::MIN, f32::max);
    let clamp = |value: f32, limit: u32| value.clamp(0.0, limit as f32) as u32;
    let area = [
        clamp((low(|p| p.x) * w).floor() - 1.0, width),
        clamp((low(|p| p.y) * h).floor() - 1.0, height),
        clamp((high(|p| p.x) * w).ceil() + 1.0, width),
        clamp((high(|p| p.y) * h).ceil() + 1.0, height),
    ];
    if area[0] < area[2] && area[1] < area[3] {
        area
    } else {
        [0; 4]
    }
}

/// Row `y`, columns `x0..x1`, of a `width`-wide buffer of `channels` bytes a pixel.
fn span(raw: &[u8], width: u32, channels: usize, y: u32, x0: u32, x1: u32) -> &[u8] {
    let start = (y as usize * width as usize + x0 as usize) * channels;
    &raw[start..start + (x1 - x0) as usize * channels]
}

/// Whether pixels look as a new buffer's do: transparent, or a white mask.
fn blank(run: &[u8], channels: usize) -> bool {
    if channels == 4 {
        run.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0)
    } else {
        run.iter().all(|&value| value == 255)
    }
}

/// Whether two runs of pixels look the same: equal, or transparent in both.
fn same(old: &[u8], new: &[u8], channels: usize) -> bool {
    old == new
        || (channels == 4
            && (old.as_chunks::<4>().0.iter().zip(new.as_chunks::<4>().0))
                .all(|(a, b)| a == b || (a[3] == 0 && b[3] == 0)))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::{Access, MAX_LAYERS, apply, parse_edits};
    use super::*;

    #[test]
    fn pixels_are_kept_without_a_copy_while_shared_and_only_the_selected_part_after() {
        let mut document = Document::new(40, 30).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::new(40, 30)));
        let layer = &document.layers[0];
        // Painting copies pixels the request's original still shares, so
        // keeping them is free.
        let original = document.clone();
        let shared = Snapshot::take(&document.layers[0], false, [0, 0, 10, 10]).unwrap();
        assert!(matches!(shared.kept, Kept::Shared(_)));
        drop((shared, original));
        assert_eq!(Arc::strong_count(layer.pixels.as_ref().unwrap()), 1);
        // Otherwise only the selection's part, a pixel to spare, is copied.
        let copied = Snapshot::take(layer, false, [0, 0, 10, 10]).unwrap();
        assert_eq!(copied.area, [0, 0, 11, 11]);
        assert!(matches!(&copied.kept, Kept::Copy(bytes) if bytes.len() == 11 * 11 * 4));
        assert_eq!(Arc::strong_count(layer.pixels.as_ref().unwrap()), 1);
        // A layer turned or placed elsewhere is compared where the selection
        // falls on it.
        let mut turned = layer.transform;
        turned.rotation = 90.0;
        // The square falls on columns 5–15 and rows 25–30 of the turned layer.
        let [x0, y0, x1, y1] = region([0, 0, 10, 10], turned, (40, 30));
        assert!(
            x0 <= 5 && y0 <= 25 && x1 >= 15 && y1 == 30,
            "{x0} {y0} {x1} {y1}"
        );
        assert!(x1 - x0 <= 14 && y1 - y0 <= 7, "{x0} {y0} {x1} {y1}");
        let mut moved = layer.transform;
        moved.x = 100.0;
        assert_eq!(region([30, 0, 40, 10], moved, (40, 30)), [0; 4]);
    }

    fn apply_noted(document: &mut Document, edits: Vec<Value>) -> Reader {
        let edits = parse_edits(json!(edits)).unwrap();
        let mut reader = Reader::new(Access::anywhere(), MAX_LAYERS).with_notes();
        apply(document, &edits, &mut reader).unwrap();
        reader
    }

    #[test]
    fn a_strokes_check_costs_what_the_stroke_does_not_what_the_selection_does() {
        let mut document = Document::new(400, 300).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            400,
            300,
            image::Rgba([100, 100, 100, 255]),
        )));
        let mut edits =
            vec![json!({"op": "select_rect", "x": 0, "y": 0, "width": 400, "height": 300})];
        edits.extend((0..50).map(
            |i| json!({"op": "stroke", "points": [[8 * i, 6 * i]], "size": 6, "color": "#ff0000"}),
        ));
        let mut reader = apply_noted(&mut document, edits);
        let work = reader.notes.as_ref().unwrap().work;
        let notes = reader.take_notes();
        assert_eq!(notes.len(), 50);
        assert!(notes.iter().all(|note| note.changed));
        // Each dab's check copies and compares about 150 pixels; the layer
        // the selection covers is 120,000.
        assert!(work <= 50 * 2 * 15 * 15, "{work} pixels for 50 dabs");
    }

    /// A stroke's `changed`, compared only within its reach, agrees with
    /// comparing all that the selection covers, for strokes of every kind on
    /// layers that are turned, flipped, scaled and grow under them.
    #[test]
    fn a_strokes_reach_covers_every_pixel_it_can_change() {
        let mut state = 11_u32;
        let mut next = move |limit: u32| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) % limit
        };
        let mut outcomes = [0; 2];
        for case in 0..400 {
            let mut document = Document::new(60, 40).unwrap();
            let layer = &mut document.layers[0];
            layer.pixels = Some(Arc::new(RgbaImage::from_fn(30, 20, |x, y| {
                let alpha = if (x + y) % 7 == 0 { 0 } else { 255 };
                image::Rgba([(x * 8) as u8, (y * 12) as u8, 90, alpha])
            })));
            let place = |x, y, width, height, rotation, flip_x, flip_y| Transform {
                x,
                y,
                width,
                height,
                rotation,
                flip_x,
                flip_y,
                ..Transform::new(30, 20)
            };
            layer.transform = match case % 4 {
                0 => place(0.0, 0.0, 30.0, 20.0, 0.0, false, false),
                1 => place(10.0, 5.0, 45.0, 30.0, 30.0, false, true),
                2 => place(5.0, 10.0, 15.0, 10.0, -100.0, true, false),
                _ => place(0.0, 0.0, 60.0, 40.0, 0.0, true, true),
            };
            let select = vec![json!({
                "op": "select_rect", "ellipse": next(3) == 0,
                "x": next(55), "y": next(35), "width": 1 + next(40), "height": 1 + next(30),
            })];
            let coordinate = |value: u32| value as f32 / 10.0 - 10.0;
            let points: Vec<Value> = (0..1 + next(4))
                .map(|_| {
                    let (x, y) = (coordinate(next(800)), coordinate(next(600)));
                    match next(3) {
                        0 => json!([x, y, next(11) as f32 / 10.0]),
                        _ => json!([x, y]),
                    }
                })
                .collect();
            let mask = next(5) == 0;
            let color = ["#ff0000", "#00ff0080", "#5a5a5a", "#ffffff"][next(4) as usize];
            // Dodge and Burn change pixels, not masks.
            let mode = ["paint", "erase", "dodge", "burn"][next(if mask { 2 } else { 4 }) as usize];
            let mut stroke = json!({
                "op": "stroke", "points": points, "mask": mask, "color": color, "mode": mode,
                "size": 1 + next(24), "hardness": next(11) as f32 / 10.0,
                "opacity": (2 + next(9)) as f32 / 10.0,
            });
            if next(2) == 0 {
                stroke["taper_in"] = json!(next(30));
                stroke["taper_out"] = json!(next(30));
            }
            if next(2) == 0 {
                stroke["spacing"] = json!(next(20) as f32 / 10.0);
                stroke["scatter"] = json!(next(30) as f32 / 10.0);
                stroke["scatter_count"] = json!(1 + next(4));
                stroke["size_jitter"] = json!(next(11) as f32 / 10.0);
                stroke["seed"] = json!(next(1000));
            }
            if next(3) == 0 {
                let symmetry = ["vertical", "horizontal", "radial"][next(3) as usize];
                stroke["symmetry"] = json!({
                    "mode": symmetry, "segments": 2 + next(7), "center": [next(61), next(41)],
                });
            }
            let mut reader = apply_noted(&mut document, select);
            assert!(reader.take_notes().is_empty());
            let before = document.clone();
            let mut reader = apply_noted(&mut document, vec![stroke.clone()]);
            let [note] = reader.take_notes()[..] else {
                panic!("case {case}: one note");
            };
            // As the notes compared before: all the selection covers.
            let bounds = before
                .selection
                .as_deref()
                .and_then(crate::selection::bounds);
            let bounds = bounds.map(|(x0, y0, x1, y1)| [x0, y0, x1, y1]);
            let active = before.active().unwrap();
            let whole = Watch {
                layer: active.id,
                mask,
                selection: bounds,
                area: bounds,
                before: bounds.and_then(|bounds| Snapshot::take(active, mask, bounds)),
            };
            let expected = whole.changed(&document, &mut 0);
            assert_eq!(note.changed, expected, "case {case}: {stroke}");
            outcomes[usize::from(expected)] += 1;
        }
        assert!(outcomes[0] > 50 && outcomes[1] > 50, "{outcomes:?}");
    }
}

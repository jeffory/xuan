use std::sync::Arc;

use super::{
    Brush, Document, Layer, PaintMode, Point, Result, StrokeOptions,
    dynamics::{Dynamics, Piece, Walker},
    symmetry::Reflection,
};

/// Coverage and original pixels for one pointer-down/up gesture. Overlapping
/// segments use the strongest coverage, so event frequency cannot darken joins.
/// With a flow below 100%, dabs instead build up toward that coverage
/// ([`super::build_up`]), laid out by distance so frequency still cannot.
#[derive(Default)]
pub struct Stroke {
    bounds: [u32; 4],
    pixels: Vec<StrokePixel>,
    /// Lays out dabs and taper when the brush has dynamics.
    walker: Option<Walker>,
    /// The samples so far, to paint the stroke again once its length is
    /// known (for the taper at its end).
    samples: Vec<(Point, Brush)>,
    /// The symmetric copies every piece is painted as, fixed when the
    /// stroke starts; empty without symmetry.
    copies: Option<Vec<Reflection>>,
    /// Whether the copy being painted is mirrored in x and y, for the
    /// pencil's whole-pixel dabs.
    pub(super) flip: [bool; 2],
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct StrokePixel {
    pub original: [u8; 4],
    pub amount: f32,
}

impl Stroke {
    /// Append a segment, keeping paint/erase opacity consistent within this stroke.
    /// Start a fresh `Stroke` for each gesture; retouch tools retain their buildup.
    pub fn segment(
        &mut self,
        document: &mut Document,
        from: Point,
        to: Point,
        from_brush: &Brush,
        brush: &Brush,
        options: StrokeOptions<'_>,
    ) -> Result<()> {
        self.start(document, from_brush, options.mode);
        if self.walker.is_some() || walks(from_brush, options.mode) {
            if self.samples.is_empty() {
                self.samples.push((from, from_brush.clone()));
            }
            self.samples.push((to, brush.clone()));
            let walker = self
                .walker
                .get_or_insert_with(|| Walker::new(walked(from_brush, options.mode), None));
            let mut pieces = Vec::new();
            walker.walk([from, to], [from_brush, brush], &mut pieces);
            return self.draw(document, pieces, &options);
        }
        if self.symmetric() {
            let piece = Piece::Segment([from, to], [from_brush.clone(), brush.clone()]);
            return self.draw(document, vec![piece], &options);
        }
        let accumulate = options.mask_target || dynamic(options.mode);
        super::stroke_segment(
            document,
            from,
            to,
            from_brush,
            brush,
            options,
            accumulate.then_some(self),
        )
    }

    /// Paint a whole stroke through `samples`, each a point and the brush
    /// there (with pen pressure already applied). A single sample paints one
    /// dab. As the stroke's length is known, the taper at its end is drawn
    /// at once. Use a fresh `Stroke`.
    pub fn path(
        &mut self,
        document: &mut Document,
        samples: &[(Point, Brush)],
        options: StrokeOptions<'_>,
    ) -> Result<()> {
        let Some((_, first_brush)) = samples.first() else {
            return Ok(());
        };
        self.start(document, first_brush, options.mode);
        let pairs = samples
            .windows(2)
            .map(|pair| (&pair[0], &pair[1]))
            .chain((samples.len() == 1).then(|| (&samples[0], &samples[0])));
        if !walks(first_brush, options.mode) {
            for ((from, from_brush), (to, brush)) in pairs {
                self.segment(document, *from, *to, from_brush, brush, copy(&options))?;
            }
            return Ok(());
        }
        let total = samples
            .windows(2)
            .map(|pair| pair[0].0.distance(pair[1].0))
            .sum::<f32>();
        let mut walker = Walker::new(walked(first_brush, options.mode), Some(total));
        let mut pieces = Vec::new();
        for ((from, from_brush), (to, brush)) in pairs {
            walker.walk([*from, *to], [from_brush, brush], &mut pieces);
        }
        self.draw(document, pieces, &options)
    }

    /// End the gesture. A brush that tapers at the end paints the stroke
    /// again now that its length is known, from the original pixels.
    pub fn finish(&mut self, document: &mut Document, options: StrokeOptions<'_>) -> Result<()> {
        let Some(walker) = self.walker.take() else {
            return Ok(());
        };
        let samples = std::mem::take(&mut self.samples);
        let dynamics = walker.dynamics();
        if !(dynamics.tapered() && dynamics.taper_out > 0.0) {
            return Ok(());
        }
        self.restore(document, options.mask_target);
        self.path(document, &samples, options)
    }

    /// Fix the stroke's symmetric copies at its first piece, from the
    /// brush's symmetry on this canvas. Only the Brush, Pencil, Eraser and
    /// Dodge, Burn and Sponge paint symmetrically.
    fn start(&mut self, document: &Document, brush: &Brush, mode: PaintMode) {
        if self.copies.is_none() {
            self.copies = Some(if dynamic(mode) && brush.symmetry.active() {
                brush.symmetry.copies(document.width, document.height)
            } else {
                Vec::new()
            });
        }
    }

    fn symmetric(&self) -> bool {
        self.copies
            .as_ref()
            .is_some_and(|copies| !copies.is_empty())
    }

    /// Paint the pieces, each once for every symmetric copy. All copies
    /// share this stroke's coverage, so where they cross a pixel takes the
    /// strongest of them instead of being painted twice.
    fn draw(
        &mut self,
        document: &mut Document,
        pieces: Vec<Piece>,
        options: &StrokeOptions<'_>,
    ) -> Result<()> {
        let copies = self.copies.clone().unwrap_or_default();
        for piece in pieces {
            let (from, to, from_brush, brush) = match &piece {
                Piece::Segment([from, to], [from_brush, brush]) => (*from, *to, from_brush, brush),
                Piece::Dab(point, brush) => (*point, *point, brush, brush),
            };
            if copies.is_empty() {
                super::stroke_segment(
                    document,
                    from,
                    to,
                    from_brush,
                    brush,
                    copy(options),
                    Some(self),
                )?;
                continue;
            }
            // The pencil snaps to whole pixels; snapping before mirroring
            // keeps ties (x.5) from rounding to different sides in the copies.
            let (from, to) = if options.mode == PaintMode::Pencil {
                (
                    super::pencil::snapped(from, brush.diameter),
                    super::pencil::snapped(to, brush.diameter),
                )
            } else {
                (from, to)
            };
            for reflection in &copies {
                self.flip = reflection.flips();
                let result = super::stroke_segment(
                    document,
                    reflection.point(from),
                    reflection.point(to),
                    &reflection.brush(from_brush),
                    &reflection.brush(brush),
                    copy(options),
                    Some(self),
                );
                self.flip = [false; 2];
                result?;
            }
        }
        Ok(())
    }

    /// Put back the pixels the stroke has painted and forget its coverage.
    fn restore(&mut self, document: &mut Document, mask: bool) {
        let [left, top, right, _] = self.bounds;
        let stride = (right - left) as usize;
        if stride == 0 || self.pixels.is_empty() {
            return;
        }
        let Some(layer) = document.active_mut() else {
            return;
        };
        for (index, sample) in self.pixels.iter_mut().enumerate() {
            if sample.amount <= 0.0 {
                continue;
            }
            sample.amount = 0.0;
            let x = left + (index % stride) as u32;
            let y = top + (index / stride) as u32;
            if mask {
                if let Some(mask) = &mut layer.mask {
                    Arc::make_mut(&mut mask.pixels).put_pixel(
                        x,
                        y,
                        image::Luma([sample.original[0]]),
                    );
                }
            } else if let Some(pixels) = &mut layer.pixels {
                Arc::make_mut(pixels).put_pixel(x, y, image::Rgba(sample.original));
            }
        }
    }

    pub(super) fn shift(&mut self, [x, y]: [u32; 2]) {
        if !self.pixels.is_empty() {
            self.bounds[0] += x;
            self.bounds[2] += x;
            self.bounds[1] += y;
            self.bounds[3] += y;
        }
    }

    pub(super) fn prepare(&mut self, layer: &Layer, mask: bool, bounds: [u32; 4]) {
        let [left, top, right, bottom] = self.bounds;
        if !self.pixels.is_empty()
            && bounds[0] >= left
            && bounds[1] >= top
            && bounds[2] <= right
            && bounds[3] <= bottom
        {
            return;
        }
        let (width, height) = if mask {
            layer.mask.as_ref().unwrap().pixels.dimensions()
        } else {
            layer.pixels.as_ref().unwrap().dimensions()
        };
        // Reserve in blocks to avoid reallocating for every small pointer move.
        // Only the stroke's bounding region needs snapshots, not the whole layer.
        let mut next = [
            bounds[0] / 64 * 64,
            bounds[1] / 64 * 64,
            (bounds[2].div_ceil(64) * 64).min(width),
            (bounds[3].div_ceil(64) * 64).min(height),
        ];
        if !self.pixels.is_empty() {
            next = [
                next[0].min(left),
                next[1].min(top),
                next[2].max(right),
                next[3].max(bottom),
            ];
        }
        let stride = (next[2] - next[0]) as usize;
        let mut pixels = Vec::with_capacity(stride * (next[3] - next[1]) as usize);
        for y in next[1]..next[3] {
            for x in next[0]..next[2] {
                let original = if mask {
                    let value = layer.mask.as_ref().unwrap().pixels.get_pixel(x, y)[0];
                    [value, value, value, 255]
                } else {
                    layer.pixels.as_ref().unwrap().get_pixel(x, y).0
                };
                pixels.push(StrokePixel {
                    original,
                    amount: 0.0,
                });
            }
        }
        for y in top..bottom {
            let start = (y - next[1]) as usize * stride + (left - next[0]) as usize;
            pixels[start..start + (right - left) as usize]
                .copy_from_slice(self.row(left, right, y));
        }
        self.bounds = next;
        self.pixels = pixels;
    }

    pub(crate) fn row(&self, left: u32, right: u32, y: u32) -> &[StrokePixel] {
        let start = self.index(left, y);
        &self.pixels[start..start + (right - left) as usize]
    }

    pub(crate) fn pixel_mut(&mut self, x: u32, y: u32) -> &mut StrokePixel {
        let index = self.index(x, y);
        &mut self.pixels[index]
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y - self.bounds[1]) as usize * (self.bounds[2] - self.bounds[0]) as usize
            + (x - self.bounds[0]) as usize
    }
}

/// Whether a mode paints with brush dynamics and symmetry, and keeps its
/// coverage and original pixels for the whole stroke. Dodge, Burn and
/// Sponge rely on that to change each pixel once, from its original value.
fn dynamic(mode: PaintMode) -> bool {
    matches!(
        mode,
        PaintMode::Paint
            | PaintMode::Erase
            | PaintMode::Pencil
            | PaintMode::Dodge
            | PaintMode::Burn
            | PaintMode::Sponge
    )
}

/// Whether a mode builds up paint with the brush's flow: the Brush and
/// the Eraser, on pixels or a mask.
pub(super) fn flows(mode: PaintMode) -> bool {
    matches!(mode, PaintMode::Paint | PaintMode::Erase)
}

/// Whether a stroke is laid out by a [`Walker`]: with dynamics, or as dabs
/// for a flow below 100%.
fn walks(brush: &Brush, mode: PaintMode) -> bool {
    dynamic(mode) && (brush.dynamics.active() || (flows(mode) && brush.flow < 1.0))
}

/// The dynamics a [`Walker`] lays the stroke out with. Flow builds up dab
/// by dab, so a flow below 100% paints dabs even without spacing.
fn walked(brush: &Brush, mode: PaintMode) -> Dynamics {
    if flows(mode) && brush.flow < 1.0 {
        brush.dynamics.with_flow()
    } else {
        brush.dynamics
    }
}

fn copy<'a>(options: &StrokeOptions<'a>) -> StrokeOptions<'a> {
    StrokeOptions {
        mode: options.mode,
        mask_target: options.mask_target,
        source: options.source,
        clone_offset: options.clone_offset,
    }
}

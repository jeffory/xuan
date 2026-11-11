//! Brush dynamics: dab spacing, taper, scatter and jitter along a stroke.
//!
//! A stroke with dynamics is turned into pieces that the plain brush paints:
//! swept segments when the brush is continuous (only taper is on), or single
//! dabs (zero-length segments) when spacing, scatter or jitter is on. Every
//! piece goes through [`super::stroke_segment`], so the GPU and CPU paths,
//! selections, masks and per-stroke coverage all work as for plain strokes.
//!
//! Randomness comes from a hash of the seed and the dab's index along the
//! stroke, so the same samples and seed always give the same pixels.

use super::{Brush, Point};

/// How a brush varies along a stroke. The default is a plain brush.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dynamics {
    /// Distance between dabs as a fraction of the diameter (0.25 is 25%).
    /// Zero sweeps a continuous stroke, unless scatter or jitter is on, which
    /// paint dabs at 25%.
    pub spacing: f32,
    /// Length in document pixels over which the stroke grows from nothing.
    pub taper_in: f32,
    /// Length in document pixels over which the stroke fades out at its end.
    pub taper_out: f32,
    /// Taper scales the size.
    pub taper_size: bool,
    /// Taper scales the opacity.
    pub taper_opacity: bool,
    /// Largest distance a dab moves from the path, as a fraction of the diameter.
    pub scatter: f32,
    /// Dabs at every spacing step (1 to 16).
    pub count: u32,
    /// How much smaller a dab may randomly be, 0 to 1.
    pub size_jitter: f32,
    /// How much more transparent a dab may randomly be, 0 to 1.
    pub opacity_jitter: f32,
    /// How far a dab's hue may randomly turn, 0 to 1 (1 is ±180°).
    pub hue_jitter: f32,
    /// The random sequence; the same seed repeats a stroke exactly.
    pub seed: u64,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self {
            spacing: 0.0,
            taper_in: 0.0,
            taper_out: 0.0,
            taper_size: true,
            taper_opacity: false,
            scatter: 0.0,
            count: 1,
            size_jitter: 0.0,
            opacity_jitter: 0.0,
            hue_jitter: 0.0,
            seed: 0,
        }
    }
}

/// Spacing used for dabs when only scatter or jitter asks for them.
pub const DEFAULT_SPACING: f32 = 0.25;
/// Spacing used for dabs when only a flow below 100% asks for them; close,
/// so the build-up is smooth.
pub const FLOW_SPACING: f32 = 0.1;
/// The shortest distance between dabs, in document pixels.
pub const MIN_STEP: f32 = 1.0;
/// The most dabs at one spacing step.
pub const MAX_COUNT: u32 = 16;

impl Dynamics {
    /// Whether the stroke differs from a plain brush stroke.
    pub fn active(&self) -> bool {
        self.dabs() || self.tapered()
    }

    /// Whether the stroke is painted as separate dabs.
    pub fn dabs(&self) -> bool {
        self.spacing > 0.0
            || self.scatter > 0.0
            || self.count > 1
            || self.size_jitter > 0.0
            || self.opacity_jitter > 0.0
            || self.hue_jitter > 0.0
    }

    /// These dynamics for a stroke whose flow builds up: as dabs, at
    /// [`FLOW_SPACING`] unless something else already asks for dabs.
    pub fn with_flow(mut self) -> Self {
        if !self.dabs() {
            self.spacing = FLOW_SPACING;
        }
        self
    }

    pub fn tapered(&self) -> bool {
        (self.taper_in > 0.0 || self.taper_out > 0.0) && (self.taper_size || self.taper_opacity)
    }

    /// Distance to the next dab after one of this diameter.
    pub fn step(&self, diameter: f32) -> f32 {
        let spacing = if self.spacing > 0.0 {
            self.spacing
        } else {
            DEFAULT_SPACING
        };
        (spacing * diameter).max(MIN_STEP)
    }

    /// An error message when a setting is out of range.
    pub fn validate(&self) -> Result<(), String> {
        let check = |value: f32, max: f32, what: &str| {
            if value.is_finite() && (0.0..=max).contains(&value) {
                Ok(())
            } else {
                Err(format!("{what} must be between 0 and {max}"))
            }
        };
        check(self.spacing, 10.0, "The spacing")?;
        check(self.taper_in, 100_000.0, "The taper")?;
        check(self.taper_out, 100_000.0, "The taper")?;
        check(self.scatter, 10.0, "The scatter")?;
        check(self.size_jitter, 1.0, "The size jitter")?;
        check(self.opacity_jitter, 1.0, "The opacity jitter")?;
        check(self.hue_jitter, 1.0, "The hue jitter")?;
        if !(1..=MAX_COUNT).contains(&self.count) {
            return Err(format!(
                "The scatter count must be between 1 and {MAX_COUNT}"
            ));
        }
        Ok(())
    }
}

/// What the brush paints for part of a stroke.
pub(crate) enum Piece {
    Segment([Point; 2], [Brush; 2]),
    Dab(Point, Brush),
}

/// Walks a stroke's samples in order and lays out its pieces.
#[derive(Clone, Debug)]
pub(crate) struct Walker {
    dynamics: Dynamics,
    /// The stroke's length when it is known, for the taper at its end.
    total: Option<f32>,
    travelled: f32,
    next: f32,
    index: u64,
}

/// Longest swept piece inside a taper, in document pixels.
pub const TAPER_PIECE: f32 = 4.0;

impl Walker {
    pub fn new(dynamics: Dynamics, total: Option<f32>) -> Self {
        Self {
            dynamics,
            total,
            travelled: 0.0,
            next: 0.0,
            index: 0,
        }
    }

    pub fn dynamics(&self) -> Dynamics {
        self.dynamics
    }

    /// The taper factor at a distance along the stroke.
    fn taper(&self, distance: f32) -> f32 {
        let d = &self.dynamics;
        if self.total.is_some_and(|total| total <= 0.0) {
            // A single dab has no ends to taper.
            return 1.0;
        }
        let mut factor = 1.0f32;
        if d.taper_in > 0.0 {
            factor *= (distance / d.taper_in).clamp(0.0, 1.0);
        }
        if let Some(total) = self.total
            && d.taper_out > 0.0
        {
            factor *= ((total - distance) / d.taper_out).clamp(0.0, 1.0);
        }
        factor
    }

    /// The brush at `t` between two samples, tapered at `distance`.
    fn brush_at(&self, brushes: [&Brush; 2], t: f32, distance: f32) -> Brush {
        let [from, to] = brushes;
        let mut brush = to.clone();
        brush.diameter = from.diameter + (to.diameter - from.diameter) * t;
        brush.opacity = from.opacity + (to.opacity - from.opacity) * t;
        brush.flow = from.flow + (to.flow - from.flow) * t;
        brush.tilt = [0, 1].map(|axis| from.tilt[axis] + (to.tilt[axis] - from.tilt[axis]) * t);
        if self.dynamics.tapered() {
            let factor = self.taper(distance);
            if self.dynamics.taper_size {
                brush.diameter *= factor;
            }
            if self.dynamics.taper_opacity {
                brush.opacity *= factor;
            }
        }
        brush
    }

    /// The pieces from `from` to `to`, continuing the stroke so far.
    pub fn walk(
        &mut self,
        [from, to]: [Point; 2],
        [from_brush, brush]: [&Brush; 2],
        pieces: &mut Vec<Piece>,
    ) {
        let length = from.distance(to);
        let start = self.travelled;
        let end = start + length;
        let at = |t: f32| Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        let t_of = |distance: f32| {
            if length > 0.0 {
                ((distance - start) / length).clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        if self.dynamics.dabs() {
            while self.next <= end {
                let distance = self.next;
                let t = t_of(distance);
                let mut base = self.brush_at([from_brush, brush], t, distance);
                let step = self.dynamics.step(base.diameter);
                base.flow = dab_flow(base.flow, step / base.diameter.max(MIN_STEP));
                let centre = at(t);
                for k in 0..self.dynamics.count.clamp(1, MAX_COUNT) {
                    pieces.push(self.dab(centre, &base, k));
                }
                self.index += 1;
                self.next = distance + step;
            }
        } else {
            // Continuous: cut at the taper's edges and into short pieces inside it.
            let mut cuts = vec![start, end];
            let d = &self.dynamics;
            let mut zones = vec![(0.0, d.taper_in)];
            if let Some(total) = self.total {
                zones.push((total - d.taper_out, total));
            }
            for (low, high) in zones {
                let (low, high) = (low.max(start), high.min(end));
                if high <= low {
                    continue;
                }
                let n = ((high - low) / TAPER_PIECE).ceil().max(1.0) as usize;
                cuts.extend((0..=n).map(|i| low + (high - low) * i as f32 / n as f32));
            }
            cuts.sort_by(f32::total_cmp);
            cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            if cuts.len() == 1 {
                cuts.push(cuts[0]);
            }
            for pair in cuts.windows(2) {
                let (t0, t1) = (t_of(pair[0]), t_of(pair[1]));
                pieces.push(Piece::Segment(
                    [at(t0), at(t1)],
                    [
                        self.brush_at([from_brush, brush], t0, pair[0]),
                        self.brush_at([from_brush, brush], t1, pair[1]),
                    ],
                ));
            }
        }
        self.travelled = end;
    }

    /// The `k`-th dab at a spacing step, scattered and jittered.
    fn dab(&self, centre: Point, base: &Brush, k: u32) -> Piece {
        let d = &self.dynamics;
        let mut random = Random::new(d.seed, self.index, k);
        let mut brush = base.clone();
        let (r_size, r_opacity, r_hue, r_angle, r_radius) = (
            random.next(),
            random.next(),
            random.next(),
            random.next(),
            random.next(),
        );
        brush.diameter *= 1.0 - d.size_jitter * r_size;
        brush.opacity *= 1.0 - d.opacity_jitter * r_opacity;
        if d.hue_jitter > 0.0 {
            brush.color = turn_hue(brush.color, (r_hue * 2.0 - 1.0) * d.hue_jitter * 0.5);
        }
        let mut point = centre;
        if d.scatter > 0.0 {
            let angle = r_angle * std::f32::consts::TAU;
            let reach = d.scatter * base.diameter * r_radius.sqrt();
            point = Point::new(
                centre.x + angle.cos() * reach,
                centre.y + angle.sin() * reach,
            );
        }
        Piece::Dab(point, brush)
    }
}

/// The flow of each of the dabs `step` (a fraction of the diameter) apart
/// that together lay down `flow` in one pass. A pixel on the path is under
/// about `1 / step` dabs, each moving it `flow` of the rest of the way, so
/// the stroke's flow does not depend on the spacing. Dabs a diameter or
/// more apart each take the whole flow.
pub(crate) fn dab_flow(flow: f32, step: f32) -> f32 {
    if flow >= 1.0 || step >= 1.0 {
        return flow;
    }
    1.0 - (1.0 - flow.max(0.0)).powf(step.max(0.0))
}

/// A small deterministic random sequence (SplitMix64).
struct Random(u64);

impl Random {
    fn new(seed: u64, index: u64, k: u32) -> Self {
        let seed = Self(seed).bits();
        let index = Self(seed ^ index).bits();
        Self(index ^ u64::from(k))
    }

    fn bits(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in [0, 1).
    fn next(&mut self) -> f32 {
        (self.bits() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The colour with its hue turned by `turns` of the colour wheel.
fn turn_hue(color: [u8; 4], turns: f32) -> [u8; 4] {
    let [r, g, b] = [0, 1, 2].map(|i| f32::from(color[i]) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let chroma = max - min;
    if chroma <= 0.0 {
        return color;
    }
    let hue = if max == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    } / 6.0;
    let hue = (hue + turns).rem_euclid(1.0) * 6.0;
    let x = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let channel = |v: f32| ((v + min) * 255.0).round().clamp(0.0, 255.0) as u8;
    [channel(r), channel(g), channel(b), color[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hue_turns_keep_grey_and_wrap_around() {
        assert_eq!(turn_hue([128, 128, 128, 200], 0.3), [128, 128, 128, 200]);
        assert_eq!(turn_hue([255, 0, 0, 255], 1.0 / 3.0), [0, 255, 0, 255]);
        assert_eq!(turn_hue([255, 0, 0, 255], -1.0 / 3.0), [0, 0, 255, 255]);
        assert_eq!(turn_hue([200, 100, 50, 9], 0.0), [200, 100, 50, 9]);
    }

    #[test]
    fn dabs_follow_the_spacing_across_segments() {
        let dynamics = Dynamics {
            spacing: 1.0,
            ..Dynamics::default()
        };
        let brush = Brush {
            diameter: 10.0,
            dynamics,
            ..Brush::default()
        };
        let mut walker = Walker::new(dynamics, None);
        let mut pieces = Vec::new();
        let points = [0.0, 0.0, 7.0, 25.0, 31.0].map(|x| Point::new(x, 0.0));
        for pair in points.windows(2) {
            walker.walk([pair[0], pair[1]], [&brush, &brush], &mut pieces);
        }
        let xs: Vec<f32> = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Dab(point, _) => point.x.round(),
                Piece::Segment(..) => panic!("expected dabs"),
            })
            .collect();
        assert_eq!(xs, [0.0, 10.0, 20.0, 30.0]);
    }
}

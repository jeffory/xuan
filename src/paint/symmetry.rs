//! Paint symmetry: a stroke painted again mirrored across an axis, or
//! rotated around a centre into N segments.
//!
//! Symmetry works on the pieces a stroke is laid out into (swept segments or
//! dabs, see [`super::dynamics`]): every piece is painted once for each copy,
//! moved by that copy's transform. The dab layout, taper, scatter and jitter
//! are worked out once along the drawn path, so each copy is an exact
//! mirror or rotation of it, including its random scatter and jitter. All
//! copies share the stroke's coverage, so where they overlap a pixel takes
//! the strongest coverage rather than being painted twice.

use super::{Brush, Point};

/// How a stroke is repeated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SymmetryMode {
    /// One stroke, as drawn.
    #[default]
    Off,
    /// Mirrored across a vertical axis through the centre (left ↔ right).
    Vertical,
    /// Mirrored across a horizontal axis through the centre (top ↔ bottom).
    Horizontal,
    /// Rotated around the centre into `segments` equal turns.
    Radial,
}

/// Paint symmetry for the Brush, Pencil and Eraser. Off by default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Symmetry {
    pub mode: SymmetryMode,
    /// Copies around the centre in radial mode, 2 to 32.
    pub segments: u32,
    /// The centre (or the point the axis goes through) in document pixels;
    /// `None` is the middle of the canvas.
    pub center: Option<Point>,
}

impl Default for Symmetry {
    fn default() -> Self {
        Self {
            mode: SymmetryMode::Off,
            segments: 6,
            center: None,
        }
    }
}

/// The fewest radial segments.
pub const MIN_SEGMENTS: u32 = 2;
/// The most radial segments.
pub const MAX_SEGMENTS: u32 = 32;

impl Symmetry {
    pub fn active(&self) -> bool {
        self.mode != SymmetryMode::Off
    }

    /// How many times a stroke is painted, the drawn one included.
    pub fn count(&self) -> u32 {
        match self.mode {
            SymmetryMode::Off => 1,
            SymmetryMode::Vertical | SymmetryMode::Horizontal => 2,
            SymmetryMode::Radial => self.segments.clamp(MIN_SEGMENTS, MAX_SEGMENTS),
        }
    }

    /// The centre on a canvas of this size.
    pub fn center_in(&self, width: u32, height: u32) -> Point {
        self.center
            .unwrap_or(Point::new(width as f32 * 0.5, height as f32 * 0.5))
    }

    /// An error message when the segments or the centre are out of range.
    /// The centre must lie on the canvas.
    pub fn validate(&self, width: u32, height: u32) -> Result<(), String> {
        if !(MIN_SEGMENTS..=MAX_SEGMENTS).contains(&self.segments) {
            return Err(format!(
                "The symmetry segments must be between {MIN_SEGMENTS} and {MAX_SEGMENTS}"
            ));
        }
        if let Some(center) = self.center
            && !(center.x.is_finite()
                && center.y.is_finite()
                && (0.0..=width as f32).contains(&center.x)
                && (0.0..=height as f32).contains(&center.y))
        {
            return Err(format!(
                "The symmetry centre must be on the canvas, 0–{width} by 0–{height}"
            ));
        }
        Ok(())
    }

    /// The transforms of every copy, the drawn stroke (the identity) first,
    /// on a canvas of this size.
    pub fn copies(&self, width: u32, height: u32) -> Vec<Reflection> {
        let center = self.center_in(width, height);
        let identity = Reflection {
            center,
            matrix: IDENTITY,
        };
        match self.mode {
            SymmetryMode::Off => vec![identity],
            SymmetryMode::Vertical => vec![
                identity,
                Reflection {
                    center,
                    matrix: [-1.0, 0.0, 0.0, 1.0],
                },
            ],
            SymmetryMode::Horizontal => vec![
                identity,
                Reflection {
                    center,
                    matrix: [1.0, 0.0, 0.0, -1.0],
                },
            ],
            SymmetryMode::Radial => {
                let n = self.count();
                std::iter::once(identity)
                    .chain((1..n).map(|k| {
                        let angle = std::f64::consts::TAU * f64::from(k) / f64::from(n);
                        let (sin, cos) = (angle.sin() as f32, angle.cos() as f32);
                        Reflection {
                            center,
                            matrix: [cos, -sin, sin, cos],
                        }
                    }))
                    .collect()
            }
        }
    }
}

const IDENTITY: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// One copy of a symmetric stroke: a mirror or rotation around a centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reflection {
    center: Point,
    /// Row-major 2×2: `[a, b, c, d]` maps (x, y) to (ax + by, cx + dy).
    matrix: [f32; 4],
}

impl Reflection {
    pub fn identity(&self) -> bool {
        self.matrix == IDENTITY
    }

    /// Whether this copy mirrors x and y: a mirror, or a half turn (both).
    /// Other turns are neither.
    pub fn flips(&self) -> [bool; 2] {
        let [a, b, c, d] = self.matrix;
        if b.abs() < 1e-6 && c.abs() < 1e-6 {
            [a < 0.0, d < 0.0]
        } else {
            [false; 2]
        }
    }

    fn vector(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d] = self.matrix;
        [a * x + b * y, c * x + d * y]
    }

    /// Where a point of the drawn stroke lands in this copy.
    pub fn point(&self, point: Point) -> Point {
        if self.identity() {
            return point;
        }
        let [x, y] = self.vector([point.x - self.center.x, point.y - self.center.y]);
        Point::new(self.center.x + x, self.center.y + y)
    }

    /// The brush for this copy, its tilt turned with the stroke.
    pub fn brush(&self, brush: &Brush) -> Brush {
        let mut brush = brush.clone();
        if !self.identity() && brush.tilt != [0.0; 2] {
            brush.tilt = self.vector(brush.tilt);
        }
        brush
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_mirror_and_turn_around_the_centre() {
        let mirror = Symmetry {
            mode: SymmetryMode::Vertical,
            ..Symmetry::default()
        };
        let copies = mirror.copies(100, 60);
        assert_eq!(copies.len(), 2);
        assert_eq!(
            copies[0].point(Point::new(10.0, 7.0)),
            Point::new(10.0, 7.0)
        );
        assert_eq!(
            copies[1].point(Point::new(10.0, 7.0)),
            Point::new(90.0, 7.0)
        );
        let flip = Symmetry {
            mode: SymmetryMode::Horizontal,
            center: Some(Point::new(0.0, 20.0)),
            ..Symmetry::default()
        };
        assert_eq!(
            flip.copies(100, 60)[1].point(Point::new(10.0, 7.0)),
            Point::new(10.0, 33.0)
        );
        let radial = Symmetry {
            mode: SymmetryMode::Radial,
            segments: 4,
            center: Some(Point::new(50.0, 50.0)),
        };
        let points: Vec<_> = (radial.copies(100, 100).iter())
            .map(|copy| copy.point(Point::new(80.0, 50.0)))
            .map(|p| (p.x.round(), p.y.round()))
            .collect();
        assert_eq!(
            points,
            [(80.0, 50.0), (50.0, 80.0), (20.0, 50.0), (50.0, 20.0)]
        );
        // A tilted brush turns with its copy.
        let brush = Brush {
            tilt: [30.0, 0.0],
            ..Brush::default()
        };
        let turned = radial.copies(100, 100)[1].brush(&brush).tilt;
        assert!(turned[0].abs() < 1e-4 && (turned[1] - 30.0).abs() < 1e-4);
        assert_eq!(mirror.copies(100, 60)[1].brush(&brush).tilt, [-30.0, 0.0]);
    }

    #[test]
    fn segments_and_centre_are_checked() {
        let radial = |segments, center| Symmetry {
            mode: SymmetryMode::Radial,
            segments,
            center,
        };
        assert!(radial(2, None).validate(10, 10).is_ok());
        assert!(
            radial(32, Some(Point::new(10.0, 0.0)))
                .validate(10, 10)
                .is_ok()
        );
        assert!(radial(1, None).validate(10, 10).is_err());
        assert!(radial(33, None).validate(10, 10).is_err());
        assert!(
            radial(6, Some(Point::new(10.5, 5.0)))
                .validate(10, 10)
                .is_err()
        );
        assert!(
            radial(6, Some(Point::new(5.0, -1.0)))
                .validate(10, 10)
                .is_err()
        );
        assert!(
            radial(6, Some(Point::new(f32::NAN, 5.0)))
                .validate(10, 10)
                .is_err()
        );
        assert_eq!(radial(12, None).count(), 12);
        assert_eq!(Symmetry::default().count(), 1);
        assert_eq!(Symmetry::default().copies(4, 4).len(), 1);
    }
}

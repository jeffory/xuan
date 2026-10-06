//! An editable form of a [`VectorPath`], for the Pen tool: anchors with in and out handles
//! and corner or smooth flags, grouped into open or closed subpaths. Paths convert to it and
//! back without changing their shape; quadratic segments become the equivalent cubics.
use anyhow::Result;
use kurbo::{BezPath, CubicBez, Line, ParamCurve, ParamCurveNearest, PathEl, Point};

use crate::vector::VectorPath;

/// How close, relative to the handle lengths, two handles must be to opposite directions for
/// an anchor read from a path to count as smooth.
const SMOOTH_TOLERANCE: f64 = 1e-3;

/// One anchor point with its two handles. A handle at the anchor's own position is retracted:
/// the segment on that side leaves the anchor without a tangent of its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub point: Point,
    /// The control point of the segment arriving at this anchor.
    pub handle_in: Point,
    /// The control point of the segment leaving this anchor.
    pub handle_out: Point,
    /// Whether dragging one handle turns the other to keep the curve smooth.
    pub smooth: bool,
}

impl Anchor {
    /// A corner anchor with both handles retracted.
    pub fn corner(point: Point) -> Self {
        Self {
            point,
            handle_in: point,
            handle_out: point,
            smooth: false,
        }
    }

    /// A smooth anchor whose out handle is at `handle_out` and in handle mirrors it.
    pub fn smooth(point: Point, handle_out: Point) -> Self {
        Self {
            point,
            handle_in: point + (point - handle_out),
            handle_out,
            smooth: true,
        }
    }

    pub fn has_in(&self) -> bool {
        self.handle_in != self.point
    }

    pub fn has_out(&self) -> bool {
        self.handle_out != self.point
    }

    pub fn handle(&self, side: Side) -> Point {
        match side {
            Side::In => self.handle_in,
            Side::Out => self.handle_out,
        }
    }

    fn handle_mut(&mut self, side: Side) -> &mut Point {
        match side {
            Side::In => &mut self.handle_in,
            Side::Out => &mut self.handle_out,
        }
    }
}

/// Which of an anchor's two handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    In,
    Out,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Self::In => Self::Out,
            Self::Out => Self::In,
        }
    }
}

/// A run of anchors joined by segments, closed back to its first anchor or open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subpath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

impl Subpath {
    /// How many segments the subpath has: one between each pair of neighbours, and one back
    /// to the start when it is closed.
    pub fn segment_count(&self) -> usize {
        match self.anchors.len() {
            0 | 1 => 0,
            n if self.closed => n,
            n => n - 1,
        }
    }

    /// Segment `index`, from anchor `index` to the next, as a cubic (lines have their control
    /// points on the anchors).
    pub fn segment(&self, index: usize) -> CubicBez {
        let a = self.anchors[index];
        let b = self.anchors[(index + 1) % self.anchors.len()];
        CubicBez::new(a.point, a.handle_out, b.handle_in, b.point)
    }

    /// Whether segment `index` is straight: both its handles retracted.
    pub fn is_line(&self, index: usize) -> bool {
        let a = self.anchors[index];
        let b = self.anchors[(index + 1) % self.anchors.len()];
        !a.has_out() && !b.has_in()
    }

    fn push_to(&self, path: &mut BezPath) {
        let Some(first) = self.anchors.first() else {
            return;
        };
        path.move_to(first.point);
        for index in 0..self.segment_count() {
            let last = self.closed && index + 1 == self.anchors.len();
            if self.is_line(index) {
                // Z draws the closing line itself.
                if !last {
                    path.line_to(self.anchors[index + 1].point);
                }
            } else {
                let c = self.segment(index);
                path.curve_to(c.p1, c.p2, c.p3);
            }
        }
        if self.closed {
            path.close_path();
        }
    }
}

/// What a point near an editable path is over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Anchor {
        subpath: usize,
        anchor: usize,
    },
    Handle {
        subpath: usize,
        anchor: usize,
        side: Side,
    },
    /// The segment from anchor `segment` to the next, at curve parameter `t`.
    Segment {
        subpath: usize,
        segment: usize,
        t: f64,
    },
}

/// A path as anchors and handles, for editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditPath {
    pub subpaths: Vec<Subpath>,
}

impl EditPath {
    /// The anchors of `path`. A closed subpath whose last segment ends on its first point
    /// shares that anchor, so the start has the closing segment's in handle.
    pub fn from_bez(path: &BezPath) -> Self {
        let mut subpaths: Vec<Subpath> = Vec::new();
        let mut current: Option<Subpath> = None;
        // Where a segment drawn right after Z starts: the closed subpath's start.
        let mut restart = Point::ZERO;
        for element in path.elements() {
            if !matches!(element, PathEl::MoveTo(_) | PathEl::ClosePath) && current.is_none() {
                current = Some(Subpath {
                    anchors: vec![Anchor::corner(restart)],
                    closed: false,
                });
            }
            match *element {
                PathEl::MoveTo(p) => {
                    subpaths.extend(current.take());
                    current = Some(Subpath {
                        anchors: vec![Anchor::corner(p)],
                        closed: false,
                    });
                }
                PathEl::LineTo(p) => {
                    if let Some(sub) = &mut current {
                        sub.anchors.push(Anchor::corner(p));
                    }
                }
                PathEl::QuadTo(c, p) => {
                    if let Some(sub) = &mut current {
                        let last = sub.anchors.len() - 1;
                        let start = sub.anchors[last].point;
                        sub.anchors[last].handle_out = start + (c - start) * (2.0 / 3.0);
                        sub.anchors.push(Anchor {
                            handle_in: p + (c - p) * (2.0 / 3.0),
                            ..Anchor::corner(p)
                        });
                    }
                }
                PathEl::CurveTo(c1, c2, p) => {
                    if let Some(sub) = &mut current {
                        let last = sub.anchors.len() - 1;
                        sub.anchors[last].handle_out = c1;
                        sub.anchors.push(Anchor {
                            handle_in: c2,
                            ..Anchor::corner(p)
                        });
                    }
                }
                PathEl::ClosePath => {
                    if let Some(mut sub) = current.take() {
                        if sub.anchors.len() > 1
                            && sub.anchors.first().map(|a| a.point)
                                == sub.anchors.last().map(|a| a.point)
                        {
                            let last = sub.anchors.pop().unwrap();
                            sub.anchors[0].handle_in = last.handle_in;
                        }
                        sub.closed = true;
                        restart = sub.anchors[0].point;
                        subpaths.push(sub);
                    }
                }
            }
        }
        subpaths.extend(current);
        for sub in &mut subpaths {
            for anchor in &mut sub.anchors {
                anchor.smooth = is_smooth(anchor);
            }
        }
        Self { subpaths }
    }

    pub fn from_vector(path: &VectorPath) -> Self {
        Self::from_bez(path.bez())
    }

    pub fn to_bez(&self) -> BezPath {
        let mut path = BezPath::new();
        for sub in &self.subpaths {
            sub.push_to(&mut path);
        }
        path
    }

    /// The path as a validated [`VectorPath`].
    pub fn to_vector(&self) -> Result<VectorPath> {
        VectorPath::from_bez(self.to_bez())
    }

    pub fn is_empty(&self) -> bool {
        self.subpaths.iter().all(|s| s.anchors.is_empty())
    }

    pub fn anchor(&self, subpath: usize, anchor: usize) -> Option<&Anchor> {
        self.subpaths.get(subpath)?.anchors.get(anchor)
    }

    /// What lies within `radius` of `point`: handles first (they sit on top), then anchors,
    /// then the nearest segment. `handles` limits handle hits to those shown.
    pub fn hit(&self, point: Point, radius: f64, handles: impl Fn(usize, usize) -> bool) -> Option<Hit> {
        let mut best: Option<(f64, Hit)> = None;
        let mut consider = |distance: f64, hit: Hit| {
            if distance <= radius && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, hit));
            }
        };
        for (s, sub) in self.subpaths.iter().enumerate() {
            for (a, anchor) in sub.anchors.iter().enumerate() {
                if !handles(s, a) {
                    continue;
                }
                for side in [Side::In, Side::Out] {
                    let handle = anchor.handle(side);
                    if handle != anchor.point {
                        consider(
                            handle.distance(point),
                            Hit::Handle {
                                subpath: s,
                                anchor: a,
                                side,
                            },
                        );
                    }
                }
            }
        }
        if best.is_some() {
            return best.map(|(_, hit)| hit);
        }
        for (s, sub) in self.subpaths.iter().enumerate() {
            for (a, anchor) in sub.anchors.iter().enumerate() {
                consider(
                    anchor.point.distance(point),
                    Hit::Anchor {
                        subpath: s,
                        anchor: a,
                    },
                );
            }
        }
        if best.is_some() {
            return best.map(|(_, hit)| hit);
        }
        for (s, sub) in self.subpaths.iter().enumerate() {
            for segment in 0..sub.segment_count() {
                let nearest = if sub.is_line(segment) {
                    let c = sub.segment(segment);
                    Line::new(c.p0, c.p3).nearest(point, 1e-6)
                } else {
                    sub.segment(segment).nearest(point, 1e-6)
                };
                consider(
                    nearest.distance_sq.sqrt(),
                    Hit::Segment {
                        subpath: s,
                        segment,
                        t: nearest.t,
                    },
                );
            }
        }
        best.map(|(_, hit)| hit)
    }

    /// Move an anchor, with its handles, to `to`.
    pub fn move_anchor(&mut self, subpath: usize, anchor: usize, to: Point) {
        let a = &mut self.subpaths[subpath].anchors[anchor];
        let delta = to - a.point;
        a.point = to;
        a.handle_in += delta;
        a.handle_out += delta;
    }

    /// Move one handle to `to`. On a smooth anchor the other handle turns to stay opposite,
    /// keeping its length (or mirroring this one when it had none), unless `independent`
    /// breaks the symmetry, which makes the anchor a corner.
    pub fn move_handle(&mut self, subpath: usize, anchor: usize, side: Side, to: Point, independent: bool) {
        let a = &mut self.subpaths[subpath].anchors[anchor];
        *a.handle_mut(side) = to;
        if independent {
            a.smooth = false;
            return;
        }
        if !a.smooth {
            return;
        }
        let direction = a.point - to;
        let other = a.handle(side.opposite());
        let length = (other - a.point).hypot();
        let opposite = if length > 0.0 && direction.hypot() > 0.0 {
            a.point + direction.normalize() * length
        } else {
            a.point + direction
        };
        *a.handle_mut(side.opposite()) = opposite;
    }

    /// Pull symmetric handles out of an anchor, the out handle at `to`; the anchor becomes smooth.
    pub fn pull_handles(&mut self, subpath: usize, anchor: usize, to: Point) {
        let a = &mut self.subpaths[subpath].anchors[anchor];
        *a = Anchor::smooth(a.point, to);
    }

    /// Split segment `segment` of `subpath` at curve parameter `t`, keeping the curve's shape
    /// exactly; the new anchor's index.
    pub fn split(&mut self, subpath: usize, segment: usize, t: f64) -> usize {
        let sub = &mut self.subpaths[subpath];
        let t = t.clamp(0.0, 1.0);
        let next = (segment + 1) % sub.anchors.len();
        let insert = segment + 1;
        if sub.is_line(segment) {
            let c = sub.segment(segment);
            let point = c.p0.lerp(c.p3, t);
            sub.anchors.insert(insert, Anchor::corner(point));
            return insert;
        }
        let c = sub.segment(segment);
        let left = c.subsegment(0.0..t);
        let right = c.subsegment(t..1.0);
        sub.anchors[segment].handle_out = left.p1;
        sub.anchors[next].handle_in = right.p2;
        sub.anchors.insert(
            insert,
            Anchor {
                point: left.p3,
                handle_in: left.p2,
                handle_out: right.p1,
                smooth: true,
            },
        );
        insert
    }

    /// Remove an anchor; its neighbours keep their handles. A subpath left without a segment
    /// is removed with it.
    pub fn delete_anchor(&mut self, subpath: usize, anchor: usize) {
        let sub = &mut self.subpaths[subpath];
        sub.anchors.remove(anchor);
        if sub.anchors.len() < 2 {
            self.subpaths.remove(subpath);
        }
    }

    /// Make a smooth anchor (or any anchor with handles) a corner with its handles retracted,
    /// or a corner a smooth anchor with handles along the line between its neighbours, a third
    /// of the way to each.
    pub fn convert(&mut self, subpath: usize, anchor: usize) {
        let sub = &mut self.subpaths[subpath];
        let n = sub.anchors.len();
        let a = sub.anchors[anchor];
        if a.smooth || a.has_in() || a.has_out() {
            sub.anchors[anchor] = Anchor::corner(a.point);
            return;
        }
        let previous = (anchor > 0 || sub.closed).then(|| sub.anchors[(anchor + n - 1) % n].point);
        let next = (anchor + 1 < n || sub.closed).then(|| sub.anchors[(anchor + 1) % n].point);
        let direction = match (previous, next) {
            (Some(p), Some(q)) => q - p,
            (Some(p), None) => a.point - p,
            (None, Some(q)) => q - a.point,
            (None, None) => return,
        };
        if direction.hypot() == 0.0 {
            return;
        }
        let unit = direction.normalize();
        let before = previous.map_or(0.0, |p| p.distance(a.point) / 3.0);
        let after = next.map_or(0.0, |q| q.distance(a.point) / 3.0);
        sub.anchors[anchor] = Anchor {
            point: a.point,
            handle_in: a.point - unit * before,
            handle_out: a.point + unit * after,
            smooth: true,
        };
    }
}

/// Whether both handles are out and point in opposite directions.
fn is_smooth(anchor: &Anchor) -> bool {
    if !anchor.has_in() || !anchor.has_out() {
        return false;
    }
    let a = anchor.handle_in - anchor.point;
    let b = anchor.handle_out - anchor.point;
    let cross = a.cross(b).abs();
    cross <= SMOOTH_TOLERANCE * a.hypot() * b.hypot() && a.dot(b) < 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(path: &BezPath, n: usize) -> Vec<Point> {
        let mut points = Vec::new();
        for seg in path.segments() {
            for i in 0..=n {
                points.push(seg.eval(i as f64 / n as f64));
            }
        }
        points
    }

    fn close_to(path: &BezPath, point: Point) -> bool {
        path.segments()
            .any(|seg| seg.nearest(point, 1e-9).distance_sq.sqrt() < 1e-6)
    }

    #[test]
    fn paths_round_trip_through_anchors() {
        for d in [
            "M 0 0 L 10 0 L 10 10",
            "M 0 0 L 10 0 L 10 10 Z",
            "M 0 0 C 0 10 10 10 10 0 C 10 -10 0 -10 0 0 Z",
            "M 0 0 C 5 5 10 5 15 0 L 20 20 Z M 30 30 L 40 40",
        ] {
            let path = VectorPath::parse(d).unwrap();
            let edit = EditPath::from_vector(&path);
            let back = edit.to_bez();
            assert_eq!(
                VectorPath::from_bez(back).unwrap().to_svg(),
                path.to_svg(),
                "{d}"
            );
        }
        let closed = EditPath::from_vector(
            &VectorPath::parse("M 0 0 C 0 10 10 10 10 0 C 10 -10 0 -10 0 0 Z").unwrap(),
        );
        assert_eq!(closed.subpaths[0].anchors.len(), 2);
        assert!(closed.subpaths[0].closed);
        assert!(closed.subpaths[0].anchors.iter().all(|a| a.smooth));
    }

    #[test]
    fn quadratics_become_the_same_cubics() {
        let path = VectorPath::parse("M 0 0 Q 10 20 20 0").unwrap();
        let edit = EditPath::from_vector(&path);
        let back = edit.to_bez();
        for point in sample(path.bez(), 16) {
            assert!(close_to(&back, point), "{point:?}");
        }
    }

    #[test]
    fn splitting_keeps_the_curve() {
        let path = VectorPath::parse("M 0 0 C 0 40 60 40 60 0 L 60 -20").unwrap();
        let mut edit = EditPath::from_vector(&path);
        let index = edit.split(0, 0, 0.3);
        assert_eq!(index, 1);
        edit.split(0, 2, 0.5);
        assert_eq!(edit.subpaths[0].anchors.len(), 5);
        let after = edit.to_bez();
        for point in sample(path.bez(), 32) {
            assert!(close_to(&after, point), "{point:?}");
        }
        for point in sample(&after, 32) {
            assert!(close_to(path.bez(), point), "{point:?}");
        }
        assert!(edit.subpaths[0].anchors[1].smooth);
        assert_eq!(edit.subpaths[0].anchors[3].point, Point::new(60.0, -10.0));
    }

    #[test]
    fn handles_move_symmetrically_unless_independent() {
        let mut edit = EditPath {
            subpaths: vec![Subpath {
                anchors: vec![
                    Anchor::corner(Point::new(0.0, 0.0)),
                    Anchor::smooth(Point::new(10.0, 0.0), Point::new(14.0, 0.0)),
                    Anchor::corner(Point::new(20.0, 0.0)),
                ],
                closed: false,
            }],
        };
        edit.move_handle(0, 1, Side::Out, Point::new(10.0, 8.0), false);
        let a = edit.subpaths[0].anchors[1];
        assert!((a.handle_in - Point::new(10.0, -4.0)).hypot() < 1e-9, "{a:?}");
        assert!(a.smooth);
        edit.move_handle(0, 1, Side::In, Point::new(5.0, 0.0), true);
        let a = edit.subpaths[0].anchors[1];
        assert_eq!(a.handle_out, Point::new(10.0, 8.0));
        assert!(!a.smooth);
    }

    #[test]
    fn anchors_are_deleted_and_converted() {
        let mut edit = EditPath::from_vector(&VectorPath::parse("M 0 0 L 10 0 L 20 0 L 20 10").unwrap());
        edit.convert(0, 1);
        let a = edit.subpaths[0].anchors[1];
        assert!(a.smooth);
        assert_eq!(a.handle_out, Point::new(10.0 + 10.0 / 3.0, 0.0));
        edit.convert(0, 1);
        assert!(!edit.subpaths[0].anchors[1].has_out());
        edit.delete_anchor(0, 1);
        assert_eq!(edit.to_bez().to_svg(), "M0 0L20 0L20 10");
        edit.delete_anchor(0, 0);
        edit.delete_anchor(0, 0);
        assert!(edit.is_empty());
    }

    #[test]
    fn hits_prefer_handles_then_anchors_then_segments() {
        let edit = EditPath {
            subpaths: vec![Subpath {
                anchors: vec![
                    Anchor::smooth(Point::new(0.0, 0.0), Point::new(0.0, 10.0)),
                    Anchor::corner(Point::new(20.0, 0.0)),
                ],
                closed: false,
            }],
        };
        let all = |_, _| true;
        assert!(matches!(
            edit.hit(Point::new(0.5, 9.0), 3.0, all),
            Some(Hit::Handle { side: Side::Out, .. })
        ));
        assert!(matches!(edit.hit(Point::new(19.0, 1.0), 3.0, all), Some(Hit::Anchor { anchor: 1, .. })));
        let on = edit.subpaths[0].segment(0).eval(0.5);
        assert!(matches!(edit.hit(on, 3.0, all), Some(Hit::Segment { segment: 0, .. })));
        assert_eq!(edit.hit(Point::new(50.0, 50.0), 3.0, all), None);
    }
}

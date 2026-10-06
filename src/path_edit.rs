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
    pub fn hit(
        &self,
        point: Point,
        radius: f64,
        handles: impl Fn(usize, usize) -> bool,
    ) -> Option<Hit> {
        let mut best: Option<(f64, Hit)> = None;
        let consider = |best: &mut Option<(f64, Hit)>, distance: f64, hit: Hit| {
            if distance <= radius && best.is_none_or(|(d, _)| distance < d) {
                *best = Some((distance, hit));
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
                            &mut best,
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
                    &mut best,
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
                    &mut best,
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
    pub fn move_handle(
        &mut self,
        subpath: usize,
        anchor: usize,
        side: Side,
        to: Point,
        independent: bool,
    ) {
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

/// The outline of a selection mask's selected half (values of 128 and up), as Photoshop's
/// Make Work Path: closed polygons along the pixel edges, simplified so that they stray at most
/// `tolerance` pixels from them. Holes wind the other way, so the nonzero rule keeps them open.
/// The tolerance doubles until the path fits [`crate::vector::MAX_SEGMENTS`]. `None` when
/// nothing is selected.
pub fn trace_mask(mask: &image::GrayImage, tolerance: f64) -> Option<VectorPath> {
    let (width, height) = (mask.width() as i64, mask.height() as i64);
    let inside = |x: i64, y: i64| {
        x >= 0 && y >= 0 && x < width && y < height && mask.get_pixel(x as u32, y as u32)[0] >= 128
    };
    // Directed pixel edges with the selection on their right (clockwise on screen).
    let mut edges: std::collections::HashMap<(i64, i64), Vec<(i64, i64)>> =
        std::collections::HashMap::new();
    for y in 0..height {
        for x in 0..width {
            if !inside(x, y) {
                continue;
            }
            let mut edge = |from, to| edges.entry(from).or_default().push(to);
            if !inside(x, y - 1) {
                edge((x, y), (x + 1, y));
            }
            if !inside(x + 1, y) {
                edge((x + 1, y), (x + 1, y + 1));
            }
            if !inside(x, y + 1) {
                edge((x + 1, y + 1), (x, y + 1));
            }
            if !inside(x - 1, y) {
                edge((x, y + 1), (x, y));
            }
        }
    }
    // Chain the edges into loops; every corner has as many edges in as out, so each closes.
    let mut starts: Vec<(i64, i64)> = edges.keys().copied().collect();
    starts.sort_unstable_by_key(|&(x, y)| (y, x));
    let mut loops: Vec<Vec<Point>> = Vec::new();
    for start in starts {
        while edges.get(&start).is_some_and(|e| !e.is_empty()) {
            let mut points = vec![start];
            let mut at = start;
            while let Some(next) = edges.get_mut(&at).and_then(|e| e.pop()) {
                at = next;
                if at == start {
                    break;
                }
                points.push(at);
            }
            loops.push(
                points
                    .into_iter()
                    .map(|(x, y)| Point::new(x as f64, y as f64))
                    .collect(),
            );
        }
    }
    if loops.is_empty() {
        return None;
    }
    let mut tolerance = tolerance.max(0.0);
    loop {
        let mut path = BezPath::new();
        let mut segments = 0;
        for points in &loops {
            let simple = simplify_loop(points, tolerance);
            if simple.len() < 3 {
                continue;
            }
            segments += simple.len();
            path.move_to(simple[0]);
            for &p in &simple[1..] {
                path.line_to(p);
            }
            path.close_path();
        }
        if segments <= crate::vector::MAX_SEGMENTS || tolerance > 1e6 {
            return VectorPath::from_bez(path).ok().filter(|p| !p.is_empty());
        }
        tolerance = (tolerance * 2.0).max(1.0);
    }
}

/// A closed polygon with points removed while it stays within `tolerance` of the original
/// (Ramer–Douglas–Peucker, split at the point farthest from the first).
fn simplify_loop(points: &[Point], tolerance: f64) -> Vec<Point> {
    if points.len() < 4 {
        return points.to_vec();
    }
    let far = (1..points.len())
        .max_by(|&a, &b| {
            let da = points[a].distance_squared(points[0]);
            let db = points[b].distance_squared(points[0]);
            da.total_cmp(&db)
        })
        .unwrap_or(1);
    let mut kept = vec![false; points.len() + 1];
    kept[0] = true;
    kept[far] = true;
    kept[points.len()] = true;
    let at = |i: usize| points[i % points.len()];
    let mut stack = vec![(0, far), (far, points.len())];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let line = Line::new(at(a), at(b));
        let (index, distance) = (a + 1..b)
            .map(|i| {
                let p = at(i);
                let d = if line.p0 == line.p1 {
                    p.distance(line.p0)
                } else {
                    line.nearest(p, 1e-9).distance_sq.sqrt()
                };
                (i, d)
            })
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .unwrap();
        if distance > tolerance {
            kept[index] = true;
            stack.push((a, index));
            stack.push((index, b));
        }
    }
    (0..points.len()).filter(|&i| kept[i]).map(at).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_trace_to_outlines_with_holes() {
        let mut mask = image::GrayImage::new(20, 20);
        for y in 2..18 {
            for x in 2..18 {
                let hole = (8..12).contains(&x) && (8..12).contains(&y);
                if !hole {
                    mask.put_pixel(x, y, image::Luma([255]));
                }
            }
        }
        let path = trace_mask(&mask, 0.5).unwrap();
        let edit = EditPath::from_vector(&path);
        assert_eq!(edit.subpaths.len(), 2);
        assert!(
            edit.subpaths
                .iter()
                .all(|s| s.closed && s.anchors.len() == 4)
        );
        // Filled back, it is the same selection.
        let filled = path.mask(crate::vector::FillRule::Nonzero, 20, 20);
        assert_eq!(filled.as_raw(), mask.as_raw());
        assert!(trace_mask(&image::GrayImage::new(4, 4), 1.0).is_none());
        // A circle simplifies to far fewer points than its pixel steps.
        let circle = image::GrayImage::from_fn(64, 64, |x, y| {
            let d = (x as f32 - 31.5).hypot(y as f32 - 31.5);
            image::Luma([if d < 25.0 { 255 } else { 0 }])
        });
        let traced = EditPath::from_vector(&trace_mask(&circle, 1.0).unwrap());
        assert_eq!(traced.subpaths.len(), 1);
        assert!(
            traced.subpaths[0].anchors.len() < 60,
            "{}",
            traced.subpaths[0].anchors.len()
        );
    }

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
        assert!(
            (a.handle_in - Point::new(10.0, -4.0)).hypot() < 1e-9,
            "{a:?}"
        );
        assert!(a.smooth);
        edit.move_handle(0, 1, Side::In, Point::new(5.0, 0.0), true);
        let a = edit.subpaths[0].anchors[1];
        assert_eq!(a.handle_out, Point::new(10.0, 8.0));
        assert!(!a.smooth);
    }

    #[test]
    fn anchors_are_deleted_and_converted() {
        let mut edit =
            EditPath::from_vector(&VectorPath::parse("M 0 0 L 10 0 L 20 0 L 20 10").unwrap());
        edit.convert(0, 1);
        let a = edit.subpaths[0].anchors[1];
        assert!(a.smooth);
        assert_eq!(a.handle_out, Point::new(10.0 + 10.0 / 3.0, 0.0));
        edit.convert(0, 1);
        assert!(!edit.subpaths[0].anchors[1].has_out());
        edit.delete_anchor(0, 1);
        assert_eq!(edit.to_bez().to_svg(), "M0,0 L20,0 L20,10");
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
            Some(Hit::Handle {
                side: Side::Out,
                ..
            })
        ));
        assert!(matches!(
            edit.hit(Point::new(19.0, 1.0), 3.0, all),
            Some(Hit::Anchor { anchor: 1, .. })
        ));
        let on = edit.subpaths[0].segment(0).eval(0.5);
        assert!(matches!(
            edit.hit(on, 3.0, all),
            Some(Hit::Segment { segment: 0, .. })
        ));
        assert_eq!(edit.hit(Point::new(50.0, 50.0), 3.0, all), None);
    }
}

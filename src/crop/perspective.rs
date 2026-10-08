//! Perspective Crop: four corners placed on something photographed at an angle (a page, a
//! screen, a sign) and the projective map that straightens them into an upright rectangle, as
//! Photoshop's Perspective Crop does. Plain data and pure functions: the app draws the corners and
//! [`crate::operations::perspective_crop`] gives every layer the map.
use crate::{
    crop::CropBox,
    document::{Point, Transform},
    geometry::Homography,
};

/// The corners as the canvas shows them: top-left, top-right, bottom-right, bottom-left.
pub type Quad = [Point; 4];

/// The unit square's corners, in the order of a [`Quad`].
const UNIT: Quad = [
    Point::new(0.0, 0.0),
    Point::new(1.0, 0.0),
    Point::new(1.0, 1.0),
    Point::new(0.0, 1.0),
];

/// Smallest area, in square canvas pixels, of a quad that can be straightened.
pub const MIN_AREA: f32 = 16.0;
/// Shortest side, in canvas pixels.
pub const MIN_SIDE: f32 = 2.0;
/// The sine of the smallest turn at a corner: one turning by less than about half a degree lies
/// on a straight line between its neighbours.
const MIN_TURN: f32 = 0.01;
/// How near the vanishing line a point may come, as a fraction of the divisor in the middle of the
/// quad; nearer, it would land thousands of times further away than the quad is wide.
const MIN_DIVISOR: f32 = 1e-3;
/// How far from the unit square, in units of the layer, a composed warp can be and still be dropped.
const UNWARPED: f32 = 1e-4;

/// Why four corners cannot be straightened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Degenerate {
    /// Two sides cross, as in a bow tie.
    SelfIntersecting,
    /// A corner points inwards or lies on a straight side.
    Concave,
    /// The corners are (nearly) on top of each other.
    TooSmall,
}

impl Degenerate {
    /// What to tell the user, in English (the app translates it).
    pub fn message(self) -> &'static str {
        match self {
            Self::SelfIntersecting => {
                "The crop's sides cross. Drag the corners so they go round the shape in order."
            }
            Self::Concave => {
                "The crop's corners must make a convex shape, with no corner pointing inwards or lying on a straight side."
            }
            Self::TooSmall => "The crop's corners are too close together to straighten.",
        }
    }
}

fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

fn cross(a: Point, b: Point) -> f32 {
    a.x * b.y - a.y * b.x
}

fn length(a: Point) -> f32 {
    a.x.hypot(a.y)
}

/// The signed area (positive when the corners go clockwise on the canvas).
pub fn area(quad: Quad) -> f32 {
    (0..4)
        .map(|i| cross(quad[i], quad[(i + 1) % 4]))
        .sum::<f32>()
        * 0.5
}

/// The middle of the corners.
pub fn centroid(quad: Quad) -> Point {
    let sum = quad
        .iter()
        .fold(Point::default(), |s, p| Point::new(s.x + p.x, s.y + p.y));
    Point::new(sum.x / 4.0, sum.y / 4.0)
}

/// Whether the corners can be straightened: they make a convex shape of some size whose sides
/// do not cross. Corners going round anticlockwise are allowed and mirror the result.
pub fn check(quad: Quad) -> Result<(), Degenerate> {
    if !quad.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
        return Err(Degenerate::TooSmall);
    }
    let sides: [Point; 4] = std::array::from_fn(|i| sub(quad[(i + 1) % 4], quad[i]));
    if sides.iter().any(|&side| length(side) < MIN_SIDE) {
        return Err(Degenerate::TooSmall);
    }
    // The sine of the turn at each corner.
    let turns: [f32; 4] = std::array::from_fn(|i| {
        let (a, b) = (sides[i], sides[(i + 1) % 4]);
        cross(a, b) / (length(a) * length(b))
    });
    let left = turns.iter().filter(|&&t| t > 0.0).count();
    // A bow tie turns one way twice and the other way twice; a dart turns back once.
    if left == 2 {
        return Err(Degenerate::SelfIntersecting);
    }
    if left == 1 || left == 3 || turns.iter().any(|t| t.abs() < MIN_TURN) {
        return Err(Degenerate::Concave);
    }
    if area(quad).abs() < MIN_AREA || Homography::from_quad(quad).is_none() {
        return Err(Degenerate::TooSmall);
    }
    Ok(())
}

/// The straightened canvas size: the average of the top and bottom sides by the average of the
/// left and right ones, rounded to whole pixels.
pub fn output_size(quad: Quad) -> [u32; 2] {
    let side = |a: usize, b: usize| quad[a].distance(quad[b]);
    let width = (side(0, 1) + side(3, 2)) * 0.5;
    let height = (side(0, 3) + side(1, 2)) * 0.5;
    [width, height].map(|v| v.round().clamp(1.0, u32::MAX as f32) as u32)
}

/// The map from canvas pixels to the straightened `width` × `height` canvas: the quad's corners
/// go to the new canvas's corners. Scaled so its divisor is 1 in the middle of the quad, which
/// [`map_point`] relies on.
pub fn rectify(quad: Quad, [width, height]: [u32; 2]) -> Option<Homography> {
    let map = Homography::from_quad(quad)?
        .inverse()?
        .then(Homography::scale(width as f32, height as f32));
    let divisor = map.divisor(centroid(quad));
    (divisor.is_finite() && divisor.abs() > f32::EPSILON)
        .then(|| Homography(map.0.map(|row| row.map(|v| v / divisor))))
}

/// Where `map` (from [`rectify`]) takes `point`, unless the point is on or beyond the vanishing
/// line, where it would land at infinity or on the far side, mirrored.
pub fn map_point(map: Homography, point: Point) -> Option<Point> {
    (map.divisor(point) > MIN_DIVISOR)
        .then(|| map.map(point))
        .filter(|p| p.x.is_finite() && p.y.is_finite())
}

/// `transform` followed by `map`: where a layer (or mask) placed by `transform` lands on the
/// straightened canvas. Its pixels stay as they are; the map goes into the transform, its
/// projective part into the warp. The layer keeps its rotation and flips, turned with the map, and
/// a map that only moves and scales it (a quad that is already an upright rectangle) adds no warp.
/// `None` when part of the layer reaches the vanishing line or the result is out of range.
pub fn map_transform(transform: Transform, map: Homography) -> Option<Transform> {
    let corners = transform.corners();
    let mapped = [
        map_point(map, corners[0])?,
        map_point(map, corners[1])?,
        map_point(map, corners[2])?,
        map_point(map, corners[3])?,
    ];
    let mut result = transform;
    result.warp = None;
    let centre = transform.center();
    let (sin, cos) = transform.rotation.to_radians().sin_cos();
    let (half_width, half_height) = (transform.width * 0.5, transform.height * 0.5);
    let along = |dx: f32, dy: f32| map_point(map, Point::new(centre.x + dx, centre.y + dy));
    // The box's own axes, mapped, when its middle and sides are on the near side of the
    // vanishing line (a warped layer's box can reach further than the layer); else its bounds.
    let axes = (|| {
        Some((
            along(0.0, 0.0)?,
            along(-cos * half_width, -sin * half_width)?,
            along(cos * half_width, sin * half_width)?,
            along(sin * half_height, -cos * half_height)?,
            along(-sin * half_height, cos * half_height)?,
        ))
    })();
    if let Some((middle, left, right, top, bottom)) = axes {
        result.width = left.distance(right);
        result.height = top.distance(bottom);
        result.rotation = (right.y - left.y).atan2(right.x - left.x).to_degrees();
        result.x = middle.x - result.width * 0.5;
        result.y = middle.y - result.height * 0.5;
    } else {
        let min = mapped.iter().fold(Point::new(f32::MAX, f32::MAX), |m, p| {
            Point::new(m.x.min(p.x), m.y.min(p.y))
        });
        let max = mapped.iter().fold(Point::new(f32::MIN, f32::MIN), |m, p| {
            Point::new(m.x.max(p.x), m.y.max(p.y))
        });
        (result.x, result.y) = (min.x, min.y);
        (result.width, result.height) = (max.x - min.x, max.y - min.y);
        result.rotation = 0.0;
    }
    if !(result.width.is_finite() && result.height.is_finite()) {
        return None;
    }
    result.width = result.width.max(1.0);
    result.height = result.height.max(1.0);
    let warp = mapped.map(|p| result.inverse(p));
    if warp
        .iter()
        .zip(UNIT)
        .any(|(corner, unit)| corner.distance(unit) > UNWARPED)
    {
        result.warp = Some(warp);
    }
    result.valid().then_some(result)
}

/// Where the corners start: at the selection's bounds, else the canvas inset by a tenth of each
/// side.
pub fn initial([width, height]: [u32; 2], selection: Option<CropBox>) -> Quad {
    let rect = selection
        .map(|b| b.clamped([width, height]))
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| {
            let (x, y) = (width / 10, height / 10);
            CropBox::new(x, y, width - 2 * x, height - 2 * y)
        });
    UNIT.map(|u| rect.point([u.x, u.y]))
}

/// Four clicked points as a [`Quad`]: round their middle clockwise, from the one nearest the
/// top-left.
pub fn order(points: [Point; 4]) -> Quad {
    let middle = centroid(points);
    let mut quad = points;
    quad.sort_by(|a, b| {
        let angle = |p: &Point| (p.y - middle.y).atan2(p.x - middle.x);
        angle(a).total_cmp(&angle(b))
    });
    let first = (0..4)
        .min_by(|&a, &b| (quad[a].x + quad[a].y).total_cmp(&(quad[b].x + quad[b].y)))
        .unwrap_or(0);
    quad.rotate_left(first);
    quad
}

/// The corner within `tolerance` of `point`, the nearest if several are.
pub fn corner_at(quad: Quad, point: Point, tolerance: f32) -> Option<usize> {
    (0..4)
        .map(|i| (i, quad[i].distance(point)))
        .filter(|&(_, distance)| distance < tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Whether `point` is inside the quad (crossing either way, so a bow tie still has an inside).
pub fn contains(quad: Quad, point: Point) -> bool {
    let mut inside = false;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        if (a.y > point.y) != (b.y > point.y)
            && point.x < a.x + (point.y - a.y) / (b.y - a.y) * (b.x - a.x)
        {
            inside = !inside;
        }
    }
    inside
}

/// The quad moved by `delta`.
pub fn moved(quad: Quad, delta: Point) -> Quad {
    quad.map(|p| Point::new(p.x + delta.x, p.y + delta.y))
}

/// The grid drawn over the quad: `divisions` - 1 lines each way, as the straightened canvas's
/// even divisions appear in the photo. Empty for corners that cannot be straightened.
pub fn grid(quad: Quad, divisions: u32) -> Vec<[Point; 2]> {
    let Some(map) = Homography::from_quad(quad) else {
        return Vec::new();
    };
    (1..divisions)
        .flat_map(|i| {
            let f = i as f32 / divisions as f32;
            [
                [Point::new(f, 0.0), Point::new(f, 1.0)],
                [Point::new(0.0, f), Point::new(1.0, f)],
            ]
        })
        .map(|line| line.map(|p| map.map(p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }

    /// A page shot from below and to the left: wider at the bottom, leaning right.
    const PAGE: Quad = [
        Point::new(62.0, 38.0),
        Point::new(231.0, 52.0),
        Point::new(262.0, 288.0),
        Point::new(31.0, 270.0),
    ];

    #[test]
    fn rectify_takes_the_corners_to_the_canvas_corners_and_keeps_lines_straight() {
        let map = rectify(PAGE, [210, 297]).unwrap();
        let targets = [p(0.0, 0.0), p(210.0, 0.0), p(210.0, 297.0), p(0.0, 297.0)];
        for (corner, target) in PAGE.into_iter().zip(targets) {
            assert!(map_point(map, corner).unwrap().distance(target) < 1e-3);
        }
        // A point a third of the way along a side lands on the straightened side.
        let on_top = Point::new(
            PAGE[0].x + (PAGE[1].x - PAGE[0].x) / 3.0,
            PAGE[0].y + (PAGE[1].y - PAGE[0].y) / 3.0,
        );
        assert!(map_point(map, on_top).unwrap().y.abs() < 1e-3);
        // The diagonals cross at the middle of the straightened canvas.
        let a = Homography::from_quad(PAGE).unwrap().map(p(0.5, 0.5));
        assert!(map_point(map, a).unwrap().distance(p(105.0, 148.5)) < 1e-3);
    }

    #[test]
    fn rectify_recovers_a_known_homography() {
        // A known projective map, applied to a 300 × 200 rectangle, makes the photo's quad.
        let known = Homography([[0.9, 0.12, 40.0], [-0.05, 1.1, 30.0], [0.0004, 0.0009, 1.0]]);
        let quad =
            [p(0.0, 0.0), p(300.0, 0.0), p(300.0, 200.0), p(0.0, 200.0)].map(|c| known.map(c));
        let map = rectify(quad, [300, 200]).unwrap();
        for x in [0.0, 37.0, 150.0, 299.0] {
            for y in [0.0, 81.0, 200.0] {
                let back = map_point(map, known.map(p(x, y))).unwrap();
                assert!(back.distance(p(x, y)) < 0.01, "{x},{y} -> {back:?}");
            }
        }
    }

    #[test]
    fn output_size_averages_opposite_sides() {
        let quad = [p(0.0, 0.0), p(100.0, 0.0), p(90.0, 50.0), p(10.0, 50.0)];
        // Top 100, bottom 80; left and right both √(10² + 50²) ≈ 50.99.
        assert_eq!(output_size(quad), [90, 51]);
        let upright = [p(5.0, 5.0), p(25.0, 5.0), p(25.0, 45.0), p(5.0, 45.0)];
        assert_eq!(output_size(upright), [20, 40]);
        assert_eq!(output_size(PAGE), [201, 236]);
    }

    #[test]
    fn degenerate_quads_are_refused() {
        assert_eq!(check(PAGE), Ok(()));
        // Anticlockwise corners mirror the result but are allowed.
        let mut mirrored = PAGE;
        mirrored.reverse();
        assert_eq!(check(mirrored), Ok(()));
        // A bow tie: the bottom corners swapped.
        let bow = [PAGE[0], PAGE[1], PAGE[3], PAGE[2]];
        assert_eq!(check(bow), Err(Degenerate::SelfIntersecting));
        // A dart: the bottom-right corner pulled in past the diagonal.
        let dart = [p(0.0, 0.0), p(100.0, 0.0), p(30.0, 30.0), p(0.0, 100.0)];
        assert_eq!(check(dart), Err(Degenerate::Concave));
        // A corner on the straight line between its neighbours.
        let flat = [p(0.0, 0.0), p(50.0, 0.0), p(100.0, 0.0), p(50.0, 80.0)];
        assert_eq!(check(flat), Err(Degenerate::Concave));
        // Tiny, or two corners on top of each other.
        let tiny = [p(0.0, 0.0), p(3.0, 0.0), p(3.0, 3.0), p(0.0, 3.0)];
        assert_eq!(check(tiny), Err(Degenerate::TooSmall));
        let doubled = [p(0.0, 0.0), p(0.0, 0.0), p(50.0, 50.0), p(0.0, 50.0)];
        assert_eq!(check(doubled), Err(Degenerate::TooSmall));
        let nan = [p(f32::NAN, 0.0), PAGE[1], PAGE[2], PAGE[3]];
        assert_eq!(check(nan), Err(Degenerate::TooSmall));
        for problem in [
            Degenerate::SelfIntersecting,
            Degenerate::Concave,
            Degenerate::TooSmall,
        ] {
            assert!(!problem.message().is_empty());
        }
    }

    #[track_caller]
    fn assert_composed(original: Transform, map: Homography) -> Transform {
        let result = map_transform(original, map).expect("a composed transform");
        for u in [0.0, 0.25, 0.5, 1.0] {
            for v in [0.0, 0.4, 1.0] {
                let expected = map.map(original.point(p(u, v)));
                let actual = result.point(p(u, v));
                assert!(
                    actual.distance(expected) < 0.01,
                    "({u}, {v}): {actual:?} != {expected:?}"
                );
            }
        }
        result
    }

    #[test]
    fn layer_transforms_compose_with_the_map() {
        let map = rectify(PAGE, [210, 297]).unwrap();
        // A canvas-sized layer, a turned and flipped one, and one already warped.
        let plain = Transform::new(300, 320);
        assert!(assert_composed(plain, map).warp.is_some());
        let turned = Transform {
            x: 80.0,
            y: 90.0,
            width: 60.0,
            height: 40.0,
            rotation: 30.0,
            flip_x: true,
            ..Transform::new(1, 1)
        };
        let result = assert_composed(turned, map);
        assert!(result.flip_x);
        let warped = Transform {
            warp: Some([p(0.1, 0.0), p(1.0, 0.1), p(0.9, 1.0), p(0.0, 0.8)]),
            ..turned
        };
        assert_composed(warped, map);
    }

    #[test]
    fn an_upright_quad_only_moves_and_scales_layers() {
        let quad = [p(10.0, 20.0), p(110.0, 20.0), p(110.0, 70.0), p(10.0, 70.0)];
        let map = rectify(quad, [200, 50]).unwrap();
        let layer = Transform {
            x: 30.0,
            y: 25.0,
            width: 40.0,
            height: 10.0,
            ..Transform::new(1, 1)
        };
        let result = assert_composed(layer, map);
        assert_eq!(result.warp, None);
        assert!((result.x - 40.0).abs() < 1e-3 && (result.y - 5.0).abs() < 1e-3);
        assert!((result.width - 80.0).abs() < 1e-3 && (result.height - 10.0).abs() < 1e-3);
        assert!(result.rotation.abs() < 1e-3);
    }

    #[test]
    fn a_layer_reaching_the_vanishing_line_cannot_be_mapped() {
        // Strong perspective: the top is a sixth of the bottom, so the sides meet just above.
        let quad = [
            p(250.0, 100.0),
            p(350.0, 100.0),
            p(600.0, 400.0),
            p(0.0, 400.0),
        ];
        let map = rectify(quad, [300, 300]).unwrap();
        let near = Transform {
            x: 200.0,
            y: 120.0,
            width: 200.0,
            height: 260.0,
            ..Transform::new(1, 1)
        };
        assert_composed(near, map);
        let tall = Transform {
            x: 0.0,
            y: -400.0,
            width: 600.0,
            height: 800.0,
            ..Transform::new(1, 1)
        };
        assert_eq!(map_transform(tall, map), None);
        assert_eq!(map_point(map, p(300.0, -400.0)), None);
    }

    #[test]
    fn initial_quad_insets_the_canvas_or_takes_the_selection() {
        assert_eq!(
            initial([200, 100], None),
            [p(20.0, 10.0), p(180.0, 10.0), p(180.0, 90.0), p(20.0, 90.0)]
        );
        assert_eq!(
            initial([200, 100], Some(CropBox::new(5, 6, 30, 40))),
            [p(5.0, 6.0), p(35.0, 6.0), p(35.0, 46.0), p(5.0, 46.0)]
        );
        // An empty selection box falls back to the inset canvas.
        assert_eq!(
            initial([200, 100], Some(CropBox::new(5, 6, 0, 40))),
            initial([200, 100], None)
        );
    }

    #[test]
    fn clicked_points_are_ordered_from_the_top_left_clockwise() {
        let clicks = [PAGE[2], PAGE[0], PAGE[3], PAGE[1]];
        assert_eq!(order(clicks), PAGE);
        assert_eq!(order(PAGE), PAGE);
    }

    #[test]
    fn corners_inside_and_moving() {
        assert_eq!(corner_at(PAGE, p(64.0, 40.0), 5.0), Some(0));
        assert_eq!(corner_at(PAGE, p(140.0, 160.0), 5.0), None);
        assert!(contains(PAGE, p(140.0, 160.0)));
        assert!(!contains(PAGE, p(40.0, 40.0)));
        let shifted = moved(PAGE, p(3.0, -2.0));
        assert_eq!(shifted[2], p(265.0, 286.0));
    }

    #[test]
    fn the_grid_follows_the_perspective() {
        let lines = grid(PAGE, 3);
        assert_eq!(lines.len(), 4);
        // Every line runs from one side to the opposite one.
        let map = rectify(PAGE, [300, 300]).unwrap();
        for line in &lines {
            let [a, b] = line.map(|q| map_point(map, q).unwrap());
            assert!(
                ((a.x - b.x).abs() < 1e-2 && [100.0, 200.0].iter().any(|x| (a.x - x).abs() < 1e-2))
                    || ((a.y - b.y).abs() < 1e-2
                        && [100.0, 200.0].iter().any(|y| (a.y - y).abs() < 1e-2)),
                "{a:?} {b:?}"
            );
        }
        let bow = [PAGE[0], PAGE[1], PAGE[3], PAGE[2]];
        assert!(grid(bow, 3).is_empty());
    }
}

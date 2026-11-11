//! The Crop tool's box: its aspect ratio choices and the arithmetic of drawing, resizing and
//! moving it in whole canvas pixels. Plain data and pure functions, so the canvas only draws them.
//!
//! A fixed ratio is kept to the pixel: the box is always a whole multiple of the reduced ratio
//! (9k × 16k for 9:16), so it steps in units of the ratio rather than rounding each side on its
//! own. The box never leaves the canvas.
use crate::document::Point;

pub mod perspective;

/// Ratios whose larger term is at most this are kept exactly. Larger ones (the canvas's own ratio
/// on an odd size such as 1001 × 997) would leave only the full canvas, so they round each side
/// to the nearest pixel instead.
pub const EXACT_TERMS: u32 = 100;

/// The fixed ratios of the Ratio menu, as width : height, in menu order.
pub const PRESETS: &[[u32; 2]] = &[
    [1, 1],
    [4, 3],
    [3, 4],
    [3, 2],
    [2, 3],
    [16, 9],
    [9, 16],
    [9, 20],
    [5, 4],
    [4, 5],
];

/// The Ratio menu's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ratio {
    /// Any shape.
    #[default]
    Free,
    /// The canvas's own proportions.
    Original,
    /// One of [`PRESETS`].
    Preset([u32; 2]),
    /// The W : H typed beside the menu.
    Custom,
}

impl Ratio {
    /// Every choice, in menu order.
    pub fn menu() -> impl Iterator<Item = Self> {
        [Self::Free, Self::Original]
            .into_iter()
            .chain(PRESETS.iter().map(|&terms| Self::Preset(terms)))
            .chain([Self::Custom])
    }

    /// The width and height terms the box keeps, reduced (16:9, never 1920:1080), or `None` for
    /// Free. `canvas` is the document's size and `custom` the typed W : H.
    pub fn terms(self, canvas: [u32; 2], custom: [u32; 2]) -> Option<[u32; 2]> {
        match self {
            Self::Free => None,
            Self::Original => reduced(canvas),
            Self::Preset(terms) => reduced(terms),
            Self::Custom => reduced(custom),
        }
    }

    /// The choice with width and height exchanged, and the custom terms to go with it: a preset
    /// whose terms are the swapped ones when there is one (16:9 ↔ 9:16), otherwise Custom. Free
    /// has nothing to swap.
    pub fn swapped(self, canvas: [u32; 2], custom: [u32; 2]) -> (Self, [u32; 2]) {
        let Some([width, height]) = self.terms(canvas, custom) else {
            return (self, custom);
        };
        let turned = [height, width];
        match PRESETS.iter().find(|&&terms| terms == turned) {
            Some(&terms) => (Self::Preset(terms), custom),
            None => (Self::Custom, turned),
        }
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// `terms` divided by their greatest common divisor, or `None` when either is zero.
pub fn reduced([width, height]: [u32; 2]) -> Option<[u32; 2]> {
    if width == 0 || height == 0 {
        return None;
    }
    let divisor = gcd(width, height);
    Some([width / divisor, height / divisor])
}

/// The crop box, in whole canvas pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CropBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A handle on the box, in the order of the Move tool's handles: clockwise from the top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    /// Where the handle sits on a unit box: 0, 0.5 or 1 along each axis.
    pub fn unit(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [0.0, 0.0],
            Self::Top => [0.5, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::Right => [1.0, 0.5],
            Self::BottomRight => [1.0, 1.0],
            Self::Bottom => [0.5, 1.0],
            Self::BottomLeft => [0.0, 1.0],
            Self::Left => [0.0, 0.5],
        }
    }

    pub fn is_corner(self) -> bool {
        let [x, y] = self.unit();
        x != 0.5 && y != 0.5
    }
}

/// What a press on the canvas lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Handle(Handle),
    Inside,
}

impl CropBox {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The box with corners at `min` and `max` (exclusive), as selection bounds are given.
    pub fn from_bounds((left, top, right, bottom): (u32, u32, u32, u32)) -> Self {
        Self::new(
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        )
    }

    /// The whole canvas.
    pub fn canvas([width, height]: [u32; 2]) -> Self {
        Self::new(0, 0, width, height)
    }

    pub fn right(self) -> u32 {
        self.x + self.width
    }

    pub fn bottom(self) -> u32 {
        self.y + self.height
    }

    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The point at `unit` across the box (0, 0 is the top left; 1, 1 the bottom right).
    pub fn point(self, [u, v]: [f32; 2]) -> Point {
        Point::new(
            self.x as f32 + self.width as f32 * u,
            self.y as f32 + self.height as f32 * v,
        )
    }

    /// The top-left and bottom-right corners.
    pub fn corners(self) -> (Point, Point) {
        (self.point([0.0, 0.0]), self.point([1.0, 1.0]))
    }

    /// The part of the box on a canvas of this size, should the canvas have shrunk under it.
    pub fn clamped(self, [width, height]: [u32; 2]) -> Self {
        let x = self.x.min(width);
        let y = self.y.min(height);
        Self::new(
            x,
            y,
            self.right().min(width) - x,
            self.bottom().min(height) - y,
        )
    }

    /// The handle within `tolerance` of `point` (the nearest wins), else whether `point` is inside.
    pub fn hit(self, point: Point, tolerance: f32) -> Option<Hit> {
        Handle::ALL
            .iter()
            .map(|&handle| (handle, self.point(handle.unit()).distance(point)))
            .filter(|&(_, distance)| distance < tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| Hit::Handle(handle))
            .or_else(|| {
                let (min, max) = self.corners();
                (point.x >= min.x && point.x <= max.x && point.y >= min.y && point.y <= max.y)
                    .then_some(Hit::Inside)
            })
    }
}

/// A size in the proportions of `terms`, `scale` times the ratio (rounded) where `limit` allows.
/// Exact ratios give a whole multiple of the terms, or nothing when not even one fits; others
/// round each side to the nearest pixel.
fn sized([a, b]: [u32; 2], scale: f64, [max_width, max_height]: [u32; 2]) -> [u32; 2] {
    if a.max(b) <= EXACT_TERMS {
        let most = (max_width / a).min(max_height / b);
        let k = (scale.max(0.0).round().min(f64::from(most))) as u32;
        return [a * k, b * k];
    }
    let (a, b) = (f64::from(a), f64::from(b));
    let width = (a * scale)
        .min(f64::from(max_width))
        .min(f64::from(max_height) * a / b)
        .max(0.0);
    let height = ((width * b / a).round() as u32).min(max_height);
    let width = (width.round() as u32).min(max_width);
    if width == 0 || height == 0 {
        [0, 0]
    } else {
        [width, height]
    }
}

/// One axis of a drag from `anchor`, a pixel edge within `0..=extent`, towards `pointer`: whether
/// it runs forward (right or down), the most it can grow before the canvas edge, and how far the
/// pointer is.
fn axis(anchor: u32, pointer: f32, extent: u32) -> (bool, u32, f64) {
    let forward = pointer >= anchor as f32;
    let limit = if forward { extent - anchor } else { anchor };
    let wanted = f64::from((pointer - anchor as f32).abs());
    (
        forward,
        limit,
        if wanted.is_finite() { wanted } else { 0.0 },
    )
}

/// The start of a span of `size` that ends or begins at `anchor`.
fn place(anchor: u32, forward: bool, size: u32) -> u32 {
    if forward { anchor } else { anchor - size }
}

/// `point` rounded to the nearest pixel corner on the canvas.
fn pixel_corner(point: Point, [width, height]: [u32; 2]) -> [u32; 2] {
    let round = |value: f32, extent: u32| value.round().clamp(0.0, extent as f32) as u32;
    [round(point.x, width), round(point.y, height)]
}

/// The box dragged from `anchor` to `pointer`. With a ratio, the side the pointer is farther
/// along (in units of the ratio) decides the size, to the nearest whole unit, as far as the
/// canvas allows.
pub fn draw(anchor: Point, pointer: Point, terms: Option<[u32; 2]>, canvas: [u32; 2]) -> CropBox {
    let [ax, ay] = pixel_corner(anchor, canvas);
    let (right, max_width, want_width) = axis(ax, pointer.x, canvas[0]);
    let (down, max_height, want_height) = axis(ay, pointer.y, canvas[1]);
    let [width, height] = match terms {
        None => [
            (want_width.round() as u32).min(max_width),
            (want_height.round() as u32).min(max_height),
        ],
        Some([a, b]) => sized(
            [a, b],
            (want_width / f64::from(a)).max(want_height / f64::from(b)),
            [max_width, max_height],
        ),
    };
    CropBox::new(
        place(ax, right, width),
        place(ay, down, height),
        width,
        height,
    )
}

/// `original` with `handle` dragged by `delta`. A corner pivots on the opposite corner; an edge
/// moves alone, and with a ratio the other sides follow, centred on the box's middle (and kept on
/// the canvas). Dragging past the opposite side flips the box.
pub fn resize(
    original: CropBox,
    handle: Handle,
    delta: Point,
    terms: Option<[u32; 2]>,
    canvas: [u32; 2],
) -> CropBox {
    let original = original.clamped(canvas);
    let [u, v] = handle.unit();
    let grabbed = original.point([u, v]);
    let pointer = Point::new(grabbed.x + delta.x, grabbed.y + delta.y);
    if handle.is_corner() {
        let anchor = original.point([1.0 - u, 1.0 - v]);
        return draw(anchor, pointer, terms, canvas);
    }
    let horizontal = v == 0.5;
    // Work along the dragged axis as x; the other axis is y.
    let flip = |b: CropBox| {
        if horizontal {
            b
        } else {
            CropBox::new(b.y, b.x, b.height, b.width)
        }
    };
    let (box_, extent, along, terms) = if horizontal {
        (original, canvas, pointer.x, terms)
    } else {
        (
            flip(original),
            [canvas[1], canvas[0]],
            pointer.y,
            terms.map(|[a, b]| [b, a]),
        )
    };
    let start = if (if horizontal { u } else { v }) == 0.0 {
        box_.right()
    } else {
        box_.x
    };
    let (forward, max_width, want) = axis(start, along, extent[0]);
    let (width, height, y) = match terms {
        None => ((want.round() as u32).min(max_width), box_.height, box_.y),
        Some([a, b]) => {
            let [width, height] = sized([a, b], want / f64::from(a), [max_width, extent[1]]);
            let middle = f64::from(box_.y) + f64::from(box_.height) / 2.0;
            let top = (middle - f64::from(height) / 2.0)
                .round()
                .clamp(0.0, f64::from(extent[1] - height)) as u32;
            (width, height, top)
        }
    };
    flip(CropBox::new(place(start, forward, width), y, width, height))
}

/// `original` moved by `delta`, to whole pixels and kept on the canvas.
pub fn moved(original: CropBox, delta: Point, [width, height]: [u32; 2]) -> CropBox {
    let original = original.clamped([width, height]);
    let shift = |start: u32, by: f32, size: u32, extent: u32| {
        (start as f32 + by)
            .round()
            .clamp(0.0, extent.saturating_sub(size) as f32) as u32
    };
    CropBox {
        x: shift(original.x, delta.x, original.width, width),
        y: shift(original.y, delta.y, original.height, height),
        ..original
    }
}

/// The largest box in the proportions of `terms` that fits in `bounds`, centred in it; `bounds`
/// itself for Free. Empty when not even one unit of the ratio fits.
pub fn fit_inside(bounds: CropBox, terms: Option<[u32; 2]>) -> CropBox {
    let Some(terms) = terms else {
        return bounds;
    };
    let [width, height] = sized(terms, f64::INFINITY, [bounds.width, bounds.height]);
    CropBox::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    )
}

/// `current` refitted to a new ratio: the largest box of that ratio inside it, or, when it is too
/// small to hold one, the largest on the canvas.
pub fn conform(current: CropBox, terms: Option<[u32; 2]>, canvas: [u32; 2]) -> CropBox {
    let current = current.clamped(canvas);
    let fitted = fit_inside(current, terms);
    if fitted.is_empty() && !current.is_empty() {
        fit_inside(CropBox::canvas(canvas), terms)
    } else {
        fitted
    }
}

/// `current` turned a quarter about its middle (width and height exchanged, kept on the canvas)
/// and refitted to `terms`, for the Swap button.
pub fn turned(current: CropBox, terms: Option<[u32; 2]>, canvas: [u32; 2]) -> CropBox {
    let current = current.clamped(canvas);
    let width = current.height.min(canvas[0]);
    let height = current.width.min(canvas[1]);
    let start = |middle: f64, size: u32, extent: u32| {
        (middle - f64::from(size) / 2.0)
            .round()
            .clamp(0.0, f64::from(extent - size)) as u32
    };
    let x = start(
        f64::from(current.x) + f64::from(current.width) / 2.0,
        width,
        canvas[0],
    );
    let y = start(
        f64::from(current.y) + f64::from(current.height) / 2.0,
        height,
        canvas[1],
    );
    conform(CropBox::new(x, y, width, height), terms, canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: [u32; 2] = [400, 300];

    fn p(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }

    /// The box keeps exactly `terms`: both sides the same whole multiple of them.
    #[track_caller]
    fn assert_exact(b: CropBox, [a, c]: [u32; 2]) {
        assert!(!b.is_empty(), "{b:?} is empty");
        assert_eq!(b.width % a, 0, "{b:?} is not a multiple of {a}:{c}");
        assert_eq!(b.width / a * c, b.height, "{b:?} is not {a}:{c}");
    }

    #[track_caller]
    fn assert_on_canvas(b: CropBox, canvas: [u32; 2]) {
        assert!(
            b.right() <= canvas[0] && b.bottom() <= canvas[1],
            "{b:?} leaves {canvas:?}"
        );
    }

    #[test]
    fn menu_lists_free_original_every_preset_and_custom() {
        let menu: Vec<_> = Ratio::menu().collect();
        assert_eq!(menu.len(), PRESETS.len() + 3);
        assert_eq!(menu[0], Ratio::Free);
        assert_eq!(menu[1], Ratio::Original);
        assert_eq!(menu.last(), Some(&Ratio::Custom));
        assert!(menu.contains(&Ratio::Preset([9, 16])));
        assert!(menu.contains(&Ratio::Preset([9, 20])));
        for terms in PRESETS {
            assert_eq!(reduced(*terms), Some(*terms), "{terms:?} is not reduced");
        }
    }

    #[test]
    fn terms_are_reduced_and_original_follows_the_canvas() {
        assert_eq!(Ratio::Free.terms(CANVAS, [1, 1]), None);
        assert_eq!(Ratio::Original.terms([1920, 1080], [1, 1]), Some([16, 9]));
        assert_eq!(Ratio::Original.terms(CANVAS, [1, 1]), Some([4, 3]));
        assert_eq!(Ratio::Preset([9, 16]).terms(CANVAS, [1, 1]), Some([9, 16]));
        assert_eq!(Ratio::Custom.terms(CANVAS, [18, 40]), Some([9, 20]));
        assert_eq!(Ratio::Custom.terms(CANVAS, [0, 4]), None);
        assert_eq!(reduced([1001, 997]), Some([1001, 997]));
    }

    #[test]
    fn swap_turns_presets_original_and_custom() {
        let custom = [7, 2];
        assert_eq!(
            Ratio::Preset([16, 9]).swapped(CANVAS, custom),
            (Ratio::Preset([9, 16]), custom)
        );
        assert_eq!(
            Ratio::Preset([1, 1]).swapped(CANVAS, custom),
            (Ratio::Preset([1, 1]), custom)
        );
        // 20:9 is not a preset, so it becomes Custom.
        assert_eq!(
            Ratio::Preset([9, 20]).swapped(CANVAS, custom),
            (Ratio::Custom, [20, 9])
        );
        assert_eq!(
            Ratio::Custom.swapped(CANVAS, [9, 20]),
            (Ratio::Custom, [20, 9])
        );
        assert_eq!(
            Ratio::Custom.swapped(CANVAS, [3, 2]),
            (Ratio::Preset([2, 3]), [3, 2])
        );
        // The canvas's 4:3 turns into the 3:4 preset.
        assert_eq!(
            Ratio::Original.swapped(CANVAS, custom),
            (Ratio::Preset([3, 4]), custom)
        );
        assert_eq!(Ratio::Free.swapped(CANVAS, custom), (Ratio::Free, custom));
    }

    #[test]
    fn free_drags_round_to_pixels_in_every_direction() {
        let anchor = p(100.4, 100.6);
        for (pointer, expected) in [
            (p(150.4, 130.2), CropBox::new(100, 101, 50, 29)),
            (p(49.6, 130.2), CropBox::new(50, 101, 50, 29)),
            (p(150.4, 70.6), CropBox::new(100, 71, 50, 30)),
            (p(49.6, 70.6), CropBox::new(50, 71, 50, 30)),
        ] {
            assert_eq!(draw(anchor, pointer, None, CANVAS), expected);
        }
        // A click without a drag is empty.
        assert!(draw(anchor, anchor, None, CANVAS).is_empty());
    }

    #[test]
    fn drags_stay_on_the_canvas() {
        assert_eq!(
            draw(p(-20.0, -5.0), p(500.0, 900.0), None, CANVAS),
            CropBox::canvas(CANVAS)
        );
        assert_eq!(
            draw(p(390.0, 290.0), p(-50.0, -50.0), None, CANVAS),
            CropBox::new(0, 0, 390, 290)
        );
        let b = draw(p(390.0, 10.0), p(-50.0, 900.0), Some([9, 16]), CANVAS);
        assert_exact(b, [9, 16]);
        assert_on_canvas(b, CANVAS);
        // The tallest 9:16 that fits: 16 × 18 = 288.
        assert_eq!(b.height, 288);
    }

    #[test]
    fn every_ratio_keeps_exactly_in_all_four_directions() {
        let anchor = p(200.0, 150.0);
        for ratio in Ratio::menu().filter(|r| !matches!(r, Ratio::Free)) {
            let terms = ratio.terms(CANVAS, [9, 20]).unwrap();
            for (dx, dy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                for (along, across) in [(37.3, 12.9), (5.2, 81.7), (120.0, 120.0)] {
                    let pointer = p(200.0 + dx * along, 150.0 + dy * across);
                    let b = draw(anchor, pointer, Some(terms), CANVAS);
                    assert_exact(b, terms);
                    assert_on_canvas(b, CANVAS);
                    // The anchor stays a corner, on the side away from the pointer.
                    let corner_x = if dx > 0.0 { b.x } else { b.right() };
                    let corner_y = if dy > 0.0 { b.y } else { b.bottom() };
                    assert_eq!((corner_x, corner_y), (200, 150), "{ratio:?} {b:?}");
                }
            }
        }
    }

    #[test]
    fn a_ratio_follows_the_pointer_in_whole_units() {
        // 9:16 dragged 40 × 50: the width asks for 40 / 9 = 4.4 units, more than the height's
        // 50 / 16 = 3.1, so the box is the nearest whole number of units to it: 4.
        let b = draw(p(0.0, 0.0), p(40.0, 50.0), Some([9, 16]), CANVAS);
        assert_eq!(b, CropBox::new(0, 0, 36, 64));
        let b = draw(p(0.0, 0.0), p(20.0, 38.0), Some([9, 16]), CANVAS);
        assert_eq!(b, CropBox::new(0, 0, 18, 32));
        // Too close to the edge for one unit: nothing, not a box of the wrong shape.
        assert!(draw(p(395.0, 0.0), p(420.0, 40.0), Some([9, 16]), CANVAS).is_empty());
    }

    #[test]
    fn custom_nine_by_twenty() {
        let terms = Ratio::Custom.terms(CANVAS, [9, 20]);
        let b = draw(p(10.0, 10.0), p(60.0, 200.0), terms, CANVAS);
        assert_exact(b, [9, 20]);
        assert_eq!(b, CropBox::new(10, 10, 90, 200));
    }

    #[test]
    fn original_uses_the_canvas_ratio() {
        let canvas = [1920, 1080];
        let terms = Ratio::Original.terms(canvas, [1, 1]);
        let b = draw(p(0.0, 0.0), p(500.0, 100.0), terms, canvas);
        assert_exact(b, [16, 9]);
        // An odd canvas's own ratio is too fine to keep exactly, so its sides round instead.
        let odd = [1001, 997];
        let terms = Ratio::Original.terms(odd, [1, 1]);
        let b = draw(p(0.0, 0.0), p(500.0, 100.0), terms, odd);
        assert_eq!(b, CropBox::new(0, 0, 500, 498));
        let b = draw(p(0.0, 0.0), p(5000.0, 100.0), terms, odd);
        assert_eq!(b, CropBox::canvas(odd));
    }

    #[test]
    fn corner_handles_keep_the_ratio_and_pivot_on_the_opposite_corner() {
        let original = CropBox::new(100, 50, 90, 160);
        for handle in Handle::ALL.into_iter().filter(|h| h.is_corner()) {
            for delta in [
                p(13.0, -7.0),
                p(-31.5, 22.0),
                p(4.0, 4.0),
                p(-200.0, -200.0),
            ] {
                let b = resize(original, handle, delta, Some([9, 16]), CANVAS);
                assert_exact(b, [9, 16]);
                assert_on_canvas(b, CANVAS);
            }
        }
        // The bottom-right corner dragged out by a unit and a bit grows by one unit.
        let b = resize(
            original,
            Handle::BottomRight,
            p(0.0, 20.0),
            Some([9, 16]),
            CANVAS,
        );
        assert_eq!(b, CropBox::new(100, 50, 99, 176));
        // The top-left corner pivots on the bottom right.
        let b = resize(
            original,
            Handle::TopLeft,
            p(9.0, 16.0),
            Some([9, 16]),
            CANVAS,
        );
        assert_eq!(b, CropBox::new(109, 66, 81, 144));
    }

    #[test]
    fn edge_handles_keep_the_ratio_centred_on_the_box() {
        let original = CropBox::new(100, 50, 90, 160);
        for handle in Handle::ALL.into_iter().filter(|h| !h.is_corner()) {
            for delta in [
                p(13.0, -7.0),
                p(-31.5, 22.0),
                p(40.0, 40.0),
                p(-300.0, 300.0),
            ] {
                let b = resize(original, handle, delta, Some([9, 16]), CANVAS);
                assert_exact(b, [9, 16]);
                assert_on_canvas(b, CANVAS);
            }
        }
        // The right edge out by 18 adds two units; the height grows about the middle (130).
        let b = resize(original, Handle::Right, p(18.0, 3.0), Some([9, 16]), CANVAS);
        assert_eq!(b, CropBox::new(100, 34, 108, 192));
        // The bottom edge up by 32 takes two units off; the width shrinks about the middle (145).
        let b = resize(
            original,
            Handle::Bottom,
            p(5.0, -32.0),
            Some([9, 16]),
            CANVAS,
        );
        assert_eq!(b, CropBox::new(109, 50, 72, 128));
        // Grown past the canvas top, the box slides down to stay on it.
        let b = resize(original, Handle::Left, p(-90.0, 0.0), Some([9, 16]), CANVAS);
        assert_eq!(b, CropBox::new(28, 0, 162, 288));
        let b = resize(original, Handle::Left, p(-63.0, 0.0), Some([9, 16]), CANVAS);
        assert_exact(b, [9, 16]);
        assert_eq!((b.right(), b.y, b.height), (190, 0, 272));
    }

    #[test]
    fn free_edges_move_one_side_and_can_flip() {
        let original = CropBox::new(100, 50, 90, 160);
        assert_eq!(
            resize(original, Handle::Right, p(10.4, 99.0), None, CANVAS),
            CropBox::new(100, 50, 100, 160)
        );
        assert_eq!(
            resize(original, Handle::Top, p(99.0, -20.0), None, CANVAS),
            CropBox::new(100, 30, 90, 180)
        );
        assert_eq!(
            resize(original, Handle::Left, p(-500.0, 0.0), None, CANVAS),
            CropBox::new(0, 50, 190, 160)
        );
        // Past the opposite edge the box turns over that edge.
        assert_eq!(
            resize(original, Handle::Bottom, p(0.0, -200.0), None, CANVAS),
            CropBox::new(100, 10, 90, 40)
        );
        assert_eq!(
            resize(original, Handle::TopLeft, p(120.0, 0.0), None, CANVAS),
            CropBox::new(190, 50, 30, 160)
        );
    }

    #[test]
    fn moving_rounds_and_clamps_inside_the_canvas() {
        let original = CropBox::new(100, 50, 90, 160);
        assert_eq!(
            moved(original, p(10.4, -20.6), CANVAS),
            CropBox::new(110, 29, 90, 160)
        );
        assert_eq!(
            moved(original, p(-500.0, -500.0), CANVAS),
            CropBox::new(0, 0, 90, 160)
        );
        assert_eq!(
            moved(original, p(500.0, 500.0), CANVAS),
            CropBox::new(310, 140, 90, 160)
        );
        // A box left beyond a smaller canvas is cut to it first.
        let stale = CropBox::new(350, 250, 100, 100);
        assert_eq!(
            moved(stale, p(-10.0, 0.0), CANVAS),
            CropBox::new(340, 250, 50, 50)
        );
        assert_eq!(
            resize(stale, Handle::Left, p(-10.0, 0.0), None, CANVAS),
            CropBox::new(340, 250, 60, 50)
        );
        // A box as large as the canvas cannot move.
        let whole = CropBox::canvas(CANVAS);
        assert_eq!(moved(whole, p(30.0, 30.0), CANVAS), whole);
    }

    #[test]
    fn selection_bounds_start_the_box_in_the_ratio() {
        let bounds = CropBox::from_bounds((20, 30, 120, 90));
        assert_eq!(bounds, CropBox::new(20, 30, 100, 60));
        assert_eq!(fit_inside(bounds, None), bounds);
        // The largest 9:16 inside 100 × 60 is 27 × 48, centred.
        let b = fit_inside(bounds, Some([9, 16]));
        assert_eq!(b, CropBox::new(56, 36, 27, 48));
        let b = fit_inside(bounds, Some([16, 9]));
        assert_eq!(b, CropBox::new(22, 33, 96, 54));
        assert!(fit_inside(CropBox::new(0, 0, 8, 8), Some([9, 16])).is_empty());
    }

    #[test]
    fn changing_the_ratio_refits_the_box() {
        let current = CropBox::new(100, 50, 90, 160);
        let b = conform(current, Some([1, 1]), CANVAS);
        assert_eq!(b, CropBox::new(100, 85, 90, 90));
        // Too small for one 9:20 unit: the largest on the canvas instead.
        let tiny = CropBox::new(0, 0, 8, 8);
        let b = conform(tiny, Some([9, 20]), CANVAS);
        assert_exact(b, [9, 20]);
        assert_eq!(b.height, 300);
        // A box left beyond a smaller canvas is cut to it first.
        let b = conform(CropBox::new(350, 250, 100, 100), None, CANVAS);
        assert_eq!(b, CropBox::new(350, 250, 50, 50));
    }

    #[test]
    fn swapping_turns_the_box_about_its_middle() {
        let current = CropBox::new(100, 50, 90, 160);
        let b = turned(current, Some([16, 9]), CANVAS);
        assert_exact(b, [16, 9]);
        assert_eq!(b, CropBox::new(65, 85, 160, 90));
        assert_eq!(turned(b, Some([9, 16]), CANVAS), current);
        // Too long to turn in place: kept on the canvas.
        let wide = CropBox::new(0, 0, 400, 100);
        let b = turned(wide, None, CANVAS);
        assert_eq!(b, CropBox::new(150, 0, 100, 300));
    }

    #[test]
    fn hits_find_the_nearest_handle_then_the_inside() {
        let b = CropBox::new(100, 50, 90, 160);
        assert_eq!(
            b.hit(p(101.0, 49.0), 5.0),
            Some(Hit::Handle(Handle::TopLeft))
        );
        assert_eq!(
            b.hit(p(145.0, 212.0), 5.0),
            Some(Hit::Handle(Handle::Bottom))
        );
        assert_eq!(
            b.hit(p(192.0, 130.0), 5.0),
            Some(Hit::Handle(Handle::Right))
        );
        assert_eq!(b.hit(p(120.0, 100.0), 5.0), Some(Hit::Inside));
        assert_eq!(b.hit(p(20.0, 20.0), 5.0), None);
        // A tiny box: the nearer of two overlapping handles.
        let small = CropBox::new(10, 10, 2, 2);
        assert_eq!(
            small.hit(p(12.0, 12.0), 5.0),
            Some(Hit::Handle(Handle::BottomRight))
        );
    }

    #[test]
    fn clamping_and_corners() {
        let b = CropBox::new(350, 250, 100, 100);
        assert_eq!(b.clamped(CANVAS), CropBox::new(350, 250, 50, 50));
        assert!(CropBox::new(500, 0, 10, 10).clamped(CANVAS).is_empty());
        assert_eq!(b.corners(), (p(350.0, 250.0), p(450.0, 350.0)));
        assert_eq!(Handle::ALL.iter().filter(|h| h.is_corner()).count(), 4);
    }
}

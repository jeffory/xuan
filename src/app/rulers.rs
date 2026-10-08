//! View → Rulers: document-pixel rulers along the top and left of the canvas.
//!
//! Follows Compositor's `UI/CanvasRulers.swift`: numbered ticks about 70 points apart in 1-2-5
//! steps, each split in ten with a longer tick at the half. Positions come from the same mapping
//! as the canvas (`canvas::image_origin` and the zoom), so a tick sits exactly over its pixel.
use egui::{Color32, FontId, Painter, Pos2, Rect, Stroke, pos2, vec2};

/// Width of the left ruler and height of the top one, in points.
pub(super) const RULER_SIZE: f32 = 18.0;

const BACKGROUND: Color32 = Color32::from_gray(51);
const TICK: Color32 = Color32::from_gray(158);
const LABEL: Color32 = Color32::from_gray(199);
const EDGE: Color32 = Color32::from_gray(20);

/// Document pixels between numbered ticks: the first 1-2-5 step at least 70 points apart.
pub(super) fn major_step(zoom: f32) -> f32 {
    const NICE: [f32; 18] = [
        1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1_000.0, 2_000.0,
        2_500.0, 5_000.0, 10_000.0, 20_000.0, 25_000.0,
    ];
    let target = 70.0 / zoom.max(0.0001);
    NICE.into_iter()
        .find(|step| *step >= target)
        .unwrap_or(50_000.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TickKind {
    /// Numbered.
    Major,
    /// Halfway between two numbered ticks.
    Mid,
    Minor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tick {
    /// Position along the ruler, in screen points.
    pub screen: f32,
    /// Document pixels.
    pub value: f32,
    pub kind: TickKind,
}

impl Tick {
    /// Tick length in points: 8, 5 or 3.
    pub fn length(self) -> f32 {
        match self.kind {
            TickKind::Major => 8.0,
            TickKind::Mid => 5.0,
            TickKind::Minor => 3.0,
        }
    }
}

/// The ticks of a ruler spanning `from..=to` on screen, where document pixel 0 is at `origin` and
/// one pixel is `zoom` points long. Values are counted from 0, so labels never drift.
pub(super) fn ticks(origin: f32, zoom: f32, from: f32, to: f32) -> Vec<Tick> {
    if !(zoom.is_finite() && zoom > 0.0 && origin.is_finite() && from.is_finite() && to >= from) {
        return Vec::new();
    }
    let step = major_step(zoom);
    let minor = step / 10.0;
    let first = ((from - origin) / zoom / minor).floor() as i64;
    let last = ((to - origin) / zoom / minor).ceil() as i64;
    (first..=last)
        .map(|index| {
            let value = index as f32 * minor;
            Tick {
                screen: origin + value * zoom,
                value,
                kind: if index.rem_euclid(10) == 0 {
                    TickKind::Major
                } else if index.rem_euclid(5) == 0 {
                    TickKind::Mid
                } else {
                    TickKind::Minor
                },
            }
        })
        .filter(|tick| (from..=to).contains(&tick.screen))
        .collect()
}

/// A numbered tick's label: whole pixels.
pub(super) fn label(value: f32) -> String {
    let rounded = value.round() as i64;
    rounded.to_string()
}

/// The two ruler strips and the corner square where they meet, for a canvas area `area`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RulerLayout {
    pub corner: Rect,
    /// Along the top: measures X, and drags out horizontal guides.
    pub top: Rect,
    /// Down the left: measures Y, and drags out vertical guides.
    pub left: Rect,
    /// What is left for the canvas itself.
    pub canvas: Rect,
}

impl RulerLayout {
    pub fn new(area: Rect) -> Self {
        let inner = Rect::from_min_max(area.min + vec2(RULER_SIZE, RULER_SIZE), area.max);
        Self {
            corner: Rect::from_min_size(area.min, vec2(RULER_SIZE, RULER_SIZE)),
            top: Rect::from_min_max(
                pos2(inner.left(), area.top()),
                pos2(area.right(), inner.top()),
            ),
            left: Rect::from_min_max(
                pos2(area.left(), inner.top()),
                pos2(inner.left(), area.bottom()),
            ),
            canvas: inner,
        }
    }

    /// Whether `point` is over either ruler or the corner, where a released guide is removed.
    pub fn contains(&self, point: Pos2) -> bool {
        self.corner.contains(point) || self.top.contains(point) || self.left.contains(point)
    }
}

/// Paints both rulers for an image whose top-left corner is at `origin`.
pub(super) fn paint(painter: &Painter, layout: &RulerLayout, origin: Pos2, zoom: f32) {
    let hairline = 1.0 / painter.pixels_per_point().max(1.0);
    let font = FontId::monospace(8.0);
    // Horizontal ruler.
    let top = painter.with_clip_rect(layout.top);
    top.rect_filled(layout.top, 0.0, BACKGROUND);
    for tick in ticks(origin.x, zoom, layout.top.left(), layout.top.right()) {
        let x = tick.screen;
        top.line_segment(
            [
                pos2(x, layout.top.bottom() - tick.length()),
                pos2(x, layout.top.bottom()),
            ],
            Stroke::new(hairline, TICK),
        );
        if tick.kind == TickKind::Major {
            top.text(
                pos2(x + 2.0, layout.top.top() + 1.0),
                egui::Align2::LEFT_TOP,
                label(tick.value),
                font.clone(),
                LABEL,
            );
        }
    }
    top.line_segment(
        [
            pos2(layout.top.left(), layout.top.bottom() - hairline * 0.5),
            pos2(layout.top.right(), layout.top.bottom() - hairline * 0.5),
        ],
        Stroke::new(hairline, EDGE),
    );
    // Vertical ruler, its labels turned to read up along the tick.
    let left = painter.with_clip_rect(layout.left);
    left.rect_filled(layout.left, 0.0, BACKGROUND);
    for tick in ticks(origin.y, zoom, layout.left.top(), layout.left.bottom()) {
        let y = tick.screen;
        left.line_segment(
            [
                pos2(layout.left.right() - tick.length(), y),
                pos2(layout.left.right(), y),
            ],
            Stroke::new(hairline, TICK),
        );
        if tick.kind == TickKind::Major {
            let galley = left.layout_no_wrap(label(tick.value), font.clone(), LABEL);
            let width = galley.size().x;
            left.add(
                egui::epaint::TextShape::new(
                    pos2(layout.left.left() + 1.0, y + 2.0 + width),
                    galley,
                    LABEL,
                )
                .with_angle(-std::f32::consts::FRAC_PI_2),
            );
        }
    }
    left.line_segment(
        [
            pos2(layout.left.right() - hairline * 0.5, layout.left.top()),
            pos2(layout.left.right() - hairline * 0.5, layout.left.bottom()),
        ],
        Stroke::new(hairline, EDGE),
    );
    // The corner, with upstream's diagonal mark.
    painter.rect_filled(layout.corner, 0.0, BACKGROUND);
    painter.line_segment(
        [
            layout.corner.min + vec2(5.0, RULER_SIZE - 4.0),
            layout.corner.min + vec2(RULER_SIZE - 4.0, 5.0),
        ],
        Stroke::new(1.0_f32, Color32::from_white_alpha(71)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_ticks_are_about_70_points_apart_in_1_2_5_steps() {
        for (zoom, step) in [
            (100.0, 1.0),
            (64.0, 2.0),
            (8.0, 10.0),
            (1.0, 100.0),
            (0.5, 200.0),
            (0.3, 250.0),
            (0.1, 1_000.0),
            (0.01, 10_000.0),
            (0.001, 50_000.0),
        ] {
            assert_eq!(major_step(zoom), step, "zoom {zoom}");
            assert!(step * zoom >= 70.0 || step == 50_000.0);
        }
    }

    #[test]
    fn ticks_follow_the_canvas_mapping_at_any_zoom() {
        // At 100% with the image at x = 40, a 300 point ruler from 0 shows -40..=260.
        let ticks_100 = ticks(40.0, 1.0, 0.0, 300.0);
        let majors: Vec<_> = ticks_100
            .iter()
            .filter(|t| t.kind == TickKind::Major)
            .map(|t| (t.value, t.screen))
            .collect();
        assert_eq!(majors, [(0.0, 40.0), (100.0, 140.0), (200.0, 240.0)]);
        // A tick every 10 px, with the half at 50 longer.
        assert_eq!(ticks_100.len(), 31);
        let mid = ticks_100.iter().find(|t| t.value == 50.0).unwrap();
        assert_eq!(mid.kind, TickKind::Mid);
        assert_eq!(mid.length(), 5.0);
        assert!(ticks_100.iter().any(|t| t.value == -40.0));

        // At 200% numbered ticks fall every 50 px (100 points), still placed by the mapping.
        let zoomed = ticks(-30.0, 2.0, 0.0, 400.0);
        for tick in &zoomed {
            assert!((tick.screen - (-30.0 + tick.value * 2.0)).abs() < 1e-3);
        }
        let majors: Vec<_> = zoomed
            .iter()
            .filter(|t| t.kind == TickKind::Major)
            .map(|t| t.value)
            .collect();
        assert_eq!(majors, [50.0, 100.0, 150.0, 200.0]);

        // Zoomed far in, minor ticks fall between pixels.
        let close = ticks(0.0, 100.0, 0.0, 250.0);
        assert_eq!(close[1].value, 0.1);
        assert_eq!(
            close.iter().filter(|t| t.kind == TickKind::Major).count(),
            3
        );

        // Far out, labels stay whole numbers and every tick is on the ruler.
        let far = ticks(500.0, 0.05, 0.0, 1000.0);
        assert!(far.iter().all(|t| (0.0..=1000.0).contains(&t.screen)));
        assert!(
            far.iter()
                .any(|t| t.kind == TickKind::Major && t.value == -10_000.0)
        );
        assert_eq!(label(-10_000.0), "-10000");
        assert_eq!(label(2.4999), "2");

        assert!(ticks(0.0, 0.0, 0.0, 100.0).is_empty());
        assert!(ticks(0.0, 1.0, 100.0, 0.0).is_empty());
    }

    #[test]
    fn layout_reserves_the_strips_and_corner() {
        let layout = RulerLayout::new(Rect::from_min_size(pos2(10.0, 20.0), vec2(300.0, 200.0)));
        assert_eq!(layout.canvas.min, pos2(28.0, 38.0));
        assert_eq!(layout.canvas.max, pos2(310.0, 220.0));
        assert_eq!(layout.top.height(), RULER_SIZE);
        assert_eq!(layout.left.width(), RULER_SIZE);
        assert!(layout.contains(pos2(15.0, 25.0)));
        assert!(layout.contains(pos2(200.0, 30.0)));
        assert!(layout.contains(pos2(12.0, 200.0)));
        assert!(!layout.contains(pos2(100.0, 100.0)));
    }
}

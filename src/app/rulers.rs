//! View → Rulers: rulers along the top and left of the canvas, in document pixels or in the unit
//! chosen under Settings → Units & Rulers (issue 94).
//!
//! Follows Compositor's `UI/CanvasRulers.swift`: numbered ticks about 70 points apart in 1-2-5
//! steps, each split in ten with a longer tick at the half; an inch, or a fraction of one, is
//! split in eighths instead. Positions come from the same mapping as the canvas
//! (`canvas::image_origin` and the zoom), so a tick sits exactly over its pixel, whatever the
//! unit: a print unit converts through the document's resolution, and percent is of the
//! document's width (top) or height (left).
use egui::{FontId, Painter, Pos2, Rect, Stroke, pos2, vec2};
use xuan::units::Unit;

/// Width of the left ruler and height of the top one, in points.
pub(super) const RULER_SIZE: f32 = 18.0;

/// Numbered steps for pixels.
const PIXEL_STEPS: [f32; 18] = [
    1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1_000.0, 2_000.0, 2_500.0,
    5_000.0, 10_000.0, 20_000.0, 25_000.0,
];
/// Numbered steps for centimetres, millimetres, points, picas and percent.
const STEPS: [f32; 21] = [
    0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0,
    2_000.0, 5_000.0, 10_000.0, 20_000.0, 50_000.0,
];
/// Numbered steps for inches: halves, quarters and eighths below one.
const INCH_STEPS: [f32; 15] = [
    0.125, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0,
    5_000.0,
];
/// The most ticks one ruler draws, however far out the view is zoomed.
const MAX_TICKS: i64 = 10_000;

/// What a ruler counts in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RulerScale {
    /// Document pixels in one unit.
    pixels: f32,
    unit: Unit,
}

impl RulerScale {
    pub const PIXELS: Self = Self {
        pixels: 1.0,
        unit: Unit::Pixels,
    };

    /// `unit` at `ppi`, where 100% is `reference` pixels. Pixels when the unit cannot be
    /// converted, as for a resolution of 0.
    pub fn new(unit: Unit, ppi: f32, reference: u32) -> Self {
        unit.pixels_per_unit(f64::from(ppi), f64::from(reference))
            .map(|pixels| pixels as f32)
            .filter(|pixels| pixels.is_finite() && *pixels > 0.0)
            .map_or(Self::PIXELS, |pixels| Self { pixels, unit })
    }

    fn steps(self) -> &'static [f32] {
        match self.unit {
            Unit::Pixels => &PIXEL_STEPS,
            Unit::Inches => &INCH_STEPS,
            _ => &STEPS,
        }
    }

    /// Units between numbered ticks: the first step at least 70 points apart.
    pub fn major_step(self, zoom: f32) -> f32 {
        let target = 70.0 / (zoom * self.pixels).max(0.0001);
        let steps = self.steps();
        let fallback = if self.unit == Unit::Pixels {
            50_000.0
        } else {
            steps[steps.len() - 1]
        };
        steps
            .iter()
            .copied()
            .find(|step| *step >= target)
            .unwrap_or(fallback)
    }

    /// Ticks in one numbered step: eighths of an inch or less, tenths otherwise.
    fn divisions(self, step: f32) -> i64 {
        if self.unit == Unit::Inches && step <= 1.0 {
            8
        } else {
            10
        }
    }

    /// A numbered tick's label: whole pixels, or up to three decimals of another unit.
    pub fn label(self, value: f32) -> String {
        if self.unit == Unit::Pixels {
            return label(value);
        }
        let text = format!("{value:.3}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        if text == "-0" { "0" } else { text }.to_owned()
    }
}

/// Document pixels between numbered ticks: the first 1-2-5 step at least 70 points apart.
#[cfg(test)]
pub(super) fn major_step(zoom: f32) -> f32 {
    RulerScale::PIXELS.major_step(zoom)
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

/// The ticks of a pixel ruler spanning `from..=to` on screen; see [`ticks_in`].
#[cfg(test)]
pub(super) fn ticks(origin: f32, zoom: f32, from: f32, to: f32) -> Vec<Tick> {
    ticks_in(RulerScale::PIXELS, origin, zoom, from, to)
}

/// The ticks of a ruler in `scale` spanning `from..=to` on screen, where document pixel 0 is at
/// `origin` and one pixel is `zoom` points long. Values, in the ruler's unit, are counted from 0,
/// so labels never drift.
pub(super) fn ticks_in(scale: RulerScale, origin: f32, zoom: f32, from: f32, to: f32) -> Vec<Tick> {
    let points = zoom * scale.pixels;
    if !(points.is_finite() && points > 0.0 && origin.is_finite() && from.is_finite() && to >= from)
    {
        return Vec::new();
    }
    let step = scale.major_step(zoom);
    let divisions = scale.divisions(step);
    let minor = step / divisions as f32;
    let first = ((from - origin) / points / minor).floor() as i64;
    let last = ((to - origin) / points / minor).ceil() as i64;
    if last.saturating_sub(first) > MAX_TICKS {
        return Vec::new();
    }
    (first..=last)
        .map(|index| {
            let value = index as f32 * minor;
            Tick {
                screen: origin + value * points,
                value,
                kind: if index.rem_euclid(divisions) == 0 {
                    TickKind::Major
                } else if index.rem_euclid(divisions / 2) == 0 {
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

/// Paints both rulers for an image whose top-left corner is at `origin`, the top one in
/// `scales[0]` and the left one in `scales[1]`.
pub(super) fn paint(
    painter: &Painter,
    layout: &RulerLayout,
    origin: Pos2,
    zoom: f32,
    scales: [RulerScale; 2],
) {
    let p = super::theme::palette(painter.ctx());
    let (background, tick_color, label_color, edge) =
        (p.ruler, p.ruler_tick, p.ruler_label, p.ruler_edge);
    let hairline = 1.0 / painter.pixels_per_point().max(1.0);
    let font = FontId::monospace(8.0);
    // Horizontal ruler.
    let top = painter.with_clip_rect(layout.top);
    top.rect_filled(layout.top, 0.0, background);
    for tick in ticks_in(
        scales[0],
        origin.x,
        zoom,
        layout.top.left(),
        layout.top.right(),
    ) {
        let x = tick.screen;
        top.line_segment(
            [
                pos2(x, layout.top.bottom() - tick.length()),
                pos2(x, layout.top.bottom()),
            ],
            Stroke::new(hairline, tick_color),
        );
        if tick.kind == TickKind::Major {
            top.text(
                pos2(x + 2.0, layout.top.top() + 1.0),
                egui::Align2::LEFT_TOP,
                scales[0].label(tick.value),
                font.clone(),
                label_color,
            );
        }
    }
    top.line_segment(
        [
            pos2(layout.top.left(), layout.top.bottom() - hairline * 0.5),
            pos2(layout.top.right(), layout.top.bottom() - hairline * 0.5),
        ],
        Stroke::new(hairline, edge),
    );
    // Vertical ruler, its labels turned to read up along the tick.
    let left = painter.with_clip_rect(layout.left);
    left.rect_filled(layout.left, 0.0, background);
    for tick in ticks_in(
        scales[1],
        origin.y,
        zoom,
        layout.left.top(),
        layout.left.bottom(),
    ) {
        let y = tick.screen;
        left.line_segment(
            [
                pos2(layout.left.right() - tick.length(), y),
                pos2(layout.left.right(), y),
            ],
            Stroke::new(hairline, tick_color),
        );
        if tick.kind == TickKind::Major {
            let galley =
                left.layout_no_wrap(scales[1].label(tick.value), font.clone(), label_color);
            let width = galley.size().x;
            left.add(
                egui::epaint::TextShape::new(
                    pos2(layout.left.left() + 1.0, y + 2.0 + width),
                    galley,
                    label_color,
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
        Stroke::new(hairline, edge),
    );
    // The corner, with upstream's diagonal mark.
    painter.rect_filled(layout.corner, 0.0, background);
    painter.line_segment(
        [
            layout.corner.min + vec2(5.0, RULER_SIZE - 4.0),
            layout.corner.min + vec2(RULER_SIZE - 4.0, 5.0),
        ],
        Stroke::new(1.0_f32, p.ruler_corner),
    );
}

impl super::EditorApp {
    /// A ruler's right-click menu: the unit both rulers measure in, as under Settings → Units &
    /// Rulers.
    pub(super) fn ruler_unit_menu(&mut self, ui: &mut egui::Ui) {
        let mut unit = self.config.units.rulers;
        for choice in Unit::ALL {
            if super::widgets::menu_choice(ui, &mut unit, choice, xuan::i18n::tr(choice.name()))
                .clicked()
            {
                ui.close();
            }
        }
        if unit != self.config.units.rulers {
            self.config.units.rulers = unit;
            self.save_config();
        }
    }
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
    fn print_unit_ticks_line_up_with_the_pixels() {
        // At 300 ppi and 100%, a centimetre is 118.1 points: numbered every centimetre, a tick
        // every millimetre and a longer one at 5 mm.
        let cm = RulerScale::new(Unit::Centimeters, 300.0, 1000);
        assert_eq!(cm.major_step(1.0), 1.0);
        let ticks = ticks_in(cm, 10.0, 1.0, 0.0, 400.0);
        let majors: Vec<_> = ticks.iter().filter(|t| t.kind == TickKind::Major).collect();
        assert_eq!(majors.len(), 4);
        for (index, tick) in majors.iter().enumerate() {
            let pixels = index as f32 * 300.0 / 2.54;
            assert!((tick.screen - (10.0 + pixels)).abs() < 1e-2, "{tick:?}");
        }
        let five_mm = ticks.iter().find(|t| (t.value - 0.5).abs() < 1e-6).unwrap();
        assert_eq!(five_mm.kind, TickKind::Mid);
        assert_eq!(
            ticks
                .iter()
                .filter(|t| t.value < 1.0 && t.value >= 0.0)
                .count(),
            10
        );
        // Millimetres at 300 ppi, zoomed in to 400%: numbered every 2 mm.
        let mm = RulerScale::new(Unit::Millimeters, 300.0, 1000);
        assert_eq!(mm.major_step(4.0), 2.0);
        // An inch at 72 ppi and 100% is numbered once and split in eighths.
        let inches = RulerScale::new(Unit::Inches, 72.0, 1000);
        assert_eq!(inches.major_step(1.0), 1.0);
        let ticks = ticks_in(inches, 0.0, 1.0, 0.0, 72.0);
        assert_eq!(ticks.len(), 9);
        assert_eq!(ticks[4].kind, TickKind::Mid);
        assert_eq!(ticks[1].value, 0.125);
        assert_eq!(ticks[8].screen, 72.0);
        // Zoomed in, numbered halves split in sixteenths.
        assert_eq!(inches.major_step(2.0), 0.5);
        assert_eq!(inches.label(0.5), "0.5");
        assert_eq!(inches.label(-0.0), "0");
        assert_eq!(inches.label(1.125), "1.125");
        assert_eq!(cm.label(3.0), "3");
    }

    #[test]
    fn percent_rulers_measure_each_side_and_bad_scales_fall_back() {
        let width = RulerScale::new(Unit::Percent, 72.0, 400);
        let ticks = ticks_in(width, 0.0, 1.0, 0.0, 400.0);
        let hundred = ticks.iter().find(|t| t.value == 100.0).unwrap();
        assert_eq!((hundred.kind, hundred.screen), (TickKind::Major, 400.0));
        // No resolution or no size to measure against: pixels.
        assert_eq!(RulerScale::new(Unit::Inches, 0.0, 400), RulerScale::PIXELS);
        assert_eq!(
            RulerScale::new(Unit::Inches, f32::NAN, 400),
            RulerScale::PIXELS
        );
        assert_eq!(RulerScale::new(Unit::Percent, 72.0, 0), RulerScale::PIXELS);
        // Far out on a huge unit, the tick count stays bounded.
        let tiny = RulerScale::new(Unit::Percent, 72.0, 1);
        assert!(ticks_in(tiny, 0.0, 0.0001, -1e9, 1e9).len() as i64 <= MAX_TICKS + 1);
        let points = RulerScale::new(Unit::Points, 9600.0, 1);
        assert!(ticks_in(points, 0.0, 1e-6, 0.0, 1e9).len() as i64 <= MAX_TICKS + 1);
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

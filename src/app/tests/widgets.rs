//! Widget states: focus rings (#81).

use egui::{Color32, Rect, StrokeKind, pos2, vec2};

use super::{FOCUS_RING_WIDTH, FocusRing};

#[test]
fn the_focus_ring_goes_round_or_inside_the_control() {
    let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(36.0, 22.0));
    let accent = Color32::from_rgb(10, 132, 255);

    let around = FocusRing::Around.shape(rect, 5.0, accent);
    assert_eq!(around.rect, rect.expand(2.0), "2 points clear of the control");
    assert_eq!(around.corner_radius, egui::CornerRadius::same(7));
    assert_eq!(around.stroke_kind, StrokeKind::Outside);

    let inside = FocusRing::Inside.shape(rect, 5.0, accent);
    assert_eq!(inside.rect, rect, "flush with the control's edge");
    assert_eq!(inside.corner_radius, egui::CornerRadius::same(5));
    assert_eq!(inside.stroke_kind, StrokeKind::Inside);

    for ring in [around, inside] {
        assert_eq!(ring.stroke.width, FOCUS_RING_WIDTH);
        assert_eq!(ring.stroke.color, accent);
        assert_eq!(ring.fill, Color32::TRANSPARENT, "a ring, not a fill");
    }
}

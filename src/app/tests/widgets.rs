//! Widget states: focus rings (#81) and centred slider fills (#83).

use egui::{Color32, Rangef, Rect, StrokeKind, pos2, vec2};

use super::{FOCUS_RING_WIDTH, FocusRing, SliderTrack};

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

/// A 200-point rail from x = 0 for a -100..=100 slider: 0 sits at x = 100.
fn track(centered: bool) -> SliderTrack {
    SliderTrack {
        rail: Rect::from_min_max(pos2(0.0, 10.0), pos2(200.0, 13.0)),
        range: -100.0..=100.0,
        logarithmic: false,
        centered,
    }
}

#[test]
fn a_centred_slider_fills_from_zero_to_the_thumb() {
    let centred = track(true);
    // Negative: from the thumb right to the zero position.
    let fill = centred.fill(-50.0);
    assert_eq!(fill.x_range(), Rangef::new(50.0, 100.0));
    assert_eq!(fill.y_range(), centred.rail.y_range(), "the rail's height");
    // Zero, the neutral value: nothing filled.
    let fill = centred.fill(0.0);
    assert_eq!(fill.width(), 0.0);
    assert_eq!(fill.center().x, 100.0);
    // Positive: from the zero position right to the thumb.
    assert_eq!(centred.fill(50.0).x_range(), Rangef::new(100.0, 150.0));
    // The ends, and past them, clamp to the rail.
    assert_eq!(centred.fill(-100.0).x_range(), Rangef::new(0.0, 100.0));
    assert_eq!(centred.fill(250.0).x_range(), Rangef::new(100.0, 200.0));
}

#[test]
fn other_sliders_fill_from_the_left() {
    let plain = track(false);
    assert_eq!(plain.fill(-50.0).x_range(), Rangef::new(0.0, 50.0));
    assert_eq!(plain.fill(0.0).x_range(), Rangef::new(0.0, 100.0));
    assert_eq!(plain.fill(50.0).x_range(), Rangef::new(0.0, 150.0));
    // A centred slider whose range has no 0 inside (Hue while colourising) fills from the left.
    let positive = SliderTrack {
        range: 0.0..=360.0,
        ..track(true)
    };
    assert_eq!(positive.fill(90.0).x_range(), Rangef::new(0.0, 50.0));
}

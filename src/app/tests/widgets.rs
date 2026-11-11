//! Widget states: focus rings (#81), centred slider fills (#83) and disabled controls (#89).

use egui::{Color32, Rangef, Rect, StrokeKind, pos2, vec2};

use super::{FOCUS_RING_WIDTH, FocusRing, SliderTrack};

#[test]
fn the_focus_ring_goes_round_or_inside_the_control() {
    let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(36.0, 22.0));
    let accent = Color32::from_rgb(10, 132, 255);

    let around = FocusRing::Around.shape(rect, 5.0, accent);
    assert_eq!(
        around.rect,
        rect.expand(2.0),
        "2 points clear of the control"
    );
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

/// The text shape painting `needle`, with the colour it resolves to and its opacity.
fn text_shape(output: &egui::FullOutput, needle: &str) -> (Color32, f32) {
    fn find(shape: &egui::Shape, needle: &str) -> Option<(Color32, f32)> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == needle => {
                let section = t.galley.job.sections.first()?.format.color;
                let color = if section == Color32::PLACEHOLDER {
                    t.fallback_color
                } else {
                    section
                };
                Some((t.override_text_color.unwrap_or(color), t.opacity_factor))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|clipped| find(&clipped.shape, needle))
        .unwrap_or_else(|| panic!("no text shape for {needle:?}"))
}

#[test]
fn a_disabled_button_has_flat_muted_unfaded_text() {
    use super::super::theme::{Palette, set_palette};
    for p in [Palette::DARK, Palette::LIGHT] {
        let ctx = egui::Context::default();
        set_palette(&ctx, &p);
        let mut rects = (Rect::NOTHING, Rect::NOTHING);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                rects.0 = ui.add(super::Button::new("Add Path")).rect;
                rects.1 = ui.add_enabled(false, super::Button::new("Rename")).rect;
            });
        });
        let name = if p.dark { "dark" } else { "light" };
        let (enabled, opacity) = text_shape(&output, "Add Path");
        assert_eq!((enabled, opacity), (p.text, 1.0), "{name}: enabled label");
        let (disabled, opacity) = text_shape(&output, "Rename");
        assert_eq!(
            disabled, p.disabled_text,
            "{name}: the muted disabled colour"
        );
        assert_eq!(opacity, 1.0, "{name}: not faded by egui's disabled opacity");

        // The disabled face is a flat fill in its own colour, at full opacity; the enabled
        // button keeps its gradient bezel, which paints no flat fill of that colour.
        let flat = |rect: Rect| {
            output.shapes.iter().any(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape) => {
                    shape.rect == rect
                        && shape.fill == p.control_disabled
                        && shape.stroke.color == p.control_disabled_edge
                }
                _ => false,
            })
        };
        assert!(flat(rects.1), "{name}: the disabled button is flat");
        assert!(!flat(rects.0), "{name}: the enabled button is not");
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

/// The selected segment (Basic, Edited) is tinted and outlined with the accent, so it stands
/// out on the light track too (issue 91); the others are not.
#[test]
fn the_selected_segment_is_tinted_with_the_accent() {
    use super::super::theme::{Palette, contrast_ratio, set_palette};
    for p in [Palette::DARK, Palette::LIGHT] {
        let ctx = egui::Context::default();
        set_palette(&ctx, &p);
        let mut choice = 0;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::segmented(ui, &mut choice, &[(0, "Basic"), (1, "Tone"), (2, "Detail")]);
            });
        });
        let accented: Vec<Rect> = (output.shapes.iter())
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape) if shape.stroke.color == p.accent => Some(shape.rect),
                _ => None,
            })
            .collect();
        let tinted = output.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Rect(shape) => shape.fill == p.accent.gamma_multiply(super::SEGMENT_TINT),
            _ => false,
        });
        let name = if p.dark { "dark" } else { "light" };
        assert_eq!(accented.len(), 1, "{name}: one segment is outlined");
        assert!(tinted, "{name}: and tinted");
        assert!(
            p.segment_track
                .iter()
                .all(|track| contrast_ratio(p.accent, *track) >= 3.0),
            "{name}: the outline reaches 3:1 on the track"
        );
    }
}

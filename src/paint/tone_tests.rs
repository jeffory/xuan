//! Dodge, Burn and Sponge: the range weights and tone formulas, and strokes
//! that change each pixel once from its original value.
use super::tone::{REACH, apply, luma, range_weight};
use super::*;

fn options(mode: PaintMode, mask_target: bool) -> StrokeOptions<'static> {
    StrokeOptions {
        mode,
        mask_target,
        source: None,
        clone_offset: Point::default(),
    }
}

/// A 120 × 40 layer filled with `color`.
fn filled(color: [u8; 4]) -> Document {
    let mut document = Document::new(120, 40).unwrap();
    document.active_mut().unwrap().pixels =
        Some(Arc::new(RgbaImage::from_pixel(120, 40, Rgba(color))));
    document
}

fn tool(range: ToneRange, exposure: f32) -> Brush {
    Brush {
        diameter: 20.0,
        hardness: 1.0,
        opacity: 1.0,
        tone: Tone {
            range,
            exposure,
            saturate: false,
        },
        ..Brush::default()
    }
}

/// One gesture through `points`, a segment per pair as the canvas sends them.
fn gesture(
    document: &mut Document,
    points: &[Point],
    brush: &Brush,
    mode: PaintMode,
    mask: bool,
) -> Result<()> {
    let mut stroke = Stroke::default();
    for pair in points.windows(2) {
        stroke.segment(
            document,
            pair[0],
            pair[1],
            brush,
            brush,
            options(mode, mask),
        )?;
    }
    stroke.finish(document, options(mode, mask))
}

/// Back and forth along y = 20 between x = 20 and x = 100, `passes` times.
fn scrub(passes: usize) -> Vec<Point> {
    (0..=passes)
        .map(|i| Point::new(if i % 2 == 0 { 20.0 } else { 100.0 }, 20.0))
        .collect()
}

fn pixel(document: &Document, x: u32, y: u32) -> [u8; 4] {
    document
        .active()
        .unwrap()
        .pixels
        .as_ref()
        .unwrap()
        .get_pixel(x, y)
        .0
}

fn grey(value: f32) -> [f32; 4] {
    [value, value, value, 1.0]
}

fn full(range: ToneRange) -> Tone {
    Tone {
        range,
        exposure: 1.0,
        saturate: false,
    }
}

fn byte(value: f32) -> u8 {
    (value * 255.0).round() as u8
}

const RANGES: [ToneRange; 3] = [
    ToneRange::Shadows,
    ToneRange::Midtones,
    ToneRange::Highlights,
];

const MODES: [PaintMode; 3] = [PaintMode::Dodge, PaintMode::Burn, PaintMode::Sponge];

#[test]
fn range_weights_split_the_tones_smoothly_and_add_up_to_one() {
    assert_eq!(range_weight(ToneRange::Shadows, 0.0), 1.0);
    assert_eq!(range_weight(ToneRange::Shadows, 0.2), 1.0);
    assert_eq!(range_weight(ToneRange::Shadows, 0.5), 0.0);
    assert_eq!(range_weight(ToneRange::Midtones, 0.0), 0.0);
    assert_eq!(range_weight(ToneRange::Midtones, 0.5), 1.0);
    assert_eq!(range_weight(ToneRange::Midtones, 1.0), 0.0);
    assert_eq!(range_weight(ToneRange::Highlights, 0.5), 0.0);
    assert_eq!(range_weight(ToneRange::Highlights, 0.8), 1.0);
    assert_eq!(range_weight(ToneRange::Highlights, 1.0), 1.0);
    // Halfway down each ramp at a third and two thirds.
    assert!((range_weight(ToneRange::Shadows, 1.0 / 3.0) - 0.5).abs() < 1e-5);
    assert!((range_weight(ToneRange::Highlights, 2.0 / 3.0) - 0.5).abs() < 1e-5);
    for step in 0..=1000 {
        let l = step as f32 / 1000.0;
        let sum: f32 = RANGES.iter().map(|range| range_weight(*range, l)).sum();
        assert!((sum - 1.0).abs() < 1e-5, "{l}: {sum}");
        // Shadows mirror highlights about the middle grey.
        assert!(
            (range_weight(ToneRange::Shadows, l) - range_weight(ToneRange::Highlights, 1.0 - l))
                .abs()
                < 1e-5
        );
        // No jumps: the falloff is smooth.
        for range in RANGES {
            assert!((range_weight(range, l + 0.001) - range_weight(range, l)).abs() < 0.01);
        }
    }
    // Shadows only fall and highlights only rise.
    for step in 0..1000 {
        let (a, b) = (step as f32 / 1000.0, (step + 1) as f32 / 1000.0);
        assert!(range_weight(ToneRange::Shadows, b) <= range_weight(ToneRange::Shadows, a));
        assert!(range_weight(ToneRange::Highlights, b) >= range_weight(ToneRange::Highlights, a));
    }
}

#[test]
fn dodge_lightens_and_burn_darkens_within_their_range() {
    let dark = grey(0.1);
    let middle = grey(0.5);
    let light = grey(0.9);
    for (range, inside, outside) in [
        (ToneRange::Shadows, dark, [middle, light]),
        (ToneRange::Midtones, middle, [dark, light]),
        (ToneRange::Highlights, light, [dark, middle]),
    ] {
        let tone = full(range);
        let dodged = apply(inside, PaintMode::Dodge, tone, 1.0);
        let burned = apply(inside, PaintMode::Burn, tone, 1.0);
        let v = inside[0];
        assert!(
            (dodged[0] - (v + REACH * (1.0 - v))).abs() < 1e-6,
            "{range:?}"
        );
        assert!((burned[0] - (v - REACH * v)).abs() < 1e-6, "{range:?}");
        // Grey stays grey.
        assert_eq!(dodged[0], dodged[1]);
        assert_eq!(burned[1], burned[2]);
        for pixel in outside {
            assert_eq!(
                apply(pixel, PaintMode::Dodge, tone, 1.0),
                pixel,
                "{range:?}"
            );
            assert_eq!(apply(pixel, PaintMode::Burn, tone, 1.0), pixel, "{range:?}");
        }
    }
    // Black can be dodged in the shadows and white burned in the highlights.
    assert_eq!(
        apply(grey(0.0), PaintMode::Dodge, full(ToneRange::Shadows), 1.0)[0],
        REACH
    );
    assert_eq!(
        apply(grey(1.0), PaintMode::Burn, full(ToneRange::Highlights), 1.0)[0],
        1.0 - REACH
    );
}

#[test]
fn exposure_and_coverage_scale_the_effect() {
    let tone = Tone {
        exposure: 0.5,
        ..full(ToneRange::Midtones)
    };
    let half = apply(grey(0.5), PaintMode::Dodge, tone, 1.0);
    assert!((half[0] - (0.5 + 0.5 * REACH * 0.5)).abs() < 1e-6);
    // Half the coverage at full exposure is the same.
    let same = apply(grey(0.5), PaintMode::Dodge, full(ToneRange::Midtones), 0.5);
    assert!((same[0] - half[0]).abs() < 1e-6);
    // No exposure or no coverage changes nothing.
    let none = Tone {
        exposure: 0.0,
        ..full(ToneRange::Midtones)
    };
    for mode in MODES {
        let pixel = [0.4, 0.5, 0.6, 0.7];
        assert_eq!(apply(pixel, mode, none, 1.0), pixel);
        assert_eq!(apply(pixel, mode, full(ToneRange::Midtones), 0.0), pixel);
    }
}

#[test]
fn the_range_follows_luma_and_colour_keeps_its_hue() {
    // Pure blue is dark (luma 0.07), so it is a shadow.
    let blue = [0.0, 0.0, 1.0, 1.0];
    assert!(luma([0.0, 0.0, 1.0]) < 0.1);
    assert_eq!(
        apply(blue, PaintMode::Burn, full(ToneRange::Highlights), 1.0),
        blue
    );
    let burned = apply(blue, PaintMode::Burn, full(ToneRange::Shadows), 1.0);
    assert_eq!(burned, [0.0, 0.0, 0.5, 1.0]);
    // Every channel moves the same part of the way, so the hue holds.
    let orange = [0.8, 0.4, 0.2, 1.0];
    let dodged = apply(orange, PaintMode::Dodge, full(ToneRange::Midtones), 1.0);
    let k = (dodged[0] - orange[0]) / (1.0 - orange[0]);
    assert!(k > 0.0);
    for c in 0..3 {
        assert!((dodged[c] - (orange[c] + k * (1.0 - orange[c]))).abs() < 1e-6);
    }
    assert!(dodged[0] > dodged[1] && dodged[1] > dodged[2]);
}

#[test]
fn sponge_desaturates_to_grey_and_saturates_inside_the_gamut() {
    let orange = [0.8, 0.5, 0.3, 1.0];
    let y = luma([orange[0], orange[1], orange[2]]);
    let desaturate = full(ToneRange::Midtones);
    let saturate = Tone {
        saturate: true,
        ..desaturate
    };
    let grey = apply(orange, PaintMode::Sponge, desaturate, 1.0);
    for c in &grey[..3] {
        assert!((c - y).abs() < 1e-6);
    }
    let half = apply(orange, PaintMode::Sponge, desaturate, 0.5);
    assert!((half[0] - (y + (orange[0] - y) * 0.5)).abs() < 1e-6);
    // Saturating keeps the luma and stops at the edge of the gamut, where
    // red would go past 1 at double the saturation.
    let rich = apply(orange, PaintMode::Sponge, saturate, 1.0);
    assert!((luma([rich[0], rich[1], rich[2]]) - y).abs() < 1e-5);
    assert!((rich[0] - 1.0).abs() < 1e-5, "{rich:?}");
    assert!(rich[2] < orange[2]);
    let ratio = (rich[2] - y) / (orange[2] - y);
    assert!(ratio > 1.0);
    assert!(((rich[1] - y) / (orange[1] - y) - ratio).abs() < 1e-3);
    // A dull colour doubles its saturation.
    let dull = [0.55, 0.5, 0.45, 1.0];
    let y = luma([0.55, 0.5, 0.45]);
    let doubled = apply(dull, PaintMode::Sponge, saturate, 1.0);
    assert!((doubled[0] - (y + (0.55 - y) * 2.0)).abs() < 1e-6);
    // Grey has no saturation to change, and Sponge ignores the range.
    let flat = [0.3, 0.3, 0.3, 1.0];
    let flat_out = apply(flat, PaintMode::Sponge, saturate, 1.0);
    for c in &flat_out[..3] {
        assert!((c - 0.3).abs() < 1e-6);
    }
    for range in RANGES {
        let tone = Tone {
            range,
            ..desaturate
        };
        assert_eq!(apply(orange, PaintMode::Sponge, tone, 1.0), grey);
    }
}

#[test]
fn alpha_is_unchanged_and_transparent_pixels_stay_as_they_are() {
    let pixel = [0.5, 0.4, 0.3, 0.25];
    for mode in MODES {
        assert_eq!(apply(pixel, mode, full(ToneRange::Midtones), 1.0)[3], 0.25);
        let hidden = [0.5, 0.4, 0.3, 0.0];
        assert_eq!(apply(hidden, mode, full(ToneRange::Midtones), 1.0), hidden);
    }
}

#[test]
fn a_dodge_stroke_lightens_under_the_brush_only() {
    let mut document = filled([128, 128, 128, 200]);
    let brush = tool(ToneRange::Midtones, 0.5);
    gesture(
        &mut document,
        &[Point::new(20.0, 20.0), Point::new(100.0, 20.0)],
        &brush,
        PaintMode::Dodge,
        false,
    )
    .unwrap();
    let expected = byte(apply(grey(128.0 / 255.0), PaintMode::Dodge, brush.tone, 1.0)[0]);
    assert!(expected > 128);
    assert_eq!(
        pixel(&document, 60, 20),
        [expected, expected, expected, 200]
    );
    // Outside the brush, untouched.
    assert_eq!(pixel(&document, 60, 2), [128, 128, 128, 200]);
    assert_eq!(pixel(&document, 115, 20), [128, 128, 128, 200]);
    // Alpha everywhere unchanged.
    let layer = document.active().unwrap().pixels.as_ref().unwrap();
    assert!(layer.pixels().all(|p| p[3] == 200));
}

#[test]
fn each_mode_keeps_to_its_range_in_a_stroke() {
    // Left half dark, right half light.
    let mut document = Document::new(120, 40).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_fn(120, 40, |x, _| {
        if x < 60 {
            Rgba([30, 30, 30, 255])
        } else {
            Rgba([230, 230, 230, 255])
        }
    })));
    let points = [Point::new(20.0, 20.0), Point::new(100.0, 20.0)];
    let run = |range, mode| {
        let mut document = document.clone();
        gesture(&mut document, &points, &tool(range, 1.0), mode, false).unwrap();
        (pixel(&document, 30, 20)[0], pixel(&document, 90, 20)[0])
    };
    assert_eq!(run(ToneRange::Highlights, PaintMode::Burn), (30, 115));
    assert_eq!(run(ToneRange::Highlights, PaintMode::Dodge), (30, 243));
    assert_eq!(run(ToneRange::Shadows, PaintMode::Dodge), (143, 230));
    assert_eq!(run(ToneRange::Shadows, PaintMode::Burn), (15, 230));
    // Neither is a midtone.
    assert_eq!(run(ToneRange::Midtones, PaintMode::Dodge), (30, 230));
}

#[test]
fn going_over_the_same_spot_in_one_stroke_does_not_compound() {
    for mode in MODES {
        let brush = Brush {
            hardness: 0.5,
            ..tool(ToneRange::Midtones, 0.8)
        };
        let mut once = filled([150, 110, 90, 255]);
        let mut scrubbed = once.clone();
        gesture(&mut once, &scrub(1), &brush, mode, false).unwrap();
        gesture(&mut scrubbed, &scrub(9), &brush, mode, false).unwrap();
        assert_eq!(
            once.active().unwrap().pixels,
            scrubbed.active().unwrap().pixels,
            "{mode:?}"
        );
        let first = pixel(&once, 60, 20);
        assert_ne!(first, [150, 110, 90, 255], "{mode:?}");
        // A second stroke does go further.
        gesture(&mut once, &scrub(1), &brush, mode, false).unwrap();
        assert_ne!(pixel(&once, 60, 20), first, "{mode:?}");
    }
}

#[test]
fn overlapping_dabs_do_not_compound() {
    // Dabs a tenth of the size apart overlap many times over.
    let mut brush = tool(ToneRange::Midtones, 1.0);
    brush.hardness = 0.3;
    brush.dynamics.spacing = 0.1;
    let mut document = filled([120, 120, 120, 255]);
    gesture(
        &mut document,
        &[Point::new(20.0, 20.0), Point::new(100.0, 20.0)],
        &brush,
        PaintMode::Burn,
        false,
    )
    .unwrap();
    // On the line the dab centred there covers the pixel fully, so it is
    // burned once at full coverage, however many other dabs reach it.
    let once = byte(apply(grey(120.0 / 255.0), PaintMode::Burn, brush.tone, 1.0)[0]);
    assert_eq!(pixel(&document, 60, 20)[0], once);
    // Between the lines nothing went further than that.
    let layer = document.active().unwrap().pixels.as_ref().unwrap();
    assert!(layer.pixels().all(|p| p[0] >= once && p[3] == 255));
}

#[test]
fn zero_exposure_leaves_the_pixels_alone() {
    let mut document = filled([128, 128, 128, 255]);
    let before = document.active().unwrap().pixels.clone().unwrap();
    for mode in MODES {
        gesture(
            &mut document,
            &scrub(3),
            &tool(ToneRange::Midtones, 0.0),
            mode,
            false,
        )
        .unwrap();
    }
    let after = document.active().unwrap().pixels.clone().unwrap();
    // Not even copied.
    assert!(Arc::ptr_eq(&before, &after));
}

#[test]
fn tone_modes_are_refused_on_a_mask() {
    let mut document = filled([128, 128, 128, 255]);
    document.active_mut().unwrap().mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_pixel(120, 40, Luma([100]))),
        ..Mask::white()
    });
    let before = document.active().unwrap().mask.clone().unwrap().pixels;
    for mode in MODES {
        let error = gesture(
            &mut document,
            &scrub(1),
            &tool(ToneRange::Midtones, 1.0),
            mode,
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not masks"), "{error}");
    }
    let layer = document.active().unwrap();
    assert!(Arc::ptr_eq(&before, &layer.mask.as_ref().unwrap().pixels));
    assert!(layer.pixels.as_ref().unwrap().pixels().all(|p| p[0] == 128));
}

#[test]
fn tone_strokes_stay_inside_the_selection() {
    let mut document = filled([128, 128, 128, 255]);
    // Only the left half is selected.
    document.selection = Some(Arc::new(GrayImage::from_fn(120, 40, |x, _| {
        Luma([if x < 60 { 255 } else { 0 }])
    })));
    for mode in MODES {
        let mut document = document.clone();
        let mut brush = tool(ToneRange::Midtones, 1.0);
        brush.tone.saturate = true;
        document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_pixel(
            120,
            40,
            Rgba([150, 110, 90, 255]),
        )));
        gesture(
            &mut document,
            &[Point::new(20.0, 20.0), Point::new(100.0, 20.0)],
            &brush,
            mode,
            false,
        )
        .unwrap();
        assert_ne!(pixel(&document, 30, 20), [150, 110, 90, 255], "{mode:?}");
        assert_eq!(pixel(&document, 90, 20), [150, 110, 90, 255], "{mode:?}");
    }
}

#[test]
fn locked_layers_are_refused() {
    let mut document = filled([128, 128, 128, 255]);
    document.active_mut().unwrap().locked = true;
    assert!(
        gesture(
            &mut document,
            &scrub(1),
            &tool(ToneRange::Midtones, 1.0),
            PaintMode::Dodge,
            false,
        )
        .is_err()
    );
}

#[test]
fn transparent_pixels_stay_transparent_and_the_layer_does_not_grow() {
    let mut document = Document::new(120, 40).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::new(120, 40)));
    let before = document.active().unwrap().transform;
    // Off the edge of the layer too: unlike the Brush, nothing is added there.
    gesture(
        &mut document,
        &[Point::new(-20.0, 20.0), Point::new(100.0, 20.0)],
        &tool(ToneRange::Shadows, 1.0),
        PaintMode::Dodge,
        false,
    )
    .unwrap();
    let layer = document.active().unwrap();
    assert_eq!(layer.transform, before);
    assert!(
        layer
            .pixels
            .as_ref()
            .unwrap()
            .as_raw()
            .iter()
            .all(|v| *v == 0)
    );
}

#[test]
fn symmetric_copies_share_one_coverage() {
    // A vertical mirror at x = 60: the stroke across it overlaps its own
    // copy, which must not burn the overlap twice.
    let mut brush = tool(ToneRange::Midtones, 1.0);
    brush.symmetry.mode = SymmetryMode::Vertical;
    let mut document = filled([120, 120, 120, 255]);
    gesture(
        &mut document,
        &[Point::new(40.0, 20.0), Point::new(70.0, 20.0)],
        &brush,
        PaintMode::Burn,
        false,
    )
    .unwrap();
    let once = byte(apply(grey(120.0 / 255.0), PaintMode::Burn, brush.tone, 1.0)[0]);
    assert_eq!(pixel(&document, 60, 20)[0], once);
    // The mirrored copy runs from x = 80 back to 50, with the brush's
    // 10 px radius around both.
    assert_eq!(pixel(&document, 85, 20)[0], once);
    assert_eq!(pixel(&document, 25, 20)[0], 120);
    assert_eq!(pixel(&document, 95, 20)[0], 120);
}

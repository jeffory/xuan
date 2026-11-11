use super::*;

fn options(mode: PaintMode, mask_target: bool) -> StrokeOptions<'static> {
    StrokeOptions {
        mode,
        mask_target,
        source: None,
        clone_offset: Point::default(),
    }
}

fn blank(width: u32, height: u32) -> Document {
    let mut document = Document::new(width, height).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::new(width, height)));
    document
}

fn opaque(width: u32, height: u32) -> Document {
    let mut document = Document::new(width, height).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_pixel(
        width,
        height,
        Rgba([40, 120, 200, 255]),
    )));
    document
}

fn hard(flow: f32, opacity: f32) -> Brush {
    Brush {
        diameter: 20.0,
        hardness: 1.0,
        opacity,
        flow,
        color: [0, 0, 0, 255],
        ..Brush::default()
    }
}

/// One gesture through `points`, a segment per pair as the canvas sends them.
fn gesture(document: &mut Document, points: &[Point], brush: &Brush, mode: PaintMode, mask: bool) {
    let mut stroke = Stroke::default();
    for pair in points.windows(2) {
        stroke
            .segment(
                document,
                pair[0],
                pair[1],
                brush,
                brush,
                options(mode, mask),
            )
            .unwrap();
    }
    stroke.finish(document, options(mode, mask)).unwrap();
}

/// Back and forth along y = 20 between x = 20 and x = 100, `passes` times.
fn scrub(passes: usize) -> Vec<Point> {
    (0..=passes)
        .map(|i| Point::new(if i % 2 == 0 { 20.0 } else { 100.0 }, 20.0))
        .collect()
}

fn alpha(document: &Document, x: u32, y: u32) -> u8 {
    document
        .active()
        .unwrap()
        .pixels
        .as_ref()
        .unwrap()
        .get_pixel(x, y)[3]
}

fn mask_value(document: &Document, x: u32, y: u32) -> u8 {
    document
        .active()
        .unwrap()
        .mask
        .as_ref()
        .unwrap()
        .pixels
        .get_pixel(x, y)[0]
}

#[test]
fn build_up_at_full_flow_is_the_strongest_coverage() {
    let values = [0.0, 1e-7, 0.1, 0.3, 0.333_333_34, 0.5, 0.7, 0.999_999, 1.0];
    for previous in values {
        for amount in values {
            // Bit for bit what overlapping segments did before flow.
            assert_eq!(
                build_up(previous, amount, 1.0).to_bits(),
                previous.max(amount).to_bits()
            );
            for flow in [0.01, 0.2, 0.5, 0.99] {
                let next = build_up(previous, amount, flow);
                assert!(
                    next >= previous && next <= previous.max(amount),
                    "{previous} {amount} {flow}"
                );
            }
        }
    }
    assert!((build_up(0.0, 0.5, 0.2) - 0.1).abs() < 1e-6);
    assert!((build_up(0.1, 0.5, 0.2) - 0.18).abs() < 1e-6);
}

#[test]
fn dab_flow_lays_down_the_flow_in_one_pass_at_any_spacing() {
    for flow in [0.05, 0.2, 0.6] {
        for step in [0.02, 0.1, 0.25, 0.5] {
            let dab = dynamics::dab_flow(flow, step);
            let pass = 1.0 - (1.0 - dab).powf(1.0 / step);
            assert!((pass - flow).abs() < 1e-4, "{flow} {step} {pass}");
        }
        // Dabs a diameter or more apart do not overlap: each takes the flow.
        assert_eq!(dynamics::dab_flow(flow, 1.0), flow);
        assert_eq!(dynamics::dab_flow(flow, 3.0), flow);
    }
    assert_eq!(dynamics::dab_flow(1.0, 0.1), 1.0);
}

#[test]
fn low_flow_is_light_in_one_pass_and_builds_up_to_the_opacity() {
    for opacity in [1.0, 0.5] {
        let cap = (opacity * 255.0_f32).round() as u8;
        let mut previous = 0;
        for passes in [1, 2, 4, 8, 30] {
            let mut document = blank(120, 40);
            gesture(
                &mut document,
                &scrub(passes),
                &hard(0.2, opacity),
                PaintMode::Paint,
                false,
            );
            let value = alpha(&document, 60, 20);
            if passes == 1 {
                // One pass lays down about a fifth of the opacity.
                let expected = 0.2 * f32::from(cap);
                assert!(
                    (f32::from(value) - expected).abs() <= 0.25 * expected + 2.0,
                    "{value}"
                );
            } else {
                assert!(
                    value > previous,
                    "{passes} passes: {value} after {previous}"
                );
            }
            assert!(value <= cap, "{value} passes the opacity {cap}");
            previous = value;
        }
        // Scrubbing long enough reaches the opacity, as Flow 100% does at once.
        assert!(previous.saturating_add(2) >= cap, "{previous} of {cap}");
        let mut full = blank(120, 40);
        gesture(
            &mut full,
            &scrub(30),
            &hard(1.0, opacity),
            PaintMode::Paint,
            false,
        );
        assert_eq!(alpha(&full, 60, 20), cap);
    }
}

#[test]
fn low_flow_does_not_depend_on_how_often_the_pointer_reports() {
    let brush = Brush {
        hardness: 0.5,
        ..hard(0.3, 0.8)
    };
    let mut sparse = blank(120, 40);
    gesture(
        &mut sparse,
        &[Point::new(15.0, 20.0), Point::new(105.0, 20.0)],
        &brush,
        PaintMode::Paint,
        false,
    );
    let mut dense = blank(120, 40);
    let points: Vec<_> = (0..=90)
        .map(|i| Point::new(15.0 + i as f32, 20.0))
        .collect();
    gesture(&mut dense, &points, &brush, PaintMode::Paint, false);
    let a = sparse.active().unwrap().pixels.as_ref().unwrap();
    let b = dense.active().unwrap().pixels.as_ref().unwrap();
    assert!(
        a.as_raw()
            .iter()
            .zip(b.as_raw())
            .all(|(a, b)| a.abs_diff(*b) <= 1)
    );
    // Even along the stroke: no darker beads where samples join.
    let row: Vec<u8> = (30..90).map(|x| alpha(&dense, x, 20)).collect();
    let (low, high) = (row.iter().min().unwrap(), row.iter().max().unwrap());
    assert!(high - low <= 3, "{row:?}");
}

#[test]
fn a_live_low_flow_stroke_matches_its_whole_path() {
    let brush = Brush {
        hardness: 0.7,
        ..hard(0.25, 0.9)
    };
    let points = [
        Point::new(10.0, 10.0),
        Point::new(70.0, 30.0),
        Point::new(20.0, 32.0),
        Point::new(100.0, 12.0),
    ];
    let mut live = blank(120, 40);
    gesture(&mut live, &points, &brush, PaintMode::Paint, false);
    let mut path = blank(120, 40);
    let samples: Vec<_> = points.iter().map(|p| (*p, brush.clone())).collect();
    Stroke::default()
        .path(&mut path, &samples, options(PaintMode::Paint, false))
        .unwrap();
    let a = live.active().unwrap().pixels.as_ref().unwrap();
    let b = path.active().unwrap().pixels.as_ref().unwrap();
    assert!(
        a.as_raw()
            .iter()
            .zip(b.as_raw())
            .all(|(a, b)| a.abs_diff(*b) <= 1)
    );
}

#[test]
fn the_eraser_builds_up_with_flow() {
    let mut previous = 255;
    for passes in [1, 3, 30] {
        let mut document = opaque(120, 40);
        gesture(
            &mut document,
            &scrub(passes),
            &hard(0.2, 1.0),
            PaintMode::Erase,
            false,
        );
        let value = alpha(&document, 60, 20);
        if passes == 1 {
            assert!((190..=215).contains(&value), "{value}");
        }
        assert!(value < previous, "{passes} passes: {value}");
        previous = value;
    }
    assert!(previous <= 3, "{previous}");
    // Erasing at half opacity never removes more than half.
    let mut document = opaque(120, 40);
    gesture(
        &mut document,
        &scrub(30),
        &hard(0.2, 0.5),
        PaintMode::Erase,
        false,
    );
    assert!((127..=130).contains(&alpha(&document, 60, 20)));
}

#[test]
fn masks_build_up_with_flow() {
    for (mode, start) in [(PaintMode::Paint, 255), (PaintMode::Erase, 255)] {
        let mut previous = start;
        for passes in [1, 3, 30] {
            let mut document = opaque(120, 40);
            prepare_mask(document.active_mut().unwrap()).unwrap();
            gesture(&mut document, &scrub(passes), &hard(0.2, 0.6), mode, true);
            let value = mask_value(&document, 60, 20);
            if passes == 1 {
                // A fifth of the way from white to black at 60% opacity.
                let expected = 255.0 - 0.2 * 0.6 * 255.0;
                assert!((f32::from(value) - expected).abs() <= 8.0, "{value}");
            }
            assert!(value < previous, "{mode:?} {passes}: {value}");
            // Never darker than the opacity allows.
            assert!(value >= 101, "{value}");
            previous = value;
        }
        assert!(previous <= 105, "{previous}");
        // The pixels are untouched.
        let mut document = opaque(120, 40);
        prepare_mask(document.active_mut().unwrap()).unwrap();
        gesture(&mut document, &scrub(3), &hard(0.2, 0.6), mode, true);
        assert_eq!(alpha(&document, 60, 20), 255);
    }
}

#[test]
fn pressure_scales_the_flow_along_the_stroke() {
    // The canvas multiplies the flow by the pen pressure for each sample.
    let samples: Vec<_> = (0..=8)
        .map(|i| {
            let pressure = 0.1 + 0.9 * i as f32 / 8.0;
            (
                Point::new(15.0 + i as f32 * 12.0, 20.0),
                Brush {
                    flow: pressure,
                    ..hard(1.0, 1.0)
                },
            )
        })
        .collect();
    let mut document = blank(130, 40);
    let mut stroke = Stroke::default();
    for pair in samples.windows(2) {
        stroke
            .segment(
                &mut document,
                pair[0].0,
                pair[1].0,
                &pair[0].1,
                &pair[1].1,
                options(PaintMode::Paint, false),
            )
            .unwrap();
    }
    let light = alpha(&document, 30, 20);
    let middle = alpha(&document, 65, 20);
    let heavy = alpha(&document, 100, 20);
    assert!(light < middle && middle < heavy, "{light} {middle} {heavy}");
    assert!(light < 80, "{light}");
    assert!(heavy > 200, "{heavy}");
}

#[test]
fn the_pencil_and_retouch_brushes_ignore_flow() {
    let points = scrub(3);
    for mode in [PaintMode::Pencil, PaintMode::Smudge] {
        let paint = |flow: f32| {
            let mut document = opaque(120, 40);
            let source = render::render(&document);
            let brush = Brush {
                color: [250, 20, 20, 255],
                ..hard(flow, 0.7)
            };
            let mut stroke = Stroke::default();
            for pair in points.windows(2) {
                stroke
                    .segment(
                        &mut document,
                        pair[0],
                        pair[1],
                        &brush,
                        &brush,
                        StrokeOptions {
                            source: Some(&source),
                            clone_offset: Point::new(3.0, 1.0),
                            ..options(mode, false)
                        },
                    )
                    .unwrap();
            }
            document
                .active()
                .unwrap()
                .pixels
                .as_ref()
                .unwrap()
                .as_raw()
                .clone()
        };
        assert_eq!(paint(0.2), paint(1.0), "{mode:?}");
    }
}

#[test]
fn low_flow_still_tapers_and_scatters() {
    // Flow builds up through the brush dynamics' dabs too.
    let dynamics = Dynamics {
        taper_in: 30.0,
        taper_out: 30.0,
        taper_opacity: true,
        spacing: 0.3,
        ..Dynamics::default()
    };
    let brush = |flow| Brush {
        dynamics,
        ..hard(flow, 1.0)
    };
    let paint = |flow| {
        let mut document = blank(120, 40);
        let samples: Vec<_> = scrub(1).into_iter().map(|p| (p, brush(flow))).collect();
        Stroke::default()
            .path(&mut document, &samples, options(PaintMode::Paint, false))
            .unwrap();
        document
    };
    let (low, full) = (paint(0.3), paint(1.0));
    // Lighter in the middle, and tapered to nothing at both ends in both.
    assert!(alpha(&low, 60, 20) < alpha(&full, 60, 20) / 2);
    assert!(alpha(&low, 60, 20) > 0);
    for document in [&low, &full] {
        assert!(alpha(document, 21, 20) < alpha(document, 60, 20));
    }
}

use super::*;

fn options(mode: PaintMode) -> StrokeOptions<'static> {
    StrokeOptions {
        mode,
        mask_target: false,
        source: None,
        clone_offset: Point::default(),
    }
}

fn blank(width: u32, height: u32) -> Document {
    let mut document = Document::new(width, height).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::new(width, height)));
    document
}

/// The rendered image, with the colour of fully transparent pixels cleared.
fn pixels(document: &Document) -> RgbaImage {
    let mut image = render::render(document);
    for pixel in image.pixels_mut() {
        if pixel[3] == 0 {
            *pixel = Rgba([0; 4]);
        }
    }
    image
}

fn brush(symmetry: Symmetry, dynamics: Dynamics) -> Brush {
    Brush {
        diameter: 14.0,
        hardness: 0.6,
        opacity: 0.9,
        color: [200, 60, 30, 255],
        dynamics,
        symmetry,
        ..Brush::default()
    }
}

fn mode(mode: SymmetryMode) -> Symmetry {
    Symmetry {
        mode,
        ..Symmetry::default()
    }
}

/// A wavy path in the top-left part of the canvas.
fn wave(brush: &Brush) -> Vec<(Point, Brush)> {
    (0..=16)
        .map(|i| {
            let t = i as f32 / 16.0;
            let point = Point::new(20.0 + t * 60.0, 25.0 + (t * 7.0).sin() * 12.0);
            (point, brush.clone())
        })
        .collect()
}

fn paint(document: &mut Document, samples: &[(Point, Brush)], mode: PaintMode) {
    Stroke::default()
        .path(document, samples, options(mode))
        .unwrap();
}

fn scattered(seed: u64) -> Dynamics {
    Dynamics {
        spacing: 0.4,
        scatter: 1.5,
        count: 2,
        size_jitter: 0.5,
        opacity_jitter: 0.5,
        hue_jitter: 0.4,
        seed,
        ..Dynamics::default()
    }
}

fn largest_difference(a: &RgbaImage, b: &RgbaImage) -> u8 {
    a.pixels()
        .zip(b.pixels())
        .flat_map(|(a, b)| (0..4).map(move |i| a[i].abs_diff(b[i])))
        .max()
        .unwrap_or(0)
}

#[test]
fn vertical_and_horizontal_symmetry_mirror_the_pixels() {
    for dynamics in [
        Dynamics::default(),
        scattered(5),
        Dynamics {
            taper_in: 20.0,
            taper_out: 30.0,
            ..Dynamics::default()
        },
    ] {
        let flips: [(SymmetryMode, fn(&RgbaImage) -> RgbaImage); 2] = [
            (SymmetryMode::Vertical, image::imageops::flip_horizontal),
            (SymmetryMode::Horizontal, image::imageops::flip_vertical),
        ];
        for (axis, flip) in flips {
            for paint_mode in [PaintMode::Paint, PaintMode::Pencil] {
                let mut document = blank(200, 120);
                paint(
                    &mut document,
                    &wave(&brush(mode(axis), dynamics)),
                    paint_mode,
                );
                let image = pixels(&document);
                assert!(image.pixels().any(|p| p[3] > 0));
                let difference = largest_difference(&image, &flip(&image));
                assert!(
                    difference <= 1,
                    "{axis:?} {paint_mode:?} {dynamics:?}: {difference}"
                );
                // The drawn stroke is painted as it would be without
                // symmetry, away from where its copy reaches.
                let mut plain = blank(200, 120);
                paint(
                    &mut plain,
                    &wave(&brush(Symmetry::default(), dynamics)),
                    paint_mode,
                );
                let plain = pixels(&plain);
                // (The layer may have grown differently, which can round a
                // pixel by one.)
                for (x, y, pixel) in plain.enumerate_pixels() {
                    if x < 90 && y < 55 {
                        let other = image.get_pixel(x, y);
                        assert!(
                            (0..4).all(|i| other[i].abs_diff(pixel[i]) <= 1),
                            "{axis:?} at {x}, {y}: {other:?} {pixel:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn radial_symmetry_turns_the_stroke_around_the_centre() {
    for (segments, dynamics) in [
        (6, Dynamics::default()),
        (5, Dynamics::default()),
        (
            12,
            Dynamics {
                spacing: 0.3,
                scatter: 1.0,
                size_jitter: 0.4,
                seed: 9,
                ..Dynamics::default()
            },
        ),
    ] {
        let center = Point::new(130.0, 120.0);
        let symmetry = Symmetry {
            mode: SymmetryMode::Radial,
            segments,
            center: Some(center),
        };
        let stroke = Brush {
            hardness: 1.0,
            opacity: 1.0,
            ..brush(symmetry, dynamics)
        };
        let mut document = blank(260, 240);
        paint(&mut document, &wave(&stroke), PaintMode::Paint);
        let image = pixels(&document);
        let alpha = |x: i32, y: i32| {
            (x >= 0 && y >= 0 && x < 260 && y < 240).then(|| image.get_pixel(x as u32, y as u32)[3])
        };
        // Where a pixel and its neighbours are all painted (or all clear),
        // so is the pixel each turn takes it to.
        let uniform = |x: i32, y: i32| {
            let first = alpha(x, y)?;
            let all = (-2..=2).all(|dy| (-2..=2).all(|dx| alpha(x + dx, y + dy) == Some(first)));
            (all && (first == 0 || first == 255)).then_some(first)
        };
        let (mut painted, mut clear) = (0, 0);
        for y in (0..240).step_by(2) {
            for x in (0..260).step_by(2) {
                let Some(value) = uniform(x, y) else {
                    continue;
                };
                if value == 255 {
                    painted += 1;
                } else {
                    clear += 1;
                }
                for k in 1..segments {
                    let angle = std::f32::consts::TAU * k as f32 / segments as f32;
                    let (dx, dy) = (x as f32 + 0.5 - center.x, y as f32 + 0.5 - center.y);
                    let turned = (
                        (center.x + dx * angle.cos() - dy * angle.sin() - 0.5).round() as i32,
                        (center.y + dx * angle.sin() + dy * angle.cos() - 0.5).round() as i32,
                    );
                    if let Some(other) = alpha(turned.0, turned.1) {
                        assert_eq!(
                            other, value,
                            "{segments} segments: ({x}, {y}) turned {k} times is {turned:?}"
                        );
                    }
                }
            }
        }
        assert!(painted > 20 && clear > 100, "{painted} {clear}");
    }
}

#[test]
fn copies_that_overlap_take_the_strongest_coverage_once() {
    // A half-opaque stroke across the axis: both copies cover the same
    // pixels, which must look like one stroke, not two layered.
    let line = |symmetry| {
        let brush = Brush {
            opacity: 0.5,
            hardness: 1.0,
            ..brush(symmetry, Dynamics::default())
        };
        let samples = [
            (Point::new(40.0, 30.0), brush.clone()),
            (Point::new(160.0, 30.0), brush.clone()),
            (Point::new(100.0, 50.0), brush),
        ];
        let mut document = blank(200, 60);
        paint(&mut document, &samples, PaintMode::Paint);
        pixels(&document)
    };
    let single = line(Symmetry::default());
    let mirrored = line(mode(SymmetryMode::Vertical));
    let centre = mirrored.get_pixel(100, 30)[3];
    assert_eq!(centre, single.get_pixel(100, 30)[3]);
    assert!((126..=129).contains(&centre), "{centre}");
    assert!(mirrored.pixels().all(|p| p[3] <= 129));
    // Twelve copies around a centre all cross there: still one coat.
    let radial = line(Symmetry {
        mode: SymmetryMode::Radial,
        segments: 12,
        center: Some(Point::new(100.0, 30.0)),
    });
    assert!(radial.pixels().all(|p| p[3] <= 129));
    assert_eq!(radial.get_pixel(100, 30)[3], centre);
}

#[test]
fn symmetric_dynamics_repeat_for_a_seed_and_mirror_each_dab() {
    let run = |seed| {
        let mut document = blank(200, 120);
        paint(
            &mut document,
            &wave(&brush(mode(SymmetryMode::Vertical), scattered(seed))),
            PaintMode::Paint,
        );
        pixels(&document)
    };
    let first = run(3);
    assert_eq!(first, run(3), "the same seed paints the same pixels");
    assert_ne!(first, run(4), "another seed paints other pixels");
    // Every copy has the same scatter and jitter, mirrored: the copy on the
    // right is the left half flipped, colours included.
    let flipped = image::imageops::flip_horizontal(&first);
    assert!(largest_difference(&first, &flipped) <= 1);
}

#[test]
fn erasing_with_symmetry_clears_both_sides_once() {
    let mut document = blank(200, 120);
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_pixel(
        200,
        120,
        Rgba([20, 90, 160, 255]),
    )));
    let eraser = Brush {
        opacity: 0.5,
        ..brush(mode(SymmetryMode::Horizontal), Dynamics::default())
    };
    // Crossing the axis, so the copies overlap in the middle.
    let samples = [
        (Point::new(30.0, 40.0), eraser.clone()),
        (Point::new(60.0, 80.0), eraser),
    ];
    paint(&mut document, &samples, PaintMode::Erase);
    let image = pixels(&document);
    let difference = largest_difference(&image, &image::imageops::flip_vertical(&image));
    assert!(difference <= 1, "{difference}");
    assert!(image.pixels().all(|p| p[3] >= 126), "never erased twice");
    assert!(image.get_pixel(45, 60)[3] < 140);
}

#[test]
fn a_live_symmetric_stroke_matches_its_replay() {
    for dynamics in [
        Dynamics::default(),
        Dynamics {
            taper_out: 30.0,
            ..scattered(2)
        },
    ] {
        let symmetry = Symmetry {
            mode: SymmetryMode::Radial,
            segments: 3,
            center: None,
        };
        let samples = wave(&brush(symmetry, dynamics));
        let mut live = blank(200, 120);
        let mut stroke = Stroke::default();
        let (mut last, mut last_brush) = samples[0].clone();
        for (point, brush) in std::iter::once(samples[0].clone()).chain(samples.clone()) {
            stroke
                .segment(
                    &mut live,
                    last,
                    point,
                    &last_brush,
                    &brush,
                    options(PaintMode::Paint),
                )
                .unwrap();
            (last, last_brush) = (point, brush);
        }
        stroke.finish(&mut live, options(PaintMode::Paint)).unwrap();
        let mut replay = blank(200, 120);
        let mut with_press = vec![samples[0].clone()];
        with_press.extend(samples);
        paint(&mut replay, &with_press, PaintMode::Paint);
        assert_eq!(pixels(&live), pixels(&replay), "{dynamics:?}");
    }
}

#[test]
fn other_tools_and_symmetry_off_paint_one_stroke() {
    // Off with a centre set paints as no symmetry at all.
    let off = Symmetry {
        center: Some(Point::new(3.0, 4.0)),
        segments: 9,
        ..Symmetry::default()
    };
    let mut a = blank(200, 120);
    paint(&mut a, &wave(&brush(off, scattered(1))), PaintMode::Paint);
    let mut b = blank(200, 120);
    paint(
        &mut b,
        &wave(&brush(Symmetry::default(), scattered(1))),
        PaintMode::Paint,
    );
    assert_eq!(pixels(&a), pixels(&b));
    // Retouch tools ignore symmetry: a clone stroke paints only where drawn.
    let mut source = RgbaImage::from_pixel(200, 120, Rgba([0, 200, 0, 255]));
    source.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
    let mut document = blank(200, 120);
    Stroke::default()
        .path(
            &mut document,
            &wave(&brush(mode(SymmetryMode::Vertical), Dynamics::default())),
            StrokeOptions {
                mode: PaintMode::Clone,
                mask_target: false,
                source: Some(&source),
                clone_offset: Point::default(),
            },
        )
        .unwrap();
    let image = pixels(&document);
    assert!((0..120).all(|y| (100..200).all(|x| image.get_pixel(x, y)[3] == 0)));
    assert!(image.pixels().any(|p| p[3] > 0));
}

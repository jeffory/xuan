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

/// The rendered image, with the colour of fully transparent pixels cleared:
/// it is invisible, and depends on how far the layer has grown.
fn pixels(document: &Document) -> RgbaImage {
    let mut image = render::render(document);
    for pixel in image.pixels_mut() {
        if pixel[3] == 0 {
            *pixel = Rgba([0; 4]);
        }
    }
    image
}

fn hard(diameter: f32, dynamics: Dynamics) -> Brush {
    Brush {
        diameter,
        hardness: 1.0,
        color: [200, 60, 30, 255],
        dynamics,
        ..Brush::default()
    }
}

fn line(from: Point, to: Point, brush: &Brush, n: usize) -> Vec<(Point, Brush)> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            (
                Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t),
                brush.clone(),
            )
        })
        .collect()
}

fn paint_path(document: &mut Document, samples: &[(Point, Brush)], mode: PaintMode) {
    Stroke::default()
        .path(document, samples, options(mode))
        .unwrap();
}

/// Rows painted in a column: the stroke's width there.
fn width_at(image: &RgbaImage, x: u32) -> u32 {
    (0..image.height())
        .filter(|&y| image.get_pixel(x, y)[3] > 0)
        .count() as u32
}

#[test]
fn spacing_paints_separate_dabs() {
    let brush = hard(
        8.0,
        Dynamics {
            spacing: 2.0,
            ..Dynamics::default()
        },
    );
    let mut document = blank(100, 40);
    paint_path(
        &mut document,
        &line(Point::new(10.0, 20.0), Point::new(90.0, 20.0), &brush, 7),
        PaintMode::Paint,
    );
    let image = pixels(&document);
    // Dabs every 16 px from the start: 10, 26, 42, 58, 74, 90.
    for x in [10, 26, 42, 58, 74, 89] {
        assert_eq!(image.get_pixel(x, 20)[3], 255, "dab at {x}");
    }
    for x in [18, 34, 50, 66, 82] {
        assert_eq!(image.get_pixel(x, 20)[3], 0, "gap at {x}");
    }
    // Without spacing the same path is one continuous line.
    let mut plain = blank(100, 40);
    paint_path(
        &mut plain,
        &line(
            Point::new(10.0, 20.0),
            Point::new(90.0, 20.0),
            &hard(8.0, Dynamics::default()),
            7,
        ),
        PaintMode::Paint,
    );
    let plain = pixels(&plain);
    assert!((10..90).all(|x| plain.get_pixel(x, 20)[3] == 255));
}

#[test]
fn taper_narrows_and_fades_the_ends() {
    let from = Point::new(10.0, 30.0);
    let to = Point::new(190.0, 30.0);
    let taper = |size: bool, opacity: bool| {
        let brush = hard(
            20.0,
            Dynamics {
                taper_in: 60.0,
                taper_out: 60.0,
                taper_size: size,
                taper_opacity: opacity,
                ..Dynamics::default()
            },
        );
        let mut document = blank(200, 60);
        paint_path(&mut document, &line(from, to, &brush, 3), PaintMode::Paint);
        pixels(&document)
    };
    let sized = taper(true, false);
    let (start, middle, end) = (
        width_at(&sized, 25),
        width_at(&sized, 100),
        width_at(&sized, 175),
    );
    assert_eq!(middle, 20);
    assert!(start < 8 && start > 0, "start {start}");
    assert!(end < 8 && end > 0, "end {end}");
    assert!(width_at(&sized, 40) > start && width_at(&sized, 40) < middle);
    // The ends are full strength; only the size changes.
    assert_eq!(sized.get_pixel(25, 30)[3], 255);

    let faded = taper(false, true);
    assert_eq!(width_at(&faded, 25), 20);
    let alpha = |x| faded.get_pixel(x, 30)[3];
    // A pixel takes the strongest dab over it, up to a radius further in.
    assert_eq!(alpha(100), 255);
    assert!(alpha(12) < 90 && alpha(12) > 0, "{}", alpha(12));
    assert!(alpha(188) < 90 && alpha(188) > 0, "{}", alpha(188));
    assert!(alpha(12) < alpha(30) && alpha(30) < alpha(50) && alpha(50) < alpha(100));
}

#[test]
fn a_single_dab_is_not_tapered() {
    let brush = hard(
        10.0,
        Dynamics {
            taper_in: 50.0,
            taper_out: 50.0,
            ..Dynamics::default()
        },
    );
    let mut tapered = blank(30, 30);
    paint_path(
        &mut tapered,
        &[(Point::new(15.0, 15.0), brush.clone())],
        PaintMode::Paint,
    );
    let mut plain = blank(30, 30);
    paint_path(
        &mut plain,
        &[(Point::new(15.0, 15.0), hard(10.0, Dynamics::default()))],
        PaintMode::Paint,
    );
    assert_eq!(pixels(&tapered), pixels(&plain));
}

fn scattered(seed: u64) -> Dynamics {
    Dynamics {
        spacing: 0.5,
        scatter: 2.0,
        count: 3,
        size_jitter: 0.7,
        opacity_jitter: 0.6,
        hue_jitter: 0.5,
        seed,
        ..Dynamics::default()
    }
}

#[test]
fn scatter_and_jitter_repeat_for_a_seed_and_differ_between_seeds() {
    let paint = |seed| {
        let brush = hard(12.0, scattered(seed));
        let mut document = blank(160, 80);
        paint_path(
            &mut document,
            &line(Point::new(20.0, 40.0), Point::new(140.0, 40.0), &brush, 5),
            PaintMode::Paint,
        );
        pixels(&document)
    };
    let first = paint(7);
    assert_eq!(first, paint(7), "the same seed paints the same pixels");
    assert_ne!(first, paint(8), "another seed paints other pixels");
    // Scatter reaches off the path, and the dabs vary in colour and opacity.
    assert!(
        (0..160).any(|x| first.get_pixel(x, 40 + 14)[3] > 0 || first.get_pixel(x, 40 - 14)[3] > 0)
    );
    let colours: std::collections::BTreeSet<[u8; 3]> = first
        .pixels()
        .filter(|p| p[3] > 0)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    assert!(colours.len() > 3, "{colours:?}");
    assert!(first.pixels().any(|p| p[3] > 0 && p[3] < 255));
}

#[test]
fn a_live_stroke_finishes_as_the_whole_path_would_paint() {
    // Pointer samples arrive one at a time; the end taper is only known at
    // release, which then matches a replay of the samples (as from MCP).
    for dynamics in [
        Dynamics {
            taper_in: 30.0,
            taper_out: 40.0,
            taper_opacity: true,
            ..Dynamics::default()
        },
        Dynamics {
            taper_out: 40.0,
            ..scattered(3)
        },
        scattered(3),
    ] {
        for mode in [PaintMode::Paint, PaintMode::Erase, PaintMode::Pencil] {
            let mut original = blank(220, 90);
            if mode == PaintMode::Erase {
                original.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_pixel(
                    220,
                    90,
                    Rgba([20, 90, 160, 255]),
                )));
            }
            let mut samples = Vec::new();
            for i in 0..=24 {
                let x = 15.0 + i as f32 * 8.0;
                let y = 45.0 + (i as f32 * 0.5).sin() * 20.0;
                let pressure = 0.4 + 0.6 * (i as f32 / 24.0);
                let mut brush = hard(14.0 * pressure, dynamics);
                brush.opacity = 0.8;
                samples.push((Point::new(x, y), brush));
            }
            let mut live = original.clone();
            let mut stroke = Stroke::default();
            let (mut last, mut last_brush) = samples[0].clone();
            // As the canvas does: a dab where the pointer goes down, then each move.
            for (point, brush) in std::iter::once(samples[0].clone()).chain(samples.clone()) {
                stroke
                    .segment(&mut live, last, point, &last_brush, &brush, options(mode))
                    .unwrap();
                (last, last_brush) = (point, brush);
            }
            stroke.finish(&mut live, options(mode)).unwrap();

            let mut replay = original.clone();
            let mut with_press = vec![samples[0].clone()];
            with_press.extend(samples.clone());
            paint_path(&mut replay, &with_press, mode);
            let (a, b) = (pixels(&live), pixels(&replay));
            let diff: Vec<_> = a
                .enumerate_pixels()
                .filter(|(x, y, p)| b.get_pixel(*x, *y) != *p)
                .map(|(x, y, p)| (x, y, p.0, b.get_pixel(x, y).0))
                .take(5)
                .collect();
            assert!(diff.is_empty(), "{mode:?} {dynamics:?} {diff:?}");
            assert_ne!(pixels(&live), pixels(&original));
        }
    }
}

#[test]
fn plain_brushes_paint_as_segments_always_did() {
    let brush = Brush {
        diameter: 17.0,
        hardness: 0.7,
        opacity: 0.6,
        color: [10, 200, 100, 255],
        ..Brush::default()
    };
    let points = [
        Point::new(10.0, 10.0),
        Point::new(60.0, 40.0),
        Point::new(90.0, 15.0),
    ];
    let mut expected = blank(100, 50);
    let mut segments = Stroke::default();
    for pair in points.windows(2) {
        segments
            .segment(
                &mut expected,
                pair[0],
                pair[1],
                &brush,
                &brush,
                options(PaintMode::Paint),
            )
            .unwrap();
    }
    let mut actual = blank(100, 50);
    let samples: Vec<_> = points.iter().map(|p| (*p, brush.clone())).collect();
    paint_path(&mut actual, &samples, PaintMode::Paint);
    assert_eq!(pixels(&actual), pixels(&expected));
}

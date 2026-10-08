use super::*;

fn options(mode: PaintMode, mask_target: bool) -> StrokeOptions<'static> {
    StrokeOptions {
        mode,
        mask_target,
        source: None,
        clone_offset: Point::default(),
    }
}

fn compare(a: &Document, b: &Document, mask: bool) {
    let pixels = |document: &Document| {
        let layer = document.active().unwrap();
        if mask {
            layer.mask.as_ref().unwrap().pixels.as_raw().clone()
        } else {
            render::render(document).into_raw()
        }
    };
    let a = pixels(a);
    let b = pixels(b);
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 1));
}

#[test]
fn low_opacity_fast_strokes_match_dense_strokes_without_dark_joins() {
    let brush = Brush {
        diameter: 53.0,
        hardness: 0.83,
        opacity: 0.3,
        color: [31, 57, 93, 173],
        ..Default::default()
    };
    for mask in [false, true] {
        for mode in [PaintMode::Paint, PaintMode::Erase] {
            for background in [[0; 4], [231, 195, 141, 255], [120, 90, 60, 127]] {
                let mut original = Document::new(240, 120).unwrap();
                original.active_mut().unwrap().pixels =
                    Some(Arc::new(RgbaImage::from_pixel(240, 120, Rgba(background))));
                original.selection = Some(Arc::new(GrayImage::from_fn(240, 120, |x, _| {
                    Luma([if x < 40 {
                        0
                    } else if x < 140 {
                        127
                    } else {
                        255
                    }])
                })));
                let start = Point::new(30.0, 60.0);
                let end = Point::new(210.0, 60.0);
                let mut expected = original.clone();
                stroke(&mut expected, start, end, &brush, options(mode, mask)).unwrap();
                for samples in [vec![30, 48, 100, 177, 210], (30..=210).collect()] {
                    let mut actual = original.clone();
                    let mut stroke = Stroke::default();
                    let mut previous = start;
                    // Include a stationary release and a return pass in the same gesture.
                    for x in samples.into_iter().chain([210, 210, 30]) {
                        let point = Point::new(x as f32, 60.0);
                        stroke
                            .segment(
                                &mut actual,
                                previous,
                                point,
                                &brush,
                                &brush,
                                options(mode, mask),
                            )
                            .unwrap();
                        previous = point;
                    }
                    compare(&actual, &expected, mask);
                }
            }
        }
    }
}

#[test]
fn stroke_pressure_uses_peak_coverage_and_new_strokes_build_up() {
    let mut document = Document::new(80, 80).unwrap();
    let original = document.clone();
    let point = Point::new(40.0, 40.0);
    let mut brush = Brush {
        diameter: 30.0,
        hardness: 1.0,
        ..Default::default()
    };
    let mut stroke = Stroke::default();
    for (pressure, expected) in [(0.1, 26), (0.3, 77), (0.6, 153), (0.2, 153)] {
        let previous = brush.clone();
        brush.opacity = pressure;
        stroke
            .segment(
                &mut document,
                point,
                point,
                &previous,
                &brush,
                options(PaintMode::Paint, false),
            )
            .unwrap();
        assert_eq!(
            document
                .active()
                .unwrap()
                .pixels
                .as_ref()
                .unwrap()
                .get_pixel(40, 40)[3],
            expected
        );
    }
    assert!(render::render(&original).pixels().all(|p| p[3] == 0));
    let mut next = Stroke::default();
    next.segment(
        &mut document,
        point,
        point,
        &brush,
        &brush,
        options(PaintMode::Paint, false),
    )
    .unwrap();
    assert_eq!(
        document
            .active()
            .unwrap()
            .pixels
            .as_ref()
            .unwrap()
            .get_pixel(40, 40)[3],
        173
    );
}

#[test]
fn stroke_coverage_follows_expanding_transformed_layers() {
    let brush = Brush {
        diameter: 25.0,
        opacity: 0.3,
        ..Default::default()
    };
    let mut original = Document::new(240, 160).unwrap();
    let mut layer = Layer::image(
        "Small",
        RgbaImage::from_pixel(32, 32, Rgba([235, 170, 80, 255])),
    );
    layer.transform.x = 100.0;
    layer.transform.y = 60.0;
    layer.transform.rotation = 25.0;
    layer.transform.flip_x = true;
    original.insert(layer);
    let start = Point::new(190.0, 100.0);
    let end = Point::new(40.0, 40.0);
    let mut expected = original.clone();
    stroke(
        &mut expected,
        start,
        end,
        &brush,
        options(PaintMode::Paint, false),
    )
    .unwrap();
    let mut actual = original.clone();
    let mut stroke = Stroke::default();
    let mut previous = start;
    for i in 0..=30 {
        let point = Point::new(start.x - i as f32 * 5.0, start.y - i as f32 * 2.0);
        stroke
            .segment(
                &mut actual,
                previous,
                point,
                &brush,
                &brush,
                options(PaintMode::Paint, false),
            )
            .unwrap();
        previous = point;
    }
    compare(&actual, &expected, false);
    assert_eq!(
        original
            .active()
            .unwrap()
            .pixels
            .as_ref()
            .unwrap()
            .dimensions(),
        (32, 32)
    );
}

/// Brush, Eraser, mask, dynamics, symmetry and pencil strokes at full flow,
/// side by side in one image.
fn full_flow_strokes() -> RgbaImage {
    const W: u32 = 96;
    const H: u32 = 64;
    let background = |document: &mut Document| {
        document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_fn(W, H, |x, y| {
            Rgba([
                (x * 2) as u8,
                (y * 3) as u8,
                140,
                if x < 12 { 0 } else { 255 },
            ])
        })));
        document.selection = Some(Arc::new(GrayImage::from_fn(W, H, |x, y| {
            Luma([if y > 54 { 90 } else { 255 - x as u8 }])
        })));
    };
    let zigzag = |brush: &Brush, pressure: bool| -> Vec<(Point, Brush)> {
        (0..=12)
            .map(|i| {
                let mut brush = brush.clone();
                if pressure {
                    let p = 0.2 + 0.8 * ((i * 7) % 12) as f32 / 12.0;
                    brush.diameter *= p;
                    brush.opacity *= p;
                }
                let point = Point::new(8.0 + i as f32 * 6.5, if i % 2 == 0 { 18.0 } else { 46.0 });
                (point, brush)
            })
            .collect()
    };
    let soft = Brush {
        diameter: 15.0,
        hardness: 0.55,
        opacity: 0.7,
        color: [220, 40, 90, 200],
        ..Brush::default()
    };
    let dynamics = |dynamics: Dynamics| Brush {
        dynamics,
        ..soft.clone()
    };
    let scenes: Vec<(Brush, PaintMode, bool, bool, bool)> = vec![
        // brush, mode, mask, pressure, live segments (else a whole path)
        (soft.clone(), PaintMode::Paint, false, false, true),
        (soft.clone(), PaintMode::Paint, false, true, true),
        (soft.clone(), PaintMode::Erase, false, true, true),
        (soft.clone(), PaintMode::Paint, true, true, true),
        (soft.clone(), PaintMode::Erase, true, false, true),
        (
            dynamics(Dynamics {
                spacing: 0.4,
                ..Dynamics::default()
            }),
            PaintMode::Paint,
            false,
            true,
            true,
        ),
        (
            dynamics(Dynamics {
                taper_in: 30.0,
                taper_out: 40.0,
                taper_opacity: true,
                ..Dynamics::default()
            }),
            PaintMode::Paint,
            false,
            false,
            false,
        ),
        (
            dynamics(Dynamics {
                spacing: 0.3,
                count: 2,
                size_jitter: 0.5,
                opacity_jitter: 0.6,
                hue_jitter: 0.3,
                seed: 9,
                ..Dynamics::default()
            }),
            PaintMode::Erase,
            false,
            false,
            false,
        ),
        (
            Brush {
                symmetry: Symmetry {
                    mode: SymmetryMode::Vertical,
                    ..Symmetry::default()
                },
                ..soft.clone()
            },
            PaintMode::Paint,
            false,
            true,
            true,
        ),
        (
            Brush {
                diameter: 5.0,
                opacity: 0.8,
                ..soft.clone()
            },
            PaintMode::Pencil,
            false,
            false,
            true,
        ),
    ];
    let mut strip = RgbaImage::new(W * scenes.len() as u32, H * 2);
    for (index, (brush, mode, mask, pressure, live)) in scenes.into_iter().enumerate() {
        let mut document = Document::new(W, H).unwrap();
        background(&mut document);
        if mask {
            prepare_mask(document.active_mut().unwrap()).unwrap();
        }
        let samples = zigzag(&brush, pressure);
        let options = options(mode, mask);
        let mut stroke = Stroke::default();
        if live {
            for pair in samples.windows(2) {
                let options = StrokeOptions { ..options };
                stroke
                    .segment(
                        &mut document,
                        pair[0].0,
                        pair[1].0,
                        &pair[0].1,
                        &pair[1].1,
                        options,
                    )
                    .unwrap();
            }
            stroke.finish(&mut document, options).unwrap();
        } else {
            stroke.path(&mut document, &samples, options).unwrap();
        }
        let layer = document.active().unwrap();
        let x = index as u32 * W;
        image::imageops::replace(&mut strip, &**layer.pixels.as_ref().unwrap(), x.into(), 0);
        if let Some(mask) = &layer.mask {
            let mask = image::DynamicImage::ImageLuma8((*mask.pixels).clone()).to_rgba8();
            image::imageops::replace(&mut strip, &mask, x.into(), H.into());
        }
    }
    strip
}

/// Flow 100% paints exactly what strokes painted before Flow existed: the
/// reference was rendered by the brush before flow was added. Linux, where
/// it was rendered, matches exactly; elsewhere libm may move a pixel a level.
#[test]
fn full_flow_paints_the_strokes_as_before_flow() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/paint/strokes_before_flow.png");
    let actual = full_flow_strokes();
    if std::env::var_os("XUAN_UPDATE_STROKES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        actual.save(&path).unwrap();
    }
    let expected = image::open(&path).unwrap().to_rgba8();
    assert_eq!(actual.dimensions(), expected.dimensions());
    let tolerance = if cfg!(target_os = "linux") { 0 } else { 1 };
    let worst = actual
        .as_raw()
        .iter()
        .zip(expected.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(worst <= tolerance, "a pixel moved by {worst}");
}

use super::*;
use crate::document::Point;
use image::Rgb;
use std::sync::atomic::AtomicBool;

fn synthetic() -> DecodedRaw {
    DecodedRaw {
        camera: Rgb32FImage::from_fn(64, 48, |x, y| {
            Rgb([0.02 + x as f32 / 40.0, 0.02 + y as f32 / 30.0, 0.2])
        }),
        as_shot: [1.0; 3],
        camera_to_rgb: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        xyz_to_camera: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        metadata: RawMetadata {
            width: 64,
            height: 48,
            ..Default::default()
        },
    }
}

#[test]
fn exposure_recovers_unclipped_raw_values_and_is_repeatable() {
    let raw = synthetic();
    let cancel = AtomicBool::new(false);
    let mut settings = DevelopSettings {
        sharpen: 0.0,
        color_noise: 0.0,
        ..Default::default()
    };
    let before = render(&raw, &settings, &cancel).unwrap();
    settings.exposure = -2.0;
    let recovered = render(&raw, &settings, &cancel).unwrap();
    assert_eq!(before.get_pixel(63, 47)[0], 255);
    assert!(recovered.get_pixel(63, 47)[0] < 230);
    assert!(recovered.get_pixel(63, 47)[0] > recovered.get_pixel(55, 47)[0]);
    assert_eq!(recovered, render(&raw, &settings, &cancel).unwrap());
    assert!(raw.camera.get_pixel(63, 47)[0] > 1.0);
}

#[test]
fn validates_settings_and_cancellation() {
    let raw = synthetic();
    let mut s = DevelopSettings {
        exposure: f32::NAN,
        ..Default::default()
    };
    assert!(s.validate().is_err());
    s = DevelopSettings::default();
    s.crop = [0.5, 0.0, 0.4, 1.0];
    assert!(s.validate().is_err());
    assert!(render(&raw, &DevelopSettings::default(), &AtomicBool::new(true)).is_err());
    assert!(raw.preview_cancellable(32, &AtomicBool::new(true)).is_err());
    assert_eq!(
        raw.preview_cancellable(32, &AtomicBool::new(false))
            .unwrap()
            .camera,
        raw.preview(32).camera
    );
    assert!(decode(b"broken camera file").is_err());
    for extension in [
        "NEF", "nrw", "cr2", "CR3", "CrW", "RAF", "raf", "ARW", "aRw",
    ] {
        assert!(is_raw(Path::new(&format!("photo.{extension}"))));
    }
    assert!(!is_raw(Path::new("photo.tiff")));
}

#[test]
fn crop_and_local_adjustment_are_nondestructive() {
    let raw = synthetic();
    let mut s = DevelopSettings {
        sharpen: 0.0,
        color_noise: 0.0,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let before = render(&raw, &s, &cancel).unwrap();
    s.overlays.push(Overlay {
        exposure: -2.0,
        start: crate::document::Point::new(0.0, 0.0),
        end: crate::document::Point::new(0.0, 0.5),
        ..Default::default()
    });
    let after = render(&raw, &s, &cancel).unwrap();
    assert!(after.get_pixel(20, 0)[0] < before.get_pixel(20, 0)[0]);
    assert_eq!(after.get_pixel(20, 47), before.get_pixel(20, 47));
    s.crop = [0.25, 0.25, 0.75, 0.75];
    assert_eq!(render(&raw, &s, &cancel).unwrap().dimensions(), (32, 24));
}

#[test]
fn quarter_turns_rotate_the_complete_cropped_development_at_both_depths() {
    let raw = synthetic();
    let cancel = AtomicBool::new(false);
    let mut settings = DevelopSettings {
        crop: [0.13, 0.21, 0.92, 0.89],
        rotation: 7.0,
        overlays: vec![Overlay {
            exposure: -1.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let before = render(&raw, &settings, &cancel).unwrap();
    let before16 = render_16(&raw, &settings, &cancel).unwrap();
    for turns in 1..4 {
        settings.quarter_turns = turns;
        let expected = match turns {
            1 => image::imageops::rotate90(&before),
            2 => image::imageops::rotate180(&before),
            _ => image::imageops::rotate270(&before),
        };
        let expected16 = match turns {
            1 => image::imageops::rotate90(&before16),
            2 => image::imageops::rotate180(&before16),
            _ => image::imageops::rotate270(&before16),
        };
        assert_eq!(render(&raw, &settings, &cancel).unwrap(), expected);
        assert_eq!(render_16(&raw, &settings, &cancel).unwrap(), expected16);
    }
}

#[test]
fn rotated_crop_and_picker_coordinates_match_the_display() {
    let mut settings = DevelopSettings {
        crop: [0.125, 0.25, 0.875, 1.0],
        ..Default::default()
    };
    for turns in 0..4 {
        settings.quarter_turns = turns;
        let point = Point::new(0.375, 0.625);
        let display = settings.display_point(point);
        assert!(settings.image_point(display).distance(point) < 1e-6);
        let display_crop = settings.display_crop();
        settings.set_display_crop(display_crop);
        assert_eq!(settings.crop, [0.125, 0.25, 0.875, 1.0]);
    }
    settings.quarter_turns = 1;
    assert_eq!(
        settings.image_point(Point::new(0.0, 0.0)),
        Point::new(0.125, 1.0)
    );
    assert_eq!(settings.output_size([64, 48]), [36, 48]);
    settings.set_display_crop([0.25, 0.125, 1.0, 0.875]);
    assert_eq!(settings.crop, [0.125, 0.0, 0.875, 0.75]);
    let legacy: DevelopSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.quarter_turns, 0);
    settings.quarter_turns = 4;
    assert!(settings.validate().is_err());
}

#[test]
#[ignore = "Set XUAN_TEST_RAW to a local camera file"]
fn sample_raw_develop_roundtrip() {
    let path = std::env::var_os("XUAN_TEST_RAW")
        .or_else(|| std::env::var_os("XUAN_TEST_NEF"))
        .expect("Set XUAN_TEST_RAW");
    let (mut asset, raw) = open(Path::new(&path)).unwrap();
    if let Ok(turns) = std::env::var("XUAN_TEST_RAW_QUARTER_TURNS") {
        asset.settings.quarter_turns = turns.parse().unwrap();
        asset.settings.validate().unwrap();
    }
    assert!(is_raw(Path::new(&path)));
    assert!(raw.camera.as_raw().iter().all(|v| v.is_finite()));
    println!(
        "Camera metadata: {:?}; white balance: {:?}",
        raw.metadata, raw.as_shot
    );
    let proxy = raw.preview(1400);
    let pixels = render(&proxy, &asset.settings, &AtomicBool::new(false)).unwrap();
    if let Some(output) = std::env::var_os("XUAN_TEST_RAW_PREVIEW") {
        pixels.save(output).unwrap();
    }
    assert!(pixels.pixels().any(|p| p[0] != p[1]));
    let full = render(&raw, &asset.settings, &AtomicBool::new(false)).unwrap();
    let [width, height] = asset
        .settings
        .output_size([raw.metadata.width, raw.metadata.height]);
    assert_eq!(full.dimensions(), (width, height));
    let mut layer = crate::document::Layer::image("RAW", full);
    layer.raw = Some(asset.clone());
    let mut document = crate::document::Document::new(width, height).unwrap();
    document.select(layer.id, false);
    document.layers = vec![layer];
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("raw.xuan");
    crate::io::save(&document, &project).unwrap();
    let loaded = crate::io::load(&project).unwrap();
    let restored = loaded.layers[0].raw.as_ref().unwrap();
    assert_eq!(restored.filename, asset.filename);
    assert_eq!(restored.bytes, asset.bytes);
    assert_eq!(restored.settings, asset.settings);
    assert_eq!(loaded.layers[0].pixels, document.layers[0].pixels);
    let reopened = decode(&restored.bytes).unwrap();
    assert_eq!(
        render(
            &reopened.preview(1400),
            &restored.settings,
            &AtomicBool::new(false)
        )
        .unwrap(),
        pixels
    );
}

fn xtrans_sensor(cfa: rawler::CFA) -> rawler::RawImage {
    use rawler::{
        decoders::Camera,
        rawimage::{BlackLevel, CFAConfig, RawImageData, WhiteLevel},
    };

    let black: Vec<u32> = (0..36).map(|i| 128 + i * 16).collect();
    let mut sensor = Vec::new();
    for y in 0..24 {
        for x in 0..30 {
            let value = [0.125, 0.5, 1.25][cfa.color_at(y, x)];
            let black = black[(y % 6) * 6 + x % 6] as f32;
            sensor.push(black + value * (4096.0 - black));
        }
    }
    rawler::RawImage::new_with_data(
        Camera::default(),
        RawImageData::Float(sensor),
        30,
        24,
        1,
        [1.0; 4],
        RawPhotometricInterpretation::Cfa(CFAConfig::new(&cfa, &Default::default())),
        Some(BlackLevel::new(&black, 6, 6, 1)),
        Some(WhiteLevel::new(vec![4096])),
        false,
    )
}

#[test]
fn xtrans_preserves_colors_black_levels_and_highlights_at_every_phase_and_border() {
    let cfa = rawler::CFA::new("RBGBRGGGRGGBGGBGGRBRGRBGGGBGGRGGRGGB");
    for y in 0..6 {
        for x in 0..6 {
            let raw = xtrans_sensor(cfa.shift(x, y));
            validate_sensor(&raw).unwrap();
            let pixels = develop_camera(&raw).unwrap();
            assert_eq!(pixels.dimensions(), (30, 24));
            for pixel in pixels.pixels() {
                for (actual, expected) in pixel.0.into_iter().zip([0.125, 0.5, 1.25]) {
                    assert!((actual - expected).abs() < 1e-6, "phase {x},{y}: {pixel:?}");
                }
            }
        }
    }
}

#[test]
fn xtrans_crops_in_sensor_coordinates_and_retains_measured_samples() {
    use rawler::{
        imgop::{Dim2, Point, Rect},
        rawimage::RawImageData,
    };

    let cfa = rawler::CFA::new("RBGBRGGGRGGBGGBGGRBRGRBGGGBGGRGGRGGB");
    let mut raw = xtrans_sensor(cfa.clone());
    // A nonuniform field makes a misplaced crop or CFA phase visible.
    let RawImageData::Float(sensor) = &mut raw.data else {
        unreachable!()
    };
    for (i, value) in sensor.iter_mut().enumerate() {
        *value += i as f32;
    }
    raw.active_area = Some(Rect::new(Point::new(3, 5), Dim2::new(25, 18)));
    let active = develop_camera(&raw).unwrap();
    raw.crop_area = Some(Rect::new(Point::new(7, 8), Dim2::new(17, 12)));
    let cropped = develop_camera(&raw).unwrap();
    assert_eq!(
        cropped,
        image::imageops::crop_imm(&active, 4, 3, 17, 12).to_image()
    );
    let sensor = raw.data.as_f32();
    let black = raw.blacklevel.as_vec();
    for (x, y, pixel) in cropped.enumerate_pixels() {
        let (sx, sy) = (x as usize + 7, y as usize + 8);
        let black = black[(sy % 6) * 6 + sx % 6];
        let expected = (sensor[sy * raw.width + sx] - black) / (4096.0 - black);
        assert_eq!(pixel[cfa.color_at(sy, sx)], expected);
        assert!(pixel.0.iter().all(|v| v.is_finite()));
    }
    raw.crop_area = Some(Rect::new(Point::zero(), Dim2::new(17, 12)));
    assert!(develop_camera(&raw).is_err());
    raw.crop_area = None;
    raw.blacklevel.levels.clear();
    assert!(develop_camera(&raw).is_err());
}

#[test]
fn unsupported_raw_sensor_layouts_still_fail_validation() {
    let mut raw = xtrans_sensor(rawler::CFA::new("RGGB"));
    assert!(validate_sensor(&raw).is_ok());
    raw.cpp = 3;
    assert!(validate_sensor(&raw).is_err());
    raw.cpp = 1;
    raw.photometric = RawPhotometricInterpretation::LinearRaw;
    assert!(validate_sensor(&raw).is_err());
    raw.photometric = RawPhotometricInterpretation::Cfa(rawler::rawimage::CFAConfig::new(
        &rawler::CFA::new("RGBE"),
        &Default::default(),
    ));
    assert!(validate_sensor(&raw).is_err());
}

#[test]
fn sixteen_bit_output_preserves_more_than_eight_bit_steps() {
    let mut raw = synthetic();
    raw.camera = Rgb32FImage::from_fn(1024, 2, |x, _| Rgb([0.1 + x as f32 / 100_000.0; 3]));
    raw.metadata.width = 1024;
    raw.metadata.height = 2;
    let s = DevelopSettings {
        color_noise: 0.0,
        sharpen: 0.0,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let image = render_16(&raw, &s, &cancel).unwrap();
    let steps: std::collections::HashSet<_> = image.pixels().map(|p| p[0]).collect();
    assert!(steps.len() > 900);
    let mut bytes = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba16(image.clone())
        .write_to(&mut bytes, image::ImageFormat::Tiff)
        .unwrap();
    let restored = image::load_from_memory(bytes.get_ref()).unwrap();
    assert_eq!(restored.color(), image::ColorType::Rgba16);
    assert_eq!(restored.to_rgba16(), image);
}

#[test]
fn raw_project_assets_settings_and_history_survive_roundtrip() {
    let raw = synthetic();
    let mut asset = RawAsset {
        filename: "test.CR3".into(),
        metadata: raw.metadata.clone(),
        settings: DevelopSettings {
            exposure: -0.5,
            quarter_turns: 1,
            ..Default::default()
        },
        bytes: Arc::new(b"test fixture bytes".to_vec()),
    };
    asset.settings.overlays.push(Overlay {
        kind: OverlayKind::Brush,
        points: vec![crate::document::Point::new(0.5, 0.5)],
        ..Default::default()
    });
    let mut layer = crate::document::Layer::image(
        "RAW",
        render(&raw, &asset.settings, &AtomicBool::new(false)).unwrap(),
    );
    layer.raw = Some(asset.clone());
    layer.transform.x = 20.0;
    layer.opacity = 0.6;
    layer.mask = Some(crate::document::Mask::white());
    let mut doc = crate::document::Document::new(64, 48).unwrap();
    doc.select(layer.id, false);
    doc.layers = vec![layer];
    let mut history = crate::history::History::default();
    let before = doc.clone();
    history.begin("Develop RAW", &doc);
    asset.settings.exposure = 1.0;
    let pixels = render(&raw, &asset.settings, &AtomicBool::new(false)).unwrap();
    update_layer(&mut doc.layers[0], asset.clone(), pixels).unwrap();
    history.commit();
    assert_eq!(doc.layers[0].id, before.layers[0].id);
    assert_eq!(doc.layers[0].transform, before.layers[0].transform);
    assert_eq!(doc.layers[0].opacity, 0.6);
    assert!(doc.layers[0].mask.is_some());
    assert!(history.undo(&mut doc));
    assert_eq!(doc.layers[0].pixels, before.layers[0].pixels);
    assert_eq!(doc.layers[0].raw.as_ref().unwrap().settings.exposure, -0.5);
    assert!(history.redo(&mut doc));
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("raw.xuan");
    crate::io::save(&doc, &path).unwrap();
    let restored = crate::io::load(&path).unwrap();
    let restored_raw = restored.layers[0].raw.as_ref().unwrap();
    assert_eq!(restored_raw.filename, asset.filename);
    assert_eq!(restored_raw.bytes, asset.bytes);
    assert_eq!(restored_raw.settings, asset.settings);
    assert!(crate::paint::ensure_pixels(&mut doc.layers[0]).is_err());
    assert!(crate::paint::prepare_mask(&mut doc.layers[0]).is_ok());
    crate::operations::duplicate(&mut doc);
    assert!(Arc::ptr_eq(
        &doc.layers[0].raw.as_ref().unwrap().bytes,
        &doc.layers[1].raw.as_ref().unwrap().bytes
    ));
}

#[test]
fn corrupted_project_raw_sources_and_settings_are_rejected() {
    let raw = synthetic();
    let mut doc = crate::document::Document::new(64, 48).unwrap();
    doc.layers[0].pixels = Some(Arc::new(RgbaImage::new(64, 48)));
    doc.layers[0].raw = Some(RawAsset {
        filename: "test.NEF".into(),
        metadata: raw.metadata,
        settings: DevelopSettings::default(),
        bytes: Arc::new(vec![1, 2, 3]),
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("good.xuan");
    crate::io::save(&doc, &path).unwrap();
    let mut original = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
    let damaged = dir.path().join("missing.xuan");
    let mut out = zip::ZipWriter::new(File::create(&damaged).unwrap());
    for i in 0..original.len() {
        let file = original.by_index(i).unwrap();
        if !file.name().starts_with("raw/") {
            out.raw_copy_file(file).unwrap();
        }
    }
    out.finish().unwrap();
    assert!(
        crate::io::load(&damaged)
            .unwrap_err()
            .to_string()
            .contains("Missing project asset")
    );
    doc.layers[0].raw.as_mut().unwrap().settings.crop = [0.0; 4];
    assert!(crate::io::save(&doc, &path).is_err());
}

#[test]
fn raw_crop_preserves_the_placement_of_surviving_pixels() {
    let raw = synthetic();
    let asset = RawAsset {
        filename: "test.NEF".into(),
        metadata: raw.metadata,
        settings: DevelopSettings::default(),
        bytes: Arc::new(vec![1]),
    };
    let mut layer = crate::document::Layer::image("RAW", RgbaImage::new(64, 48));
    layer.raw = Some(asset.clone());
    layer.transform.rotation = 25.0;
    layer.transform.x = 50.0;
    let center_before = layer
        .transform
        .point(crate::document::Point::new(0.375, 0.5));
    let mut cropped = asset;
    cropped.settings.crop = [0.25, 0.0, 0.75, 1.0];
    update_layer(&mut layer, cropped, RgbaImage::new(32, 48)).unwrap();
    let center_after = layer
        .transform
        .point(crate::document::Point::new(0.25, 0.5));
    assert!(center_before.distance(center_after) < 0.001);
}

#[test]
fn raw_redevelopment_rotates_placement_and_preserves_masks_and_later_crops() {
    let raw = synthetic();
    let asset = RawAsset {
        filename: "test.RAF".into(),
        metadata: raw.metadata,
        settings: DevelopSettings::default(),
        bytes: Arc::new(vec![1]),
    };
    let mut layer = crate::document::Layer::image("RAW", RgbaImage::new(64, 48));
    layer.raw = Some(asset.clone());
    layer.transform.rotation = 25.0;
    layer.transform.flip_x = true;
    layer.transform.warp = Some([
        Point::new(0.1, 0.0),
        Point::new(1.0, 0.2),
        Point::new(0.9, 0.9),
        Point::new(0.0, 1.0),
    ]);
    layer.mask = Some(crate::document::Mask::white());
    let before = layer.transform;
    for turns in 1..4 {
        let mut rotated = layer.clone();
        let mut asset = asset.clone();
        asset.settings.quarter_turns = turns;
        let [width, height] = asset.settings.output_size([64, 48]);
        update_layer(&mut rotated, asset.clone(), RgbaImage::new(width, height)).unwrap();
        assert_eq!(rotated.transform.center(), before.center());
        assert_eq!(rotated.mask.as_ref().unwrap().placement, Some(before));
        let point = Point::new(0.3, 0.6);
        let old = before.point(point);
        let center = before.center();
        let (sin, cos) = (turns as f32 * 90.0).to_radians().sin_cos();
        let expected = Point::new(
            center.x + cos * (old.x - center.x) - sin * (old.y - center.y),
            center.y + sin * (old.x - center.x) + cos * (old.y - center.y),
        );
        assert!(
            rotated
                .transform
                .point(rotate_point(point, turns))
                .distance(expected)
                < 1e-4
        );
        asset.settings.crop = [0.25, 0.0, 0.75, 1.0];
        let [width, height] = asset.settings.output_size([64, 48]);
        update_layer(&mut rotated, asset.clone(), RgbaImage::new(width, height)).unwrap();
        assert!(
            rotated
                .transform
                .point(asset.settings.display_point(point))
                .distance(expected)
                < 1e-4
        );
    }
}

fn synthetic_negative() -> (DecodedRaw, NegativeSettings) {
    let negative = NegativeSettings {
        enabled: true,
        film_base: [0.8, 0.42, 0.18],
        density_range: [1.7, 2.1, 2.4],
        ..Default::default()
    };
    let mut raw = synthetic();
    raw.camera = Rgb32FImage::from_fn(64, 48, |x, _| {
        Rgb(std::array::from_fn(|c| {
            negative.film_base[c] * 10.0_f32.powf(-negative.density_range[c] * x as f32 / 63.0)
        }))
    });
    (raw, negative)
}

#[test]
fn negative_removes_orange_base_and_produces_neutral_positive_steps() {
    let (raw, negative) = synthetic_negative();
    let mut s = DevelopSettings {
        negative,
        color_noise: 0.0,
        sharpen: 0.0,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let original = raw.camera.clone();
    let positive = render(&raw, &s, &cancel).unwrap();
    assert_eq!(positive.get_pixel(0, 20).0, [0, 0, 0, 255]);
    assert_eq!(positive.get_pixel(63, 20).0, [255; 4]);
    for x in 1..64 {
        let p = positive.get_pixel(x, 20);
        assert!(p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1);
        assert!(p[0] >= positive.get_pixel(x - 1, 20)[0]);
    }
    let high = render_16(&raw, &s, &cancel).unwrap();
    for x in 1..64 {
        assert!(high.get_pixel(x, 20)[0] > high.get_pixel(x - 1, 20)[0]);
    }
    // Camera WB is irrelevant to calibrated film density; exposure is positive.
    s.white_balance = WhiteBalance::Temperature;
    s.temperature = 2500.0;
    s.tint = 150.0;
    assert_eq!(positive, render(&raw, &s, &cancel).unwrap());
    s.exposure = 1.0;
    assert!(
        render(&raw, &s, &cancel).unwrap().get_pixel(30, 20)[0] > positive.get_pixel(30, 20)[0]
    );
    s.negative.enabled = false;
    let disabled = render(&raw, &s, &cancel).unwrap();
    s.negative = NegativeSettings::default();
    assert_eq!(disabled, render(&raw, &s, &cancel).unwrap());
    assert_eq!(raw.camera, original);
}

#[test]
fn negative_analysis_respects_crop_and_film_base_sampling() {
    let (mut raw, expected) = synthetic_negative();
    let s = DevelopSettings::default();
    let analyzed = analyze_negative(&raw, &s);
    for c in 0..3 {
        assert!((analyzed.film_base[c] - expected.film_base[c]).abs() < 0.0001);
        assert!((analyzed.density_range[c] - expected.density_range[c]).abs() < 0.0001);
    }
    // A bright holder outside the crop must not contaminate the endpoints.
    raw.camera = Rgb32FImage::from_fn(128, 48, |x, _| {
        if x >= 64 {
            Rgb([4.0; 3])
        } else {
            Rgb(std::array::from_fn(|c| {
                expected.film_base[c] * 10.0_f32.powf(-expected.density_range[c] * x as f32 / 63.0)
            }))
        }
    });
    let cropped = DevelopSettings {
        crop: [0.0, 0.0, 0.5, 1.0],
        ..Default::default()
    };
    assert_eq!(analyze_negative(&raw, &cropped), analyzed);
    assert!(analyze_negative(&raw, &s).film_base[0] > analyzed.film_base[0]);
    raw.camera = Rgb32FImage::from_pixel(20, 20, Rgb(expected.film_base));
    let base = sample_film_base(&raw, crate::document::Point::new(0.5, 0.5)).unwrap();
    for (value, expected) in base.iter().zip(expected.film_base) {
        assert!((value - expected).abs() < 0.00001);
    }
    raw.camera.fill(0.0);
    assert!(sample_film_base(&raw, crate::document::Point::new(0.5, 0.5)).is_none());
    analyze_negative(&raw, &s).validate().unwrap();
}

#[test]
fn eyedroppers_share_patch_sampling_at_edges_and_through_geometry() {
    let mut raw = synthetic();
    let mean = |xs: std::ops::RangeInclusive<u32>, ys: std::ops::RangeInclusive<u32>| {
        let mut sum = [0.0_f32; 3];
        let mut count = 0.0;
        for y in ys {
            for x in xs.clone() {
                for (total, value) in sum.iter_mut().zip(raw.camera.get_pixel(x, y).0) {
                    *total += value;
                }
                count += 1.0;
            }
        }
        sum.map(|v| v / count)
    };
    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5);
    let wb = |m: [f32; 3]| m.map(|v| m[1] / v);
    // Interior and corner patches average only real pixels; edges are not repeated.
    let interior = mean(29..=35, 21..=27);
    let corner = mean(0..=3, 0..=3);
    let far_corner = mean(60..=63, 44..=47);
    assert!(close(
        sample_film_base(&raw, Point::new(0.5, 0.5)).unwrap(),
        interior
    ));
    assert!(close(
        sample_film_base(&raw, Point::new(0.0, 0.0)).unwrap(),
        corner
    ));
    assert!(close(
        sample_film_base(&raw, Point::new(1.0, 1.0)).unwrap(),
        far_corner
    ));
    assert!(close(
        sample_white_balance(&raw, Point::new(0.0, 0.0)).unwrap(),
        wb(corner)
    ));
    // Points the display renders as transparent cannot be sampled by either picker.
    for point in [Point::new(-0.01, 0.5), Point::new(0.5, 1.01)] {
        assert!(sample_film_base(&raw, point).is_none());
        assert!(sample_white_balance(&raw, point).is_none());
    }
    // Picker points follow the same inverse geometry as rendering.
    let rotated = DevelopSettings {
        rotation: 180.0,
        ..Default::default()
    };
    let source = source_point(Point::new(0.25, 0.25), &rotated, 64.0 / 48.0);
    assert!(source.distance(Point::new(0.75, 0.75)) < 1e-5);
    assert!(close(
        sample_white_balance(&raw, source).unwrap(),
        wb(mean(45..=51, 33..=39))
    ));
    let tilted = DevelopSettings {
        rotation: 20.0,
        ..Default::default()
    };
    let outside = source_point(Point::new(0.0, 0.0), &tilted, 64.0 / 48.0);
    assert!(sample_white_balance(&raw, outside).is_none());
    // Non-finite pixels never poison either average.
    raw.camera
        .put_pixel(32, 24, Rgb([f32::NAN, f32::INFINITY, 0.2]));
    let wb_nan = sample_white_balance(&raw, Point::new(0.5, 0.5)).unwrap();
    assert!(wb_nan.iter().all(|v| v.is_finite()));
    assert!(
        sample_film_base(&raw, Point::new(0.5, 0.5))
            .unwrap()
            .iter()
            .all(|v| v.is_finite())
    );
    // Dark pixels are real data for white balance but meaningless film transmission.
    raw.camera = Rgb32FImage::from_fn(64, 48, |x, _| {
        if x < 32 {
            Rgb([0.0; 3])
        } else {
            Rgb([0.4, 0.2, 0.1])
        }
    });
    let edge = Point::new(32.0 / 64.0, 0.5);
    assert!(close(
        sample_film_base(&raw, edge).unwrap(),
        [0.4, 0.2, 0.1]
    ));
    assert!(close(
        sample_white_balance(&raw, edge).unwrap(),
        [0.5, 1.0, 2.0]
    ));
    // A patch without green signal cannot define neutral multipliers.
    raw.camera.fill(0.0);
    assert!(sample_white_balance(&raw, Point::new(0.5, 0.5)).is_none());
}

#[test]
fn negative_settings_are_backward_compatible_and_reject_invalid_values() {
    let old: DevelopSettings = serde_json::from_str(r#"{"exposure": 1.0}"#).unwrap();
    assert!(!old.negative.enabled);
    let (_, negative) = synthetic_negative();
    let mut s = DevelopSettings {
        negative,
        ..Default::default()
    };
    assert_eq!(
        serde_json::from_str::<DevelopSettings>(&serde_json::to_string(&s).unwrap()).unwrap(),
        s
    );
    for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        s.negative.film_base[0] = value;
        assert!(s.validate().is_err());
    }
    s.negative = NegativeSettings::default();
    s.negative.density_range[1] = 0.0;
    assert!(s.validate().is_err());
    s.negative = NegativeSettings::default();
    s.negative.gamma = f32::INFINITY;
    assert!(s.validate().is_err());
}

#[test]
#[ignore = "Set XUAN_TEST_RAW to a camera scan of a film negative"]
fn sample_negative_raw_develop_roundtrip() {
    let path = std::env::var_os("XUAN_TEST_RAW").expect("Set XUAN_TEST_RAW");
    let (mut asset, raw) = open(Path::new(&path)).unwrap();
    let cancel = AtomicBool::new(false);
    let proxy = raw.preview(1200);
    if let Ok(crop) = std::env::var("XUAN_TEST_RAW_CROP") {
        asset.settings.crop = crop
            .split(',')
            .map(|v| v.parse::<f32>().unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        asset.settings.validate().unwrap();
    }
    asset.settings.negative = analyze_negative(&proxy, &asset.settings);
    println!(
        "{}: {:?}, {:?}",
        asset.filename, raw.metadata, asset.settings.negative
    );
    let pixels = render(&proxy, &asset.settings, &cancel).unwrap();
    if let Some(output) = std::env::var_os("XUAN_TEST_RAW_PREVIEW") {
        pixels.save(&output).unwrap();
        let original = Path::new(&output).with_extension("original.png");
        render(&proxy, &DevelopSettings::default(), &cancel)
            .unwrap()
            .save(original)
            .unwrap();
    }
    assert!(pixels.pixels().any(|p| (25..230).contains(&p[0])));
    let full = render(&raw, &asset.settings, &cancel).unwrap();
    let [left, top, right, bottom] = asset
        .settings
        .crop_pixels([raw.camera.width(), raw.camera.height()]);
    assert_eq!(full.dimensions(), (right - left, bottom - top));
    let high = render_16(&proxy, &asset.settings, &cancel).unwrap();
    for (eight, sixteen) in pixels.as_raw().iter().zip(high.as_raw()) {
        assert!(eight.abs_diff((f32::from(*sixteen) / 257.0).round() as u8) <= 1);
    }
    let mut layer = crate::document::Layer::image("Negative", full);
    layer.raw = Some(asset.clone());
    let mut document = crate::document::Document::new(right - left, bottom - top).unwrap();
    document.select(layer.id, false);
    document.layers = vec![layer];
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("negative.xuan");
    crate::io::save(&document, &project).unwrap();
    let restored = crate::io::load(&project).unwrap();
    let saved = restored.layers[0].raw.as_ref().unwrap();
    assert_eq!(saved.settings, asset.settings);
    assert_eq!(saved.bytes, asset.bytes);
    assert_eq!(render(&proxy, &saved.settings, &cancel).unwrap(), pixels);
}

#[test]
fn output_size_from_precomputed_crop_matches_output_size() {
    let crops = [
        [0.0, 0.0, 1.0, 1.0],
        [0.1, 0.2, 0.9, 0.7],
        [0.333, 0.171, 0.777, 0.999],
        [0.25, 0.25, 0.5, 0.5],
        [0.0, 0.5, 0.01, 1.0],
    ];
    for crop in crops {
        for quarter_turns in 0..4 {
            for size in [[64, 48], [89, 67], [1, 1], [6000, 4000]] {
                let s = DevelopSettings {
                    crop,
                    quarter_turns,
                    ..Default::default()
                };
                // Pre-change implementation, recomputing the crop bounds.
                let [l, t, r, b] = s.crop_pixels(size);
                let old = if quarter_turns % 2 == 0 {
                    [r - l, b - t]
                } else {
                    [b - t, r - l]
                };
                assert_eq!(s.output_size(size), old);
                assert_eq!(s.output_size_for_crop(s.crop_pixels(size)), old);
            }
        }
    }
}

/// Verbatim copy of the pre-hoisting `source_point`, kept as the reference.
fn reference_source_point(point: Point, s: &DevelopSettings, aspect: f32) -> Point {
    let mut x = (point.x - 0.5) * 2.0;
    let mut y = (point.y - 0.5) * 2.0 / aspect;
    let (sin, cos) = s.rotation.to_radians().sin_cos();
    (x, y) = (cos * x + sin * y, -sin * x + cos * y);
    y *= aspect;
    let perspective = (1.0 + s.perspective[0] * x * 0.004 + s.perspective[1] * y * 0.004).max(0.2);
    x /= perspective;
    y /= perspective;
    let r2 = (x * x + y * y) * 0.5;
    let scale = 1.0 + s.distortion * 0.003 * r2;
    Point::new(0.5 + x * scale * 0.5, 0.5 + y * scale * 0.5)
}

#[test]
fn hoisted_source_map_is_bit_identical_to_per_point_source_point() {
    let rotations = [
        0.0, 1e-6, -1e-6, 0.37, -12.5, 45.0, 90.0, 133.3, -179.9, 180.0,
    ];
    let aspects = [1.0, 64.0 / 48.0, 3.0 / 2.0, 0.5, 2.39];
    let warps = [
        ([0.0, 0.0], 0.0),
        ([40.0, -25.0], 30.0),
        ([-100.0, 100.0], -100.0),
    ];
    let mut checked = 0;
    for rotation in rotations {
        for aspect in aspects {
            for (perspective, distortion) in warps {
                let s = DevelopSettings {
                    rotation,
                    perspective,
                    distortion,
                    ..Default::default()
                };
                let map = process::SourceMap::new(&s, aspect);
                for i in 0..=10 {
                    for j in 0..=10 {
                        let p = Point::new(-0.1 + i as f32 * 0.12, -0.1 + j as f32 * 0.12);
                        let want = reference_source_point(p, &s, aspect);
                        for got in [map.apply(p), source_point(p, &s, aspect)] {
                            assert_eq!(got.x.to_bits(), want.x.to_bits());
                            assert_eq!(got.y.to_bits(), want.y.to_bits());
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, 10 * 5 * 3 * 121);
}

#[test]
fn as_shot_white_balance_uses_camera_coefficients_relative_to_green() {
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let gains = as_shot_white_balance([4.0, 2.0, 3.0, f32::NAN], &identity).unwrap();
    assert_eq!(gains, [2.0, 1.0, 1.5]);
}

#[test]
fn as_shot_white_balance_falls_back_to_daylight_without_camera_coefficients() {
    // A camera whose response to D65 white is (0.5, 1, 0.25) needs gains (2, 1, 4).
    let xyz_to_rgb = pseudo_inverse(SRGB_TO_XYZ_D65);
    let scale = [0.5, 1.0, 0.25];
    let xyz_to_camera: [[f32; 3]; 3] = std::array::from_fn(|i| xyz_to_rgb[i].map(|v| v * scale[i]));
    // CHDK CRW files report NaN; other files may report zeros or negatives.
    for coeffs in [
        [f32::NAN; 4],
        [0.0; 4],
        [2.0, 0.0, 1.0, 0.0],
        [-1.0, 1.0, 1.0, 1.0],
        [f32::INFINITY, 1.0, 1.0, 1.0],
    ] {
        let gains = as_shot_white_balance(coeffs, &xyz_to_camera).unwrap();
        for (gain, expected) in gains.iter().zip([2.0, 1.0, 4.0]) {
            assert!((gain - expected).abs() < 1e-3, "{coeffs:?}: {gains:?}");
        }
    }
}

#[test]
fn as_shot_white_balance_rejects_an_unusable_daylight_estimate() {
    // No coefficients and a matrix that gives no positive response to white.
    for matrix in [
        [[0.0; 3]; 3],
        [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]],
    ] {
        let error = as_shot_white_balance([f32::NAN; 4], &matrix).unwrap_err();
        assert_eq!(error.to_string(), "Invalid camera white balance");
    }
}

#[test]
fn camera_label_does_not_repeat_the_make() {
    assert_eq!(
        camera_label("NIKON CORPORATION", "NIKON D1H").as_deref(),
        Some("NIKON D1H")
    );
    assert_eq!(
        camera_label("Canon", "EOS 40D").as_deref(),
        Some("Canon EOS 40D")
    );
    assert_eq!(
        camera_label(" Canon ", "Canon EOS 50D").as_deref(),
        Some("Canon EOS 50D")
    );
    assert_eq!(camera_label("SONY", "").as_deref(), Some("SONY"));
    assert_eq!(camera_label("", "ILCE-7S").as_deref(), Some("ILCE-7S"));
    assert_eq!(camera_label(" ", ""), None);
}

#[test]
fn unknown_camera_error_names_the_camera() {
    let source = RawSource::new_from_slice(&[0u8; 16]);
    let error = decoder_error(
        &source,
        rawler::RawlerError::Unsupported {
            what: "Unknown camera".into(),
            make: "NIKON CORPORATION".into(),
            model: "NIKON D1H".into(),
            mode: "12bit".into(),
        },
    );
    assert_eq!(
        error.to_string(),
        "RAW files from the NIKON D1H are not supported yet"
    );
    // The decoder's own detail stays available in the error chain.
    assert!(format!("{error:#}").contains("12bit"));
    // Without a recognisable camera the generic message remains.
    let error = decoder_error(&source, rawler::RawlerError::DecoderFailed("bad".into()));
    assert_eq!(error.to_string(), "Unsupported or damaged RAW file");
}

// --- Real camera files -----------------------------------------------------
//
// These decode genuine files fetched by scripts/fetch-raw-fixtures.sh from the
// pinned list in testdata/raw/fixtures.txt. A missing file skips its test with a
// message, unless XUAN_REQUIRE_RAW_FIXTURES is set (as in CI), which fails it.

struct Fixture {
    id: &'static str,
    file: &'static str,
    bytes: u64,
    camera: &'static str,
    width: u32,
    height: u32,
    reject: bool,
}

fn fixtures() -> Vec<Fixture> {
    include_str!("../../testdata/raw/fixtures.txt")
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let f: Vec<&str> = line.split('|').map(str::trim).collect();
            assert_eq!(f.len(), 10, "malformed fixture line: {line}");
            Fixture {
                id: f[0],
                file: f[1],
                bytes: f[3].parse().unwrap(),
                camera: f[5],
                width: f[6].parse().unwrap(),
                height: f[7].parse().unwrap(),
                reject: match f[8] {
                    "ok" => false,
                    "reject" => true,
                    other => panic!("unknown expectation {other:?} for {}", f[0]),
                },
            }
        })
        .collect()
}

/// The fixture's bytes, or `None` (after printing why) when it is not fetched.
fn fixture_bytes(id: &str) -> Option<(Fixture, Vec<u8>)> {
    let fixture = fixtures()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("no fixture {id} in testdata/raw/fixtures.txt"));
    let dir = std::env::var_os("XUAN_RAW_FIXTURE_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/raw/cache"));
    let path = dir.join(fixture.file);
    match std::fs::read(&path) {
        Ok(bytes) => {
            assert_eq!(
                bytes.len() as u64,
                fixture.bytes,
                "{} has the wrong size; rerun scripts/fetch-raw-fixtures.sh",
                path.display()
            );
            Some((fixture, bytes))
        }
        Err(error) => {
            let message = format!(
                "RAW fixture {id} missing ({}: {error}); run scripts/fetch-raw-fixtures.sh",
                path.display()
            );
            if std::env::var_os("XUAN_REQUIRE_RAW_FIXTURES").is_some_and(|v| !v.is_empty()) {
                panic!("{message}");
            }
            eprintln!("skipping: {message}");
            None
        }
    }
}

fn check_real_raw(id: &str) {
    let Some((fixture, bytes)) = fixture_bytes(id) else {
        return;
    };
    assert!(is_raw(Path::new(fixture.file)));
    let raw = decode(&bytes).unwrap_or_else(|e| panic!("{id} failed to decode: {e:#}"));

    assert_eq!(raw.metadata.camera, fixture.camera);
    // Decoder-failure messages name the camera the same way.
    assert_eq!(
        camera_name(&RawSource::new_from_slice(&bytes)).as_deref(),
        Some(fixture.camera)
    );
    assert_eq!(
        (raw.metadata.width, raw.metadata.height),
        (fixture.width, fixture.height),
        "{id}: oriented dimensions"
    );
    assert_eq!(raw.camera.dimensions(), (fixture.width, fixture.height));
    assert!(raw.camera.as_raw().iter().all(|v| v.is_finite()));

    let matrices = [raw.xyz_to_camera, raw.camera_to_rgb];
    assert!(matrices.iter().flatten().flatten().all(|v| v.is_finite()));
    assert!(raw.xyz_to_camera.iter().flatten().any(|v| *v != 0.0));
    // camera_to_rgb is normalised so a neutral camera value stays neutral.
    for row in raw.camera_to_rgb {
        assert!(
            (row.iter().sum::<f32>() - 1.0).abs() < 1e-3,
            "{id}: {row:?}"
        );
    }

    assert_eq!(raw.as_shot[1], 1.0);
    for gain in raw.as_shot {
        assert!(
            gain.is_finite() && (0.3..8.0).contains(&gain),
            "{id}: {gain}"
        );
    }

    // A default develop of a downscaled copy must look like a photograph.
    let preview = raw.preview(256);
    assert!(preview.camera.width().max(preview.camera.height()) <= 256);
    let image = render(
        &preview,
        &DevelopSettings::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(image.dimensions(), preview.camera.dimensions());
    let luma: Vec<f64> = image
        .pixels()
        .map(|p| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
        .collect();
    let mean = luma.iter().sum::<f64>() / luma.len() as f64;
    let deviation =
        (luma.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / luma.len() as f64).sqrt();
    assert!(
        (30.0..225.0).contains(&mean) && deviation > 20.0,
        "{id}: degenerate develop (mean {mean:.1}, stddev {deviation:.1})"
    );
    let channel_mean =
        |c: usize| image.pixels().map(|p| p[c] as f64).sum::<f64>() / luma.len() as f64;
    assert!(
        (0..3).all(|c| channel_mean(c) > 10.0 && channel_mean(c) < 245.0),
        "{id}: a colour channel is blown out or empty"
    );
}

#[test]
fn real_nikon_nef_decodes() {
    check_real_raw("nikon-d70-nef");
}

#[test]
fn real_canon_cr2_portrait_decodes_oriented() {
    check_real_raw("canon-rebel-xt-cr2-portrait");
    if let Some((fixture, _)) = fixture_bytes("canon-rebel-xt-cr2-portrait") {
        assert!(fixture.height > fixture.width, "fixture must be portrait");
    }
}

#[test]
fn real_canon_cr3_decodes() {
    check_real_raw("canon-r6m3-cr3");
}

#[test]
fn real_canon_cr3_apsc_crop_decodes() {
    // rawler 0.7.2 panicked on the crop area of APS-C crop-mode CR3 files (#29).
    check_real_raw("canon-r5m2-apsc-craw-cr3");
}

#[test]
fn real_canon_crw_decodes() {
    check_real_raw("canon-d30-crw");
}

#[test]
fn real_fuji_raf_xtrans_decodes() {
    check_real_raw("fuji-x20-raf-xtrans");
    // Make sure the fixture really exercises the hand-written X-Trans path.
    if let Some((_, bytes)) = fixture_bytes("fuji-x20-raf-xtrans") {
        let header = rawler::decode_dummy(&RawSource::new_from_slice(&bytes)).unwrap();
        assert!(matches!(&header.photometric,
            RawPhotometricInterpretation::Cfa(c) if (c.cfa.width, c.cfa.height) == (6, 6)));
    }
}

#[test]
fn real_sony_arw_decodes() {
    // An APS-C crop-mode capture: the camera stored a 2816x1872 sensor area and
    // reports 2768x1848 (Sony FullImageSize), not the full-frame 4240x2832.
    check_real_raw("sony-a7s-arw");
}

#[test]
fn real_canon_sraw_is_rejected() {
    let Some((fixture, bytes)) = fixture_bytes("canon-5d2-sraw-reject") else {
        return;
    };
    assert!(fixture.reject);
    // Rawler's own debug assertions are off in test builds (see Cargo.toml), so
    // this is the same sensor-layout error that release builds report, rather
    // than a debug-only panic in rawler's header decode.
    let error = decode(&bytes).expect_err("sRAW must not be accepted");
    assert!(
        error
            .to_string()
            .contains("Canon sRAW/mRAW is not supported"),
        "{error:#}"
    );
}

#[test]
fn real_unknown_camera_is_rejected_by_name() {
    let Some((fixture, bytes)) = fixture_bytes("nikon-d1h-unknown-reject") else {
        return;
    };
    assert!(fixture.reject);
    // The Nikon D1H is missing from rawler's camera database (#29). If an
    // upgrade adds it, turn this fixture into an "ok" one.
    let error = decode(&bytes).expect_err("the D1H is not supported by rawler");
    assert_eq!(
        error.to_string(),
        "RAW files from the NIKON D1H are not supported yet"
    );
}

/// Local-only files that must fail with an error saying what is unsupported
/// (the camera, or e.g. Canon sRAW) rather than the generic "damaged" message,
/// such as samples whose licence keeps them out of the fixtures:
/// `XUAN_TEST_RAW_UNSUPPORTED=a.NEF,b.CR2 cargo test --lib unsupported -- --ignored`
#[test]
#[ignore = "Set XUAN_TEST_RAW_UNSUPPORTED to comma-separated local camera files"]
fn sample_raw_unsupported_is_explained() {
    let paths = std::env::var("XUAN_TEST_RAW_UNSUPPORTED").expect("Set XUAN_TEST_RAW_UNSUPPORTED");
    for path in paths.split(',').filter(|p| !p.is_empty()) {
        let bytes = std::fs::read(path).unwrap();
        let error = decode(&bytes).expect_err(path);
        let message = error.to_string();
        println!("{path}: {error:#}");
        assert!(
            message.contains("not supported") || message.contains("not be supported"),
            "{path}: {message}"
        );
    }
}

#[test]
fn fixture_manifest_is_consistent() {
    let list = fixtures();
    assert!(list.len() >= 6);
    let mut ids: Vec<_> = list.iter().map(|f| f.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), list.len(), "duplicate fixture ids");
    for f in &list {
        assert!(f.bytes > 0 && !f.file.is_empty(), "{}", f.id);
        assert_eq!(f.reject, f.width == 0, "{}: reject iff no dimensions", f.id);
    }
}

#[test]
fn failed_header_scan_rejects_without_a_full_decode() {
    let called = std::cell::Cell::new(false);
    let header = Err(anyhow::anyhow!("header failed"));
    let result = bounded_full_decode(header, || {
        called.set(true);
        Ok(())
    });
    assert!(!called.get());
    assert!(result.unwrap_err().to_string().contains("header failed"));
}

#[test]
fn oversized_header_rejects_without_a_full_decode() {
    let called = std::cell::Cell::new(false);
    let mut header = sensor_with(1, RawPhotometricInterpretation::LinearRaw);
    header.width = 20_000;
    header.height = 20_000;
    let result = bounded_full_decode(Ok(header), || {
        called.set(true);
        Ok(())
    });
    assert!(result.is_err() && !called.get());
    let header = sensor_with(1, RawPhotometricInterpretation::LinearRaw);
    assert!(bounded_full_decode(Ok(header), || Ok(7)).is_ok());
}

fn sensor_with(cpp: usize, photometric: RawPhotometricInterpretation) -> rawler::RawImage {
    use rawler::{
        decoders::Camera,
        rawimage::{BlackLevel, RawImageData, WhiteLevel},
    };
    let (width, height) = (12, 12);
    rawler::RawImage::new_with_data(
        Camera::default(),
        RawImageData::Float(vec![0.5; width * height * cpp]),
        width,
        height,
        cpp,
        [1.0; 4],
        photometric,
        Some(BlackLevel::new(&vec![0u32; cpp], 1, 1, cpp)),
        Some(WhiteLevel::new(vec![4096; cpp])),
        false,
    )
}

#[test]
fn sensor_validation_accepts_only_single_plane_rgb_bayer_and_xtrans() {
    use rawler::rawimage::CFAConfig;
    let cfa = |pattern: &str| {
        RawPhotometricInterpretation::Cfa(CFAConfig::new(
            &rawler::CFA::new(pattern),
            &Default::default(),
        ))
    };
    let bayer = "RGGB";
    let xtrans = "RBGBRGGGRGGBGGBGGRBRGRBGGGBGGRGGRGGB";
    assert!(validate_sensor(&sensor_with(1, cfa(bayer))).is_ok());
    assert!(validate_sensor(&sensor_with(1, cfa(xtrans))).is_ok());
    // Canon sRAW/mRAW: three planes, linear photometric.
    assert!(validate_sensor(&sensor_with(3, RawPhotometricInterpretation::LinearRaw)).is_err());
    assert!(validate_sensor(&sensor_with(1, RawPhotometricInterpretation::LinearRaw)).is_err());
    // Wrong plane count even with a valid CFA.
    assert!(validate_sensor(&sensor_with(3, cfa(bayer))).is_err());
    // Non-RGB filter arrays.
    assert!(validate_sensor(&sensor_with(1, cfa("CMYG"))).is_err());
    let message = validate_sensor(&sensor_with(3, RawPhotometricInterpretation::LinearRaw))
        .unwrap_err()
        .to_string();
    assert!(message.contains("sRAW/mRAW"));
}

/// A layer with every 8-bit level in each channel, varied alpha and fully transparent pixels.
fn filter_fixture() -> RgbaImage {
    RgbaImage::from_fn(64, 24, |x, y| {
        let level = (y * 64 + x) as u8;
        image::Rgba([
            level,
            level.wrapping_mul(7),
            level.wrapping_mul(13).wrapping_add(5),
            if x % 9 == 0 { 0 } else { 255 - (y as u8 * 3) },
        ])
    })
}

/// The Camera Raw Filter on the CPU reference path.
fn filter_cpu(original: &RgbaImage, settings: &DevelopSettings) -> Result<RgbaImage> {
    crate::gpu::scope(None, || {
        render_filter(
            &filter_source(original)?,
            original,
            settings,
            &AtomicBool::new(false),
        )
    })
}

#[test]
fn camera_raw_filter_defaults_leave_every_level_and_alpha_unchanged() {
    let original = filter_fixture();
    let source = filter_source(&original).unwrap();
    assert_eq!(source.camera.dimensions(), original.dimensions());
    assert_eq!(source.camera.get_pixel(0, 0).0, [0.0, 0.0, linear_of(5)]);
    assert_eq!(
        source.camera_to_rgb,
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    );
    assert_eq!(source.metadata.width, 64);
    let settings = DevelopSettings::camera_raw_filter();
    assert!(settings.keeps_geometry());
    assert_eq!(settings.sharpen, 0.0);
    assert_eq!(settings.color_noise, 0.0);
    assert_eq!(filter_cpu(&original, &settings).unwrap(), original);
}

/// The linear value the filter gives an 8-bit sRGB level, read back through the source.
fn linear_of(level: u8) -> f32 {
    let pixels = RgbaImage::from_pixel(1, 1, image::Rgba([level, level, level, 255]));
    filter_source(&pixels).unwrap().camera.get_pixel(0, 0)[0]
}

#[test]
fn camera_raw_filter_linearizes_srgb_and_exposure_doubles_the_light() {
    assert_eq!(linear_of(0), 0.0);
    assert_eq!(linear_of(255), 1.0);
    assert!((linear_of(128) - 0.215_860_5).abs() < 1e-6);
    assert!((linear_of(10) - 10.0 / 255.0 / 12.92).abs() < 1e-7);
    let original = RgbaImage::from_fn(2, 1, |x, _| {
        let level = if x == 0 { 128 } else { 64 };
        image::Rgba([level, level, level, 200])
    });
    let settings = DevelopSettings {
        exposure: 1.0,
        ..DevelopSettings::camera_raw_filter()
    };
    // sRGB(2 × linear(128)) = 175.56 and sRGB(2 × linear(64)) = 90.13 of 255.
    let result = filter_cpu(&original, &settings).unwrap();
    assert_eq!(result.get_pixel(0, 0).0, [176, 176, 176, 200]);
    assert_eq!(result.get_pixel(1, 0).0, [90, 90, 90, 200]);
}

#[test]
fn camera_raw_filter_temperature_is_relative_to_daylight() {
    let original = RgbaImage::from_pixel(4, 4, image::Rgba([128, 128, 128, 255]));
    let at = |temperature: f32| {
        let settings = DevelopSettings {
            white_balance: WhiteBalance::Temperature,
            temperature,
            ..DevelopSettings::camera_raw_filter()
        };
        filter_cpu(&original, &settings).unwrap().get_pixel(1, 1).0
    };
    let daylight = at(6500.0);
    for c in 0..3 {
        assert!(
            daylight[c].abs_diff(128) <= 1,
            "6500 K is neutral: {daylight:?}"
        );
    }
    // As in RAW Develop, a lower temperature corrects for warmer light: the image cools.
    let tungsten = at(3000.0);
    assert!(tungsten[2] > tungsten[0] + 40, "{tungsten:?}");
    let shade = at(10_000.0);
    assert!(shade[0] > shade[2] + 10, "{shade:?}");
}

#[test]
fn camera_raw_filter_refuses_geometry_and_strips_it_from_loaded_settings() {
    let original = filter_fixture();
    for settings in [
        DevelopSettings {
            crop: [0.1, 0.0, 1.0, 1.0],
            ..DevelopSettings::camera_raw_filter()
        },
        DevelopSettings {
            quarter_turns: 1,
            ..DevelopSettings::camera_raw_filter()
        },
        DevelopSettings {
            rotation: 3.0,
            ..DevelopSettings::camera_raw_filter()
        },
        DevelopSettings {
            distortion: 10.0,
            ..DevelopSettings::camera_raw_filter()
        },
        DevelopSettings {
            perspective: [0.0, 5.0],
            ..DevelopSettings::camera_raw_filter()
        },
        DevelopSettings {
            negative: NegativeSettings {
                enabled: true,
                ..Default::default()
            },
            ..DevelopSettings::camera_raw_filter()
        },
    ] {
        assert!(!settings.keeps_geometry());
        assert!(filter_cpu(&original, &settings).is_err());
        let stripped = settings.without_geometry();
        assert!(stripped.keeps_geometry());
        assert_eq!(stripped, DevelopSettings::camera_raw_filter());
    }
    let kept = DevelopSettings {
        exposure: 0.5,
        vignette: -20.0,
        rotation: 2.0,
        ..DevelopSettings::default()
    }
    .without_geometry();
    assert_eq!(
        (kept.exposure, kept.vignette, kept.rotation),
        (0.5, -20.0, 0.0)
    );
    // Out-of-range settings and empty layers are errors, not panics.
    let invalid = DevelopSettings {
        exposure: f32::NAN,
        ..DevelopSettings::camera_raw_filter()
    };
    assert!(filter_cpu(&original, &invalid).is_err());
    assert!(filter_source(&RgbaImage::new(0, 4)).is_err());
    let other = filter_source(&RgbaImage::new(4, 4)).unwrap();
    assert!(
        render_filter(
            &other,
            &original,
            &DevelopSettings::camera_raw_filter(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

#[test]
fn camera_raw_filter_applies_within_the_selection_and_only_to_the_unchanged_layer() {
    use crate::document::Layer;
    let original = Arc::new(RgbaImage::from_pixel(
        8,
        4,
        image::Rgba([100, 120, 140, 255]),
    ));
    let settings = DevelopSettings {
        exposure: 1.0,
        ..DevelopSettings::camera_raw_filter()
    };
    let result = filter_cpu(&original, &settings).unwrap();
    let cancel = AtomicBool::new(false);
    let mut layer = Layer::image("Photo", (*original).clone());
    layer.pixels = Some(original.clone());
    layer.transform.x = 2.0;
    let transform = layer.transform;
    // The left half of the layer, in document coordinates (the layer starts at x = 2).
    let selection =
        crate::selection::rectangle(16, 8, Point::new(2.0, 0.0), Point::new(6.0, 4.0), false);
    let mut selected = result.clone();
    crate::gpu::scope(None, || {
        within_selection(&mut selected, &original, transform, &selection, &cancel)
    })
    .unwrap();
    assert_eq!(selected.get_pixel(1, 1), result.get_pixel(1, 1));
    assert_ne!(selected.get_pixel(1, 1), original.get_pixel(1, 1));
    assert_eq!(selected.get_pixel(6, 1), original.get_pixel(6, 1));
    assert!(
        within_selection(
            &mut RgbaImage::new(2, 2),
            &original,
            transform,
            &selection,
            &cancel
        )
        .is_err()
    );

    apply_filter(&mut layer, &original, transform, selected.clone()).unwrap();
    let pixels = layer.pixels.clone().unwrap();
    assert_eq!(*pixels, selected);

    // The layer has changed since the filter opened: nothing is replaced.
    assert!(apply_filter(&mut layer, &original, transform, result.clone()).is_err());
    assert!(Arc::ptr_eq(layer.pixels.as_ref().unwrap(), &pixels));
    let mut moved = Layer::image("Photo", (*original).clone());
    moved.pixels = Some(original.clone());
    assert!(apply_filter(&mut moved, &original, transform, result.clone()).is_err());
    assert!(Arc::ptr_eq(moved.pixels.as_ref().unwrap(), &original));

    for refuse in [
        |l: &mut Layer| l.locked = true,
        |l: &mut Layer| l.pixels = None,
        |l: &mut Layer| l.filter = Some(crate::effects::Filter::BLOOM),
        |l: &mut Layer| l.group = true,
    ] {
        let mut layer = Layer::image("Photo", (*original).clone());
        layer.pixels = Some(original.clone());
        let transform = layer.transform;
        refuse(&mut layer);
        assert!(!can_filter(&layer));
        assert!(apply_filter(&mut layer, &original, transform, result.clone()).is_err());
        assert!(layer.pixels.is_none() || Arc::ptr_eq(layer.pixels.as_ref().unwrap(), &original));
    }
    // Text is rasterized, as other filters do.
    let mut text = Layer::image("Title", (*original).clone());
    text.pixels = Some(original.clone());
    text.text = Some(Default::default());
    assert!(can_filter(&text));
    let transform = text.transform;
    apply_filter(&mut text, &original, transform, result.clone()).unwrap();
    assert!(text.text.is_none());
    let mut whole = Layer::image("Photo", (*original).clone());
    whole.pixels = Some(original.clone());
    assert!(can_filter(&whole));
    let transform = whole.transform;
    assert!(apply_filter(&mut whole, &original, transform, RgbaImage::new(4, 4)).is_err());
    apply_filter(&mut whole, &original, transform, result.clone()).unwrap();
    assert_eq!(**whole.pixels.as_ref().unwrap(), result);
}

use super::*;
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
    assert!(is_raw(Path::new("PHOTO.NEF")));
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
#[ignore = "Set XUAN_TEST_RAW to a local camera file"]
fn sample_raw_develop_roundtrip() {
    let path = std::env::var_os("XUAN_TEST_RAW")
        .or_else(|| std::env::var_os("XUAN_TEST_NEF"))
        .expect("Set XUAN_TEST_RAW");
    let (asset, raw) = open(Path::new(&path)).unwrap();
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
    assert_eq!(full.dimensions(), (raw.metadata.width, raw.metadata.height));
    let mut layer = crate::document::Layer::image("RAW", full);
    layer.raw = Some(asset.clone());
    let mut document =
        crate::document::Document::new(raw.metadata.width, raw.metadata.height).unwrap();
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
    let [left, top, right, bottom] =
        crate::gpu::raw_crop(&asset.settings, [raw.camera.width(), raw.camera.height()]);
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

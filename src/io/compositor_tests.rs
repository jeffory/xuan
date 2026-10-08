//! Compositor packages built the way upstream's `ProjectStore` writes them (see upstream
//! `docs/project-format.md`), small enough to generate for each test.
use std::fs;

use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

use super::*;
use crate::{config::Language, i18n};

/// Upstream writes upper-case UUIDs.
fn upper(id: Uuid) -> String {
    id.to_string().to_uppercase()
}

fn layer(id: Uuid, name: &str) -> Value {
    json!({
        "id": upper(id), "name": name, "isVisible": true, "isGroup": false,
        "opacity": 1, "blendMode": "Normal",
        "transform": {"origin": [0, 0], "size": [4, 4], "rotation": 0,
            "flipX": false, "flipY": false, "sampling": "High quality"},
    })
}

fn image_layer(id: Uuid, name: &str) -> Value {
    let mut record = layer(id, name);
    record["imageFile"] = json!(format!("{}.png", upper(id)));
    record
}

fn folder(id: Uuid, name: &str) -> Value {
    let mut record = layer(id, name);
    record["isGroup"] = json!(true);
    record
}

fn adjustment(id: Uuid, settings: Value) -> Value {
    let mut record = layer(id, settings["kind"].as_str().unwrap());
    record["adjustment"] = settings;
    record
}

fn manifest(version: u64, layers: Vec<Value>) -> Value {
    json!({
        "format": "com.compositor.project", "version": version, "colorSpace": "sRGB",
        "documentID": upper(Uuid::new_v4()), "width": 4, "height": 4, "resolution": 72,
        "activeLayerID": layers.last().map(|l| l["id"].clone()), "layers": layers,
    })
}

/// Write `manifest` and a 4×4 PNG for every layer naming an `imageFile`.
fn package(manifest: &Value) -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("images")).unwrap();
    for record in manifest["layers"].as_array().unwrap() {
        if let Some(name) = record["imageFile"].as_str() {
            RgbaImage::from_pixel(4, 4, Rgba([200, 100, 50, 255]))
                .save(directory.path().join("images").join(name))
                .unwrap();
        }
    }
    rewrite(&directory, manifest);
    directory
}

fn rewrite(directory: &TempDir, manifest: &Value) {
    fs::write(
        directory.path().join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}

fn import(manifest: &Value) -> Result<(Document, ImportReport)> {
    load(package(manifest).path())
}

fn find(document: &Document, id: Uuid) -> &Layer {
    document.layers.iter().find(|l| l.id == id).unwrap()
}

#[test]
fn imports_swift_enum_dictionaries_and_individual_color_channels() {
    let value = json!({"kind":"Hue/Saturation", "hsvSettings": {
        "range":"Reds", "colorize":false, "invertRange":true,
        "adjustments":["Master", {"hue":5,"saturation":0,"lightness":0}, "Reds", {"hue":40,"saturation":-20,"lightness":3}],
        "bands":["Reds", {"falloffStart":310,"rangeStart":340,"rangeEnd":20,"falloffEnd":50}]
    }});
    let mut report = ImportReport::default();
    let ImportedAdjustment::Adjustment(Adjustment::HueRanges { settings }) =
        comp_adjustment(&value, 7, &mut report).unwrap()
    else {
        panic!("expected selective hue settings");
    };
    assert_eq!(settings.range, 1);
    assert_eq!(settings.adjustments[1], [40.0, -20.0, 3.0]);
    assert_eq!(settings.bands[1], [310.0, 340.0, 20.0, 50.0]);
    assert!(settings.invert_range);
    let default = json!({"black":0,"gamma":1,"white":255,"outputBlack":0,"outputWhite":255});
    let mut value = json!({"kind":"Levels","levels":{"ranges":[default,default,default,default]}});
    value["levels"]["ranges"][1]["gamma"] = json!(1.5);
    let ImportedAdjustment::Adjustment(Adjustment::LevelsChannels { ranges }) =
        comp_adjustment(&value, 7, &mut report).unwrap()
    else {
        panic!("expected channel levels");
    };
    assert_eq!(ranges[1][1], 1.5);
    assert!(report.is_empty());
}

#[test]
fn imports_swift_transform_and_rejects_path_traversal() {
    let id = Uuid::new_v4();
    let mut value = manifest(7, vec![layer(id, "Test")]);
    value["layers"][0]["transform"] =
        json!({"origin":[1,2],"size":[2,2],"rotation":30,"flipX":true,"flipY":false});
    let directory = package(&value);
    let document = load_compositor_for_test(&directory);
    assert_eq!(document.layers[0].transform.rotation, 30.0);
    assert!(document.layers[0].transform.flip_x);
    value["layers"][0]["imageFile"] = json!("../../outside.png");
    rewrite(&directory, &value);
    assert!(load(directory.path()).is_err());
}

fn load_compositor_for_test(directory: &TempDir) -> Document {
    crate::io::load_compositor(directory.path()).unwrap()
}

#[test]
fn version_7_imports_invert_black_white_and_color_balance() {
    let (invert, mono, balance, clipped) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let mut clipped_layer = image_layer(clipped, "Clipped");
    clipped_layer["maskSourceID"] = json!(upper(balance));
    let mut value = manifest(
        7,
        vec![
            image_layer(Uuid::new_v4(), "Photo"),
            adjustment(invert, json!({"kind": "Invert"})),
            adjustment(
                mono,
                json!({"kind": "Black & White", "blackWhiteSettings": {
                    "reds": -20, "yellows": 150, "greens": 40, "cyans": 60, "blues": 300,
                    "magentas": -200, "tint": true, "tintHue": 210, "tintSaturation": 35}}),
            ),
            // Settings left out take upstream's defaults.
            adjustment(
                balance,
                json!({"kind": "Color Balance", "colorBalanceSettings": {
                "shadowCyanRed": 10, "midMagentaGreen": -25, "highlightYellowBlue": 100,
                "preserveLuminosity": false}}),
            ),
            clipped_layer,
        ],
    );
    value["activeLayerID"] = json!(upper(mono));
    let (document, report) = import(&value).unwrap();
    assert_eq!(document.layers.len(), 5);
    assert_eq!(find(&document, invert).adjustment, Some(Adjustment::Invert));
    assert_eq!(
        find(&document, mono).adjustment,
        Some(Adjustment::BlackWhite {
            weights: [-20.0, 150.0, 40.0, 60.0, 300.0, -200.0],
            tint: true,
            tint_hue: 210.0,
            tint_saturation: 35.0,
        })
    );
    assert_eq!(
        find(&document, balance).adjustment,
        Some(Adjustment::ColorBalance {
            shadows: [10.0, 0.0, 0.0],
            midtones: [0.0, -25.0, 0.0],
            highlights: [0.0, 0.0, 100.0],
            preserve_luminosity: false,
        })
    );
    // Clipped to the Color Balance layer, as in upstream.
    assert_eq!(find(&document, clipped).clip_to, Some(balance));
    assert_eq!(document.active, Some(mono));
    assert!(report.is_empty(), "{report:?}");
    // Upstream's defaults when the settings are missing.
    let mut defaults = value.clone();
    defaults["layers"][2]["adjustment"] = json!({"kind": "Black & White"});
    defaults["layers"][3]["adjustment"] = json!({"kind": "Color Balance"});
    let (document, _) = import(&defaults).unwrap();
    assert_eq!(
        find(&document, mono).adjustment,
        Some(Adjustment::BLACK_WHITE)
    );
    assert_eq!(
        find(&document, balance).adjustment,
        Some(Adjustment::COLOR_BALANCE)
    );
    // Out of upstream's ranges.
    let mut wrong = value.clone();
    wrong["layers"][2]["adjustment"]["blackWhiteSettings"]["reds"] = json!(301);
    assert!(import(&wrong).is_err());
    let mut wrong = value;
    wrong["layers"][3]["adjustment"]["colorBalanceSettings"]["shadowCyanRed"] = json!(-101);
    assert!(import(&wrong).is_err());
}

#[test]
fn effects_on_folders_and_adjustment_layers_are_left_out() {
    let (group, invert, photo) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let effects = json!({"colorOverlay": {"red": 1, "green": 0, "blue": 0, "opacity": 1}});
    let mut group_record = folder(group, "Folder");
    group_record["effects"] = effects.clone();
    let mut invert_record = adjustment(invert, json!({"kind": "Invert"}));
    invert_record["effects"] = effects.clone();
    let mut photo_record = image_layer(photo, "Photo");
    photo_record["effects"] = effects;
    let (document, report) = import(&manifest(
        8,
        vec![group_record, photo_record, invert_record],
    ))
    .unwrap();
    assert_eq!(report.count(Dropped::LayerEffect), 2);
    assert!(find(&document, group).effects.is_none());
    assert!(find(&document, invert).effects.is_none());
    let overlay = find(&document, photo)
        .effects
        .as_ref()
        .unwrap()
        .color_overlay
        .unwrap();
    assert_eq!(overlay.color, [255, 0, 0]);
}

#[test]
fn version_8_imports_folder_opacity_and_guides() {
    let (group, child) = (Uuid::new_v4(), Uuid::new_v4());
    let mut group_record = folder(group, "Folder");
    group_record["opacity"] = json!(0.5);
    let mut child_record = image_layer(child, "Inside");
    child_record["parentID"] = json!(upper(group));
    let (vertical, horizontal) = (Uuid::new_v4(), Uuid::new_v4());
    let mut value = manifest(8, vec![group_record, child_record]);
    value["guides"] = json!([
        {"id": upper(vertical), "axis": "vertical", "position": 1.5},
        {"id": upper(horizontal), "axis": "horizontal", "position": -20},
    ]);
    let (document, report) = import(&value).unwrap();
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(find(&document, group).opacity, 0.5);
    assert_eq!(find(&document, child).parent, Some(group));
    // The folder's opacity multiplies into its layers: the pixel is half covered.
    let pixel = crate::render::render(&document).get_pixel(1, 1).0;
    assert!(pixel[3].abs_diff(128) <= 1, "{pixel:?}");
    assert_eq!(
        document.guides,
        vec![
            Guide {
                id: vertical,
                axis: GuideAxis::Vertical,
                position: 1.5
            },
            Guide {
                id: horizontal,
                axis: GuideAxis::Horizontal,
                position: -20.0
            },
        ]
    );
    assert_eq!(document.grid, None);

    // Neither existed before version 8.
    let mut older = value.clone();
    older["version"] = json!(7);
    assert!(import(&older).is_err());
    older["guides"] = Value::Null;
    assert!(import(&older).is_err());
    older["layers"][0]["opacity"] = json!(1);
    import(&older).unwrap();
}

#[test]
fn imported_guides_save_as_xuan_version_5() {
    let guide = Uuid::new_v4();
    let mut value = manifest(11, vec![image_layer(Uuid::new_v4(), "Photo")]);
    value["guides"] = json!([{"id": upper(guide), "axis": "horizontal", "position": 2.25}]);
    let (document, _) = import(&value).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("imported.xuan");
    crate::io::save(&document, &path).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    let saved: Value = serde_json::from_reader(archive.by_name("manifest.json").unwrap()).unwrap();
    assert_eq!(saved["version"], 5);
    assert_eq!(saved["document"]["guides"][0]["position"], 2.25);
    assert!(saved["document"].get("grid").is_none());
    let reloaded = crate::io::load(&path).unwrap();
    assert_eq!(reloaded.guides, document.guides);
    assert_eq!(reloaded.grid, None);
}

#[test]
fn version_9_imports_blur_and_noise_as_filter_layers() {
    let (gaussian, motion, noise, wide) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let mut motion_record = adjustment(
        motion,
        json!({"kind": "Motion Blur", "motionAngle": 30, "motionDistance": 40}),
    );
    motion_record["opacity"] = json!(0.25);
    let mut wide_record = adjustment(wide, json!({"kind": "Gaussian Blur", "blurRadius": 250}));
    wide_record["blendMode"] = json!("Multiply");
    let value = manifest(
        9,
        vec![
            image_layer(Uuid::new_v4(), "Photo"),
            adjustment(
                gaussian,
                json!({"kind": "Gaussian Blur", "blurRadius": 2.5}),
            ),
            motion_record,
            adjustment(
                noise,
                json!({"kind": "Add Noise", "noiseAmount": 12, "noiseGaussian": false,
                    "noiseMonochromatic": true, "noiseSeed": 77}),
            ),
            wide_record,
        ],
    );
    let (document, report) = import(&value).unwrap();
    assert_eq!(
        find(&document, gaussian).filter,
        Some(Filter::GaussianBlur { radius: 2.5 })
    );
    let motion = find(&document, motion);
    assert_eq!(
        motion.filter,
        Some(Filter::MotionBlur {
            distance: 40.0,
            angle: -30.0
        })
    );
    assert_eq!(motion.opacity, 0.25);
    assert!(motion.adjustment.is_none() && motion.pixels.is_none());
    assert_eq!(
        find(&document, noise).filter,
        Some(Filter::Noise {
            amount: 12.0,
            monochrome: true
        })
    );
    // Beyond Xuan's blur range, and blended: both adapted, counted once each.
    let wide = find(&document, wide);
    assert_eq!(wide.filter, Some(Filter::GaussianBlur { radius: 100.0 }));
    assert_eq!(wide.blend, BlendMode::Normal);
    assert_eq!(report.count(Dropped::FilterSettings), 2);
    crate::render::render(&document);

    // These kinds arrived in version 9.
    let mut older = value.clone();
    older["version"] = json!(8);
    assert!(import(&older).is_err());
}

#[test]
fn version_10_imports_text_and_letter_colors() {
    let id = Uuid::new_v4();
    let mut record = image_layer(id, "Title");
    record["text"] = json!({
        "content": "Héllo", "fontName": "HelveticaNeue-BoldItalic", "fontSize": 36,
        "red": 1, "green": 0.5, "blue": 0, "alignment": "Left", "tracking": 0, "leading": 0,
        "colorRuns": [{"location": 1, "length": 2, "red": 0, "green": 0, "blue": 1}],
    });
    // Valid upstream, but larger than Xuan's text tool sets: kept as pixels.
    let large = Uuid::new_v4();
    let mut large_record = image_layer(large, "Poster");
    large_record["text"] = json!({"content": "Big", "fontName": "Helvetica", "fontSize": 1500});
    let value = manifest(10, vec![large_record, record]);
    let (document, report) = import(&value).unwrap();
    assert!(find(&document, large).text.is_none());
    assert!(find(&document, large).pixels.is_some());
    assert_eq!(report.count(Dropped::TextAsPixels), 1);
    let text = find(&document, id).text.as_ref().unwrap();
    assert_eq!(text.content, "Héllo");
    assert_eq!(text.family, "Helvetica Neue");
    assert!(text.bold && text.italic);
    assert_eq!(text.size, 36.0);
    assert_eq!(text.color, [255, 128, 0, 255]);
    assert!(find(&document, id).pixels.is_some());
    // "él" keeps its own colour.
    assert_eq!(
        text.runs,
        vec![TextRun {
            start: 1,
            end: 3,
            style: RunStyle {
                color: Some([0, 0, 255, 255]),
                ..Default::default()
            },
        }]
    );
    assert_eq!(report.count(Dropped::TextLayout), 0);

    let mut older = value.clone();
    older["version"] = json!(9);
    assert!(import(&older).is_err());
}

/// Upstream counts runs in UTF-16 units; Xuan's runs count letters. Runs that start or end
/// inside a surrogate pair take the whole letter, colour and font runs combine, and the
/// letters keep their styles when the text is edited afterwards.
#[test]
fn letter_runs_map_utf16_ranges_to_letters_and_survive_editing() {
    let id = Uuid::new_v4();
    let mut record = image_layer(id, "Emoji");
    record["text"] = json!({
        "content": "a😀bcd", "fontName": "Helvetica", "fontSize": 20,
        "red": 0, "green": 0, "blue": 0,
        // "😀" is units 1-2: the colour run starts on its second half.
        "colorRuns": [{"location": 2, "length": 2, "red": 0, "green": 1, "blue": 0}],
        "fontRuns": [{"location": 3, "length": 3, "fontName": "Menlo-BoldItalic"}],
    });
    let (document, _) = import(&manifest(11, vec![record])).unwrap();
    let mut text = find(&document, id).text.clone().unwrap();
    let green = Some([0, 255, 0, 255]);
    let menlo = RunStyle {
        family: Some("Menlo".into()),
        bold: Some(true),
        italic: Some(true),
        ..Default::default()
    };
    assert_eq!(
        text.runs,
        vec![
            TextRun {
                start: 1,
                end: 2,
                style: RunStyle {
                    color: green,
                    ..Default::default()
                },
            },
            TextRun {
                start: 2,
                end: 3,
                style: RunStyle {
                    color: green,
                    ..menlo.clone()
                },
            },
            TextRun {
                start: 3,
                end: 5,
                style: menlo.clone(),
            },
        ]
    );
    text.validate().unwrap();
    text.replace_content("a😀bcd!".into());
    assert_eq!(text.runs.last().unwrap().end, 6);
    assert_eq!(text.letter_style(5).family, "Menlo");
    assert_eq!(text.letter_style(1).color, [0, 255, 0, 255]);
    crate::text::TextRenderer::default().render(&text).unwrap();

    // A run that changes nothing from the layer's own style adds none.
    let mut record = image_layer(id, "Plain");
    record["text"] = json!({
        "content": "abc", "fontName": "Helvetica-Bold", "red": 1, "green": 0, "blue": 0,
        "colorRuns": [{"location": 0, "length": 3, "red": 1, "green": 0, "blue": 0}],
        "fontRuns": [{"location": 0, "length": 1, "fontName": "Helvetica-Bold"}],
    });
    let (document, _) = import(&manifest(11, vec![record])).unwrap();
    assert!(find(&document, id).text.as_ref().unwrap().runs.is_empty());
}

#[test]
fn version_11_imports_text_fonts_effects_and_photoshop_blend_modes() {
    let (title, shadowed, burned, line) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let mut title_record = image_layer(title, "Title");
    title_record["text"] = json!({
        "content": "Mixed faces", "fontName": "TimesNewRomanPSMT", "fontSize": 24,
        "red": 0, "green": 0, "blue": 0, "alignment": "Center", "tracking": 0, "leading": 0,
        "boxSize": [300, 80],
        "colorRuns": [{"location": 0, "length": 5, "red": 1, "green": 0, "blue": 0}],
        "fontRuns": [{"location": 6, "length": 5, "fontName": "Menlo-Regular"}],
    });
    let mut shadowed_record = image_layer(shadowed, "Shadowed");
    shadowed_record["blendMode"] = json!("Linear Burn");
    shadowed_record["effects"] = json!({
        "stroke": {"size": 4, "red": 0, "green": 0, "blue": 0, "opacity": 1, "inside": false},
        "shadow": {"enabled": false, "angle": 90, "distance": 20, "blur": 20,
            "red": 0, "green": 0, "blue": 0, "opacity": 0.5},
        "outerGlow": {"size": 20, "red": 1, "green": 1, "blue": 1, "opacity": 0.75},
    });
    let mut burned_record = image_layer(burned, "Burned");
    burned_record["blendMode"] = json!("Linear Burn");
    let mut soft = image_layer(Uuid::new_v4(), "Soft");
    soft["blendMode"] = json!("Soft Light");
    let mut screen = image_layer(Uuid::new_v4(), "Screen");
    screen["blendMode"] = json!("Screen");
    let mut line_record = image_layer(line, "Line");
    line_record["shape"] = json!({"kind": "Line", "red": 0, "green": 0, "blue": 0,
        "cornerRadius": 0, "lineWidth": 3, "start": [0, 0], "end": [1, 1]});
    let value = manifest(
        11,
        vec![
            title_record,
            shadowed_record,
            burned_record,
            soft,
            screen,
            line_record,
        ],
    );
    let (document, report) = import(&value).unwrap();
    let text = find(&document, title).text.as_ref().unwrap();
    assert_eq!(
        (text.family.as_str(), text.bold),
        ("Times New Roman", false)
    );
    // "Mixed" is red and "faces" in Menlo, which Menlo-Regular names.
    let run = |start, end, style| TextRun { start, end, style };
    assert_eq!(
        text.runs,
        vec![
            run(
                0,
                5,
                RunStyle {
                    color: Some([255, 0, 0, 255]),
                    ..Default::default()
                }
            ),
            run(
                6,
                11,
                RunStyle {
                    family: Some("Menlo".into()),
                    ..Default::default()
                }
            ),
        ]
    );
    assert_eq!(report.count(Dropped::TextLayout), 1);
    // Effects come across with their settings; the hidden shadow stays hidden.
    assert_eq!(report.count(Dropped::LayerEffect), 0);
    let effects = find(&document, shadowed).effects.clone().unwrap();
    let stroke = effects.stroke.unwrap();
    assert_eq!(
        (stroke.size, stroke.color, stroke.inside),
        (4.0, [0; 3], false)
    );
    let shadow = effects.drop_shadow.unwrap();
    assert!(!shadow.enabled);
    assert_eq!(
        (shadow.angle, shadow.distance, shadow.blur, shadow.opacity),
        (90.0, 20.0, 20.0, 0.5)
    );
    let glow = effects.outer_glow.unwrap();
    assert_eq!(
        (glow.size, glow.color, glow.opacity),
        (20.0, [255; 3], 0.75)
    );
    assert!(effects.color_overlay.is_none() && effects.inner_glow.is_none());
    // Photoshop's blend modes come across under upstream's names.
    assert_eq!(find(&document, shadowed).blend, BlendMode::LinearBurn);
    assert_eq!(find(&document, burned).blend, BlendMode::LinearBurn);
    assert_eq!(document.layers[3].blend, BlendMode::SoftLight);
    assert_eq!(
        document.layers[4].blend,
        BlendMode::Screen,
        "supported modes keep their names"
    );
    assert!(find(&document, line).shape.is_none());
    assert!(find(&document, line).pixels.is_some());
    assert_eq!(report.count(Dropped::LineShape), 1);

    // Per-letter fonts arrived in version 11.
    let mut older = value.clone();
    older["version"] = json!(10);
    assert!(import(&older).is_err());
    // Version 12 is not understood yet, and is refused rather than half read.
    older["version"] = json!(12);
    let error = import(&older).unwrap_err().to_string();
    assert!(error.contains("version 12"), "{error}");
}

#[test]
fn summarizes_what_the_import_left_out_in_each_language() {
    let mut report = ImportReport::default();
    assert_eq!(report.summary(), None);
    for _ in 0..3 {
        report.add(Dropped::LayerEffect);
    }
    report.add(Dropped::LineShape);
    let summary = report.summary().unwrap();
    assert!(summary.starts_with("Imported with changes."), "{summary}");
    assert!(
        summary.contains("Layer effects on folders or adjustment layers: 3"),
        "{summary}"
    );
    assert!(
        summary.contains("Live line shapes (imported as pixels): 1"),
        "{summary}"
    );

    let every = [
        Dropped::LayerEffect,
        Dropped::TextLayout,
        Dropped::TextAsPixels,
        Dropped::LineShape,
        Dropped::FilterSettings,
        Dropped::ClippingMask,
    ];
    let english: Vec<_> = every.iter().map(|item| item.label()).collect();
    i18n::set_language(&Language::new("zh-CN"));
    let chinese: Vec<_> = every.iter().map(|item| item.label()).collect();
    let heading = report.summary().unwrap();
    i18n::set_language(&Language::english());
    for (english, chinese) in english.iter().zip(&chinese) {
        assert_ne!(english, chinese, "missing zh-CN text for {english}");
    }
    assert!(!heading.starts_with("Imported"), "{heading}");
}

#[test]
fn rejects_hostile_or_damaged_packages() {
    let photo = Uuid::new_v4();
    let base = manifest(11, vec![image_layer(photo, "Photo")]);
    import(&base).unwrap();
    let with = |change: &dyn Fn(&mut Value)| {
        let mut value = base.clone();
        change(&mut value);
        value
    };
    let text = |text: Value| with(&|v: &mut Value| v["layers"][0]["text"] = text.clone());
    let adjusted = |settings: Value| {
        with(&|v: &mut Value| {
            v["layers"]
                .as_array_mut()
                .unwrap()
                .push(adjustment(Uuid::new_v4(), settings.clone()))
        })
    };
    let guide = |position: f64| json!({"id": upper(Uuid::new_v4()), "axis": "vertical", "position": position});
    let cases = [
        // Canvas and layer counts.
        ("huge canvas", with(&|v| v["width"] = json!(40_000))),
        ("4 billion wide", with(&|v| v["width"] = json!(1_u64 << 32))),
        ("future version", with(&|v| v["version"] = json!(99))),
        // Guides.
        (
            "too many guides",
            with(&|v| v["guides"] = Value::Array((0..1_001).map(|_| guide(1.0)).collect())),
        ),
        ("far guide", with(&|v| v["guides"] = json!([guide(1.0e12)]))),
        (
            "duplicate guides",
            with(&|v| {
                let id = upper(Uuid::new_v4());
                v["guides"] = json!([{"id": id, "axis": "vertical", "position": 1},
                    {"id": id, "axis": "horizontal", "position": 2}]);
            }),
        ),
        (
            "diagonal guide",
            with(&|v| {
                v["guides"] =
                    json!([{"id": upper(Uuid::new_v4()), "axis": "diagonal", "position": 1}])
            }),
        ),
        // Folders.
        (
            "folder opacity",
            with(&|v| {
                v["layers"]
                    .as_array_mut()
                    .unwrap()
                    .push(folder(Uuid::new_v4(), "F"));
                v["layers"][1]["opacity"] = json!(2.0);
            }),
        ),
        (
            "folder blend",
            with(&|v| {
                v["layers"]
                    .as_array_mut()
                    .unwrap()
                    .push(folder(Uuid::new_v4(), "F"));
                v["layers"][1]["blendMode"] = json!("Multiply");
            }),
        ),
        (
            "unknown blend",
            with(&|v| v["layers"][0]["blendMode"] = json!("Sparkle")),
        ),
        (
            // Photoshop has Dissolve, but upstream never writes it.
            "Xuan-only blend",
            with(&|v| v["layers"][0]["blendMode"] = json!("Dissolve")),
        ),
        // Text.
        (
            "long text",
            text(json!({"content": "x".repeat(MAX_TEXT_UNITS + 1), "fontSize": 12})),
        ),
        (
            "huge font",
            text(json!({"content": "Hi", "fontSize": 1.0e9})),
        ),
        (
            "text color out of range",
            text(json!({"content": "Hi", "red": 7})),
        ),
        (
            "huge text box",
            text(json!({"content": "Hi", "boxSize": [30_000, 30_000]})),
        ),
        (
            "overlapping runs",
            text(json!({"content": "Hello", "colorRuns": [
                {"location": 0, "length": 3, "red": 0, "green": 0, "blue": 0},
                {"location": 2, "length": 2, "red": 0, "green": 0, "blue": 0}]})),
        ),
        (
            "run past the text",
            text(json!({"content": "Hi", "fontRuns": [
                {"location": 1, "length": u64::MAX, "fontName": "Menlo"}]})),
        ),
        (
            "font name with a newline",
            text(json!({"content": "Hi", "fontRuns": [
                {"location": 0, "length": 1, "fontName": "Menlo\nBold"}]})),
        ),
        (
            "text without pixels",
            with(&|v| {
                v["layers"][0]["imageFile"] = Value::Null;
                v["layers"][0]["text"] = json!({"content": "Hi"});
            }),
        ),
        // Adjustments.
        (
            "huge blur",
            adjusted(json!({"kind": "Gaussian Blur", "blurRadius": 1.0e9})),
        ),
        (
            "motion angle",
            adjusted(json!({"kind": "Motion Blur", "motionAngle": 120})),
        ),
        (
            "noise amount",
            adjusted(json!({"kind": "Add Noise", "noiseAmount": -1})),
        ),
        ("unknown kind", adjusted(json!({"kind": "Sharpen More"}))),
        // Effects and shapes.
        (
            "effects list",
            with(&|v| v["layers"][0]["effects"] = json!(["stroke"])),
        ),
        (
            "effect value",
            with(&|v| v["layers"][0]["effects"] = json!({"stroke": 4})),
        ),
        (
            "effect out of range",
            with(&|v| v["layers"][0]["effects"] = json!({"outerGlow": {"size": 501}})),
        ),
        (
            "effect color out of range",
            with(&|v| v["layers"][0]["effects"] = json!({"colorOverlay": {"red": 1.5}})),
        ),
        (
            "shape kind",
            with(&|v| v["layers"][0]["shape"] = json!({"kind": "Star", "cornerRadius": 0})),
        ),
    ];
    for (name, value) in cases {
        assert!(import(&value).is_err(), "{name} was accepted");
    }
}

#[test]
fn rejects_deep_folder_nesting_and_validates_wide_projects_quickly() {
    // 64 ancestors is the deepest a layer may sit; one more folder is refused.
    let chain = |depth: usize| {
        let ids: Vec<_> = (0..depth).map(|_| Uuid::new_v4()).collect();
        let mut layers = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let mut record = folder(*id, "Folder");
            if i > 0 {
                record["parentID"] = json!(upper(ids[i - 1]));
            }
            layers.push(record);
        }
        let mut leaf = layer(Uuid::new_v4(), "Leaf");
        leaf["parentID"] = json!(upper(*ids.last().unwrap()));
        layers.push(leaf);
        layers
    };
    import(&manifest(8, chain(64))).unwrap();
    assert!(import(&manifest(8, chain(65))).is_err());

    // A cycle between two folders.
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let mut first = folder(a, "A");
    first["parentID"] = json!(upper(b));
    let mut second = folder(b, "B");
    second["parentID"] = json!(upper(a));
    assert!(import(&manifest(8, vec![first, second])).is_err());

    // The most layers a project may hold, nested as deeply as allowed, loads in well under the
    // time a quadratic hierarchy check would take; one layer more is refused.
    let mut layers = Vec::new();
    while layers.len() + 65 <= crate::document::MAX_LAYERS {
        layers.extend(chain(64));
    }
    while layers.len() < crate::document::MAX_LAYERS {
        layers.push(layer(Uuid::new_v4(), "Filler"));
    }
    let started = std::time::Instant::now();
    let (document, _) = import(&manifest(8, layers.clone())).unwrap();
    assert_eq!(document.layers.len(), crate::document::MAX_LAYERS);
    assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
    layers.push(layer(Uuid::new_v4(), "One too many"));
    assert!(import(&manifest(8, layers)).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_assets() {
    let id = Uuid::new_v4();
    let value = manifest(11, vec![image_layer(id, "Photo")]);
    let directory = package(&value);
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.png");
    RgbaImage::new(1, 1).save(&target).unwrap();
    let asset = directory
        .path()
        .join("images")
        .join(format!("{}.png", upper(id)));
    fs::remove_file(&asset).unwrap();
    std::os::unix::fs::symlink(&target, &asset).unwrap();
    assert!(load(directory.path()).is_err());
}

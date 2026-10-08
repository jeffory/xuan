//! Proof-of-concept plugins in `poc/plugins/`, run against the real editor to
//! find out what the plugin SDK can and cannot do today. These are findings,
//! not regression tests: see `poc/README.md`.
#![cfg(unix)]
use super::*;
use std::time::{Duration, Instant};
use xuan::document::Layer;
use xuan::plugins::{Manifest, ui::Node};

fn poc(name: &str) -> Manifest {
    Manifest::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("poc/plugins")
            .join(name),
    )
    .unwrap()
}

fn built(name: &str, binary: &str) -> bool {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("poc/plugins")
        .join(name)
        .join("target/release")
        .join(binary);
    let ok = path.is_file();
    if !ok {
        eprintln!(
            "{} is not built; run cargo build --release there",
            path.display()
        );
    }
    ok
}

fn python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn run_until(
    context: &egui::Context,
    app: &mut EditorApp,
    mut done: impl FnMut(&EditorApp) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "timed out; error: {:?}, status: {}",
            app.error,
            app.status
        );
        frame(context, app);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A document of solid-colour frames, bottom to top.
fn open_frames(app: &mut EditorApp, size: u32, frames: &[(&str, [u8; 4])]) {
    let mut document = Document::new(size, size).unwrap();
    document.layers.clear();
    for (name, color) in frames {
        let layer = Layer::image(
            *name,
            image::RgbaImage::from_pixel(size, size, image::Rgba(*color)),
        );
        document.active = Some(layer.id);
        document.layers.push(layer);
    }
    app.sessions
        .push(Session::new(document, "frames".into(), None));
    app.current = app.sessions.len() - 1;
}

fn write_cube(path: &Path, header: &str, entries: impl Iterator<Item = [f32; 3]>) {
    let mut text = String::from(header);
    for [r, g, b] in entries {
        text.push_str(&format!("{r:.6} {g:.6} {b:.6}\n"));
    }
    std::fs::write(path, text).unwrap();
}

/// 3D LUT that rotates the channels (out = g, b, r): something no per-channel
/// adjustment in Xuan can express.
fn rotation_cube(path: &Path, n: usize) {
    let mut entries = Vec::new();
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let v = |i: usize| i as f32 / (n - 1) as f32;
                entries.push([v(g), v(b), v(r)]);
            }
        }
    }
    write_cube(
        path,
        &format!("TITLE \"Rotate\"\nLUT_3D_SIZE {n}\n"),
        entries.into_iter(),
    );
}

#[test]
fn lut_loader_bakes_3d_luts_and_maps_1d_luts_onto_curves() {
    if !built("lut-loader", "lut-loader") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let cube3 = dir.path().join("rotate.cube");
    rotation_cube(&cube3, 17);
    let cube1 = dir.path().join("lift.cube");
    write_cube(
        &cube1,
        "TITLE \"Gamma 0.6\"\nLUT_1D_SIZE 1024\n",
        (0..1024).map(|i| {
            let v = (i as f32 / 1023.0).powf(0.6);
            [v, v * 0.9, v]
        }),
    );

    let (context, mut app) = app();
    app.install_plugins(vec![poc("lut-loader")], vec![]);
    assert!(app.plugins.errors.is_empty(), "{:?}", app.plugins.errors);
    app.grant_plugin("lut-loader", true);
    open_frames(&mut app, 40, &[("Photo", [200, 100, 50, 255])]);

    // 3D: works today, baked into a new layer above the source.
    let inputs = serde_json::json!({"file": cube3});
    app.start_plugin_action_with("lut-loader", "apply", Some(&inputs));
    app.run_plugin_action();
    assert!(app.error.is_none(), "{:?}", app.error);
    run_until(&context, &mut app, |app| {
        app.dialog == Some(Dialog::PluginProposal)
    });
    let message = app
        .plugins
        .proposal
        .as_ref()
        .unwrap()
        .message
        .clone()
        .unwrap();
    eprintln!("3D LUT: {message}");
    app.resolve_proposal(true);
    let document = &app.session().unwrap().document;
    let baked = document
        .layers
        .iter()
        .find(|l| l.name == "LUT: Rotate")
        .unwrap();
    let pixel = baked.pixels.as_ref().unwrap().get_pixel(10, 10).0;
    assert_eq!(pixel, [100, 50, 200, 255], "the channels are rotated");
    assert!(
        baked.adjustment.is_none(),
        "baked pixels, not an adjustment"
    );

    // 1D: becomes a Curves adjustment layer (non-destructive, editable).
    let inputs = serde_json::json!({"file": cube1});
    app.start_plugin_action_with("lut-loader", "curves", Some(&inputs));
    app.run_plugin_action();
    run_until(&context, &mut app, |app| {
        app.dialog == Some(Dialog::PluginProposal)
    });
    let message = app
        .plugins
        .proposal
        .as_ref()
        .unwrap()
        .message
        .clone()
        .unwrap();
    eprintln!("1D LUT: {message}");
    app.resolve_proposal(true);
    let document = &app.session().unwrap().document;
    let curves = document
        .layers
        .iter()
        .find(|l| l.name == "LUT: Gamma 0.6")
        .unwrap();
    assert!(matches!(
        curves.adjustment,
        Some(xuan::document::Adjustment::CurvesChannels { .. })
    ));

    // 3D into Curves is refused with a reason.
    let inputs = serde_json::json!({"file": cube3});
    app.start_plugin_action_with("lut-loader", "curves", Some(&inputs));
    app.run_plugin_action();
    run_until(&context, &mut app, |app| {
        app.error.is_some() || app.status.contains("3D LUTs")
    });
    eprintln!("3D into Curves: {:?} / {}", app.error, app.status);
    app.stop_plugin("lut-loader");
}

/// How long the round trip takes on a 24-megapixel photo: the host writes a
/// PNG, the plugin decodes, applies and encodes one, the host reads it back.
/// `cargo test --release -- --ignored lut_round_trip` for real numbers.
#[test]
#[ignore]
fn lut_round_trip_on_a_24_megapixel_layer() {
    if !built("lut-loader", "lut-loader") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let cube = dir.path().join("rotate.cube");
    rotation_cube(&cube, 33);
    let (context, mut app) = app();
    app.install_plugins(vec![poc("lut-loader")], vec![]);
    app.grant_plugin("lut-loader", true);
    let (w, h) = (6000, 4000);
    let mut document = Document::new(w, h).unwrap();
    // A photo-like gradient, so PNG cannot compress it to nothing.
    document.layers[0].pixels = Some(std::sync::Arc::new(image::RgbaImage::from_fn(
        w,
        h,
        |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8, 255]),
    )));
    app.sessions
        .push(Session::new(document, "photo".into(), None));
    app.current = app.sessions.len() - 1;
    let started = Instant::now();
    let inputs = serde_json::json!({"file": cube});
    app.start_plugin_action_with("lut-loader", "apply", Some(&inputs));
    app.run_plugin_action();
    run_until(&context, &mut app, |app| {
        app.dialog == Some(Dialog::PluginProposal)
    });
    let message = app
        .plugins
        .proposal
        .as_ref()
        .unwrap()
        .message
        .clone()
        .unwrap();
    eprintln!(
        "24 MP: {message}; from Run to proposal {} ms",
        started.elapsed().as_millis()
    );
    app.stop_plugin("lut-loader");
}

fn gif_frames(path: &Path) -> Vec<(image::RgbaImage, u32)> {
    use image::AnimationDecoder;
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let decoder = image::codecs::gif::GifDecoder::new(file).unwrap();
    decoder
        .into_frames()
        .collect_frames()
        .unwrap()
        .into_iter()
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            (f.into_buffer(), n / d.max(1))
        })
        .collect()
}

fn export_with(
    context: &egui::Context,
    app: &mut EditorApp,
    plugin: &str,
    format: &str,
    path: &Path,
) {
    app.start_plugin_export(plugin, format, path).unwrap();
    run_until(context, app, |app| app.plugins.formats.is_empty());
}

#[test]
fn animated_gif_exports_layers_as_frames_with_caveats() {
    if !built("animated-gif", "animated-gif") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    app.install_plugins(vec![poc("animated-gif")], vec![]);
    assert!(app.plugins.errors.is_empty(), "{:?}", app.plugins.errors);
    app.grant_plugin("animated-gif", true);
    open_frames(
        &mut app,
        32,
        &[
            ("Red (50ms)", [255, 0, 0, 255]),
            ("Green", [0, 255, 0, 255]),
            ("Blue (200ms)", [0, 0, 255, 255]),
        ],
    );

    // 1. Each layer's own pixels: three frames with their delays.
    let out = dir.path().join("pixels.gif");
    export_with(&context, &mut app, "animated-gif", "gif", &out);
    assert!(app.error.is_none(), "{:?}", app.error);
    let frames = gif_frames(&out);
    let summary: Vec<_> = frames
        .iter()
        .map(|(f, ms)| (f.get_pixel(5, 5).0, *ms))
        .collect();
    eprintln!("pixels mode frames: {summary:?}");
    assert_eq!(frames.len(), 3);
    assert_eq!(summary[0], ([255, 0, 0, 255], 50));
    assert_eq!(summary[2].1, 200);

    // 2. A frame that is flipped or has 50% opacity on the canvas comes out
    //    unflipped and opaque: layer/export is the stored pixels only.
    {
        let document = &mut app.session_mut().unwrap().document;
        let layer = &mut document.layers[1];
        layer.opacity = 0.5;
        let mut half = image::RgbaImage::from_pixel(32, 32, image::Rgba([0, 255, 0, 255]));
        for y in 0..32 {
            for x in 0..16 {
                half.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        layer.pixels = Some(std::sync::Arc::new(half));
        layer.transform.flip_x = true;
    }
    app.session_mut().unwrap().invalidate();
    export_with(&context, &mut app, "animated-gif", "gif", &out);
    let frames = gif_frames(&out);
    eprintln!(
        "flipped 50% frame, left pixel: {:?} (canvas shows green at 50% over red)",
        frames[1].0.get_pixel(2, 5).0
    );
    assert_eq!(frames[1].0.get_pixel(2, 5).0, [255, 255, 255, 255]);

    // 3. Composite mode renders what the canvas shows, but only by editing
    //    the document: one undo step per frame plus one to restore.
    app.set_plugin_setting("animated-gif", "frames", serde_json::json!("composite"));
    let steps = app.session().unwrap().history.names().count();
    let out2 = dir.path().join("composite.gif");
    export_with(&context, &mut app, "animated-gif", "gif", &out2);
    assert!(app.error.is_none(), "{:?}", app.error);
    let added = app.session().unwrap().history.names().count() - steps;
    let dirty = app.session().unwrap().history.dirty();
    eprintln!("composite mode added {added} undo steps; document modified: {dirty}");
    assert_eq!(added, 4);
    let frames = gif_frames(&out2);
    assert_eq!(frames.len(), 3);

    // 4. Switching tabs while an export runs: layer/export reads the
    //    current tab, not the one being exported.
    app.set_plugin_setting("animated-gif", "frames", serde_json::json!("pixels"));
    let exported = app.current;
    app.start_plugin_export("animated-gif", "gif", &dir.path().join("race.gif"))
        .unwrap();
    open_frames(&mut app, 8, &[("Other tab", [0, 0, 0, 255])]);
    assert_ne!(app.current, exported);
    run_until(&context, &mut app, |app| app.plugins.formats.is_empty());
    eprintln!("tab switched during export: {:?}", app.error);
    assert!(
        app.error
            .as_deref()
            .unwrap_or_default()
            .contains("changed tabs")
    );
    app.error = None;

    // 5. The plugin declares GIF import, but Xuan opens .gif itself: the
    //    animation arrives as a single frame and the plugin is never asked.
    app.open_path(&out, false);
    assert!(app.error.is_none(), "{:?}", app.error);
    let layers = app.session().unwrap().document.layers.len();
    eprintln!("opening a 3-frame GIF gave {layers} layer(s)");
    assert_eq!(layers, 1);
    app.stop_plugin("animated-gif");
}

fn find_node<'a>(node: &'a Node, wanted: &dyn Fn(&Node) -> bool) -> Option<&'a Node> {
    if wanted(node) {
        return Some(node);
    }
    match node {
        Node::Column { children, .. } | Node::Row { children, .. } => {
            children.iter().find_map(|child| find_node(child, wanted))
        }
        _ => None,
    }
}

#[test]
fn timeline_pane_scrubs_through_undo_history_and_plays_in_the_sidebar() {
    if !python() {
        eprintln!("python3 not available; skipping");
        return;
    }
    let (context, mut app) = app();
    app.install_plugins(vec![poc("timeline")], vec![]);
    assert!(app.plugins.errors.is_empty(), "{:?}", app.plugins.errors);
    app.grant_plugin("timeline", true);
    let colors: Vec<(String, [u8; 4])> = (0..8)
        .map(|i| (format!("Frame {}", i + 1), [i * 30, 100, 255 - i * 30, 255]))
        .collect();
    let frames: Vec<(&str, [u8; 4])> = colors.iter().map(|(n, c)| (n.as_str(), *c)).collect();
    open_frames(&mut app, 64, &frames);
    let key = "plugin:timeline/timeline";
    run_until(&context, &mut app, |app| {
        app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
    });
    assert!(app.error.is_none(), "{:?}", app.error);

    // Scrubbing is an edit: every step lands in the undo history.
    let steps = app.session().unwrap().history.names().count();
    for _ in 0..3 {
        app.render_pane(
            key,
            "event",
            Some(xuan::plugins::ui::Event {
                widget: "next".into(),
                value: serde_json::Value::Null,
            }),
        );
        let target = app.session().unwrap().history.names().count() + 1;
        run_until(&context, &mut app, |app| {
            app.session().unwrap().history.names().count() >= target
        });
    }
    let session = app.session().unwrap();
    let added = session.history.names().count() - steps;
    eprintln!(
        "3 scrubs added {added} undo steps, last {:?}; modified: {}",
        session.history.undo_name(),
        session.history.dirty()
    );
    assert_eq!(added, 3);
    assert_eq!(session.history.undo_name(), Some("Show frame 4"));

    // Playback: the pane swaps an image as fast as pane/update goes.
    app.render_pane(
        key,
        "event",
        Some(xuan::plugins::ui::Event {
            widget: "fps".into(),
            value: serde_json::json!(30.0),
        }),
    );
    run_until(&context, &mut app, |app| !app.plugins.panes[key].pending);
    app.render_pane(
        key,
        "event",
        Some(xuan::plugins::ui::Event {
            widget: "play".into(),
            value: serde_json::Value::Null,
        }),
    );
    let started = Instant::now();
    let mut seen = Vec::new();
    while started.elapsed() < Duration::from_secs(3) {
        frame(&context, &mut app);
        if let Some(Node::Image { src, .. }) = app.plugins.panes[key]
            .tree
            .as_ref()
            .and_then(|tree| find_node(tree, &|n| matches!(n, Node::Image { .. })))
            && seen.last() != Some(src)
        {
            seen.push(src.clone());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let rate = seen.len() as f64 / started.elapsed().as_secs_f64();
    eprintln!(
        "playback at 30 fps requested: {} image changes in 3 s ({rate:.1}/s)",
        seen.len()
    );
    assert!(seen.len() > 10);
    app.render_pane(
        key,
        "event",
        Some(xuan::plugins::ui::Event {
            widget: "play".into(),
            value: serde_json::Value::Null,
        }),
    );
    run_until(&context, &mut app, |app| !app.plugins.panes[key].pending);
    app.stop_plugin("timeline");
}

fn write_ora(path: &Path) {
    use std::io::Write;
    let png = |color: [u8; 4]| {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(16, 16, image::Rgba(color))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    };
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"image/openraster").unwrap();
    let stack = r#"<?xml version="1.0" encoding="UTF-8"?>
<image w="32" h="32" xres="300">
  <stack>
    <layer name="Ink" src="data/ink.png" x="8" y="8" composite-op="svg:multiply"/>
    <stack name="Colours" opacity="0.5" composite-op="svg:screen">
      <layer name="Flat" src="data/flat.png" x="0" y="0"/>
      <layer name="Shade" src="data/shade.png" x="16" y="16" visibility="hidden" composite-op="krita:dodge"/>
    </stack>
    <layer name="Paper" src="data/paper.png"/>
  </stack>
</image>"#;
    zip.start_file("stack.xml", stored).unwrap();
    zip.write_all(stack.as_bytes()).unwrap();
    for (name, color) in [
        ("data/ink.png", [0, 0, 0, 255]),
        ("data/flat.png", [255, 0, 0, 255]),
        ("data/shade.png", [0, 0, 255, 255]),
        ("data/paper.png", [255, 255, 255, 255]),
    ] {
        zip.start_file(name, stored).unwrap();
        zip.write_all(&png(color)).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn openraster_imports_pixel_layers_and_flattens_groups() {
    if !python() {
        eprintln!("python3 not available; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ora = dir.path().join("drawing.ora");
    write_ora(&ora);
    let (context, mut app) = app();
    app.install_plugins(vec![poc("openraster")], vec![]);
    assert!(app.plugins.errors.is_empty(), "{:?}", app.plugins.errors);
    app.grant_plugin("openraster", true);
    let sessions = app.sessions.len();
    app.open_path(&ora, false);
    assert!(app.error.is_none(), "{:?}", app.error);
    run_until(&context, &mut app, |app| {
        app.sessions.len() > sessions || app.error.is_some()
    });
    assert!(app.error.is_none(), "{:?}", app.error);
    let document = &app.session().unwrap().document;
    let layers: Vec<_> = document
        .layers
        .iter()
        .map(|l| {
            (
                l.name.clone(),
                l.blend,
                l.opacity,
                l.visible,
                l.group,
                l.transform.x,
            )
        })
        .collect();
    eprintln!(
        "ORA layers bottom to top: {layers:#?}\nstatus: {}",
        app.status
    );
    assert_eq!(document.layers.len(), 4);
    assert!(
        document.layers.iter().all(|l| !l.group),
        "no groups come across"
    );
    assert_eq!(document.resolution, 300.0);
    let ink = document.layers.iter().find(|l| l.name == "Ink").unwrap();
    assert_eq!(ink.blend, xuan::blend::BlendMode::Multiply);
    assert_eq!(ink.transform.x, 8.0);

    // Export writes each top-level pixel layer back out.
    let back = dir.path().join("back.ora");
    export_with(&context, &mut app, "openraster", "ora", &back);
    assert!(app.error.is_none(), "{:?}", app.error);
    let archive = zip::ZipArchive::new(std::fs::File::open(&back).unwrap()).unwrap();
    let names: Vec<_> = archive.file_names().map(str::to_owned).collect();
    eprintln!("exported ORA entries: {names:?}");
    assert!(names.contains(&"stack.xml".to_owned()));
    assert!(names.iter().filter(|n| n.starts_with("data/")).count() >= 4);
    app.stop_plugin("openraster");
}

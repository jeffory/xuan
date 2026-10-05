//! The example plugins shipped in `plugins/`, run through their SDKs.
use super::*;
use std::time::{Duration, Instant};
use xuan::plugins::{Manifest, ui::Node};

fn python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn example(name: &str) -> Manifest {
    Manifest::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join(name),
    )
    .unwrap()
}

fn run_until(
    context: &egui::Context,
    app: &mut EditorApp,
    mut done: impl FnMut(&EditorApp) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "timed out; error: {:?}",
            app.error
        );
        frame(context, app);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn find<'a>(node: &'a Node, wanted: &str) -> Option<&'a Node> {
    match node {
        Node::Column { children, .. } | Node::Row { children, .. } => {
            children.iter().find_map(|child| find(child, wanted))
        }
        Node::Image { .. } if wanted == "image" => Some(node),
        Node::Select { .. } if wanted == "select" => Some(node),
        Node::Label { .. } if wanted == "label" => Some(node),
        _ => None,
    }
}

#[test]
fn bundled_plugins_load_with_their_shortcuts() {
    let (_context, mut app) = app();
    let manifests = ["comfy-cloud", "histogram", "invert-regions"].map(example);
    app.install_plugins(manifests.to_vec(), vec![]);
    assert!(app.plugins.errors.is_empty(), "{:?}", app.plugins.errors);
    assert!(
        app.plugins
            .shortcuts
            .iter()
            .any(|(_, plugin, _)| plugin == "invert-regions")
    );
}

#[cfg(unix)]
#[test]
fn histogram_pane_renders_through_the_python_sdk() {
    if !python() {
        eprintln!("python3 not available; skipping");
        return;
    }
    let (context, mut app) = app();
    app.install_plugins(vec![example("histogram")], vec![]);
    app.dimensions = [32, 24];
    app.new_document();
    app.command("fill_fg");
    let key = "plugin:histogram/histogram";
    run_until(&context, &mut app, |app| {
        app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
    });
    assert!(app.error.is_none(), "{:?}", app.error);
    let tree = app.plugins.panes[key].tree.clone().unwrap();
    let Some(Node::Image { src, .. }) = find(&tree, "image") else {
        panic!("{tree:?}");
    };
    assert!(src.starts_with("data:image/png;base64,"));
    assert!(find(&tree, "select").is_some());
    let decoded = xuan::plugins::ui::decode_base64(src.split_once(";base64,").unwrap().1).unwrap();
    let image = image::load_from_memory(&decoded).unwrap();
    assert_eq!((image.width(), image.height()), (256, 96));
    // The pane is drawn with the data URL loaded as a texture.
    frame(&context, &mut app);
    assert!(app.plugins.panes[key].images.contains_key(src.as_str()));

    // Changing a widget re-renders through `pane/render` with an event.
    app.render_pane(
        key,
        "event",
        Some(xuan::plugins::ui::Event {
            widget: "channel".into(),
            value: serde_json::Value::String("r".into()),
        }),
    );
    run_until(&context, &mut app, |app| {
        !app.plugins.panes[key].pending
            && matches!(
                find(app.plugins.panes[key].tree.as_ref().unwrap(), "select"),
                Some(Node::Select { value, .. }) if value == "r"
            )
    });
    // Settings reach the plugin while it runs.
    app.set_plugin_setting("histogram", "log_scale", serde_json::Value::Bool(true));
    assert_eq!(
        app.config.plugins["histogram"].settings["log_scale"],
        toml::Value::Boolean(true)
    );
    app.stop_plugin("histogram");
    assert!(app.error.is_none(), "{:?}", app.error);
}

/// Needs `cargo build --release` in `plugins/invert-regions` first.
#[cfg(unix)]
#[test]
#[ignore]
fn invert_regions_action_runs_through_the_rust_sdk() {
    let (context, mut app) = app();
    app.install_plugins(vec![example("invert-regions")], vec![]);
    app.dimensions = [40, 30];
    app.new_document();
    app.brush.color = [200, 100, 50, 255];
    app.command("fill_fg");
    app.start_plugin_action("invert-regions", "invert");
    app.add_region(Point::new(10.0, 10.0), Point::new(30.0, 20.0));
    app.run_plugin_action();
    assert!(app.error.is_none(), "{:?}", app.error);
    run_until(&context, &mut app, |app| {
        app.dialog == Some(Dialog::PluginProposal)
    });
    let document = &app.session().unwrap().document;
    let layer = document
        .layers
        .iter()
        .find(|l| l.name == "Inverted")
        .unwrap();
    let pixels = layer.pixels.as_ref().unwrap();
    let inside = pixels.get_pixel(pixels.width() / 2, pixels.height() / 2);
    assert_eq!(inside.0[..3], [55, 155, 205]);
    assert!(layer.mask.is_some());
    assert_eq!(app.status, "Inverted 1 region(s)");
    app.resolve_proposal(true);
}

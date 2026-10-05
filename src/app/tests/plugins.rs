//! Plugins hosted in the editor, exercised with a shell script that speaks
//! the protocol.
use super::*;
use std::time::{Duration, Instant};
use xuan::plugins::Manifest;

const MANIFEST: &str = r#"
[plugin]
id = "mock"
name = "Mock"
version = "0.1.0"
command = ["sh", "plugin.sh"]

[[actions]]
id = "echo"
label = "Echo Source…"
menu = "Filter"
shortcut = "Ctrl+Shift+E"
source = { crop_to_regions = true, padding = 0.0 }

[[actions.inputs]]
id = "regions"
type = "regions"
fields = [{ id = "desc", type = "text" }]

[[actions.inputs]]
id = "prompt"
type = "text"
default = "hello"

[[panes]]
id = "info"
title = "Mock info"
refresh = "document"

[[formats]]
id = "foo"
label = "Foo image"
extensions = ["foo"]
import = true
"#;

/// Answers initialize, copies the job's source image back as the result,
/// renders a small pane and imports `.foo` files as a fixture PNG.
fn script(fixture: &Path) -> String {
    format!(
        r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocol":1}}}}\n' "$id" ;;
    *'"method":"action/estimate"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"cost":"free"}}}}\n' "$id" ;;
    *'"method":"action/run"'*)
      job=$(printf '%s' "$line" | sed -n 's/.*"job":"\([^"]*\)".*/\1/p')
      work=$(printf '%s' "$line" | sed -n 's/.*"work_dir":"\([^"]*\)".*/\1/p')
      src=$(printf '%s' "$line" | sed -n 's/.*"source":{{.*"path":"\([^"]*\)".*/\1/p')
      prompt=$(printf '%s' "$line" | sed -n 's/.*"prompt":"\([^"]*\)".*/\1/p')
      printf '{{"jsonrpc":"2.0","method":"job/progress","params":{{"job":"%s","fraction":0.5,"message":"copying"}}}}\n' "$job"
      cp "$src" "$work/result.png"
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"outputs":[{{"kind":"image","path":"%s/result.png","name":"Echoed"}},{{"kind":"text","text":"prompt=%s"}}]}}}}\n' "$id" "$work" "$prompt" ;;
    *'"method":"pane/render"'*)
      case "$line" in
        *'"reason":"event"'*) text="clicked" ;;
        *'"reason":"document"'*) text="changed" ;;
        *) text="opened" ;;
      esac
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"type":"column","children":[{{"type":"label","text":"%s"}},{{"type":"button","id":"go","label":"Go"}}]}}}}\n' "$id" "$text" ;;
    *'"method":"format/import"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"width":8,"height":8,"layers":[{{"name":"Imported","image":"{fixture}"}}]}}}}\n' "$id" ;;
    *'"method":"shutdown"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":null}}\n' "$id"; exit 0 ;;
    *'"method":"host/'*|*'"method":"document/changed"'*|*'"method":"job/cancel"'*) ;;
    *) printf 'unexpected: %s\n' "$line" >&2 ;;
  esac
done
"#,
        fixture = fixture.display()
    )
}

fn install_mock(app: &mut EditorApp, dir: &Path) {
    let fixture = dir.join("fixture.png");
    RgbaImage::from_pixel(8, 8, image::Rgba([0, 200, 0, 255]))
        .save(&fixture)
        .unwrap();
    std::fs::write(dir.join("plugin.sh"), script(&fixture)).unwrap();
    std::fs::write(dir.join("plugin.toml"), MANIFEST).unwrap();
    let manifest = Manifest::load(dir).unwrap();
    app.install_plugins(vec![manifest], vec![]);
}

fn run_until(
    context: &egui::Context,
    app: &mut EditorApp,
    mut done: impl FnMut(&EditorApp) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "timed out; error: {:?}",
            app.error
        );
        frame(context, app);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn plugin_action_proposes_a_masked_layer_that_records_its_origin() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    assert_eq!(
        app.pane_title("plugin:mock/info").as_deref(),
        Some("Mock info")
    );
    assert!(app.config.panes.get("plugin:mock/info").is_some());
    assert_eq!(
        app.plugin_menu_items()[&xuan::plugins::manifest::Menu::Filter].len(),
        1
    );

    app.dimensions = [64, 48];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);
    let source = app.session().unwrap().document.active.unwrap();

    // The dialog opens with the Region tool and the declared defaults.
    app.start_plugin_action("mock", "echo");
    assert!(app.plugins.action.is_some());
    assert_eq!(app.tool, Tool::Region);
    assert_eq!(
        app.plugins.action.as_ref().unwrap().values["prompt"],
        serde_json::Value::String("hello".into())
    );
    // Drag a region on the canvas, then add one through the API.
    frame(&context, &mut app);
    let zoom = app.session().unwrap().zoom;
    let origin = app.canvas_rect.unwrap().min;
    let at = |p: Point| origin + Vec2::new(p.x, p.y) * zoom;
    pointer_frame(
        &context,
        &mut app,
        at(Point::new(8.0, 8.0)),
        Some(true),
        egui::Modifiers::NONE,
    );
    pointer_frame(
        &context,
        &mut app,
        at(Point::new(24.0, 20.0)),
        None,
        egui::Modifiers::NONE,
    );
    pointer_frame(
        &context,
        &mut app,
        at(Point::new(24.0, 20.0)),
        Some(false),
        egui::Modifiers::NONE,
    );
    assert_eq!(
        app.plugins.action.as_ref().unwrap().regions.len(),
        1,
        "{:?}",
        app.error
    );
    app.add_region(Point::new(40.0, 10.0), Point::new(60.0, 40.0));
    let edit = app.plugins.action.as_mut().unwrap();
    assert_eq!(edit.regions.len(), 2);
    assert_eq!(edit.selected, Some(1));
    edit.regions[1].fields.insert("desc".into(), "hat".into());
    app.select_region_at(Point::new(10.0, 10.0));
    assert_eq!(app.plugins.action.as_ref().unwrap().selected, Some(0));
    // Drawing regions never touches the document.
    assert_eq!(app.session().unwrap().history.names().count(), 1);

    app.run_plugin_action();
    assert!(app.error.is_none(), "{:?}", app.error);
    assert!(app.plugins.action.is_none());
    assert_ne!(app.tool, Tool::Region);
    assert_eq!(app.plugins.jobs.len(), 1);
    run_until(&context, &mut app, |app| {
        app.dialog == Some(Dialog::PluginProposal)
    });
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(app.status, "prompt=hello");
    let document = &app.session().unwrap().document;
    assert_eq!(document.layers.len(), 2);
    let layer = document.layers.iter().find(|l| l.name == "Echoed").unwrap();
    assert_eq!(document.active, Some(layer.id));
    // The crop around both regions, padded by nothing, is placed back where it came from.
    // The pointer drag lands within a pixel of the requested corner.
    assert!((layer.transform.x - 8.0).abs() <= 1.0 && (layer.transform.y - 8.0).abs() <= 1.0);
    assert!((layer.transform.width - 52.0).abs() <= 1.0);
    assert!((layer.transform.height - 32.0).abs() <= 1.0);
    let (width, height) = layer.pixels.as_ref().unwrap().dimensions();
    assert_eq!(
        (width as f32, height as f32),
        (layer.transform.width, layer.transform.height)
    );
    let mask = &layer.mask.as_ref().unwrap().pixels;
    assert!(mask.get_pixel(5, 5)[0] > 200, "inside the first region");
    assert!(mask.get_pixel(30, 2)[0] < 60, "between the regions");
    let generated = layer.generated.clone().unwrap();
    assert_eq!(
        (generated.plugin.as_str(), generated.action.as_str()),
        ("mock", "echo")
    );
    assert_eq!(generated.source, Some(source));
    assert!(generated.source_hash.is_some());
    assert_eq!(generated.inputs["prompt"], "hello");
    assert_eq!(generated.inputs["regions"][1]["fields"]["desc"], "hat");
    assert!((generated.inputs["regions"][0]["x"].as_f64().unwrap() - 8.0).abs() <= 1.0);

    // Compare hides the result; accepting commits a single undo step.
    app.toggle_proposal_compare();
    assert!(!app.session().unwrap().document.layers[1].visible);
    app.toggle_proposal_compare();
    app.resolve_proposal(true);
    assert_eq!(app.dialog, None);
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), 2);
    assert_eq!(session.history.undo_name(), Some("Echo Source"));
    app.command("undo");
    assert_eq!(app.session().unwrap().document.layers.len(), 1);
    app.command("redo");
    assert_eq!(app.session().unwrap().document.layers.len(), 2);

    // Re-running starts the dialog again from the stored inputs.
    app.command("rerun_plugin");
    let edit = app.plugins.action.as_ref().unwrap();
    assert_eq!(edit.regions.len(), 2);
    assert_eq!(edit.regions[1].fields["desc"], "hat");
    assert_eq!(app.session().unwrap().document.active, Some(source));
    app.close_plugin_action();

    // A second run that is discarded leaves nothing behind.
    app.start_plugin_action("mock", "echo");
    app.run_plugin_action();
    run_until(&context, &mut app, |app| {
        app.dialog == Some(Dialog::PluginProposal)
    });
    assert_eq!(app.session().unwrap().document.layers.len(), 3);
    app.resolve_proposal(false);
    assert_eq!(app.session().unwrap().document.layers.len(), 2);
    assert_eq!(app.session().unwrap().history.names().count(), 2);

    // Provenance survives the project file, which becomes version 5.
    let path = dir.path().join("project.xuan");
    io::save(&app.session().unwrap().document, &path).unwrap();
    let loaded = io::load(&path).unwrap();
    let restored = loaded.layers.iter().find(|l| l.name == "Echoed").unwrap();
    assert_eq!(restored.generated.as_ref().unwrap().action, "echo");
    assert!(restored.mask.is_some());
}

#[cfg(unix)]
#[test]
fn plugin_panes_render_events_and_document_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [16, 16];
    app.new_document();
    frame(&context, &mut app);
    let key = "plugin:mock/info";
    // The pane is drawn by the sidebar, which asks for its first render.
    run_until(&context, &mut app, |app| {
        app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
    });
    let text = |app: &EditorApp| match app.plugins.panes[key].tree.as_ref().unwrap() {
        xuan::plugins::ui::Node::Column { children, .. } => match &children[0] {
            xuan::plugins::ui::Node::Label { text, .. } => text.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    assert_eq!(text(&app), "opened");
    app.render_pane(
        key,
        "event",
        Some(xuan::plugins::ui::Event {
            widget: "go".into(),
            value: serde_json::Value::Bool(true),
        }),
    );
    run_until(&context, &mut app, |app| {
        !app.plugins.panes[key].pending && app.plugins.panes[key].tree.is_some()
    });
    run_until(&context, &mut app, |app| text(app) == "clicked");
    // An edit re-renders panes that asked to follow the document.
    app.command("fill_fg");
    run_until(&context, &mut app, |app| text(app) == "changed");
    assert!(app.plugins.running("mock"));
    app.stop_plugin("mock");
    assert!(!app.plugins.running("mock"));
}

#[cfg(unix)]
#[test]
fn plugin_formats_import_documents_and_layers() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    assert_eq!(app.plugin_import_extensions(), ["foo"]);
    let file = dir.path().join("picture.foo");
    std::fs::write(&file, b"").unwrap();
    app.open_path(&file, false);
    assert!(app.error.is_none(), "{:?}", app.error);
    let session = app.session().unwrap();
    assert_eq!(session.title, "picture");
    assert_eq!((session.document.width, session.document.height), (8, 8));
    assert_eq!(session.document.layers[0].name, "Imported");
    assert!(session.history.dirty());
    app.open_path(&file, true);
    assert_eq!(app.session().unwrap().document.layers.len(), 2);
    assert_eq!(app.session().unwrap().history.names().count(), 1);
    // Built-in formats are never handed to plugins.
    assert!(app.plugin_import_format(Path::new("x.FOO")).is_some());
    assert!(app.plugin_import_format(Path::new("x.png")).is_none());
    assert!(builtin_extension(Path::new("x.PNG")));
    frame(&context, &mut app);
}

#[cfg(unix)]
#[test]
fn ungranted_plugins_ask_for_permission_before_running() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    let fixture = dir.path().join("fixture.png");
    RgbaImage::new(2, 2).save(&fixture).unwrap();
    std::fs::write(dir.path().join("plugin.sh"), script(&fixture)).unwrap();
    std::fs::write(
        dir.path().join("plugin.toml"),
        MANIFEST.replace(
            "[[actions]]",
            "[permissions]\nnetwork = [\"example.com\"]\n\n[[actions]]",
        ),
    )
    .unwrap();
    app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    assert!(!app.plugin_granted("mock"));
    app.start_plugin_action("mock", "echo");
    assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
    assert!(app.plugins.action.is_none());
    frame(&context, &mut app);
    app.grant_plugin("mock", true);
    assert!(app.plugin_granted("mock"));
    app.dialog = None;
    app.plugins.permission_request = None;
    app.start_plugin_action("mock", "echo");
    assert!(app.plugins.action.is_some());
    app.close_plugin_action();
    app.grant_plugin("mock", false);
    assert!(!app.plugin_granted("mock"));
    app.config.plugins.get_mut("mock").unwrap().enabled = false;
    assert!(app.pane_title("plugin:mock/info").is_none());
    assert!(app.plugin_menu_items().is_empty());
}

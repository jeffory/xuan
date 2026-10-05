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

    // Provenance survives the project file, which becomes version 6.
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
    // A document refresh asked for while a render is pending is not dropped:
    // the pane renders again once the pending render ends.
    app.render_pane(
        key,
        "event",
        Some(xuan::plugins::ui::Event {
            widget: "go".into(),
            value: serde_json::Value::Bool(true),
        }),
    );
    assert!(app.plugins.panes[key].pending);
    app.render_pane(key, "document", None);
    assert!(app.plugins.panes[key].dirty);
    run_until(&context, &mut app, |app| text(app) == "changed");
    assert!(!app.plugins.panes[key].dirty);
    app.render_pane(
        key,
        "event",
        Some(xuan::plugins::ui::Event {
            widget: "go".into(),
            value: serde_json::Value::Bool(true),
        }),
    );
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

/// Answers `initialize` and ignores everything else, except `format/import`,
/// where it crashes without answering.
const CRASHING_IMPORT: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":1}}\n' "$id" ;;
    *'"method":"format/import"'*) exit 3 ;;
  esac
done
"#;

#[cfg(unix)]
#[test]
fn a_plugin_that_crashes_during_a_blocking_call_is_cleaned_up() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    std::fs::write(dir.path().join("plugin.sh"), CRASHING_IMPORT).unwrap();
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);

    // A job and a pane render wait for answers that never come.
    app.start_plugin_action("mock", "echo");
    app.add_region(Point::new(1.0, 1.0), Point::new(8.0, 8.0));
    app.run_plugin_action();
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(app.plugins.jobs.len(), 1);
    let key = "plugin:mock/info";
    app.render_pane(key, "open", None);
    assert!(app.plugins.panes[key].pending);

    // The plugin dies while Xuan blocks on format/import.
    let file = dir.path().join("picture.foo");
    std::fs::write(&file, b"").unwrap();
    app.open_path(&file, false);
    assert!(
        app.error.as_deref().is_some_and(|e| e.contains("exited")),
        "{:?}",
        app.error
    );
    assert!(!app.plugins.running("mock"));
    assert!(app.plugins.jobs.is_empty());
    assert!(!app.plugins.panes[key].pending);
    assert!(app.plugins.panes[key].error.is_some());
    app.error = None;
    frame(&context, &mut app);

    // The document is not wedged: a new action starts a new process.
    app.start_plugin_action("mock", "echo");
    app.add_region(Point::new(1.0, 1.0), Point::new(8.0, 8.0));
    app.run_plugin_action();
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(app.plugins.jobs.len(), 1);
    assert!(app.plugins.running("mock"));
}

#[cfg(unix)]
#[test]
fn reloading_fails_the_jobs_of_removed_and_changed_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    // The plugin never answers runs or renders.
    std::fs::write(dir.path().join("plugin.sh"), CRASHING_IMPORT).unwrap();
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);
    let start = |app: &mut EditorApp| {
        app.start_plugin_action("mock", "echo");
        app.add_region(Point::new(1.0, 1.0), Point::new(8.0, 8.0));
        app.run_plugin_action();
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.plugins.jobs.len(), 1);
    };
    let key = "plugin:mock/info";

    // Removed: the job ends with an error and the document is free again.
    start(&mut app);
    app.render_pane(key, "open", None);
    app.install_plugins(vec![], vec![]);
    assert!(app.plugins.jobs.is_empty());
    assert!(!app.plugins.running("mock"));
    assert!(
        app.error.as_deref().is_some_and(|e| e.contains("Reload")),
        "{:?}",
        app.error
    );
    app.error = None;

    // Re-added: a new process starts and new actions run.
    let manifest = Manifest::load(dir.path()).unwrap();
    app.install_plugins(vec![manifest.clone()], vec![]);
    start(&mut app);
    // Unchanged on reload: the process and its job are kept.
    app.install_plugins(vec![manifest.clone()], vec![]);
    assert_eq!(app.plugins.jobs.len(), 1);
    assert!(app.plugins.running("mock"));
    // Changed: the process restarts, so its job fails too.
    let mut changed = manifest;
    changed.plugin.version = "0.2.0".into();
    app.install_plugins(vec![changed], vec![]);
    assert!(app.plugins.jobs.is_empty());
    assert!(!app.plugins.running("mock"));
    app.error = None;
    start(&mut app);
    frame(&context, &mut app);
    assert_eq!(app.plugins.jobs.len(), 1);
}

fn echoed(app: &EditorApp, index: usize) -> usize {
    app.sessions[index]
        .document
        .layers
        .iter()
        .filter(|l| l.name == "Echoed")
        .count()
}

#[cfg(unix)]
#[test]
fn a_result_that_finishes_while_a_dialog_is_open_is_applied_afterwards() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);

    app.start_plugin_action("mock", "echo");
    app.run_plugin_action();
    // The user opens Manage Plugins while the job runs.
    app.dialog = Some(Dialog::Plugins);
    run_until(&context, &mut app, |app| app.plugins.jobs.is_empty());
    for _ in 0..3 {
        frame(&context, &mut app);
    }
    assert!(app.error.is_none(), "{:?}", app.error);
    assert!(app.plugins.proposal.is_none());
    assert_eq!(app.plugins.completed.len(), 1);
    assert_eq!(echoed(&app, 0), 0);

    app.dialog = None;
    frame(&context, &mut app);
    assert_eq!(app.dialog, Some(Dialog::PluginProposal));
    assert!(app.plugins.completed.is_empty());
    assert_eq!(echoed(&app, 0), 1);
    app.resolve_proposal(true);
    assert_eq!(echoed(&app, 0), 1);
}

#[cfg(unix)]
#[test]
fn results_for_two_documents_are_proposed_one_after_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [16, 16];
    for _ in 0..2 {
        app.new_document();
        app.command("fill_fg");
        frame(&context, &mut app);
    }
    let ids: Vec<_> = app.sessions.iter().map(|s| s.document.id).collect();
    for index in 0..2 {
        app.current = index;
        app.start_plugin_action("mock", "echo");
        app.run_plugin_action();
        assert!(app.error.is_none(), "{:?}", app.error);
    }
    assert_eq!(app.plugins.jobs.len(), 2);

    // The first result is proposed; the second waits instead of being lost.
    run_until(&context, &mut app, |app| {
        app.plugins.jobs.is_empty() && app.plugins.proposal.is_some()
    });
    for _ in 0..3 {
        frame(&context, &mut app);
    }
    assert!(app.error.is_none(), "{:?}", app.error);
    let first = app.plugins.proposal.as_ref().unwrap().document;
    assert_eq!(app.plugins.completed.len(), 1);
    app.resolve_proposal(true);
    frame(&context, &mut app);
    let second = app.plugins.proposal.as_ref().unwrap().document;
    assert_ne!(first, second);
    assert_eq!(app.dialog, Some(Dialog::PluginProposal));
    app.resolve_proposal(true);
    assert!(app.plugins.completed.is_empty());
    assert!(ids.contains(&first) && ids.contains(&second));
    assert_eq!((echoed(&app, 0), echoed(&app, 1)), (1, 1));
    for session in &app.sessions {
        assert_eq!(session.history.undo_name(), Some("Echo Source"));
    }
}

#[test]
fn reset_panel_layout_keeps_the_installed_plugin_panes() {
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    let key = "plugin:mock/info";
    app.config.panes.set_hidden(key, true);
    app.config.panes.set_height(key, 300.0);
    app.config.panes.toggle_collapsed(xuan::panes::LAYERS);
    // A pane of a plugin that is no longer installed is forgotten.
    app.config.panes.ensure("plugin:gone/pane");
    app.command("reset_panels");
    let ids: Vec<_> = app.config.panes.0.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, [xuan::panes::NAVIGATOR, xuan::panes::LAYERS, key]);
    let pane = app.config.panes.get(key).unwrap();
    assert!(!pane.hidden && !pane.collapsed && pane.height == 0.0);
    assert!(!app.config.panes.get(xuan::panes::LAYERS).unwrap().collapsed);
    let entries = app.pane_entries();
    assert!(entries.iter().any(|(id, _, visible)| id == key && *visible));
}

fn click(context: &egui::Context, app: &mut EditorApp, pos: Pos2) {
    pointer_frame(context, app, pos, None, egui::Modifiers::NONE);
    pointer_frame(context, app, pos, Some(true), egui::Modifiers::NONE);
    pointer_frame(context, app, pos, Some(false), egui::Modifiers::NONE);
}

#[cfg(unix)]
#[test]
fn a_proposal_is_never_accepted_without_the_accept_button() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    // A second plugin, not yet granted, whose pane offers "Review Permissions…".
    std::fs::copy(dir.path().join("plugin.sh"), other.path().join("plugin.sh")).unwrap();
    let guarded = Manifest::parse(
        &MANIFEST
            .replace("id = \"mock\"", "id = \"guarded\"")
            .replace("shortcut = \"Ctrl+Shift+E\"\n", "")
            .replace(
                "[[actions]]",
                "[permissions]\nnetwork = [\"example.com\"]\n\n[[actions]]",
            ),
        other.path(),
    )
    .unwrap();
    let mock = app.plugins.manifest("mock").unwrap().clone();
    app.install_plugins(vec![mock, guarded], vec![]);
    app.config.panes.set_hidden("plugin:mock/info", true);
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);
    let button = |context: &egui::Context, app: &mut EditorApp| {
        frame(context, app)
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Review Permissions…" => {
                    Some(text.pos + Vec2::new(5.0, 5.0))
                }
                _ => None,
            })
            .expect("the pane offers to review permissions")
    };
    let propose = |context: &egui::Context, app: &mut EditorApp| {
        app.start_plugin_action("mock", "echo");
        app.run_plugin_action();
        run_until(context, app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        assert_eq!(echoed(app, 0), 1);
    };
    let steps = app.session().unwrap().history.names().count();

    // While the proposal is open, the pane takes no clicks.
    propose(&context, &mut app);
    let pos = button(&context, &mut app);
    click(&context, &mut app, pos);
    frame(&context, &mut app);
    assert_eq!(app.dialog, Some(Dialog::PluginProposal));
    assert!(app.plugins.proposal.is_some());
    assert_eq!(app.session().unwrap().history.names().count(), steps);

    // Whatever replaces the proposal's dialog discards it; nothing is committed.
    app.dialog = Some(Dialog::PluginPermissions);
    app.plugins.permission_request = Some((
        "guarded".into(),
        super::super::plugins::PendingStart::Action("echo".into()),
    ));
    frame(&context, &mut app);
    assert!(app.plugins.proposal.is_none());
    assert_eq!(echoed(&app, 0), 0);
    assert_eq!(app.session().unwrap().history.names().count(), steps);
    assert_ne!(
        app.session().unwrap().history.undo_name(),
        Some("Echo Source")
    );
    app.dialog = None;
    app.plugins.permission_request = None;

    // With nothing open, the same button works.
    frame(&context, &mut app);
    let pos = button(&context, &mut app);
    click(&context, &mut app, pos);
    assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
}

#[cfg(unix)]
#[test]
fn plugin_menus_panes_and_chords_work_through_the_ui() {
    use super::ui::UiTest;
    let dir = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(dir.path());
    install_mock(ui.app_mut(), dir.path());
    ui.app_mut().command("fill_fg");
    ui.settle();

    // The pane is listed in the Window menu after the built-in panes.
    ui.open_menu("Window");
    assert!(ui.has_role(egui::accesskit::Role::CheckBox, "Mock info"));
    ui.key(egui::Key::Escape);
    let ids: Vec<_> = ui.app().pane_entries().into_iter().map(|e| e.0).collect();
    assert_eq!(
        ids,
        [
            xuan::panes::NAVIGATOR,
            xuan::panes::LAYERS,
            "plugin:mock/info"
        ]
    );

    // The menu item shows the chord, and the chord opens the action dialog
    // instead of running Merge (Ctrl+E).
    ui.open_menu("Filter");
    assert!(ui.has("Echo Source… Ctrl+Shift+E"));
    ui.key(egui::Key::Escape);
    let steps = ui.app().session().unwrap().history.names().count();
    ui.press(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::E);
    assert!(ui.app().plugins.action.is_some(), "{:?}", ui.app().error);
    assert_eq!(ui.app().session().unwrap().history.names().count(), steps);
}

#[test]
fn shortcuts_match_their_modifiers_exactly() {
    use super::shortcuts::{builtin_for, consume_exact};
    use egui::{Key, Modifiers};
    let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
    let press = |modifiers: Modifiers, key: Key| {
        let mut input = egui::InputState::default();
        input.events.push(text_key(key, modifiers));
        input
    };
    // Ctrl+Shift+I is not Ctrl+I, and Ctrl+E is not Ctrl+Shift+E.
    let mut input = press(ctrl_shift, Key::I);
    assert!(!consume_exact(&mut input, Modifiers::CTRL, Key::I));
    assert!(consume_exact(&mut input, ctrl_shift, Key::I));
    assert!(input.events.is_empty());
    let mut input = press(Modifiers::CTRL, Key::E);
    assert!(!consume_exact(&mut input, ctrl_shift, Key::E));
    assert!(consume_exact(&mut input, Modifiers::CTRL, Key::E));
    let mut input = press(Modifiers::CTRL | Modifiers::ALT, Key::E);
    assert!(!consume_exact(&mut input, Modifiers::CTRL, Key::E));
    // `+` needs Shift on many layouts.
    let mut input = press(ctrl_shift, Key::Plus);
    assert!(consume_exact(&mut input, Modifiers::CTRL, Key::Plus));

    assert_eq!(builtin_for(ctrl_shift, Key::I), Some("invert_selection"));
    assert_eq!(builtin_for(Modifiers::CTRL, Key::I), Some("invert"));
    assert_eq!(builtin_for(Modifiers::CTRL, Key::E), Some("merge"));
    assert_eq!(builtin_for(Modifiers::CTRL, Key::H), Some("hide_controls"));
    assert_eq!(builtin_for(Modifiers::NONE, Key::F1), Some("shortcuts"));
    assert_eq!(builtin_for(Modifiers::CTRL, Key::R), Some("toggle_rulers"));
    assert_eq!(
        builtin_for(Modifiers::CTRL, Key::Semicolon),
        Some("toggle_guides")
    );
    assert_eq!(builtin_for(ctrl_shift, Key::Quote), Some("toggle_grid"));
    assert_eq!(builtin_for(ctrl_shift, Key::E), None);
    assert_eq!(builtin_for(Modifiers::CTRL | Modifiers::ALT, Key::I), None);
}

#[cfg(unix)]
#[test]
fn plugin_shortcuts_are_not_swallowed_by_builtins_and_collisions_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    assert!(app.plugins.errors.is_empty(), "{:?}", app.plugins.errors);
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);
    let steps = app.session().unwrap().history.names().count();
    let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;

    // Ctrl+Shift+E reaches the plugin instead of Merge (Ctrl+E).
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(egui::Key::E, ctrl_shift)],
        ctrl_shift,
    );
    assert!(app.plugins.action.is_some(), "{:?}", app.error);
    assert_eq!(app.session().unwrap().history.names().count(), steps);
    app.close_plugin_action();

    // A plugin chord that Xuan already uses is reported and left out.
    let colliding = MANIFEST.replace("Ctrl+Shift+E", "Ctrl+Shift+I");
    std::fs::write(dir.path().join("plugin.toml"), colliding).unwrap();
    app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
    assert!(app.plugins.shortcuts.is_empty());
    assert_eq!(app.plugins.errors.len(), 1);
    let error = &app.plugins.errors[0].error;
    assert!(
        error.contains("Ctrl+Shift+I") && error.contains("invert_selection"),
        "{error}"
    );
    let items = app.plugin_menu_items();
    assert_eq!(items[&xuan::plugins::manifest::Menu::Filter][0].3, "");

    // The built-in keeps working and the plugin does not start.
    app.command("select_all");
    let steps = app.session().unwrap().history.names().count();
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(egui::Key::I, ctrl_shift)],
        ctrl_shift,
    );
    assert!(app.plugins.action.is_none());
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), steps + 1);
    assert_eq!(session.history.undo_name(), Some("Invert Selection"));

    // Two plugins cannot share a chord either.
    let other = tempfile::tempdir().unwrap();
    let first = Manifest::parse(&MANIFEST.replace("Ctrl+Shift+E", "Ctrl+Alt+E"), dir.path());
    let second = Manifest::parse(
        &MANIFEST
            .replace("Ctrl+Shift+E", "Ctrl+Alt+E")
            .replace("id = \"mock\"", "id = \"mock2\""),
        other.path(),
    );
    app.install_plugins(vec![first.unwrap(), second.unwrap()], vec![]);
    assert_eq!(app.plugins.shortcuts.len(), 1);
    assert!(app.plugins.errors[0].error.contains("mock/echo"));
}

//! Plugins hosted in the editor, exercised with a shell script that speaks
//! the protocol.
use super::*;
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
/// renders a small pane and imports `.foo` files as a fixture PNG. Every
/// line it receives is appended to `received.log` in its folder, and a click
/// on a pane widget named `export` makes it ask for `document/export`.
fn script(fixture: &Path) -> String {
    format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> received.log
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
        *'"widget":"export"'*)
          printf '{{"jsonrpc":"2.0","id":"export","method":"document/export","params":{{}}}}\n'
          text="exporting" ;;
        *'"reason":"event"'*) text="clicked" ;;
        *'"reason":"document"'*) text="changed" ;;
        *) text="opened" ;;
      esac
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"type":"column","children":[{{"type":"label","text":"%s"}},{{"type":"button","id":"go","label":"Go"}}]}}}}\n' "$id" "$text" ;;
    *'"method":"format/import"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"width":8,"height":8,"layers":[{{"name":"Imported","image":"{fixture}"}}]}}}}\n' "$id" ;;
    *'"method":"format/export"'*)
      image=$(printf '%s' "$line" | sed -n 's/.*"image":"\([^"]*\)".*/\1/p')
      out=$(printf '%s' "$line" | sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
      cp "$image" "$out"
      printf '{{"jsonrpc":"2.0","id":%s,"result":null}}\n' "$id" ;;
    *'"method":"shutdown"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":null}}\n' "$id"; exit 0 ;;
    *'"method":"host/'*|*'"method":"document/changed"'*|*'"method":"job/cancel"'*) ;;
    *'"id":"export"'*) ;;
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
    app.grant_plugin("mock", true);
}

/// The mock manifest declaring network `hosts`, with a second action,
/// "Send Layer", that has no inputs and sends the active layer.
fn network_manifest(hosts: &str) -> String {
    format!(
        "{}\n[[actions]]\nid = \"send\"\nlabel = \"Send Layer\"\n",
        MANIFEST.replace(
            "[[actions]]",
            &format!("[permissions]\nnetwork = [{hosts}]\n\n[[actions]]"),
        )
    )
}

/// Installs and allows the mock plugin declaring the host `example.com`.
fn install_network_mock(app: &mut EditorApp, dir: &Path) {
    let fixture = dir.join("fixture.png");
    RgbaImage::from_pixel(8, 8, image::Rgba([0, 200, 0, 255]))
        .save(&fixture)
        .unwrap();
    std::fs::write(dir.join("plugin.sh"), script(&fixture)).unwrap();
    std::fs::write(dir.join("plugin.toml"), network_manifest("\"example.com\"")).unwrap();
    app.install_plugins(vec![Manifest::load(dir).unwrap()], vec![]);
    app.grant_plugin("mock", true);
}

fn send_without_asking(app: &EditorApp) -> bool {
    app.stored_grant("mock")
        .is_some_and(|grant| grant.send_without_asking)
}

#[test]
fn exports_to_network_plugins_follow_the_consent_rules() {
    use crate::app::plugin_consent::ConsentRequest;
    use serde_json::json;
    use xuan::plugins::protocol::CANCELLED;
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    app.config_path = Some(config.path().join("config.toml"));
    install_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    // A plugin that declares no network hosts is never asked about.
    assert!(!app.sends_need_consent("mock"));
    assert_eq!(app.export_answer("mock"), Some(true));
    assert!(plugin_request(&mut app, "document/export", json!({})).is_ok());

    install_network_mock(&mut app, dir.path());
    assert!(app.sends_need_consent("mock"));
    assert_eq!(app.export_answer("mock"), None);
    for method in ["document/export", "layer/export", "selection/export"] {
        let refused = plugin_request(&mut app, method, json!({})).unwrap_err();
        assert_eq!(refused.code, CANCELLED, "{method}");
    }
    // The document's structure is not document data.
    assert!(plugin_request(&mut app, "document/get", json!({})).is_ok());
    // While an action the user confirmed runs, the plugin may export more;
    // a job that had nothing to confirm gives it nothing extra.
    app.plugins.jobs.push(mock_job(&app));
    assert_eq!(app.export_answer("mock"), None);
    app.plugins.jobs[0].consented = true;
    assert_eq!(app.export_answer("mock"), Some(true));
    assert!(plugin_request(&mut app, "document/export", json!({})).is_ok());
    app.plugins.jobs.clear();
    // An answer holds until the plugin stops.
    app.plugins.export_answers.insert("mock".into(), false);
    assert_eq!(app.export_answer("mock"), Some(false));
    app.stop_plugin("mock");
    assert_eq!(app.export_answer("mock"), None);

    // "Don't ask again" is stored in the grant and saved.
    app.plugins.consent = Some(ConsentRequest {
        plugin: "mock".into(),
        action: None,
        items: Vec::new(),
        dont_ask: true,
    });
    app.dialog = Some(Dialog::PluginConsent);
    app.answer_consent(true);
    assert_eq!(app.dialog, None);
    assert!(send_without_asking(&app));
    assert!(!app.sends_need_consent("mock"));
    assert_eq!(app.export_answer("mock"), Some(true));
    let saved = xuan::config::Config::load(app.config_path.as_ref().unwrap()).unwrap();
    assert!(
        saved.plugins["mock"]
            .grant
            .as_ref()
            .unwrap()
            .send_without_asking
    );
    // Allowing the same plugin again keeps it; "Ask Again" forgets it.
    app.grant_plugin("mock", true);
    assert!(send_without_asking(&app));
    app.ask_before_sending_again("mock");
    assert!(!send_without_asking(&app));
    assert_eq!(app.export_answer("mock"), None);

    // A grant change resets it: the plugin is reviewed and asks again.
    app.config
        .plugins
        .get_mut("mock")
        .unwrap()
        .grant
        .as_mut()
        .unwrap()
        .send_without_asking = true;
    let wider = Manifest::parse(
        &network_manifest("\"example.com\", \"upload.example.com\""),
        dir.path(),
    )
    .unwrap();
    app.install_plugins(vec![wider], vec![]);
    assert!(!app.plugin_granted("mock"));
    assert!(app.sends_need_consent("mock"));
    app.grant_plugin("mock", true);
    assert!(!send_without_asking(&app));
    assert!(app.sends_need_consent("mock"));

    // A grant saved before the answer existed still allows the plugin.
    app.save_config();
    let text = std::fs::read_to_string(app.config_path.as_ref().unwrap()).unwrap();
    assert!(!text.contains("send_without_asking"), "{text}");
    app.config = toml::from_str(&text.replace('\n', "\r\n")).unwrap();
    assert!(app.plugin_granted("mock"));
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

#[test]
fn grants_cover_the_reviewed_folder_command_and_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    let secrets = dir.path().join("secrets.toml");
    app.plugins.secrets_path = Some(secrets.clone());
    let with = |extra: &str, folder: &Path| {
        Manifest::parse(
            &MANIFEST.replace(
                "[[actions]]",
                &format!("{extra}\n\n[[settings]]\nid = \"key\"\ntype = \"secret\"\nlabel = \"Key\"\n\n[[actions]]"),
            ),
            folder,
        )
        .unwrap()
    };
    let reviewed = with("[permissions]\nsecrets = [\"key\"]", dir.path());
    app.install_plugins(vec![reviewed.clone()], vec![]);
    assert!(!app.plugin_granted("mock"));
    app.grant_plugin("mock", true);
    assert!(app.plugin_granted("mock"));
    app.plugins.secrets.set("mock", "key", "sk-123");

    // An update that asks for more needs another review.
    let wider = with(
        "[permissions]\nsecrets = [\"key\"]\nnetwork = [\"evil.example\"]",
        dir.path(),
    );
    app.install_plugins(vec![wider.clone()], vec![]);
    assert!(!app.plugin_granted("mock"));
    app.start_plugin_action("mock", "echo");
    assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
    app.dialog = None;
    app.grant_plugin("mock", true);
    assert!(app.plugin_granted("mock"));
    assert_eq!(app.plugins.secrets.get("mock", "key"), Some("sk-123"));
    // So does a changed command.
    let mut command = wider.clone();
    command.plugin.command = vec!["sh".into(), "other.sh".into()];
    app.install_plugins(vec![command], vec![]);
    assert!(!app.plugin_granted("mock"));

    // The same id from another folder gets neither the grant nor the secrets.
    let impostor = with(
        "[permissions]\nsecrets = [\"key\"]\nnetwork = [\"evil.example\"]",
        shadow.path(),
    );
    app.install_plugins(vec![impostor], vec![]);
    assert!(!app.plugin_granted("mock"));
    // Allowing it forgets the secrets entered for the other folder.
    app.grant_plugin("mock", true);
    assert_eq!(app.plugins.secrets.get("mock", "key"), None);
    let (_, secret_values) = app.plugin_settings(app.plugins.manifest("mock").unwrap());
    assert!(secret_values.as_object().unwrap().is_empty());
    assert!(
        !std::fs::read_to_string(&secrets)
            .unwrap()
            .contains("sk-123")
    );

    // Grants from before they were recorded must be reviewed again.
    let old: xuan::config::Config =
        toml::from_str("[plugins.mock]\nenabled = true\ngranted = true\n").unwrap();
    app.config = old;
    app.install_plugins(vec![reviewed], vec![]);
    assert!(!app.plugin_granted("mock"));
}

#[test]
fn plugin_data_without_a_config_folder_is_private_and_unpredictable() {
    let (_context, mut editor) = app();
    let first = editor.plugins.data_dir("mock").unwrap();
    assert!(first.is_dir());
    assert_eq!(editor.plugins.data_dir("mock").unwrap(), first);
    assert!(!first.starts_with(std::env::temp_dir().join("xuan-plugin-data")));
    // Another session gets another folder.
    let (_context, mut other) = app();
    assert_ne!(other.plugins.data_dir("mock").unwrap(), first);
    assert_ne!(editor.plugins.data_dir("other").unwrap(), first);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&first), 0o700);
        // So are the folders the host exchanges files with plugins in.
        let job = xuan::plugins::private_dir("xuan-job-").unwrap();
        assert_eq!(mode(job.path()), 0o700);
    }
    // With a configuration folder it persists next to the configuration.
    let config = tempfile::tempdir().unwrap();
    editor.plugins.config_dir = Some(config.path().to_path_buf());
    assert_eq!(
        editor.plugins.data_dir("mock").unwrap(),
        config.path().join("plugin-data/mock")
    );
    assert!(config.path().join("plugin-data/mock").is_dir());
}

#[test]
fn a_secrets_file_that_fails_to_load_is_never_overwritten_or_quoted() {
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    app.config_path = Some(dir.path().join("config.toml"));
    let secrets = dir.path().join("secrets.toml");
    let broken = "[mock]\nkey = \"sk-live-123\nother = \"sk-live-456\"\n";
    std::fs::write(&secrets, broken).unwrap();
    app.load_plugins();
    let error = app.error.take().unwrap();
    assert!(error.contains("Could not load plugin secrets"), "{error}");
    assert!(!error.contains("sk-live"), "{error}");

    let manifest = Manifest::parse(
        &MANIFEST.replace(
            "[[actions]]",
            "[permissions]\nsecrets = [\"key\"]\n\n[[settings]]\nid = \"key\"\ntype = \"secret\"\n\n[[actions]]",
        ),
        dir.path(),
    )
    .unwrap();
    app.install_plugins(vec![manifest.clone()], vec![]);
    app.set_plugin_setting("mock", "key", serde_json::Value::String("sk-new".into()));
    let error = app.error.take().unwrap();
    assert!(error.contains("left unchanged"), "{error}");
    assert_eq!(std::fs::read_to_string(&secrets).unwrap(), broken);

    // Once the file is fixed and reloaded, secrets are saved again.
    std::fs::write(&secrets, "[mock]\nkey = \"sk-old\"\n").unwrap();
    app.load_plugins();
    assert!(app.error.is_none(), "{:?}", app.error);
    app.install_plugins(vec![manifest], vec![]);
    app.set_plugin_setting("mock", "key", serde_json::Value::String("sk-new".into()));
    assert!(app.error.is_none(), "{:?}", app.error);
    assert!(
        std::fs::read_to_string(&secrets)
            .unwrap()
            .contains("sk-new")
    );
}

/// A finished job of the mock plugin, for feeding results straight in.
fn mock_job(app: &EditorApp) -> crate::app::plugins::PluginJob {
    crate::app::plugins::PluginJob {
        id: uuid::Uuid::new_v4(),
        plugin: "mock".into(),
        action: "echo".into(),
        label: "Echo".into(),
        document: app.session().unwrap().document.id,
        _work_dir: xuan::plugins::private_dir("xuan-job-").unwrap(),
        prepared: xuan::plugins::jobs::Prepared::none(),
        regions: Vec::new(),
        inputs: serde_json::json!({}),
        into: xuan::plugins::manifest::ResultInto::Layer,
        mask_to_regions: false,
        progress: None,
        message: String::new(),
        cancelled: false,
        consented: false,
    }
}

#[test]
fn oversized_results_are_refused_before_their_images_are_read() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    let job = mock_job(&app);
    let fixture = dir.path().join("fixture.png");
    let sessions = app.sessions.len();
    let layers = app.session().unwrap().document.layers.len();
    let error = |app: &mut EditorApp, result| {
        format!("{:#}", app.apply_job_result(&job, result).unwrap_err())
    };

    // Too many outputs: refused without touching the (missing) files.
    let outputs: Vec<_> = (0..=xuan::plugins::edits::MAX_OUTPUTS)
        .map(|_| json!({"kind": "image", "path": dir.path().join("missing.png")}))
        .collect();
    let message = error(&mut app, json!({ "outputs": outputs }));
    assert!(message.contains("outputs"), "{message}");
    // Too many layers, even as new documents.
    let outputs: Vec<_> = (0..=xuan::plugins::edits::MAX_LAYERS)
        .map(|_| json!({"kind": "document", "path": fixture}))
        .collect();
    let message = error(&mut app, json!({ "outputs": outputs }));
    assert!(message.contains("layers"), "{message}");
    // Too many edits across batches.
    let select = json!({"op": "select", "layer": app.session().unwrap().document.layers[0].id});
    let batch = json!({"kind": "edit", "edits": vec![select; 600]});
    let message = error(&mut app, json!({ "outputs": [batch.clone(), batch] }));
    assert!(message.contains("edits"), "{message}");
    assert_eq!(app.sessions.len(), sessions);
    assert_eq!(app.session().unwrap().document.layers.len(), layers);
    assert!(app.plugins.proposal.is_none());
    // A result within the limits still works.
    app.apply_job_result(
        &job,
        json!({"outputs": [{"kind": "image", "path": fixture}]}),
    )
    .unwrap();
    assert!(app.plugins.proposal.is_some());
}

#[test]
fn image_outputs_may_declare_their_placed_size() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    let job = mock_job(&app);
    let fixture = dir.path().join("fixture.png");
    for (extra, ok) in [
        (json!({"width": 0}), false),
        (json!({"height": -3.0}), false),
        (json!({"width": 1e9}), false),
        (json!({"fit": "bogus"}), false),
        (json!({"fit": "source"}), false),
        (json!({"fit": "source", "width": 4}), false),
        (json!({"width": 12.0, "height": 6.0}), true),
    ] {
        let mut output = json!({"kind": "image", "path": fixture});
        output
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let result = app.apply_job_result(&job, json!({"outputs": [output]}));
        assert_eq!(result.is_ok(), ok, "{extra}: {result:?}");
    }
}

/// A grey mask PNG in `dir`, one of the mock plugin's folders.
fn write_mask(
    dir: &Path,
    name: &str,
    (width, height): (u32, u32),
    value: impl Fn(u32, u32) -> u8,
) -> std::path::PathBuf {
    let path = dir.join(name);
    image::GrayImage::from_fn(width, height, |x, y| image::Luma([value(x, y)]))
        .save(&path)
        .unwrap();
    path
}

/// A document-sized selection, selected where `inside` holds.
fn selection_where(app: &EditorApp, inside: impl Fn(u32, u32) -> bool) -> Arc<image::GrayImage> {
    let document = &app.session().unwrap().document;
    Arc::new(image::GrayImage::from_fn(
        document.width,
        document.height,
        |x, y| image::Luma([if inside(x, y) { 255 } else { 0 }]),
    ))
}

fn selection_at(app: &EditorApp, x: u32, y: u32) -> Option<u8> {
    let selection = app.session().unwrap().document.selection.as_ref()?;
    Some(selection.get_pixel(x, y)[0])
}

#[test]
fn mask_outputs_combine_with_the_selection_in_every_mode() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    app.command("fill_fg");
    let job = mock_job(&app);
    let pixels = app.session().unwrap().document.layers[0].pixels.clone();
    // Selected: the left half. The mask: the top half, and a half-selected
    // bottom-right quarter.
    let left = selection_where(&app, |x, _| x < 4);
    let mask = write_mask(dir.path(), "mask.png", (8, 8), |x, y| {
        match (x < 4, y < 4) {
            (_, true) => 255,
            (false, false) => 128,
            (true, false) => 0,
        }
    });
    // Pixels: both, mask only, selection only, the grey quarter.
    let points = [(1, 1), (6, 1), (1, 6), (6, 6)];
    for (mode, expected) in [
        ("replace", [255, 255, 0, 128]),
        ("add", [255, 255, 255, 128]),
        ("subtract", [0, 0, 255, 0]),
        ("intersect", [255, 0, 0, 0]),
    ] {
        app.session_mut().unwrap().document.selection = Some(left.clone());
        app.session_mut().unwrap().history.commit();
        let result = json!({"outputs": [{"kind": "mask", "path": mask, "mode": mode}]});
        app.apply_job_result(&job, result).unwrap();
        assert_eq!(app.dialog, Some(Dialog::PluginProposal));
        // The proposed selection shows before it is accepted.
        let shown = points.map(|(x, y)| selection_at(&app, x, y).unwrap());
        assert_eq!(shown, expected, "{mode}");
        app.resolve_proposal(true);
        let accepted = points.map(|(x, y)| selection_at(&app, x, y).unwrap());
        assert_eq!(accepted, expected, "{mode}");
        let session = app.session().unwrap();
        assert_eq!(session.history.undo_name(), Some("Echo"));
        // Only the selection changed: no layers, no pixels.
        assert_eq!(session.document.layers.len(), 1);
        assert_eq!(session.document.layers[0].pixels, pixels);
        // One undo step brings the previous selection back.
        app.command("undo");
        assert_eq!(
            app.session().unwrap().document.selection,
            Some(left.clone())
        );
    }
    // Without a mode it replaces, and with no selection every mode starts from nothing.
    app.session_mut().unwrap().document.selection = None;
    app.apply_job_result(&job, json!({"outputs": [{"kind": "mask", "path": mask}]}))
        .unwrap();
    assert_eq!(selection_at(&app, 1, 1), Some(255));
    app.resolve_proposal(true);
    // Several masks apply in order.
    let result = json!({"outputs": [
        {"kind": "mask", "path": mask, "mode": "replace"},
        {"kind": "mask", "path": mask, "mode": "subtract"},
    ]});
    app.apply_job_result(&job, result).unwrap();
    assert_eq!(selection_at(&app, 1, 1), Some(0));
    app.resolve_proposal(true);
}

#[test]
fn a_discarded_mask_leaves_the_selection_and_compare_shows_the_old_one() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    let job = mock_job(&app);
    let left = selection_where(&app, |x, _| x < 4);
    app.session_mut().unwrap().document.selection = Some(left.clone());
    app.session_mut().unwrap().history.commit();
    let steps = app.session().unwrap().history.names().count();
    let mask = write_mask(
        dir.path(),
        "mask.png",
        (8, 8),
        |_, y| if y < 4 { 255 } else { 0 },
    );
    let result = json!({"outputs": [{"kind": "mask", "path": mask}]});

    app.apply_job_result(&job, result.clone()).unwrap();
    assert_eq!(selection_at(&app, 6, 1), Some(255));
    app.resolve_proposal(false);
    assert_eq!(
        app.session().unwrap().document.selection,
        Some(left.clone())
    );
    assert_eq!(app.session().unwrap().history.names().count(), steps);
    assert!(app.dialog.is_none());

    // Compare swaps the old selection back in; accepting while comparing
    // still applies the new one.
    app.apply_job_result(&job, result).unwrap();
    app.toggle_proposal_compare();
    assert_eq!(
        app.session().unwrap().document.selection,
        Some(left.clone())
    );
    app.toggle_proposal_compare();
    assert_eq!(selection_at(&app, 6, 1), Some(255));
    app.toggle_proposal_compare();
    app.resolve_proposal(true);
    assert_eq!(selection_at(&app, 6, 1), Some(255));
    assert_eq!(selection_at(&app, 1, 6), Some(0));
    assert_eq!(app.session().unwrap().history.names().count(), steps + 1);
}

#[test]
fn a_mask_fitted_to_the_source_covers_the_source_layer() {
    use serde_json::json;
    use xuan::plugins::{
        jobs,
        manifest::{Source, SourceKind},
    };
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    // The layer's 16x16 pixels are shown at half size from (4, 4).
    let transform = &mut app.session_mut().unwrap().document.layers[0].transform;
    (transform.x, transform.y, transform.width, transform.height) = (4.0, 4.0, 8.0, 8.0);
    let mut job = mock_job(&app);
    let source = Source {
        from: SourceKind::Layer,
        max_side: Some(16),
        crop_to_regions: false,
        padding: 0.0,
    };
    job.prepared = jobs::prepare(
        &app.session().unwrap().document,
        &source,
        &[],
        job._work_dir.path(),
    )
    .unwrap();
    // A 2x2 mask, much smaller than the 16x16 source that was sent.
    let mask = write_mask(dir.path(), "small.png", (2, 2), |_, _| 255);
    let result = json!({"outputs": [{"kind": "mask", "path": mask, "fit": "source"}]});
    app.apply_job_result(&job, result).unwrap();
    for (x, y, expected) in [
        (3, 3, 0),
        (4, 4, 255),
        (11, 11, 255),
        (12, 12, 0),
        (8, 2, 0),
    ] {
        assert_eq!(selection_at(&app, x, y), Some(expected), "({x}, {y})");
    }
    app.resolve_proposal(true);
    // At its pixel size it covers two source pixels: one document unit.
    let result = json!({"outputs": [{"kind": "mask", "path": mask, "x": 8, "y": 8}]});
    app.apply_job_result(&job, result).unwrap();
    assert_eq!(selection_at(&app, 8, 8), Some(255));
    assert_eq!(selection_at(&app, 9, 9), Some(0));
    assert_eq!(selection_at(&app, 4, 4), Some(0));
    app.resolve_proposal(false);
}

#[test]
fn a_read_only_plugin_can_propose_a_selection_but_not_edit_pixels() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    assert_eq!(
        app.plugins.manifest("mock").unwrap().permissions.document,
        xuan::plugins::manifest::DocumentAccess::Read
    );
    app.dimensions = [8, 8];
    app.new_document();
    app.command("fill_fg");
    let pixels = app.session().unwrap().document.layers[0].pixels.clone();
    let job = mock_job(&app);
    let mask = write_mask(
        dir.path(),
        "mask.png",
        (8, 8),
        |x, _| if x < 2 { 255 } else { 0 },
    );
    app.apply_job_result(&job, json!({"outputs": [{"kind": "mask", "path": mask}]}))
        .unwrap();
    app.resolve_proposal(true);
    assert_eq!(selection_at(&app, 0, 0), Some(255));
    assert_eq!(selection_at(&app, 5, 0), Some(0));
    // Editing the document, even the selection, still needs `document = "edit"`.
    let steps = app.session().unwrap().history.names().count();
    for edits in [
        json!([{"op": "set_selection", "mask": mask}]),
        json!([{"op": "replace_pixels", "layer": app.session().unwrap().document.layers[0].id, "image": mask}]),
    ] {
        let error = plugin_request(
            &mut app,
            "document/edit",
            json!({"name": "x", "edits": edits}),
        )
        .unwrap_err();
        assert!(
            error.message.contains("document = \"edit\""),
            "{}",
            error.message
        );
    }
    let error = plugin_request(&mut app, "host/run", json!({"action": "invert"})).unwrap_err();
    assert!(
        error.message.contains("document = \"edit\""),
        "{}",
        error.message
    );
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), steps);
    assert_eq!(session.document.layers[0].pixels, pixels);
}

#[test]
fn a_proposed_selection_is_accepted_or_discarded_through_the_ui() {
    use crate::app::tests::ui::UiTest;
    use egui::accesskit::Role;
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(config.path());
    install_mock(ui.app_mut(), dir.path());
    ui.app_mut()
        .config
        .panes
        .set_hidden("plugin:mock/info", true);
    ui.settle();
    let job = mock_job(ui.app());
    let mask = write_mask(dir.path(), "mask.png", (20, 16), |x, _| {
        if x < 10 { 255 } else { 0 }
    });
    let result = json!({"outputs": [{"kind": "mask", "path": mask}]});
    let steps = ui.app().session().unwrap().history.names().count();

    ui.app_mut().apply_job_result(&job, result.clone()).unwrap();
    ui.settle();
    assert!(ui.has_role(Role::Button, "Accept"));
    assert!(ui.has_role(Role::Button, "Discard"));
    // The marching ants already show the proposed selection.
    assert_eq!(selection_at(ui.app(), 2, 2), Some(255));
    ui.click_role(Role::Button, "Discard");
    assert!(ui.app().plugins.proposal.is_none());
    assert_eq!(ui.app().session().unwrap().document.selection, None);
    assert_eq!(ui.app().session().unwrap().history.names().count(), steps);

    ui.app_mut().apply_job_result(&job, result).unwrap();
    ui.settle();
    ui.click_role(Role::Button, "Accept");
    assert!(ui.app().plugins.proposal.is_none());
    assert_eq!(selection_at(ui.app(), 2, 2), Some(255));
    assert_eq!(selection_at(ui.app(), 15, 2), Some(0));
    let history = &ui.app().session().unwrap().history;
    assert_eq!(history.undo_name(), Some("Echo"));
    assert_eq!(history.names().count(), steps + 1);
}

#[test]
fn invalid_and_oversized_masks_are_refused() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    let job = mock_job(&app);
    let left = selection_where(&app, |x, _| x < 4);
    app.session_mut().unwrap().document.selection = Some(left.clone());
    app.session_mut().unwrap().history.commit();
    let mask = write_mask(dir.path(), "mask.png", (8, 8), |_, _| 255);
    // Wider than any image Xuan opens: refused from its header.
    let wide = write_mask(dir.path(), "wide.png", (30_001, 1), |_, _| 255);
    let foreign = write_mask(outside.path(), "foreign.png", (8, 8), |_, _| 255);
    let text = dir.path().join("not-a.png");
    std::fs::write(&text, "not a png\r\n").unwrap();
    let many: Vec<_> = (0..=xuan::plugins::edits::MAX_OUTPUTS)
        .map(|_| json!({"kind": "mask", "path": mask}))
        .collect();
    for (output, message) in [
        (json!({"kind": "mask", "path": wide}), "exceeds limit"),
        (json!({"kind": "mask", "path": foreign}), "outside"),
        (json!({"kind": "mask", "path": text}), "decode"),
        (
            json!({"kind": "mask", "path": dir.path().join("missing.png")}),
            "Cannot read",
        ),
        (
            json!({"kind": "mask", "path": mask, "mode": "xor"}),
            "malformed",
        ),
        (json!({"kind": "mask"}), "malformed"),
        (
            json!({"kind": "mask", "path": mask, "fit": "source"}),
            "source",
        ),
        (json!({"kind": "mask", "path": mask, "width": 0}), "width"),
        (json!({"kind": "mask", "path": mask, "x": 1e9}), "fit"),
        (json!(many), "outputs"),
    ] {
        let outputs = if output.is_array() {
            output
        } else {
            json!([output])
        };
        let error = app
            .apply_job_result(&job, json!({ "outputs": outputs }))
            .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(message), "{message}: {error}");
        assert!(app.plugins.proposal.is_none());
        assert_eq!(
            app.session().unwrap().document.selection,
            Some(left.clone())
        );
    }
}

fn plugin_request(
    app: &mut EditorApp,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, xuan::plugins::protocol::RpcError> {
    let request = xuan::plugins::protocol::Request {
        jsonrpc: "2.0".into(),
        id: xuan::plugins::protocol::Id::Number(1),
        method: method.into(),
        params,
    };
    app.service_request("mock", &request)
}

#[test]
fn host_side_file_access_stays_in_the_plugins_folders() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    let foreign = outside.path().join("foreign.png");
    RgbaImage::from_pixel(4, 4, image::Rgba([9, 9, 9, 255]))
        .save(&foreign)
        .unwrap();
    let install = |app: &mut EditorApp, filesystem: &str| {
        let text = MANIFEST.replace(
            "[[actions]]",
            &format!(
                "[permissions]\ndocument = \"edit\"\nfilesystem = \"{filesystem}\"\n\n[[actions]]"
            ),
        );
        app.install_plugins(vec![Manifest::parse(&text, dir.path()).unwrap()], vec![]);
    };
    install(&mut app, "none");
    app.dimensions = [8, 8];
    app.new_document();
    let layers = |app: &EditorApp| app.session().unwrap().document.layers.len();

    // Exports go to the scratch folder or one of the plugin's own folders.
    let export = plugin_request(&mut app, "document/export", json!({})).unwrap();
    assert!(Path::new(export["path"].as_str().unwrap()).is_file());
    let error =
        plugin_request(&mut app, "document/export", json!({"dir": outside.path()})).unwrap_err();
    assert!(error.message.contains("outside"), "{}", error.message);
    let export = plugin_request(&mut app, "document/export", json!({"dir": dir.path()})).unwrap();
    assert!(
        Path::new(export["path"].as_str().unwrap()).starts_with(dir.path().canonicalize().unwrap())
    );
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);

    // Edits read images only from those folders.
    let add = |image: &Path| json!({"name": "x", "edits": [{"op": "add_layer", "image": image}]});
    let error = plugin_request(&mut app, "document/edit", add(&foreign)).unwrap_err();
    assert!(error.message.contains("outside"), "{}", error.message);
    assert_eq!(layers(&app), 1);
    plugin_request(
        &mut app,
        "document/edit",
        add(Path::new(export["path"].as_str().unwrap())),
    )
    .unwrap();
    assert_eq!(layers(&app), 2);
    // So do job results and host/open.
    let job = mock_job(&app);
    let result = json!({"outputs": [{"kind": "image", "path": foreign}]});
    let error = format!(
        "{:#}",
        app.apply_job_result(&job, result.clone()).unwrap_err()
    );
    assert!(error.contains("outside"), "{error}");
    let error = plugin_request(&mut app, "host/open", json!({"path": foreign})).unwrap_err();
    assert!(error.message.contains("outside"), "{}", error.message);
    assert_eq!(app.sessions.len(), 1);

    // `filesystem = "read"` lets the host read anywhere, but not write.
    install(&mut app, "read");
    plugin_request(&mut app, "document/edit", add(&foreign)).unwrap();
    assert_eq!(layers(&app), 3);
    app.apply_job_result(&job, result).unwrap();
    app.resolve_proposal(false);
    assert!(plugin_request(&mut app, "document/export", json!({"dir": outside.path()})).is_err());
    // `filesystem = "write"` lets it write anywhere too.
    install(&mut app, "write");
    let export =
        plugin_request(&mut app, "document/export", json!({"dir": outside.path()})).unwrap();
    assert!(
        Path::new(export["path"].as_str().unwrap())
            .starts_with(outside.path().canonicalize().unwrap())
    );
}

#[test]
fn pane_updates_only_reach_declared_panes() {
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    let update = |app: &mut EditorApp, pane: &str| {
        app.handle_notification(
            "mock",
            xuan::plugins::protocol::Notification {
                jsonrpc: "2.0".into(),
                method: "pane/update".into(),
                params: serde_json::json!({"pane": pane, "tree": {"type": "label", "text": "hi"}}),
            },
        )
    };
    for index in 0..100 {
        update(&mut app, &format!("made-up-{index}"));
    }
    assert!(app.plugins.panes.is_empty());
    update(&mut app, "info");
    assert!(app.plugins.panes["plugin:mock/info"].tree.is_some());
    assert_eq!(app.plugins.panes.len(), 1);
}

#[test]
fn rerun_inputs_from_a_project_are_checked_against_the_action() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    let fixture = dir.path().join("fixture.png");
    RgbaImage::new(2, 2).save(&fixture).unwrap();
    std::fs::write(
        dir.path().join("plugin.toml"),
        MANIFEST.replace("type = \"regions\"\n", "type = \"regions\"\nmax = 2\n"),
    )
    .unwrap();
    app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
    app.grant_plugin("mock", true);
    app.dimensions = [64, 64];
    app.new_document();
    app.command("fill_fg");
    // A hostile project stores wrong types, broken numbers and too many regions.
    let region = |x: serde_json::Value| json!({"x": x, "y": 1, "width": 5, "height": 5, "fields": {"desc": 7, "evil": "x"}});
    let stored = json!({
        "prompt": {"not": "text"},
        "regions": [region(json!(1)), region(json!("NaN")), region(json!(2)), region(json!(3))],
        "unknown": "x",
    });
    app.start_plugin_action_with("mock", "echo", Some(&stored));
    let edit = app.plugins.action.as_ref().unwrap();
    assert_eq!(edit.values["prompt"], "hello");
    assert!(!edit.values.contains_key("unknown"));
    assert_eq!(edit.regions.len(), 2);
    assert_eq!(edit.regions[1].x, 2.0);
    assert_eq!(edit.regions[0].fields["desc"], "");
    assert!(!edit.regions[0].fields.contains_key("evil"));
    // The region limit holds for drawn regions and the selection too.
    app.add_region(Point::new(10.0, 10.0), Point::new(20.0, 20.0));
    app.command("select_all");
    app.add_selection_region();
    assert_eq!(app.plugins.action.as_ref().unwrap().regions.len(), 2);
    assert_eq!(app.status, "No more regions can be added");
    app.plugins
        .action
        .as_mut()
        .unwrap()
        .regions
        .push(xuan::plugins::jobs::Region::rect(0.0, 0.0, 4.0, 4.0));
    app.run_plugin_action();
    assert!(app.error.take().is_some_and(|e| e.contains("at most")));
    assert!(app.plugins.jobs.is_empty());
}

#[test]
fn plugin_messages_and_dialogs_name_the_plugin_and_its_id() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    // A plugin that calls itself Xuan.
    std::fs::write(
        dir.path().join("plugin.toml"),
        MANIFEST.replace("name = \"Mock\"", "name = \"Xuan\""),
    )
    .unwrap();
    app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
    app.handle_notification(
        "mock",
        xuan::plugins::protocol::Notification {
            jsonrpc: "2.0".into(),
            method: "host/status".into(),
            params: serde_json::json!({"message": "Saved\nproject.xuan"}),
        },
    );
    assert_eq!(app.status, "Xuan (plugin mock): Saved project.xuan");
    // The registry, and so the menus and shortcut list, mark plugin actions.
    let entry = app.keymap.get("mock/echo").unwrap();
    assert_eq!(entry.label(), "Echo Source… · Xuan");
    let items = app.plugin_menu_items();
    let item = &items[&xuan::plugins::manifest::Menu::Filter][0];
    assert_eq!(item.label, "Echo Source… · Xuan");
    assert!(item.source.contains("(plugin mock)"));
    assert!(item.source.contains(&dir.path().display().to_string()));
    // Errors and the permission dialog say which plugin, by id and folder.
    app.dimensions = [8, 8];
    app.new_document();
    app.start_plugin_action("mock", "echo");
    assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
    frame(&context, &mut app);
    let texts: Vec<String> = frame(&context, &mut app)
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("Run Xuan (plugin mock)?")),
        "{texts:?}"
    );
    let folder = std::fs::canonicalize(dir.path())
        .unwrap()
        .display()
        .to_string();
    assert!(texts.iter().any(|t| t.contains(&folder)), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t == "Xuan (plugin mock) needs your permission to run."),
        "{texts:?}"
    );
    app.dialog = None;
    let error = xuan::plugins::protocol::RpcError::new(-32000, "Out of credits");
    assert_eq!(
        app.describe_rpc_error("mock", &error),
        "Xuan (plugin mock): Out of credits"
    );
}

#[test]
fn shortcuts_match_their_modifiers_exactly() {
    use super::shortcuts::consume_exact;
    use egui::{Key, Modifiers};
    let keymap = super::commands::Keymap::default();
    let builtin_for = |mods: Modifiers, key: Key| {
        keymap
            .lookup(mods, key, false)
            .map(|entry| entry.id.as_str())
    };
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
    assert_eq!(
        builtin_for(Modifiers::CTRL, Key::H),
        Some("toggle_controls")
    );
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

/// Tests that run the mock plugin, a POSIX shell script.
#[cfg(unix)]
mod unix {
    use super::*;
    use std::time::{Duration, Instant};

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
        // Text from the plugin is marked as the plugin's.
        assert_eq!(app.status, "Mock (plugin mock): prompt=hello");
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

    #[test]
    fn plugin_formats_import_documents_and_layers() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_mock(&mut app, dir.path());
        assert_eq!(app.plugin_import_extensions(), ["foo"]);
        let file = dir.path().join("picture.foo");
        std::fs::write(&file, b"").unwrap();
        // The import runs in the background and opens the file when done.
        app.open_path(&file, false);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.plugins.formats.len(), 1);
        run_until(&context, &mut app, |app| app.plugins.formats.is_empty());
        assert!(app.error.is_none(), "{:?}", app.error);
        let session = app.session().unwrap();
        assert_eq!(session.title, "picture");
        assert_eq!((session.document.width, session.document.height), (8, 8));
        assert_eq!(session.document.layers[0].name, "Imported");
        assert!(session.history.dirty());
        app.open_path(&file, true);
        run_until(&context, &mut app, |app| app.plugins.formats.is_empty());
        assert_eq!(app.session().unwrap().document.layers.len(), 2);
        assert_eq!(app.session().unwrap().history.names().count(), 1);
        // Built-in formats are never handed to plugins.
        assert!(app.plugin_import_format(Path::new("x.FOO")).is_some());
        assert!(app.plugin_import_format(Path::new("x.png")).is_none());
        assert!(builtin_extension(Path::new("x.PNG")));
        frame(&context, &mut app);
    }

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

    #[test]
    fn a_plugin_that_crashes_during_an_import_is_cleaned_up() {
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

        // The plugin dies during format/import.
        let file = dir.path().join("picture.foo");
        std::fs::write(&file, b"").unwrap();
        app.open_path(&file, false);
        run_until(&context, &mut app, |app| app.plugins.formats.is_empty());
        assert!(
            app.error
                .as_deref()
                .is_some_and(|e| e.contains("stopped unexpectedly") && e.contains("picture.foo")),
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

    #[test]
    fn slow_plugins_start_and_handle_formats_without_blocking_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_mock(&mut app, dir.path());
        // The plugin takes a while before it reads `initialize`.
        let fixture = dir.path().join("fixture.png");
        std::fs::write(
            dir.path().join("plugin.sh"),
            format!("sleep 1\n{}", script(&fixture)),
        )
        .unwrap();
        app.dimensions = [16, 16];
        app.new_document();
        let key = "plugin:mock/info";
        let started = Instant::now();
        app.render_pane(key, "open", None);
        let file = dir.path().join("picture.foo");
        std::fs::write(&file, b"").unwrap();
        app.open_path(&file, false);
        assert!(
            started.elapsed() < Duration::from_millis(700),
            "{:?}",
            started.elapsed()
        );
        assert!(app.plugins.starting("mock"));
        assert!(app.error.is_none(), "{:?}", app.error);
        // What was asked for meanwhile is sent once it has started.
        run_until(&context, &mut app, |app| {
            app.plugins.formats.is_empty() && app.plugins.panes[key].tree.is_some()
        });
        assert!(!app.plugins.starting("mock"));
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.session().unwrap().title, "picture");

        // Exports run in the background too.
        let out = dir.path().join("out.foo");
        app.start_plugin_export("mock", "foo", &out).unwrap();
        assert_eq!(app.plugins.formats.len(), 1);
        run_until(&context, &mut app, |app| app.plugins.formats.is_empty());
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(app.status.contains("out.foo"), "{}", app.status);
        assert_eq!(xuan::io::import_image(&out).unwrap().dimensions(), (8, 8));

        // A pending export can be cancelled; the late answer is ignored.
        std::fs::remove_file(&out).unwrap();
        app.start_plugin_export("mock", "foo", &out).unwrap();
        let id = app.plugins.formats[0].id;
        app.cancel_format_job(id);
        assert!(app.plugins.formats.is_empty());
        for _ in 0..20 {
            frame(&context, &mut app);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.error.is_none(), "{:?}", app.error);
    }

    #[test]
    fn a_plugin_that_fails_to_start_is_reported_with_its_log() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_mock(&mut app, dir.path());
        std::fs::write(dir.path().join("plugin.sh"), "echo boom >&2\nexit 3\n").unwrap();
        app.dimensions = [16, 16];
        app.new_document();
        let key = "plugin:mock/info";
        app.render_pane(key, "open", None);
        let file = dir.path().join("picture.foo");
        std::fs::write(&file, b"").unwrap();
        app.open_path(&file, false);
        run_until(&context, &mut app, |app| app.plugins.formats.is_empty());
        let error = app.error.clone().unwrap_or_default();
        assert!(error.contains("did not start"), "{error}");
        let pane = app.plugins.panes[key].error.clone().unwrap_or_default();
        assert!(
            pane.contains("did not start") && pane.contains("boom"),
            "{pane}"
        );
        assert!(!app.plugins.running("mock"));
        assert!(!app.plugins.starting("mock"));
    }

    fn echoed(app: &EditorApp, index: usize) -> usize {
        app.sessions[index]
            .document
            .layers
            .iter()
            .filter(|l| l.name == "Echoed")
            .count()
    }

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

    fn click(context: &egui::Context, app: &mut EditorApp, pos: Pos2) {
        pointer_frame(context, app, pos, None, egui::Modifiers::NONE);
        pointer_frame(context, app, pos, Some(true), egui::Modifiers::NONE);
        pointer_frame(context, app, pos, Some(false), egui::Modifiers::NONE);
    }

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
            crate::app::plugins::PendingStart::Action("echo".into()),
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

    #[test]
    fn plugin_menus_panes_and_chords_work_through_the_ui() {
        use crate::app::tests::ui::UiTest;
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
        // Plugin items name their plugin, so they never pass for Xuan's own.
        assert!(ui.has("Echo Source… · Mock Ctrl+Shift+E"));
        ui.key(egui::Key::Escape);
        let steps = ui.app().session().unwrap().history.names().count();
        ui.press(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::E);
        assert!(ui.app().plugins.action.is_some(), "{:?}", ui.app().error);
        assert_eq!(ui.app().session().unwrap().history.names().count(), steps);
    }

    fn host_run(
        app: &mut EditorApp,
        plugin: &str,
        action: &str,
    ) -> Result<serde_json::Value, xuan::plugins::protocol::RpcError> {
        let request = xuan::plugins::protocol::Request {
            jsonrpc: "2.0".into(),
            id: xuan::plugins::protocol::Id::Number(1),
            method: "host/run".into(),
            params: serde_json::json!({"action": action, "inputs": {"prompt": "from host/run"}}),
        };
        app.service_request(plugin, &request)
    }

    #[test]
    fn host_run_only_allows_safe_commands_and_the_plugins_own_actions() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_mock(&mut app, dir.path());
        let reader = app.plugins.manifest("mock").unwrap().clone();
        let mut editor = Manifest::parse(
            &MANIFEST
                .replace("id = \"mock\"", "id = \"editor\"")
                .replace("shortcut = \"Ctrl+Shift+E\"\n", "")
                .replace(
                    "[[actions]]",
                    "[permissions]\ndocument = \"edit\"\n\n[[actions]]",
                ),
            other.path(),
        )
        .unwrap();
        editor.dir = dir.path().to_path_buf();
        app.install_plugins(vec![reader, editor], vec![]);
        app.grant_plugin("editor", true);
        let path = dir.path().join("work.xuan");
        app.dimensions = [16, 16];
        app.new_document();
        app.command("new_layer");
        io::save(&app.session().unwrap().document, &path).unwrap();
        app.session_mut().unwrap().path = Some(path.clone());
        frame(&context, &mut app);
        let saved = std::fs::read(&path).unwrap();
        let layers = |app: &EditorApp| app.session().unwrap().document.layers.len();

        // A read-only plugin can move the view but not edit, save or close.
        assert!(host_run(&mut app, "mock", "zoom_in").is_ok());
        for command in ["flatten", "delete_layer"] {
            let error = host_run(&mut app, "mock", command).unwrap_err();
            assert!(
                error.message.contains("edit"),
                "{command}: {}",
                error.message
            );
        }
        for command in [
            "save", "save_as", "export", "close", "quit", "open", "settings", "plugins", "paste",
            "copy",
        ] {
            assert!(host_run(&mut app, "mock", command).is_err(), "{command}");
        }
        assert_eq!(layers(&app), 2);
        assert_eq!(app.sessions.len(), 1);
        assert!(!app.close_app);
        // Nothing reached the file.
        assert_eq!(std::fs::read(&path).unwrap(), saved);

        // A plugin with edit access may make undoable edits, but still not save.
        assert!(host_run(&mut app, "editor", "flatten").is_ok());
        assert_eq!(layers(&app), 1);
        assert!(host_run(&mut app, "editor", "save").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);

        // Another plugin's action is refused; the plugin's own runs with its inputs.
        assert!(host_run(&mut app, "mock", "editor/echo").is_err());
        assert!(app.plugins.action.is_none());
        assert_eq!(app.dialog, None);
        assert!(host_run(&mut app, "mock", "mock/echo").is_ok());
        let edit = app.plugins.action.as_ref().unwrap();
        assert_eq!(
            (edit.plugin.as_str(), edit.action.as_str()),
            ("mock", "echo")
        );
        assert_eq!(edit.values["prompt"], "from host/run");
    }

    #[test]
    fn a_new_plugin_without_permissions_does_not_start_until_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        let fixture = dir.path().join("fixture.png");
        RgbaImage::new(2, 2).save(&fixture).unwrap();
        // The process would leave a marker if it ever started.
        let marker = dir.path().join("started");
        std::fs::write(
            dir.path().join("plugin.sh"),
            format!("touch '{}'\n{}", marker.display(), script(&fixture)),
        )
        .unwrap();
        std::fs::write(dir.path().join("plugin.toml"), MANIFEST).unwrap();
        let manifest = Manifest::load(dir.path()).unwrap();
        assert!(manifest.permissions.is_empty());
        app.install_plugins(vec![manifest], vec![]);
        app.dimensions = [16, 16];
        app.new_document();
        let key = "plugin:mock/info";
        // The pane is visible, but only offers a review.
        assert!(app.pane_open(key));
        for _ in 0..5 {
            frame(&context, &mut app);
        }
        app.command("fill_fg");
        frame(&context, &mut app);
        assert!(!app.plugin_granted("mock"));
        assert!(!app.plugins.running("mock"));
        assert!(app.plugins.panes.get(key).is_none_or(|p| !p.pending));
        let shapes = frame(&context, &mut app).shapes;
        assert!(shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "Review Permissions…"
        )));
        // Actions and formats ask first too.
        app.start_plugin_action("mock", "echo");
        assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
        assert!(app.plugins.action.is_none());
        app.dialog = None;
        app.plugins.permission_request = None;
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists());

        app.grant_plugin("mock", true);
        run_until(&context, &mut app, |app| {
            app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        assert!(app.plugins.running("mock"));
        assert!(marker.exists());
    }

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
        assert!(app.keymap.keys("mock/echo").is_empty());
        assert_eq!(app.plugins.errors.len(), 1);
        let error = &app.plugins.errors[0].error;
        assert!(
            error.contains("Ctrl+Shift+I") && error.contains("invert_selection"),
            "{error}"
        );
        let items = app.plugin_menu_items();
        assert_eq!(
            items[&xuan::plugins::manifest::Menu::Filter][0].shortcut,
            ""
        );

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
        let bound = app.keymap.entries().iter();
        assert_eq!(
            bound
                .filter(|e| e.id.ends_with("/echo") && !e.keys.is_empty())
                .count(),
            1
        );
        assert!(app.plugins.errors[0].error.contains("mock/echo"));
    }

    /// Everything the mock plugin received so far.
    fn received(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("received.log")).unwrap_or_default()
    }

    /// Runs frames for a moment, so a plugin that was sent something has time to log it.
    fn settle(context: &egui::Context, app: &mut EditorApp) {
        for _ in 0..20 {
            frame(context, app);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_network_plugin_asks_before_its_action_sends_document_data() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_network_mock(&mut app, dir.path());
        app.dimensions = [32, 32];
        app.new_document();
        app.command("fill_fg");
        frame(&context, &mut app);
        let layer = app
            .session()
            .unwrap()
            .document
            .active()
            .unwrap()
            .name
            .clone();

        // The estimate goes out before the user agreed to anything, so it
        // carries neither the image nor the texts.
        app.start_plugin_action("mock", "echo");
        run_until(&context, &mut app, |app| {
            app.plugins.action.as_ref().unwrap().estimate.is_some()
        });
        let log = received(dir.path());
        let estimate = log.lines().find(|l| l.contains("action/estimate")).unwrap();
        assert!(estimate.contains("\"source\":null"), "{estimate}");
        assert!(!estimate.contains("hello"), "{estimate}");

        app.add_region(Point::new(4.0, 4.0), Point::new(20.0, 20.0));
        (app.plugins.action.as_mut().unwrap().regions[0].fields)
            .insert("desc".into(), "a red hat".into());
        app.run_plugin_action();
        assert_eq!(app.dialog, Some(Dialog::PluginConsent));
        assert!(app.plugins.jobs.is_empty());
        let consent = app.plugins.consent.clone().unwrap();
        assert_eq!(consent.action.as_deref(), Some("echo"));
        assert_eq!(
            consent.items,
            [
                format!("The pixels of the layer “{layer}”, cropped around the regions"),
                "Positions and sizes of the regions: 1".into(),
                "Region 1 · desc: “a red hat”".into(),
                "prompt: “hello”".into(),
                "The document's size and the names and positions of its layers".into(),
            ]
        );
        frame(&context, &mut app);

        // Cancel sends nothing and keeps the action's dialog for changes.
        app.answer_consent(false);
        assert_eq!(app.dialog, None);
        assert!(app.plugins.consent.is_none());
        assert!(app.plugins.action.is_some());
        assert!(app.plugins.jobs.is_empty());
        assert_eq!(app.status, "Cancelled; nothing was sent");
        settle(&context, &mut app);
        let log = received(dir.path());
        assert!(!log.contains("action/run"), "{log}");
        assert!(
            !log.contains("a red hat") && !log.contains("source.png"),
            "{log}"
        );

        // Send runs it with exactly that.
        app.run_plugin_action();
        assert_eq!(app.dialog, Some(Dialog::PluginConsent));
        app.answer_consent(true);
        assert_eq!(app.plugins.jobs.len(), 1);
        assert!(app.plugins.jobs[0].consented);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let log = received(dir.path());
        let run = log.lines().find(|l| l.contains("action/run")).unwrap();
        assert!(run.contains("a red hat") && run.contains("\"prompt\":\"hello\""));
        assert!(run.contains("source.png"));
        // Not saved: the next run asks again.
        assert!(!send_without_asking(&app));
    }

    #[test]
    fn a_plugin_without_network_hosts_runs_without_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_mock(&mut app, dir.path());
        app.dimensions = [16, 16];
        app.new_document();
        app.command("fill_fg");
        app.start_plugin_action("mock", "echo");
        app.run_plugin_action();
        assert!(app.plugins.consent.is_none());
        assert_ne!(app.dialog, Some(Dialog::PluginConsent));
        assert_eq!(app.plugins.jobs.len(), 1);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
    }

    #[test]
    fn dont_ask_again_lasts_until_the_grant_changes() {
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        app.config_path = Some(config.path().join("config.toml"));
        install_network_mock(&mut app, dir.path());
        app.dimensions = [16, 16];
        app.new_document();
        app.command("fill_fg");
        let layer = app
            .session()
            .unwrap()
            .document
            .active()
            .unwrap()
            .name
            .clone();

        // An action without inputs asks straight away.
        app.start_plugin_action("mock", "send");
        assert_eq!(app.dialog, Some(Dialog::PluginConsent));
        assert_eq!(
            app.plugins.consent.as_ref().unwrap().items,
            [
                format!("The pixels of the layer “{layer}”"),
                "The document's size and the names and positions of its layers".into(),
            ]
        );
        app.plugins.consent.as_mut().unwrap().dont_ask = true;
        app.answer_consent(true);
        assert_eq!(app.plugins.jobs.len(), 1);
        assert!(send_without_asking(&app));
        let text = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
        assert!(text.contains("send_without_asking = true"), "{text}");
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        app.resolve_proposal(false);

        // The next run goes straight out.
        app.start_plugin_action("mock", "send");
        assert!(app.plugins.consent.is_none());
        assert_eq!(app.plugins.jobs.len(), 1);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        app.resolve_proposal(false);

        // New permissions are reviewed again, and the answer goes with the old grant.
        let wider = Manifest::parse(
            &network_manifest("\"example.com\", \"upload.example.com\""),
            dir.path(),
        )
        .unwrap();
        app.install_plugins(vec![wider], vec![]);
        app.start_plugin_action("mock", "send");
        assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
        app.dialog = None;
        app.plugins.permission_request = None;
        app.grant_plugin("mock", true);
        app.start_plugin_action("mock", "send");
        assert_eq!(app.dialog, Some(Dialog::PluginConsent));
        assert!(!send_without_asking(&app));
        // Cancelling an action without inputs leaves nothing open.
        app.answer_consent(false);
        assert!(app.plugins.action.is_none());
        assert!(app.plugins.jobs.is_empty());
    }

    #[test]
    fn exports_outside_an_action_wait_for_the_users_answer() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_network_mock(&mut app, dir.path());
        app.dimensions = [16, 16];
        app.new_document();
        frame(&context, &mut app);
        let key = "plugin:mock/info";
        run_until(&context, &mut app, |app| {
            app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        let export = |app: &mut EditorApp| {
            app.render_pane(
                key,
                "event",
                Some(xuan::plugins::ui::Event {
                    widget: "export".into(),
                    value: serde_json::Value::Bool(true),
                }),
            );
        };
        let answers = |dir: &Path| -> Vec<String> {
            received(dir)
                .lines()
                .filter(|l| l.contains("\"id\":\"export\""))
                .map(str::to_owned)
                .collect()
        };

        // The request waits while the user is asked.
        export(&mut app);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginConsent)
        });
        let consent = app.plugins.consent.clone().unwrap();
        assert_eq!(consent.action, None);
        assert_eq!(consent.items, ["The whole image, flattened"]);
        assert_eq!(app.plugins.held.len(), 1);
        settle(&context, &mut app);
        assert!(answers(dir.path()).is_empty());

        // Cancel answers it with an error, and later ones without asking.
        app.answer_consent(false);
        run_until(&context, &mut app, |_| answers(dir.path()).len() == 1);
        assert!(answers(dir.path())[0].contains("-32800"));
        export(&mut app);
        run_until(&context, &mut app, |_| answers(dir.path()).len() == 2);
        assert!(answers(dir.path())[1].contains("-32800"));
        assert_eq!(app.dialog, None);

        // Once the plugin stops, it is asked again; Send hands over the export.
        app.stop_plugin("mock");
        export(&mut app);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginConsent)
        });
        app.answer_consent(true);
        run_until(&context, &mut app, |_| answers(dir.path()).len() == 3);
        let sent = &answers(dir.path())[2];
        assert!(
            sent.contains("\"result\"") && sent.contains(".png"),
            "{sent}"
        );
        assert!(!send_without_asking(&app));
    }

    #[test]
    fn offline_mode_stops_network_plugins_and_disables_their_actions() {
        let dir = tempfile::tempdir().unwrap();
        let plain_dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        app.config_path = Some(config.path().join("config.toml"));
        install_network_mock(&mut app, dir.path());
        std::fs::copy(
            dir.path().join("plugin.sh"),
            plain_dir.path().join("plugin.sh"),
        )
        .unwrap();
        let plain = Manifest::parse(
            &MANIFEST
                .replace("id = \"mock\"", "id = \"plain\"")
                .replace("shortcut = \"Ctrl+Shift+E\"\n", ""),
            plain_dir.path(),
        )
        .unwrap();
        let network = app.plugins.manifest("mock").unwrap().clone();
        app.install_plugins(vec![network, plain], vec![]);
        app.grant_plugin("plain", true);
        app.config.panes.set_hidden("plugin:plain/info", true);
        app.dimensions = [16, 16];
        app.new_document();
        app.command("fill_fg");
        let key = "plugin:mock/info";
        run_until(&context, &mut app, |app| {
            app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        assert!(app.plugins.running("mock"));
        assert!(app.command_enabled("mock/echo"));

        app.set_network_plugins_disabled(true);
        assert!(!app.plugins.running("mock"));
        assert!(!app.command_enabled("mock/echo"));
        assert!(!app.command_enabled("mock/send"));
        assert!(app.command_enabled("plain/echo"));
        let menu = app.plugin_menu_items();
        let filter = &menu[&xuan::plugins::manifest::Menu::Filter];
        assert!(filter.iter().any(|i| i.plugin == "mock" && !i.enabled));
        assert!(filter.iter().any(|i| i.plugin == "plain" && i.enabled));
        let text = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
        assert!(text.contains("disable_network_plugins = true"), "{text}");

        // Nothing starts it again: not its pane, its actions or a render.
        app.start_plugin_action("mock", "echo");
        assert!(app.plugins.action.is_none());
        assert!(app.status.contains("uses the network"), "{}", app.status);
        app.run_command("mock/send");
        assert!(app.plugins.consent.is_none() && app.plugins.jobs.is_empty());
        app.render_pane(key, "open", None);
        let output = frame(&context, &mut app);
        settle(&context, &mut app);
        assert!(!app.plugins.running("mock"));
        assert!(app.plugins.panes.get(key).is_none_or(|p| p.tree.is_none()));
        let placeholder = output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text)
                if text.galley.text().contains("plugins that use the network are disabled"))
        });
        assert!(placeholder, "the pane says why it is empty");
        // A plugin that declares no network hosts still works.
        app.start_plugin_action("plain", "echo");
        assert!(
            app.plugins.action.is_some(),
            "{:?} {:?} {}",
            app.error,
            app.dialog,
            app.status
        );
        app.close_plugin_action();

        // Turned off, the pane opens again.
        app.set_network_plugins_disabled(false);
        assert!(app.command_enabled("mock/echo"));
        run_until(&context, &mut app, |app| {
            app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        assert!(app.plugins.running("mock"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn blocking_the_network_restarts_plugins_without_hosts_under_the_filter() {
        if let Err(error) = xuan::plugins::sandbox::available() {
            eprintln!("skipped: seccomp filters are not available here: {error:#}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let plain_dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        app.config_path = Some(config.path().join("config.toml"));
        // Whatever the release default is.
        app.config.block_undeclared_network = Some(false);
        install_network_mock(&mut app, dir.path());
        std::fs::copy(
            dir.path().join("plugin.sh"),
            plain_dir.path().join("plugin.sh"),
        )
        .unwrap();
        let plain = Manifest::parse(
            &MANIFEST
                .replace("id = \"mock\"", "id = \"plain\"")
                .replace("shortcut = \"Ctrl+Shift+E\"\n", ""),
            plain_dir.path(),
        )
        .unwrap();
        let network = app.plugins.manifest("mock").unwrap().clone();
        app.install_plugins(vec![network, plain], vec![]);
        app.grant_plugin("plain", true);
        app.dimensions = [16, 16];
        app.new_document();
        app.command("fill_fg");
        let panes_open = |app: &EditorApp| {
            ["plugin:mock/info", "plugin:plain/info"].iter().all(|key| {
                app.plugins
                    .panes
                    .get(*key)
                    .is_some_and(|p| p.tree.is_some())
            })
        };
        let blocked_note = |app: &EditorApp, plugin| {
            app.plugins
                .log(plugin)
                .iter()
                .any(|l| l.starts_with("Network blocked by Xuan"))
        };
        run_until(&context, &mut app, panes_open);
        assert!(!app.plugin_network_blocked("plain"));
        assert!(!blocked_note(&app, "plain"));

        // Turned on, the plugin without hosts restarts under the filter and
        // the one with hosts keeps running unfiltered.
        app.set_block_undeclared_network(true);
        assert!(!app.plugins.running("plain"));
        assert!(app.plugins.running("mock"));
        run_until(&context, &mut app, panes_open);
        assert!(app.plugin_network_blocked("plain") && blocked_note(&app, "plain"));
        assert!(!app.plugin_network_blocked("mock") && !blocked_note(&app, "mock"));
        let text = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
        assert!(text.contains("block_undeclared_network = true"), "{text}");
        // Its actions run as before.
        app.start_plugin_action("plain", "echo");
        assert!(
            app.plugins.action.is_some(),
            "{:?} {}",
            app.error,
            app.status
        );
        app.close_plugin_action();

        // Turned off, it restarts without the filter.
        app.set_block_undeclared_network(false);
        assert!(!app.plugins.running("plain"));
        run_until(&context, &mut app, panes_open);
        assert!(!app.plugin_network_blocked("plain") && !blocked_note(&app, "plain"));
        let text = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
        assert!(text.contains("block_undeclared_network = false"), "{text}");
    }

    #[test]
    fn the_send_prompt_works_through_the_ui() {
        use crate::app::tests::ui::UiTest;
        use egui::accesskit::Role;
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(config.path());
        install_network_mock(ui.app_mut(), dir.path());
        ui.app_mut()
            .config
            .panes
            .set_hidden("plugin:mock/info", true);
        ui.app_mut().command("fill_fg");
        ui.settle();

        ui.open_menu("Plugins");
        ui.click("Send Layer · Mock");
        assert_eq!(ui.app().dialog, Some(Dialog::PluginConsent));
        assert!(ui.has_role(Role::Button, "Send"));
        assert!(ui.has_role(Role::Button, "Cancel"));
        assert!(ui.has_role(Role::CheckBox, "Don't ask again for this plugin"));
        assert!(ui.has("Mock (plugin mock) says it connects to: example.com"));
        ui.click_role(Role::Button, "Cancel");
        assert_eq!(ui.app().dialog, None);
        assert!(ui.app().plugins.jobs.is_empty());
        assert!(!received(dir.path()).contains("action/run"));

        ui.open_menu("Plugins");
        ui.click("Send Layer · Mock");
        ui.click_role(Role::CheckBox, "Don't ask again for this plugin");
        ui.click_role(Role::Button, "Send");
        assert_ne!(ui.app().dialog, Some(Dialog::PluginConsent));
        assert!(send_without_asking(ui.app()));
        assert!(
            !ui.app().plugins.jobs.is_empty() || ui.app().plugins.proposal.is_some(),
            "{:?}",
            ui.app().error
        );
    }

    #[test]
    fn offline_mode_greys_out_network_actions_in_the_menus_and_palette() {
        use crate::app::tests::ui::UiTest;
        use egui::accesskit::Role;
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(config.path());
        install_network_mock(ui.app_mut(), dir.path());
        ui.app_mut()
            .config
            .panes
            .set_hidden("plugin:mock/info", true);
        ui.app_mut().command("fill_fg");
        ui.settle();

        // The quick toggle in Plugins → Manage Plugins…
        ui.open_menu("Plugins");
        ui.click("Manage Plugins…");
        ui.click_role(Role::CheckBox, "Disable plugins that use the network");
        assert!(ui.app().config.disable_network_plugins);
        ui.click("Done");

        ui.open_menu("Plugins");
        assert!(!ui.enabled("Send Layer · Mock"));
        ui.key(egui::Key::Escape);
        ui.press(egui::Modifiers::CTRL, egui::Key::K);
        ui.type_keys("send layer");
        assert!(!ui.enabled("Send Layer · Mock, Plugins"));
        ui.key(egui::Key::Enter);
        assert!(ui.app().plugins.consent.is_none());
        assert!(!ui.app().plugins.running("mock"));
    }
}

//! Plugins hosted in the editor, exercised with a shell script that speaks
//! the protocol.
use super::*;
use xuan::plugins::{Manifest, manifest::DocumentAccess};

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
      case "$line" in
        *'"action":"outpaint"'*)
          printf '{{"jsonrpc":"2.0","id":%s,"result":{{"outputs":[{{"kind":"edit","edits":[{{"op":"extend_canvas","left":4,"top":2,"bottom":6}}]}},{{"kind":"image","path":"%s/result.png","name":"Outpainted","fit":"source"}}]}}}}\n' "$id" "$work" ;;
        *)
          printf '{{"jsonrpc":"2.0","id":%s,"result":{{"outputs":[{{"kind":"image","path":"%s/result.png","name":"Echoed"}},{{"kind":"text","text":"prompt=%s"}}]}}}}\n' "$id" "$work" "$prompt" ;;
      esac ;;
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
    install_mock_with(app, dir, DocumentAccess::Edit);
}

/// The mock plugin as declared: it asks for no permissions, so `document` is
/// "read".
fn install_read_mock(app: &mut EditorApp, dir: &Path) {
    install_mock_with(app, dir, DocumentAccess::Read);
}

fn install_mock_with(app: &mut EditorApp, dir: &Path, document: DocumentAccess) {
    let fixture = dir.join("fixture.png");
    RgbaImage::from_pixel(8, 8, image::Rgba([0, 200, 0, 255]))
        .save(&fixture)
        .unwrap();
    std::fs::write(dir.join("plugin.sh"), script(&fixture)).unwrap();
    std::fs::write(dir.join("plugin.toml"), MANIFEST).unwrap();
    let mut manifest = Manifest::load(dir).unwrap();
    manifest.permissions.document = document;
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
    let mut manifest = Manifest::load(dir).unwrap();
    manifest.permissions.document = DocumentAccess::Edit;
    app.install_plugins(vec![manifest], vec![]);
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
        provider: None,
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

/// The mock plugin with a secret setting called `key` whose value is `SECRET`.
const SECRET: &str = "sk-live-0123456789";

fn install_secret_mock(app: &mut EditorApp, dir: &Path) {
    RgbaImage::from_pixel(8, 8, image::Rgba([0, 200, 0, 255]))
        .save(dir.join("fixture.png"))
        .unwrap();
    let manifest = Manifest::parse(
        &MANIFEST.replace(
            "[[actions]]",
            "[permissions]\nsecrets = [\"key\"]\ndocument = \"edit\"\n\n[[settings]]\nid = \"key\"\ntype = \"secret\"\n\n[[actions]]",
        ),
        dir,
    )
    .unwrap();
    app.install_plugins(vec![manifest], vec![]);
    app.grant_plugin("mock", true);
    app.plugins
        .secrets
        .0
        .entry("mock".into())
        .or_default()
        .insert("key".into(), SECRET.into());
}

#[test]
fn provenance_is_kept_on_the_layer_and_never_holds_the_plugins_secrets() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_secret_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    let job = mock_job(&app);
    let fixture = dir.path().join("fixture.png");

    // A plugin that puts its API key into the record, under a generic name,
    // under its own setting name and inside a value.
    app.apply_job_result(
        &job,
        json!({"outputs": [{
            "kind": "image", "path": fixture,
            "provenance": {
                "model": "sdxl.safetensors", "sampler": "euler", "steps": 20, "seed": 7,
                "cfg": 6.5, "service": "mock cloud", "request_id": "r-1",
                "api_key": SECRET,
                "extra": {"key": "other", "note": format!("signed with {SECRET}"), "lora": "a"},
            },
        }]}),
    )
    .unwrap();
    assert!(
        app.status.contains("3 provenance entries"),
        "{}",
        app.status
    );
    assert!(!app.status.contains(SECRET));
    app.resolve_proposal(true);
    let document = &app.session().unwrap().document;
    let layer = document.layers.last().unwrap();
    let record = layer.provenance.clone().unwrap();
    assert_eq!(record.model.as_deref(), Some("sdxl.safetensors"));
    assert_eq!(
        (record.steps, record.seed, record.cfg),
        (Some(20), Some(7), Some(6.5))
    );
    assert_eq!(record.extra.len(), 1);
    assert_eq!(record.extra["lora"], "a");
    assert!(layer.generated.is_some());
    assert!(!serde_json::to_string(document).unwrap().contains(SECRET));

    // The project is saved as format 8 and the record comes back.
    let path = dir.path().join("project.xuan");
    io::save(document, &path).unwrap();
    let loaded = io::load(&path).unwrap();
    assert_eq!(loaded.layers.last().unwrap().provenance, Some(record));

    // Unknown keys and oversized records refuse the whole result.
    let layers = app.session().unwrap().document.layers.len();
    for bad in [
        json!({"modle": "typo"}),
        json!({"model": "x".repeat(300)}),
        json!({"extra": {"a": {"b": {"c": {"d": {"e": 1}}}}}}),
        json!("a string"),
    ] {
        let result = app.apply_job_result(
            &job,
            json!({"outputs": [{"kind": "image", "path": fixture, "provenance": bad}]}),
        );
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("provenance"), "{bad}: {error}");
    }
    assert_eq!(app.session().unwrap().document.layers.len(), layers);
    // Without provenance the layer simply has none, and a null is no record.
    for provenance in [json!(null), json!({})] {
        app.apply_job_result(
            &job,
            json!({"outputs": [{"kind": "image", "path": fixture, "provenance": provenance}]}),
        )
        .unwrap();
        app.resolve_proposal(true);
        assert!(
            app.session()
                .unwrap()
                .document
                .layers
                .last()
                .unwrap()
                .provenance
                .is_none()
        );
    }
}

/// An action that extends a composite source by 4 px on the left, 2 px at
/// the top and the `amount` input (6 by default) at the bottom.
const OUTPAINT: &str = r#"
[[actions]]
id = "outpaint"
label = "Outpaint"
source = { from = "composite", extend = { left = 4, top = 2, bottom = "amount" } }

[[actions.inputs]]
id = "amount"
type = "integer"
default = 6
"#;

/// The mock manifest with [`OUTPAINT`], declaring `document = "edit"` when
/// `edit` is set.
fn outpaint_manifest(edit: bool) -> String {
    let manifest = if edit {
        MANIFEST.replacen(
            "[[actions]]",
            "[permissions]\ndocument = \"edit\"\n\n[[actions]]",
            1,
        )
    } else {
        MANIFEST.to_owned()
    };
    format!("{manifest}{OUTPAINT}")
}

fn install_outpaint(app: &mut EditorApp, dir: &Path, edit: bool) {
    let manifest = Manifest::parse(&outpaint_manifest(edit), dir).unwrap();
    app.install_plugins(vec![manifest], vec![]);
    app.grant_plugin("mock", true);
}

/// A finished outpaint job of the mock plugin, with its source prepared as
/// the host prepares it.
fn outpaint_job(app: &EditorApp) -> crate::app::plugins::PluginJob {
    let spec = (app.plugins.manifest("mock").unwrap())
        .action("outpaint")
        .unwrap()
        .clone();
    let mut job = mock_job(app);
    let inputs = serde_json::Map::from_iter([("amount".to_owned(), serde_json::json!(6))]);
    job.action = "outpaint".into();
    job.prepared = xuan::plugins::jobs::prepare(
        &app.session().unwrap().document,
        &spec.source.with_inputs(&inputs),
        &[],
        job._work_dir.path(),
    )
    .unwrap();
    job
}

#[test]
fn extending_the_canvas_needs_document_edit_even_in_a_result() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_outpaint(&mut app, dir.path(), false);
    app.dimensions = [8, 8];
    app.new_document();
    let steps = app.session().unwrap().history.names().count();
    let extend = json!({"op": "extend_canvas", "left": 2});
    let size = |app: &EditorApp| {
        let document = &app.session().unwrap().document;
        (document.width, document.height)
    };

    let error = plugin_request(
        &mut app,
        "document/edit",
        json!({"name": "Grow", "edits": [extend]}),
    )
    .unwrap_err();
    assert!(
        error.message.contains("document = \"edit\""),
        "{}",
        error.message
    );
    let job = outpaint_job(&app);
    let result = json!({"outputs": [{"kind": "edit", "edits": [extend]}]});
    let error = format!(
        "{:#}",
        app.apply_job_result(&job, result.clone()).unwrap_err()
    );
    assert!(error.contains("document = \"edit\""), "{error}");
    assert_eq!(size(&app), (8, 8));
    assert!(app.plugins.proposal.is_none());
    assert_eq!(app.session().unwrap().history.names().count(), steps);

    // With the permission, both work: the request as one undo step.
    install_outpaint(&mut app, dir.path(), true);
    plugin_request(
        &mut app,
        "document/edit",
        json!({"name": "Grow", "edits": [extend]}),
    )
    .unwrap();
    assert_eq!(size(&app), (10, 8));
    assert_eq!(app.session().unwrap().history.undo_name(), Some("Grow"));
    app.command("undo");
    assert_eq!(size(&app), (8, 8));
    app.apply_job_result(&job, result).unwrap();
    assert_eq!(size(&app), (10, 8));
    app.resolve_proposal(false);
    assert_eq!(size(&app), (8, 8));
    // Past the size limits the request changes nothing.
    let huge = json!({"op": "extend_canvas", "right": 30_000});
    let error = plugin_request(&mut app, "document/edit", json!({"edits": [huge]})).unwrap_err();
    assert!(error.message.contains("30000"), "{}", error.message);
    assert_eq!(size(&app), (8, 8));
}

#[test]
fn an_outpaint_result_extends_the_canvas_and_fits_the_extended_source() {
    use serde_json::json;
    use xuan::layout::{Guide, GuideAxis};
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_outpaint(&mut app, dir.path(), true);
    app.dimensions = [8, 8];
    app.new_document();
    app.command("fill_fg");
    let session = app.session_mut().unwrap();
    session.document.guides = vec![
        Guide::new(GuideAxis::Vertical, 3.0),
        Guide::new(GuideAxis::Horizontal, 5.0),
    ];
    session.history.commit();
    session.fit = false;
    let original = app.session().unwrap().document.clone();
    let layout = |document: &Document| -> Vec<_> {
        (document.layers.iter())
            .map(|l| (l.id, l.transform, l.pixels.clone()))
            .collect()
    };
    let steps = app.session().unwrap().history.names().count();
    let job = outpaint_job(&app);
    let export = job.prepared.export.as_ref().unwrap();
    assert_eq!((export.width, export.height), (12, 16));

    // A twice-as-dense result for the extended source, and a mask of the
    // left strip of new canvas, both fitted to the source.
    let result_png = dir.path().join("outpainted.png");
    RgbaImage::from_pixel(24, 32, image::Rgba([1, 2, 3, 255]))
        .save(&result_png)
        .unwrap();
    let mask = write_mask(dir.path(), "left.png", (12, 16), |x, _| {
        if x < 4 { 255 } else { 0 }
    });
    let result = json!({"outputs": [
        {"kind": "edit", "edits": [{"op": "extend_canvas", "left": 4, "top": 2, "bottom": 6}]},
        {"kind": "image", "path": result_png, "name": "Outpainted", "fit": "source"},
        {"kind": "mask", "path": mask, "fit": "source"},
    ]});
    // The same layout Canvas Size gives, anchored right and a quarter down.
    let mut resized = original.clone();
    xuan::operations::canvas_size(&mut resized, 12, 16, [1.0, 0.25]).unwrap();
    let check = |app: &EditorApp| {
        let document = &app.session().unwrap().document;
        assert_eq!((document.width, document.height), (12, 16));
        let base = &document.layers[0].transform;
        assert_eq!((base.x, base.y), (4.0, 2.0));
        assert_eq!(*base, resized.layers[0].transform);
        let guides: Vec<f32> = document.guides.iter().map(|g| g.position).collect();
        assert_eq!(guides, [7.0, 7.0]);
        assert_eq!(document.guides, resized.guides);
        let added = document
            .layers
            .iter()
            .find(|l| l.name == "Outpainted")
            .unwrap();
        let t = added.transform;
        assert_eq!((t.x, t.y, t.width, t.height), (0.0, 0.0, 12.0, 16.0));
        assert_eq!(added.pixels.as_ref().unwrap().dimensions(), (24, 32));
        // The mask lands on the new left strip.
        assert_eq!(selection_at(app, 1, 8), Some(255));
        assert_eq!(selection_at(app, 4, 8), Some(0));
    };

    app.apply_job_result(&job, result.clone()).unwrap();
    assert_eq!(app.dialog, Some(Dialog::PluginProposal));
    check(&app);
    assert!(app.session().unwrap().fit);
    // Discard leaves the document as it was.
    app.session_mut().unwrap().fit = false;
    app.resolve_proposal(false);
    let session = app.session().unwrap();
    assert_eq!(session.document.width, original.width);
    assert_eq!(layout(&session.document), layout(&original));
    assert_eq!(session.document.guides, original.guides);
    assert_eq!(session.history.names().count(), steps);
    assert!(session.fit);

    // Accept makes it one undo step, and undo brings the old canvas back.
    app.apply_job_result(&job, result).unwrap();
    app.resolve_proposal(true);
    check(&app);
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), steps + 1);
    assert_eq!(session.history.undo_name(), Some("Echo"));
    app.command("undo");
    let document = &app.session().unwrap().document;
    assert_eq!((document.width, document.height), (8, 8));
    assert_eq!(layout(document), layout(&original));
    assert_eq!(document.guides, original.guides);
}

#[test]
fn the_send_prompt_names_the_extension() {
    use crate::app::plugins::ActionEdit;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    app.config_path = None;
    let text = outpaint_manifest(false).replacen(
        "[[actions]]",
        "[permissions]\nnetwork = [\"example.com\"]\n\n[[actions]]",
        1,
    );
    app.install_plugins(vec![Manifest::parse(&text, dir.path()).unwrap()], vec![]);
    app.grant_plugin("mock", true);
    app.dimensions = [8, 8];
    app.new_document();
    let spec = (app.plugins.manifest("mock").unwrap())
        .action("outpaint")
        .unwrap()
        .clone();
    let edit = |amount: i64| ActionEdit {
        plugin: "mock".into(),
        action: "outpaint".into(),
        values: serde_json::Map::from_iter([("amount".to_owned(), serde_json::json!(amount))]),
        regions: Vec::new(),
        selected: None,
        estimate: None,
        previous_tool: app.tool,
        into: xuan::plugins::manifest::ResultInto::Layer,
        consented: false,
        provider: None,
    };
    app.plugins.action = Some(edit(6));
    assert_eq!(
        app.action_consent_items(&spec)[0],
        "The whole image, flattened, extended by 4 px on the left, 2 px at the top, 6 px at the bottom"
    );
    // Sides of 0 are left out, and the input's value is the one used.
    app.plugins.action = Some(edit(0));
    assert_eq!(
        app.action_consent_items(&spec)[0],
        "The whole image, flattened, extended by 4 px on the left, 2 px at the top"
    );
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
        ..Source::default()
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
    install_read_mock(&mut app, dir.path());
    assert_eq!(
        app.plugins.manifest("mock").unwrap().permissions.document,
        DocumentAccess::Read
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

/// What a result can change that a read-only plugin must not touch.
fn document_state(app: &EditorApp) -> (usize, usize, Vec<uuid::Uuid>, Vec<Vec<u8>>, bool, usize) {
    let session = app.session().unwrap();
    let document = &session.document;
    (
        app.sessions.len(),
        document.width as usize * 10_000 + document.height as usize,
        document.layers.iter().map(|l| l.id).collect(),
        (document.layers.iter())
            .map(|l| {
                l.pixels
                    .as_deref()
                    .map(|p| p.as_raw().clone())
                    .unwrap_or_default()
            })
            .collect(),
        document.selection.is_some(),
        session.history.names().count(),
    )
}

#[test]
fn a_read_only_plugin_cannot_return_results_that_change_the_document() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_read_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    app.command("fill_fg");
    let layer = app.session().unwrap().document.layers[0].id;
    let fixture = dir.path().join("fixture.png");
    let mask = write_mask(dir.path(), "mask.png", (8, 8), |_, _| 255);
    let before = document_state(&app);

    let layer_job = mock_job(&app);
    let mut replace_job = mock_job(&app);
    replace_job.into = xuan::plugins::manifest::ResultInto::Replace;
    replace_job.prepared.layer = Some(layer);
    let image = json!({"kind": "image", "path": fixture});
    let edit = |edit| json!({"kind": "edit", "edits": [edit]});
    let results = [
        (&layer_job, json!({"outputs": [image]})),
        (&replace_job, json!({"outputs": [image]})),
        // The whole result is refused, also next to a harmless mask.
        (
            &layer_job,
            json!({"outputs": [{"kind": "mask", "path": mask}, image]}),
        ),
        (
            &layer_job,
            json!({"outputs": [edit(json!({"op": "add_layer", "image": fixture}))]}),
        ),
        (
            &layer_job,
            json!({"outputs": [edit(json!({"op": "replace_pixels", "layer": layer, "image": fixture}))]}),
        ),
        (
            &layer_job,
            json!({"outputs": [edit(json!({"op": "remove_layer", "layer": layer}))]}),
        ),
        (
            &layer_job,
            json!({"outputs": [edit(json!({"op": "set", "layer": layer, "name": "x"}))]}),
        ),
        (
            &layer_job,
            json!({"outputs": [edit(json!({"op": "extend_canvas", "left": 2}))]}),
        ),
        (
            &layer_job,
            json!({"outputs": [{"kind": "edit", "edits": [
                {"op": "set_selection", "mask": mask},
                {"op": "extend_canvas", "right": 1},
            ]}]}),
        ),
    ];
    for (job, result) in results {
        let error = format!(
            "{:#}",
            app.apply_job_result(job, result.clone()).unwrap_err()
        );
        assert!(error.contains("document = \"edit\""), "{result}: {error}");
        assert!(app.plugins.proposal.is_none(), "{result}");
        assert_eq!(document_state(&app), before, "{result}");
    }
}

#[test]
fn a_read_only_plugin_can_return_masks_selections_and_new_documents() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_read_mock(&mut app, dir.path());
    app.dimensions = [8, 8];
    app.new_document();
    app.command("fill_fg");
    let fixture = dir.path().join("fixture.png");
    let mask = write_mask(
        dir.path(),
        "mask.png",
        (8, 8),
        |x, _| if x < 2 { 255 } else { 0 },
    );
    let job = mock_job(&app);

    // A mask and a selection-only edit are proposals about the selection.
    for output in [
        json!({"kind": "mask", "path": mask}),
        json!({"kind": "edit", "edits": [{"op": "set_selection", "mask": mask}]}),
        json!({"kind": "text", "text": "done"}),
    ] {
        app.apply_job_result(&job, json!({"outputs": [output]}))
            .unwrap();
        let proposed = app.plugins.proposal.is_some();
        app.resolve_proposal(false);
        assert_eq!(proposed, output["kind"] != "text", "{output}");
    }
    app.apply_job_result(&job, json!({"outputs": [{"kind": "mask", "path": mask}]}))
        .unwrap();
    app.resolve_proposal(true);
    assert_eq!(selection_at(&app, 0, 0), Some(255));

    // New documents leave the open one alone, as a `document` output or as an
    // image of an action whose result goes into a new document.
    let layers = app.session().unwrap().document.layers.len();
    let tabs = app.sessions.len();
    app.current = 0;
    app.apply_job_result(
        &job,
        json!({"outputs": [{"kind": "document", "path": fixture, "name": "New"}]}),
    )
    .unwrap();
    assert_eq!(app.sessions.len(), tabs + 1);
    app.current = 0;
    let mut into_document = mock_job(&app);
    into_document.into = xuan::plugins::manifest::ResultInto::Document;
    app.apply_job_result(
        &into_document,
        json!({"outputs": [{"kind": "image", "path": fixture}]}),
    )
    .unwrap();
    assert_eq!(app.sessions.len(), tabs + 2);
    assert_eq!(app.sessions[0].document.layers.len(), layers);
    assert!(app.plugins.proposal.is_none());
}

#[test]
fn an_editing_plugin_still_proposes_layers_replacements_and_edits() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    assert_eq!(
        app.plugins.manifest("mock").unwrap().permissions.document,
        DocumentAccess::Edit
    );
    app.dimensions = [8, 8];
    app.new_document();
    app.command("fill_fg");
    let fixture = dir.path().join("fixture.png");
    let job = mock_job(&app);
    let before = document_state(&app);
    for output in [
        json!({"kind": "image", "path": fixture}),
        json!({"kind": "edit", "edits": [{"op": "add_layer", "image": fixture}]}),
        json!({"kind": "edit", "edits": [{"op": "extend_canvas", "left": 1}]}),
    ] {
        app.apply_job_result(&job, json!({"outputs": [output]}))
            .unwrap();
        assert!(app.plugins.proposal.is_some(), "{output}");
        app.resolve_proposal(true);
        assert_ne!(document_state(&app), before, "{output}");
        let session = app.session_mut().unwrap();
        assert!(session.history.undo(&mut session.document));
    }
}

#[test]
fn a_read_only_manifest_cannot_declare_a_result_that_changes_the_image() {
    let dir = tempfile::tempdir().unwrap();
    let with = |permissions: &str, result: &str| {
        let text = format!(
            "{}\n{permissions}\n[[actions]]\nid = \"a\"\nlabel = \"A\"\n{result}\n",
            MANIFEST.split("[[actions]]").next().unwrap()
        );
        Manifest::parse(&text, dir.path())
    };
    for into in ["layer", "replace", "ask"] {
        let error = with("", &format!("result = {{ into = \"{into}\" }}"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("document = \"read\""), "{into}: {error}");
        assert!(error.contains("action `a`"), "{into}: {error}");
        assert!(
            with(
                "[permissions]\ndocument = \"read\"\n",
                &format!("result = {{ into = \"{into}\" }}")
            )
            .is_err()
        );
        // The plugin that declares what it needs is fine.
        with(
            "[permissions]\ndocument = \"edit\"\n",
            &format!("result = {{ into = \"{into}\" }}"),
        )
        .unwrap();
    }
    // New documents, and actions that leave `result` out (masks), are fine.
    with("", "result = { into = \"document\" }").unwrap();
    with("", "").unwrap();
}

#[test]
fn the_permission_review_says_what_a_read_only_plugin_can_do() {
    use crate::app::tests::ui::UiTest;
    let config = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let folder = installable_plugin(source.path(), "reader", "1.0.0", "");
    let mut ui = UiTest::new();
    ui.isolate_config(config.path());
    ui.open_menu("Plugins");
    ui.click("Install from Folder or Zip…");
    ui.drop_files(&[&folder]);
    assert!(ui.has("Id: inst"));
    assert!(ui.has(
        "• Can read the document and propose selections or new documents, but can't change your image"
    ));
    assert!(!ui.has("• Edits documents directly (as undoable steps)"));
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
const INPAINT: &str = r#"
[[actions]]
id = "inpaint"
label = "Inpaint"
source = { from = "composite", max_side = 512, mask = "selection" }
"#;

#[test]
fn a_selection_mask_is_listed_for_consent_and_needs_a_selection() {
    use crate::app::plugins::ActionEdit;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    app.config_path = None;
    let fixture = dir.path().join("fixture.png");
    RgbaImage::new(8, 8).save(&fixture).unwrap();
    std::fs::write(dir.path().join("plugin.sh"), script(&fixture)).unwrap();
    std::fs::write(
        dir.path().join("plugin.toml"),
        format!("{}{INPAINT}", network_manifest("\"example.com\"")),
    )
    .unwrap();
    app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
    app.grant_plugin("mock", true);
    app.dimensions = [8, 8];
    app.new_document();
    let spec = app
        .plugins
        .manifest("mock")
        .unwrap()
        .action("inpaint")
        .unwrap()
        .clone();
    app.plugins.action = Some(ActionEdit {
        plugin: "mock".into(),
        action: "inpaint".into(),
        values: serde_json::Map::new(),
        regions: Vec::new(),
        selected: None,
        estimate: None,
        previous_tool: app.tool,
        into: xuan::plugins::manifest::ResultInto::Layer,
        consented: false,
        provider: None,
    });
    // Nothing selected: the action refuses to start, and no mask is listed.
    app.start_plugin_action("mock", "inpaint");
    assert!(app.error.take().is_some());
    assert!(
        !app.action_consent_items(&spec)
            .contains(&"The selection, as a mask".to_string())
    );
    let selection = selection_where(&app, |x, _| x < 4);
    app.session_mut().unwrap().document.selection = Some(selection);
    let items = app.action_consent_items(&spec);
    assert_eq!(
        items[..3],
        [
            "The whole image, flattened, longest side at most 512 px",
            "The selection, as a mask",
            "The document's size and the names and positions of its layers",
        ]
    );
}

/// A plugin folder to install, `<root>/<folder>`, with the id `inst`.
fn installable_plugin(root: &Path, folder: &str, version: &str, permissions: &str) -> PathBuf {
    let dir = root.join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "[plugin]\nid = \"inst\"\nname = \"Installed\"\nversion = \"{version}\"\ncommand = [\"sh\", \"plugin.sh\"]\n{permissions}"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("plugin.sh"), "exit 0\n").unwrap();
    dir
}

fn staged(app: &EditorApp) -> &xuan::plugins::install::Staged {
    let install = app.plugins.install.as_ref().expect("the install window");
    install
        .staged
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", install.error))
}

#[test]
fn installing_does_not_allow_and_updates_keep_the_grant_only_when_unchanged() {
    use crate::app::plugin_install::GrantAfterInstall;
    let config = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let plugins_dir = config.path().join("plugins");
    let (_context, mut app) = app();
    app.config_path = Some(config.path().join("config.toml"));

    app.dialog = Some(Dialog::Plugins);
    app.open_plugin_install();
    assert_eq!(app.dialog, Some(Dialog::PluginInstall));
    let v1 = installable_plugin(source.path(), "v1", "1.0.0", "");
    app.stage_plugin_install(&v1);
    assert!(!staged(&app).update);
    assert_eq!(staged(&app).target, plugins_dir.join("inst"));
    assert_eq!(
        app.grant_after_install(staged(&app)),
        GrantAfterInstall::Asks
    );
    assert!(!plugins_dir.exists(), "nothing is copied before Install");
    app.commit_plugin_install();
    assert!(plugins_dir.join("inst/plugin.sh").is_file());
    // Back in Manage Plugins with the new plugin selected, not allowed.
    assert_eq!(app.dialog, Some(Dialog::Plugins));
    assert!(app.plugins.install.is_none());
    assert_eq!(app.plugins.manager_selected.as_deref(), Some("inst"));
    assert!(app.plugins.manifest("inst").is_some());
    assert!(!app.plugin_granted("inst"));
    app.grant_plugin("inst", true);
    assert!(app.plugin_granted("inst"));

    // An update with the same command and permissions keeps the grant.
    let v2 = installable_plugin(source.path(), "v2", "2.0.0", "");
    app.stage_plugin_install(&v2);
    assert!(staged(&app).update);
    assert_eq!(
        app.grant_after_install(staged(&app)),
        GrantAfterInstall::Kept
    );
    app.commit_plugin_install();
    assert_eq!(
        app.plugins.manifest("inst").unwrap().plugin.version,
        "2.0.0"
    );
    assert!(app.plugin_granted("inst"));

    // One that asks for more is reviewed again before it runs.
    let v3 = installable_plugin(
        source.path(),
        "v3",
        "3.0.0",
        "[permissions]\nnetwork = [\"example.com\"]\n",
    );
    app.stage_plugin_install(&v3);
    assert_eq!(
        app.grant_after_install(staged(&app)),
        GrantAfterInstall::AsksAgain
    );
    app.commit_plugin_install();
    assert_eq!(
        app.plugins.manifest("inst").unwrap().plugin.version,
        "3.0.0"
    );
    assert!(!app.plugin_granted("inst"));

    // Problems are shown in the window and nothing is copied.
    app.stage_plugin_install(&source.path().join("missing"));
    let install = app.plugins.install.as_ref().unwrap();
    assert!(install.staged.is_none());
    assert!(install.error.as_ref().unwrap().contains("missing"));
    assert_eq!(app.dialog, Some(Dialog::PluginInstall));

    // Without a configuration folder there is nowhere to install to.
    app.config_path = None;
    app.stage_plugin_install(&v1);
    assert!(
        app.plugins
            .install
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("configuration folder")
    );
}

#[test]
fn a_folder_or_zip_dropped_on_manage_plugins_is_staged_for_install() {
    let config = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    app.config_path = Some(config.path().join("config.toml"));
    let folder = installable_plugin(source.path(), "dropped", "1.0.0", "");
    app.dialog = Some(Dialog::Plugins);
    app.queue_drop(vec![folder.clone()]);
    assert!(app.pending_drops.is_empty());
    assert_eq!(app.dialog, Some(Dialog::PluginInstall));
    assert_eq!(staged(&app).source, folder);
    assert!(app.plugins.install.as_ref().unwrap().from_manager);
    // Elsewhere a dropped folder is still a project to open.
    app.dialog = None;
    app.plugins.install = None;
    app.queue_drop(vec![folder]);
    assert_eq!(app.pending_drops.len(), 1);
    assert!(app.plugins.install.is_none());
}

#[test]
fn the_install_review_shows_the_plugin_before_anything_is_copied() {
    use crate::app::tests::ui::UiTest;
    let config = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let folder = installable_plugin(
        source.path(),
        "reviewed",
        "1.2.3",
        "[permissions]\nnetwork = [\"example.com\"]\ndocument = \"edit\"\n",
    );
    let target = config.path().join("plugins").join("inst");
    let mut ui = UiTest::new();
    ui.isolate_config(config.path());

    ui.open_menu("Plugins");
    ui.click("Install from Folder or Zip…");
    assert!(ui.has("Choose Zip…") && ui.has("Choose Folder…"));
    // The native file picker cannot run here; dropping the folder is the
    // other way in.
    ui.drop_files(&[&folder]);
    assert!(ui.has("Id: inst"));
    assert!(ui.has("Version: 1.2.3"));
    assert!(ui.has(&format!("From: {}", folder.display())));
    assert!(ui.has(&format!("Installs to: {}", target.display())));
    assert!(ui.has("Runs: sh plugin.sh"));
    assert!(ui.has("• Says it connects to: example.com"));
    assert!(ui.has("• Edits documents directly (as undoable steps)"));
    assert!(ui.has(
        "• Installing does not allow it to run: Xuan asks for that the first time it starts."
    ));
    assert!(!target.exists());
    ui.click("Cancel");
    assert_eq!(ui.app().dialog, None);
    assert!(!target.exists(), "Cancel copies nothing");

    // From Manage Plugins, Install… leads to the same review.
    ui.open_menu("Plugins");
    ui.click("Manage Plugins…");
    ui.click("Install…");
    ui.drop_files(&[&folder]);
    ui.click("Install");
    assert!(target.join("plugin.toml").is_file());
    assert_eq!(ui.app().dialog, Some(Dialog::Plugins));
    assert_eq!(ui.app().plugins.manager_selected.as_deref(), Some("inst"));
    assert!(!ui.app().plugin_granted("inst"));
    assert!(
        ui.has_role(egui::accesskit::Role::Button, "Allow"),
        "allowing stays in the usual review"
    );

    // Installing it again is an update, and says what happens to the grant.
    ui.app_mut().grant_plugin("inst", true);
    ui.click("Install…");
    ui.drop_files(&[&folder]);
    assert!(ui.has("• Replaces the installed version 1.2.3"));
    assert!(ui.has(
        "• It stays allowed: you allowed it before with the same folder, command and permissions."
    ));
    ui.click("Update");
    assert!(ui.app().plugin_granted("inst"));
}

#[test]
fn new_edit_ops_need_edit_access_and_make_one_undo_step() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_read_mock(&mut app, dir.path());
    app.dimensions = [32, 24];
    app.new_document();
    app.command("fill_fg");
    let edits = json!({"name": "Agent edit", "edits": [
        {"op": "add_shape_layer", "shape": "Rectangle", "x": 2, "y": 2, "width": 8, "height": 6, "color": "#ff0000"},
        {"op": "add_adjustment_layer", "adjustment": "Invert"},
        {"op": "select_rect", "x": 0, "y": 0, "width": 10, "height": 10},
    ]});
    // A read-only plugin can't, even for the selection.
    let refused = plugin_request(&mut app, "document/edit", edits.clone()).unwrap_err();
    assert!(refused.message.contains("edit"), "{}", refused.message);
    let select = json!({"edits": [{"op": "select_rect", "x": 0, "y": 0, "width": 4, "height": 4}]});
    assert!(plugin_request(&mut app, "document/edit", select).is_err());
    assert!(app.session().unwrap().document.selection.is_none());

    install_mock(&mut app, dir.path());
    let before = app.session().unwrap().document.clone();
    let steps = app.session().unwrap().history.names().count();
    let answer = plugin_request(&mut app, "document/edit", edits).unwrap();
    let added: Vec<uuid::Uuid> = serde_json::from_value(answer["layers"].clone()).unwrap();
    assert_eq!(added.len(), 2);
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), steps + 1);
    assert_eq!(session.history.undo_name(), Some("Agent edit"));
    assert!(session.document.selection.is_some());
    assert!(
        added
            .iter()
            .all(|id| session.document.layers.iter().any(|l| l.id == *id))
    );
    app.command("undo");
    let document = &app.session().unwrap().document;
    assert_eq!(document.layers.len(), before.layers.len());
    assert!(document.selection.is_none());

    // A failing batch changes nothing and adds no step.
    let failing = json!({"edits": [
        {"op": "add_empty_layer"},
        {"op": "apply_filter", "filter": {"GaussianBlur": {"radius": 1000}}},
    ]});
    assert!(plugin_request(&mut app, "document/edit", failing).is_err());
    assert_eq!(
        app.session().unwrap().document.layers.len(),
        before.layers.len()
    );
    assert_eq!(
        app.session().unwrap().history.redo_name(),
        Some("Agent edit")
    );
}

#[test]
fn a_gradient_edit_is_one_undo_step() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [32, 24];
    app.new_document();
    let before = app.session().unwrap().document.layers[0].pixels.clone();
    let steps = app.session().unwrap().history.names().count();
    let edits = json!({"name": "Gradient", "edits": [
        {"op": "select_rect", "x": 0, "y": 0, "width": 16, "height": 24},
        {"op": "gradient", "start": [0, 0], "end": [32, 0], "stops": [
            {"position": 0, "color": "#ff0000"},
            {"position": 0.5, "color": "#00ff00"},
            {"position": 1, "color": "#0000ff"},
        ]},
    ]});
    plugin_request(&mut app, "document/edit", edits).unwrap();
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), steps + 1);
    assert_eq!(session.history.undo_name(), Some("Gradient"));
    let pixels = session.document.layers[0].pixels.clone().unwrap();
    assert_ne!(pixels.get_pixel(2, 2).0, pixels.get_pixel(30, 2).0);
    app.command("undo");
    assert_eq!(app.session().unwrap().history.names().count(), steps);
    assert_eq!(app.session().unwrap().document.layers[0].pixels, before);
}

#[test]
fn edits_name_layers_the_request_added_and_fail_as_a_whole() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [32, 24];
    app.new_document();
    let before = app.session().unwrap().document.clone();
    let steps = app.session().unwrap().history.names().count();
    // A text layer, then its opacity and placement through `$1`, and two
    // dabs on a new layer `$2`: one request, one undo step.
    let edits = json!({"name": "Batch", "edits": [
        {"op": "add_text_layer", "text": "A", "x": 2, "y": 2, "size": 12},
        {"op": "set", "layer": "$1", "opacity": 0.5, "name": "Letter"},
        {"op": "transform", "layer": "$1", "rotation": 15},
        {"op": "add_empty_layer", "name": "Stars", "above": "$1"},
        {"op": "stroke", "layer": "$2", "points": [[8, 8]], "size": 4, "hardness": 1, "color": "#ffffff"},
        {"op": "stroke", "layer": "$2", "points": [[24, 16]], "size": 4, "hardness": 1, "color": "#ffffff"},
    ]});
    let answer = plugin_request(&mut app, "document/edit", edits).unwrap();
    let added: Vec<uuid::Uuid> = serde_json::from_value(answer["layers"].clone()).unwrap();
    assert_eq!(added.len(), 2);
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), steps + 1);
    let layer = |id| session.document.layers.iter().find(|l| l.id == id).unwrap();
    assert_eq!(layer(added[0]).name, "Letter");
    assert_eq!(layer(added[0]).opacity, 0.5);
    assert_eq!(layer(added[0]).transform.rotation, 15.0);
    let stars = layer(added[1]).pixels.clone().unwrap();
    assert_eq!(stars.get_pixel(8, 8).0[3], 255);
    assert_eq!(stars.get_pixel(24, 16).0[3], 255);
    assert_eq!(stars.get_pixel(16, 12).0[3], 0, "no line between the dabs");

    // A step naming a layer the request has not added fails the request,
    // saying which edit; nothing is applied and no step is added.
    let failing = json!({"edits": [
        {"op": "add_empty_layer"},
        {"op": "set", "layer": "$2", "opacity": 0.5},
    ]});
    let error = plugin_request(&mut app, "document/edit", failing).unwrap_err();
    assert!(
        error.message.contains("Edit 2 (set)") && error.message.contains("`$2`"),
        "{}",
        error.message
    );
    let bad = json!({"edits": [{"op": "set", "layer": "$0", "opacity": 0.5}]});
    let error = plugin_request(&mut app, "document/edit", bad).unwrap_err();
    assert!(error.message.contains("`$0`"), "{}", error.message);
    let session = app.session().unwrap();
    assert_eq!(session.history.names().count(), steps + 1);
    assert_eq!(session.document.layers.len(), before.layers.len() + 2);
}

#[test]
fn move_layer_is_one_undo_step() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [16, 16];
    app.new_document();
    let answer = plugin_request(
        &mut app,
        "document/edit",
        json!({"edits": [{"op": "add_empty_layer"}, {"op": "add_empty_layer"}]}),
    )
    .unwrap();
    let added: Vec<uuid::Uuid> = serde_json::from_value(answer["layers"].clone()).unwrap();
    let order = |app: &EditorApp| -> Vec<uuid::Uuid> {
        app.session()
            .unwrap()
            .document
            .layers
            .iter()
            .map(|l| l.id)
            .collect()
    };
    let before = order(&app);
    let steps = app.session().unwrap().history.names().count();
    plugin_request(
        &mut app,
        "document/edit",
        json!({"name": "Move", "edits": [{"op": "move_layer", "layer": added[1], "below": before[0]}]}),
    )
    .unwrap();
    assert_eq!(order(&app), [added[1], before[0], added[0]]);
    assert_eq!(app.session().unwrap().history.names().count(), steps + 1);
    app.command("undo");
    assert_eq!(order(&app), before);
}

#[test]
fn canvas_ops_are_refused_in_results_but_selection_ops_are_proposed() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [16, 16];
    app.new_document();
    let job = mock_job(&app);
    for op in [
        json!({"op": "crop", "x": 0, "y": 0, "width": 4, "height": 4}),
        json!({"op": "resize_canvas", "width": 4, "height": 4}),
        json!({"op": "resize_image", "width": 4, "height": 4}),
    ] {
        let result = json!({"outputs": [{"kind": "edit", "edits": [op.clone()]}]});
        let error = format!("{:#}", app.apply_job_result(&job, result).unwrap_err());
        assert!(error.contains("document/edit"), "{op}: {error}");
    }
    assert_eq!(app.session().unwrap().document.width, 16);
    assert!(app.plugins.proposal.is_none());

    // A read-only plugin may propose a selection made with the new ops.
    install_read_mock(&mut app, dir.path());
    let result = json!({"outputs": [{"kind": "edit", "edits": [
        {"op": "select_rect", "x": 0, "y": 0, "width": 8, "height": 8, "ellipse": true},
        {"op": "feather_selection", "radius": 1},
    ]}]});
    app.apply_job_result(&job, result).unwrap();
    assert!(app.plugins.proposal.is_some());
    assert!(app.session().unwrap().document.selection.is_some());
    app.resolve_proposal(false);
    assert!(app.session().unwrap().document.selection.is_none());
    let result =
        json!({"outputs": [{"kind": "edit", "edits": [{"op": "fill", "color": "#000000"}]}]});
    let error = format!("{:#}", app.apply_job_result(&job, result).unwrap_err());
    assert!(error.contains("document = \"edit\""), "{error}");
}

#[test]
fn plugins_list_and_switch_documents_and_greyed_out_commands_are_refused() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_read_mock(&mut app, dir.path());
    assert_eq!(
        plugin_request(&mut app, "document/list", json!({})).unwrap()["documents"],
        json!([])
    );
    app.dimensions = [10, 8];
    app.new_document();
    app.dimensions = [20, 16];
    app.new_document();
    let first = app.sessions[0].document.id;
    let listed = plugin_request(&mut app, "document/list", json!({})).unwrap();
    let documents = listed["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0]["width"], 10);
    assert_eq!(documents[1]["current"], true);
    assert!(documents[0].get("path").is_none(), "paths stay private");
    // Switching tabs is a view change: a read-only plugin may.
    plugin_request(&mut app, "document/activate", json!({"document": first})).unwrap();
    assert_eq!(app.current, 0);
    for bad in [json!({}), json!({"document": uuid::Uuid::new_v4()})] {
        assert!(plugin_request(&mut app, "document/activate", bad).is_err());
    }
    // One switch a second: a client cannot make the tabs flicker.
    let second = app.sessions[1].document.id;
    let error =
        plugin_request(&mut app, "document/activate", json!({"document": second})).unwrap_err();
    assert_eq!(error.code, xuan::plugins::protocol::RATE_LIMITED);
    assert!(error.data["retry_after"].as_f64().unwrap() > 0.0);
    assert_eq!(app.current, 0, "not switched");
    // Asking for the current document is free.
    plugin_request(&mut app, "document/activate", json!({"document": first})).unwrap();
    let past = std::time::Instant::now() - crate::app::plugins::ACTIVATE_INTERVAL;
    app.plugins.activated_at.insert("mock".into(), past);
    plugin_request(&mut app, "document/activate", json!({"document": second})).unwrap();
    assert_eq!(app.current, 1);
    // A command greyed out in its menu is refused rather than ignored.
    install_mock(&mut app, dir.path());
    let request = |action: &str| xuan::plugins::protocol::Request {
        jsonrpc: "2.0".into(),
        id: xuan::plugins::protocol::Id::Number(1),
        method: "host/run".into(),
        params: json!({"action": action}),
    };
    let error = app
        .service_request("mock", &request("select_mask_black"))
        .unwrap_err();
    assert!(error.message.contains("not available"), "{}", error.message);
    assert!(app.service_request("mock", &request("select_all")).is_ok());
    assert!(app.service_request("mock", &request("feather")).is_ok());
    // File requests never run straight from `service_request`.
    let error = (app.service_request(
        "mock",
        &xuan::plugins::protocol::Request {
            method: "file/save_as".into(),
            ..request("x")
        },
    ))
    .unwrap_err();
    assert!(error.message.contains("user"), "{}", error.message);
}

/// The mock plugin with `document = "edit"` and `edit_prompt = "session"`.
fn install_session_mock(app: &mut EditorApp, dir: &Path) {
    install_mock(app, dir);
    let mut manifest = app.plugins.manifest("mock").unwrap().clone();
    manifest.permissions.edit_prompt = xuan::plugins::manifest::EditPrompt::Session;
    app.install_plugins(vec![manifest], vec![]);
    app.grant_plugin("mock", true);
}

fn session_request(method: &str, params: serde_json::Value) -> xuan::plugins::protocol::Request {
    xuan::plugins::protocol::Request {
        jsonrpc: "2.0".into(),
        id: xuan::plugins::protocol::Id::Number(1),
        method: method.into(),
        params,
    }
}

#[test]
fn edit_sessions_gate_direct_edits_until_the_user_allows_them() {
    use crate::app::plugin_sessions::{EditAnswer, EditSessionRequest};
    use serde_json::json;
    use xuan::plugins::protocol::CANCELLED;
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    app.config_path = Some(config.path().join("config.toml"));
    // Without the manifest's edit_prompt, nothing changes.
    install_mock(&mut app, dir.path());
    app.dimensions = [16, 16];
    app.new_document();
    let edit = |session: &str| {
        session_request(
            "document/edit",
            json!({"name": "Agent", "session": session, "edits": [{"op": "add_empty_layer"}]}),
        )
    };
    assert!(app.service_request("mock", &edit("")).is_ok());
    assert!(!app.gated_edit("mock", &edit("")));

    install_session_mock(&mut app, dir.path());
    let layers = |app: &EditorApp| app.session().unwrap().document.layers.len();
    let before = layers(&app);
    // Direct edits are refused until the session is answered; reading and
    // moving the view are not held.
    for request in [
        edit(""),
        session_request("host/run", json!({"action": "new_layer"})),
        session_request("host/run", json!({"action": "undo"})),
    ] {
        assert!(app.gated_edit("mock", &request), "{}", request.method);
        assert_eq!(
            app.service_request("mock", &request).unwrap_err().code,
            CANCELLED
        );
    }
    assert_eq!(layers(&app), before);
    for request in [
        session_request("document/get", json!({})),
        session_request("host/run", json!({"action": "zoom_in"})),
        session_request("document/list", json!({})),
    ] {
        assert!(!app.gated_edit("mock", &request));
        assert!(app.service_request("mock", &request).is_ok());
    }
    assert_eq!(
        app.service_request("mock", &session_request("session/status", json!({})))
            .unwrap(),
        json!({"edit_prompt": "session", "edits": "ask", "auto": false, "save_auto": false})
    );

    // Allow holds for its session only; Deny refuses the session.
    let prompt = |session: &str| EditSessionRequest {
        plugin: "mock".into(),
        session: session.into(),
        edit: "Agent".into(),
        deny_all: false,
    };
    app.plugins.edit_prompt = Some(prompt("a"));
    app.answer_edit_session(EditAnswer::Allow);
    assert!(app.service_request("mock", &edit("a")).is_ok());
    assert_eq!(layers(&app), before + 1);
    assert!(app.service_request("mock", &edit("b")).is_err());
    app.plugins.edit_prompt = Some(prompt("b"));
    app.answer_edit_session(EditAnswer::Deny);
    assert_eq!(
        app.service_request("mock", &edit("b")).unwrap_err().code,
        CANCELLED
    );
    assert_eq!(
        app.service_request(
            "mock",
            &session_request("session/status", json!({"session": "b"}))
        )
        .unwrap()["edits"],
        "denied"
    );
    // The answers last until the process stops.
    app.stop_plugin("mock");
    assert!(app.service_request("mock", &edit("a")).is_err());

    // Always Allow is auto mode, stored in the grant and saved.
    app.plugins.edit_prompt = Some(prompt("c"));
    app.answer_edit_session(EditAnswer::Always);
    assert!(app.stored_grant("mock").unwrap().edit_without_asking);
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
    assert!(saved.contains("edit_without_asking = true"), "{saved}");
    for session in ["", "c", "d"] {
        assert!(
            app.service_request("mock", &edit(session)).is_ok(),
            "{session}"
        );
    }
    assert_eq!(
        app.service_request("mock", &session_request("session/status", json!({})))
            .unwrap(),
        json!({"edit_prompt": "session", "edits": "allowed", "auto": true, "save_auto": false})
    );
    // Turning it off in Manage Plugins asks again, even in allowed sessions.
    app.set_edit_auto_mode("mock", false);
    assert!(!app.stored_grant("mock").unwrap().edit_without_asking);
    assert!(app.service_request("mock", &edit("c")).is_err());
    app.set_edit_auto_mode("mock", true);
    assert!(app.service_request("mock", &edit("c")).is_ok());
    // Reviewed again with other permissions, the grant starts without it.
    let mut changed = app.plugins.manifest("mock").unwrap().clone();
    changed.permissions.secrets = vec!["token".into()];
    app.install_plugins(vec![changed], vec![]);
    assert!(!app.plugin_granted("mock"));
    app.grant_plugin("mock", true);
    assert!(!app.stored_grant("mock").unwrap().edit_without_asking);
    assert!(app.service_request("mock", &edit("c")).is_err());
    // Unchanged permissions keep it.
    app.set_edit_auto_mode("mock", true);
    app.grant_plugin("mock", true);
    assert!(app.stored_grant("mock").unwrap().edit_without_asking);
}

#[test]
fn host_run_picks_its_layers_and_reports_what_it_added_and_started() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [32, 24];
    app.new_document();
    app.command("fill_fg");
    let base = app.session().unwrap().document.layers[0].id;
    let ids = |answer: &serde_json::Value| -> Vec<uuid::Uuid> {
        serde_json::from_value(answer["layers"].clone()).unwrap()
    };
    let describe = |app: &mut EditorApp, id: uuid::Uuid| {
        let document = plugin_request(app, "document/get", json!({})).unwrap();
        (document["layers"].as_array().unwrap().iter())
            .find(|layer| layer["id"] == json!(id))
            .cloned()
            .unwrap()
    };

    // New layers come back with their ids.
    let answer = plugin_request(&mut app, "host/run", json!({"action": "new_layer"})).unwrap();
    let new = ids(&answer);
    assert_eq!(new.len(), 1);
    assert_eq!(app.session().unwrap().document.active, Some(new[0]));
    assert_eq!(answer["running"], false);

    // `layers` chooses what the command acts on.
    let answer = plugin_request(
        &mut app,
        "host/run",
        json!({"action": "duplicate", "layers": [base]}),
    )
    .unwrap();
    let copies = ids(&answer);
    assert_eq!(copies.len(), 1);
    let document = &app.session().unwrap().document;
    let copy = document.layers.iter().find(|l| l.id == copies[0]).unwrap();
    assert_eq!(copy.pixels, document.layers[0].pixels, "a copy of the base");
    plugin_request(
        &mut app,
        "host/run",
        json!({"action": "flip_h", "layers": [base]}),
    )
    .unwrap();
    let flipped = describe(&mut app, base);
    assert_eq!(
        (&flipped["flip_x"], &flipped["flip_y"]),
        (&json!(true), &json!(false))
    );

    // A mask attached to the image is reported on it and exported through it.
    let answer = plugin_request(
        &mut app,
        "host/run",
        json!({"action": "mask", "layers": [base]}),
    )
    .unwrap();
    let mask = ids(&answer);
    assert_eq!(mask.len(), 1);
    let image = describe(&mut app, base);
    assert_eq!(image["has_mask"], true);
    assert_eq!(
        image["masks"],
        json!([{"layer": mask[0], "enabled": true, "linked": true}])
    );
    assert_eq!(describe(&mut app, mask[0])["attached_to"], json!(base));
    plugin_request(
        &mut app,
        "host/run",
        json!({"action": "disable_mask", "layers": [mask[0]]}),
    )
    .unwrap();
    assert_eq!(describe(&mut app, base)["masks"][0]["enabled"], false);
    let export = plugin_request(
        &mut app,
        "layer/export",
        json!({"layer": base, "what": "mask"}),
    )
    .unwrap();
    assert_eq!(export["mask_layer"], json!(mask[0]));
    let _ = std::fs::remove_file(export["path"].as_str().unwrap());

    // Bad layers, or layers with a command that does not edit, change nothing.
    let steps = app.session().unwrap().history.names().count();
    let active = app.session().unwrap().document.active;
    for params in [
        json!({"action": "invert", "layers": [uuid::Uuid::new_v4()]}),
        json!({"action": "invert", "layers": "nope"}),
        json!({"action": "zoom_in", "layers": [base]}),
        json!({"action": "mock/echo", "layers": [base]}),
    ] {
        assert!(plugin_request(&mut app, "host/run", params).is_err());
    }
    // A command greyed out for the chosen layer leaves the old one active.
    let error = plugin_request(
        &mut app,
        "host/run",
        json!({"action": "select_mask_black", "layers": [copies[0]]}),
    )
    .unwrap_err();
    assert!(error.message.contains("not available"), "{}", error.message);
    assert_eq!(app.session().unwrap().document.active, active);
    assert_eq!(app.session().unwrap().history.names().count(), steps);

    // A command that starts a job says it is still running.
    let answer = plugin_request(
        &mut app,
        "host/run",
        json!({"action": "remove_flat_background", "layers": [base]}),
    )
    .unwrap();
    assert_eq!(answer["running"], true);
    assert!(app.job.is_some());
}

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

    /// Remembers the API key it is given at start-up and reports it in the
    /// provenance of its result, under several names.
    const LEAKY: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      key=$(printf '%s' "$line" | sed -n 's/.*"secrets":{"key":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":1}}\n' "$id" ;;
    *'"method":"action/estimate"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"cost":"free"}}\n' "$id" ;;
    *'"method":"action/run"'*)
      work=$(printf '%s' "$line" | sed -n 's/.*"work_dir":"\([^"]*\)".*/\1/p')
      src=$(printf '%s' "$line" | sed -n 's/.*"source":{.*"path":"\([^"]*\)".*/\1/p')
      cp "$src" "$work/result.png"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"outputs":[{"kind":"image","path":"%s/result.png","name":"Leaky","provenance":{"model":"m-1","seed":5,"api_key":"%s","extra":{"url":"https://x/?k=%s","fine":true}}}]}}\n' "$id" "$work" "$key" "$key" ;;
    *'"method":"shutdown"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"; exit 0 ;;
  esac
done
"#;

    #[test]
    fn a_plugin_that_reports_its_api_key_in_provenance_has_it_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_secret_mock(&mut app, dir.path());
        std::fs::write(dir.path().join("plugin.sh"), LEAKY).unwrap();
        app.dimensions = [16, 16];
        app.new_document();
        app.command("fill_fg");
        frame(&context, &mut app);
        app.start_plugin_action("mock", "echo");
        app.add_region(Point::new(1.0, 1.0), Point::new(8.0, 8.0));
        app.run_plugin_action();
        assert!(app.error.is_none(), "{:?}", app.error);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        assert!(app.error.is_none(), "{:?}", app.error);
        app.resolve_proposal(true);
        let document = &app.session().unwrap().document;
        let layer = document.layers.iter().find(|l| l.name == "Leaky").unwrap();
        let record = layer.provenance.as_ref().unwrap();
        assert_eq!(
            (record.model.as_deref(), record.seed),
            (Some("m-1"), Some(5))
        );
        // The key, which did reach the plugin, is nowhere in the document or
        // the status line; only the harmless entry is left.
        assert_eq!(record.extra.len(), 1);
        assert_eq!(record.extra["fine"], true);
        assert!(!serde_json::to_string(document).unwrap().contains(SECRET));
        assert!(!app.status.contains(SECRET), "{}", app.status);
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
    fn a_stopped_plugin_pane_drops_its_old_page_and_reopens_or_explains() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_mock(&mut app, dir.path());
        app.dimensions = [16, 16];
        app.new_document();
        frame(&context, &mut app);
        let key = "plugin:mock/info";
        run_until(&context, &mut app, |app| {
            app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        assert!(app.plugins.running("mock"));
        // A deliberate stop leaves no stale page behind.
        app.stop_plugin("mock");
        assert!(!app.plugins.running("mock"));
        assert!(app.plugins.panes[key].tree.is_none());
        // Drawing the pane again starts the plugin and shows a fresh page.
        run_until(&context, &mut app, |app| {
            app.plugins.running("mock")
                && app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        // While not granted, a stop leaves no page and drawing does not restart it.
        app.stop_plugin("mock");
        app.grant_plugin("mock", false);
        for _ in 0..10 {
            frame(&context, &mut app);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.plugins.panes[key].tree.is_none());
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
        install_read_mock(&mut app, dir.path());
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
        install_read_mock(&mut app, dir.path());
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
    fn an_action_with_a_selection_mask_sends_it_in_the_source_size() {
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        app.config_path = Some(config.path().join("config.toml"));
        let fixture = dir.path().join("fixture.png");
        RgbaImage::new(8, 8).save(&fixture).unwrap();
        std::fs::write(dir.path().join("plugin.sh"), script(&fixture)).unwrap();
        std::fs::write(
            dir.path().join("plugin.toml"),
            format!("{MANIFEST}{INPAINT}"),
        )
        .unwrap();
        let mut manifest = Manifest::load(dir.path()).unwrap();
        manifest.permissions.document = DocumentAccess::Edit;
        app.install_plugins(vec![manifest], vec![]);
        app.grant_plugin("mock", true);
        app.dimensions = [16, 16];
        app.new_document();
        app.command("fill_fg");

        // Nothing is selected: the action refuses before anything is sent.
        app.start_plugin_action("mock", "inpaint");
        assert!(app.error.take().is_some());
        assert!(app.plugins.jobs.is_empty());
        settle(&context, &mut app);
        assert!(!received(dir.path()).contains("action/run"));

        let selection = selection_where(&app, |x, y| x < 8 && y < 4);
        app.session_mut().unwrap().document.selection = Some(selection);
        app.start_plugin_action("mock", "inpaint");
        assert_eq!(app.plugins.jobs.len(), 1);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let log = received(dir.path());
        let run = log.lines().find(|l| l.contains("action/run")).unwrap();
        let mask = run
            .split("\"mask\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        assert!(mask.ends_with("selection.png"), "{run}");
        // The source is the 16x16 composite, which max_side 512 does not scale.
        assert!(run.contains("\"width\":16"), "{run}");
    }

    #[test]
    fn an_outpaint_action_extends_the_canvas_through_the_mock_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        app.config_path = Some(config.path().join("config.toml"));
        let fixture = dir.path().join("fixture.png");
        RgbaImage::new(8, 8).save(&fixture).unwrap();
        std::fs::write(dir.path().join("plugin.sh"), script(&fixture)).unwrap();
        std::fs::write(dir.path().join("plugin.toml"), outpaint_manifest(true)).unwrap();
        app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
        app.grant_plugin("mock", true);
        app.dimensions = [8, 8];
        app.new_document();
        app.command("fill_fg");
        let steps = app.session().unwrap().history.names().count();
        let source = app.session().unwrap().document.layers[0].id;

        app.start_plugin_action("mock", "outpaint");
        assert!(app.plugins.action.is_some(), "{:?}", app.error);
        app.run_plugin_action();
        assert_eq!(app.plugins.jobs.len(), 1, "{:?}", app.error);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let log = received(dir.path());
        let run = log.lines().find(|l| l.contains("action/run")).unwrap();
        // The padded composite and the mask of its new area, 12 x 16.
        assert!(run.contains("extend.png"), "{run}");
        assert!(
            run.contains("\"width\":12") && run.contains("\"height\":16"),
            "{run}"
        );
        let document = &app.session().unwrap().document;
        assert_eq!((document.width, document.height), (12, 16));
        let base = document.layers.iter().find(|l| l.id == source).unwrap();
        assert_eq!((base.transform.x, base.transform.y), (4.0, 2.0));
        let added = document
            .layers
            .iter()
            .find(|l| l.name == "Outpainted")
            .unwrap();
        let t = added.transform;
        assert_eq!((t.x, t.y, t.width, t.height), (0.0, 0.0, 12.0, 16.0));
        // The source sent back: transparent new canvas around the old image.
        let pixels = added.pixels.as_ref().unwrap();
        assert_eq!(pixels.dimensions(), (12, 16));
        assert_eq!(pixels.get_pixel(1, 1)[3], 0);
        assert_eq!(pixels.get_pixel(6, 5)[3], 255);
        app.resolve_proposal(true);
        let session = app.session().unwrap();
        assert_eq!(session.history.names().count(), steps + 1);
        assert_eq!(session.history.undo_name(), Some("Outpaint"));
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

    /// The mock plugin's answers: the lines Xuan sent it with this id.
    fn answer(dir: &Path, id: i64) -> Option<serde_json::Value> {
        received(dir).lines().find_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            (value.get("id") == Some(&serde_json::json!(id)) && value.get("method").is_none())
                .then_some(value)
        })
    }

    fn file_request(
        id: i64,
        method: &str,
        params: serde_json::Value,
    ) -> xuan::plugins::protocol::Request {
        xuan::plugins::protocol::Request {
            jsonrpc: "2.0".into(),
            id: xuan::plugins::protocol::Id::Number(id),
            method: method.into(),
            params,
        }
    }

    #[test]
    fn plugins_save_export_and_open_files_only_through_the_user() {
        use crate::app::plugin_files::SaveDialog;
        use serde_json::json;
        use std::sync::{Arc, Mutex};
        use xuan::plugins::protocol::{CANCELLED, INVALID_PARAMS, INVALID_REQUEST};
        let dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        // Any plugin may ask: the user decides.
        install_read_mock(&mut app, dir.path());
        app.dimensions = [16, 12];
        app.new_document();
        app.command("fill_fg");
        app.render_pane("plugin:mock/info", "open", None);
        run_until(&context, &mut app, |app| {
            app.plugins.running("mock") && !app.plugins.starting("mock")
        });
        let shown: Arc<Mutex<Vec<SaveDialog>>> = Arc::default();
        let choice: Arc<Mutex<Option<PathBuf>>> = Arc::default();
        let (seen, chosen) = (shown.clone(), choice.clone());
        app.plugins.save_dialog = Some(Arc::new(move |dialog: &SaveDialog| {
            seen.lock().unwrap().push(dialog.clone());
            chosen.lock().unwrap().clone()
        }));
        let wait = |context: &egui::Context, app: &mut EditorApp, id: i64| {
            run_until(context, app, |_| answer(dir.path(), id).is_some());
            answer(dir.path(), id).unwrap()
        };

        // Cancelling the save dialog saves nothing.
        app.queue_file_request(
            "mock",
            file_request(
                101,
                "file/save_as",
                json!({"suggested_name": "../evil/Agent work.png"}),
            ),
        );
        let cancelled = wait(&context, &mut app, 101);
        assert_eq!(cancelled["error"]["code"], CANCELLED);
        let dialog = shown.lock().unwrap()[0].clone();
        assert_eq!(dialog.file_name, "Agent work.xuan");
        assert!(
            dialog.title.contains("Mock (plugin mock)"),
            "{}",
            dialog.title
        );
        assert!(app.session().unwrap().path.is_none());

        // Right after a cancel, the plugin may not ask again.
        app.queue_file_request("mock", file_request(110, "file/save_as", json!({})));
        assert_eq!(wait(&context, &mut app, 110)["error"]["code"], CANCELLED);
        assert_eq!(shown.lock().unwrap().len(), 1, "no second dialog");
        let past = std::time::Instant::now() - crate::app::plugin_sessions::COOLDOWN;
        app.plugins.file_refused_at.insert("mock".into(), past);

        // Saving where the user chose: the project now lives there.
        let project = out.path().join("agent.xuan");
        *choice.lock().unwrap() = Some(project.clone());
        app.queue_file_request("mock", file_request(102, "file/save_as", json!({})));
        let saved = wait(&context, &mut app, 102);
        assert_eq!(saved["result"], json!({"name": "agent.xuan"}), "no folder");
        assert!(project.exists());
        let session = app.session().unwrap();
        assert_eq!(session.path.as_deref(), Some(project.as_path()));
        assert_eq!(session.title, "agent");
        assert!(!session.history.dirty());

        // Exporting writes an image and leaves the project alone.
        let image = out.path().join("export.jpg");
        *choice.lock().unwrap() = Some(image.clone());
        app.queue_file_request(
            "mock",
            file_request(
                103,
                "file/export",
                json!({"format": "jpeg", "suggested_name": "Shot"}),
            ),
        );
        let exported = wait(&context, &mut app, 103);
        assert_eq!(exported["result"], json!({"name": "export.jpg"}));
        assert_eq!(
            xuan::io::import_image(&image).unwrap().dimensions(),
            (16, 12)
        );
        let dialog = shown.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            (dialog.file_name.as_str(), dialog.extensions.as_slice()),
            ("Shot.jpg", ["jpg".to_owned()].as_slice())
        );
        assert_eq!(
            app.session().unwrap().path.as_deref(),
            Some(project.as_path())
        );
        app.queue_file_request(
            "mock",
            file_request(104, "file/export", json!({"format": "gif"})),
        );
        assert_eq!(
            wait(&context, &mut app, 104)["error"]["code"],
            INVALID_PARAMS
        );

        // Opening asks first, naming the file; Cancel opens nothing.
        let sessions = app.sessions.len();
        app.queue_file_request(
            "mock",
            file_request(105, "file/open", json!({"path": image})),
        );
        app.queue_file_request(
            "mock",
            file_request(106, "file/open", json!({"path": image})),
        );
        // One request at a time.
        assert_eq!(
            wait(&context, &mut app, 106)["error"]["code"],
            INVALID_REQUEST
        );
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginFile)
        });
        let canonical = std::fs::canonicalize(&image).unwrap();
        assert!(matches!(
            &app.plugins.file_prompt.as_ref().unwrap().action,
            crate::app::plugin_files::FileAction::Open { path, .. } if *path == canonical
        ));
        app.answer_file_prompt(false);
        assert_eq!(wait(&context, &mut app, 105)["error"]["code"], CANCELLED);
        assert_eq!(app.sessions.len(), sessions);
        app.plugins.file_refused_at.insert("mock".into(), past);
        app.queue_file_request(
            "mock",
            file_request(107, "file/open", json!({"path": image})),
        );
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginFile)
        });
        app.answer_file_prompt(true);
        let opened = wait(&context, &mut app, 107);
        assert_eq!(opened["result"]["ok"], true);
        assert_eq!(app.sessions.len(), sessions + 1);
        assert_eq!(
            opened["result"]["document"],
            json!(app.session().unwrap().document.id)
        );
        // A file swapped while the prompt is open is not opened.
        let decoy = out.path().join("decoy.png");
        std::fs::copy(&image, &decoy).unwrap();
        let link = out.path().join("link.png");
        std::os::unix::fs::symlink(&image, &link).unwrap();
        app.plugins.file_refused_at.clear();
        app.queue_file_request(
            "mock",
            file_request(111, "file/open", json!({"path": link})),
        );
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginFile)
        });
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&decoy, &link).unwrap();
        let sessions = app.sessions.len();
        app.answer_file_prompt(true);
        let refused = wait(&context, &mut app, 111);
        assert_eq!(refused["error"]["code"], INVALID_PARAMS, "{refused}");
        assert_eq!(app.sessions.len(), sessions);
        // Missing and unsuitable paths get the same answer: no probing.
        let mut messages = Vec::new();
        for (id, path) in [
            (112, out.path().join("missing.png")),
            (113, "/dev/null".into()),
        ] {
            app.queue_file_request("mock", file_request(id, "file/open", json!({"path": path})));
            messages.push(wait(&context, &mut app, id)["error"]["message"].clone());
        }
        assert_eq!(messages[0], messages[1]);
        assert!(!messages[0].to_string().contains("missing.png"));
        for (id, path) in [
            (108, json!("relative.png")),
            (109, json!(out.path().join("missing.png"))),
        ] {
            app.queue_file_request("mock", file_request(id, "file/open", json!({"path": path})));
            assert_eq!(
                wait(&context, &mut app, id)["error"]["code"],
                INVALID_PARAMS
            );
        }
        assert!(app.dialog.is_none());
        app.stop_plugin("mock");
    }

    #[test]
    fn plugin_saves_land_only_where_the_user_confirmed_and_errors_name_no_folders() {
        use crate::app::plugin_files::SaveDialog;
        use serde_json::json;
        use std::sync::{Arc, Mutex};
        use xuan::plugins::protocol::{CANCELLED, INTERNAL_ERROR, INVALID_PARAMS};
        let dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_read_mock(&mut app, dir.path());
        app.dimensions = [8, 6];
        app.new_document();
        app.render_pane("plugin:mock/info", "open", None);
        run_until(&context, &mut app, |app| {
            app.plugins.running("mock") && !app.plugins.starting("mock")
        });
        // The dialog's answers, in turn.
        let shown: Arc<Mutex<Vec<SaveDialog>>> = Arc::default();
        let answers: Arc<Mutex<Vec<Option<PathBuf>>>> = Arc::default();
        let (seen, queue) = (shown.clone(), answers.clone());
        app.plugins.save_dialog = Some(Arc::new(move |dialog: &SaveDialog| {
            seen.lock().unwrap().push(dialog.clone());
            queue.lock().unwrap().remove(0)
        }));
        let wait = |context: &egui::Context, app: &mut EditorApp, id: i64| {
            run_until(context, app, |_| answer(dir.path(), id).is_some());
            answer(dir.path(), id).unwrap()
        };
        let ask = |app: &mut EditorApp, id: i64, method: &str, choices: Vec<Option<PathBuf>>| {
            app.plugins.file_refused_at.clear();
            shown.lock().unwrap().clear();
            *answers.lock().unwrap() = choices;
            app.queue_file_request("mock", file_request(id, method, json!({"format": "png"})));
            let answer = wait(&context, app, id);
            assert!(answers.lock().unwrap().is_empty(), "every answer used");
            (answer, shown.lock().unwrap().clone())
        };
        let files = || {
            let mut names: Vec<String> = std::fs::read_dir(out.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };

        // A name without the extension is not changed silently: the dialog
        // asks again, in the same folder, with the extension added, and the
        // file is written where that answer says.
        let (saved, dialogs) = ask(
            &mut app,
            301,
            "file/save_as",
            vec![
                Some(out.path().join("agent")),
                Some(out.path().join("agent.xuan")),
            ],
        );
        assert_eq!(saved["result"], json!({"name": "agent.xuan"}), "{saved}");
        assert_eq!(dialogs.len(), 2);
        assert_eq!(dialogs[1].file_name, "agent.xuan");
        assert_eq!(dialogs[1].directory.as_deref(), Some(out.path()));
        assert_eq!(files(), ["agent.xuan"]);

        // Choosing a name without it twice writes nothing.
        let (refused, _) = ask(
            &mut app,
            302,
            "file/save_as",
            vec![
                Some(out.path().join("plain")),
                Some(out.path().join("plain")),
            ],
        );
        assert_eq!(refused["error"]["code"], INVALID_PARAMS, "{refused}");
        let message = refused["error"]["message"].as_str().unwrap();
        assert!(message.contains(".xuan"), "{message}");
        assert!(!message.contains(out.path().to_str().unwrap()), "{message}");
        assert_eq!(files(), ["agent.xuan"]);

        // An export under an unknown extension asks again too; cancelling
        // that writes nothing.
        let (cancelled, dialogs) = ask(
            &mut app,
            303,
            "file/export",
            vec![Some(out.path().join("shot.bmp")), None],
        );
        assert_eq!(cancelled["error"]["code"], CANCELLED);
        assert_eq!(dialogs[1].file_name, "shot.bmp.png");
        assert_eq!(files(), ["agent.xuan"]);

        // A known extension in any case is written exactly there.
        let (exported, dialogs) = ask(
            &mut app,
            304,
            "file/export",
            vec![Some(out.path().join("shot.PNG"))],
        );
        assert_eq!(exported["result"], json!({"name": "shot.PNG"}));
        assert_eq!(dialogs.len(), 1);
        assert_eq!(files(), ["agent.xuan", "shot.PNG"]);

        // Errors from writing name the file, never its folders.
        let missing = out.path().join("private folder").join("deeper");
        let (failed, _) = ask(
            &mut app,
            305,
            "file/export",
            vec![Some(missing.join("lost.png"))],
        );
        assert_eq!(failed["error"]["code"], INTERNAL_ERROR, "{failed}");
        let message = failed["error"]["message"].as_str().unwrap();
        assert!(!message.contains("private folder"), "{message}");
        assert!(!message.contains(out.path().to_str().unwrap()), "{message}");

        // Nor does an open that fails show where a link led.
        let hidden = out.path().join("secret place");
        std::fs::create_dir(&hidden).unwrap();
        std::fs::write(hidden.join("broken.png"), b"not a png").unwrap();
        let link = dir.path().join("link.png");
        std::os::unix::fs::symlink(hidden.join("broken.png"), &link).unwrap();
        app.plugins.file_refused_at.clear();
        app.queue_file_request(
            "mock",
            file_request(306, "file/open", json!({"path": link})),
        );
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginFile)
        });
        app.answer_file_prompt(true);
        let opened = wait(&context, &mut app, 306);
        let message = opened["error"]["message"].as_str().unwrap();
        assert!(message.contains("broken.png"), "{message}");
        assert!(!message.contains("secret place"), "{message}");
        app.error = None;
        app.stop_plugin("mock");
    }

    #[test]
    fn held_edits_wait_for_the_session_prompt_and_apply_in_order() {
        use crate::app::plugin_sessions::EditAnswer;
        use serde_json::json;
        use xuan::plugins::protocol::CANCELLED;
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_session_mock(&mut app, dir.path());
        app.dimensions = [16, 16];
        app.new_document();
        app.render_pane("plugin:mock/info", "open", None);
        run_until(&context, &mut app, |app| {
            app.plugins.running("mock") && !app.plugins.starting("mock")
        });
        let steps = app.session().unwrap().history.names().count();
        let edit = |id: i64, name: &str, session: &str| {
            file_request(
                id,
                "document/edit",
                json!({"name": name, "session": session, "edits": [{"op": "add_empty_layer", "name": name}]}),
            )
        };
        // Two edits of one session wait behind a single prompt.
        assert!(app.hold_edit("mock", edit(201, "First", "s1")).is_none());
        assert!(app.hold_edit("mock", edit(202, "Second", "s1")).is_none());
        // Reading is never held.
        let read = file_request(203, "document/get", json!({"session": "s1"}));
        assert!(app.hold_edit("mock", read).is_some());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        let prompt = app.plugins.edit_prompt.clone().unwrap();
        assert_eq!(
            (prompt.session.as_str(), prompt.edit.as_str()),
            ("s1", "First")
        );
        assert_eq!(app.session().unwrap().history.names().count(), steps);
        app.answer_edit_session(EditAnswer::Allow);
        run_until(&context, &mut app, |_| answer(dir.path(), 202).is_some());
        assert!(answer(dir.path(), 201).unwrap()["result"]["layers"].is_array());
        let session = app.session().unwrap();
        // Each edit is its own undo step, in order.
        assert_eq!(session.history.names().count(), steps + 2);
        assert_eq!(session.history.undo_name(), Some("Second"));
        // A new session asks again; Deny refuses it.
        assert!(app.hold_edit("mock", edit(204, "Third", "s2")).is_none());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        app.answer_edit_session(EditAnswer::Deny);
        run_until(&context, &mut app, |_| answer(dir.path(), 204).is_some());
        assert_eq!(answer(dir.path(), 204).unwrap()["error"]["code"], CANCELLED);
        // The allowed session goes on without asking.
        assert!(app.hold_edit("mock", edit(205, "Fourth", "s1")).is_some());
        // Right after Deny, a new session is refused without a prompt.
        assert!(app.hold_edit("mock", edit(207, "Sneaky", "s9")).is_none());
        run_until(&context, &mut app, |_| answer(dir.path(), 207).is_some());
        assert_eq!(answer(dir.path(), 207).unwrap()["error"]["code"], CANCELLED);
        assert!(app.plugins.edit_prompt.is_none() && app.plugins.edit_held.is_empty());
        // Later it may ask again; "refuse every session" then holds until
        // the plugin stops.
        let past = std::time::Instant::now() - crate::app::plugin_sessions::COOLDOWN;
        app.plugins.edit_refused_at.insert("mock".into(), past);
        assert!(app.hold_edit("mock", edit(208, "Again", "s8")).is_none());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        app.plugins.edit_prompt.as_mut().unwrap().deny_all = true;
        app.answer_edit_session(EditAnswer::Deny);
        run_until(&context, &mut app, |_| answer(dir.path(), 208).is_some());
        app.plugins.edit_refused_at.insert("mock".into(), past);
        assert!(
            app.hold_edit("mock", edit(209, "More", "s7")).is_some(),
            "answered: refused"
        );
        assert_eq!(app.edit_answer("mock", "s7"), Some(false));
        // The allowed session is refused too.
        assert_eq!(app.edit_answer("mock", "s1"), Some(false));
        app.plugins.edits_refused.clear();
        // Stopping the plugin drops what waits and closes its prompt.
        assert!(app.hold_edit("mock", edit(206, "Fifth", "s3")).is_none());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        app.stop_plugin("mock");
        frame(&context, &mut app);
        assert!(app.plugins.edit_held.is_empty());
        assert!(app.plugins.edit_prompt.is_none());
        assert!(app.dialog.is_none());
    }

    #[test]
    fn the_session_prompt_names_the_plugin_and_always_allow_turns_on_auto_mode() {
        use crate::app::tests::ui::UiTest;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(config.path());
        install_session_mock(ui.app_mut(), dir.path());
        ui.app_mut().render_pane("plugin:mock/info", "open", None);
        for _ in 0..200 {
            ui.settle();
            if ui.app().plugins.running("mock") && !ui.app().plugins.starting("mock") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let request = file_request(
            301,
            "document/edit",
            json!({"name": "Agent paint", "edits": [{"op": "add_empty_layer"}]}),
        );
        assert!(ui.app_mut().hold_edit("mock", request).is_none());
        ui.settle();
        assert!(ui.has("Allow Mock (plugin mock) to edit your documents for this session?"));
        assert!(ui.has("“Agent paint”"));
        ui.click("Always Allow");
        assert!(ui.app().edits_without_asking("mock"));
        assert!(ui.app().plugins.edit_prompt.is_none());
        assert_eq!(
            ui.app().session().unwrap().history.undo_name(),
            Some("Agent paint")
        );
        // Manage Plugins shows the switch, which turns auto mode off.
        ui.app_mut().command("plugins");
        ui.app_mut().plugins.manager_selected = Some("mock".into());
        ui.settle();
        ui.click_role(
            egui::accesskit::Role::CheckBox,
            "Edit without asking (auto mode)",
        );
        assert!(!ui.app().edits_without_asking("mock"));
        ui.app_mut().stop_plugin("mock");
    }

    /// Start the installed mock plugin and wait until it runs.
    fn start_mock(ui: &mut crate::app::tests::ui::UiTest) {
        ui.app_mut().render_pane("plugin:mock/info", "open", None);
        for _ in 0..200 {
            ui.settle();
            if ui.app().plugins.running("mock") && !ui.app().plugins.starting("mock") {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the mock plugin did not start");
    }

    /// Help → About replaces the open dialog; then About is closed.
    fn open_and_close_about(ui: &mut crate::app::tests::ui::UiTest) {
        ui.app_mut().command("about");
        ui.settle();
        assert_eq!(ui.app().dialog, Some(Dialog::About));
        ui.app_mut().dialog = None;
        ui.settle();
    }

    #[test]
    fn the_session_prompt_comes_back_after_another_dialog_replaced_it() {
        use crate::app::tests::ui::UiTest;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(config.path());
        install_session_mock(ui.app_mut(), dir.path());
        start_mock(&mut ui);
        let request = file_request(
            401,
            "document/edit",
            json!({"name": "Agent paint", "edits": [{"op": "add_empty_layer"}]}),
        );
        assert!(ui.app_mut().hold_edit("mock", request).is_none());
        ui.settle();
        let title = "Allow Mock (plugin mock) to edit your documents for this session?";
        assert!(ui.has(title));
        open_and_close_about(&mut ui);
        // The held edit still waits for an answer, so its prompt shows again.
        assert!(!ui.app().plugins.edit_held.is_empty());
        assert!(ui.has(title), "the held edit's prompt never came back");
        // Opening a document clears the dialog too.
        ui.app_mut().new_document();
        ui.settle();
        assert!(ui.has(title), "the prompt did not come back after New");
        ui.click_role(egui::accesskit::Role::Button, "Allow");
        assert!(ui.app().plugins.edit_held.is_empty());
        assert!(ui.app().plugins.edit_prompt.is_none());
        assert_eq!(ui.app().dialog, None);
        ui.app_mut().stop_plugin("mock");
    }

    #[test]
    fn the_send_prompt_comes_back_after_another_dialog_replaced_it() {
        use crate::app::tests::ui::UiTest;
        use egui::accesskit::Role;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(config.path());
        install_network_mock(ui.app_mut(), dir.path());
        start_mock(&mut ui);
        let title = "Send to Mock (plugin mock)?";

        // An export the plugin asks for outside an action waits for the user.
        let request = file_request(402, "document/export", json!({}));
        ui.app_mut().plugins.held.push(("mock".into(), request));
        ui.settle();
        assert!(ui.has(title));
        open_and_close_about(&mut ui);
        assert_eq!(ui.app().plugins.held.len(), 1);
        assert!(ui.has(title), "the held export's prompt never came back");
        ui.click_role(Role::Button, "Cancel");
        assert!(ui.app().plugins.held.is_empty());
        assert!(ui.app().plugins.consent.is_none());
        for _ in 0..200 {
            if answer(dir.path(), 402).is_some() {
                break;
            }
            ui.settle();
            std::thread::sleep(Duration::from_millis(10));
        }
        let answered = answer(dir.path(), 402).expect("the export was never answered");
        assert_eq!(
            answered["error"]["code"],
            xuan::plugins::protocol::CANCELLED
        );

        // An action's prompt comes back while its action is still open…
        ui.app_mut().command("fill_fg");
        ui.open_menu("Plugins");
        ui.click("Send Layer · Mock");
        assert_eq!(ui.app().dialog, Some(Dialog::PluginConsent));
        open_and_close_about(&mut ui);
        assert_eq!(ui.app().dialog, Some(Dialog::PluginConsent));
        assert!(ui.has(title), "the action's prompt never came back");
        // …and is dropped once the action was closed.
        ui.app_mut().command("about");
        ui.settle();
        ui.app_mut().close_plugin_action();
        ui.app_mut().dialog = None;
        ui.settle();
        assert!(ui.app().plugins.consent.is_none());
        assert_eq!(ui.app().dialog, None);
        ui.app_mut().stop_plugin("mock");
    }

    #[test]
    fn the_file_prompt_comes_back_after_another_dialog_replaced_it() {
        use crate::app::tests::ui::UiTest;
        use egui::accesskit::Role;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(config.path());
        install_read_mock(ui.app_mut(), dir.path());
        start_mock(&mut ui);
        let image = out.path().join("asked.png");
        RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 200, 255]))
            .save(&image)
            .unwrap();
        ui.app_mut().queue_file_request(
            "mock",
            file_request(403, "file/open", json!({"path": image})),
        );
        ui.settle();
        let title = "Open a file?";
        assert!(ui.has(title));
        open_and_close_about(&mut ui);
        assert!(ui.app().plugins.file_prompt.is_some());
        assert!(ui.has(title), "the file request's prompt never came back");
        let sessions = ui.app().sessions.len();
        ui.click_role(Role::Button, "Open");
        assert!(ui.app().plugins.file_prompt.is_none());
        assert_eq!(ui.app().sessions.len(), sessions + 1);
        for _ in 0..200 {
            if answer(dir.path(), 403).is_some() {
                break;
            }
            ui.settle();
            std::thread::sleep(Duration::from_millis(10));
        }
        let answered = answer(dir.path(), 403).expect("the file request was never answered");
        assert_eq!(answered["result"]["ok"], true);
        ui.app_mut().stop_plugin("mock");
    }

    /// The plugin's `request/cancel` for request `id`.
    fn withdraw(app: &mut EditorApp, id: i64) {
        app.handle_notification(
            "mock",
            xuan::plugins::protocol::Notification {
                jsonrpc: "2.0".into(),
                method: "request/cancel".into(),
                params: serde_json::json!({"id": id}),
            },
        );
    }

    #[test]
    fn a_withdrawn_request_closes_its_prompt_and_a_late_answer_does_nothing() {
        use crate::app::plugin_sessions::EditAnswer;
        use serde_json::json;
        use xuan::plugins::protocol::CANCELLED;
        let dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_session_mock(&mut app, dir.path());
        app.dimensions = [16, 16];
        app.new_document();
        app.render_pane("plugin:mock/info", "open", None);
        run_until(&context, &mut app, |app| {
            app.plugins.running("mock") && !app.plugins.starting("mock")
        });
        let steps = app.session().unwrap().history.names().count();
        let edit = |id: i64, name: &str, session: &str| {
            file_request(
                id,
                "document/edit",
                json!({"name": name, "session": session, "edits": [{"op": "add_empty_layer", "name": name}]}),
            )
        };

        // A held edit whose client gave up: its prompt closes, and Allow
        // clicked late applies nothing.
        assert!(app.hold_edit("mock", edit(501, "Late", "s1")).is_none());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        withdraw(&mut app, 501);
        assert!(app.plugins.edit_held.is_empty());
        assert!(app.plugins.edit_prompt.is_none());
        assert_eq!(app.dialog, None);
        run_until(&context, &mut app, |_| answer(dir.path(), 501).is_some());
        assert_eq!(answer(dir.path(), 501).unwrap()["error"]["code"], CANCELLED);
        for _ in 0..5 {
            frame(&context, &mut app);
        }
        assert_eq!(app.dialog, None, "the prompt came back");
        app.answer_edit_session(EditAnswer::Allow);
        assert_eq!(app.session().unwrap().history.names().count(), steps);
        // Nothing was answered for the session: its next edit asks again.
        assert_eq!(app.edit_answer("mock", "s1"), None);
        // Withdrawing is not a refusal: there is no cooldown.
        assert!(!app.edit_cooling_down("mock"));

        // While another edit of the session waits, the prompt asks about it.
        assert!(app.hold_edit("mock", edit(502, "One", "s2")).is_none());
        assert!(app.hold_edit("mock", edit(503, "Two", "s2")).is_none());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        withdraw(&mut app, 502);
        assert_eq!(app.plugins.edit_held.len(), 1);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        assert_eq!(app.plugins.edit_prompt.as_ref().unwrap().edit, "Two");
        app.answer_edit_session(EditAnswer::Allow);
        run_until(&context, &mut app, |_| answer(dir.path(), 503).is_some());
        assert!(answer(dir.path(), 503).unwrap()["result"]["layers"].is_array());
        let session = app.session().unwrap();
        assert_eq!(session.history.names().count(), steps + 1);
        assert_eq!(session.history.undo_name(), Some("Two"));

        // A prompt that another dialog replaced does not come back once its
        // request was withdrawn.
        assert!(app.hold_edit("mock", edit(504, "Hidden", "s3")).is_none());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginEditSession)
        });
        app.dialog = Some(Dialog::About);
        withdraw(&mut app, 504);
        assert_eq!(app.dialog, Some(Dialog::About));
        app.dialog = None;
        for _ in 0..5 {
            frame(&context, &mut app);
        }
        assert_eq!(app.dialog, None, "the withdrawn prompt came back");
        assert!(app.plugins.edit_prompt.is_none());

        // The same for "Open a file?": Open clicked late opens nothing, and
        // the plugin may ask again at once.
        let image = out.path().join("asked.png");
        RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 200, 255]))
            .save(&image)
            .unwrap();
        let sessions = app.sessions.len();
        app.queue_file_request(
            "mock",
            file_request(505, "file/open", json!({"path": image})),
        );
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginFile)
        });
        withdraw(&mut app, 505);
        assert!(app.plugins.file_prompt.is_none());
        assert_eq!(app.dialog, None);
        run_until(&context, &mut app, |_| answer(dir.path(), 505).is_some());
        assert_eq!(answer(dir.path(), 505).unwrap()["error"]["code"], CANCELLED);
        app.answer_file_prompt(true);
        for _ in 0..5 {
            frame(&context, &mut app);
        }
        assert_eq!(app.sessions.len(), sessions);
        assert_eq!(app.dialog, None);
        assert!(!app.plugins.file_refused_at.contains_key("mock"));
        // A queued one is dropped before its prompt shows.
        app.dialog = Some(Dialog::About);
        app.queue_file_request(
            "mock",
            file_request(506, "file/open", json!({"path": image})),
        );
        assert_eq!(app.plugins.file_requests.len(), 1);
        withdraw(&mut app, 506);
        assert!(app.plugins.file_requests.is_empty());
        app.dialog = None;
        run_until(&context, &mut app, |_| answer(dir.path(), 506).is_some());
        assert_eq!(app.dialog, None);
        assert_eq!(app.sessions.len(), sessions);

        // A withdrawn request that was answered already is left alone, and
        // an unknown id does nothing.
        withdraw(&mut app, 503);
        withdraw(&mut app, 9999);
        let answers = received(dir.path())
            .lines()
            .filter(|line| line.contains("\"id\":503"))
            .count();
        assert_eq!(answers, 1);
        app.stop_plugin("mock");
    }

    #[test]
    fn a_withdrawn_export_closes_the_send_prompt() {
        use serde_json::json;
        use xuan::plugins::protocol::CANCELLED;
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_network_mock(&mut app, dir.path());
        app.dimensions = [16, 16];
        app.new_document();
        app.render_pane("plugin:mock/info", "open", None);
        run_until(&context, &mut app, |app| {
            app.plugins.running("mock") && !app.plugins.starting("mock")
        });
        for id in [601, 602] {
            let request = file_request(id, "document/export", json!({}));
            app.plugins.held.push(("mock".into(), request));
        }
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginConsent)
        });
        // The prompt comes back while another export waits on it.
        withdraw(&mut app, 601);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginConsent)
        });
        assert_eq!(app.plugins.held.len(), 1);
        withdraw(&mut app, 602);
        assert!(app.plugins.held.is_empty() && app.plugins.consent.is_none());
        assert_eq!(app.dialog, None);
        for id in [601, 602] {
            run_until(&context, &mut app, |_| answer(dir.path(), id).is_some());
            assert_eq!(answer(dir.path(), id).unwrap()["error"]["code"], CANCELLED);
        }
        for _ in 0..5 {
            frame(&context, &mut app);
        }
        assert_eq!(app.dialog, None, "the withdrawn prompt came back");
        // Send clicked late sends nothing and records no answer.
        app.answer_consent(true);
        assert_eq!(app.export_answer("mock"), None);
        app.stop_plugin("mock");
    }
}

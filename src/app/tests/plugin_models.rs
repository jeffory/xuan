//! Models a plugin declares: the confirmation before downloading, downloads
//! through a fake transport (the real https client is tested with a local
//! server in `xuan::plugins::models`), verification, deletion, offline mode,
//! the paths `initialize` hands over and the Models section of Manage Plugins.
use std::{
    collections::HashMap,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use super::*;
use crate::app::tests::ui::UiTest;
use xuan::plugins::{
    Manifest,
    models::{Reply, Transport},
};

const NET: &[u8] = &[42; 1500];
const EXTRA: &[u8] = b"extra weights";

fn sha256(data: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, data)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn manifest_text(models: &str) -> String {
    format!(
        r#"[plugin]
id = "modeled"
name = "Modeled"
version = "1.0.0"
command = ["sh", "plugin.sh"]

[[actions]]
id = "run"
label = "Run Model"
kind = "command"
models = ["net"]

{models}"#
    )
}

fn net_model() -> String {
    format!(
        "[[models]]\nid = \"net\"\nurl = \"https://models.example/v1/net.onnx\"\nsha256 = \"{}\"\nsize = {}\nlicense = \"Apache-2.0\"\nsource = \"Test net\"\n",
        sha256(NET),
        NET.len()
    )
}

fn extra_model() -> String {
    format!(
        "[[models]]\nid = \"extra\"\nurl = \"https://cdn.example/extra.bin\"\nsha256 = \"{}\"\nsize = {}\n",
        sha256(EXTRA),
        EXTRA.len()
    )
}

/// Answers `initialize` and `action/run` and logs every line it receives
/// to `received.log`, and its `XUAN_MODELS_DIR` to `env.log`.
const SCRIPT: &str = r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> received.log
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' "$XUAN_MODELS_DIR" > env.log
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":1}}\n' "$id" ;;
    *'"method":"action/run"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"outputs":[]}}\n' "$id" ;;
    *'"method":"shutdown"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"; exit 0 ;;
  esac
done
"#;

fn install(app: &mut EditorApp, dir: &Path, models: &str) {
    std::fs::write(dir.join("plugin.sh"), SCRIPT).unwrap();
    std::fs::write(dir.join("plugin.toml"), manifest_text(models)).unwrap();
    let manifest = Manifest::load(dir).unwrap();
    app.install_plugins(vec![manifest], vec![]);
    app.grant_plugin("modeled", true);
}

/// Serves files from memory. While `gate` is closed, a body stops after
/// its first half, so a test can see a download in progress.
#[derive(Default)]
struct Fake {
    files: HashMap<String, Vec<u8>>,
    gate: Arc<AtomicBool>,
    requested: Mutex<Vec<String>>,
}

impl Fake {
    fn new() -> Self {
        let fake = Self {
            files: HashMap::from([
                (
                    "https://models.example/v1/net.onnx".to_owned(),
                    NET.to_vec(),
                ),
                ("https://cdn.example/extra.bin".to_owned(), EXTRA.to_vec()),
            ]),
            ..Self::default()
        };
        fake.gate.store(true, Ordering::Relaxed);
        fake
    }

    fn closed() -> Self {
        let fake = Self::new();
        fake.gate.store(false, Ordering::Relaxed);
        fake
    }
}

struct Gated {
    data: Vec<u8>,
    sent: usize,
    gate: Arc<AtomicBool>,
}

impl Read for Gated {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let half = self.data.len() / 2;
        if self.sent >= half {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !self.gate.load(Ordering::Relaxed) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let end = if self.sent < half {
            half
        } else {
            self.data.len()
        };
        let count = (end - self.sent).min(buffer.len());
        buffer[..count].copy_from_slice(&self.data[self.sent..self.sent + count]);
        self.sent += count;
        Ok(count)
    }
}

impl Transport for Fake {
    fn get(&self, url: &url::Url, _size: u64) -> anyhow::Result<Reply> {
        self.requested.lock().unwrap().push(url.to_string());
        let data = self.files.get(url.as_str()).cloned();
        Ok(match data {
            Some(data) => Reply {
                status: 200,
                location: None,
                length: Some(data.len() as u64),
                body: Box::new(Gated {
                    data,
                    sent: 0,
                    gate: self.gate.clone(),
                }),
            },
            None => Reply {
                status: 404,
                location: None,
                length: None,
                body: Box::new(std::io::empty()),
            },
        })
    }
}

fn use_fake(app: &mut EditorApp, fake: Fake) -> Arc<Fake> {
    let fake = Arc::new(fake);
    app.plugins.transport = Some(fake.clone());
    fake
}

/// Run frames until `done`, failing after 20 seconds.
fn wait(ui: &mut UiTest, mut done: impl FnMut(&EditorApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(ui.app()) {
        assert!(
            Instant::now() < deadline,
            "timed out; error: {:?}",
            ui.app().error
        );
        ui.settle();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn status(app: &EditorApp, id: &str) -> ModelStatusView {
    let manifest = app.plugins.manifest("modeled").unwrap();
    let model = manifest.models.iter().find(|m| m.id == id).unwrap();
    let (status, size) = app.model_status("modeled", model);
    ModelStatusView(status.text(), size)
}

#[derive(Debug, PartialEq)]
struct ModelStatusView(String, Option<u64>);

fn view(text: &str, size: Option<u64>) -> ModelStatusView {
    ModelStatusView(text.into(), size)
}

#[test]
fn models_live_in_a_private_folder_inside_the_plugin_data_and_never_the_real_config() {
    let plugin = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    install(ui.app_mut(), plugin.path(), &net_model());
    // Without a configuration folder (as in every test) the data folder is
    // a private temporary one, so nothing lands in the user's own folders.
    assert_eq!(ui.app().plugins.models_dir_path("modeled"), None);
    let dir = ui.app_mut().plugins.models_dir("modeled").unwrap();
    assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
    assert!(dir.ends_with("models"));
    assert_eq!(
        ui.app().plugins.models_dir_path("modeled"),
        Some(dir.clone())
    );
    if let Some(home) = std::env::var_os("HOME") {
        assert!(!dir.starts_with(Path::new(&home).join(".config")));
    }
    // With one, it is <config>/plugin-data/<id>/models, owner-only.
    let config = tempfile::tempdir().unwrap();
    ui.isolate_config(config.path());
    ui.app_mut().plugins.config_dir = Some(config.path().to_path_buf());
    let dir = ui.app_mut().plugins.models_dir("modeled").unwrap();
    assert_eq!(dir, config.path().join("plugin-data/modeled/models"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
    // Host-side file requests can read models but never write into the folder.
    let access = ui.app().plugins.access("modeled");
    let error = access.writable_dir(&dir).unwrap_err().to_string();
    assert!(error.contains("managed by Xuan"), "{error}");
    assert!(access.writable_dir(dir.parent().unwrap()).is_ok());
}

#[test]
fn the_confirmation_lists_each_model_and_downloads_only_when_asked() {
    let plugin = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    install(
        ui.app_mut(),
        plugin.path(),
        &format!("{}{}", net_model(), extra_model()),
    );
    let fake = use_fake(ui.app_mut(), Fake::new());
    ui.app_mut()
        .request_model_download("modeled", vec!["net".into(), "extra".into()]);
    ui.settle();
    assert_eq!(ui.app().dialog, Some(Dialog::PluginModels));
    assert!(ui.has("Xuan downloads these models for Modeled (plugin modeled):"));
    assert!(ui.has("• net — 1.5 KB from models.example"));
    assert!(ui.has("  License: Apache-2.0"));
    assert!(ui.has("  Source: Test net"));
    assert!(ui.has("• extra — 13 B from cdn.example"));
    assert!(ui.has("  The plugin gives no license or source."));
    assert!(ui.has("Total: 1.5 KB"));
    ui.click("Cancel");
    assert_eq!(ui.app().plugins.model_request, None);
    assert!(
        fake.requested.lock().unwrap().is_empty(),
        "Cancel downloads nothing"
    );
    assert_eq!(status(ui.app(), "net"), view("Not downloaded", None));

    ui.app_mut()
        .request_model_download("modeled", vec!["net".into(), "extra".into()]);
    ui.settle();
    ui.click("Download");
    wait(&mut ui, |app| app.plugins.model_jobs.is_empty());
    assert_eq!(ui.app().error, None);
    assert_eq!(status(ui.app(), "net"), view("Ready", Some(1500)));
    assert_eq!(status(ui.app(), "extra"), view("Ready", Some(13)));
    let dir = ui.app().plugins.models_dir_path("modeled").unwrap();
    assert_eq!(std::fs::read(dir.join("net.onnx")).unwrap(), NET);
    assert_eq!(std::fs::read(dir.join("extra.bin")).unwrap(), EXTRA);
    assert_eq!(
        ui.app().model_paths("modeled"),
        serde_json::json!({"net": dir.join("net.onnx"), "extra": dir.join("extra.bin")})
    );
}

#[test]
fn offline_mode_downloads_nothing() {
    let plugin = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    install(ui.app_mut(), plugin.path(), &net_model());
    let fake = use_fake(ui.app_mut(), Fake::new());
    ui.app_mut().config.disable_network_plugins = true;
    // A plugin that declares no hosts still runs offline, but Xuan does
    // not download its models.
    assert!(ui.app().plugin_available("modeled"));
    ui.app_mut().start_plugin_action("modeled", "run");
    ui.settle();
    assert_eq!(ui.app().dialog, Some(Dialog::PluginModels));
    assert!(ui.has("Models are not downloaded while plugins that use the network are disabled"));
    assert!(!ui.enabled("Download"));
    let error = ui
        .app_mut()
        .start_model_job("modeled", "net", false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("not downloaded"), "{error}");
    assert!(ui.app().plugins.model_jobs.is_empty());
    assert!(fake.requested.lock().unwrap().is_empty());
}

#[test]
fn a_download_shows_progress_and_cancelling_leaves_nothing() {
    let plugin = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    install(ui.app_mut(), plugin.path(), &net_model());
    let fake = use_fake(ui.app_mut(), Fake::closed());
    ui.app_mut()
        .start_model_job("modeled", "net", false)
        .unwrap();
    wait(&mut ui, |app| {
        app.plugins
            .model_jobs
            .first()
            .is_some_and(|job| job.fraction() >= 0.5)
    });
    assert_eq!(status(ui.app(), "net").0, "Downloading 50%");
    // The job window shows it with the other background jobs.
    assert!(ui.has_role(egui::accesskit::Role::Label, "Downloading net"));
    assert!(ui.has("750 B / 1.5 KB"));
    let id = ui.app().plugins.model_jobs[0].id;
    ui.app_mut().cancel_model_job(id);
    fake.gate.store(true, Ordering::Relaxed);
    wait(&mut ui, |app| app.plugins.model_jobs.is_empty());
    assert_eq!(ui.app().error, None);
    assert_eq!(ui.app().status, "Cancelled");
    assert_eq!(status(ui.app(), "net"), view("Not downloaded", None));
    let dir = ui.app().plugins.models_dir_path("modeled").unwrap();
    assert!(!dir.join("net.part").exists() && !dir.join("net.onnx").exists());
}

#[test]
fn a_corrupted_model_is_found_by_verifying_it_again_and_is_not_handed_over() {
    let plugin = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    install(ui.app_mut(), plugin.path(), &net_model());
    use_fake(ui.app_mut(), Fake::new());
    ui.app_mut()
        .start_model_job("modeled", "net", false)
        .unwrap();
    wait(&mut ui, |app| app.plugins.model_jobs.is_empty());
    let dir = ui.app().plugins.models_dir_path("modeled").unwrap();
    let path = dir.join("net.onnx");
    // Same size, other bytes, another modification time.
    let mut bad = NET.to_vec();
    bad[700] = 0;
    std::fs::write(&path, &bad).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    assert_eq!(status(ui.app(), "net"), view("Not verified", Some(1500)));
    assert_eq!(ui.app().model_paths("modeled"), serde_json::json!({}));
    // Running the action that needs it verifies it first, finds it
    // corrupt and offers to download it again instead of running.
    ui.app_mut().start_plugin_action("modeled", "run");
    wait(&mut ui, |app| app.plugins.model_jobs.is_empty());
    assert_eq!(status(ui.app(), "net"), view("Corrupt", Some(1500)));
    assert!(
        ui.app()
            .error
            .as_deref()
            .is_some_and(|e| e.contains("does not match the size or SHA-256")),
        "{:?}",
        ui.app().error
    );
    assert!(ui.app().plugins.jobs.is_empty(), "the action did not run");
    assert_eq!(ui.app().model_paths("modeled"), serde_json::json!({}));
    // A file of the wrong size is corrupt straight away.
    std::fs::write(&path, b"short").unwrap();
    assert_eq!(status(ui.app(), "net"), view("Corrupt", Some(5)));
}

#[test]
fn the_models_section_downloads_shows_and_deletes_models() {
    let plugin = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(config.path());
    ui.app_mut().plugins.config_dir = Some(config.path().to_path_buf());
    install(ui.app_mut(), plugin.path(), &net_model());
    use_fake(ui.app_mut(), Fake::new());
    let dir = config.path().join("plugin-data/modeled/models");

    ui.open_menu("Plugins");
    ui.click("Manage Plugins…");
    // The only plugin is selected; its permissions mention the models.
    assert!(
        ui.has("• Uses models that Xuan downloads, after asking you: net (1.5 KB, models.example)")
    );
    assert!(ui.has("Models"));
    assert!(ui.has("net"));
    assert!(ui.has("Not downloaded"));
    assert!(!ui.has("Delete"), "nothing to delete yet");
    ui.click("Download");
    // Downloading always asks first, with the host and size.
    assert_eq!(ui.app().dialog, Some(Dialog::PluginModels));
    assert!(ui.has("• net — 1.5 KB from models.example"));
    ui.click("Download");
    wait(&mut ui, |app| app.plugins.model_jobs.is_empty());
    assert_eq!(
        ui.app().dialog,
        Some(Dialog::Plugins),
        "back to Manage Plugins"
    );
    assert!(ui.has("Ready"));
    assert!(ui.has("1.5 KB"));
    assert!(dir.join("net.onnx").is_file());

    ui.click("Delete");
    assert!(ui.has("Not downloaded"));
    assert!(!dir.join("net.onnx").exists());

    // Delete All Models removes the whole folder.
    ui.app_mut()
        .start_model_job("modeled", "net", false)
        .unwrap();
    wait(&mut ui, |app| app.plugins.model_jobs.is_empty());
    assert!(ui.has("Ready"));
    ui.click("Delete All Models");
    assert!(!dir.exists());
    assert!(ui.has("Not downloaded"));
}

#[cfg(unix)]
#[test]
fn an_action_downloads_its_models_first_then_runs_with_their_paths() {
    let plugin = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    install(
        ui.app_mut(),
        plugin.path(),
        &format!("{}{}", net_model(), extra_model()),
    );
    let fake = use_fake(ui.app_mut(), Fake::new());
    ui.app_mut().start_plugin_action("modeled", "run");
    ui.settle();
    // Only the model the action needs is offered.
    assert_eq!(ui.app().dialog, Some(Dialog::PluginModels));
    assert!(ui.has("“Run Model” needs these models from Modeled (plugin modeled):"));
    assert!(ui.has("• net — 1.5 KB from models.example"));
    assert!(!ui.has("• extra — 13 B from cdn.example"));
    assert!(
        !plugin.path().join("received.log").exists(),
        "nothing ran yet"
    );
    ui.click("Download");
    let log = plugin.path().join("received.log");
    wait(&mut ui, |app| {
        app.plugins.model_jobs.is_empty()
            && app.plugins.jobs.is_empty()
            && std::fs::read_to_string(&log).is_ok_and(|text| text.contains("action/run"))
    });
    assert_eq!(
        *fake.requested.lock().unwrap(),
        ["https://models.example/v1/net.onnx"]
    );
    let received = std::fs::read_to_string(plugin.path().join("received.log")).unwrap();
    let initialize: serde_json::Value = received
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|message| message["method"] == "initialize")
        .unwrap();
    let dir = ui.app().plugins.models_dir_path("modeled").unwrap();
    assert_eq!(initialize["params"]["models_dir"], serde_json::json!(dir));
    assert_eq!(
        initialize["params"]["models"],
        serde_json::json!({"net": dir.join("net.onnx")})
    );
    assert!(received.contains("\"method\":\"action/run\""));
    let env = std::fs::read_to_string(plugin.path().join("env.log")).unwrap();
    assert_eq!(env.trim_end(), dir.display().to_string());

    // A model downloaded while the plugin runs is announced to it.
    ui.app_mut()
        .start_model_job("modeled", "extra", false)
        .unwrap();
    wait(&mut ui, |app| app.plugins.model_jobs.is_empty());
    wait(&mut ui, |_| {
        std::fs::read_to_string(plugin.path().join("received.log"))
            .unwrap()
            .contains("models/changed")
    });
    let received = std::fs::read_to_string(plugin.path().join("received.log")).unwrap();
    let changed: serde_json::Value = received
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|message| message["method"] == "models/changed")
        .unwrap();
    assert_eq!(
        changed["params"]["models"],
        serde_json::json!({"net": dir.join("net.onnx"), "extra": dir.join("extra.bin")})
    );
    // Once the models are ready the action runs straight away.
    ui.app_mut().start_plugin_action("modeled", "run");
    assert_ne!(ui.app().dialog, Some(Dialog::PluginModels));
}

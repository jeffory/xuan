//! Write Xuan plugins in Rust.
//!
//! A plugin is a program that reads JSON-RPC requests from stdin and answers
//! on stdout; `docs/PLUGINS.md` in the Xuan repository describes the
//! protocol. This crate does the plumbing so a plugin is a few closures:
//!
//! ```no_run
//! use xuan_plugin::{Plugin, Output, ui};
//!
//! Plugin::new()
//!     .action("invert", |job| {
//!         job.progress(Some(0.1), Some("reading"));
//!         // read job.source_path(), write job.path("out.png") …
//!         Ok(vec![Output::image(job.path("out.png"), Some("Inverted"), 0.0, 0.0)])
//!     })
//!     .pane("info", |pane| Ok(ui::column(vec![ui::heading("Hello"), ui::label(&pane.reason)])))
//!     .run();
//! ```
//!
//! Handlers run on worker threads, so they may call back into the editor
//! through [`Host`] while the editor keeps servicing messages.
use std::{
    collections::HashMap,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const PROTOCOL: u32 = 1;

/// Error codes the editor understands.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The job was cancelled by the user.
    pub const CANCELLED: i64 = -32800;
    /// The plugin needs configuring; the editor offers to open its settings.
    pub const NEEDS_SETUP: i64 = -32001;
    pub const INSUFFICIENT_CREDITS: i64 = -32002;
    pub const RATE_LIMITED: i64 = -32003;
}

/// A JSON-RPC error, returned from handlers or received from the editor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: Value::Null,
        }
    }

    pub fn cancelled() -> Self {
        Self::new(codes::CANCELLED, "Cancelled")
    }

    /// The editor shows the message with a button that opens the plugin's settings.
    pub fn needs_setup(message: impl Into<String>) -> Self {
        Self::new(codes::NEEDS_SETUP, message)
    }

    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::new(codes::INTERNAL_ERROR, message.to_string())
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

impl From<std::io::Error> for RpcError {
    fn from(error: std::io::Error) -> Self {
        Self::internal(error)
    }
}

impl From<serde_json::Error> for RpcError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(codes::INVALID_PARAMS, error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, RpcError>;

struct Transport {
    stdout: Mutex<std::io::Stdout>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, mpsc::Sender<std::result::Result<Value, RpcError>>>>,
}

impl Transport {
    fn send(&self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        if let Ok(mut stdout) = self.stdout.lock() {
            let _ = stdout.write_all(line.as_bytes());
            let _ = stdout.flush();
        }
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (send, receive) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| RpcError::internal("poisoned"))?
            .insert(id, send);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        match receive.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                Err(RpcError::internal(format!(
                    "the editor did not answer {method}"
                )))
            }
        }
    }

    fn resolve(&self, message: &Value) {
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            return;
        };
        let Some(sender) = self.pending.lock().ok().and_then(|mut p| p.remove(&id)) else {
            return;
        };
        let result = match message.get("error") {
            Some(error) if !error.is_null() => Err(serde_json::from_value(error.clone())
                .unwrap_or_else(|_| RpcError::internal("malformed error"))),
            _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = sender.send(result);
    }
}

/// Requests a plugin can make of the editor.
#[derive(Clone)]
pub struct Host {
    transport: Arc<Transport>,
    pub timeout: Duration,
}

impl Host {
    pub fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.transport.request(method, params, self.timeout)
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.transport
            .send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Add a line to the plugin log in Plugins → Manage Plugins….
    pub fn log(&self, message: impl std::fmt::Display) {
        self.notify(
            "host/log",
            json!({"level": "info", "message": message.to_string()}),
        );
    }

    /// Show a message in the editor's status bar.
    pub fn status(&self, message: impl std::fmt::Display) {
        self.notify("host/status", json!({"message": message.to_string()}));
    }

    /// The open document: size, layers and selection, or `Null`.
    pub fn document(&self) -> Result<Value> {
        self.request("document/get", Value::Null)
    }

    /// Write a layer's pixels (`what = "pixels"`) or mask as PNG.
    pub fn export_layer(&self, layer: &str, what: &str, max_side: Option<u32>) -> Result<Export> {
        let value = self.request(
            "layer/export",
            json!({"layer": layer, "what": what, "max_side": max_side}),
        )?;
        Ok(serde_json::from_value(value)?)
    }

    /// Write the flattened document as PNG.
    pub fn export_document(&self, max_side: Option<u32>) -> Result<Export> {
        let value = self.request("document/export", json!({"max_side": max_side}))?;
        Ok(serde_json::from_value(value)?)
    }

    /// Write the selection mask cropped to its bounds, if there is one.
    pub fn export_selection(&self) -> Result<Option<Export>> {
        let value = self.request("selection/export", json!({}))?;
        Ok(serde_json::from_value(value)?)
    }

    /// Apply edits as one undo step. Needs `document = "edit"` in the manifest.
    pub fn edit(&self, name: &str, edits: Vec<Value>) -> Result<()> {
        self.request("document/edit", json!({"name": name, "edits": edits}))?;
        Ok(())
    }

    /// Run an allowed host command, or one of this plugin's own `plugin/action`.
    pub fn run(&self, action: &str, inputs: Value) -> Result<()> {
        self.request("host/run", json!({"action": action, "inputs": inputs}))?;
        Ok(())
    }

    pub fn open_path(&self, path: &Path) -> Result<()> {
        self.request("host/open", json!({"path": path}))?;
        Ok(())
    }

    pub fn open_url(&self, url: &str) -> Result<()> {
        self.request("host/open", json!({"url": url}))?;
        Ok(())
    }
}

/// A PNG the editor wrote for the plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default = "one")]
    pub scale: f32,
}

fn one() -> f32 {
    1.0
}

/// A region the user marked, in the coordinates of the source image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub index: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub mask: Option<PathBuf>,
    #[serde(default)]
    pub fields: serde_json::Map<String, Value>,
}

/// The `source` field of a job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default = "one")]
    pub scale: f32,
    /// The selection as a grey PNG the size of `path`, when the action sets
    /// `source.mask = "selection"` (white selected, black not).
    #[serde(default)]
    pub mask: Option<PathBuf>,
}

/// One `action/run`.
pub struct Job {
    pub id: String,
    pub action: String,
    pub inputs: Value,
    pub source: Option<Source>,
    pub document: Value,
    pub work_dir: PathBuf,
    pub host: Host,
    cancelled: Arc<AtomicBool>,
}

impl Job {
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// `Err(cancelled)` once the user cancelled the job.
    pub fn check_cancelled(&self) -> Result<()> {
        if self.cancelled() {
            Err(RpcError::cancelled())
        } else {
            Ok(())
        }
    }

    pub fn progress(&self, fraction: Option<f32>, message: Option<&str>) {
        self.host.notify(
            "job/progress",
            json!({"job": self.id, "fraction": fraction.map(|f| f.clamp(0.0, 1.0)), "message": message}),
        );
    }

    /// A file path inside the job's working directory. Write outputs here:
    /// the host reads result images only from the plugin's own folders
    /// unless the manifest declares `filesystem = "read"`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.work_dir.join(name)
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source.as_ref().map(|s| s.path.as_path())
    }

    /// The selection mask sent with the source (`source.mask = "selection"`),
    /// a grey PNG with the same size and crop as [`Job::source_path`].
    pub fn selection_mask_path(&self) -> Option<&Path> {
        self.source.as_ref().and_then(|s| s.mask.as_deref())
    }

    /// A named input, deserialized.
    pub fn input<T: serde::de::DeserializeOwned>(&self, id: &str) -> Result<T> {
        let value =
            self.inputs.get(id).cloned().ok_or_else(|| {
                RpcError::new(codes::INVALID_PARAMS, format!("missing input {id}"))
            })?;
        Ok(serde_json::from_value(value)?)
    }

    /// The regions input, if the action has one.
    pub fn regions(&self) -> Vec<Region> {
        self.inputs
            .as_object()
            .into_iter()
            .flat_map(|inputs| inputs.values())
            .find_map(|value| {
                let regions: Vec<Region> = serde_json::from_value(value.clone()).ok()?;
                (!regions.is_empty()).then_some(regions)
            })
            .unwrap_or_default()
    }
}

/// How a `mask` output combines with the current selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// What an action returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Output {
    Image {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default)]
        x: f32,
        #[serde(default)]
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mask: Option<PathBuf>,
        /// Placed width in document units, instead of the pixel width.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
        /// Placed height in document units, instead of the pixel height.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<f32>,
        /// `"source"`: cover the bounds of the source that was sent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<String>,
        /// Model and sampler details; see [`Output::with_provenance`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provenance: Option<Value>,
    },
    Document {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provenance: Option<Value>,
    },
    /// A grey PNG that becomes the selection; see [`Output::mask`].
    Mask {
        path: PathBuf,
        #[serde(default)]
        mode: MaskMode,
        #[serde(default)]
        x: f32,
        #[serde(default)]
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<String>,
    },
    Edit {
        edits: Vec<Value>,
    },
    Text {
        text: String,
    },
    None,
}

impl Output {
    pub fn image(path: impl Into<PathBuf>, name: Option<&str>, x: f32, y: f32) -> Self {
        Self::Image {
            path: path.into(),
            name: name.map(str::to_owned),
            x,
            y,
            mask: None,
            width: None,
            height: None,
            fit: None,
            provenance: None,
        }
    }

    /// Place an image or mask at `width` x `height` document units rather
    /// than its pixel size. A higher-resolution result then keeps its extra
    /// pixels at a higher pixel density. Giving only one side keeps the
    /// aspect ratio.
    pub fn with_size(mut self, new_width: Option<f32>, new_height: Option<f32>) -> Self {
        if let Self::Image {
            width, height, fit, ..
        }
        | Self::Mask {
            width, height, fit, ..
        } = &mut self
        {
            *width = new_width;
            *height = new_height;
            *fit = None;
        }
        self
    }

    /// Place an image or mask over the bounds of the source that was sent
    /// (`fit = "source"`), however many pixels it has.
    pub fn fit_source(mut self) -> Self {
        if let Self::Image {
            width, height, fit, ..
        }
        | Self::Mask {
            width, height, fit, ..
        } = &mut self
        {
            *width = None;
            *height = None;
            *fit = Some("source".into());
        }
        self
    }

    /// A grey PNG (white selected, black not, grey partly) that becomes the
    /// document's selection once the user accepts the result, combined with
    /// the current selection by `mode`. Placed like an image at `x`, `y` in
    /// source pixels. A plugin needs no `document = "edit"` for it.
    pub fn mask(path: impl Into<PathBuf>, mode: MaskMode) -> Self {
        Self::Mask {
            path: path.into(),
            mode,
            x: 0.0,
            y: 0.0,
            width: None,
            height: None,
            fit: None,
        }
    }

    /// Move an image or mask to `x`, `y` in source pixels.
    pub fn at(mut self, new_x: f32, new_y: f32) -> Self {
        if let Self::Image { x, y, .. } | Self::Mask { x, y, .. } = &mut self {
            *x = new_x;
            *y = new_y;
        }
        self
    }

    pub fn with_mask(self, mask: impl Into<PathBuf>) -> Self {
        match self {
            Self::Image {
                path,
                name,
                x,
                y,
                width,
                height,
                fit,
                provenance,
                ..
            } => Self::Image {
                path,
                name,
                x,
                y,
                mask: Some(mask.into()),
                width,
                height,
                fit,
                provenance,
            },
            other => other,
        }
    }

    /// Record how an `image` or `document` output was made, shown in the
    /// layer's info and saved with the project. Known keys: `model`,
    /// `model_hash`, `weights_sha256` (64 hex digits), `sampler`, `scheduler`,
    /// `steps`, `seed`, `cfg`, `service`, `request_id`, and an `extra` object
    /// for anything else. Strings are limited to 256 bytes and the whole
    /// record to 8 KiB; unknown keys fail the result. Secret-like keys
    /// (`api_key`, `token`, `authorization`, `password`, `secret`, ...) and
    /// your secrets' values are removed by the host: never put them here.
    pub fn with_provenance(mut self, value: Value) -> Self {
        if let Self::Image { provenance, .. } | Self::Document { provenance, .. } = &mut self {
            *provenance = Some(value);
        }
        self
    }

    pub fn document(path: impl Into<PathBuf>, name: Option<&str>) -> Self {
        Self::Document {
            path: path.into(),
            name: name.map(str::to_owned),
            provenance: None,
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

/// A `pane/render` request.
pub struct Pane {
    pub id: String,
    pub reason: String,
    pub widget: Option<String>,
    pub value: Value,
    pub document: Value,
    pub host: Host,
}

type ActionHandler = Arc<dyn Fn(&Job) -> Result<Vec<Output>> + Send + Sync>;
type EstimateHandler = Arc<dyn Fn(&Job) -> Result<Value> + Send + Sync>;
type PaneHandler = Arc<dyn Fn(&Pane) -> Result<Value> + Send + Sync>;
type ImportHandler = Arc<dyn Fn(&Host, &Path, &Path) -> Result<Value> + Send + Sync>;
type ExportHandler = Arc<dyn Fn(&Host, &Path, &Path, &Value) -> Result<()> + Send + Sync>;
type SettingsHandler = Arc<dyn Fn(&Settings) + Send + Sync>;

/// Settings and secrets as the editor last sent them. Its `Debug` output
/// names the secrets but never shows their values.
#[derive(Clone, Default, PartialEq)]
pub struct Settings {
    pub settings: Value,
    pub secrets: Value,
    pub plugin_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secrets: Vec<(&str, &str)> = self
            .secrets
            .as_object()
            .map(|secrets| {
                secrets
                    .keys()
                    .map(|key| (key.as_str(), "<redacted>"))
                    .collect()
            })
            .unwrap_or_default();
        f.debug_struct("Settings")
            .field("settings", &self.settings)
            .field("secrets", &secrets)
            .field("plugin_dir", &self.plugin_dir)
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

impl Settings {
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.settings.get(id)
    }

    pub fn secret(&self, id: &str) -> Option<&str> {
        self.secrets.get(id).and_then(Value::as_str)
    }
}

/// Register handlers, then call [`Plugin::run`].
#[derive(Default)]
pub struct Plugin {
    actions: HashMap<String, ActionHandler>,
    estimates: HashMap<String, EstimateHandler>,
    panes: HashMap<String, PaneHandler>,
    importers: HashMap<String, ImportHandler>,
    exporters: HashMap<String, ExportHandler>,
    on_settings: Option<SettingsHandler>,
}

impl Plugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn action(
        mut self,
        id: &str,
        handler: impl Fn(&Job) -> Result<Vec<Output>> + Send + Sync + 'static,
    ) -> Self {
        self.actions.insert(id.into(), Arc::new(handler));
        self
    }

    /// Answer `action/estimate` with `{"cost": "…", "seconds": n}`.
    pub fn estimate(
        mut self,
        id: &str,
        handler: impl Fn(&Job) -> Result<Value> + Send + Sync + 'static,
    ) -> Self {
        self.estimates.insert(id.into(), Arc::new(handler));
        self
    }

    pub fn pane(
        mut self,
        id: &str,
        handler: impl Fn(&Pane) -> Result<Value> + Send + Sync + 'static,
    ) -> Self {
        self.panes.insert(id.into(), Arc::new(handler));
        self
    }

    /// `handler(host, path, work_dir)` returns `{"width", "height", "layers": […]}`.
    pub fn importer(
        mut self,
        format: &str,
        handler: impl Fn(&Host, &Path, &Path) -> Result<Value> + Send + Sync + 'static,
    ) -> Self {
        self.importers.insert(format.into(), Arc::new(handler));
        self
    }

    /// `handler(host, path, image, document)` writes the file.
    pub fn exporter(
        mut self,
        format: &str,
        handler: impl Fn(&Host, &Path, &Path, &Value) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.exporters.insert(format.into(), Arc::new(handler));
        self
    }

    /// Called after `initialize` and whenever settings change.
    pub fn on_settings(mut self, handler: impl Fn(&Settings) + Send + Sync + 'static) -> Self {
        self.on_settings = Some(Arc::new(handler));
        self
    }

    /// Serve requests until the editor sends `shutdown` or closes stdin.
    pub fn run(self) {
        let transport = Arc::new(Transport {
            stdout: Mutex::new(std::io::stdout()),
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
        });
        let host = Host {
            transport: transport.clone(),
            timeout: Duration::from_secs(120),
        };
        let runtime = Arc::new(Runtime {
            plugin: self,
            host,
            settings: Mutex::new(Settings::default()),
            jobs: Mutex::new(HashMap::new()),
        });
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.lock().read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = line.trim();
            if text.is_empty() {
                continue;
            }
            let Ok(message) = serde_json::from_str::<Value>(text) else {
                eprintln!(
                    "xuan-plugin: ignoring invalid JSON: {}",
                    &text[..text.len().min(200)]
                );
                continue;
            };
            match (
                message.get("method").and_then(Value::as_str),
                message.get("id"),
            ) {
                (Some(_), Some(_)) => {
                    let runtime = runtime.clone();
                    std::thread::spawn(move || runtime.handle_request(message));
                }
                (Some(_), None) => runtime.handle_notification(&message),
                (None, Some(_)) => transport.resolve(&message),
                (None, None) => {}
            }
        }
    }
}

struct Runtime {
    plugin: Plugin,
    host: Host,
    settings: Mutex<Settings>,
    jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Runtime {
    fn handle_request(&self, message: Value) {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = self.dispatch(&method, params);
        let response = match result {
            Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        };
        self.host.transport.send(&response);
        if method == "shutdown" {
            std::process::exit(0);
        }
    }

    fn dispatch(&self, method: &str, params: Value) -> Result<Value> {
        let string = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default()
        };
        match method {
            "initialize" => {
                let settings = Settings {
                    settings: params.get("settings").cloned().unwrap_or(Value::Null),
                    secrets: params.get("secrets").cloned().unwrap_or(Value::Null),
                    plugin_dir: PathBuf::from(string("plugin_dir")),
                    data_dir: PathBuf::from(string("data_dir")),
                };
                if let Some(handler) = &self.plugin.on_settings {
                    handler(&settings);
                }
                *self
                    .settings
                    .lock()
                    .map_err(|_| RpcError::internal("poisoned"))? = settings;
                Ok(json!({"protocol": PROTOCOL}))
            }
            "shutdown" | "pane/close" => Ok(Value::Null),
            "action/run" | "action/estimate" => {
                let action = string("action");
                let cancelled = Arc::new(AtomicBool::new(false));
                let job = Job {
                    id: string("job"),
                    action: action.clone(),
                    inputs: params.get("inputs").cloned().unwrap_or(Value::Null),
                    source: params
                        .get("source")
                        .filter(|s| !s.is_null())
                        .and_then(|s| serde_json::from_value(s.clone()).ok()),
                    document: params.get("document").cloned().unwrap_or(Value::Null),
                    work_dir: PathBuf::from(string("work_dir")),
                    host: self.host.clone(),
                    cancelled: cancelled.clone(),
                };
                if method == "action/estimate" {
                    return match self.plugin.estimates.get(&action) {
                        Some(handler) => handler(&job),
                        None => Ok(json!({})),
                    };
                }
                let handler = self.plugin.actions.get(&action).ok_or_else(|| {
                    RpcError::new(codes::METHOD_NOT_FOUND, format!("no action {action}"))
                })?;
                if let Ok(mut jobs) = self.jobs.lock() {
                    jobs.insert(job.id.clone(), cancelled);
                }
                let result = handler(&job);
                if let Ok(mut jobs) = self.jobs.lock() {
                    jobs.remove(&job.id);
                }
                Ok(json!({"outputs": result?}))
            }
            "pane/render" => {
                let pane_id = string("pane");
                let handler = self.plugin.panes.get(&pane_id).ok_or_else(|| {
                    RpcError::new(codes::METHOD_NOT_FOUND, format!("no pane {pane_id}"))
                })?;
                let event = params.get("event").cloned().unwrap_or(Value::Null);
                let pane = Pane {
                    id: pane_id,
                    reason: string("reason"),
                    widget: event
                        .get("widget")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    value: event.get("value").cloned().unwrap_or(Value::Null),
                    document: params.get("document").cloned().unwrap_or(Value::Null),
                    host: self.host.clone(),
                };
                handler(&pane)
            }
            "format/import" => {
                let format = string("format");
                let handler = self
                    .plugin
                    .importers
                    .get(&format)
                    .ok_or_else(|| RpcError::new(codes::METHOD_NOT_FOUND, "no importer"))?;
                handler(
                    &self.host,
                    Path::new(&string("path")),
                    Path::new(&string("work_dir")),
                )
            }
            "format/export" => {
                let format = string("format");
                let handler = self
                    .plugin
                    .exporters
                    .get(&format)
                    .ok_or_else(|| RpcError::new(codes::METHOD_NOT_FOUND, "no exporter"))?;
                handler(
                    &self.host,
                    Path::new(&string("path")),
                    Path::new(&string("image")),
                    params.get("document").unwrap_or(&Value::Null),
                )?;
                Ok(Value::Null)
            }
            other => Err(RpcError::new(
                codes::METHOD_NOT_FOUND,
                format!("unknown method {other}"),
            )),
        }
    }

    fn handle_notification(&self, message: &Value) {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("job/cancel") => {
                if let (Some(job), Ok(jobs)) =
                    (params.get("job").and_then(Value::as_str), self.jobs.lock())
                {
                    if let Some(flag) = jobs.get(job) {
                        flag.store(true, Ordering::Relaxed);
                    }
                }
            }
            Some("settings/changed") => {
                if let Ok(mut settings) = self.settings.lock() {
                    settings.settings = params.get("settings").cloned().unwrap_or(Value::Null);
                    settings.secrets = params.get("secrets").cloned().unwrap_or(Value::Null);
                    if let Some(handler) = &self.plugin.on_settings {
                        handler(&settings);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Builders for the widget tree a pane returns.
pub mod ui {
    use serde_json::{json, Value};

    pub fn column(children: Vec<Value>) -> Value {
        json!({"type": "column", "children": children})
    }
    pub fn row(children: Vec<Value>) -> Value {
        json!({"type": "row", "children": children})
    }
    pub fn heading(text: &str) -> Value {
        json!({"type": "heading", "text": text})
    }
    pub fn label(text: &str) -> Value {
        json!({"type": "label", "text": text})
    }
    pub fn muted(text: &str) -> Value {
        json!({"type": "label", "text": text, "muted": true, "small": true})
    }
    pub fn separator() -> Value {
        json!({"type": "separator"})
    }
    pub fn space(size: f32) -> Value {
        json!({"type": "space", "size": size})
    }
    pub fn button(id: &str, label: &str) -> Value {
        json!({"type": "button", "id": id, "label": label})
    }
    pub fn primary_button(id: &str, label: &str) -> Value {
        json!({"type": "button", "id": id, "label": label, "primary": true})
    }
    pub fn checkbox(id: &str, label: &str, value: bool) -> Value {
        json!({"type": "checkbox", "id": id, "label": label, "value": value})
    }
    pub fn text(id: &str, value: &str, placeholder: &str) -> Value {
        json!({"type": "text", "id": id, "value": value, "placeholder": placeholder})
    }
    pub fn multiline(id: &str, value: &str, placeholder: &str) -> Value {
        json!({"type": "text", "id": id, "value": value, "placeholder": placeholder, "multiline": true})
    }
    pub fn number(id: &str, value: f64, min: Option<f64>, max: Option<f64>) -> Value {
        json!({"type": "number", "id": id, "value": value, "min": min, "max": max})
    }
    pub fn slider(id: &str, value: f64, min: f64, max: f64, label: &str) -> Value {
        json!({"type": "slider", "id": id, "value": value, "min": min, "max": max, "label": label})
    }
    pub fn select(id: &str, value: &str, options: &[(&str, &str)]) -> Value {
        let options: Vec<Value> = options
            .iter()
            .map(|(id, label)| json!({"id": id, "label": label}))
            .collect();
        json!({"type": "select", "id": id, "value": value, "options": options})
    }
    pub fn color(id: &str, value: &str) -> Value {
        json!({"type": "color", "id": id, "value": value})
    }
    pub fn image(src: &str, width: Option<f32>, height: Option<f32>) -> Value {
        json!({"type": "image", "src": src, "width": width, "height": height})
    }
    pub fn progress(value: Option<f32>, label: &str) -> Value {
        json!({"type": "progress", "value": value, "label": label})
    }
    pub fn list(id: &str, items: Vec<Value>, selected: Option<&str>) -> Value {
        json!({"type": "list", "id": id, "items": items, "selected": selected})
    }
    pub fn item(id: &str, label: &str, detail: &str) -> Value {
        json!({"id": id, "label": label, "detail": detail})
    }
    pub fn swatches(id: Option<&str>, colors: &[&str], selected: Option<&str>) -> Value {
        json!({"type": "swatches", "id": id, "colors": colors, "selected": selected})
    }
    pub fn link(label: &str, url: &str) -> Value {
        json!({"type": "link", "label": label, "url": url})
    }

    /// An `image` source embedding PNG bytes instead of a file.
    pub fn png_data_url(png: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::from("data:image/png;base64,");
        for chunk in png.chunks(3) {
            let bytes = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_debug_output_hides_secret_values() {
        let settings = Settings {
            settings: json!({"max_side": 1024}),
            secrets: json!({"api_key": "sk-live-123"}),
            ..Settings::default()
        };
        let text = format!("{settings:?} {settings:#?}");
        assert!(!text.contains("sk-live"), "{text}");
        assert!(
            text.contains("api_key") && text.contains("max_side"),
            "{text}"
        );
        assert_eq!(settings.secret("api_key"), Some("sk-live-123"));
    }

    #[test]
    fn a_job_source_carries_the_selection_mask_path() {
        let with: Source = serde_json::from_value(json!({
            "path": "/j/source.png", "width": 8, "height": 6, "mask": "/j/selection.png",
        }))
        .unwrap();
        assert_eq!(with.mask.as_deref(), Some(Path::new("/j/selection.png")));
        let without: Source = serde_json::from_value(
            json!({"path": "/j/source.png", "width": 8, "height": 6, "mask": null}),
        )
        .unwrap();
        assert!(without.mask.is_none());
    }

    #[test]
    fn provenance_is_serialized_on_images_and_documents_only() {
        let record = json!({"model": "sdxl", "seed": 7, "extra": {"lora": "a"}});
        let image = Output::image("/tmp/out.png", None, 0.0, 0.0)
            .with_provenance(record.clone())
            .with_mask("/tmp/m.png");
        let value = serde_json::to_value(&image).unwrap();
        assert_eq!(value["provenance"], record);
        assert_eq!(value["mask"], "/tmp/m.png");
        let document = Output::document("/tmp/d.png", None).with_provenance(record.clone());
        assert_eq!(
            serde_json::to_value(&document).unwrap()["provenance"],
            record
        );
        // Other outputs ignore it.
        let text = Output::text("hi").with_provenance(record);
        assert!(serde_json::to_value(&text)
            .unwrap()
            .get("provenance")
            .is_none());
        let back: Output = serde_json::from_value(serde_json::to_value(&image).unwrap()).unwrap();
        assert_eq!(back, image);
    }

    #[test]
    fn outputs_serialize_with_the_protocol_tags() {
        let output = Output::image("/tmp/out.png", Some("Out"), 1.0, 2.0).with_mask("/tmp/m.png");
        let value = serde_json::to_value(&output).unwrap();
        assert_eq!(value["kind"], "image");
        assert_eq!(value["mask"], "/tmp/m.png");
        assert!(value.get("provenance").is_none());
        assert!(value.get("width").is_none() && value.get("fit").is_none());
        let fitted = Output::image("/tmp/out.png", None, 0.0, 0.0)
            .with_mask("/tmp/m.png")
            .fit_source();
        let value = serde_json::to_value(&fitted).unwrap();
        assert_eq!(
            (value["fit"].as_str(), value["mask"].as_str()),
            (Some("source"), Some("/tmp/m.png"))
        );
        let sized = Output::image("/tmp/out.png", None, 0.0, 0.0).with_size(Some(40.0), None);
        let value = serde_json::to_value(&sized).unwrap();
        assert_eq!(value["width"], 40.0);
        assert!(value.get("height").is_none() && value.get("fit").is_none());
        assert_eq!(
            serde_json::to_value(Output::None).unwrap(),
            json!({"kind": "none"})
        );
        let mask = Output::mask("/tmp/m.png", MaskMode::Subtract)
            .at(2.0, 3.0)
            .fit_source();
        assert_eq!(
            serde_json::to_value(&mask).unwrap(),
            json!({"kind": "mask", "path": "/tmp/m.png", "mode": "subtract", "x": 2.0, "y": 3.0, "fit": "source"})
        );
        let sized = Output::mask("/tmp/m.png", MaskMode::default()).with_size(None, Some(9.0));
        let value = serde_json::to_value(&sized).unwrap();
        assert_eq!(
            (value["mode"].as_str(), value["height"].as_f64()),
            (Some("replace"), Some(9.0))
        );
        assert!(value.get("fit").is_none() && value.get("width").is_none());
        // Only images and masks have a placement.
        assert_eq!(Output::text("hi").fit_source(), Output::text("hi"));
        assert_eq!(ui::png_data_url(b"hi"), "data:image/png;base64,aGk=");
        assert_eq!(ui::png_data_url(b"hello"), "data:image/png;base64,aGVsbG8=");
    }

    #[test]
    fn regions_are_found_among_inputs() {
        let job = Job {
            id: "j".into(),
            action: "a".into(),
            inputs: json!({"prompt": "x", "regions": [{"index": 1, "x": 1, "y": 2, "width": 3, "height": 4, "fields": {"desc": "hat"}}]}),
            source: None,
            document: Value::Null,
            work_dir: PathBuf::from("/tmp"),
            host: Host {
                transport: Arc::new(Transport {
                    stdout: Mutex::new(std::io::stdout()),
                    next_id: AtomicI64::new(1),
                    pending: Mutex::new(HashMap::new()),
                }),
                timeout: Duration::from_secs(1),
            },
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let regions = job.regions();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].fields["desc"], "hat");
        assert_eq!(job.input::<String>("prompt").unwrap(), "x");
        assert!(job.input::<String>("missing").is_err());
        assert_eq!(job.path("out.png"), PathBuf::from("/tmp/out.png"));
        assert!(job.check_cancelled().is_ok());
    }
}

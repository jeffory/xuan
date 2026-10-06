//! Hosting plugins in the editor: starting their processes, answering their
//! requests between frames, running actions as background jobs and turning
//! results into proposals the user accepts or discards.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use uuid::Uuid;
use xuan::{
    config::{PluginGrant, Secrets},
    document::{Document, Generated, Layer},
    i18n::tr,
    plugins::{
        self, LoadError, Manifest, edits,
        host::{Incoming, Process},
        jobs::{self, Prepared, Region},
        manifest::{Action, ActionKind, Capability, DocumentAccess, InputKind, Menu, ResultInto},
        protocol::{self, Id, Message, Notification, Request, Response, RpcError},
        ui::Node,
    },
    provenance::{Provenance, Redactor},
    selection::{self, SelectionMode},
};

use super::{
    Dialog, EditorApp, Session, Tool,
    commands::{self, HostRun},
};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);
const FORMAT_TIMEOUT: Duration = Duration::from_secs(300);
/// Shortest time between two links a plugin opens.
const LINK_INTERVAL: Duration = Duration::from_secs(1);
/// Shortest time between two document switches of a plugin, so a plugin
/// (or the MCP client behind one) cannot flip the user's tabs rapidly.
pub(super) const ACTIVATE_INTERVAL: Duration = Duration::from_secs(1);

/// Identifier of a plugin pane in the sidebar layout.
pub(super) fn pane_key(plugin: &str, pane: &str) -> String {
    format!("plugin:{plugin}/{pane}")
}

fn split_pane_key(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("plugin:")?.split_once('/')
}

/// Everything the editor keeps about plugins.
#[derive(Default)]
pub(super) struct PluginState {
    pub manifests: Vec<Manifest>,
    pub errors: Vec<LoadError>,
    processes: HashMap<String, Process>,
    /// Plugins whose `initialize` is unanswered, and since when.
    starting: HashMap<String, std::time::Instant>,
    pending: HashMap<(String, Id), Pending>,
    pub secrets: Secrets,
    pub secrets_path: Option<PathBuf>,
    /// The secrets file exists but could not be read; it is never overwritten.
    pub secrets_unreadable: bool,
    pub(super) config_dir: Option<PathBuf>,
    scratch: HashMap<String, tempfile::TempDir>,
    /// Private data folders for plugins when there is no configuration folder.
    data_fallback: HashMap<String, tempfile::TempDir>,
    pub jobs: Vec<PluginJob>,
    pub formats: Vec<FormatJob>,
    /// Finished jobs whose results wait, in order, for the editor to be free.
    pub completed: std::collections::VecDeque<(PluginJob, Value)>,
    pub panes: HashMap<String, PaneState>,
    pub proposal: Option<Proposal>,
    pub action: Option<ActionEdit>,
    /// A plugin waiting for its permissions to be accepted, and what to do then.
    pub permission_request: Option<(String, PendingStart)>,
    pub manager_selected: Option<String>,
    /// Plugins → Install from Folder or Zip…, while it is open.
    pub install: Option<super::plugin_install::PluginInstall>,
    revisions: HashMap<Uuid, u64>,
    /// Problems found while loading the plugins, before shortcut collisions are added.
    load_errors: Vec<LoadError>,
    /// When a plugin last opened a link.
    last_link: Option<std::time::Instant>,
    /// The prompt before document data goes to a plugin that declares
    /// network hosts, while it is open.
    pub consent: Option<super::plugin_consent::ConsentRequest>,
    /// Whether the user allowed exports a plugin asked for outside a
    /// confirmed action, until its process stops.
    pub export_answers: HashMap<String, bool>,
    /// Export requests waiting for that answer.
    pub held: Vec<(String, Request)>,
    /// Model downloads and verifications running in the background.
    pub model_jobs: Vec<super::plugin_models::ModelJob>,
    /// The confirmation before downloading models, while it is open.
    pub model_request: Option<super::plugin_models::ModelRequest>,
    /// A plugin action that runs once its models are downloaded or verified,
    /// with its inputs.
    pub awaiting_models: Option<(String, (String, Option<Value>))>,
    /// A provider run waiting for its action to start (after a permission prompt or
    /// a model download); see `providers.rs`.
    pub provider_pending: Option<super::providers::ProviderRun>,
    /// How models are downloaded: https, or a fake in tests.
    pub transport: Option<Arc<dyn plugins::models::Transport>>,
    /// `file/save_as`, `file/export` and `file/open` requests waiting for
    /// their turn; see `plugin_files.rs`.
    pub file_requests: std::collections::VecDeque<super::plugin_files::FileRequest>,
    /// The `file/open` request whose prompt is open.
    pub file_prompt: Option<super::plugin_files::FileRequest>,
    /// Shows the save dialog for plugins: the system's, or a fake in tests.
    pub save_dialog: Option<super::plugin_files::SaveDialogHook>,
    /// Whether the user allowed a plugin's direct edits, by plugin and
    /// session, until its process stops; see `plugin_sessions.rs`.
    pub edit_answers: HashMap<(String, String), bool>,
    /// Direct edits waiting for that answer.
    pub edit_held: Vec<(String, Request)>,
    /// The prompt asking for it, while it is open.
    pub edit_prompt: Option<super::plugin_sessions::EditSessionRequest>,
    /// Plugins whose every session the user refused until they stop.
    pub edits_refused: std::collections::HashSet<String>,
    /// When the user last refused a plugin's edits; see `COOLDOWN`.
    pub edit_refused_at: HashMap<String, std::time::Instant>,
    /// When the user last cancelled a plugin's file request.
    pub file_refused_at: HashMap<String, std::time::Instant>,
    /// When a plugin last switched the current document; see
    /// `ACTIVATE_INTERVAL`.
    pub activated_at: HashMap<String, std::time::Instant>,
}

enum Pending {
    /// The plugin's `initialize`; it is starting until this is answered.
    Initialize,
    Job(Uuid),
    /// A `format/import` or `format/export` of a [`FormatJob`].
    Format(Uuid),
    Render(String),
    Estimate(String, String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PendingStart {
    Action(String),
    Pane(String),
}

pub(super) struct PluginJob {
    pub id: Uuid,
    pub plugin: String,
    pub action: String,
    pub label: String,
    pub document: Uuid,
    /// Kept so the job files live until the result is applied.
    pub _work_dir: tempfile::TempDir,
    pub prepared: Prepared,
    pub regions: Vec<Region>,
    pub inputs: Value,
    pub into: ResultInto,
    pub mask_to_regions: bool,
    pub progress: Option<f32>,
    pub message: String,
    pub cancelled: bool,
    /// The user confirmed sending its document data, so it may export more
    /// while it runs.
    pub consented: bool,
    /// Set when the job stands in for a built-in command.
    pub provider: Option<super::providers::ProviderRun>,
}

/// A plugin action in a menu.
pub(super) struct PluginMenuItem {
    pub label: String,
    pub plugin: String,
    pub action: String,
    pub shortcut: String,
    /// The plugin's name, id and folder, shown on hover.
    pub source: String,
    /// Whether the registry lets it run now (see `command_enabled`).
    pub enabled: bool,
}

/// A file a plugin imports or exports while the editor stays usable.
pub(super) struct FormatJob {
    pub id: Uuid,
    pub plugin: String,
    pub label: String,
    kind: FormatKind,
    started: std::time::Instant,
    /// Holds the files exchanged until the plugin answers.
    work_dir: tempfile::TempDir,
}

enum FormatKind {
    Import { path: PathBuf, as_layer: bool },
    Export { path: PathBuf },
}

/// Layers a job added that the user has not accepted yet.
pub(super) struct Proposal {
    pub document: Uuid,
    pub name: String,
    /// The plugin it comes from, as [`PluginState::source`] shows it.
    pub source: String,
    pub layers: Vec<Uuid>,
    /// The selection before and after, when the result changes it.
    pub selection: Option<ProposedSelection>,
    pub comparing: bool,
    pub message: Option<String>,
}

/// A selection a result proposes; Compare shows `before` in its place.
pub(super) struct ProposedSelection {
    pub before: Option<Arc<image::GrayImage>>,
    pub after: Option<Arc<image::GrayImage>>,
}

/// The open action dialog.
pub(super) struct ActionEdit {
    pub plugin: String,
    pub action: String,
    pub values: Map<String, Value>,
    pub regions: Vec<Region>,
    pub selected: Option<usize>,
    pub estimate: Option<String>,
    pub previous_tool: Tool,
    /// Where results go when the action leaves the choice to the user.
    pub into: ResultInto,
    /// The user confirmed sending this run's document data.
    pub consented: bool,
    /// Set when the action stands in for a built-in command: it runs without its
    /// dialog and its mask is applied as that command's result.
    pub provider: Option<super::providers::ProviderRun>,
}

#[derive(Default)]
pub(super) struct PaneState {
    pub tree: Option<Node>,
    pub pending: bool,
    pub error: Option<String>,
    pub queued: Option<plugins::ui::Event>,
    /// A render was asked for while one was pending; render again when it ends.
    pub dirty: bool,
    /// Pane images by source, including the ones that failed to load, so a
    /// broken image is not read again every frame.
    pub images: HashMap<String, PaneImage>,
    pub drafts: HashMap<String, String>,
}

/// A loaded pane image, or a failure to load it.
pub(super) struct PaneImage {
    /// Modification time and size of the file it was read from; a change
    /// loads it again. `None` for data URLs and missing files.
    pub stamp: Option<(Option<std::time::SystemTime>, u64)>,
    pub texture: Option<egui::TextureHandle>,
}

impl PluginState {
    pub fn manifest(&self, plugin: &str) -> Option<&Manifest> {
        self.manifests.iter().find(|m| m.plugin.id == plugin)
    }

    /// Whether the plugin's process has not answered `initialize` yet.
    #[cfg(all(test, unix))]
    pub fn starting(&self, plugin: &str) -> bool {
        self.starting.contains_key(plugin)
    }

    /// How the user is told which plugin something comes from: its name,
    /// which any plugin may choose, plus its unique id, for example
    /// "Mock (plugin mock)". A plugin cannot pass as Xuan or another plugin.
    pub fn source(&self, plugin: &str) -> String {
        let name = self
            .manifest(plugin)
            .map_or(plugin, |m| m.plugin.name.as_str());
        format!("{} ({} {plugin})", one_line(name, 80), tr("plugin"))
    }

    /// A status bar message from a plugin, marked with where it comes from.
    pub fn status_from(&self, plugin: &str, message: &str) -> String {
        format!("{}: {}", self.source(plugin), one_line(message, 200))
    }

    pub fn running(&self, plugin: &str) -> bool {
        self.processes.contains_key(plugin)
    }

    pub(super) fn process_mut(&mut self, plugin: &str) -> Option<&mut Process> {
        self.processes.get_mut(plugin)
    }

    /// Forget the answers and held requests of a plugin whose process
    /// stopped or whose grant changed.
    fn forget_session(&mut self, plugin: &str) {
        self.export_answers.remove(plugin);
        self.held.retain(|(id, _)| id != plugin);
        self.file_requests
            .retain(|request| request.plugin != plugin);
        self.edit_answers.retain(|(id, _), _| id != plugin);
        self.edit_held.retain(|(id, _)| id != plugin);
        self.edits_refused.remove(plugin);
        if (self.edit_prompt.as_ref()).is_some_and(|prompt| prompt.plugin == plugin) {
            self.edit_prompt = None;
        }
    }

    pub fn log(&self, plugin: &str) -> Vec<String> {
        self.processes
            .get(plugin)
            .map(Process::log)
            .unwrap_or_default()
    }

    fn scratch_dir(&mut self, plugin: &str) -> Result<PathBuf> {
        if !self.scratch.contains_key(plugin) {
            self.scratch
                .insert(plugin.into(), plugins::private_dir("xuan-plugin-")?);
        }
        Ok(self.scratch[plugin].path().to_path_buf())
    }

    /// The data folder made for the plugin this session when there is no
    /// configuration folder.
    pub(super) fn data_fallback_path(&self, plugin: &str) -> Option<PathBuf> {
        self.data_fallback
            .get(plugin)
            .map(|d| d.path().to_path_buf())
    }

    /// Where the host may read and write files for this plugin: its folder,
    /// its data folder and the scratch and job folders made for it, unless
    /// the manifest's `filesystem` permission allows more. Its models folder
    /// is read only: only Xuan puts files there.
    pub(super) fn access(&self, plugin: &str) -> edits::Access {
        let Some(manifest) = self.manifest(plugin) else {
            return edits::Access::default();
        };
        let data = match &self.config_dir {
            Some(config_dir) => Some(plugins::data_dir(config_dir, plugin)),
            None => self.data_fallback_path(plugin),
        };
        let models = data.as_deref().map(plugins::models::models_dir);
        let roots = [Some(manifest.dir.clone()), data]
            .into_iter()
            .flatten()
            .chain(self.scratch.get(plugin).map(|d| d.path().to_path_buf()))
            .chain(
                (self.jobs.iter())
                    .filter(|job| job.plugin == plugin)
                    .map(|job| job._work_dir.path().to_path_buf()),
            )
            .chain(
                (self.formats.iter())
                    .filter(|job| job.plugin == plugin)
                    .map(|job| job.work_dir.path().to_path_buf()),
            );
        let access = edits::Access::new(roots, manifest.permissions.filesystem);
        match models {
            Some(models) => access.read_only(&models),
            None => access,
        }
    }

    /// Write the secrets file, unless it failed to load: saving then would
    /// replace the user's stored secrets with the few entered since.
    pub(super) fn save_secrets(&self) -> Result<()> {
        let Some(path) = &self.secrets_path else {
            return Ok(());
        };
        ensure!(
            !self.secrets_unreadable,
            "{} {}",
            path.display(),
            tr(
                "could not be read, so it is left unchanged. Fix or remove it, then press Reload in Plugins → Manage Plugins…"
            )
        );
        self.secrets.save(path)
    }

    /// The plugin's persistent data folder, created if needed. Without a
    /// configuration folder it is a private temporary folder (owner-only on
    /// Unix) that lasts for this session, never a predictable shared path.
    pub(super) fn data_dir(&mut self, plugin: &str) -> Result<PathBuf> {
        if let Some(config_dir) = &self.config_dir {
            let dir = plugins::data_dir(config_dir, plugin);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Cannot create {}", dir.display()))?;
            return Ok(dir);
        }
        if !self.data_fallback.contains_key(plugin) {
            let dir = plugins::private_dir("xuan-plugin-data-")
                .context("Cannot create a data folder for the plugin")?;
            self.data_fallback.insert(plugin.into(), dir);
        }
        Ok(self.data_fallback[plugin].path().to_path_buf())
    }

    /// Replace the loaded manifests, keeping processes of unchanged plugins.
    pub fn install(&mut self, manifests: Vec<Manifest>, errors: Vec<LoadError>) {
        self.processes
            .retain(|id, _| manifests.iter().any(|m| &m.plugin.id == id));
        self.starting
            .retain(|id, _| manifests.iter().any(|m| &m.plugin.id == id));
        self.panes.clear();
        self.manifests = manifests;
        self.load_errors = errors.clone();
        self.errors = errors;
    }

    /// Reports manifest shortcuts that could not be used beside the load errors. The editor
    /// works these out with its key bindings; see `commands::Keymap::build`.
    pub fn set_shortcut_errors(&mut self, errors: Vec<LoadError>) {
        self.errors = self.load_errors.clone();
        self.errors.extend(errors);
    }

    /// Stop every plugin when the editor quits, giving them a moment to exit.
    pub fn stop_all(&mut self) {
        self.starting.clear();
        let processes = self.processes.drain().map(|(_, mut process)| {
            let _ = process.request("shutdown", Value::Null);
            process
        });
        Process::stop_all(processes, Duration::from_millis(300));
    }
}

impl EditorApp {
    /// Discover plugins next to the configuration file and on `XUAN_PLUGIN_PATH`.
    pub(super) fn load_plugins(&mut self) {
        let config_dir = self
            .config_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        self.plugins.config_dir = config_dir.clone();
        self.plugins.secrets_path = config_dir.as_ref().map(|dir| dir.join("secrets.toml"));
        self.plugins.secrets_unreadable = false;
        if let Some(path) = &self.plugins.secrets_path {
            match Secrets::load(path) {
                Ok(secrets) => self.plugins.secrets = secrets,
                Err(error) => {
                    self.plugins.secrets_unreadable = true;
                    self.error = Some(format!(
                        "{}\n\n{error:#}",
                        tr("Could not load plugin secrets")
                    ))
                }
            }
        }
        let bundled = plugins::bundled_dir();
        let dirs = plugins::plugin_dirs(
            config_dir.as_deref(),
            std::env::var_os(plugins::PATH_VARIABLE).as_deref(),
            bundled.as_deref(),
        );
        let (manifests, errors) = plugins::discover(&dirs, bundled.as_deref());
        self.install_plugins(manifests, errors);
    }

    pub(super) fn install_plugins(&mut self, manifests: Vec<Manifest>, errors: Vec<LoadError>) {
        // Plugins that went away or changed lose their process, and with it
        // everything still waiting for that process.
        let mut gone: Vec<String> = self
            .plugins
            .manifests
            .iter()
            .filter(|old| !manifests.contains(old))
            .map(|old| old.plugin.id.clone())
            .collect();
        let orphans = (self.plugins.processes.keys())
            .chain(self.plugins.jobs.iter().map(|job| &job.plugin))
            .chain(self.plugins.pending.keys().map(|(plugin, _)| plugin))
            .filter(|id| !manifests.iter().any(|m| &m.plugin.id == *id))
            .cloned()
            .collect::<Vec<_>>();
        gone.extend(orphans);
        gone.sort();
        gone.dedup();
        for plugin in &gone {
            let name = self.plugins.source(plugin);
            self.end_plugin(
                plugin,
                &format!("{name} {}", tr("was removed or changed by Reload")),
            );
        }
        self.plugins.install(manifests, errors);
        self.rebuild_keymap();
        if self.add_plugin_panes() {
            self.save_config();
        }
    }

    /// Add the panes of installed plugins that the layout does not know yet.
    pub(super) fn add_plugin_panes(&mut self) -> bool {
        let mut changed = false;
        for manifest in &self.plugins.manifests {
            for pane in &manifest.panes {
                let key = pane_key(&manifest.plugin.id, &pane.id);
                if self.config.panes.get(&key).is_none() {
                    self.config.panes.ensure(&key);
                    changed = true;
                }
            }
        }
        changed
    }

    /// The default sidebar: the built-in panes, then every installed plugin's.
    pub(super) fn reset_panes(&mut self) {
        self.config.panes.reset();
        self.add_plugin_panes();
        self.save_config();
    }

    pub(super) fn plugin_pane_title(&self, key: &str) -> Option<String> {
        let (plugin, pane) = split_pane_key(key)?;
        let manifest = self.plugins.manifest(plugin)?;
        if !self.config.plugins.get(plugin).is_none_or(|c| c.enabled) {
            return None;
        }
        manifest.pane(pane).map(|pane| pane.title.clone())
    }

    pub(super) fn plugin_enabled(&self, plugin: &str) -> bool {
        self.config.plugins.get(plugin).is_none_or(|c| c.enabled)
    }

    /// Whether the plugin may run: enabled, and allowed by the user exactly as
    /// it is now (same folder, command and permissions). Every plugin needs
    /// this before its process first starts, even one that declares nothing,
    /// so a folder dropped into a plugin directory never runs on its own.
    pub(super) fn plugin_granted(&self, plugin: &str) -> bool {
        let Some(manifest) = self.plugins.manifest(plugin) else {
            return false;
        };
        self.plugin_enabled(plugin)
            && self
                .stored_grant(plugin)
                .is_some_and(|grant| grant.covers(&grant_for(manifest)))
    }

    /// What the user last allowed for this plugin, which may no longer match it.
    pub(super) fn stored_grant(&self, plugin: &str) -> Option<&PluginGrant> {
        self.config.plugins.get(plugin)?.grant.as_ref()
    }

    pub(super) fn grant_plugin(&mut self, plugin: &str, granted: bool) {
        let mut grant = if granted {
            self.plugins.manifest(plugin).map(grant_for)
        } else {
            None
        };
        // Answers stored with the grant last only while it covers the same
        // folder, command and permissions.
        if let (Some(old), Some(new)) = (self.stored_grant(plugin), &mut grant)
            && old.covers(new)
        {
            new.send_without_asking = old.send_without_asking;
            new.edit_without_asking = old.edit_without_asking;
        } else {
            self.plugins.forget_session(plugin);
        }
        // Secrets were entered for the folder the user allowed before; never
        // hand them to a plugin with the same id from another folder.
        if let (Some(old), Some(new)) = (self.stored_grant(plugin), &grant)
            && old.dir != new.dir
            && self.plugins.secrets.clear(plugin)
            && let Err(error) = self.plugins.save_secrets()
        {
            self.error = Some(format!("{}\n\n{error:#}", tr("Could not save secrets")));
        }
        self.config.plugins.entry(plugin.into()).or_default().grant = grant;
        self.save_config();
        if !granted {
            self.stop_plugin(plugin);
        }
    }

    pub(super) fn stop_plugin(&mut self, plugin: &str) {
        self.plugins.forget_session(plugin);
        self.plugins.starting.remove(plugin);
        if let Some(mut process) = self.plugins.processes.remove(plugin) {
            let _ = process.request("shutdown", Value::Null);
            process.stop();
        }
        self.plugins.pending.retain(|(id, _), _| id != plugin);
        self.plugins.jobs.retain(|job| job.plugin != plugin);
        self.fail_format_jobs(plugin, None);
        for (key, pane) in &mut self.plugins.panes {
            if split_pane_key(key).is_some_and(|(id, _)| id == plugin) {
                pane.pending = false;
                pane.dirty = false;
                pane.queued = None;
                // The old page describes a process that is gone: drop it so the
                // next draw opens the pane again, or shows why it cannot.
                pane.tree = None;
            }
        }
    }

    /// Stop a plugin and fail what was waiting for it with `reason`.
    pub(super) fn end_plugin(&mut self, plugin: &str, reason: &str) {
        self.plugins.forget_session(plugin);
        self.plugins.starting.remove(plugin);
        if let Some(mut process) = self.plugins.processes.remove(plugin) {
            let _ = process.request("shutdown", Value::Null);
            process.stop();
        }
        let failed: Vec<_> = (self.plugins.jobs.iter())
            .filter(|job| job.plugin == plugin)
            .map(|job| job.id)
            .collect();
        for id in failed {
            self.finish_plugin_job(id, Err(reason.to_owned()));
        }
        self.fail_format_jobs(plugin, Some(reason));
        self.plugins.pending.retain(|(id, _), _| id != plugin);
        for (key, pane) in &mut self.plugins.panes {
            if split_pane_key(key).is_some_and(|(id, _)| id == plugin) {
                pane.pending = false;
                pane.dirty = false;
                pane.queued = None;
            }
        }
    }

    /// Keeps a plugin's secrets, by name and by value, out of provenance.
    fn provenance_redactor(&self, plugin: &str) -> Redactor {
        let names = self.plugins.manifest(plugin).into_iter().flat_map(|m| {
            (m.permissions.secrets.iter())
                .chain(
                    m.settings
                        .iter()
                        .filter(|s| s.kind == InputKind::Secret)
                        .map(|s| &s.id),
                )
                .map(String::as_str)
        });
        let values = (self.plugins.secrets.0.get(plugin).into_iter())
            .flat_map(|m| m.values().map(String::as_str));
        Redactor::new(names, values)
    }

    /// Settings values for `initialize` and `settings/changed`.
    pub(super) fn plugin_settings(&self, manifest: &Manifest) -> (Value, Value) {
        let stored = self
            .config
            .plugins
            .get(&manifest.plugin.id)
            .map(|c| &c.settings);
        let mut settings = Map::new();
        let mut secrets = Map::new();
        for setting in &manifest.settings {
            if setting.kind == InputKind::Secret {
                if manifest.permissions.secrets.contains(&setting.id)
                    && let Some(value) = self.plugins.secrets.get(&manifest.plugin.id, &setting.id)
                {
                    secrets.insert(setting.id.clone(), Value::String(value.into()));
                }
                continue;
            }
            let value = stored
                .and_then(|table| table.get(&setting.id))
                .and_then(|value| serde_json::to_value(value).ok())
                .unwrap_or_else(|| setting.initial());
            settings.insert(setting.id.clone(), value);
        }
        (Value::Object(settings), Value::Object(secrets))
    }

    /// Tell a running plugin its settings changed.
    pub(super) fn notify_settings(&mut self, plugin: &str) {
        let Some(manifest) = self.plugins.manifest(plugin).cloned() else {
            return;
        };
        let (settings, secrets) = self.plugin_settings(&manifest);
        if let Some(process) = self.plugins.processes.get_mut(plugin) {
            let _ = process.notify(
                "settings/changed",
                json!({"settings": settings, "secrets": secrets}),
            );
        }
    }

    /// Start the plugin process if needed. Fails when it is not granted.
    fn plugin_process(&mut self, plugin: &str) -> Result<&mut Process> {
        self.reap_plugin(plugin);
        if !self.plugins.processes.contains_key(plugin) {
            let manifest = self
                .plugins
                .manifest(plugin)
                .cloned()
                .with_context(|| format!("No plugin {plugin}"))?;
            ensure!(
                self.plugin_enabled(plugin),
                "{} is disabled",
                self.plugins.source(plugin)
            );
            ensure!(
                self.plugin_granted(plugin),
                "{} needs its permissions accepted first",
                self.plugins.source(plugin)
            );
            ensure!(
                !self.plugin_offline(plugin),
                "{} {}",
                self.plugins.source(plugin),
                tr("uses the network, and plugins that use the network are disabled")
            );
            let data_dir = self.plugins.data_dir(plugin)?;
            let models_dir = self.plugins.models_dir(plugin)?;
            // Only files unchanged since they were verified, looked up
            // before the process starts.
            let models = self.model_paths(plugin);
            let env = vec![
                ("XUAN_PLUGIN_ID".to_owned(), plugin.to_owned()),
                ("XUAN_DATA_DIR".to_owned(), data_dir.display().to_string()),
                (
                    "XUAN_MODELS_DIR".to_owned(),
                    models_dir.display().to_string(),
                ),
                ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
            ];
            let context = self.context.clone();
            let wake: plugins::host::Wake = Arc::new(move || context.request_repaint());
            let blocked = self.plugin_network_blocked(plugin);
            let mut process = match Process::spawn(&manifest, &env, Some(wake), blocked) {
                Ok(process) => process,
                Err(error) if blocked => {
                    return Err(error.context(format!(
                        "{} {}",
                        self.plugins.source(plugin),
                        tr("did not start with its network blocked, and Xuan does not run it unfiltered while “Block network for plugins that don't declare it” is on in Settings")
                    )));
                }
                Err(error) => return Err(error),
            };
            let (settings, secrets) = self.plugin_settings(&manifest);
            // Started without waiting: the answer arrives with the other
            // messages, and what is sent meanwhile waits in the process.
            let id = process.initialize(json!({
                "protocol": plugins::manifest::PROTOCOL,
                "host": {"name": "Xuan", "version": env!("CARGO_PKG_VERSION")},
                "plugin_dir": manifest.dir,
                "data_dir": data_dir,
                "models_dir": models_dir,
                "models": models,
                "settings": settings,
                "secrets": secrets,
            }))?;
            self.plugins
                .pending
                .insert((plugin.to_owned(), id), Pending::Initialize);
            self.plugins
                .starting
                .insert(plugin.to_owned(), std::time::Instant::now());
            self.plugins.processes.insert(plugin.into(), process);
        }
        Ok(self.plugins.processes.get_mut(plugin).unwrap())
    }

    /// Service plugin messages. Called once per frame.
    pub(super) fn poll_plugins(&mut self) {
        let mut incoming: Vec<(String, Incoming)> = Vec::new();
        for (id, process) in &mut self.plugins.processes {
            incoming.extend(process.poll().into_iter().map(|m| (id.clone(), m)));
        }
        for (plugin, message) in incoming {
            self.dispatch_plugin_message(&plugin, message);
        }
        self.release_held_requests();
        self.release_held_edits();
        self.release_file_requests();
        self.check_starting_plugins();
        self.check_format_jobs();
        self.apply_completed_results();
        self.refresh_panes_for_changes();
        self.poll_model_jobs();
        if !self.plugins.jobs.is_empty()
            || !self.plugins.model_jobs.is_empty()
            || !self.plugins.starting.is_empty()
            || !self.plugins.formats.is_empty()
            || !self.plugins.completed.is_empty()
            || self
                .plugins
                .pending
                .values()
                .any(|p| matches!(p, Pending::Render(_)))
        {
            self.context
                .request_repaint_after(Duration::from_millis(150));
        }
    }

    fn dispatch_plugin_message(&mut self, plugin: &str, message: Incoming) {
        match message {
            Incoming::Message(Message::Request(request)) => {
                // Saving and opening files wait for the user's choice.
                if super::plugin_files::FILE_METHODS.contains(&request.method.as_str()) {
                    self.queue_file_request(plugin, request);
                    return;
                }
                // Direct edits wait for the session's answer when the plugin
                // asked for the prompt.
                let Some(request) = self.hold_edit(plugin, request) else {
                    return;
                };
                // Exports wait while the user is asked whether to send them.
                if super::plugin_consent::EXPORT_METHODS.contains(&request.method.as_str())
                    && self.export_answer(plugin).is_none()
                {
                    self.plugins.held.push((plugin.to_owned(), request));
                    return;
                }
                let result = self.service_request(plugin, &request);
                if let Some(process) = self.plugins.processes.get_mut(plugin) {
                    let _ = process.respond(request.id, result);
                }
            }
            Incoming::Message(Message::Notification(notification)) => {
                self.handle_notification(plugin, notification);
            }
            Incoming::Message(Message::Response(response)) => {
                self.handle_response(plugin, response);
            }
            Incoming::Invalid(_) => {}
            Incoming::Closed => self.plugin_closed(plugin),
        }
    }

    /// Clean up after a plugin whose process has died: handle what it sent
    /// last, fail its jobs and renders, and forget the process so the next
    /// use starts a new one.
    fn reap_plugin(&mut self, plugin: &str) {
        let Some(process) = self.plugins.processes.get_mut(plugin) else {
            return;
        };
        if process.alive() {
            return;
        }
        for message in process.poll() {
            self.dispatch_plugin_message(plugin, message);
        }
        if self.plugins.processes.contains_key(plugin) {
            self.plugin_closed(plugin);
        }
    }

    fn plugin_closed(&mut self, plugin: &str) {
        if self.plugins.starting.contains_key(plugin) {
            self.plugin_failed_to_start(plugin, tr("it exited"));
            return;
        }
        let name = self.plugins.source(plugin);
        self.plugins.processes.remove(plugin);
        self.plugins.forget_session(plugin);
        let failed: Vec<_> = self
            .plugins
            .jobs
            .iter()
            .filter(|job| job.plugin == plugin)
            .map(|job| job.id)
            .collect();
        let reason = format!("{name} {}", tr("stopped unexpectedly"));
        for id in failed {
            self.finish_plugin_job(id, Err(reason.clone()));
        }
        self.fail_format_jobs(plugin, Some(&reason));
        self.plugins.pending.retain(|(id, _), _| id != plugin);
        for (key, pane) in &mut self.plugins.panes {
            if split_pane_key(key).is_some_and(|(id, _)| id == plugin) {
                pane.pending = false;
                pane.dirty = false;
                pane.queued = None;
                pane.error = Some(format!("{name} {}", tr("stopped unexpectedly")));
            }
        }
    }

    /// A plugin that exited, failed or timed out before answering
    /// `initialize`: stop it and fail everything that waited for it, with the
    /// end of its log so the user can see why.
    fn plugin_failed_to_start(&mut self, plugin: &str, error: &str) {
        let name = self.plugins.source(plugin);
        let log = self.plugins.log(plugin);
        let tail = log[log.len().saturating_sub(5)..].join("\n");
        let message = format!("{name} {}: {error}\n{tail}", tr("did not start"))
            .trim_end()
            .to_owned();
        self.plugins.starting.remove(plugin);
        self.plugins.forget_session(plugin);
        if let Some(mut process) = self.plugins.processes.remove(plugin) {
            process.stop();
        }
        let failed: Vec<_> = (self.plugins.jobs.iter())
            .filter(|job| job.plugin == plugin)
            .map(|job| job.id)
            .collect();
        let mut reported = !failed.is_empty();
        for id in failed {
            self.finish_plugin_job(id, Err(message.clone()));
        }
        reported |= self.fail_format_jobs(plugin, Some(&message));
        self.plugins.pending.retain(|(id, _), _| id != plugin);
        for (key, pane) in &mut self.plugins.panes {
            if split_pane_key(key).is_some_and(|(id, _)| id == plugin) {
                pane.pending = false;
                pane.dirty = false;
                pane.queued = None;
                pane.error = Some(message.clone());
            }
        }
        if !reported {
            self.status = message.lines().next().unwrap_or_default().to_owned();
        }
    }

    /// Fail plugins that have not answered `initialize` in time.
    fn check_starting_plugins(&mut self) {
        let late: Vec<String> = (self.plugins.starting.iter())
            .filter(|(_, since)| since.elapsed() > INITIALIZE_TIMEOUT)
            .map(|(plugin, _)| plugin.clone())
            .collect();
        for plugin in late {
            self.plugin_failed_to_start(&plugin, tr("it did not answer in time"));
        }
    }

    pub(super) fn service_request(
        &mut self,
        plugin: &str,
        request: &Request,
    ) -> Result<Value, RpcError> {
        let params = &request.params;
        let string = |key: &str| params.get(key).and_then(Value::as_str).map(str::to_owned);
        let uuid = |key: &str| -> Result<Uuid, RpcError> {
            string(key)
                .and_then(|s| Uuid::parse_str(&s).ok())
                .ok_or_else(|| RpcError::invalid_params(format!("`{key}` must be a layer id")))
        };
        let max_side = params
            .get("max_side")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(16, 30_000) as u32);
        let internal =
            |error: anyhow::Error| RpcError::new(protocol::INTERNAL_ERROR, format!("{error:#}"));
        // A direct edit of a plugin that asks per session needs that answer.
        if let Some(error) = self.edit_refused(plugin, request) {
            return Err(error);
        }
        match request.method.as_str() {
            "session/status" => Ok(self.edit_session_status(plugin, request)),
            "document/get" => Ok(self
                .session()
                .map_or(Value::Null, |session| edits::describe(&session.document))),
            // The open tabs, without their paths.
            "document/list" => Ok(json!({
                "documents": (self.sessions.iter().enumerate())
                    .map(|(index, session)| json!({
                        "id": session.document.id,
                        "title": session.title,
                        "width": session.document.width,
                        "height": session.document.height,
                        "layers": session.document.layers.len(),
                        "current": index == self.current,
                        "modified": session.history.dirty(),
                        "saved": session.path.is_some(),
                    }))
                    .collect::<Vec<_>>(),
            })),
            // Switching tabs only changes the view, like `host/run` `fit`.
            "document/activate" => {
                let id = string("document")
                    .and_then(|s| Uuid::parse_str(&s).ok())
                    .ok_or_else(|| RpcError::invalid_params("`document` must be a document id"))?;
                if self.dialog.is_some() || self.job.is_some() || self.develop.is_some() {
                    return Err(RpcError::new(
                        protocol::INVALID_REQUEST,
                        "The editor is busy",
                    ));
                }
                let index = (self.sessions.iter())
                    .position(|s| s.document.id == id)
                    .ok_or_else(|| RpcError::invalid_params("No such open document"))?;
                if index != self.current {
                    // At most one switch a second, so the tabs cannot be
                    // made to flicker under the user's hands. Asking for the
                    // current document is free.
                    let now = std::time::Instant::now();
                    if let Some(last) = self.plugins.activated_at.get(plugin)
                        && now.duration_since(*last) < ACTIVATE_INTERVAL
                    {
                        let wait = ACTIVATE_INTERVAL - now.duration_since(*last);
                        let mut error = RpcError::new(
                            protocol::RATE_LIMITED,
                            "Switched documents less than a second ago; wait a moment before switching again",
                        );
                        error.data = json!({"retry_after": wait.as_secs_f64()});
                        return Err(error);
                    }
                    self.plugins.activated_at.insert(plugin.to_owned(), now);
                    // As clicking its tab does.
                    self.cancel_gesture();
                    self.current = index;
                    self.mask_target = false;
                }
                Ok(json!({"ok": true}))
            }
            // Answered once the user chose; see `plugin_files.rs`.
            "file/save_as" | "file/export" | "file/open" => Err(RpcError::new(
                protocol::INVALID_REQUEST,
                "File requests are answered after the user's choice",
            )),
            "layer/export" | "document/export" | "selection/export" => {
                if let Some(error) = self.export_refused(plugin) {
                    return Err(error);
                }
                // A folder the plugin names must be one of its own, unless
                // its manifest allows writing elsewhere.
                let dir = match string("dir") {
                    Some(dir) => self
                        .plugins
                        .access(plugin)
                        .writable_dir(Path::new(&dir))
                        .map_err(|e| RpcError::invalid_params(format!("{e:#}")))?,
                    None => self.plugins.scratch_dir(plugin).map_err(internal)?,
                };
                let Some(session) = self.session() else {
                    return Err(RpcError::new(
                        protocol::INVALID_PARAMS,
                        "No document is open",
                    ));
                };
                let name = format!("{}.png", Uuid::new_v4());
                let export = match request.method.as_str() {
                    "layer/export" => {
                        let what = match string("what").as_deref() {
                            Some("mask") => edits::What::Mask,
                            _ => edits::What::Pixels,
                        };
                        edits::export_layer(
                            &session.document,
                            uuid("layer")?,
                            what,
                            max_side,
                            &dir,
                            &name,
                        )
                    }
                    "document/export" => {
                        edits::export_composite(&session.document, max_side, &dir, &name)
                    }
                    _ => {
                        return edits::export_selection(&session.document, &dir, &name)
                            .map(|export| serde_json::to_value(export).unwrap_or(Value::Null))
                            .map_err(internal);
                    }
                };
                export
                    .map(|export| serde_json::to_value(export).unwrap_or(Value::Null))
                    .map_err(internal)
            }
            "document/edit" => {
                let manifest = self
                    .plugins
                    .manifest(plugin)
                    .ok_or_else(|| RpcError::new(protocol::INTERNAL_ERROR, "unknown plugin"))?;
                if manifest.permissions.document != DocumentAccess::Edit {
                    return Err(RpcError::new(
                        protocol::INVALID_REQUEST,
                        "The manifest does not declare document = \"edit\"",
                    ));
                }
                let name = string("name").unwrap_or_else(|| manifest.plugin.name.clone());
                let edits: Vec<edits::Edit> = serde_json::from_value(
                    params.get("edits").cloned().unwrap_or(Value::Array(vec![])),
                )
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                if self.dialog.is_some() || self.job.is_some() || self.gesture.is_some() {
                    return Err(RpcError::new(
                        protocol::INVALID_REQUEST,
                        "The editor is busy",
                    ));
                }
                if self.session().is_none() {
                    return Err(RpcError::new(
                        protocol::INVALID_PARAMS,
                        "No document is open",
                    ));
                }
                // Text is drawn with the editor's renderer, whose fonts are
                // loaded once.
                let mut reader = edits::Reader::new(self.plugins.access(plugin), edits::MAX_LAYERS)
                    .with_text_renderer(self.text_renderer.take());
                let session = self.session_mut().expect("checked above");
                let mut document = session.document.clone();
                let applied = edits::apply(&mut document, &edits, &mut reader);
                self.text_renderer = reader.take_text_renderer();
                let added = applied.map_err(internal)?;
                let session = self.session_mut().expect("checked above");
                session.history.begin(&name, &session.document);
                session.fit |= (document.width, document.height)
                    != (session.document.width, session.document.height);
                session.document = document;
                session.document.promote_image_masks();
                session.history.commit();
                session.invalidate();
                Ok(json!({"ok": true, "layers": added}))
            }
            "host/run" => {
                let action = string("action")
                    .ok_or_else(|| RpcError::invalid_params("`action` is required"))?;
                if self.dialog.is_some() || self.job.is_some() {
                    return Err(RpcError::new(
                        protocol::INVALID_REQUEST,
                        "The editor is busy",
                    ));
                }
                let edit = self
                    .plugins
                    .manifest(plugin)
                    .is_some_and(|m| m.permissions.document == DocumentAccess::Edit);
                match action.split_once('/') {
                    // A plugin may start its own actions, never another plugin's.
                    Some((owner, id)) if owner == plugin => {
                        let id = id.to_owned();
                        self.start_plugin_action_with(plugin, &id, params.get("inputs"));
                    }
                    Some(_) => {
                        return Err(RpcError::new(
                            protocol::INVALID_REQUEST,
                            "A plugin can only run its own actions",
                        ));
                    }
                    None if commands::host_run(&action) == HostRun::View
                        || (edit && commands::host_run(&action) == HostRun::Edit) =>
                    {
                        // As its menu item would be: greyed out commands do nothing.
                        if !self.command_enabled(&action) {
                            return Err(RpcError::new(
                                protocol::INVALID_REQUEST,
                                format!(
                                    "`{action}` is not available now, for example because \
                                     nothing is selected or the active layer does not suit it"
                                ),
                            ));
                        }
                        self.command(&action)
                    }
                    None if commands::host_run(&action) == HostRun::Edit => {
                        return Err(RpcError::new(
                            protocol::INVALID_REQUEST,
                            "The manifest does not declare document = \"edit\"",
                        ));
                    }
                    None => {
                        return Err(RpcError::new(
                            protocol::INVALID_REQUEST,
                            format!("Plugins cannot run `{action}`"),
                        ));
                    }
                }
                Ok(json!({"ok": true}))
            }
            "host/open" => {
                if let Some(path) = string("path") {
                    let path = (self.plugins.access(plugin))
                        .readable(Path::new(&path))
                        .map_err(|e| RpcError::invalid_params(format!("{e:#}")))?;
                    // Projects may be folders; anything else must be a
                    // regular file, as a FIFO would block the UI.
                    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file() || m.is_dir()) {
                        return Err(RpcError::invalid_params(format!(
                            "{} is not a regular file",
                            path.display()
                        )));
                    }
                    self.open_path(&path, false);
                    Ok(json!({"ok": true}))
                } else if let Some(url) = string("url") {
                    let url =
                        checked_url(&url).map_err(|e| RpcError::invalid_params(e.to_string()))?;
                    // One link at a time, so a plugin cannot flood the browser.
                    let now = std::time::Instant::now();
                    if self
                        .plugins
                        .last_link
                        .is_some_and(|last| now.duration_since(last) < LINK_INTERVAL)
                    {
                        return Err(RpcError::new(
                            protocol::RATE_LIMITED,
                            "Links can be opened at most once a second",
                        ));
                    }
                    self.plugins.last_link = Some(now);
                    self.context.open_url(egui::OpenUrl::new_tab(url));
                    Ok(json!({"ok": true}))
                } else {
                    Err(RpcError::invalid_params("`path` or `url` is required"))
                }
            }
            other => Err(RpcError::method_not_found(other)),
        }
    }

    pub(super) fn handle_notification(&mut self, plugin: &str, notification: Notification) {
        let params = &notification.params;
        match notification.method.as_str() {
            "job/progress" => {
                let job = params
                    .get("job")
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok());
                if let Some(job) = self
                    .plugins
                    .jobs
                    .iter_mut()
                    .find(|j| j.plugin == plugin && Some(j.id) == job)
                {
                    if let Some(fraction) = params.get("fraction").and_then(Value::as_f64) {
                        job.progress = Some(fraction.clamp(0.0, 1.0) as f32);
                    }
                    if let Some(message) = params.get("message").and_then(Value::as_str) {
                        job.message = message.chars().take(200).collect();
                    }
                }
            }
            "host/log" => {
                if let (Some(process), Some(message)) = (
                    self.plugins.processes.get(plugin),
                    params.get("message").and_then(Value::as_str),
                ) {
                    process.note(message.to_owned());
                }
            }
            "request/cancel" => {
                if let Some(id) =
                    (params.get("id").cloned()).and_then(|id| serde_json::from_value::<Id>(id).ok())
                {
                    self.withdraw_request(plugin, &id);
                }
            }
            "host/status" => {
                if let Some(message) = params.get("message").and_then(Value::as_str) {
                    self.status = self.plugins.status_from(plugin, message);
                }
            }
            "pane/update" => {
                // Only panes the manifest declares, so a plugin cannot grow
                // the pane table without bound.
                if let Some(pane) = params.get("pane").and_then(Value::as_str)
                    && self
                        .plugins
                        .manifest(plugin)
                        .is_some_and(|m| m.pane(pane).is_some())
                {
                    let key = pane_key(plugin, pane);
                    let state = self.plugins.panes.entry(key).or_default();
                    match Node::parse(params.get("tree").cloned().unwrap_or(Value::Null)) {
                        Ok(tree) => {
                            state.tree = Some(tree);
                            state.error = None;
                        }
                        Err(error) => state.error = Some(error.to_string()),
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_response(&mut self, plugin: &str, response: Response) {
        let Some(pending) = self
            .plugins
            .pending
            .remove(&(plugin.to_owned(), response.id))
        else {
            return;
        };
        let result = match (response.result, response.error) {
            (Some(value), _) => Ok(value),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(Value::Null),
        };
        match pending {
            Pending::Initialize => {
                self.plugins.starting.remove(plugin);
                let version = result
                    .as_ref()
                    .ok()
                    .and_then(|value| value.get("protocol"))
                    .and_then(Value::as_u64);
                let error = match result {
                    Err(error) => Some(error.message),
                    Ok(_)
                        if version.is_some_and(|v| v != u64::from(plugins::manifest::PROTOCOL)) =>
                    {
                        Some(format!(
                            "{} {}",
                            tr("it speaks protocol"),
                            version.unwrap_or_default()
                        ))
                    }
                    Ok(_) => (self.plugins.processes.get_mut(plugin))
                        .and_then(|process| process.set_ready().err())
                        .map(|error| error.to_string()),
                };
                if let Some(error) = error {
                    self.plugin_failed_to_start(plugin, &error);
                }
            }
            Pending::Job(id) => {
                let result = result.map_err(|error| self.describe_rpc_error(plugin, &error));
                self.finish_plugin_job(id, result);
            }
            Pending::Format(id) => {
                let result = result.map_err(|error| self.describe_rpc_error(plugin, &error));
                self.finish_format_job(id, result);
            }
            Pending::Render(key) => {
                let Some(state) = self.plugins.panes.get_mut(&key) else {
                    return;
                };
                state.pending = false;
                match result {
                    Ok(Value::Null) => {}
                    Ok(value) => match Node::parse(value) {
                        Ok(tree) => {
                            state.tree = Some(tree);
                            state.error = None;
                        }
                        Err(error) => state.error = Some(error.to_string()),
                    },
                    Err(error) => state.error = Some(error.message),
                }
                // An event render describes the current document too.
                let dirty = std::mem::take(&mut state.dirty);
                if let Some(event) = state.queued.take() {
                    self.render_pane(&key, "event", Some(event));
                } else if dirty {
                    self.render_pane(&key, "document", None);
                }
            }
            Pending::Estimate(plugin_id, action) => {
                if let Some(edit) = &mut self.plugins.action
                    && edit.plugin == plugin_id
                    && edit.action == action
                    && let Ok(value) = result
                {
                    let cost = value.get("cost").and_then(Value::as_str).unwrap_or("");
                    let seconds = value.get("seconds").and_then(Value::as_f64);
                    let mut text = cost.to_owned();
                    if let Some(seconds) = seconds {
                        if !text.is_empty() {
                            text.push_str(" · ");
                        }
                        text.push_str(&format!("≈{seconds:.0} s"));
                    }
                    edit.estimate = (!text.is_empty()).then_some(text);
                }
            }
        }
    }

    pub(super) fn describe_rpc_error(&self, plugin: &str, error: &RpcError) -> String {
        let name = self.plugins.source(plugin);
        match error.code {
            protocol::CANCELLED => tr("Cancelled").into(),
            protocol::NEEDS_SETUP => format!(
                "{name}: {}\n\n{}",
                error.message,
                tr("Open Plugins → Manage Plugins… to configure it.")
            ),
            protocol::INSUFFICIENT_CREDITS => format!("{name}: {}", error.message),
            protocol::RATE_LIMITED => {
                let retry = error.data.get("retry_after").and_then(Value::as_f64);
                match retry {
                    Some(seconds) => format!(
                        "{name}: {} ({} {seconds:.0} s)",
                        error.message,
                        tr("retry in")
                    ),
                    None => format!("{name}: {}", error.message),
                }
            }
            _ => format!("{name}: {}", error.message),
        }
    }

    // --- Actions ---------------------------------------------------------

    /// Menu entries grouped by the menu they asked for.
    /// Menu items of plugin actions. The label is the registry's, which
    /// names the plugin, so plugin items stand out.
    pub(super) fn plugin_menu_items(&self) -> HashMap<Menu, Vec<PluginMenuItem>> {
        let mut items: HashMap<Menu, Vec<_>> = HashMap::new();
        for manifest in &self.plugins.manifests {
            if !self.plugin_enabled(&manifest.plugin.id) {
                continue;
            }
            let mut source = format!(
                "{}\n{}",
                self.plugins.source(&manifest.plugin.id),
                manifest.dir.display()
            );
            if self.plugin_offline(&manifest.plugin.id) {
                source.push_str(&format!(
                    "\n{}",
                    tr("Off: plugins that use the network are disabled")
                ));
            }
            for action in &manifest.actions {
                let id = format!("{}/{}", manifest.plugin.id, action.id);
                let label = self.keymap.get(&id).map_or_else(
                    || super::commands::plugin_action_label(&action.label, &manifest.plugin.name),
                    |entry| entry.label().to_owned(),
                );
                items.entry(action.menu).or_default().push(PluginMenuItem {
                    label,
                    plugin: manifest.plugin.id.clone(),
                    action: action.id.clone(),
                    shortcut: self.keymap.shortcut(&id),
                    source: source.clone(),
                    enabled: self.command_enabled(&id),
                });
            }
        }
        items
    }

    /// Open the action's dialog, or run it straight away when it has no inputs.
    /// Runs a plugin action as its menu item, shortcut or `host/run` does. A provider
    /// run still waiting for its action is dropped: this is the action itself.
    pub(super) fn start_plugin_action(&mut self, plugin: &str, action: &str) {
        self.plugins.provider_pending = None;
        self.start_plugin_action_with(plugin, action, None);
    }

    pub(super) fn start_plugin_action_with(
        &mut self,
        plugin: &str,
        action: &str,
        inputs: Option<&Value>,
    ) {
        let Some(manifest) = self.plugins.manifest(plugin).cloned() else {
            return;
        };
        let Some(spec) = manifest.action(action).cloned() else {
            return;
        };
        if !self.plugin_enabled(plugin) {
            return;
        }
        if self.plugin_offline(plugin) {
            self.status = format!(
                "{} {}",
                self.plugins.source(plugin),
                tr("uses the network, and plugins that use the network are disabled")
            );
            return;
        }
        if !self.plugin_granted(plugin) {
            self.plugins.permission_request =
                Some((plugin.into(), PendingStart::Action(action.into())));
            self.dialog = Some(Dialog::PluginPermissions);
            return;
        }
        if !self.action_models_ready(plugin, action, inputs) {
            return;
        }
        if self
            .plugins
            .jobs
            .iter()
            .any(|job| Some(job.document) == self.session().map(|s| s.document.id))
        {
            self.error = Some(tr("A plugin action is already running on this document").into());
            return;
        }
        if spec.needs_image()
            && self
                .session()
                .and_then(|s| s.document.active())
                .is_none_or(|layer| layer.pixels.is_none() || layer.group)
            && spec.source.from == plugins::manifest::SourceKind::Layer
        {
            self.error = Some(tr("Select an image layer first").into());
            return;
        }
        if spec.kind != ActionKind::Generate && self.session().is_none() {
            self.error = Some(tr("Open a document first").into());
            return;
        }
        if spec.needs_image()
            && spec.source.mask == plugins::manifest::SourceMask::Selection
            && spec.source.mask_empty == plugins::manifest::MaskEmpty::Error
            && self
                .session()
                .is_some_and(|s| s.document.selection.is_none())
        {
            self.error = Some(tr("Select an area first").into());
            return;
        }
        let mut values = Map::new();
        let mut regions = Vec::new();
        for input in &spec.inputs {
            // Inputs from a project file or a plugin are checked against
            // the action's spec before the dialog shows them.
            let value = match inputs.and_then(|i| i.get(&input.id)) {
                Some(previous) => input.coerce(previous),
                None => input.initial(),
            };
            if input.kind == InputKind::Regions {
                regions = regions_from_value(&value);
                continue;
            }
            values.insert(input.id.clone(), value);
        }
        let provider = (self.plugins.provider_pending.take())
            .filter(|run| run.plugin == plugin && run.action == action);
        if spec.inputs.is_empty() || provider.is_some() {
            self.plugins.action = Some(ActionEdit {
                plugin: plugin.into(),
                action: action.into(),
                values,
                regions,
                selected: None,
                estimate: None,
                previous_tool: self.tool,
                into: ResultInto::Layer,
                consented: false,
                provider,
            });
            self.run_plugin_action();
            return;
        }
        let previous_tool = self.tool;
        self.plugins.action = Some(ActionEdit {
            plugin: plugin.into(),
            action: action.into(),
            values,
            regions,
            selected: None,
            estimate: None,
            previous_tool,
            into: if self.session().is_some() {
                ResultInto::Layer
            } else {
                ResultInto::Document
            },
            consented: false,
            provider: None,
        });
        if spec.regions_input().is_some() {
            self.set_tool(Tool::Region);
        }
        self.request_estimate();
    }

    pub(super) fn close_plugin_action(&mut self) {
        if let Some(edit) = self.plugins.action.take()
            && self.tool == Tool::Region
        {
            self.set_tool(if edit.previous_tool == Tool::Region {
                Tool::Move
            } else {
                edit.previous_tool
            });
        }
    }

    fn action_params(
        &mut self,
        job: Uuid,
        work_dir: &Path,
        estimate: bool,
    ) -> Result<(Value, Prepared, Vec<Region>, Value)> {
        let edit = self.plugins.action.as_ref().context("no action")?;
        let manifest = self.plugins.manifest(&edit.plugin).context("no plugin")?;
        let spec = manifest.action(&edit.action).context("no action")?.clone();
        let regions = edit.regions.clone();
        let mut inputs = edit.values.clone();
        // Before the user confirmed sending document data, an estimate gets
        // neither the image nor the regions and texts.
        let withheld = estimate && self.sends_need_consent(&edit.plugin);
        let document = self.session().map(|s| s.document.clone());
        let prepared = match (&document, spec.kind) {
            (Some(document), ActionKind::Edit)
                if !withheld
                    && (!estimate || spec.source.from != plugins::manifest::SourceKind::None) =>
            {
                jobs::prepare(
                    document,
                    &spec.source.with_inputs(&edit.values),
                    &regions,
                    work_dir,
                )?
            }
            _ => Prepared::none(),
        };
        if withheld {
            for input in &spec.inputs {
                if matches!(
                    input.kind,
                    InputKind::Text | InputKind::Multiline | InputKind::Path | InputKind::Secret
                ) {
                    inputs.remove(&input.id);
                }
            }
        }
        if let Some(input) = spec.regions_input() {
            inputs.insert(input.id.clone(), Value::Array(prepared.regions.clone()));
        }
        // A provider run says what it stands in for and where the user pointed, in the
        // source's pixels like everything else the plugin gets.
        if let Some(run) = &edit.provider {
            inputs.insert("capability".into(), json!(run.capability.id()));
            if let Some(point) = run.point {
                let (x, y) = prepared.from_document(point);
                inputs.insert("point".into(), json!({"x": x, "y": y}));
            }
            if let Some([left, top, right, bottom]) = run.rect {
                let corners = [(left, top), (right, top), (right, bottom), (left, bottom)]
                    .map(|(x, y)| prepared.from_document(xuan::document::Point::new(x, y)));
                let min_x = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
                let min_y = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min);
                let max_x = corners
                    .iter()
                    .map(|c| c.0)
                    .fold(f32::NEG_INFINITY, f32::max);
                let max_y = corners
                    .iter()
                    .map(|c| c.1)
                    .fold(f32::NEG_INFINITY, f32::max);
                inputs.insert(
                    "rect".into(),
                    json!({"x": min_x, "y": min_y, "width": max_x - min_x, "height": max_y - min_y}),
                );
            }
        }
        // Inputs as stored on the layer: regions in document coordinates.
        let mut stored = edit.values.clone();
        if let Some(input) = spec.regions_input() {
            stored.insert(input.id.clone(), regions_to_value(&regions));
        }
        let params = json!({
            "job": job,
            "action": spec.id,
            "work_dir": work_dir,
            "inputs": inputs,
            "source": prepared.describe(),
            "document": document.as_ref().map(edits::describe),
        });
        Ok((params, prepared, regions, Value::Object(stored)))
    }

    fn request_estimate(&mut self) {
        let Some(edit) = &self.plugins.action else {
            return;
        };
        let (plugin, action) = (edit.plugin.clone(), edit.action.clone());
        let Ok(work_dir) = self.plugins.scratch_dir(&plugin) else {
            return;
        };
        let Ok((params, ..)) = self.action_params(Uuid::new_v4(), &work_dir, true) else {
            return;
        };
        let Ok(process) = self.plugin_process(&plugin) else {
            return;
        };
        if let Ok(id) = process.request("action/estimate", params) {
            self.plugins
                .pending
                .insert((plugin.clone(), id), Pending::Estimate(plugin, action));
        }
    }

    /// Send the open action to its plugin as a background job.
    pub(super) fn run_plugin_action(&mut self) {
        let Some(edit) = &self.plugins.action else {
            return;
        };
        let plugin = edit.plugin.clone();
        let Some(manifest) = self.plugins.manifest(&plugin).cloned() else {
            return;
        };
        let Some(spec) = manifest.action(&edit.action).cloned() else {
            return;
        };
        if let Some(input) = spec.regions_input()
            && let Some(min) = input.min
            && (edit.regions.len() as f64) < min
        {
            self.error = Some(format!(
                "{} {}",
                tr("Draw at least this many regions:"),
                min as u32
            ));
            return;
        }
        if let Some(input) = spec.regions_input()
            && edit.regions.len() > region_limit(input)
        {
            self.error = Some(format!(
                "{} {}",
                tr("Draw at most this many regions:"),
                region_limit(input)
            ));
            return;
        }
        let chosen = edit.into;
        // A run that sends document data to a plugin that declares network
        // hosts waits for the user to confirm it.
        let consented =
            (self.plugins.action.as_mut()).is_some_and(|edit| std::mem::take(&mut edit.consented));
        if !consented && self.sends_need_consent(&plugin) {
            let items = self.action_consent_items(&spec);
            if !items.is_empty() {
                self.plugins.consent = Some(super::plugin_consent::ConsentRequest {
                    plugin: plugin.clone(),
                    action: Some(spec.id.clone()),
                    items,
                    dont_ask: false,
                });
                self.dialog = Some(Dialog::PluginConsent);
                return;
            }
        }
        let document = self.session().map(|s| s.document.id);
        let provider = (self.plugins.action.as_ref()).and_then(|edit| edit.provider.clone());
        let job = Uuid::new_v4();
        let result = (|| -> Result<()> {
            let work_dir = plugins::private_dir("xuan-job-")?;
            let (params, prepared, regions, inputs) =
                self.action_params(job, work_dir.path(), false)?;
            let process = self.plugin_process(&plugin)?;
            let id = process.request("action/run", params)?;
            self.plugins
                .pending
                .insert((plugin.clone(), id), Pending::Job(job));
            self.plugins.jobs.push(PluginJob {
                id: job,
                plugin: plugin.clone(),
                action: spec.id.clone(),
                label: spec.label.trim_end_matches('…').to_owned(),
                document: document.unwrap_or_default(),
                _work_dir: work_dir,
                prepared,
                regions,
                inputs,
                into: if spec.result.into == ResultInto::Ask {
                    chosen
                } else {
                    spec.result.into
                },
                mask_to_regions: spec.result.mask_to_regions,
                progress: None,
                message: String::new(),
                cancelled: false,
                consented,
                provider,
            });
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.close_plugin_action();
                self.status = format!("{} {}", spec.label.trim_end_matches('…'), tr("started"));
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    pub(super) fn cancel_plugin_job(&mut self, id: Uuid) {
        if let Some(job) = self.plugins.jobs.iter_mut().find(|job| job.id == id) {
            job.cancelled = true;
            let plugin = job.plugin.clone();
            let notified = self
                .plugins
                .processes
                .get_mut(&plugin)
                .is_some_and(|process| {
                    process.alive() && process.notify("job/cancel", json!({"job": id})).is_ok()
                });
            if !notified {
                // Nothing will answer; end the job now.
                self.finish_plugin_job(id, Err(tr("Cancelled").into()));
            }
        }
    }

    fn finish_plugin_job(&mut self, id: Uuid, result: Result<Value, String>) {
        let Some(index) = self.plugins.jobs.iter().position(|job| job.id == id) else {
            return;
        };
        let job = self.plugins.jobs.remove(index);
        let value = match result {
            Ok(value) if !job.cancelled => value,
            Ok(_) => {
                self.status = tr("Cancelled").into();
                return;
            }
            Err(error) => {
                if job.cancelled || error == tr("Cancelled") {
                    self.status = tr("Cancelled").into();
                } else {
                    self.error = Some(error);
                }
                return;
            }
        };
        if !self.ready_for_plugin_result() {
            self.status = format!(
                "{} {}",
                job.label,
                tr("finished; its result is shown when the editor is free")
            );
        }
        self.plugins.completed.push_back((job, value));
        self.apply_completed_results();
    }

    /// Whether a job result can be shown now: nothing else is open or under way
    /// that a proposal would interrupt.
    fn ready_for_plugin_result(&self) -> bool {
        self.dialog.is_none()
            && self.error.is_none()
            && self.plugins.proposal.is_none()
            && self.job.is_none()
            && self.gesture.is_none()
            && self.effect.is_none()
            && self.text_edit.is_none()
            && self.develop.is_none()
    }

    /// Apply finished results in the order their jobs finished, one at a time:
    /// a result that becomes a proposal waits for the user before the next.
    pub(super) fn apply_completed_results(&mut self) {
        while self.ready_for_plugin_result()
            && let Some((job, value)) = self.plugins.completed.pop_front()
        {
            if let Err(error) = self.apply_job_result(&job, value) {
                self.error = Some(format!("{}: {error:#}", job.label));
            }
        }
    }

    pub(super) fn apply_job_result(&mut self, job: &PluginJob, value: Value) -> Result<()> {
        #[derive(serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Output {
            Image {
                path: PathBuf,
                #[serde(default)]
                name: Option<String>,
                #[serde(default)]
                x: f32,
                #[serde(default)]
                y: f32,
                #[serde(default)]
                mask: Option<PathBuf>,
                #[serde(default)]
                provenance: Option<Value>,
                #[serde(flatten)]
                placed: jobs::Placed,
            },
            Document {
                path: PathBuf,
                #[serde(default)]
                name: Option<String>,
                #[serde(default)]
                provenance: Option<Value>,
            },
            /// A grey PNG that becomes the selection, combined with the
            /// current one by `mode`.
            Mask {
                path: PathBuf,
                #[serde(default)]
                mode: SelectionMode,
                #[serde(default)]
                x: f32,
                #[serde(default)]
                y: f32,
                #[serde(flatten)]
                placed: jobs::Placed,
            },
            Edit {
                edits: Vec<edits::Edit>,
            },
            Text {
                text: String,
            },
            None,
        }
        let outputs: Vec<Output> = serde_json::from_value(
            value
                .get("outputs")
                .cloned()
                .unwrap_or(Value::Array(vec![])),
        )
        .context("The plugin returned malformed outputs")?;
        ensure!(
            outputs.len() <= edits::MAX_OUTPUTS,
            "The plugin returned more than {} outputs",
            edits::MAX_OUTPUTS
        );
        let edit_count: usize = (outputs.iter())
            .map(|output| match output {
                Output::Edit { edits } => edits.len(),
                _ => 0,
            })
            .sum();
        ensure!(
            edit_count <= edits::MAX_EDITS,
            "The plugin returned more than {} edits",
            edits::MAX_EDITS
        );
        let manifest = self.plugins.manifest(&job.plugin).cloned();
        // Extending the canvas is a document change, even as a proposal.
        let result_edits = || {
            outputs.iter().flat_map(|output| match output {
                Output::Edit { edits } => edits.as_slice(),
                _ => &[],
            })
        };
        if let Some(edit) = result_edits().find(|edit| edit.direct_only()) {
            bail!(
                "The plugin returned a {} edit, which changes the canvas; send it with \
                 document/edit instead of in a result",
                edit.op()
            );
        }
        if result_edits().any(edits::Edit::needs_edit_access) {
            ensure!(
                manifest
                    .as_ref()
                    .is_some_and(|m| m.permissions.document == DocumentAccess::Edit),
                "extend_canvas needs document = \"edit\" in the plugin's manifest"
            );
        }
        // A plugin that may only read proposes selections and new documents.
        // Refuse the whole result before anything is applied.
        if !manifest
            .as_ref()
            .is_some_and(|m| m.permissions.document == DocumentAccess::Edit)
        {
            let new_document = job.into == ResultInto::Document
                || (job.prepared.export.is_none()
                    && !self.sessions.iter().any(|s| s.document.id == job.document));
            let changes = outputs.iter().find_map(|output| match output {
                Output::Image { .. } if !new_document => Some("an image placed in the document"),
                Output::Edit { edits } if !edits.iter().all(edits::Edit::is_selection_only) => {
                    Some("an edit")
                }
                _ => None,
            });
            if let Some(what) = changes {
                bail!(
                    "The plugin returned {what}, which changes the document, but its manifest \
                     has document = \"read\". A read-only plugin may only return selections \
                     (masks), new documents and text; it needs document = \"edit\" for more"
                );
            }
        }
        // Images and masks are placed on the document as it was sent; when
        // the result also extends the canvas they move with its content.
        let (dx, dy) = edits::origin_shift(result_edits());
        // Every image of the result shares one pixel and layer budget, and
        // is read only from the plugin's folders, including this job's.
        let access = self.plugins.access(&job.plugin).with(job._work_dir.path());
        let mut reader = edits::Reader::new(access, edits::MAX_LAYERS);
        let generated = |source: Option<Uuid>, hash: Option<String>| Generated {
            plugin: job.plugin.clone(),
            version: manifest
                .as_ref()
                .map(|m| m.plugin.version.clone())
                .unwrap_or_default(),
            action: job.action.clone(),
            inputs: job.inputs.clone(),
            source,
            source_hash: hash,
            created: timestamp(),
        };
        // What a layer's provenance must never hold: this plugin's secrets.
        let redactor = self.provenance_redactor(&job.plugin);
        let mut stripped = 0;
        let mut message = None;
        let mut new_documents = Vec::new();
        let mut layers: Vec<Layer> = Vec::new();
        let mut replace: Option<(Uuid, image::RgbaImage, Option<Provenance>)> = None;
        let mut edit_batches = Vec::new();
        let mut masks = Vec::new();
        let regions =
            (!job.regions.is_empty() && job.mask_to_regions).then_some(job.regions.as_slice());
        for output in outputs {
            match output {
                Output::Image {
                    path,
                    name,
                    x,
                    y,
                    mask,
                    provenance,
                    placed,
                } => {
                    placed.validate()?;
                    reader.add_layer()?;
                    let provenance = take_provenance(provenance, &redactor, &mut stripped)?;
                    let image = reader.rgba(&path)?;
                    let name = name.unwrap_or_else(|| job.label.clone());
                    let session = self.sessions.iter().find(|s| s.document.id == job.document);
                    if job.into == ResultInto::Document
                        || (job.prepared.export.is_none() && session.is_none())
                    {
                        new_documents.push((name, image, provenance));
                        continue;
                    }
                    let source_layer = job.prepared.layer.filter(|id| {
                        session.is_some_and(|s| s.document.layers.iter().any(|l| l.id == *id))
                    });
                    if job.into == ResultInto::Replace
                        && let Some(source) = source_layer
                        && let Some(pixels) = session
                            .and_then(|s| s.document.layers.iter().find(|l| l.id == source))
                            .and_then(|l| l.pixels.as_deref())
                    {
                        replace = Some((
                            source,
                            jobs::replace_pixels(&job.prepared, pixels, &image, x, y, &placed)?,
                            provenance,
                        ));
                        continue;
                    }
                    let mut layer = if job.prepared.export.is_some() {
                        jobs::place_layer(&job.prepared, &name, image, x, y, &placed, regions)?
                    } else {
                        let transform =
                            jobs::unsourced_placement(image.dimensions(), x, y, &placed)?;
                        let mut layer = Layer::image(name, image);
                        layer.transform = transform;
                        layer
                    };
                    if let Some(mask) = mask {
                        layer.mask = Some(xuan::document::Mask {
                            pixels: Arc::new(reader.gray(&mask)?),
                            ..xuan::document::Mask::white()
                        });
                    }
                    layer.generated = Some(generated(source_layer, job.prepared.hash.clone()));
                    layer.provenance = provenance;
                    layer.transform.x += dx;
                    layer.transform.y += dy;
                    layers.push(layer);
                }
                Output::Document {
                    path,
                    name,
                    provenance,
                } => {
                    reader.add_layer()?;
                    let provenance = take_provenance(provenance, &redactor, &mut stripped)?;
                    new_documents.push((
                        name.unwrap_or_else(|| job.label.clone()),
                        reader.rgba(&path)?,
                        provenance,
                    ));
                }
                Output::Mask {
                    path,
                    mode,
                    x,
                    y,
                    placed,
                } => {
                    placed.validate()?;
                    masks.push((reader.gray(&path)?, mode, x, y, placed));
                }
                Output::Edit { edits } => edit_batches.push(edits),
                Output::Text { text } => message = Some(text.chars().take(500).collect::<String>()),
                Output::None => {}
            }
        }
        for (name, image, provenance) in new_documents {
            let mut document = Document::new(image.width(), image.height())?;
            let mut layer = Layer::image(&name, image);
            layer.generated = Some(generated(None, None));
            layer.provenance = provenance;
            document.layers = vec![layer];
            document.active = Some(document.layers[0].id);
            document.selected = [document.layers[0].id].into();
            self.sessions.push(Session::new(document, name, None));
            self.current = self.sessions.len() - 1;
            self.session_mut().unwrap().history.mark_modified();
        }
        let has_changes = !layers.is_empty()
            || replace.is_some()
            || !edit_batches.is_empty()
            || !masks.is_empty();
        if let Some(message) = &message {
            self.status = self.plugins.status_from(&job.plugin, message);
        }
        if stripped > 0 {
            self.status = format!(
                "{} {} {}",
                job.label,
                stripped,
                tr("provenance entries that looked like secrets were removed")
            );
        }
        if !has_changes {
            return Ok(());
        }
        let Some(index) = self
            .sessions
            .iter()
            .position(|s| s.document.id == job.document)
        else {
            bail!("{}", tr("The document was closed"));
        };
        self.current = index;
        let session = &mut self.sessions[index];
        let mut document = session.document.clone();
        for batch in &edit_batches {
            edits::apply(&mut document, batch, &mut reader)?;
        }
        if let Some((id, pixels, provenance)) = replace
            && let Some(layer) = document.layers.iter_mut().find(|l| l.id == id)
        {
            layer.provenance = provenance;
            layer.raw = None;
            layer.text = None;
            layer.shape = None;
            layer.pixels = Some(Arc::new(pixels));
            layer.generated = Some(generated(Some(id), job.prepared.hash.clone()));
        }
        if let Some(source) = job
            .prepared
            .layer
            .filter(|id| document.layers.iter().any(|l| l.id == *id))
        {
            document.select(source, false);
        }
        let mut ids: Vec<Uuid> = layers.iter().map(|l| l.id).collect();
        for layer in layers {
            document.insert(layer);
        }
        // Masks become the selection, in order, after every other change.
        // Changing the selection is not a pixel edit, so any plugin may.
        let mut before = (!masks.is_empty()).then(|| document.selection.clone());
        let mut shifted = job.prepared.clone();
        shifted.transform.x += dx;
        shifted.transform.y += dy;
        for (mask, mode, x, y, placed) in masks {
            let size = (document.width, document.height);
            // Without a source, `x`, `y` are document units themselves.
            let (x, y) = if shifted.export.is_some() {
                (x, y)
            } else {
                (x + dx, y + dy)
            };
            let coverage = jobs::place_mask(&shifted, size, &mask, x, y, &placed)?;
            // A provider's mask combines as the command the user ran asked.
            let mode = job.provider.as_ref().map_or(mode, |run| run.mode);
            selection::combine(&mut document, coverage, mode);
        }
        // Remove Background through a provider: the host lays the mask on the layer
        // the user chose, as the built-in command does; the selection stays as it was.
        if let Some(run) =
            (job.provider.as_ref()).filter(|run| run.capability == Capability::RemoveBackground)
            && let Some(old_selection) = before.take()
        {
            ensure!(
                manifest
                    .as_ref()
                    .is_some_and(|m| m.permissions.document == DocumentAccess::Edit),
                "Remove Background needs document = \"edit\" in the plugin's manifest"
            );
            let index = (run.layer)
                .and_then(|id| document.layers.iter().position(|l| l.id == id))
                .filter(|&i| document.layers[i].pixels.is_some() && !document.layers[i].locked)
                .context(tr("Select an unlocked image layer"))?;
            let matte = xuan::paint::mask_from_selection(&document, &document.layers[index]);
            xuan::retouch::apply_layer_matte(&mut document.layers[index], matte);
            document.selection = old_selection;
            let existing: std::collections::HashSet<Uuid> =
                document.layers.iter().map(|l| l.id).collect();
            document.promote_image_masks();
            ids.extend(
                (document.layers.iter())
                    .map(|l| l.id)
                    .filter(|id| !existing.contains(id)),
            );
        }
        let selection = before.map(|before| ProposedSelection {
            before,
            after: document.selection.clone(),
        });
        let name = (job.provider.as_ref()).map_or(job.label.clone(), |run| {
            tr(run.capability.label()).to_owned()
        });
        document.validate()?;
        session.history.begin(&name, &session.document);
        // A result that resized the canvas is shown whole.
        session.fit |=
            (document.width, document.height) != (session.document.width, session.document.height);
        session.document = document;
        session.invalidate();
        self.plugins.proposal = Some(Proposal {
            document: job.document,
            name,
            source: self.plugins.source(&job.plugin),
            layers: ids,
            selection,
            comparing: false,
            message,
        });
        self.dialog = Some(Dialog::PluginProposal);
        Ok(())
    }

    pub(super) fn resolve_proposal(&mut self, accept: bool) {
        let Some(proposal) = self.plugins.proposal.take() else {
            return;
        };
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|s| s.document.id == proposal.document)
        {
            if accept {
                for layer in &mut session.document.layers {
                    if proposal.layers.contains(&layer.id) {
                        layer.visible = true;
                    }
                }
                if let Some(selection) = proposal.selection {
                    session.document.selection = selection.after;
                }
                session.history.commit();
                self.status = format!("{} {}", proposal.name, tr("applied"));
            } else {
                let size = (session.document.width, session.document.height);
                session.history.cancel(&mut session.document);
                session.fit |= size != (session.document.width, session.document.height);
                self.status = format!("{} {}", proposal.name, tr("discarded"));
            }
            session.invalidate();
        }
        if self.dialog == Some(Dialog::PluginProposal) {
            self.dialog = None;
        }
    }

    pub(super) fn toggle_proposal_compare(&mut self) {
        let Some(proposal) = &mut self.plugins.proposal else {
            return;
        };
        proposal.comparing = !proposal.comparing;
        let comparing = proposal.comparing;
        let layers = proposal.layers.clone();
        let document = proposal.document;
        let selection = proposal.selection.as_ref().map(|selection| {
            if comparing {
                selection.before.clone()
            } else {
                selection.after.clone()
            }
        });
        if let Some(session) = self.sessions.iter_mut().find(|s| s.document.id == document) {
            for layer in &mut session.document.layers {
                if layers.contains(&layer.id) {
                    layer.visible = !comparing;
                }
            }
            if let Some(selection) = selection {
                session.document.selection = selection;
            }
            session.invalidate();
        }
    }

    // --- Regions ---------------------------------------------------------

    pub(super) fn add_region(&mut self, start: xuan::document::Point, end: xuan::document::Point) {
        let Some(edit) = &mut self.plugins.action else {
            return;
        };
        let Some(manifest) = self
            .plugins
            .manifests
            .iter()
            .find(|m| m.plugin.id == edit.plugin)
        else {
            return;
        };
        let Some(input) = manifest
            .action(&edit.action)
            .and_then(Action::regions_input)
        else {
            return;
        };
        let x = start.x.min(end.x);
        let y = start.y.min(end.y);
        let width = (start.x - end.x).abs();
        let height = (start.y - end.y).abs();
        if width < 2.0 || height < 2.0 {
            return;
        }
        if edit.regions.len() >= region_limit(input) {
            self.status = tr("No more regions can be added").into();
            return;
        }
        let mut region = Region::rect(x, y, width, height);
        for field in &input.fields {
            region.fields.insert(field.id.clone(), field.initial());
        }
        edit.regions.push(region);
        edit.selected = Some(edit.regions.len() - 1);
    }

    pub(super) fn add_selection_region(&mut self) {
        let Some(selection) = self.session().and_then(|s| s.document.selection.clone()) else {
            self.status = tr("Make a selection first").into();
            return;
        };
        let Some(mut region) = Region::from_mask(selection) else {
            return;
        };
        let Some(edit) = &mut self.plugins.action else {
            return;
        };
        let Some(input) = self
            .plugins
            .manifests
            .iter()
            .find(|m| m.plugin.id == edit.plugin)
            .and_then(|m| m.action(&edit.action))
            .and_then(Action::regions_input)
        else {
            return;
        };
        if edit.regions.len() >= region_limit(input) {
            self.status = tr("No more regions can be added").into();
            return;
        }
        let fields = input.fields.clone();
        for field in &fields {
            region.fields.insert(field.id.clone(), field.initial());
        }
        edit.regions.push(region);
        edit.selected = Some(edit.regions.len() - 1);
    }

    pub(super) fn select_region_at(&mut self, point: xuan::document::Point) {
        if let Some(edit) = &mut self.plugins.action {
            edit.selected = edit
                .regions
                .iter()
                .rposition(|region| region.contains(point));
        }
    }

    // --- Panes -----------------------------------------------------------

    /// Ask a plugin for a pane's contents.
    pub(super) fn render_pane(
        &mut self,
        key: &str,
        reason: &str,
        event: Option<plugins::ui::Event>,
    ) {
        let Some((plugin, pane)) = split_pane_key(key).map(|(p, q)| (p.to_owned(), q.to_owned()))
        else {
            return;
        };
        let state = self.plugins.panes.entry(key.to_owned()).or_default();
        if state.pending {
            match event {
                Some(event) => state.queued = Some(event),
                None => state.dirty = true,
            }
            return;
        }
        if !self.plugin_granted(&plugin) || self.plugin_offline(&plugin) {
            return;
        }
        let document = self.session().map(|s| edits::describe(&s.document));
        let params = json!({
            "pane": pane,
            "reason": reason,
            "event": event,
            "document": document,
        });
        match self.plugin_process(&plugin) {
            Ok(process) => match process.request("pane/render", params) {
                Ok(id) => {
                    self.plugins
                        .pending
                        .insert((plugin, id), Pending::Render(key.to_owned()));
                    let state = self.plugins.panes.entry(key.to_owned()).or_default();
                    state.pending = true;
                    state.error = None;
                }
                Err(error) => {
                    self.plugins.panes.entry(key.to_owned()).or_default().error =
                        Some(error.to_string());
                }
            },
            Err(error) => {
                self.plugins.panes.entry(key.to_owned()).or_default().error =
                    Some(format!("{error:#}"));
            }
        }
    }

    /// Re-render document-driven panes after an edit and tell plugins.
    fn refresh_panes_for_changes(&mut self) {
        let Some((id, revision)) = self.session().map(|s| (s.document.id, s.history.revision))
        else {
            return;
        };
        if self.plugins.revisions.get(&id) == Some(&revision) {
            return;
        }
        self.plugins.revisions.insert(id, revision);
        for process in self.plugins.processes.values_mut() {
            let _ = process.notify("document/changed", json!({"id": id, "revision": revision}));
        }
        let keys: Vec<String> = self
            .plugins
            .manifests
            .iter()
            .flat_map(|manifest| {
                manifest
                    .panes
                    .iter()
                    .filter(|pane| pane.refresh == plugins::manifest::Refresh::Document)
                    .map(|pane| pane_key(&manifest.plugin.id, &pane.id))
            })
            .filter(|key| {
                self.config
                    .panes
                    .get(key)
                    .is_some_and(|p| !p.hidden && !p.collapsed)
                    && self
                        .plugins
                        .panes
                        .get(key)
                        .is_some_and(|s| s.tree.is_some())
            })
            .collect();
        for key in keys {
            self.render_pane(&key, "document", None);
        }
    }

    // --- Formats ---------------------------------------------------------

    /// The plugin and format that imports files with this extension.
    pub(super) fn plugin_import_format(&self, path: &Path) -> Option<(String, String)> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        self.plugins
            .manifests
            .iter()
            .filter(|m| self.plugin_available(&m.plugin.id))
            .find_map(|manifest| {
                manifest
                    .import_extensions()
                    .find(|(_, e)| *e == extension)
                    .map(|(format, _)| (manifest.plugin.id.clone(), format.id.clone()))
            })
    }

    /// Export formats plugins offer: (extension, label, plugin, format).
    pub(super) fn plugin_export_formats(&self) -> Vec<(String, String, String, String)> {
        self.plugins
            .manifests
            .iter()
            .filter(|m| self.plugin_available(&m.plugin.id))
            .flat_map(|manifest| {
                manifest
                    .formats
                    .iter()
                    .filter(|f| f.export)
                    .filter_map(|format| {
                        format.extensions.first().map(|extension| {
                            (
                                extension.to_ascii_lowercase(),
                                format.label.clone(),
                                manifest.plugin.id.clone(),
                                format.id.clone(),
                            )
                        })
                    })
            })
            .collect()
    }

    pub(super) fn plugin_import_extensions(&self) -> Vec<String> {
        self.plugins
            .manifests
            .iter()
            .filter(|m| self.plugin_available(&m.plugin.id))
            .flat_map(|m| m.import_extensions().map(|(_, e)| e))
            .collect()
    }

    /// Start loading a file through a plugin's `format/import`. The file
    /// opens when the plugin answers; the editor stays usable meanwhile.
    pub(super) fn start_plugin_import(
        &mut self,
        plugin: &str,
        format: &str,
        path: &Path,
        as_layer: bool,
    ) -> Result<()> {
        if !self.plugin_granted(plugin) {
            self.plugins.permission_request =
                Some((plugin.into(), PendingStart::Action(String::new())));
            self.dialog = Some(Dialog::PluginPermissions);
            bail!(
                "{}",
                tr("Accept the plugin's permissions, then open the file again")
            );
        }
        let work_dir = plugins::private_dir("xuan-import-")?;
        let params = json!({"format": format, "path": path, "work_dir": work_dir.path()});
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let label = format!("{} {name}", tr("Opening"));
        self.start_format_job(
            plugin,
            "format/import",
            params,
            label,
            FormatKind::Import {
                path: path.to_path_buf(),
                as_layer,
            },
            work_dir,
        )
    }

    /// Start saving the flattened document through a plugin's
    /// `format/export`. The status bar reports when it is written.
    pub(super) fn start_plugin_export(
        &mut self,
        plugin: &str,
        format: &str,
        path: &Path,
    ) -> Result<()> {
        let Some(document) = self.session().map(|s| s.document.clone()) else {
            bail!("No document is open");
        };
        if !self.plugin_granted(plugin) {
            bail!(
                "{}",
                tr("Accept the plugin's permissions in Plugins → Manage Plugins… first")
            );
        }
        let work_dir = plugins::private_dir("xuan-export-")?;
        let export = edits::export_composite(&document, None, work_dir.path(), "image.png")?;
        let params = json!({
            "format": format,
            "path": path,
            "image": export.path,
            "document": edits::describe(&document),
        });
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let label = format!("{} {name}", tr("Exporting"));
        self.start_format_job(
            plugin,
            "format/export",
            params,
            label,
            FormatKind::Export {
                path: path.to_path_buf(),
            },
            work_dir,
        )
    }

    fn start_format_job(
        &mut self,
        plugin: &str,
        method: &str,
        params: Value,
        label: String,
        kind: FormatKind,
        work_dir: tempfile::TempDir,
    ) -> Result<()> {
        let process = self.plugin_process(plugin)?;
        let request = process.request(method, params)?;
        let id = Uuid::new_v4();
        self.plugins
            .pending
            .insert((plugin.to_owned(), request), Pending::Format(id));
        self.status = format!("{label}…");
        self.plugins.formats.push(FormatJob {
            id,
            plugin: plugin.to_owned(),
            label,
            kind,
            started: std::time::Instant::now(),
            work_dir,
        });
        Ok(())
    }

    /// Stop waiting for a format job; a late answer is ignored.
    pub(super) fn cancel_format_job(&mut self, id: Uuid) {
        self.plugins.formats.retain(|job| job.id != id);
        self.plugins
            .pending
            .retain(|_, pending| !matches!(pending, Pending::Format(job) if *job == id));
        self.status = tr("Cancelled").into();
    }

    /// End the format jobs of a plugin that stopped; `reason` is shown for
    /// each, or nothing when the user stopped it.
    fn fail_format_jobs(&mut self, plugin: &str, reason: Option<&str>) -> bool {
        let failed: Vec<Uuid> = (self.plugins.formats.iter())
            .filter(|job| job.plugin == plugin)
            .map(|job| job.id)
            .collect();
        for id in &failed {
            match reason {
                Some(reason) => self.finish_format_job(*id, Err(reason.to_owned())),
                None => self.plugins.formats.retain(|job| job.id != *id),
            }
        }
        !failed.is_empty()
    }

    /// Fail format jobs that have waited too long.
    fn check_format_jobs(&mut self) {
        let late: Vec<Uuid> = (self.plugins.formats.iter())
            .filter(|job| job.started.elapsed() > FORMAT_TIMEOUT)
            .map(|job| job.id)
            .collect();
        for id in late {
            self.plugins
                .pending
                .retain(|_, pending| !matches!(pending, Pending::Format(job) if *job == id));
            self.finish_format_job(id, Err(tr("the plugin did not answer in time").into()));
        }
    }

    fn finish_format_job(&mut self, id: Uuid, result: Result<Value, String>) {
        let Some(index) = self.plugins.formats.iter().position(|job| job.id == id) else {
            return;
        };
        let job = self.plugins.formats.remove(index);
        match job.kind {
            FormatKind::Import { path, as_layer } => {
                let document = result.map_err(anyhow::Error::msg).and_then(|value| {
                    self.imported_document(&job.plugin, value, job.work_dir.path())
                });
                match document {
                    Ok(document) => self.open_imported(document, &path, as_layer),
                    Err(error) => {
                        self.error = Some(format!(
                            "{} {}\n\n{error:#}",
                            tr("Could not open"),
                            path.display()
                        ))
                    }
                }
            }
            FormatKind::Export { path } => match result {
                Ok(_) => self.status = format!("{} {}", tr("Exported"), path.display()),
                Err(error) => {
                    self.error = Some(format!(
                        "{} {}\n\n{error}",
                        tr("Could not export"),
                        path.display()
                    ))
                }
            },
        }
    }

    /// The document a plugin's `format/import` answer describes. Its images
    /// are read from the plugin's folders and the import's `work_dir`.
    fn imported_document(&self, plugin: &str, result: Value, work_dir: &Path) -> Result<Document> {
        #[derive(serde::Deserialize)]
        struct Imported {
            width: u32,
            height: u32,
            #[serde(default)]
            resolution: Option<f32>,
            #[serde(default)]
            layers: Vec<ImportedLayer>,
        }
        #[derive(serde::Deserialize)]
        struct ImportedLayer {
            name: Option<String>,
            image: PathBuf,
            #[serde(default)]
            x: Option<f32>,
            #[serde(default)]
            y: Option<f32>,
            #[serde(default)]
            mask: Option<PathBuf>,
            #[serde(default)]
            opacity: Option<f32>,
            #[serde(default)]
            blend: Option<xuan::blend::BlendMode>,
            #[serde(default)]
            visible: Option<bool>,
        }
        let imported: Imported =
            serde_json::from_value(result).context("Malformed import result")?;
        let mut document = Document::new(imported.width, imported.height)?;
        document.layers.clear();
        document.active = None;
        document.selected.clear();
        if let Some(resolution) = imported.resolution
            && resolution.is_finite()
            && (1.0..=9600.0).contains(&resolution)
        {
            document.resolution = resolution;
        }
        ensure!(!imported.layers.is_empty(), "The plugin returned no layers");
        ensure!(
            imported.layers.len() <= edits::MAX_IMPORT_LAYERS,
            "The plugin returned more than {} layers",
            edits::MAX_IMPORT_LAYERS
        );
        let access = self.plugins.access(plugin).with(work_dir);
        let mut reader = edits::Reader::new(access, edits::MAX_IMPORT_LAYERS);
        for layer in imported.layers {
            let edits = vec![edits::Edit::AddLayer {
                image: layer.image,
                name: layer.name,
                x: layer.x,
                y: layer.y,
                width: None,
                height: None,
                mask: layer.mask,
                above: None,
                opacity: layer.opacity,
                blend: layer.blend,
            }];
            let added = edits::apply(&mut document, &edits, &mut reader)?;
            if let (Some(false), Some(id)) = (layer.visible, added.first())
                && let Some(layer) = document.layers.iter_mut().find(|l| l.id == *id)
            {
                layer.visible = false;
            }
        }
        Ok(document)
    }
}

/// `text` on one line, without control characters, cut to `max` characters.
pub(super) fn one_line(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

/// The grant that allows this manifest to run as it is. A bundled plugin's
/// folder is stored by name (see [`plugins::grant_dir`]), so the grant
/// survives Xuan upgrades and the AppImage's changing mount point.
pub(super) fn grant_for(manifest: &Manifest) -> PluginGrant {
    PluginGrant {
        dir: plugins::grant_dir(&manifest.dir, plugins::bundled_dir().as_deref()),
        command: manifest.plugin.command.clone(),
        permissions: manifest.permissions.clone(),
        send_without_asking: false,
        edit_without_asking: false,
    }
}

/// The most regions an action's regions input takes.
fn region_limit(input: &plugins::manifest::Input) -> usize {
    input.max.map_or(plugins::manifest::MAX_REGIONS, |max| {
        (max.max(0.0) as usize).min(plugins::manifest::MAX_REGIONS)
    })
}

/// Regions stored on a generated layer: document coordinates plus fields.
fn regions_to_value(regions: &[Region]) -> Value {
    Value::Array(
        regions
            .iter()
            .map(|region| {
                json!({
                    "x": region.x,
                    "y": region.y,
                    "width": region.width,
                    "height": region.height,
                    "fields": region.fields,
                })
            })
            .collect(),
    )
}

fn regions_from_value(value: &Value) -> Vec<Region> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let number = |key: &str| item.get(key)?.as_f64().map(|v| v as f32);
                    let mut region = Region::rect(
                        number("x")?,
                        number("y")?,
                        number("width")?,
                        number("height")?,
                    );
                    if let Some(fields) = item.get("fields").and_then(Value::as_object) {
                        region.fields = fields.clone();
                    }
                    Some(region)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The provenance a plugin reported with an output, without secrets, counting
/// the entries removed.
fn take_provenance(
    reported: Option<Value>,
    redactor: &Redactor,
    stripped: &mut usize,
) -> Result<Option<Provenance>> {
    let Some(reported) = reported.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let (record, removed) = Provenance::from_plugin(&reported, redactor)?;
    *stripped += removed;
    Ok(record)
}

fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let days = seconds / 86_400;
    let rem = seconds % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A link a plugin may open: an absolute http(s) URL with a host and no
/// credentials, normalized. Links are opened by the platform's browser
/// launcher, never through a shell.
fn checked_url(text: &str) -> Result<String> {
    let url = url::Url::parse(text.trim()).context("Not a valid link")?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Only http(s) links can be opened"
    );
    ensure!(
        url.host_str().is_some_and(|host| !host.is_empty()),
        "The link has no host"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Links with credentials cannot be opened"
    );
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_may_only_open_web_links_with_a_host() {
        assert_eq!(
            checked_url(" https://example.com/a?b=1 ").unwrap(),
            "https://example.com/a?b=1"
        );
        assert_eq!(
            checked_url("http://Example.COM").unwrap(),
            "http://example.com/"
        );
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:a@example.com",
            "ftp://example.com/",
            "https://",
            "https://user:pw@example.com/",
            "https://cloud.comfy.org@evil.com/",
            "example.com",
            "",
        ] {
            assert!(checked_url(bad).is_err(), "{bad}");
        }
        // Shell metacharacters stay inside the URL: no shell ever sees it.
        let url = checked_url("https://x.example/&calc|a^b%PATH%").unwrap();
        assert!(url.starts_with("https://x.example/"), "{url}");
        let source = include_str!("plugins.rs");
        let shell = ["Command::new(\"cmd\")", "Command::new(\"xdg-open\")"];
        assert!(shell.iter().all(|s| !source.contains(s)));
    }

    #[test]
    fn the_protocol_docs_cover_every_method_the_host_speaks() {
        let source = include_str!("plugins.rs");
        let source = &source[..source.find("#[cfg(test)]\nmod tests").unwrap()];
        let source = format!(
            "{source}{}{}{}",
            include_str!("plugin_models.rs"),
            include_str!("plugin_files.rs"),
            include_str!("plugin_sessions.rs")
        );
        let source = source.as_str();
        let docs = include_str!("../../docs/PLUGINS.md");
        let mut methods = Vec::new();
        for (index, _) in source.match_indices('"') {
            let rest = &source[index + 1..];
            let Some(end) = rest.find('"') else { continue };
            let word = &rest[..end];
            let quoted_method = word.split_once('/').is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && word
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c == b'/' || c == b'_')
            });
            if quoted_method && !methods.contains(&word) {
                methods.push(word);
            }
        }
        assert!(methods.len() >= 15, "{methods:?}");
        for method in methods {
            assert!(
                docs.contains(&format!("`{method}`")),
                "{method} is not documented"
            );
        }
        // Who receives `document/changed` matches the code: every running plugin.
        assert!(source.contains("for process in self.plugins.processes.values_mut() {\n            let _ = process.notify(\"document/changed\""));
        assert!(
            docs.contains("`document/changed` `{id, revision}`, sent to\nevery running plugin")
        );
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let text = timestamp();
        assert_eq!(text.len(), 20, "{text}");
        assert!(text.ends_with('Z'));
        assert!(text.starts_with("20"));
    }

    #[test]
    fn regions_roundtrip_through_stored_inputs() {
        let mut region = Region::rect(1.0, 2.0, 3.0, 4.0);
        region
            .fields
            .insert("desc".into(), Value::String("hat".into()));
        let value = regions_to_value(&[region.clone()]);
        let back = regions_from_value(&value);
        assert_eq!(back, vec![region]);
        assert!(regions_from_value(&Value::Null).is_empty());
        assert_eq!(pane_key("a", "b"), "plugin:a/b");
        assert_eq!(split_pane_key("plugin:a/b"), Some(("a", "b")));
        assert_eq!(split_pane_key("layers"), None);
    }
}

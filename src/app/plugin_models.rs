//! Model files for plugins (`[[models]]`): the confirmation before Xuan
//! downloads them, the background downloads and verifications, the Models
//! section of Manage Plugins, and the verified paths handed to plugins.
//! The downloading itself is [`xuan::plugins::models`].
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};

use anyhow::{Context, Result, bail};
use egui::RichText;
use serde_json::{Map, Value, json};
use uuid::Uuid;
use xuan::{
    i18n::tr,
    plugins::{
        self,
        manifest::Model,
        models::{self, Cancelled, Progress, State, Transport},
    },
};

use super::{Dialog, EditorApp, plugins::one_line, theme, widgets};

/// A download or verification running in the background.
pub(super) struct ModelJob {
    pub id: Uuid,
    pub plugin: String,
    pub model: String,
    /// Hashing a file already on disk rather than downloading it.
    pub verify: bool,
    pub size: u64,
    pub progress: Arc<Progress>,
    /// The worker's answer: whether the file is now verified and matches.
    receive: Receiver<Result<bool, String>>,
}

impl ModelJob {
    pub fn fraction(&self) -> f32 {
        (self.progress.done() as f64 / self.size.max(1) as f64).clamp(0.0, 1.0) as f32
    }

    pub fn label(&self) -> String {
        format!(
            "{} {}",
            if self.verify {
                tr("Verifying")
            } else {
                tr("Downloading")
            },
            self.model
        )
    }
}

/// The confirmation before downloading models.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelRequest {
    pub plugin: String,
    pub models: Vec<String>,
    /// The action to run once every model it needs is ready, with its inputs.
    pub then: Option<(String, Option<Value>)>,
}

/// What a model looks like in Manage Plugins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ModelStatus {
    Missing,
    Downloading(f32),
    Verifying(f32),
    /// The right size, but changed since it was hashed.
    Unverified,
    Ready,
    Corrupt,
}

impl ModelStatus {
    pub fn text(self) -> String {
        match self {
            Self::Missing => tr("Not downloaded").into(),
            Self::Downloading(fraction) => {
                format!("{} {:.0}%", tr("Downloading"), fraction * 100.0)
            }
            Self::Verifying(fraction) => format!("{} {:.0}%", tr("Verifying"), fraction * 100.0),
            Self::Unverified => tr("Not verified").into(),
            Self::Ready => tr("Ready").into(),
            Self::Corrupt => tr("Corrupt").into(),
        }
    }
}

/// The colour of a corrupt model's status.
const CORRUPT: egui::Color32 = egui::Color32::from_rgb(255, 140, 140);

/// `bytes` as KB, MB or GB (powers of 1000, as file managers show them).
pub(super) fn format_size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes < 1000.0 {
        format!("{bytes} B")
    } else if bytes < 1e6 {
        format!("{:.1} KB", bytes / 1e3)
    } else if bytes < 1e9 {
        format!("{:.1} MB", bytes / 1e6)
    } else {
        format!("{:.2} GB", bytes / 1e9)
    }
}

impl super::plugins::PluginState {
    /// The plugin's models folder, without creating it. `None` when it has
    /// no data folder yet.
    pub(super) fn models_dir_path(&self, plugin: &str) -> Option<PathBuf> {
        let data = match &self.config_dir {
            Some(config_dir) => plugins::data_dir(config_dir, plugin),
            None => self.data_fallback_path(plugin)?,
        };
        Some(models::models_dir(&data))
    }

    /// The plugin's models folder, created private (0700) if needed.
    pub(super) fn models_dir(&mut self, plugin: &str) -> Result<PathBuf> {
        let dir = models::models_dir(&self.data_dir(plugin)?);
        models::create_private_dir(&dir)
            .with_context(|| format!("Cannot create {}", dir.display()))?;
        Ok(dir)
    }

    fn transport(&mut self) -> Arc<dyn Transport> {
        self.transport
            .get_or_insert_with(|| Arc::new(models::Https::new()))
            .clone()
    }

    fn model_job(&self, plugin: &str, model: &str) -> Option<&ModelJob> {
        (self.model_jobs.iter()).find(|job| job.plugin == plugin && job.model == model)
    }
}

impl EditorApp {
    fn model_spec(&self, plugin: &str, model: &str) -> Option<Model> {
        let manifest = self.plugins.manifest(plugin)?;
        manifest.models.iter().find(|m| m.id == model).cloned()
    }

    /// A model's status and its size on disk.
    pub(super) fn model_status(&self, plugin: &str, model: &Model) -> (ModelStatus, Option<u64>) {
        let on_disk = (self.plugins.models_dir_path(plugin))
            .map_or((State::Missing, None), |dir| models::state(model, &dir));
        if let Some(job) = self.plugins.model_job(plugin, &model.id) {
            let status = if job.verify {
                ModelStatus::Verifying(job.fraction())
            } else {
                ModelStatus::Downloading(job.fraction())
            };
            return (status, on_disk.1);
        }
        let status = match on_disk.0 {
            State::Missing => ModelStatus::Missing,
            State::Unverified => ModelStatus::Unverified,
            State::Ready => ModelStatus::Ready,
            State::Corrupt => ModelStatus::Corrupt,
        };
        (status, on_disk.1)
    }

    /// The verified models, by id, as `initialize` and `models/changed`
    /// hand them to the plugin.
    pub(super) fn model_paths(&self, plugin: &str) -> Value {
        let (Some(manifest), Some(dir)) = (
            self.plugins.manifest(plugin),
            self.plugins.models_dir_path(plugin),
        ) else {
            return Value::Object(Map::new());
        };
        let paths: Map<String, Value> = models::ready_paths(&manifest.models, &dir)
            .into_iter()
            .map(|(id, path)| (id, json!(path)))
            .collect();
        Value::Object(paths)
    }

    /// Whether the models `action` needs are ready. When they are not, files
    /// that only need hashing are verified in the background, missing or
    /// corrupt ones are offered for download, and the action runs once they
    /// are all ready.
    pub(super) fn action_models_ready(
        &mut self,
        plugin: &str,
        action: &str,
        inputs: Option<&Value>,
    ) -> bool {
        let Some(spec) = (self.plugins.manifest(plugin)).and_then(|m| m.action(action)) else {
            return true;
        };
        let mut download = Vec::new();
        let mut waiting = false;
        for id in spec.models.clone() {
            let Some(model) = self.model_spec(plugin, &id) else {
                continue;
            };
            match self.model_status(plugin, &model).0 {
                ModelStatus::Ready => {}
                ModelStatus::Downloading(_) | ModelStatus::Verifying(_) => waiting = true,
                ModelStatus::Unverified => {
                    waiting = true;
                    if let Err(error) = self.start_model_job(plugin, &id, true) {
                        self.error = Some(format!("{error:#}"));
                        return false;
                    }
                }
                ModelStatus::Missing | ModelStatus::Corrupt => download.push(id),
            }
        }
        let then = Some((action.to_owned(), inputs.cloned()));
        if !download.is_empty() {
            self.plugins.model_request = Some(ModelRequest {
                plugin: plugin.into(),
                models: download,
                then,
            });
            self.dialog = Some(Dialog::PluginModels);
            return false;
        }
        if waiting {
            self.plugins.awaiting_models = Some((plugin.into(), then.unwrap()));
            self.status = tr("Waiting for the plugin's models…").into();
            return false;
        }
        true
    }

    /// Ask before downloading these models of a plugin.
    pub(super) fn request_model_download(&mut self, plugin: &str, models: Vec<String>) {
        self.plugins.model_request = Some(ModelRequest {
            plugin: plugin.into(),
            models,
            then: None,
        });
        self.dialog = Some(Dialog::PluginModels);
    }

    /// Download (or with `verify`, hash again) one model in the background.
    pub(super) fn start_model_job(
        &mut self,
        plugin: &str,
        model: &str,
        verify: bool,
    ) -> Result<()> {
        if self.plugins.model_job(plugin, model).is_some() {
            return Ok(());
        }
        let spec = self
            .model_spec(plugin, model)
            .with_context(|| format!("{} has no model {model}", self.plugins.source(plugin)))?;
        if !verify && self.config.disable_network_plugins {
            bail!(
                "{}",
                tr("Models are not downloaded while plugins that use the network are disabled")
            );
        }
        let dir = self.plugins.models_dir(plugin)?;
        let transport = self.plugins.transport();
        let progress = Arc::new(Progress::default());
        let (send, receive) = mpsc::channel();
        let context = self.context.clone();
        let worker = progress.clone();
        let size = spec.size;
        std::thread::Builder::new()
            .name(format!("model {plugin}/{model}"))
            .spawn(move || {
                let result = if verify {
                    models::verify(&spec, &dir, &worker)
                } else {
                    models::download(transport.as_ref(), &spec, &dir, &worker).map(|_| true)
                };
                let result = result.map_err(|error| {
                    if error.is::<Cancelled>() {
                        String::new()
                    } else {
                        format!("{error:#}")
                    }
                });
                let _ = send.send(result);
                context.request_repaint();
            })
            .context("Cannot start the download")?;
        self.plugins.model_jobs.push(ModelJob {
            id: Uuid::new_v4(),
            plugin: plugin.into(),
            model: model.into(),
            verify,
            size,
            progress,
            receive,
        });
        Ok(())
    }

    /// Stop a download or verification. The worker removes its partial
    /// file; the job ends when it has.
    pub(super) fn cancel_model_job(&mut self, id: Uuid) {
        if let Some(job) = self.plugins.model_jobs.iter().find(|job| job.id == id) {
            job.progress.cancel();
            if self
                .plugins
                .awaiting_models
                .as_ref()
                .is_some_and(|(plugin, _)| plugin == &job.plugin)
            {
                self.plugins.awaiting_models = None;
            }
        }
    }

    /// Finish the downloads and verifications whose workers answered.
    /// Called once per frame.
    pub(super) fn poll_model_jobs(&mut self) {
        let mut finished = Vec::new();
        self.plugins.model_jobs.retain(|job| {
            let result = match job.receive.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => Err(tr("The download stopped").into()),
            };
            finished.push((job.plugin.clone(), job.model.clone(), job.verify, result));
            false
        });
        if finished.is_empty() {
            return;
        }
        let mut changed = Vec::new();
        for (plugin, model, verify, result) in finished {
            let source = self.plugins.source(&plugin);
            match result {
                Ok(true) => {
                    self.status = format!(
                        "{source}: {model} {}",
                        if verify {
                            tr("verified")
                        } else {
                            tr("downloaded and verified")
                        }
                    );
                }
                Ok(false) => {
                    self.error = Some(format!(
                        "{source}: {model} {}",
                        tr(
                            "does not match the size or SHA-256 its plugin declares. Download it again from Plugins → Manage Plugins… → Models."
                        )
                    ));
                    self.drop_awaiting(&plugin);
                }
                Err(error) if error.is_empty() => {
                    self.status = tr("Cancelled").into();
                    self.drop_awaiting(&plugin);
                }
                Err(error) => {
                    self.error = Some(format!(
                        "{} {model} ({source})\n\n{error}",
                        tr("Could not download")
                    ));
                    self.drop_awaiting(&plugin);
                }
            }
            if !changed.contains(&plugin) {
                changed.push(plugin);
            }
        }
        for plugin in changed {
            self.notify_models(&plugin);
        }
        self.resume_awaiting_action();
    }

    fn drop_awaiting(&mut self, plugin: &str) {
        if (self.plugins.awaiting_models.as_ref()).is_some_and(|(id, _)| id == plugin) {
            self.plugins.awaiting_models = None;
        }
    }

    /// Run the action that waited for models once none of its plugin's
    /// downloads or verifications are left.
    fn resume_awaiting_action(&mut self) {
        let Some((plugin, _)) = &self.plugins.awaiting_models else {
            return;
        };
        if self
            .plugins
            .model_jobs
            .iter()
            .any(|job| &job.plugin == plugin)
        {
            return;
        }
        let (plugin, (action, inputs)) = self.plugins.awaiting_models.take().unwrap();
        self.start_plugin_action_with(&plugin, &action, inputs.as_ref());
    }

    /// Tell a running plugin which models are ready now.
    pub(super) fn notify_models(&mut self, plugin: &str) {
        let models = self.model_paths(plugin);
        if let Some(process) = self.plugins.process_mut(plugin) {
            let _ = process.notify("models/changed", json!({ "models": models }));
        }
    }

    /// Delete one model's file.
    pub(super) fn delete_model(&mut self, plugin: &str, model: &str) {
        let (Some(spec), Some(dir)) = (
            self.model_spec(plugin, model),
            self.plugins.models_dir_path(plugin),
        ) else {
            return;
        };
        if self.plugins.model_job(plugin, model).is_some() {
            return;
        }
        match models::delete(&spec, &dir) {
            Ok(()) => self.status = format!("{} {model}", tr("Deleted model")),
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        self.notify_models(plugin);
    }

    /// Delete the plugin's whole models folder.
    pub(super) fn delete_all_models(&mut self, plugin: &str) {
        let Some(dir) = self.plugins.models_dir_path(plugin) else {
            return;
        };
        if self
            .plugins
            .model_jobs
            .iter()
            .any(|job| job.plugin == plugin)
        {
            return;
        }
        match models::delete_all(&dir) {
            Ok(()) => {
                self.status = format!(
                    "{} {}",
                    tr("Deleted the models of"),
                    self.plugins.source(plugin)
                )
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        self.notify_models(plugin);
    }

    /// Answer the confirmation: start the downloads, or drop the request.
    pub(super) fn answer_model_request(&mut self, download: bool) {
        let Some(request) = self.plugins.model_request.take() else {
            return;
        };
        if self.dialog == Some(Dialog::PluginModels) {
            self.dialog = request.then.is_none().then_some(Dialog::Plugins);
        }
        if !download {
            self.status = tr("Cancelled; nothing was downloaded").into();
            return;
        }
        for model in &request.models {
            if let Err(error) = self.start_model_job(&request.plugin, model, false) {
                self.error = Some(format!("{error:#}"));
                return;
            }
        }
        if let Some(then) = request.then {
            self.plugins.awaiting_models = Some((request.plugin, then));
        }
    }

    pub(super) fn plugin_models_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.plugins.model_request.clone() else {
            self.dialog = None;
            return;
        };
        let Some(manifest) = self.plugins.manifest(&request.plugin).cloned() else {
            self.plugins.model_request = None;
            self.dialog = None;
            return;
        };
        let source = self.plugins.source(&request.plugin);
        let offline = self.config.disable_network_plugins;
        let specs: Vec<&Model> = (request.models.iter())
            .filter_map(|id| manifest.models.iter().find(|m| &m.id == id))
            .collect();
        let total: u64 = specs.iter().map(|m| m.size).sum();
        let mut open = true;
        let mut answer = None;
        widgets::Window::new(tr("Download Models?"))
            .id(("plugin_models", &request.plugin))
            .default_width(460.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let lead = match (request.then.as_ref())
                    .and_then(|(action, _)| manifest.action(action))
                {
                    Some(action) => format!(
                        "“{}” {} {source}:",
                        one_line(action.label.trim_end_matches('…'), 80),
                        tr("needs these models from")
                    ),
                    None => format!("{} {source}:", tr("Xuan downloads these models for")),
                };
                ui.add(egui::Label::new(RichText::new(lead).strong()).wrap());
                ui.add_space(4.0);
                for model in &specs {
                    ui.add(
                        egui::Label::new(format!(
                            "• {} — {} {} {}",
                            model.id,
                            format_size(model.size),
                            tr("from"),
                            one_line(&model.host(), 120)
                        ))
                        .wrap(),
                    );
                    let mut details = Vec::new();
                    if !model.license.is_empty() {
                        details.push(format!("{} {}", tr("License:"), one_line(&model.license, 200)));
                    }
                    if !model.source.is_empty() {
                        details.push(format!("{} {}", tr("Source:"), one_line(&model.source, 200)));
                    }
                    if details.is_empty() {
                        details.push(tr("The plugin gives no license or source.").into());
                    }
                    for detail in details {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("  {detail}")).small().color(theme::MUTED),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.add_space(8.0);
                ui.label(format!("{} {}", tr("Total:"), format_size(total)));
                ui.add(
                    egui::Label::new(
                        RichText::new(tr(
                            "Xuan downloads them over https into the plugin's models folder and checks each against the size and SHA-256 the plugin declares. It never runs or unpacks them; the plugin loads them.",
                        ))
                        .small()
                        .color(theme::MUTED),
                    )
                    .wrap(),
                );
                if offline {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(tr(
                                "Models are not downloaded while plugins that use the network are disabled",
                            ))
                            .color(theme::MUTED),
                        )
                        .wrap(),
                    );
                }
                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!offline, widgets::Button::new(tr("Download")).primary())
                            .clicked()
                        {
                            answer = Some(true);
                        }
                        if widgets::button(ui, tr("Cancel")).clicked() {
                            answer = Some(false);
                        }
                    });
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
            answer = Some(false);
        }
        if let Some(download) = answer {
            self.answer_model_request(download);
        }
    }

    /// The Models section of Manage Plugins for one plugin.
    pub(super) fn plugin_models_section(
        &mut self,
        ui: &mut egui::Ui,
        manifest: &plugins::Manifest,
    ) {
        if manifest.models.is_empty() {
            return;
        }
        let id = &manifest.plugin.id;
        let mut download = None;
        let mut delete = None;
        let mut delete_all = false;
        let mut cancel = None;
        let mut verify = None;
        ui.label(RichText::new(tr("Models")).strong());
        let busy = self.plugins.model_jobs.iter().any(|job| &job.plugin == id);
        egui::Grid::new(("plugin_models", id))
            .num_columns(4)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for model in &manifest.models {
                    let (status, on_disk) = self.model_status(id, model);
                    ui.label(&model.id).on_hover_text(format!(
                        "{}\n{} {}",
                        model.url,
                        tr("Size:"),
                        format_size(model.size)
                    ));
                    ui.label(RichText::new(status.text()).color(match status {
                        ModelStatus::Corrupt => CORRUPT,
                        ModelStatus::Ready => ui.visuals().text_color(),
                        _ => theme::MUTED,
                    }));
                    ui.label(
                        RichText::new(on_disk.map_or_else(|| "—".to_owned(), format_size))
                            .color(theme::MUTED),
                    );
                    ui.horizontal(|ui| match status {
                        ModelStatus::Downloading(_) | ModelStatus::Verifying(_) => {
                            if let Some(job) = self.plugins.model_job(id, &model.id)
                                && widgets::button(ui, tr("Cancel")).clicked()
                            {
                                cancel = Some(job.id);
                            }
                        }
                        _ => {
                            if status != ModelStatus::Ready
                                && widgets::button(ui, tr("Download"))
                                    .on_hover_text(format!(
                                        "{} {}",
                                        format_size(model.size),
                                        model.host()
                                    ))
                                    .clicked()
                            {
                                download = Some(model.id.clone());
                            }
                            if status == ModelStatus::Unverified
                                && widgets::button(ui, tr("Verify")).clicked()
                            {
                                verify = Some(model.id.clone());
                            }
                            if on_disk.is_some() && widgets::button(ui, tr("Delete")).clicked() {
                                delete = Some(model.id.clone());
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, widgets::Button::new(tr("Delete All Models")))
                .clicked()
            {
                delete_all = true;
            }
            if let Some(dir) = self.plugins.models_dir_path(id) {
                ui.label(
                    RichText::new(dir.display().to_string())
                        .small()
                        .color(theme::MUTED),
                );
            }
        });
        ui.add_space(8.0);
        if let Some(model) = download {
            self.request_model_download(id, vec![model]);
        }
        if let Some(model) = verify
            && let Err(error) = self.start_model_job(id, &model, true)
        {
            self.error = Some(format!("{error:#}"));
        }
        if let Some(model) = delete {
            self.delete_model(id, &model);
        }
        if delete_all {
            self.delete_all_models(id);
        }
        if let Some(job) = cancel {
            self.cancel_model_job(job);
        }
    }
}

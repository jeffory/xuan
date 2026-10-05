//! Windows for plugins: the action dialog built from a manifest's inputs,
//! permission prompts, the plugin manager, running jobs and proposals.
use egui::RichText;
use serde_json::Value;
use xuan::{
    i18n::tr,
    plugins::{
        manifest::{Input, InputKind, Manifest, ResultInto},
        ui::Node,
    },
};

use super::{EditorApp, plugins::PendingStart, theme, widgets};

/// Draw one input and update its JSON value. Returns whether it changed.
pub(super) fn input_widget(
    ui: &mut egui::Ui,
    input: &Input,
    value: &mut Value,
    salt: impl std::hash::Hash,
) -> bool {
    let id = egui::Id::new(("plugin_input", &input.id, salt));
    let mut changed = false;
    let label = |ui: &mut egui::Ui| {
        let response = ui.label(input.label());
        if !input.help.is_empty() {
            response.on_hover_text(&input.help);
        }
    };
    match input.kind {
        InputKind::Text | InputKind::Path | InputKind::Secret => {
            ui.horizontal(|ui| {
                label(ui);
                let mut text = value.as_str().unwrap_or_default().to_owned();
                let mut edit = egui::TextEdit::singleline(&mut text)
                    .id(id)
                    .hint_text(&input.placeholder)
                    .desired_width(if input.kind == InputKind::Path {
                        180.0
                    } else {
                        220.0
                    });
                if input.kind == InputKind::Secret {
                    edit = edit.password(true);
                }
                if ui.add(edit).changed() {
                    *value = Value::String(text);
                    changed = true;
                }
                if input.kind == InputKind::Path
                    && widgets::button(ui, tr("Browse…")).clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    *value = Value::String(path.display().to_string());
                    changed = true;
                }
            });
        }
        InputKind::Multiline => {
            label(ui);
            let mut text = value.as_str().unwrap_or_default().to_owned();
            if ui
                .add(
                    egui::TextEdit::multiline(&mut text)
                        .id(id)
                        .hint_text(&input.placeholder)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                *value = Value::String(text);
                changed = true;
            }
        }
        InputKind::Integer | InputKind::Seed | InputKind::Number => {
            ui.horizontal(|ui| {
                label(ui);
                let integer = input.kind != InputKind::Number;
                let mut number = value.as_f64().unwrap_or(0.0);
                let min = input
                    .min
                    .unwrap_or(if integer { i64::MIN as f64 } else { f64::MIN });
                let max = input
                    .max
                    .unwrap_or(if integer { i64::MAX as f64 } else { f64::MAX });
                let response = if let (Some(min), Some(max)) = (input.min, input.max) {
                    ui.spacing_mut().slider_width = 140.0;
                    let mut slider = widgets::Slider::new(&mut number, min..=max);
                    if integer {
                        slider = slider.max_decimals(0);
                    }
                    ui.add(slider)
                } else {
                    let mut field = widgets::Number::new(&mut number)
                        .size(egui::vec2(110.0, 22.0))
                        .speed(input.step.unwrap_or(if integer { 1.0 } else { 0.1 }))
                        .range(min..=max);
                    if integer {
                        field = field.max_decimals(0);
                    }
                    ui.add(field)
                };
                if response.changed() {
                    *value = if integer {
                        Value::from(number.round() as i64)
                    } else {
                        Value::from(number)
                    };
                    changed = true;
                }
                if input.kind == InputKind::Seed && widgets::button(ui, tr("Random")).clicked() {
                    let seed = (uuid::Uuid::new_v4().as_u128() % u128::from(u32::MAX)) as i64;
                    *value = Value::from(seed);
                    changed = true;
                }
            });
        }
        InputKind::Bool => {
            let mut checked = value.as_bool().unwrap_or(false);
            if widgets::checkbox(ui, &mut checked, input.label()).changed() {
                *value = Value::Bool(checked);
                changed = true;
            }
        }
        InputKind::Enum => {
            ui.horizontal(|ui| {
                label(ui);
                let current = value.as_str().unwrap_or_default().to_owned();
                let selected = input
                    .values
                    .iter()
                    .find(|c| c.id == current)
                    .map_or(current.clone(), |c| c.label.clone());
                let mut choice = current.clone();
                widgets::PopUp::from_id_salt(id)
                    .selected_text(selected)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for option in &input.values {
                            widgets::menu_choice(ui, &mut choice, option.id.clone(), &option.label);
                        }
                    });
                if choice != current {
                    *value = Value::String(choice);
                    changed = true;
                }
            });
        }
        InputKind::Color => {
            ui.horizontal(|ui| {
                label(ui);
                let mut color = value
                    .as_str()
                    .and_then(Node::color)
                    .unwrap_or([255, 255, 255, 255]);
                if widgets::color_well(ui, &mut color).changed() {
                    *value = Value::String(Node::color_text(color));
                    changed = true;
                }
            });
        }
        InputKind::Regions => {}
    }
    changed
}

impl EditorApp {
    pub(super) fn plugin_action_dialog(&mut self, ctx: &egui::Context) {
        let Some((plugin, action)) = self
            .plugins
            .action
            .as_ref()
            .map(|edit| (edit.plugin.clone(), edit.action.clone()))
        else {
            return;
        };
        let Some(spec) = self
            .plugins
            .manifest(&plugin)
            .and_then(|m| m.action(&action))
            .cloned()
        else {
            self.close_plugin_action();
            return;
        };
        let mut open = true;
        let mut run = false;
        let mut cancel = false;
        let mut use_selection = false;
        let self_has_document = self.session().is_some();
        widgets::Window::new(spec.label.trim_end_matches('…'))
            .id(("plugin_action", &plugin, &action))
            .default_width(380.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let Some(edit) = &mut self.plugins.action else {
                    return;
                };
                if !spec.description.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&spec.description).color(theme::MUTED)).wrap());
                    ui.add_space(8.0);
                }
                for input in &spec.inputs {
                    if input.kind == InputKind::Regions {
                        ui.add_space(4.0);
                        ui.label(RichText::new(input.label()).strong());
                        ui.add(
                            egui::Label::new(
                                RichText::new(tr(
                                    "Drag on the canvas to add a box, or add the current selection.",
                                ))
                                .small()
                                .color(theme::MUTED),
                            )
                            .wrap(),
                        );
                        let mut remove = None;
                        for index in 0..edit.regions.len() {
                            let selected = edit.selected == Some(index);
                            let summary = edit.regions[index]
                                .fields
                                .values()
                                .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
                                .unwrap_or("")
                                .to_owned();
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(selected, format!("{:02}  {summary}", index + 1))
                                    .clicked()
                                {
                                    edit.selected = Some(index);
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("✕").on_hover_text(tr("Remove region")).clicked() {
                                        remove = Some(index);
                                    }
                                });
                            });
                            if selected {
                                ui.indent(("region_fields", index), |ui| {
                                    for field in &input.fields {
                                        let value = edit.regions[index]
                                            .fields
                                            .entry(field.id.clone())
                                            .or_insert_with(|| field.initial());
                                        input_widget(ui, field, value, index);
                                    }
                                });
                            }
                        }
                        if let Some(index) = remove {
                            edit.regions.remove(index);
                            edit.selected = edit.selected.and_then(|s| match s.cmp(&index) {
                                std::cmp::Ordering::Less => Some(s),
                                std::cmp::Ordering::Equal => None,
                                std::cmp::Ordering::Greater => Some(s - 1),
                            });
                        }
                        ui.horizontal(|ui| {
                            if widgets::button(ui, tr("Add Selection")).clicked() {
                                use_selection = true;
                            }
                            if !edit.regions.is_empty() && widgets::button(ui, tr("Clear All")).clicked() {
                                edit.regions.clear();
                                edit.selected = None;
                            }
                        });
                        ui.add_space(6.0);
                        continue;
                    }
                    let value = edit
                        .values
                        .entry(input.id.clone())
                        .or_insert_with(|| input.initial());
                    input_widget(ui, input, value, (&plugin, &action));
                    ui.add_space(4.0);
                }
                if spec.result.into == ResultInto::Ask {
                    ui.horizontal(|ui| {
                        ui.label(tr("Result"));
                        let has_document = self_has_document;
                        ui.add_enabled_ui(has_document, |ui| {
                            if ui
                                .selectable_label(edit.into == ResultInto::Layer, tr("New layer"))
                                .clicked()
                            {
                                edit.into = ResultInto::Layer;
                            }
                        });
                        if ui
                            .selectable_label(
                                edit.into == ResultInto::Document,
                                tr("New document"),
                            )
                            .clicked()
                        {
                            edit.into = ResultInto::Document;
                        }
                    });
                    ui.add_space(4.0);
                }
                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(estimate) = &edit.estimate {
                        ui.label(RichText::new(estimate).color(theme::MUTED));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        run = widgets::primary_button(ui, tr("Run")).clicked();
                        cancel = widgets::button(ui, tr("Cancel")).clicked();
                    });
                });
            });
        if use_selection {
            self.add_selection_region();
        }
        if run {
            self.run_plugin_action();
        } else if !open || cancel {
            self.close_plugin_action();
        }
    }

    pub(super) fn plugin_permissions_dialog(&mut self, ctx: &egui::Context) {
        let Some((plugin, next)) = self.plugins.permission_request.clone() else {
            self.dialog = None;
            return;
        };
        let Some(manifest) = self.plugins.manifest(&plugin).cloned() else {
            self.plugins.permission_request = None;
            self.dialog = None;
            return;
        };
        let mut open = true;
        let mut decision = None;
        widgets::Window::new(format!("{} {}?", tr("Allow"), manifest.plugin.name))
            .id(("plugin_permissions", &plugin))
            .default_width(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add(
                    egui::Label::new(format!(
                        "{} {} {}",
                        manifest.plugin.name,
                        manifest.plugin.version,
                        tr("asks for the following. Xuan cannot enforce these; only run plugins you trust."),
                    ))
                    .wrap(),
                );
                ui.add_space(8.0);
                permissions_list(ui, &manifest);
                ui.add_space(12.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary_button(ui, tr("Allow")).clicked() {
                            decision = Some(true);
                        }
                        if widgets::button(ui, tr("Deny")).clicked() {
                            decision = Some(false);
                        }
                    });
                });
            });
        match decision {
            Some(true) => {
                self.grant_plugin(&plugin, true);
                self.plugins.permission_request = None;
                self.dialog = None;
                match next {
                    PendingStart::Action(action) if !action.is_empty() => {
                        self.start_plugin_action(&plugin, &action)
                    }
                    PendingStart::Pane(key) => self.render_pane(&key, "open", None),
                    PendingStart::Action(_) => {}
                }
            }
            Some(false) => {
                self.plugins.permission_request = None;
                self.dialog = None;
            }
            None if !open => {
                self.plugins.permission_request = None;
                self.dialog = None;
            }
            None => {}
        }
    }

    pub(super) fn plugin_manager_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut done = false;
        let mut reload = false;
        let mut selected = self
            .plugins
            .manager_selected
            .clone()
            .or_else(|| self.plugins.manifests.first().map(|m| m.plugin.id.clone()));
        let manifests: Vec<Manifest> = self.plugins.manifests.clone();
        let errors = self.plugins.errors.clone();
        let mut grant: Option<(String, bool)> = None;
        let mut enable: Option<(String, bool)> = None;
        let mut setting_changed: Option<(String, String, Value)> = None;
        let plugin_dir = self
            .config_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|dir| dir.join("plugins"));
        widgets::Window::new(tr("Plugins"))
            .id("plugin_manager")
            .default_width(680.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(180.0);
                        ui.set_min_height(320.0);
                        egui::ScrollArea::vertical()
                            .id_salt("plugin_list")
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for manifest in &manifests {
                                    let enabled = self
                                        .config
                                        .plugins
                                        .get(&manifest.plugin.id)
                                        .is_none_or(|c| c.enabled);
                                    let text = RichText::new(&manifest.plugin.name);
                                    let text = if enabled { text } else { text.color(theme::MUTED) };
                                    if ui
                                        .selectable_label(selected.as_deref() == Some(&manifest.plugin.id), text)
                                        .clicked()
                                    {
                                        selected = Some(manifest.plugin.id.clone());
                                    }
                                }
                                if manifests.is_empty() {
                                    ui.label(RichText::new(tr("No plugins installed")).color(theme::MUTED));
                                }
                            });
                        ui.add_space(8.0);
                        if widgets::button(ui, tr("Reload")).clicked() {
                            reload = true;
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(440.0);
                        let Some(manifest) = selected
                            .as_ref()
                            .and_then(|id| manifests.iter().find(|m| &m.plugin.id == id))
                        else {
                            ui.heading(tr("Plugins"));
                            ui.add_space(8.0);
                            ui.add(
                                egui::Label::new(tr(
                                    "Install a plugin by placing its folder, with a plugin.toml manifest, in the plugins directory, then press Reload.",
                                ))
                                .wrap(),
                            );
                            if let Some(dir) = &plugin_dir {
                                ui.add_space(8.0);
                                ui.label(RichText::new(dir.display().to_string()).color(theme::MUTED));
                            }
                            return;
                        };
                        let id = manifest.plugin.id.clone();
                        let config = self.config.plugins.get(&id).cloned().unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.heading(&manifest.plugin.name);
                            ui.label(RichText::new(&manifest.plugin.version).color(theme::MUTED));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let mut enabled = config.enabled;
                                if widgets::checkbox(ui, &mut enabled, tr("Enabled")).changed() {
                                    enable = Some((id.clone(), enabled));
                                }
                            });
                        });
                        if !manifest.plugin.description.is_empty() {
                            ui.add(egui::Label::new(&manifest.plugin.description).wrap());
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new(manifest.dir.display().to_string()).small().color(theme::MUTED));
                        let status = if self.plugins.running(&id) {
                            tr("Running")
                        } else {
                            tr("Not running")
                        };
                        ui.label(RichText::new(status).small().color(theme::MUTED));
                        ui.add_space(8.0);
                        if !manifest.permissions.is_empty() {
                            ui.label(RichText::new(tr("Permissions")).strong());
                            permissions_list(ui, manifest);
                            ui.horizontal(|ui| {
                                if config.granted {
                                    ui.label(RichText::new(tr("Allowed")).color(theme::MUTED));
                                    if widgets::button(ui, tr("Revoke")).clicked() {
                                        grant = Some((id.clone(), false));
                                    }
                                } else if widgets::primary_button(ui, tr("Allow")).clicked() {
                                    grant = Some((id.clone(), true));
                                }
                            });
                            ui.add_space(8.0);
                        }
                        if !manifest.settings.is_empty() {
                            ui.label(RichText::new(tr("Settings")).strong());
                            for setting in &manifest.settings {
                                let mut value = if setting.kind == InputKind::Secret {
                                    Value::String(
                                        self.plugins
                                            .secrets
                                            .get(&id, &setting.id)
                                            .unwrap_or_default()
                                            .to_owned(),
                                    )
                                } else {
                                    config
                                        .settings
                                        .get(&setting.id)
                                        .and_then(|v| serde_json::to_value(v).ok())
                                        .unwrap_or_else(|| setting.initial())
                                };
                                if input_widget(ui, setting, &mut value, ("setting", &id)) {
                                    setting_changed = Some((id.clone(), setting.id.clone(), value));
                                }
                            }
                            ui.add_space(8.0);
                        }
                        let summary = format!(
                            "{} {} · {} {} · {} {}",
                            manifest.actions.len(),
                            tr("actions"),
                            manifest.panes.len(),
                            tr("panes"),
                            manifest.formats.len(),
                            tr("formats")
                        );
                        ui.label(RichText::new(summary).small().color(theme::MUTED));
                        let log = self.plugins.log(&id);
                        if !log.is_empty() {
                            egui::CollapsingHeader::new(tr("Log"))
                                .id_salt(("plugin_log", &id))
                                .show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt(("plugin_log_scroll", &id))
                                        .max_height(140.0)
                                        .stick_to_bottom(true)
                                        .show(ui, |ui| {
                                            for line in log.iter().rev().take(200).rev() {
                                                ui.label(RichText::new(line).monospace().small());
                                            }
                                        });
                                });
                        }
                    });
                });
                if !errors.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(tr("Could not load")).strong());
                    for error in &errors {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{}: {}", error.dir.display(), error.error))
                                    .small()
                                    .color(theme::MUTED),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(dir) = &plugin_dir {
                        ui.label(RichText::new(format!("{}: {}", tr("Plugins folder"), dir.display())).small().color(theme::MUTED));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary_button(ui, tr("Done")).clicked() {
                            done = true;
                        }
                    });
                });
            });
        self.plugins.manager_selected = selected;
        if let Some((id, granted)) = grant {
            self.grant_plugin(&id, granted);
        }
        if let Some((id, enabled)) = enable {
            self.config.plugins.entry(id.clone()).or_default().enabled = enabled;
            self.save_config();
            if !enabled {
                self.stop_plugin(&id);
            }
        }
        if let Some((id, setting, value)) = setting_changed {
            self.set_plugin_setting(&id, &setting, value);
        }
        if reload {
            self.load_plugins();
        }
        if !open || done {
            self.dialog = None;
        }
    }

    /// Store one setting (or secret) and tell the plugin.
    pub(super) fn set_plugin_setting(&mut self, plugin: &str, setting: &str, value: Value) {
        let is_secret = self
            .plugins
            .manifest(plugin)
            .and_then(|m| m.settings.iter().find(|s| s.id == setting))
            .is_some_and(|s| s.kind == InputKind::Secret);
        if is_secret {
            self.plugins
                .secrets
                .set(plugin, setting, value.as_str().unwrap_or_default());
            if let Some(path) = self.plugins.secrets_path.clone()
                && let Err(error) = self.plugins.secrets.save(&path)
            {
                self.error = Some(format!(
                    "{}\n\n{error:#}",
                    tr("Could not save plugin secrets")
                ));
            }
        } else if let Ok(value) = toml::Value::try_from(json_to_toml(value)) {
            self.config
                .plugins
                .entry(plugin.into())
                .or_default()
                .settings
                .insert(setting.into(), value);
            self.save_config();
        }
        self.notify_settings(plugin);
    }

    /// Floating status for running plugin jobs.
    pub(super) fn plugin_job_windows(&mut self, ctx: &egui::Context) {
        let jobs: Vec<(uuid::Uuid, String, Option<f32>, String)> = self
            .plugins
            .jobs
            .iter()
            .map(|job| (job.id, job.label.clone(), job.progress, job.message.clone()))
            .collect();
        let mut cancel = None;
        for (index, (id, label, progress, message)) in jobs.iter().enumerate() {
            egui::Window::new(label)
                .id(egui::Id::new(("plugin_job", id)))
                .anchor(
                    egui::Align2::RIGHT_BOTTOM,
                    egui::vec2(-16.0, -40.0 - 70.0 * index as f32),
                )
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .frame(theme::frame().inner_margin(egui::Margin::same(10)))
                .show(ctx, |ui| {
                    ui.set_width(240.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(tr("Cancel")).clicked() {
                                cancel = Some(*id);
                            }
                        });
                    });
                    match progress {
                        Some(fraction) => {
                            ui.add(egui::ProgressBar::new(*fraction).desired_height(6.0));
                        }
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new(tr("Working…")).color(theme::MUTED));
                            });
                        }
                    }
                    if !message.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(message).small().color(theme::MUTED))
                                .wrap(),
                        );
                    }
                });
        }
        if let Some(id) = cancel {
            self.cancel_plugin_job(id);
        }
    }

    pub(super) fn plugin_proposal_dialog(&mut self, ctx: &egui::Context) {
        let Some((name, message, comparing)) = self
            .plugins
            .proposal
            .as_ref()
            .map(|p| (p.name.clone(), p.message.clone(), p.comparing))
        else {
            self.dialog = None;
            return;
        };
        let mut accept = false;
        let mut discard = false;
        let mut compare = false;
        egui::Window::new(&name)
            .id(egui::Id::new("plugin_proposal"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .frame(theme::frame().inner_margin(egui::Margin::symmetric(14, 10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&name).strong());
                    if let Some(message) = &message {
                        ui.label(RichText::new(message).color(theme::MUTED));
                    }
                    ui.add_space(12.0);
                    if ui
                        .selectable_label(comparing, tr("Compare"))
                        .on_hover_text(tr("Hide the result to see the original"))
                        .clicked()
                    {
                        compare = true;
                    }
                    if widgets::button(ui, tr("Discard")).clicked() {
                        discard = true;
                    }
                    if widgets::primary_button(ui, tr("Accept")).clicked() {
                        accept = true;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            accept = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            discard = true;
        }
        if compare {
            self.toggle_proposal_compare();
        }
        if accept {
            self.resolve_proposal(true);
        } else if discard {
            self.resolve_proposal(false);
        }
    }
}

fn permissions_list(ui: &mut egui::Ui, manifest: &Manifest) {
    let permissions = &manifest.permissions;
    let mut any = false;
    if !permissions.network.is_empty() {
        any = true;
        ui.label(format!(
            "• {} {}",
            tr("Connects to:"),
            permissions.network.join(", ")
        ));
    }
    if !permissions.secrets.is_empty() {
        any = true;
        ui.label(format!(
            "• {} {}",
            tr("Receives these secrets:"),
            permissions.secrets.join(", ")
        ));
    }
    if permissions.document == xuan::plugins::manifest::DocumentAccess::Edit {
        any = true;
        ui.label(format!(
            "• {}",
            tr("Edits documents directly (as undoable steps)")
        ));
    }
    match permissions.filesystem {
        xuan::plugins::manifest::FilesystemAccess::None => {}
        xuan::plugins::manifest::FilesystemAccess::Read => {
            any = true;
            ui.label(format!("• {}", tr("Reads files outside its folders")));
        }
        xuan::plugins::manifest::FilesystemAccess::Write => {
            any = true;
            ui.label(format!(
                "• {}",
                tr("Reads and writes files outside its folders")
            ));
        }
    }
    if !any {
        ui.label(RichText::new(tr("Reads the document only")).color(theme::MUTED));
    }
}

/// JSON values from the settings form as TOML.
fn json_to_toml(value: Value) -> toml::Value {
    match value {
        Value::Null => toml::Value::String(String::new()),
        Value::Bool(b) => toml::Value::Boolean(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else {
                toml::Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => toml::Value::String(s),
        Value::Array(items) => toml::Value::Array(items.into_iter().map(json_to_toml).collect()),
        Value::Object(map) => {
            toml::Value::Table(map.into_iter().map(|(k, v)| (k, json_to_toml(v))).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_values_convert_to_toml() {
        assert_eq!(json_to_toml(Value::from(3)), toml::Value::Integer(3));
        assert_eq!(json_to_toml(Value::from(2.5)), toml::Value::Float(2.5));
        assert_eq!(json_to_toml(Value::Bool(true)), toml::Value::Boolean(true));
        assert_eq!(
            json_to_toml(serde_json::json!({"a": ["x"]})),
            toml::Value::Table(
                [(
                    "a".to_owned(),
                    toml::Value::Array(vec![toml::Value::String("x".into())])
                )]
                .into_iter()
                .collect()
            )
        );
    }
}

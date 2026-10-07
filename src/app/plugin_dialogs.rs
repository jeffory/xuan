//! Windows for plugins: the action dialog built from a manifest's inputs,
//! permission prompts, the plugin manager, running jobs and proposals.
use super::theme::PaletteExt as _;
use egui::RichText;
use serde_json::Value;
use xuan::{
    i18n::tr,
    plugins::{
        manifest::{Input, InputKind, Manifest, ResultInto},
        sandbox,
        ui::Node,
    },
};

use super::{EditorApp, plugins::PendingStart, theme, widgets};

/// A grant's folder in the permission prompt: `dir` itself, or for a plugin
/// that comes with Xuan (whose grant stores only the folder's name, see
/// `plugins::grant_dir`), the folder it is in now, `actual`.
fn grant_folder_text(dir: &std::path::Path, actual: &std::path::Path) -> String {
    if !xuan::plugins::is_bundled_grant_dir(dir) {
        return dir.display().to_string();
    }
    if actual == dir {
        let name = dir.file_name().unwrap_or_default().to_string_lossy();
        format!("{} ({name})", tr("the plugins that come with Xuan"))
    } else {
        format!("{} ({})", actual.display(), tr("comes with Xuan"))
    }
}

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
        // An action without inputs has no dialog; it is open only while the
        // user confirms what it sends.
        if spec.inputs.is_empty() {
            if self.plugins.consent.is_none() {
                self.close_plugin_action();
            }
            return;
        }
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
                    ui.add(egui::Label::new(RichText::new(&spec.description).color(ui.palette().muted)).wrap());
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
                                .color(ui.palette().muted),
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
                        ui.label(RichText::new(estimate).color(ui.palette().muted));
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
        let grant = super::plugins::grant_for(&manifest);
        let previous = self
            .stored_grant(&plugin)
            .filter(|g| !g.covers(&grant))
            .cloned();
        // The title names the plugin's id too: its name is its own choice.
        let source = self.plugins.source(&plugin);
        let blocked = sandbox::blocks_network(
            self.config.block_undeclared_network(),
            &manifest.permissions,
        );
        let title = if manifest.permissions.is_empty() {
            format!("{} {source}?", tr("Run"))
        } else {
            format!("{} {source}?", tr("Allow"))
        };
        widgets::Window::new(title)
            .id(("plugin_permissions", &plugin))
            .default_width(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add(
                    egui::Label::new(format!(
                        "{source} {} {}",
                        manifest.plugin.version,
                        if blocked {
                            tr("runs as a program with your rights. Xuan enforces the permissions below only for what Xuan itself sends the plugin and does for it, and blocks its network. The plugin can still read your files. Only run plugins you trust.")
                        } else {
                            tr("runs as a program with your rights. Xuan enforces the permissions below only for what Xuan itself sends the plugin and does for it. The plugin can still read your files and contact any server, whatever it declares. Only run plugins you trust.")
                        },
                    ))
                    .wrap(),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "{} {}\n{} {}",
                        tr("Folder:"),
                        grant_folder_text(&grant.dir, &manifest.dir),
                        tr("Runs:"),
                        grant.command.join(" ")
                    ))
                    .small()
                    .color(ui.palette().muted),
                );
                ui.add_space(8.0);
                permissions_list(ui, &manifest, blocked);
                if let Some(previous) = &previous {
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("Changed since you allowed it:")).strong());
                    if previous.dir != grant.dir {
                        ui.label(format!(
                            "• {} {}",
                            tr("It was in"),
                            grant_folder_text(&previous.dir, &previous.dir)
                        ));
                    }
                    if previous.command != grant.command {
                        ui.label(format!(
                            "• {} {}",
                            tr("It ran"),
                            previous.command.join(" ")
                        ));
                    }
                    if previous.permissions != grant.permissions {
                        ui.label(format!("• {}", tr("Its permissions changed")));
                    }
                }
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
                    // Not `start_plugin_action`: a provider run waiting for this
                    // grant continues as one.
                    PendingStart::Action(action) if !action.is_empty() => {
                        self.start_plugin_action_with(&plugin, &action, None)
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
        let mut install = false;
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
        let mut ask_again: Option<String> = None;
        let mut auto_mode: Option<(String, bool)> = None;
        let mut offline = self.config.disable_network_plugins;
        let mut block = self.config.block_undeclared_network();
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
                let mut separator = None;
                let columns = ui.horizontal_top(|ui| {
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
                                    let text = if enabled { text } else { text.color(ui.palette().muted) };
                                    if ui
                                        .selectable_label(selected.as_deref() == Some(&manifest.plugin.id), text)
                                        .clicked()
                                    {
                                        selected = Some(manifest.plugin.id.clone());
                                    }
                                }
                                if manifests.is_empty() {
                                    ui.label(RichText::new(tr("No plugins installed")).color(ui.palette().muted));
                                }
                            });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if widgets::button(ui, tr("Reload")).clicked() {
                                reload = true;
                            }
                            if widgets::button(ui, tr("Install…"))
                                .on_hover_text(tr("Install a plugin from a folder or a .zip archive. You can also drop one on this window."))
                                .clicked()
                            {
                                install = true;
                            }
                        });
                    });
                    // A separator here would take all the height the window
                    // may grow to; draw one as tall as the columns instead.
                    separator = Some(ui.cursor().min.x + 6.0);
                    ui.add_space(12.0);
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
                                ui.label(RichText::new(dir.display().to_string()).color(ui.palette().muted));
                            }
                            return;
                        };
                        let id = manifest.plugin.id.clone();
                        let config = self.config.plugins.get(&id).cloned().unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.heading(&manifest.plugin.name);
                            ui.label(RichText::new(&manifest.plugin.version).color(ui.palette().muted));
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
                        let folder = grant_folder_text(&super::plugins::grant_for(manifest).dir, &manifest.dir);
                        ui.label(RichText::new(folder).small().color(ui.palette().muted));
                        let status = if self.plugin_offline(&id) {
                            tr("Off: plugins that use the network are disabled")
                        } else if self.plugins.running(&id) {
                            tr("Running")
                        } else {
                            tr("Not running")
                        };
                        ui.label(RichText::new(status).small().color(ui.palette().muted));
                        ui.add_space(8.0);
                        {
                            ui.label(RichText::new(tr("Permissions")).strong());
                            permissions_list(ui, manifest, self.plugin_network_blocked(&id));
                            ui.horizontal(|ui| {
                                if self.plugin_granted(&id) {
                                    ui.label(RichText::new(tr("Allowed")).color(ui.palette().muted));
                                    if widgets::button(ui, tr("Revoke")).clicked() {
                                        grant = Some((id.clone(), false));
                                    }
                                } else if widgets::primary_button(ui, tr("Allow")).clicked() {
                                    grant = Some((id.clone(), true));
                                }
                            });
                            if self.plugin_granted(&id)
                                && self.stored_grant(&id).is_some_and(|g| g.send_without_asking)
                            {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(tr("Gets document data without asking"))
                                            .color(ui.palette().muted),
                                    );
                                    if widgets::button(ui, tr("Ask Again")).clicked() {
                                        ask_again = Some(id.clone());
                                    }
                                });
                            }
                            if self.plugin_granted(&id) && self.asks_before_edits(&id) {
                                let mut auto = self.edits_without_asking(&id);
                                if widgets::checkbox(ui, &mut auto, tr("Edit without asking (auto mode)"))
                                    .on_hover_text(tr("Off: the plugin asks before its first edit in each session. On: it edits your documents without asking; every edit is still one step you can undo."))
                                    .changed()
                                {
                                    auto_mode = Some((id.clone(), auto));
                                }
                            }
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
                        for pane in (manifest.panes.iter())
                            .filter(|pane| pane.placement == xuan::plugins::manifest::PanePlacement::Settings)
                        {
                            ui.label(RichText::new(&pane.title).strong());
                            self.plugin_settings_pane(ui, &super::plugins::pane_key(&id, &pane.id));
                            ui.add_space(8.0);
                        }
                        self.plugin_models_section(ui, manifest);
                        let summary = format!(
                            "{} {} · {} {} · {} {}",
                            manifest.actions.len(),
                            tr("actions"),
                            manifest.panes.len(),
                            tr("panes"),
                            manifest.formats.len(),
                            tr("formats")
                        );
                        ui.label(RichText::new(summary).small().color(ui.palette().muted));
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
                if let Some(x) = separator {
                    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                    ui.painter().vline(x, columns.response.rect.y_range(), stroke);
                }
                if !errors.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(tr("Could not load")).strong());
                    for error in &errors {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{}: {}", error.dir.display(), error.error))
                                    .small()
                                    .color(ui.palette().muted),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.separator();
                widgets::checkbox(ui, &mut offline, tr("Disable plugins that use the network"))
                    .on_hover_text(super::plugin_consent::offline_mode_note(&self.config));
                if sandbox::SUPPORTED {
                    widgets::checkbox(ui, &mut block, tr("Block network for plugins that don't declare it"))
                        .on_hover_text(tr("Plugins that declare no network hosts cannot open network sockets, not even to this computer (localhost). Running plugins restart to apply it. Plugins that declare hosts are not blocked."));
                }
                ui.horizontal(|ui| {
                    if let Some(dir) = &plugin_dir {
                        ui.label(RichText::new(format!("{}: {}", tr("Plugins folder"), dir.display())).small().color(ui.palette().muted));
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
        if let Some(id) = ask_again {
            self.ask_before_sending_again(&id);
        }
        if let Some((id, on)) = auto_mode {
            self.set_edit_auto_mode(&id, on);
        }
        if offline != self.config.disable_network_plugins {
            self.set_network_plugins_disabled(offline);
        }
        if block != self.config.block_undeclared_network() {
            self.set_block_undeclared_network(block);
        }
        if reload {
            self.load_plugins();
        }
        if !open || done {
            self.dialog = None;
        } else if install {
            self.open_plugin_install();
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
            if let Err(error) = self.plugins.save_secrets() {
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

    /// Plugin actions, plugin imports and exports, and model downloads that
    /// are still running, oldest first.
    pub(super) fn running_jobs(&self) -> Vec<RunningJob> {
        let actions = self.plugins.jobs.iter().map(|job| RunningJob {
            id: job.id,
            label: job.label.trim_end_matches('…').to_owned(),
            progress: job.progress,
            message: job.message.clone(),
            document: Some(job.document),
        });
        let formats = self.plugins.formats.iter().map(|job| RunningJob {
            id: job.id,
            label: job.label.clone(),
            progress: None,
            message: String::new(),
            document: None,
        });
        let models = self.plugins.model_jobs.iter().map(|job| RunningJob {
            id: job.id,
            label: job.label(),
            progress: Some(job.fraction()),
            message: format!(
                "{} / {}",
                super::plugin_models::format_size(job.progress.done()),
                super::plugin_models::format_size(job.size)
            ),
            document: None,
        });
        actions.chain(formats).chain(models).collect()
    }

    pub(super) fn cancel_running_job(&mut self, id: uuid::Uuid) {
        if self.plugins.model_jobs.iter().any(|job| job.id == id) {
            self.cancel_model_job(id);
        } else if self.plugins.formats.iter().any(|job| job.id == id) {
            self.cancel_format_job(id);
        } else {
            self.cancel_plugin_job(id);
        }
    }

    /// The running jobs at the right of the status bar, laid out right to
    /// left: Cancel, progress, a count that lists every job when there are
    /// several, then the job's name and message. The job shown is the first
    /// one for the current document, else the oldest.
    pub(super) fn job_status(&mut self, ui: &mut egui::Ui, jobs: &[RunningJob]) {
        let current = self.session().map(|s| s.document.id);
        let index = (jobs.iter())
            .position(|job| current.is_some() && job.document == current)
            .unwrap_or(0);
        let Some(job) = jobs.get(index) else {
            return;
        };
        let muted = ui.palette().muted;
        let mut cancel = None;
        if ui.small_button(tr("Cancel")).clicked() {
            cancel = Some(job.id);
        }
        progress_indicator(ui, job.progress, 120.0);
        if jobs.len() > 1 {
            let count = tr("{index} of {count}")
                .replace("{index}", &(index + 1).to_string())
                .replace("{count}", &jobs.len().to_string());
            let button = ui
                .add(egui::Button::new(RichText::new(count).size(11.0)).small())
                .on_hover_text(tr("Show all running jobs"));
            egui::Popup::menu(&button).show(|ui| {
                ui.set_min_width(280.0);
                for job in jobs {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&job.label).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(tr("Cancel")).clicked() {
                                cancel = Some(job.id);
                            }
                        });
                    });
                    progress_indicator(ui, job.progress, 280.0);
                    if !job.message.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(&job.message).small().color(muted))
                                .wrap(),
                        );
                    }
                    ui.add_space(4.0);
                }
            });
        }
        let text = if job.message.is_empty() {
            job.label.clone()
        } else {
            format!("{} · {}", job.label, job.message)
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.0)).truncate());
        if let Some(id) = cancel {
            self.cancel_running_job(id);
        }
    }

    pub(super) fn plugin_proposal_dialog(&mut self, ctx: &egui::Context) {
        let Some((name, source, message, comparing)) = self.plugins.proposal.as_ref().map(|p| {
            (
                p.name.clone(),
                p.source.clone(),
                p.message.clone(),
                p.comparing,
            )
        }) else {
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
            .frame(theme::frame(&ctx.palette()).inner_margin(egui::Margin::symmetric(14, 10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&name).strong())
                        .on_hover_text(&source);
                    ui.label(RichText::new(&source).small().color(ui.palette().muted));
                    if let Some(message) = &message {
                        ui.label(RichText::new(message).color(ui.palette().muted));
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

/// The permissions a plugin declares. `blocked` says whether it starts with
/// its network blocked ([`xuan::plugins::sandbox`]).
pub(super) fn permissions_list(ui: &mut egui::Ui, manifest: &Manifest, blocked: bool) {
    let permissions = &manifest.permissions;
    if blocked {
        ui.label(format!("• {}", tr("Network blocked by Xuan (Linux)")));
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "  {}",
                    tr("It cannot open network sockets, not even to this computer.")
                ))
                .small()
                .color(ui.palette().muted),
            )
            .wrap(),
        );
    }
    if !permissions.network.is_empty() {
        ui.add(
            egui::Label::new(format!(
                "• {} {}",
                tr("Says it connects to:"),
                permissions.network.join(", ")
            ))
            .wrap(),
        );
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "  {}",
                    tr("Not enforced: it can contact any server. Xuan asks before sending it your image, regions or text.")
                ))
                .small()
                .color(ui.palette().muted),
            )
            .wrap(),
        );
    }
    if !manifest.models.is_empty() {
        let models = (manifest.models.iter())
            .map(|model| {
                format!(
                    "{} ({}, {})",
                    model.id,
                    super::plugin_models::format_size(model.size),
                    super::plugins::one_line(&model.host(), 120)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        ui.add(
            egui::Label::new(format!(
                "• {} {models}",
                tr("Uses models that Xuan downloads, after asking you:")
            ))
            .wrap(),
        );
    }
    if !permissions.secrets.is_empty() {
        ui.label(format!(
            "• {} {}",
            tr("Receives these secrets:"),
            permissions.secrets.join(", ")
        ));
    }
    if permissions.document == xuan::plugins::manifest::DocumentAccess::Edit {
        ui.label(format!(
            "• {}",
            tr("Edits documents directly (as undoable steps)")
        ));
        if permissions.edit_prompt == xuan::plugins::manifest::EditPrompt::Session {
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "  {}",
                        tr("Asks you before its first edit in each session, unless you turn on auto mode.")
                    ))
                    .small()
                    .color(ui.palette().muted),
                )
                .wrap(),
            );
        }
    } else {
        ui.label(format!(
            "• {}",
            tr("Can read the document and propose selections or new documents, but can't change your image")
        ));
    }
    match permissions.filesystem {
        xuan::plugins::manifest::FilesystemAccess::None => {}
        xuan::plugins::manifest::FilesystemAccess::Read => {
            ui.label(format!(
                "• {}",
                tr(
                    "Has Xuan read any file for it, not only those in its own and temporary folders"
                )
            ));
        }
        xuan::plugins::manifest::FilesystemAccess::Write => {
            ui.label(format!(
                "• {}",
                tr("Has Xuan read and write any file or folder for it, not only its own and temporary folders")
            ));
        }
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

/// A background job as the status bar shows it.
pub(super) struct RunningJob {
    pub id: uuid::Uuid,
    pub label: String,
    pub progress: Option<f32>,
    pub message: String,
    /// The document a plugin action works on.
    pub document: Option<uuid::Uuid>,
}

/// A progress bar once the job reports a fraction, a spinner until then.
fn progress_indicator(ui: &mut egui::Ui, progress: Option<f32>, width: f32) {
    match progress {
        Some(fraction) => {
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(width)
                    .desired_height(6.0),
            );
        }
        None => {
            ui.spinner();
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

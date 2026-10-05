//! Asking before document data goes to a plugin that declares network hosts,
//! offline mode, which keeps such plugins from starting at all, and blocking
//! the network of plugins that declare none (Linux only).
//!
//! Xuan cannot stop a plugin that declares hosts from connecting anywhere, so
//! these rules cover what Xuan itself hands over: the source image of an
//! action, its regions and text inputs, and the layer, composite and
//! selection exports a plugin asks for. See "Network" in `docs/PLUGINS.md`.
use egui::RichText;
use serde_json::Value;
use xuan::{
    i18n::tr,
    plugins::{
        manifest::{Action, ActionKind, InputKind, SourceKind, SourceMask},
        protocol::{self, Request, RpcError},
        sandbox,
    },
};

use super::{Dialog, EditorApp, plugins::one_line, theme, widgets};

/// Requests that hand a plugin pixels outside the source of an action.
pub(super) const EXPORT_METHODS: [&str; 3] =
    ["layer/export", "document/export", "selection/export"];

/// What offline mode means for the plugins it does not stop.
pub(super) fn offline_mode_note(config: &xuan::config::Config) -> &'static str {
    if sandbox::SUPPORTED && config.block_undeclared_network() {
        tr(
            "Plugins that declare network hosts do not start and their actions are unavailable. Xuan blocks the network of the others.",
        )
    } else {
        tr(
            "Plugins that declare network hosts do not start and their actions are unavailable. A plugin that declares none could still connect.",
        )
    }
}

/// Document data waiting for the user's answer before it is sent.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ConsentRequest {
    pub plugin: String,
    /// The action whose run waits for the answer, or `None` for exports the
    /// plugin asked for outside a consented action.
    pub action: Option<String>,
    /// What would be sent, one line each.
    pub items: Vec<String>,
    /// The "Don't ask again for this plugin" checkbox.
    pub dont_ask: bool,
}

impl EditorApp {
    /// Whether the plugin's manifest declares network hosts.
    pub(super) fn plugin_uses_network(&self, plugin: &str) -> bool {
        self.plugins
            .manifest(plugin)
            .is_some_and(|m| !m.permissions.network.is_empty())
    }

    /// Whether offline mode keeps this plugin from starting.
    pub(super) fn plugin_offline(&self, plugin: &str) -> bool {
        self.config.disable_network_plugins && self.plugin_uses_network(plugin)
    }

    /// Whether the plugin starts with its network blocked: "Block network
    /// for plugins that don't declare it" is on, it declares no network
    /// hosts, and this is Linux.
    pub(super) fn plugin_network_blocked(&self, plugin: &str) -> bool {
        self.plugins.manifest(plugin).is_some_and(|m| {
            sandbox::blocks_network(self.config.block_undeclared_network(), &m.permissions)
        })
    }

    /// Enabled, and not held back by offline mode.
    pub(super) fn plugin_available(&self, plugin: &str) -> bool {
        self.plugin_enabled(plugin) && !self.plugin_offline(plugin)
    }

    /// Whether document data for this plugin needs the user's confirmation:
    /// it declares network hosts and the user did not choose "Don't ask
    /// again" for the grant it runs under.
    pub(super) fn sends_need_consent(&self, plugin: &str) -> bool {
        self.plugin_uses_network(plugin)
            && !(self.plugin_granted(plugin)
                && self
                    .stored_grant(plugin)
                    .is_some_and(|g| g.send_without_asking))
    }

    /// Turn offline mode on or off, from Settings or Manage Plugins.
    pub(super) fn set_network_plugins_disabled(&mut self, disabled: bool) {
        if self.config.disable_network_plugins != disabled {
            self.config.disable_network_plugins = disabled;
            self.save_config();
        }
        self.apply_network_plugins_setting();
    }

    /// Stop the plugins offline mode holds back, with their panes, open
    /// action dialog and pending prompts. Their panes open again when it is
    /// turned off.
    pub(super) fn apply_network_plugins_setting(&mut self) {
        let offline: Vec<String> = (self.plugins.manifests.iter())
            .map(|m| m.plugin.id.clone())
            .filter(|id| self.plugin_offline(id))
            .collect();
        for plugin in &offline {
            self.shut_down_plugin(plugin);
        }
    }

    /// Turn "Block network for plugins that don't declare it" on or off.
    pub(super) fn set_block_undeclared_network(&mut self, block: bool) {
        let changed = self.config.block_undeclared_network() != block;
        self.config.block_undeclared_network = Some(block);
        self.save_config();
        if changed {
            self.apply_block_network_setting();
        }
    }

    /// The filter is set when a plugin starts, so stop the running plugins
    /// it applies to, with their panes and open action dialog. They start
    /// again with the new setting when next used, their panes at once.
    pub(super) fn apply_block_network_setting(&mut self) {
        if !sandbox::SUPPORTED {
            return;
        }
        let affected: Vec<String> = (self.plugins.manifests.iter())
            .filter(|m| m.permissions.network.is_empty())
            .map(|m| m.plugin.id.clone())
            .filter(|id| self.plugins.running(id))
            .collect();
        for plugin in &affected {
            self.shut_down_plugin(plugin);
        }
    }

    /// Stop a plugin with its panes, open action dialog and pending prompts.
    /// Panes that are shown open, and start it, again.
    fn shut_down_plugin(&mut self, plugin: &str) {
        self.stop_plugin(plugin);
        let prefix = format!("plugin:{plugin}/");
        self.plugins
            .panes
            .retain(|key, _| !key.starts_with(&prefix));
        if self
            .plugins
            .action
            .as_ref()
            .is_some_and(|e| e.plugin == plugin)
        {
            self.close_plugin_action();
        }
        if self
            .plugins
            .consent
            .as_ref()
            .is_some_and(|c| c.plugin == plugin)
        {
            self.plugins.consent = None;
            if self.dialog == Some(Dialog::PluginConsent) {
                self.dialog = None;
            }
        }
    }

    /// Forget the "Don't ask again" answer stored with a plugin's grant.
    pub(super) fn ask_before_sending_again(&mut self, plugin: &str) {
        if let Some(grant) = (self.config.plugins.get_mut(plugin)).and_then(|c| c.grant.as_mut())
            && grant.send_without_asking
        {
            grant.send_without_asking = false;
            self.save_config();
        }
        self.plugins.export_answers.remove(plugin);
    }

    /// What running the open action sends besides the document's structure,
    /// one line each, or nothing when it sends no document data.
    pub(super) fn action_consent_items(&self, spec: &Action) -> Vec<String> {
        let Some(edit) = &self.plugins.action else {
            return Vec::new();
        };
        let document = self.session().map(|s| &s.document);
        let mut items = Vec::new();
        if let Some(document) = document
            && spec.kind == ActionKind::Edit
        {
            let mut image = match spec.source.from {
                SourceKind::None => None,
                SourceKind::Layer => Some(format!(
                    "{} “{}”",
                    tr("The pixels of the layer"),
                    document
                        .active()
                        .map_or(String::new(), |layer| one_line(&layer.name, 80))
                )),
                SourceKind::Composite => Some(tr("The whole image, flattened").to_owned()),
                SourceKind::Selection => {
                    Some(tr("The flattened image inside the selection").to_owned())
                }
            };
            if let Some(image) = &mut image {
                if spec.source.crop_to_regions
                    && !edit.regions.is_empty()
                    && spec.source.from != SourceKind::Selection
                {
                    image.push_str(&format!(", {}", tr("cropped around the regions")));
                }
                if spec.source.from == SourceKind::Composite
                    && let Some(extend) = &spec.source.with_inputs(&edit.values).extend
                {
                    let margins = extend.margins();
                    let sides: Vec<String> = [
                        (margins.left, tr("on the left")),
                        (margins.top, tr("at the top")),
                        (margins.right, tr("on the right")),
                        (margins.bottom, tr("at the bottom")),
                    ]
                    .into_iter()
                    .filter(|(pixels, _)| *pixels > 0)
                    .map(|(pixels, side)| format!("{pixels} px {side}"))
                    .collect();
                    if !sides.is_empty() {
                        image.push_str(&format!(", {} {}", tr("extended by"), sides.join(", ")));
                    }
                }
                if let Some(side) = spec.source.max_side {
                    image.push_str(&format!(", {} {side} px", tr("longest side at most")));
                }
            }
            let has_image = image.is_some();
            items.extend(image);
            // A constant mask for "nothing selected" carries no document data.
            if has_image
                && spec.source.mask == SourceMask::Selection
                && document.selection.is_some()
            {
                items.push(tr("The selection, as a mask").into());
            }
        }
        if let Some(input) = spec.regions_input()
            && !edit.regions.is_empty()
        {
            let mut line = format!(
                "{} {}",
                tr("Positions and sizes of the regions:"),
                edit.regions.len()
            );
            if edit.regions.iter().any(|r| r.mask.is_some()) {
                line.push_str(&format!(", {}", tr("with the shape of the selection")));
            }
            items.push(line);
            for (index, region) in edit.regions.iter().enumerate() {
                for field in &input.fields {
                    if let Some(text) = text_value(field.kind, region.fields.get(&field.id)) {
                        items.push(format!(
                            "{} {} · {}: {text}",
                            tr("Region"),
                            index + 1,
                            one_line(field.label(), 80)
                        ));
                    }
                }
            }
        }
        for input in &spec.inputs {
            if let Some(text) = text_value(input.kind, edit.values.get(&input.id)) {
                items.push(format!("{}: {text}", one_line(input.label(), 80)));
            }
        }
        if items.is_empty() {
            return items;
        }
        let others: Vec<String> = (spec.inputs.iter())
            .filter(|input| {
                !matches!(
                    input.kind,
                    InputKind::Text
                        | InputKind::Multiline
                        | InputKind::Path
                        | InputKind::Secret
                        | InputKind::Regions
                )
            })
            .map(|input| one_line(input.label(), 80))
            .collect();
        if !others.is_empty() {
            items.push(format!("{} {}", tr("Options:"), others.join(", ")));
        }
        if document.is_some() {
            items.push(tr("The document's size and the names and positions of its layers").into());
        }
        items
    }

    /// What an export request would send, for the prompt.
    fn export_item(&self, request: &Request) -> String {
        let params = &request.params;
        match request.method.as_str() {
            "layer/export" => {
                let name = (params.get("layer").and_then(Value::as_str))
                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                    .and_then(|id| {
                        self.session()?
                            .document
                            .layers
                            .iter()
                            .find(|l| l.id == id)
                            .map(|l| one_line(&l.name, 80))
                    })
                    .unwrap_or_default();
                let what = if params.get("what").and_then(Value::as_str) == Some("mask") {
                    tr("The mask of the layer")
                } else {
                    tr("The pixels of the layer")
                };
                format!("{what} “{name}”")
            }
            "document/export" => tr("The whole image, flattened").into(),
            _ => tr("The shape of the selection").into(),
        }
    }

    /// The user's answer for exports this plugin asks for: `Some(true)` to
    /// send, `Some(false)` to refuse, `None` when the user must be asked.
    /// Exports are sent without asking to a plugin that declares no network
    /// hosts, after "Don't ask again", and while one of its actions the user
    /// confirmed is running; otherwise the user is asked once for as long as
    /// the plugin's process runs.
    pub(super) fn export_answer(&self, plugin: &str) -> Option<bool> {
        if !self.sends_need_consent(plugin)
            || (self.plugins.jobs.iter()).any(|job| job.plugin == plugin && job.consented)
        {
            return Some(true);
        }
        self.plugins.export_answers.get(plugin).copied()
    }

    /// The error for an export the user did not allow.
    pub(super) fn export_refused(&self, plugin: &str) -> Option<RpcError> {
        (self.export_answer(plugin) != Some(true)).then(|| {
            RpcError::new(
                protocol::CANCELLED,
                "The user did not allow sending document data to this plugin",
            )
        })
    }

    /// Answer the exports that waited for the user, and ask about the next
    /// one when nothing else is open.
    pub(super) fn release_held_requests(&mut self) {
        let held = std::mem::take(&mut self.plugins.held);
        let mut waiting = Vec::new();
        for (plugin, request) in held {
            if !self.plugins.running(&plugin) {
                continue;
            }
            if self.export_answer(&plugin).is_none() {
                waiting.push((plugin, request));
                continue;
            }
            let result = self.service_request(&plugin, &request);
            if let Some(process) = self.plugins.process_mut(&plugin) {
                let _ = process.respond(request.id, result);
            }
        }
        self.plugins.held = waiting;
        if self.plugins.consent.is_none()
            && self.dialog.is_none()
            && let Some((plugin, request)) = self.plugins.held.first()
        {
            self.plugins.consent = Some(ConsentRequest {
                plugin: plugin.clone(),
                action: None,
                items: vec![self.export_item(request)],
                dont_ask: false,
            });
            self.dialog = Some(Dialog::PluginConsent);
        }
    }

    /// Apply the answer to the open prompt.
    pub(super) fn answer_consent(&mut self, send: bool) {
        let Some(request) = self.plugins.consent.take() else {
            return;
        };
        if self.dialog == Some(Dialog::PluginConsent) {
            self.dialog = None;
        }
        let plugin = request.plugin;
        if send
            && request.dont_ask
            && self.plugin_granted(&plugin)
            && let Some(grant) =
                (self.config.plugins.get_mut(&plugin)).and_then(|c| c.grant.as_mut())
        {
            grant.send_without_asking = true;
            self.save_config();
        }
        match request.action {
            Some(action) => {
                let waiting = (self.plugins.action.as_ref())
                    .is_some_and(|edit| edit.plugin == plugin && edit.action == action);
                if !waiting {
                    return;
                }
                if send {
                    if let Some(edit) = &mut self.plugins.action {
                        edit.consented = true;
                    }
                    self.run_plugin_action();
                } else {
                    self.status = tr("Cancelled; nothing was sent").into();
                    // Without inputs there is no dialog to go back to.
                    let inputs = (self.plugins.manifest(&plugin))
                        .and_then(|m| m.action(&action))
                        .is_some_and(|spec| !spec.inputs.is_empty());
                    if !inputs {
                        self.close_plugin_action();
                    }
                }
            }
            None => {
                self.plugins.export_answers.insert(plugin, send);
                self.release_held_requests();
            }
        }
    }

    pub(super) fn plugin_consent_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.plugins.consent.clone() else {
            self.dialog = None;
            return;
        };
        let Some(manifest) = self.plugins.manifest(&request.plugin).cloned() else {
            self.plugins.consent = None;
            self.dialog = None;
            return;
        };
        let source = self.plugins.source(&request.plugin);
        let action = (request.action.as_deref())
            .and_then(|id| manifest.action(id))
            .map(|spec| one_line(spec.label.trim_end_matches('…'), 80));
        let mut open = true;
        let mut answer = None;
        let mut dont_ask = request.dont_ask;
        widgets::Window::new(format!("{} {source}?", tr("Send to")))
            .id(("plugin_consent", &request.plugin))
            .default_width(440.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let hosts = (manifest.permissions.network.iter())
                    .map(|host| one_line(host, 120))
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.add(
                    egui::Label::new(format!(
                        "{source} {} {hosts}",
                        tr("says it connects to:")
                    ))
                    .wrap(),
                );
                ui.add_space(8.0);
                let lead = match &action {
                    Some(action) => format!("{} “{action}”, {}", tr("To run"), tr("Xuan sends it:")),
                    None => tr("It asks Xuan for this outside an action you started:").into(),
                };
                ui.label(RichText::new(lead).strong());
                for item in &request.items {
                    ui.add(egui::Label::new(format!("• {item}")).wrap());
                }
                ui.add_space(8.0);
                let note = if action.is_some() {
                    tr("While the action runs, the plugin may also ask for other layers or the whole image. Xuan cannot check where the plugin sends what it receives.")
                } else {
                    tr("Your answer holds until the plugin stops. Xuan cannot check where the plugin sends what it receives.")
                };
                ui.add(egui::Label::new(RichText::new(note).small().color(theme::MUTED)).wrap());
                ui.add_space(8.0);
                widgets::checkbox(ui, &mut dont_ask, tr("Don't ask again for this plugin"));
                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary_button(ui, tr("Send")).clicked() {
                            answer = Some(true);
                        }
                        if widgets::button(ui, tr("Cancel")).clicked() {
                            answer = Some(false);
                        }
                    });
                });
            });
        if let Some(consent) = &mut self.plugins.consent {
            consent.dont_ask = dont_ask;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
            answer = Some(false);
        }
        if let Some(send) = answer {
            self.answer_consent(send);
        }
    }
}

/// A text-like input's value as the prompt shows it, or `None` when empty
/// or not text. Secrets are not shown.
fn text_value(kind: InputKind, value: Option<&Value>) -> Option<String> {
    let text = value?.as_str().filter(|text| !text.trim().is_empty())?;
    match kind {
        InputKind::Text | InputKind::Multiline | InputKind::Path => {
            let shown = one_line(text, 200);
            let more = if text.chars().count() > 200 {
                "…"
            } else {
                ""
            };
            Some(format!("“{shown}{more}”"))
        }
        InputKind::Secret => Some("••••".into()),
        _ => None,
    }
}

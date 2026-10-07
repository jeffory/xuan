//! Asking before a plugin edits documents directly, once per session.
//!
//! A plugin whose manifest says `edit_prompt = "session"` (the MCP server,
//! whose edits come from an LLM client) has its direct edits held until the
//! user allows them: `document/edit` and the `host/run` commands that edit.
//! A session is the plugin's process, divided further by the `session` id it
//! may send with each request, so each client connection asks again. The
//! user answers **Allow** or **Deny** for the session, or **Always Allow**,
//! which turns on auto mode: `edit_without_asking` in the plugin's grant,
//! dropped with the grant when the plugin's folder, command or permissions
//! change. Results of actions keep their own Accept/Discard proposals. See
//! "Edit sessions" in `docs/PLUGINS.md`.
use super::theme::PaletteExt as _;
use egui::RichText;
use serde_json::{Value, json};
use xuan::{
    i18n::tr,
    plugins::{
        manifest::{DocumentAccess, EditPrompt},
        protocol::{self, Id, Request, RpcError},
    },
};

use super::{
    Dialog, EditorApp,
    commands::{self, HostRun},
    plugins::one_line,
    widgets,
};

/// Edits one plugin may have waiting for the answer.
const MAX_HELD: usize = 64;
/// The longest session id kept; a longer one is cut.
const MAX_SESSION: usize = 128;
/// After the user refuses, how long a plugin may not ask again: its new
/// sessions' edits are refused without a prompt meanwhile, so a client
/// cannot wear the user down by starting session after session.
pub(super) const COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

/// The open prompt: the first edit of a session waiting for an answer.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EditSessionRequest {
    pub plugin: String,
    pub session: String,
    /// The name of the edit that asked, as its undo step will show it.
    pub edit: String,
    /// "Refuse every session until the plugin stops", with Deny.
    pub deny_all: bool,
}

/// The user's answer to the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditAnswer {
    Allow,
    Deny,
    /// Allow, and stop asking: auto mode.
    Always,
}

/// The session a request belongs to: its `session` param, or the plugin's
/// process alone.
pub(super) fn request_session(request: &Request) -> String {
    let session = request
        .params
        .get("session")
        .and_then(Value::as_str)
        .unwrap_or_default();
    one_line(session, MAX_SESSION)
}

impl EditorApp {
    /// Whether the plugin's direct edits wait for the session prompt.
    pub(super) fn asks_before_edits(&self, plugin: &str) -> bool {
        self.plugins.manifest(plugin).is_some_and(|m| {
            m.permissions.edit_prompt == EditPrompt::Session
                && m.permissions.document == DocumentAccess::Edit
        })
    }

    /// Whether auto mode is on for the plugin's current grant.
    pub(super) fn edits_without_asking(&self, plugin: &str) -> bool {
        self.plugin_granted(plugin)
            && self
                .stored_grant(plugin)
                .is_some_and(|grant| grant.edit_without_asking)
    }

    /// Whether the request edits a document directly and so waits for the
    /// session's answer: `document/edit`, or a `host/run` of a built-in
    /// command that edits. Starting the plugin's own action does not: its
    /// result is a proposal.
    pub(super) fn gated_edit(&self, plugin: &str, request: &Request) -> bool {
        if !self.asks_before_edits(plugin) {
            return false;
        }
        match request.method.as_str() {
            "document/edit" => true,
            "host/run" => request
                .params
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|action| {
                    !action.contains('/') && commands::host_run(action) == HostRun::Edit
                }),
            _ => false,
        }
    }

    /// The answer for the plugin's edits in this session: `Some(true)` to
    /// apply them, `Some(false)` to refuse, `None` when the user must be asked.
    pub(super) fn edit_answer(&self, plugin: &str, session: &str) -> Option<bool> {
        if !self.asks_before_edits(plugin) || self.edits_without_asking(plugin) {
            return Some(true);
        }
        if self.plugins.edits_refused.contains(plugin) {
            return Some(false);
        }
        self.plugins
            .edit_answers
            .get(&(plugin.to_owned(), session.to_owned()))
            .copied()
    }

    /// The error for a direct edit the user did not allow (yet).
    pub(super) fn edit_refused(&self, plugin: &str, request: &Request) -> Option<RpcError> {
        if !self.gated_edit(plugin, request) {
            return None;
        }
        (self.edit_answer(plugin, &request_session(request)) != Some(true)).then(|| {
            RpcError::new(
                protocol::CANCELLED,
                "The user did not allow this plugin to edit documents in this session",
            )
        })
    }

    /// Whether the user refused this plugin's edits too recently for it to
    /// ask again.
    pub(super) fn edit_cooling_down(&self, plugin: &str) -> bool {
        (self.plugins.edit_refused_at.get(plugin)).is_some_and(|at| at.elapsed() < COOLDOWN)
    }

    /// Hold a direct edit until the user answered for its session. Returns
    /// the request when it need not wait.
    pub(super) fn hold_edit(&mut self, plugin: &str, request: Request) -> Option<Request> {
        if !self.gated_edit(plugin, &request)
            || self
                .edit_answer(plugin, &request_session(&request))
                .is_some()
        {
            return Some(request);
        }
        if self.edit_cooling_down(plugin) {
            // Refused without asking: see `COOLDOWN`.
            self.refuse_edit(plugin, request);
            return None;
        }
        let held = (self.plugins.edit_held.iter())
            .filter(|(id, _)| id == plugin)
            .count();
        if held >= MAX_HELD {
            if let Some(process) = self.plugins.process_mut(plugin) {
                let _ = process.respond(
                    request.id,
                    Err(RpcError::new(
                        protocol::RATE_LIMITED,
                        "Too many edits are waiting for the user's answer",
                    )),
                );
            }
            return None;
        }
        self.plugins.edit_held.push((plugin.to_owned(), request));
        None
    }

    fn refuse_edit(&mut self, plugin: &str, request: Request) {
        if let Some(process) = self.plugins.process_mut(plugin) {
            let _ = process.respond(
                request.id,
                Err(RpcError::new(
                    protocol::CANCELLED,
                    "The user refused this plugin's edits just now; it may ask again later",
                )),
            );
        }
    }

    /// Apply the edits whose session was answered, and ask about the next
    /// session when nothing else is open.
    pub(super) fn release_held_edits(&mut self) {
        let held = std::mem::take(&mut self.plugins.edit_held);
        let mut waiting = Vec::new();
        for (plugin, request) in held {
            if !self.plugins.running(&plugin) {
                continue;
            }
            if self
                .edit_answer(&plugin, &request_session(&request))
                .is_none()
            {
                if self.edit_cooling_down(&plugin) {
                    self.refuse_edit(&plugin, request);
                } else {
                    waiting.push((plugin, request));
                }
                continue;
            }
            let result = self.service_request(&plugin, &request);
            if let Some(process) = self.plugins.process_mut(&plugin) {
                let _ = process.respond(request.id, result);
            }
        }
        // Edits that arrived while these were applied wait behind them.
        waiting.append(&mut self.plugins.edit_held);
        self.plugins.edit_held = waiting;
        // Another dialog may have replaced the open prompt (a menu command,
        // opening a document): its edits still wait, so show it again.
        if let Some(prompt) = &self.plugins.edit_prompt
            && self.dialog.is_none()
        {
            if self.plugins.running(&prompt.plugin) {
                self.dialog = Some(Dialog::PluginEditSession);
            } else {
                self.plugins.edit_prompt = None;
            }
        }
        if self.plugins.edit_prompt.is_none()
            && self.dialog.is_none()
            && let Some((plugin, request)) = self.plugins.edit_held.first()
        {
            let edit = match request.method.as_str() {
                "host/run" => (request.params.get("action").and_then(Value::as_str))
                    .and_then(commands::find)
                    .map_or_else(String::new, |command| tr(command.label).to_owned()),
                _ => request
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| one_line(name, 120))
                    .unwrap_or_default(),
            };
            self.plugins.edit_prompt = Some(EditSessionRequest {
                plugin: plugin.clone(),
                session: request_session(request),
                edit,
                deny_all: false,
            });
            self.dialog = Some(Dialog::PluginEditSession);
        }
    }

    /// `request/cancel`: the plugin no longer waits for the answer to its
    /// request `id` (its client gave up, say). A request still held for the
    /// user is dropped and answered with `-32800`, and the prompt it waits
    /// on is dropped, not just hidden, so a late answer does nothing and it
    /// does not come back. When other requests still wait, the next frame
    /// asks about the first of them. A request already answered, or a save
    /// dialog already open, is not affected.
    pub(super) fn withdraw_request(&mut self, plugin: &str, id: &Id) {
        let mut withdrawn = Vec::new();
        // A direct edit waiting for its session's answer.
        if let Some(index) = (self.plugins.edit_held.iter())
            .position(|(owner, request)| owner == plugin && &request.id == id)
        {
            let (_, request) = self.plugins.edit_held.remove(index);
            let session = request_session(&request);
            let asking = (self.plugins.edit_prompt.as_ref())
                .is_some_and(|prompt| prompt.plugin == plugin && prompt.session == session);
            if asking {
                self.plugins.edit_prompt = None;
                if self.dialog == Some(Dialog::PluginEditSession) {
                    self.dialog = None;
                }
            }
            withdrawn.push(request.id);
        }
        // An export waiting for the send prompt.
        if let Some(index) = (self.plugins.held.iter())
            .position(|(owner, request)| owner == plugin && &request.id == id)
        {
            let (_, request) = self.plugins.held.remove(index);
            let asking = (self.plugins.consent.as_ref())
                .is_some_and(|consent| consent.plugin == plugin && consent.action.is_none());
            if asking {
                self.plugins.consent = None;
                if self.dialog == Some(Dialog::PluginConsent) {
                    self.dialog = None;
                }
            }
            withdrawn.push(request.id);
        }
        // A file request waiting for its turn, or showing its prompt.
        if let Some(index) = (self.plugins.file_requests.iter())
            .position(|request| request.plugin == plugin && &request.id == id)
            && let Some(request) = self.plugins.file_requests.remove(index)
        {
            withdrawn.push(request.id);
        }
        if (self.plugins.file_prompt.as_ref())
            .is_some_and(|request| request.plugin == plugin && &request.id == id)
            && let Some(request) = self.plugins.file_prompt.take()
        {
            if self.dialog == Some(Dialog::PluginFile) {
                self.dialog = None;
            }
            withdrawn.push(request.id);
        }
        for id in withdrawn {
            if let Some(process) = self.plugins.process_mut(plugin) {
                let _ = process.respond(
                    id,
                    Err(RpcError::new(
                        protocol::CANCELLED,
                        "The plugin withdrew this request",
                    )),
                );
            }
        }
    }

    /// Apply the answer to the open prompt.
    pub(super) fn answer_edit_session(&mut self, answer: EditAnswer) {
        let Some(request) = self.plugins.edit_prompt.take() else {
            return;
        };
        if self.dialog == Some(Dialog::PluginEditSession) {
            self.dialog = None;
        }
        if answer == EditAnswer::Always {
            self.set_edit_auto_mode(&request.plugin, true);
        }
        if answer == EditAnswer::Deny {
            (self.plugins.edit_refused_at)
                .insert(request.plugin.clone(), std::time::Instant::now());
            if request.deny_all {
                self.plugins.edits_refused.insert(request.plugin.clone());
            }
        }
        self.plugins.edit_answers.insert(
            (request.plugin, request.session),
            answer != EditAnswer::Deny,
        );
        self.release_held_edits();
    }

    /// Turn auto mode on or off for a plugin. Turning it off also forgets the
    /// sessions already allowed, so the next edit asks.
    pub(super) fn set_edit_auto_mode(&mut self, plugin: &str, on: bool) {
        if on && !self.plugin_granted(plugin) {
            return;
        }
        if let Some(grant) = (self.config.plugins.get_mut(plugin)).and_then(|c| c.grant.as_mut())
            && grant.edit_without_asking != on
        {
            grant.edit_without_asking = on;
            self.save_config();
        }
        if !on {
            self.plugins.edit_answers.retain(|(id, _), _| id != plugin);
        }
    }

    /// `session/status`: how the plugin's edits are handled in a session.
    pub(super) fn edit_session_status(&self, plugin: &str, request: &Request) -> Value {
        let session = request_session(request);
        let asks = self.asks_before_edits(plugin);
        let edits = match self.edit_answer(plugin, &session) {
            Some(true) => "allowed",
            Some(false) => "denied",
            None => "ask",
        };
        json!({
            "edit_prompt": if asks { "session" } else { "none" },
            "edits": edits,
            "auto": asks && self.edits_without_asking(plugin),
            "save_auto": self.saves_without_asking(plugin),
        })
    }

    pub(super) fn plugin_edit_session_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.plugins.edit_prompt.clone() else {
            self.dialog = None;
            return;
        };
        if !self.plugins.running(&request.plugin) {
            self.plugins.edit_prompt = None;
            self.dialog = None;
            return;
        }
        let source = self.plugins.source(&request.plugin);
        let mut open = true;
        let mut answer = None;
        let mut deny_all = request.deny_all;
        widgets::Window::new(format!(
            "{} {source} {}",
            tr("Allow"),
            tr("to edit your documents for this session?")
        ))
        .id(("plugin_edit_session", &request.plugin))
        .default_width(460.0)
        .open(&mut open)
        .show_with_footer(ctx, |ui| {
            let lead = format!(
                "{source} {}",
                tr("asks to change your documents directly, starting with:")
            );
            ui.add(egui::Label::new(lead).wrap());
            if !request.edit.is_empty() {
                ui.label(RichText::new(format!("“{}”", request.edit)).strong());
            }
            if !request.session.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} {}", tr("Session:"), request.session))
                            .small()
                            .color(ui.palette().muted),
                    )
                    .wrap(),
                );
            }
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    RichText::new(tr(
                        "Each change is one step you can undo, and shows at once. Your answer holds until the plugin stops or starts a new session.",
                    ))
                    .small()
                    .color(ui.palette().muted),
                )
                .wrap(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(tr(
                        "Always Allow turns on auto mode: the plugin edits without asking until you turn it off in Plugins → Manage Plugins….",
                    ))
                    .small()
                    .color(ui.palette().muted),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            widgets::checkbox(
                ui,
                &mut deny_all,
                tr("With Deny: refuse every session until the plugin stops"),
            );
        }, |ui, ()| {
            let mut always = false;
            let response = widgets::dialog_footer(
                ui,
                widgets::FooterButtons::commit(tr("Allow")).cancel_label(tr("Deny")),
                |ui| {
                    always = widgets::button(ui, tr("Always Allow")).clicked();
                },
            );
            if always {
                answer = Some(EditAnswer::Always);
            } else if response.commit {
                answer = Some(EditAnswer::Allow);
            } else if response.cancel {
                answer = Some(EditAnswer::Deny);
            }
        });
        if let Some(prompt) = &mut self.plugins.edit_prompt {
            prompt.deny_all = deny_all;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
            answer = Some(EditAnswer::Deny);
        }
        if let Some(answer) = answer {
            self.answer_edit_session(answer);
        }
    }
}

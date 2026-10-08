//! Running another plugin's actions for a plugin that drives the editor (the
//! MCP server, for its clients). A plugin may start only its own actions,
//! unless the user turned on **Run other plugins' actions** in its grant
//! (`run_other_actions`). Then `plugins/actions` lists the other plugins'
//! actions, and `host/run` with `<plugin>/<action>` starts one with the
//! `inputs`, `into` and `layers` it gives:
//!
//! - The request is checked first: the setting, the plugin and action,
//!   whether the plugin is enabled and not held back by offline mode, the
//!   inputs against the manifest (strictly, see
//!   [`xuan::plugins::manifest::Action::check_inputs`]), and `into` for
//!   actions that let the user choose where the result goes.
//! - The first run of each other plugin in a session waits for **Run
//!   “Generate Image” for MCP Server?**: **Allow** (that plugin's runs in
//!   that session), **Always Allow** (`run_without_asking` in the grant) or
//!   **Cancel**, after which the plugin may not ask again for a while.
//! - Then the action starts as a surface run does, without its dialog,
//!   under the other plugin's own rules: its grant (the permission prompt
//!   when it has none), offline mode, its models (the download prompt) and
//!   the prompt before document data is sent to a plugin that declares
//!   network hosts. The request is answered once the job starts, or with why
//!   nothing runs; its result is a proposal like any other.
//!
//! See "Running other plugins' actions" in `docs/PLUGINS.md`.
use super::theme::PaletteExt as _;
use egui::RichText;
use serde_json::{Map, Value, json};
use uuid::Uuid;
use xuan::{
    i18n::tr,
    plugins::{
        edits,
        manifest::{Action, InputKind, ResultInto},
        protocol::{self, Id, Request, RpcError},
    },
};

use super::{
    Dialog, EditorApp,
    plugin_sessions::{COOLDOWN, request_session},
    plugins::{ActionEdit, PendingStart, one_line, regions_from_value},
    widgets,
};

/// A run another plugin asked for, as its open action and its job carry it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CallerRun {
    /// The plugin that asked.
    pub plugin: String,
    /// Its `host/run` request, answered once the job starts or nothing will
    /// run.
    pub request: Id,
}

/// A `host/run` of another plugin's action, checked.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PluginRun {
    /// The plugin whose action runs.
    pub plugin: String,
    pub action: String,
    /// Every input's value, regions included.
    pub values: Map<String, Value>,
    /// Where the result goes, for an action that lets the user choose.
    pub into: ResultInto,
    /// The layers to select first.
    pub layers: Option<Vec<Uuid>>,
}

/// The open prompt: a run waiting for the user's answer.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RunPrompt {
    /// The plugin that asked, its session and its request.
    pub caller: String,
    pub session: String,
    pub request: Id,
    /// The plugin whose action runs.
    pub plugin: String,
    /// The action's label, without its "…".
    pub label: String,
    /// What it runs with, one line each.
    pub details: Vec<String>,
}

/// The user's answer to the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunAnswer {
    /// Run it, and the plugin's other runs in the session.
    Allow,
    /// Run it, and stop asking: `run_without_asking`.
    Always,
    Cancel,
}

/// The plugin and action of a `host/run` request that names another
/// plugin's action.
fn other_plugins_action<'a>(caller: &str, request: &'a Request) -> Option<(&'a str, &'a str)> {
    if request.method != "host/run" {
        return None;
    }
    let action = request.params.get("action")?.as_str()?;
    let (plugin, id) = action.split_once('/')?;
    (plugin != caller).then_some((plugin, id))
}

/// Where a result goes, in words.
fn result_place(into: ResultInto) -> &'static str {
    match into {
        ResultInto::Layer | ResultInto::Ask => "in a new layer",
        ResultInto::Replace => "in place of the source layer's pixels",
        ResultInto::Document => "in a new document",
    }
}

impl EditorApp {
    /// Whether the plugin's grant lets it list and run other plugins'
    /// actions.
    pub(super) fn runs_other_actions(&self, plugin: &str) -> bool {
        self.plugin_granted(plugin)
            && self
                .stored_grant(plugin)
                .is_some_and(|grant| grant.run_other_actions)
    }

    /// Whether its runs of other plugins' actions skip the prompt.
    pub(super) fn runs_without_asking(&self, plugin: &str) -> bool {
        self.runs_other_actions(plugin)
            && self
                .stored_grant(plugin)
                .is_some_and(|grant| grant.run_without_asking)
    }

    /// Turn "Run other plugins' actions" on or off. Turning it off also
    /// turns off running them without asking and forgets the sessions
    /// already allowed.
    pub(super) fn set_run_other_actions(&mut self, plugin: &str, on: bool) {
        if on && !self.plugin_granted(plugin) {
            return;
        }
        if let Some(grant) = (self.config.plugins.get_mut(plugin)).and_then(|c| c.grant.as_mut())
            && (grant.run_other_actions != on || (!on && grant.run_without_asking))
        {
            grant.run_other_actions = on;
            grant.run_without_asking &= on;
            self.save_config();
        }
        if !on {
            self.plugins
                .run_answers
                .retain(|(caller, ..)| caller != plugin);
        }
    }

    /// Turn running other plugins' actions without asking on or off.
    /// Turning it off also forgets the sessions already allowed, so the next
    /// run asks.
    pub(super) fn set_run_without_asking(&mut self, plugin: &str, on: bool) {
        if on && !self.runs_other_actions(plugin) {
            return;
        }
        if let Some(grant) = (self.config.plugins.get_mut(plugin)).and_then(|c| c.grant.as_mut())
            && grant.run_without_asking != on
        {
            grant.run_without_asking = on;
            self.save_config();
        }
        if !on {
            self.plugins
                .run_answers
                .retain(|(caller, ..)| caller != plugin);
        }
    }

    /// The refusal for a plugin that may not run other plugins' actions,
    /// naming the setting that allows it.
    fn needs_run_setting(&self, plugin: &str) -> RpcError {
        RpcError::new(
            protocol::INVALID_REQUEST,
            format!(
                "{} may not run other plugins' actions. The user can turn on “Run other plugins' actions” for it in Plugins → Manage Plugins…",
                self.plugins.source(plugin)
            ),
        )
    }

    /// `plugins/actions`: the actions of the installed, enabled plugins
    /// other than `caller`, with their declared inputs.
    pub(super) fn list_plugin_actions(&self, caller: &str) -> Result<Value, RpcError> {
        if !self.runs_other_actions(caller) {
            return Err(self.needs_run_setting(caller));
        }
        let actions: Vec<Value> = (self.plugins.manifests.iter())
            .filter(|m| m.plugin.id != caller && self.plugin_enabled(&m.plugin.id))
            .flat_map(|manifest| {
                let plugin = &manifest.plugin.id;
                let about = json!({
                    "plugin": plugin,
                    "plugin_name": manifest.plugin.name,
                    "network": self.plugin_uses_network(plugin),
                    "allowed": self.plugin_granted(plugin),
                    "available": !self.plugin_offline(plugin),
                });
                manifest.actions.iter().map(move |action| {
                    let mut entry = about.clone();
                    let source = if action.needs_image() {
                        json!(action.source.from)
                    } else {
                        json!("none")
                    };
                    let fields = json!({
                        "id": action.id,
                        "action": format!("{plugin}/{}", action.id),
                        "label": action.label,
                        "description": action.description,
                        "kind": action.kind,
                        "source": source,
                        "surfaces": action.surfaces,
                        "result": action.result.into,
                        "inputs": action.inputs,
                    });
                    if let (Some(entry), Value::Object(fields)) = (entry.as_object_mut(), fields) {
                        entry.extend(fields);
                    }
                    entry
                })
            })
            .collect();
        Ok(json!({"actions": actions}))
    }

    /// Check a `host/run` of another plugin's action before it waits for
    /// anything: the setting, the plugin and action, whether it can run at
    /// all, its inputs, `into` and the form of `layers`.
    pub(super) fn plugin_run(
        &self,
        caller: &str,
        request: &Request,
    ) -> Result<PluginRun, RpcError> {
        if !self.runs_other_actions(caller) {
            return Err(self.needs_run_setting(caller));
        }
        let (plugin, id) = other_plugins_action(caller, request)
            .ok_or_else(|| RpcError::invalid_params("`action` must be <plugin>/<action>"))?;
        let params = &request.params;
        let manifest = (self.plugins.manifest(plugin)).ok_or_else(|| {
            RpcError::invalid_params(format!("No plugin `{plugin}` is installed"))
        })?;
        let source = self.plugins.source(plugin);
        if !self.plugin_enabled(plugin) {
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                format!("{source} is disabled"),
            ));
        }
        let spec = manifest.action(id).ok_or_else(|| {
            RpcError::invalid_params(format!(
                "{source} has no action `{id}`; plugins/actions lists them"
            ))
        })?;
        if self.plugin_offline(plugin) {
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                format!("{source} uses the network, and plugins that use the network are disabled"),
            ));
        }
        let values = spec
            .check_inputs(params.get("inputs"))
            .map_err(RpcError::invalid_params)?;
        let label = one_line(spec.label.trim_end_matches('…'), 80);
        let into = match (spec.result.into, params.get("into")) {
            (ResultInto::Ask, None | Some(Value::Null)) => {
                return Err(RpcError::invalid_params(format!(
                    "“{label}” lets the user choose where its result goes: give `into` as \"layer\" (a new layer in the current document) or \"document\" (a new document)"
                )));
            }
            (ResultInto::Ask, Some(into)) => match into.as_str() {
                Some("layer") => ResultInto::Layer,
                Some("document") => ResultInto::Document,
                _ => {
                    return Err(RpcError::invalid_params(
                        "`into` must be \"layer\" or \"document\", as the action's dialog offers",
                    ));
                }
            },
            (into, None | Some(Value::Null)) => into,
            (into, Some(_)) => {
                return Err(RpcError::invalid_params(format!(
                    "`into` goes only with actions that let the user choose where the result goes; “{label}” puts it {}",
                    result_place(into)
                )));
            }
        };
        let layers =
            match params.get("layers") {
                None | Some(Value::Null) => None,
                Some(value) => Some(serde_json::from_value::<Vec<Uuid>>(value.clone()).map_err(
                    |_| RpcError::invalid_params("`layers` must be a list of layer ids"),
                )?),
            };
        Ok(PluginRun {
            plugin: plugin.to_owned(),
            action: id.to_owned(),
            values,
            into,
            layers,
        })
    }

    /// Whether the session may run `plugin`'s actions without asking.
    pub(super) fn run_allowed(&self, caller: &str, session: &str, plugin: &str) -> bool {
        self.runs_without_asking(caller)
            || self.plugins.run_answers.contains(&(
                caller.to_owned(),
                session.to_owned(),
                plugin.to_owned(),
            ))
    }

    /// Whether the user cancelled this plugin's run too recently for it to
    /// ask again.
    pub(super) fn run_cooling_down(&self, caller: &str) -> bool {
        (self.plugins.run_refused_at.get(caller)).is_some_and(|at| at.elapsed() < COOLDOWN)
    }

    /// Handle a `host/run` of another plugin's action, answering it when it
    /// is done with. Returns the request when it is something else.
    pub(super) fn hold_plugin_run(&mut self, caller: &str, request: Request) -> Option<Request> {
        if other_plugins_action(caller, &request).is_none() {
            return Some(request);
        }
        if let Some(result) = self.plugin_run_request(caller, &request) {
            self.respond_to_plugin(caller, request.id, result);
        }
        None
    }

    /// A `host/run` of another plugin's action: the answer when there is one
    /// now, or `None` while it waits for the user (the prompt, or the send
    /// prompt of the plugin it runs).
    pub(super) fn plugin_run_request(
        &mut self,
        caller: &str,
        request: &Request,
    ) -> Option<Result<Value, RpcError>> {
        let run = match self.plugin_run(caller, request) {
            Ok(run) => run,
            Err(error) => return Some(Err(error)),
        };
        if self.run_allowed(caller, &request_session(request), &run.plugin) {
            return self.start_plugin_run(caller, &request.id, run).transpose();
        }
        if self.run_cooling_down(caller) {
            return Some(Err(RpcError::new(
                protocol::CANCELLED,
                "The user cancelled this plugin's last run just now; it may ask again later",
            )));
        }
        // One at a time, so runs that may cost money cannot stack up behind
        // one answer.
        if self.plugins.run_held.iter().any(|(id, _)| id == caller) {
            return Some(Err(RpcError::new(
                protocol::INVALID_REQUEST,
                "Another run of this plugin is waiting for the user's answer",
            )));
        }
        self.plugins
            .run_held
            .push((caller.to_owned(), request.clone()));
        None
    }

    /// Start an allowed run under the other plugin's own rules, as its menu
    /// item would: `Ok(Some(answer))` once its job runs, `Ok(None)` while
    /// its send prompt asks the user (the request is answered then), or why
    /// nothing runs. Nothing is reported to the user but the prompts a menu
    /// run shows; the plugin that asked gets the reason.
    pub(super) fn start_plugin_run(
        &mut self,
        caller: &str,
        request: &Id,
        run: PluginRun,
    ) -> Result<Option<Value>, RpcError> {
        if self.dialog.is_some()
            || self.job.is_some()
            || self.develop.is_some()
            || self.plugins.action.is_some()
            || self.plugins.run_waiting.is_some()
        {
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                "The editor is busy",
            ));
        }
        let Some(spec) = (self.plugins.manifest(&run.plugin))
            .and_then(|m| m.action(&run.action))
            .cloned()
        else {
            return Err(RpcError::invalid_params("No such action"));
        };
        let source = self.plugins.source(&run.plugin);
        if self.plugin_offline(&run.plugin) {
            self.status = format!(
                "{source} {}",
                tr("uses the network, and plugins that use the network are disabled")
            );
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                format!("{source} uses the network, and plugins that use the network are disabled"),
            ));
        }
        // The permission prompt its menu item shows. Nothing resumes after
        // it: the plugin that asked runs the action again.
        if !self.plugin_granted(&run.plugin) {
            self.plugins.permission_request =
                Some((run.plugin.clone(), PendingStart::Action(String::new())));
            self.dialog = Some(Dialog::PluginPermissions);
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                format!(
                    "{source} has not been allowed to run yet. Xuan now asks the user to allow it; run the action again once they have"
                ),
            ));
        }
        // The layers to select first, as clicking them would; they stay
        // selected only if the action starts or asks to send.
        let selected = self
            .session()
            .map(|s| (s.document.active, s.document.selected.clone()));
        if let Some(layers) = &run.layers {
            let session = self
                .session_mut()
                .ok_or_else(|| RpcError::new(protocol::INVALID_PARAMS, "No document is open"))?;
            edits::select_layers(&mut session.document, layers)
                .map_err(|e| RpcError::invalid_params(format!("{e:#}")))?;
            self.mask_target = false;
        }
        let result = self.start_checked_run(caller, request, &spec, run);
        if result.is_err()
            && let (Some(session), Some((active, selected))) = (self.session_mut(), selected)
        {
            session.document.active = active;
            session.document.selected = selected;
        }
        result
    }

    /// The rest of [`EditorApp::start_plugin_run`], once the layers are
    /// selected.
    fn start_checked_run(
        &mut self,
        caller: &str,
        request: &Id,
        spec: &Action,
        run: PluginRun,
    ) -> Result<Option<Value>, RpcError> {
        if let Some(problem) = self.action_start_problem(spec) {
            return Err(RpcError::new(protocol::INVALID_REQUEST, problem));
        }
        if run.into == ResultInto::Layer
            && spec.result.into == ResultInto::Ask
            && self.session().is_none()
        {
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                "No document is open for a new layer; give `into` as \"document\"",
            ));
        }
        // Its models, after the download prompt a menu run shows. As for a
        // menu run, the action's dialog opens with these inputs once they
        // are ready, for the user to run.
        let inputs = Value::Object(run.values.clone());
        if !self.action_models_ready(&run.plugin, &run.action, Some(&inputs)) {
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                format!(
                    "{} needs its models first. Xuan is asking the user to download them, or checking them; run the action again once they are ready",
                    self.plugins.source(&run.plugin)
                ),
            ));
        }
        let mut values = run.values;
        let regions = (spec.regions_input())
            .and_then(|input| values.remove(&input.id))
            .map(|value| regions_from_value(&value))
            .unwrap_or_default();
        self.plugins.action = Some(ActionEdit {
            plugin: run.plugin,
            action: run.action,
            values,
            regions,
            selected: None,
            estimate: None,
            previous_tool: self.tool,
            into: if spec.result.into == ResultInto::Ask {
                run.into
            } else {
                ResultInto::Layer
            },
            consented: false,
            provider: None,
            surface: None,
            caller: Some(CallerRun {
                plugin: caller.to_owned(),
                request: request.clone(),
            }),
        });
        self.run_for_caller()
    }

    /// Run the open action another plugin asked for: `Ok(Some(answer))`
    /// once its job runs, `Ok(None)` while the send prompt asks the user, or
    /// why it did not start. Its failures go to the plugin that asked, not
    /// to the user; a run that did not start leaves nothing open.
    fn run_for_caller(&mut self) -> Result<Option<Value>, RpcError> {
        let Some(caller) = (self.plugins.action.as_ref()).and_then(|edit| edit.caller.clone())
        else {
            return Err(RpcError::new(protocol::INTERNAL_ERROR, "No run is open"));
        };
        let jobs = self.plugins.jobs.len();
        let shown = self.error.take();
        self.run_plugin_action();
        let failure = std::mem::replace(&mut self.error, shown);
        if self.plugins.jobs.len() > jobs
            && let Some(job) = self.plugins.jobs.last()
        {
            let label = (self.plugins.manifest(&job.plugin))
                .and_then(|m| m.action(&job.action))
                .map_or_else(|| job.label.clone(), |spec| spec.label.clone());
            let answer = json!({
                "ok": true,
                "running": true,
                "job": job.id,
                "plugin": job.plugin,
                "plugin_name": (self.plugins.manifest(&job.plugin)).map(|m| m.plugin.name.clone()),
                "action": job.action,
                "label": label,
            });
            // The plugin's log keeps what it started, with the other plugin.
            let note = format!(
                "{} “{}” · {}",
                tr("Started"),
                one_line(&label, 80),
                self.plugins.source(&job.plugin)
            );
            if let Some(process) = self.plugins.process_mut(&caller.plugin) {
                process.note(note);
            }
            return Ok(Some(answer));
        }
        let open = (self.plugins.action.as_ref())
            .is_some_and(|edit| edit.caller.as_ref() == Some(&caller));
        if open && self.dialog == Some(Dialog::PluginConsent) {
            self.plugins.run_waiting = Some(caller);
            return Ok(None);
        }
        if open {
            self.plugins.action = None;
        }
        Err(RpcError::new(
            protocol::INTERNAL_ERROR,
            failure.unwrap_or_else(|| "The action did not start".into()),
        ))
    }

    /// The answer to the send prompt of a run another plugin asked for:
    /// **Send** starts it and answers with its job, **Cancel** answers that
    /// nothing was sent.
    pub(super) fn answer_run_consent(&mut self, send: bool) {
        let Some(caller) = (self.plugins.action.as_ref()).and_then(|edit| edit.caller.clone())
        else {
            return;
        };
        if self.plugins.run_waiting.as_ref() == Some(&caller) {
            self.plugins.run_waiting = None;
        }
        let source = (self.plugins.action.as_ref())
            .map(|edit| self.plugins.source(&edit.plugin))
            .unwrap_or_default();
        let result = if send {
            if let Some(edit) = &mut self.plugins.action {
                edit.consented = true;
            }
            self.run_for_caller().and_then(|answer| {
                answer.ok_or_else(|| {
                    RpcError::new(protocol::INTERNAL_ERROR, "The action did not start")
                })
            })
        } else {
            self.status = tr("Cancelled; nothing was sent").into();
            self.close_plugin_action();
            Err(RpcError::new(
                protocol::CANCELLED,
                format!(
                    "The user did not allow sending document data to {source}; nothing was run"
                ),
            ))
        };
        self.respond_to_plugin(&caller.plugin, caller.request, result);
    }

    /// `request/cancel` for a run: one waiting for the prompt, whose prompt
    /// is dropped, or one waiting for the send prompt of the plugin it runs,
    /// which is closed with its action. Returns the request's id when there
    /// was one.
    pub(super) fn withdraw_plugin_run(&mut self, plugin: &str, id: &Id) -> Option<Id> {
        if let Some(index) = (self.plugins.run_held.iter())
            .position(|(caller, request)| caller == plugin && &request.id == id)
        {
            let (_, request) = self.plugins.run_held.remove(index);
            let asking = (self.plugins.run_prompt.as_ref())
                .is_some_and(|prompt| prompt.caller == plugin && &prompt.request == id);
            if asking {
                self.plugins.run_prompt = None;
                if self.dialog == Some(Dialog::PluginRun) {
                    self.dialog = None;
                }
            }
            return Some(request.id);
        }
        let waiting = (self.plugins.run_waiting.as_ref())
            .is_some_and(|run| run.plugin == plugin && &run.request == id);
        if !waiting {
            return None;
        }
        self.plugins.run_waiting = None;
        self.close_caller_run();
        Some(id.clone())
    }

    /// Close the open run another plugin asked for, with its send prompt.
    fn close_caller_run(&mut self) {
        if !(self.plugins.action.as_ref()).is_some_and(|edit| edit.caller.is_some()) {
            return;
        }
        if (self.plugins.consent.as_ref()).is_some_and(|consent| consent.action.is_some()) {
            self.plugins.consent = None;
            if self.dialog == Some(Dialog::PluginConsent) {
                self.dialog = None;
            }
        }
        self.close_plugin_action();
    }

    /// Start the runs whose session was answered, answer a run whose send
    /// prompt went away, and ask about the next run when nothing else is
    /// open. Called once per frame.
    pub(super) fn release_held_runs(&mut self) {
        // A run waiting for its send prompt whose action was closed, whose
        // prompt is gone, or whose caller stopped will not start.
        if let Some(waiting) = self.plugins.run_waiting.clone() {
            let open = (self.plugins.action.as_ref())
                .is_some_and(|edit| edit.caller.as_ref() == Some(&waiting));
            if !open || self.plugins.consent.is_none() || !self.plugins.running(&waiting.plugin) {
                self.plugins.run_waiting = None;
                if open {
                    self.close_caller_run();
                }
                self.respond_to_plugin(
                    &waiting.plugin,
                    waiting.request,
                    Err(RpcError::new(
                        protocol::CANCELLED,
                        "The run was closed before it started; nothing was run",
                    )),
                );
            }
        }
        let held = std::mem::take(&mut self.plugins.run_held);
        let mut waiting = Vec::new();
        for (caller, request) in held {
            if !self.plugins.running(&caller) {
                continue;
            }
            // Checked again: the setting, the plugin or its inputs' spec may
            // have changed while it waited.
            let run = match self.plugin_run(&caller, &request) {
                Ok(run) => run,
                Err(error) => {
                    self.respond_to_plugin(&caller, request.id, Err(error));
                    continue;
                }
            };
            if self.run_allowed(&caller, &request_session(&request), &run.plugin) {
                if let Some(result) = self.start_plugin_run(&caller, &request.id, run).transpose() {
                    self.respond_to_plugin(&caller, request.id, result);
                }
                continue;
            }
            if self.run_cooling_down(&caller) {
                self.respond_to_plugin(
                    &caller,
                    request.id,
                    Err(RpcError::new(
                        protocol::CANCELLED,
                        "The user cancelled this plugin's last run just now; it may ask again later",
                    )),
                );
                continue;
            }
            waiting.push((caller, request));
        }
        // Runs that arrived while these started wait behind them.
        waiting.append(&mut self.plugins.run_held);
        self.plugins.run_held = waiting;
        // Another dialog may have replaced the open prompt (a menu command,
        // opening a document): its run still waits, so show it again.
        if let Some(prompt) = &self.plugins.run_prompt
            && self.dialog.is_none()
        {
            let held = (self.plugins.run_held.iter())
                .any(|(caller, request)| caller == &prompt.caller && request.id == prompt.request);
            if held {
                self.dialog = Some(Dialog::PluginRun);
            } else {
                self.plugins.run_prompt = None;
            }
        }
        // A run can start only when the editor is free, so it asks then.
        if self.plugins.run_prompt.is_none()
            && self.dialog.is_none()
            && self.job.is_none()
            && self.develop.is_none()
            && self.plugins.action.is_none()
            && let Some((caller, request)) = self.plugins.run_held.first().cloned()
            && let Ok(run) = self.plugin_run(&caller, &request)
        {
            self.plugins.run_prompt = Some(self.prompt_for_run(&caller, &request, &run));
            self.dialog = Some(Dialog::PluginRun);
        }
    }

    /// The prompt for a held run: the action, and what it runs with.
    fn prompt_for_run(&self, caller: &str, request: &Request, run: &PluginRun) -> RunPrompt {
        let spec = (self.plugins.manifest(&run.plugin)).and_then(|m| m.action(&run.action));
        let label = spec.map_or_else(
            || run.action.clone(),
            |spec| one_line(spec.label.trim_end_matches('…'), 80),
        );
        let mut details = Vec::new();
        let mut options = Vec::new();
        for input in spec.map(|spec| spec.inputs.as_slice()).unwrap_or_default() {
            let value = run.values.get(&input.id);
            let name = one_line(input.label(), 80);
            match input.kind {
                InputKind::Text | InputKind::Multiline => {
                    if let Some(text) = super::plugin_consent::text_value(input.kind, value) {
                        details.push(format!("{name}: {text}"));
                    }
                }
                InputKind::Regions => {
                    let count = value.and_then(Value::as_array).map_or(0, Vec::len);
                    if count > 0 {
                        details.push(format!("{} {count}", tr("Regions:")));
                    }
                }
                InputKind::Path | InputKind::Secret => {}
                InputKind::Enum => {
                    let id = value.and_then(Value::as_str).unwrap_or_default();
                    let choice = (input.values.iter())
                        .find(|choice| choice.id == id)
                        .map_or(id, |choice| choice.label.as_str());
                    options.push(format!("{name}: {}", one_line(choice, 80)));
                }
                _ => {
                    if let Some(value) = value {
                        options.push(format!("{name}: {}", one_line(&value.to_string(), 40)));
                    }
                }
            }
        }
        if !options.is_empty() {
            details.push(options.join(", "));
        }
        if spec.is_some_and(|spec| spec.result.into == ResultInto::Ask) {
            details.push(match run.into {
                ResultInto::Document => tr("Result: a new document").to_owned(),
                _ => tr("Result: a new layer").to_owned(),
            });
        }
        if let Some(layers) = &run.layers
            && let Some(session) = self.session()
        {
            let names: Vec<String> = (layers.iter())
                .filter_map(|id| session.document.layers.iter().find(|l| l.id == *id))
                .map(|layer| format!("“{}”", one_line(&layer.name, 80)))
                .collect();
            if !names.is_empty() {
                details.push(format!("{} {}", tr("On the layers:"), names.join(", ")));
            }
        }
        RunPrompt {
            caller: caller.to_owned(),
            session: request_session(request),
            request: request.id.clone(),
            plugin: run.plugin.clone(),
            label,
            details,
        }
    }

    /// Apply the answer to the open prompt. A run whose request was
    /// withdrawn took its prompt with it, so a late answer does nothing.
    pub(super) fn answer_plugin_run(&mut self, answer: RunAnswer) {
        if self.dialog == Some(Dialog::PluginRun) {
            self.dialog = None;
        }
        let Some(prompt) = self.plugins.run_prompt.take() else {
            return;
        };
        match answer {
            RunAnswer::Cancel => {
                (self.plugins.run_refused_at)
                    .insert(prompt.caller.clone(), std::time::Instant::now());
                if let Some(index) = (self.plugins.run_held.iter()).position(|(caller, request)| {
                    caller == &prompt.caller && request.id == prompt.request
                }) {
                    let (_, request) = self.plugins.run_held.remove(index);
                    let source = self.plugins.source(&prompt.plugin);
                    self.respond_to_plugin(
                        &prompt.caller,
                        request.id,
                        Err(RpcError::new(
                            protocol::CANCELLED,
                            format!(
                                "The user did not allow running “{}” of {source}; nothing was run",
                                prompt.label
                            ),
                        )),
                    );
                }
            }
            RunAnswer::Allow | RunAnswer::Always => {
                if answer == RunAnswer::Always {
                    self.set_run_without_asking(&prompt.caller, true);
                }
                (self.plugins.run_answers).insert((prompt.caller, prompt.session, prompt.plugin));
            }
        }
        self.release_held_runs();
    }

    pub(super) fn plugin_run_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.plugins.run_prompt.clone() else {
            self.dialog = None;
            return;
        };
        if !self.plugins.running(&prompt.caller) {
            self.plugins.run_prompt = None;
            self.dialog = None;
            return;
        }
        let caller = self.plugins.source(&prompt.caller);
        let target = self.plugins.source(&prompt.plugin);
        let hosts = (self.plugins.manifest(&prompt.plugin))
            .map(|m| {
                (m.permissions.network.iter())
                    .map(|host| one_line(host, 120))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let mut open = true;
        let mut answer = None;
        widgets::Window::new(format!(
            "{} “{}” {} {caller}?",
            tr("Run"),
            prompt.label,
            tr("for")
        ))
        .id(("plugin_run", &prompt.caller))
        .default_width(460.0)
        .open(&mut open)
        .show_with_footer(ctx, |ui| {
            let lead = format!(
                "{caller} {}",
                tr("asks to run another plugin's action for its client:")
            );
            ui.add(egui::Label::new(lead).wrap());
            ui.add(
                egui::Label::new(RichText::new(format!("“{}” · {target}", prompt.label)).strong())
                    .wrap(),
            );
            for detail in &prompt.details {
                ui.add(egui::Label::new(format!("• {detail}")).wrap());
            }
            if !prompt.session.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} {}", tr("Session:"), prompt.session))
                            .small()
                            .color(ui.palette().muted),
                    )
                    .wrap(),
                );
            }
            ui.add_space(8.0);
            if !hosts.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{target} {} {hosts}. {}",
                            tr("says it connects to:"),
                            tr("A run can send document data there and may use paid credits.")
                        ))
                        .color(ui.palette().warning),
                    )
                    .wrap(),
                );
            }
            ui.add(
                egui::Label::new(
                    RichText::new(tr(
                        "It runs as if you chose it from the menu: the other plugin's own prompts still apply, and its result is a proposal you accept or discard. Allow holds for this session's runs of that plugin until the asking plugin stops.",
                    ))
                    .small()
                    .color(ui.palette().muted),
                )
                .wrap(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(tr(
                        "Always Allow lets the plugin run other plugins' actions without this prompt until you turn it off in Plugins → Manage Plugins….",
                    ))
                    .small()
                    .color(ui.palette().muted),
                )
                .wrap(),
            );
        }, |ui, ()| {
            let mut always = false;
            let response =
                widgets::dialog_footer(ui, widgets::FooterButtons::commit(tr("Allow")), |ui| {
                    always = widgets::button(ui, tr("Always Allow")).clicked();
                });
            if always {
                answer = Some(RunAnswer::Always);
            } else if response.commit {
                answer = Some(RunAnswer::Allow);
            } else if response.cancel {
                answer = Some(RunAnswer::Cancel);
            }
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
            answer = Some(RunAnswer::Cancel);
        }
        if let Some(answer) = answer {
            self.answer_plugin_run(answer);
        }
    }
}

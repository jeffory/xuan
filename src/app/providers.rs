//! Providers: a plugin that stands in for a built-in algorithm (Select Subject,
//! Remove Background, the Magic tool's Object mode), chosen in Settings → Selection.
//!
//! The built-in algorithms are classical (see `xuan::segment`); machine-learning
//! models live in plugins, which declare `[[provides]]` in their manifest. When the
//! user picks one, the command runs that plugin action with the usual plugin rules
//! (grant, send consent, offline mode, model downloads) and its `mask` output comes
//! back as a proposal: the selection for Select Subject and Object mode, and the source
//! layer's mask for Remove Background. A provider that cannot run (not installed,
//! disabled, or using the network in offline mode) falls back to the built-in
//! algorithm with a notice.
use uuid::Uuid;
use xuan::{
    document::Point,
    i18n::tr,
    plugins::manifest::{Capability, DocumentAccess},
    selection::SelectionMode,
};

use super::EditorApp;

/// A choice in Settings → Selection: the plugin id (`None` for built-in) and its label.
pub(super) type Choice = (Option<String>, String);

/// A command handed to a provider plugin, kept with the action until its result is
/// applied.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ProviderRun {
    pub capability: Capability,
    pub plugin: String,
    pub action: String,
    pub document: Uuid,
    /// Remove Background: the layer that gets the mask.
    pub layer: Option<Uuid>,
    /// Object mode: the click, in document pixels.
    pub point: Option<Point>,
    /// Object mode: the dragged box, `[left, top, right, bottom]` in document pixels.
    pub rect: Option<[f32; 4]>,
    /// How the result combines with the selection.
    pub mode: SelectionMode,
}

impl EditorApp {
    /// The plugin and action chosen for `capability`, if it can run now. A chosen
    /// plugin that cannot leaves a notice in the status bar, and the caller uses the
    /// built-in algorithm.
    pub(super) fn provider(&mut self, capability: Capability) -> Option<(String, String)> {
        let plugin = self.config.providers.get(capability)?.to_owned();
        let action = (self.plugins.manifest(&plugin))
            .and_then(|m| m.provider(capability))
            .map(|a| a.id.clone());
        let usable = action.is_some()
            && self.plugin_enabled(&plugin)
            && !self.plugin_offline(&plugin)
            && (capability != Capability::RemoveBackground
                || self
                    .plugins
                    .manifest(&plugin)
                    .is_some_and(|m| m.permissions.document == DocumentAccess::Edit));
        if !usable {
            let why = if action.is_none() {
                tr("is not installed or no longer provides it")
            } else if self.plugin_offline(&plugin) {
                tr("uses the network, and plugins that use the network are disabled")
            } else {
                tr("is disabled")
            };
            let source = if self.plugins.manifest(&plugin).is_some() {
                self.plugins.source(&plugin)
            } else {
                plugin.clone()
            };
            self.notice_provider_fallback(&format!(
                "{source} {why}; {} {}",
                tr("used the built-in"),
                tr(capability.label())
            ));
            return None;
        }
        Some((plugin, action.unwrap()))
    }

    fn notice_provider_fallback(&mut self, message: &str) {
        self.status = message.to_owned();
        self.provider_notice = Some(message.to_owned());
    }

    /// Runs `capability` through its provider plugin. Returns false when the built-in
    /// algorithm should run instead.
    pub(super) fn run_provider(
        &mut self,
        capability: Capability,
        point: Option<Point>,
        rect: Option<[f32; 4]>,
        mode: SelectionMode,
    ) -> bool {
        self.provider_notice = None;
        let Some((plugin, action)) = self.provider(capability) else {
            return false;
        };
        let Some(session) = self.session() else {
            return true;
        };
        let run = ProviderRun {
            capability,
            plugin: plugin.clone(),
            action: action.clone(),
            document: session.document.id,
            layer: session.document.active,
            point,
            rect,
            mode,
        };
        // Kept until the action starts: a permission prompt or a model download
        // may come first, and the action resumes as this provider run.
        self.plugins.provider_pending = Some(run);
        self.start_plugin_action_with(&plugin, &action, None);
        true
    }

    /// The choices for `capability` in Settings: the built-in algorithm and every
    /// installed plugin that provides it, as `(plugin id, label)`.
    pub(super) fn provider_choices(&self, capability: Capability) -> Vec<Choice> {
        let mut choices = vec![(None, tr("Built-in").to_owned())];
        for manifest in &self.plugins.manifests {
            if manifest.provider(capability).is_some() {
                choices.push((
                    Some(manifest.plugin.id.clone()),
                    format!("{} ({})", manifest.plugin.name, manifest.plugin.id),
                ));
            }
        }
        choices
    }
}

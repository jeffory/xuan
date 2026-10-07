//! Plugin actions started from Xuan's own UI (the Layers panel, the AI
//! Region tool and New Image) instead of their dialog.
use serde_json::{Map, Value};
use xuan::{
    i18n::tr,
    plugins::{
        jobs::Region,
        manifest::{InputKind, ResultInto, Surface},
    },
};

use super::{
    Dialog, EditorApp,
    plugins::{ActionEdit, PendingStart},
};

/// How a surface started an action.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SurfaceRun {
    pub surface: Surface,
    /// Document pixels the result will cover.
    pub target: (u32, u32),
    /// New Image: make the canvas exactly `target`.
    pub exact: bool,
    /// New Image: the new document's resolution.
    pub resolution: f32,
}

impl EditorApp {
    /// Run `action` from a surface with the values the surface collected
    /// (missing ones take their defaults) and, for the region surface, its
    /// boxes. The usual prompts apply: permission, send consent and model
    /// downloads. Returns false when nothing will happen, with the reason in
    /// the status bar.
    #[allow(dead_code)] // the surfaces that call it follow
    pub(super) fn run_from_surface(
        &mut self,
        plugin: &str,
        action: &str,
        given: &Map<String, Value>,
        regions: Vec<Region>,
        run: SurfaceRun,
    ) -> bool {
        let Some(spec) = (self.plugins.manifest(plugin))
            .and_then(|m| m.action(action))
            .cloned()
        else {
            return false;
        };
        if !self.plugin_enabled(plugin) {
            self.status = format!("{} {}", self.plugins.source(plugin), tr("is disabled"));
            return false;
        }
        if self.plugin_offline(plugin) {
            self.status = format!(
                "{} {}",
                self.plugins.source(plugin),
                tr("uses the network, and plugins that use the network are disabled.")
            );
            return false;
        }
        if !self.plugin_granted(plugin) {
            // Nothing to resume after the grant: the user generates again.
            self.plugins.permission_request =
                Some((plugin.into(), PendingStart::Action(String::new())));
            self.dialog = Some(Dialog::PluginPermissions);
            return true;
        }
        let mut values = Map::new();
        for input in spec.inputs.iter().filter(|i| i.kind != InputKind::Regions) {
            let value = given
                .get(&input.id)
                .map_or_else(|| input.initial(), |v| input.coerce(v));
            values.insert(input.id.clone(), value);
        }
        if !self.action_models_ready(plugin, action, Some(&Value::Object(values.clone()))) {
            return true;
        }
        let into = if run.surface == Surface::Document {
            ResultInto::Document
        } else {
            ResultInto::Layer
        };
        self.plugins.action = Some(ActionEdit {
            plugin: plugin.into(),
            action: action.into(),
            values,
            regions,
            selected: None,
            estimate: None,
            previous_tool: self.tool,
            into,
            consented: false,
            provider: None,
            surface: Some(run),
        });
        self.run_plugin_action();
        true
    }
}

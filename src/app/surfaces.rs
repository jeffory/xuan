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
    plugin_dialogs::input_widget,
    plugins::{ActionEdit, PendingStart},
    theme::{self, PaletteExt as _},
    widgets,
};

/// A popover a surface opened.
pub(super) enum SurfacePopup {
    /// From the New layer with AI button, anchored above it.
    Layer {
        plugin: String,
        action: String,
        anchor: egui::Pos2,
    },
    /// An AI Region box's popover.
    Region { document: uuid::Uuid, index: usize },
}

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

    /// Enabled plugins' actions on `surface`: (plugin, action, label
    /// without its "…"), in manifest order.
    pub(super) fn surface_actions(&self, surface: Surface) -> Vec<(String, String, String)> {
        (self.plugins.manifests.iter())
            .filter(|m| self.plugin_enabled(&m.plugin.id))
            .flat_map(|m| {
                (m.actions.iter().filter(move |a| a.on(surface))).map(move |a| {
                    let label = a.label.trim_end_matches('…').to_owned();
                    (m.plugin.id.clone(), a.id.clone(), label)
                })
            })
            .collect()
    }

    /// The values a surface shows for an action: last used this session,
    /// else its defaults.
    pub(super) fn surface_values(&mut self, plugin: &str, action: &str) -> &mut Map<String, Value> {
        let spec = (self.plugins.manifest(plugin))
            .and_then(|m| m.action(action))
            .cloned();
        (self.plugins.surface_values)
            .entry((plugin.into(), action.into()))
            .or_insert_with(|| {
                spec.map(|spec| {
                    (spec.inputs.iter())
                        .filter(|i| i.kind != InputKind::Regions)
                        .map(|i| (i.id.clone(), i.initial()))
                        .collect()
                })
                .unwrap_or_default()
            })
    }

    /// An action's inputs for `surface`: the basic ones, then the advanced
    /// ones in a collapsed "Advanced" section. Returns whether the first
    /// basic text input has text, which Generate needs.
    pub(super) fn surface_form(
        &mut self,
        ui: &mut egui::Ui,
        plugin: &str,
        action: &str,
        surface: Surface,
        salt: &str,
    ) -> bool {
        let Some(spec) = (self.plugins.manifest(plugin))
            .and_then(|m| m.action(action))
            .cloned()
        else {
            return false;
        };
        let shown: Vec<_> = (spec.inputs.iter())
            .filter(|i| i.kind != InputKind::Regions && i.shown_on(surface))
            .cloned()
            .collect();
        let values = self.surface_values(plugin, action);
        let mut ready = true;
        let mut first_text = true;
        for input in shown.iter().filter(|i| !i.advanced) {
            let value = values
                .entry(input.id.clone())
                .or_insert_with(|| input.initial());
            input_widget(ui, input, value, (salt, "basic"));
            if first_text && matches!(input.kind, InputKind::Text | InputKind::Multiline) {
                ready = value.as_str().is_some_and(|text| !text.trim().is_empty());
                first_text = false;
            }
        }
        let advanced: Vec<_> = shown.iter().filter(|i| i.advanced).collect();
        if !advanced.is_empty() {
            egui::CollapsingHeader::new(tr("Advanced"))
                .id_salt((salt, "advanced"))
                .default_open(false)
                .show(ui, |ui| {
                    for input in advanced {
                        let value = values
                            .entry(input.id.clone())
                            .or_insert_with(|| input.initial());
                        input_widget(ui, input, value, (salt, "advanced"));
                    }
                });
        }
        ready
    }

    /// Open a surface popover, replacing any other.
    pub(super) fn open_surface_popup(&mut self, popup: SurfacePopup) {
        self.surface_popup = Some(popup);
        self.surface_popup_fresh = true;
    }

    /// Draw the open surface popover, if any.
    pub(super) fn surface_popups(&mut self, ctx: &egui::Context) {
        // A box's popover belongs to its document.
        if let Some(SurfacePopup::Region { document, .. }) = &self.surface_popup
            && self.session().map(|s| s.document.id) != Some(*document)
        {
            self.surface_popup = None;
        }
        match &self.surface_popup {
            Some(SurfacePopup::Layer { .. }) => self.layer_popup(ctx),
            Some(SurfacePopup::Region { .. }) => self.region_popup(ctx),
            None => {}
        }
    }

    /// New layer with AI: the action's label, its form and Generate, above
    /// the button that opened it.
    fn layer_popup(&mut self, ctx: &egui::Context) {
        let Some(SurfacePopup::Layer {
            plugin,
            action,
            anchor,
        }) = &self.surface_popup
        else {
            return;
        };
        let (plugin, action, anchor) = (plugin.clone(), action.clone(), *anchor);
        let label = (self.surface_actions(Surface::Layer).into_iter())
            .find(|(p, a, _)| *p == plugin && *a == action)
            .map(|(_, _, label)| label);
        let Some(label) = label else {
            self.surface_popup = None;
            return;
        };
        let mut generate = false;
        let response = egui::Area::new(egui::Id::new(("surface_popup", &plugin, &action)))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(anchor)
            .constrain(true)
            .show(ctx, |ui| {
                theme::frame(&ctx.palette())
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.label(egui::RichText::new(&label).strong());
                        ui.add_space(4.0);
                        let ready =
                            self.surface_form(ui, &plugin, &action, Surface::Layer, "layer_popup");
                        ui.add_space(4.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let button = ui
                                .add_enabled(ready, widgets::Button::new(tr("Generate")).primary());
                            let shortcut = ui
                                .input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
                            generate = button.clicked() || (ready && shortcut);
                        });
                    });
            });
        let fresh = std::mem::take(&mut self.surface_popup_fresh);
        let close = ctx.input(|i| i.key_pressed(egui::Key::Escape))
            || (!fresh && response.response.clicked_elsewhere());
        if generate {
            let values = self.surface_values(&plugin, &action).clone();
            let target = self
                .session()
                .map_or((1, 1), |s| (s.document.width, s.document.height));
            self.surface_popup = None;
            self.run_from_surface(
                &plugin,
                &action,
                &values,
                Vec::new(),
                SurfaceRun {
                    surface: Surface::Layer,
                    target,
                    exact: false,
                    resolution: 72.0,
                },
            );
        } else if close {
            self.surface_popup = None;
        }
    }
}

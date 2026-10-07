//! The AI Region tool without an open dialog: boxes on the canvas, each with
//! the region action (its verb) and the values it will run with. Boxes belong
//! to their document for the session; they are not edits of it.
use serde_json::{Map, Value};
use xuan::{
    document::Point,
    i18n::tr,
    plugins::{
        jobs::Region,
        manifest::{InputKind, Surface},
    },
};

use super::{
    EditorApp,
    plugin_dialogs::input_widget,
    plugins::region_limit,
    surfaces::{SurfacePopup, SurfaceRun},
    theme::{self, PaletteExt as _},
    widgets,
};

fn has_text(ai_box: &AiBox) -> bool {
    (ai_box.region.fields.values()).any(|v| v.as_str().is_some_and(|t| !t.trim().is_empty()))
}

/// A box drawn with the AI Region tool.
#[derive(Clone, Debug)]
pub(super) struct AiBox {
    /// Document pixels; `fields` hold the action's region fields.
    pub region: Region,
    pub plugin: String,
    pub action: String,
    /// The action's other inputs.
    pub values: Map<String, Value>,
}

impl EditorApp {
    /// A new box with the first region action and its defaults.
    fn new_ai_box(&self, mut region: Region) -> Option<AiBox> {
        let (plugin, action, _) = self.surface_actions(Surface::Region).into_iter().next()?;
        let spec = self.plugins.manifest(&plugin)?.action(&action)?.clone();
        let regions = spec.regions_input()?;
        for field in &regions.fields {
            region.fields.insert(field.id.clone(), field.initial());
        }
        let values = (spec.inputs.iter())
            .filter(|i| i.kind != InputKind::Regions)
            .map(|i| (i.id.clone(), i.initial()))
            .collect();
        Some(AiBox {
            region,
            plugin,
            action,
            values,
        })
    }

    /// Add a box dragged from `start` to `end` (document pixels) and open
    /// its popover. Boxes under two pixels are ignored.
    pub(super) fn add_ai_box(&mut self, start: Point, end: Point) {
        let (x, y) = (start.x.min(end.x), start.y.min(end.y));
        let (width, height) = ((start.x - end.x).abs(), (start.y - end.y).abs());
        if width < 2.0 || height < 2.0 {
            return;
        }
        if let Some(ai_box) = self.new_ai_box(Region::rect(x, y, width, height)) {
            self.push_ai_box(ai_box);
        }
    }

    /// Add a box shaped like the selection, keeping its mask.
    pub(super) fn add_ai_box_from_selection(&mut self) {
        let Some(selection) = self.session().and_then(|s| s.document.selection.clone()) else {
            self.status = tr("Make a selection first").into();
            return;
        };
        if let Some(ai_box) = Region::from_mask(selection).and_then(|r| self.new_ai_box(r)) {
            self.push_ai_box(ai_box);
        }
    }

    fn push_ai_box(&mut self, ai_box: AiBox) {
        let Some(session) = self.session_mut() else {
            return;
        };
        session.ai_boxes.push(ai_box);
        let index = session.ai_boxes.len() - 1;
        session.ai_selected = Some(index);
        let document = session.document.id;
        self.open_surface_popup(SurfacePopup::Region { document, index });
    }

    /// Select the topmost box under `point` and open its popover.
    pub(super) fn select_ai_box_at(&mut self, point: Point) -> bool {
        let Some(session) = self.session_mut() else {
            return false;
        };
        let found = session
            .ai_boxes
            .iter()
            .rposition(|b| b.region.contains(point));
        session.ai_selected = found;
        let document = session.document.id;
        match found {
            Some(index) => self.open_surface_popup(SurfacePopup::Region { document, index }),
            None => self.surface_popup = None,
        }
        found.is_some()
    }

    pub(super) fn delete_ai_box(&mut self, index: usize) {
        if let Some(session) = self.session_mut()
            && index < session.ai_boxes.len()
        {
            session.ai_boxes.remove(index);
            session.ai_selected = None;
        }
        self.surface_popup = None;
    }

    pub(super) fn clear_ai_boxes(&mut self) {
        if let Some(session) = self.session_mut() {
            session.ai_boxes.clear();
            session.ai_selected = None;
        }
        self.surface_popup = None;
    }

    /// Change a box's verb: the new action's defaults, keeping the box's
    /// first text (its prompt) when the new action has a text field.
    pub(super) fn set_ai_box_action(&mut self, index: usize, plugin: &str, action: &str) {
        let Some(spec) = (self.plugins.manifest(plugin))
            .and_then(|m| m.action(action))
            .cloned()
        else {
            return;
        };
        let Some(regions) = spec.regions_input().cloned() else {
            return;
        };
        let Some(ai_box) = self.session_mut().and_then(|s| s.ai_boxes.get_mut(index)) else {
            return;
        };
        let prompt = (ai_box.region.fields.values())
            .find_map(|v| v.as_str().filter(|t| !t.is_empty()).map(str::to_owned));
        ai_box.region.fields.clear();
        for field in &regions.fields {
            ai_box
                .region
                .fields
                .insert(field.id.clone(), field.initial());
        }
        let text_field = (regions.fields.iter())
            .find(|f| matches!(f.kind, InputKind::Text | InputKind::Multiline));
        if let (Some(prompt), Some(field)) = (prompt, text_field) {
            ai_box.region.fields.insert(field.id.clone(), prompt.into());
        }
        ai_box.values = (spec.inputs.iter())
            .filter(|i| i.kind != InputKind::Regions)
            .map(|i| (i.id.clone(), i.initial()))
            .collect();
        (ai_box.plugin, ai_box.action) = (plugin.into(), action.into());
    }

    /// Whether the action of the box at `index` takes several boxes at once
    /// (Precise Edit), so Generate sends every box with that action.
    fn sends_together(&self, ai_box: &AiBox) -> bool {
        (self.plugins.manifest(&ai_box.plugin))
            .and_then(|m| m.action(&ai_box.action))
            .and_then(|a| a.regions_input())
            .is_some_and(|input| region_limit(input) > 1)
    }

    /// The boxes Generate on box `index` sends, as groups of one job each:
    /// all boxes of its action with text when that action takes several,
    /// else the box alone (when it has text).
    fn ai_box_groups(&self, index: usize) -> Vec<Vec<usize>> {
        let Some(session) = self.session() else {
            return Vec::new();
        };
        let Some(chosen) = session.ai_boxes.get(index) else {
            return Vec::new();
        };
        if !self.sends_together(chosen) {
            return if has_text(chosen) {
                vec![vec![index]]
            } else {
                Vec::new()
            };
        }
        let together: Vec<usize> = (session.ai_boxes.iter().enumerate())
            .filter(|(_, b)| b.plugin == chosen.plugin && b.action == chosen.action && has_text(b))
            .map(|(i, _)| i)
            .collect();
        if together.is_empty() {
            Vec::new()
        } else {
            vec![together]
        }
    }

    /// Run the box at `index` (with the other boxes of its action when that
    /// action takes several). Boxes without text are left; sent boxes are
    /// removed. A prompt (permission, send consent) stops it after the job
    /// waiting for it. Returns the jobs started or waiting.
    pub(super) fn generate_ai_boxes(&mut self, index: usize) -> usize {
        let groups = self.ai_box_groups(index);
        let Some(session) = self.session() else {
            return 0;
        };
        let boxes = session.ai_boxes.clone();
        let mut sent = Vec::new();
        let mut started = 0;
        for group in groups {
            let picked: Vec<&AiBox> = group.iter().map(|i| &boxes[*i]).collect();
            let (x0, y0) = (picked.iter()).fold((f32::MAX, f32::MAX), |(x, y), b| {
                (x.min(b.region.x), y.min(b.region.y))
            });
            let (x1, y1) = (picked.iter()).fold((f32::MIN, f32::MIN), |(x, y), b| {
                (
                    x.max(b.region.x + b.region.width),
                    y.max(b.region.y + b.region.height),
                )
            });
            let target = (
                ((x1 - x0).ceil() as u32).max(1),
                ((y1 - y0).ceil() as u32).max(1),
            );
            let run = SurfaceRun {
                surface: Surface::Region,
                target,
                exact: false,
                resolution: 72.0,
            };
            let first = picked[0];
            let regions = picked.iter().map(|b| b.region.clone()).collect();
            if self.run_from_surface(&first.plugin, &first.action, &first.values, regions, run) {
                started += 1;
                sent.extend(group.iter().copied());
            }
            if self.dialog.is_some() {
                break;
            }
        }
        if let Some(session) = self.session_mut() {
            sent.sort_unstable();
            for i in sent.iter().rev() {
                session.ai_boxes.remove(*i);
            }
            session.ai_selected = None;
        }
        self.surface_popup = None;
        started
    }

    /// The box's region fields and its action's inputs: basic first, then a
    /// collapsed "Advanced" section. Returns whether its first text field
    /// has text, which Generate needs.
    fn ai_box_form(&mut self, ui: &mut egui::Ui, index: usize) -> bool {
        let Some(ai_box) = self.session().and_then(|s| s.ai_boxes.get(index)).cloned() else {
            return false;
        };
        let Some(spec) = (self.plugins.manifest(&ai_box.plugin))
            .and_then(|m| m.action(&ai_box.action))
            .cloned()
        else {
            return false;
        };
        let fields: Vec<_> = (spec.regions_input().map(|r| r.fields.clone()))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.shown_on(Surface::Region))
            .collect();
        let inputs: Vec<_> = (spec.inputs.iter())
            .filter(|i| i.kind != InputKind::Regions && i.shown_on(Surface::Region))
            .cloned()
            .collect();
        let Some(ai_box) = self.session_mut().and_then(|s| s.ai_boxes.get_mut(index)) else {
            return false;
        };
        let salt = ("ai_box", index);
        let mut ready = true;
        let mut first_text = true;
        for field in fields.iter().filter(|f| !f.advanced) {
            let value = (ai_box.region.fields)
                .entry(field.id.clone())
                .or_insert_with(|| field.initial());
            input_widget(ui, field, value, (salt, "field"));
            if first_text && matches!(field.kind, InputKind::Text | InputKind::Multiline) {
                ready = value.as_str().is_some_and(|t| !t.trim().is_empty());
                first_text = false;
            }
        }
        for input in inputs.iter().filter(|i| !i.advanced) {
            let value = (ai_box.values)
                .entry(input.id.clone())
                .or_insert_with(|| input.initial());
            input_widget(ui, input, value, (salt, "input"));
        }
        let advanced_fields: Vec<_> = fields.iter().filter(|f| f.advanced).collect();
        let advanced_inputs: Vec<_> = inputs.iter().filter(|i| i.advanced).collect();
        if !advanced_fields.is_empty() || !advanced_inputs.is_empty() {
            egui::CollapsingHeader::new(tr("Advanced"))
                .id_salt((salt, "advanced"))
                .default_open(false)
                .show(ui, |ui| {
                    for field in advanced_fields {
                        let value = (ai_box.region.fields)
                            .entry(field.id.clone())
                            .or_insert_with(|| field.initial());
                        input_widget(ui, field, value, (salt, "field"));
                    }
                    for input in advanced_inputs {
                        let value = (ai_box.values)
                            .entry(input.id.clone())
                            .or_insert_with(|| input.initial());
                        input_widget(ui, input, value, (salt, "input"));
                    }
                });
        }
        ready
    }

    fn verb_of(&self, plugin: &str, action: &str) -> String {
        (self.plugins.manifest(plugin))
            .and_then(|m| m.action(action))
            .map(|a| a.verb.clone())
            .unwrap_or_default()
    }

    /// A box's popover, beside the box: the verbs of the region actions,
    /// the box's form, Generate ("Edit 3 boxes" when it sends several) and
    /// Delete.
    pub(super) fn region_popup(&mut self, ctx: &egui::Context) {
        let Some(SurfacePopup::Region { index, .. }) = self.surface_popup else {
            return;
        };
        let Some(session) = self.session() else {
            return;
        };
        let Some(ai_box) = session.ai_boxes.get(index).cloned() else {
            self.surface_popup = None;
            return;
        };
        let Some(viewport) = self.canvas_rect else {
            return;
        };
        let size = egui::vec2(
            session.document.width as f32,
            session.document.height as f32,
        );
        let origin = super::canvas::image_origin(viewport, size, session.zoom, session.pan);
        let anchor = origin
            + egui::vec2(ai_box.region.x + ai_box.region.width, ai_box.region.y) * session.zoom
            + egui::vec2(8.0, 0.0);
        let verbs: Vec<_> = (self.surface_actions(Surface::Region).into_iter())
            .map(|(plugin, action, _)| {
                let verb = self.verb_of(&plugin, &action);
                (plugin, action, verb)
            })
            .collect();
        let count = self.ai_box_groups(index).first().map_or(0, Vec::len);
        let label = if count > 1 {
            format!(
                "{} {count} {}",
                self.verb_of(&ai_box.plugin, &ai_box.action),
                tr("boxes")
            )
        } else {
            tr("Generate").to_owned()
        };
        let mut change = None;
        let mut generate = false;
        let mut delete = false;
        let response = egui::Area::new(egui::Id::new(("region_popup", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain(true)
            .show(ctx, |ui| {
                theme::frame(&ctx.palette())
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        if verbs.len() > 1 {
                            ui.horizontal(|ui| {
                                for (plugin, action, verb) in &verbs {
                                    let on = ai_box.plugin == *plugin && ai_box.action == *action;
                                    if ui.selectable_label(on, verb).clicked() && !on {
                                        change = Some((plugin.clone(), action.clone()));
                                    }
                                }
                            });
                            ui.add_space(4.0);
                        }
                        let ready = self.ai_box_form(ui, index);
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            delete = widgets::button(ui, tr("Delete")).clicked();
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let button = ui
                                        .add_enabled(ready, widgets::Button::new(label).primary());
                                    let shortcut = ui.input(|i| {
                                        i.modifiers.command && i.key_pressed(egui::Key::Enter)
                                    });
                                    generate = button.clicked() || (ready && shortcut);
                                },
                            );
                        });
                    });
            });
        let fresh = std::mem::take(&mut self.surface_popup_fresh);
        // Clicks on the canvas with the tool pick or draw boxes themselves.
        let outside = !fresh
            && response.response.clicked_elsewhere()
            && !ctx
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| viewport.contains(p));
        if let Some((plugin, action)) = change {
            self.set_ai_box_action(index, &plugin, &action);
        } else if delete {
            self.delete_ai_box(index);
        } else if generate {
            self.generate_ai_boxes(index);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || outside {
            self.surface_popup = None;
        }
    }
}

//! The AI Region tool without an open dialog: boxes on the canvas, each with
//! the region action (its verb) and the values it will run with. Boxes belong
//! to their document for the session; they are not edits of it.
use serde_json::{Map, Value};
use xuan::{
    document::Point,
    i18n::tr,
    plugins::{
        jobs::Region,
        manifest::{Action, Input, InputKind, Surface},
    },
};

use super::{
    EditorApp,
    plugin_dialogs::{input_id, input_widget},
    plugins::region_limit,
    surfaces::{SurfacePopup, SurfaceRun, SurfaceStart},
    theme::{self, PaletteExt as _},
    widgets,
};

/// Where an action keeps the prompt Generate needs: the first basic text
/// field of its boxes, else its first basic text input.
enum PromptAt {
    Field(String),
    Input(String),
    /// The action asks for no text: a box is ready as it is.
    Nothing,
}

fn prompt_at(spec: &Action) -> PromptAt {
    let text = |input: &&Input| {
        !input.advanced
            && input.shown_on(Surface::Region)
            && matches!(input.kind, InputKind::Text | InputKind::Multiline)
    };
    if let Some(field) = (spec.regions_input()).and_then(|r| r.fields.iter().find(text)) {
        return PromptAt::Field(field.id.clone());
    }
    match spec.inputs.iter().find(text) {
        Some(input) => PromptAt::Input(input.id.clone()),
        None => PromptAt::Nothing,
    }
}

/// Box ids, so a run removes exactly the boxes it sent.
static NEXT_BOX: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A box drawn with the AI Region tool.
#[derive(Clone, Debug)]
pub(super) struct AiBox {
    pub id: u64,
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
        let first = self.surface_actions(Surface::Region).into_iter().next()?;
        let (plugin, action) = (first.plugin, first.action);
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
            id: NEXT_BOX.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            region,
            plugin,
            action,
            values,
        })
    }

    /// Add a box dragged from `start` to `end` (document pixels), kept on
    /// the canvas, and open its popover. Boxes under two pixels are ignored.
    pub(super) fn add_ai_box(&mut self, start: Point, end: Point) {
        let Some((width, height)) = self
            .session()
            .map(|s| (s.document.width as f32, s.document.height as f32))
        else {
            return;
        };
        let clamp = |p: Point| Point::new(p.x.clamp(0.0, width), p.y.clamp(0.0, height));
        let (start, end) = (clamp(start), clamp(end));
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

    /// The text of a box's prompt, or None when its action asks for none.
    fn box_prompt(&self, ai_box: &AiBox) -> Option<String> {
        let spec = self
            .plugins
            .manifest(&ai_box.plugin)?
            .action(&ai_box.action)?;
        let value = match prompt_at(spec) {
            PromptAt::Field(id) => ai_box.region.fields.get(&id),
            PromptAt::Input(id) => ai_box.values.get(&id),
            PromptAt::Nothing => return None,
        };
        Some(value.and_then(Value::as_str).unwrap_or_default().to_owned())
    }

    /// Whether Generate may send a box: its prompt has text, or its action
    /// asks for none.
    fn box_ready(&self, ai_box: &AiBox) -> bool {
        self.box_prompt(ai_box)
            .is_none_or(|text| !text.trim().is_empty())
    }

    /// Change a box's verb: the new action's defaults, keeping the box's
    /// prompt when both actions have one.
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
        let prompt = (self.session().and_then(|s| s.ai_boxes.get(index)))
            .and_then(|b| self.box_prompt(b))
            .filter(|text| !text.is_empty());
        let Some(ai_box) = self.session_mut().and_then(|s| s.ai_boxes.get_mut(index)) else {
            return;
        };
        ai_box.region.fields.clear();
        for field in &regions.fields {
            ai_box
                .region
                .fields
                .insert(field.id.clone(), field.initial());
        }
        ai_box.values = (spec.inputs.iter())
            .filter(|i| i.kind != InputKind::Regions)
            .map(|i| (i.id.clone(), i.initial()))
            .collect();
        match (prompt, prompt_at(&spec)) {
            (Some(prompt), PromptAt::Field(id)) => {
                ai_box.region.fields.insert(id, prompt.into());
            }
            (Some(prompt), PromptAt::Input(id)) => {
                ai_box.values.insert(id, prompt.into());
            }
            _ => {}
        }
        (ai_box.plugin, ai_box.action) = (plugin.into(), action.into());
    }

    /// How many boxes one job of the box's action takes: more than one for
    /// an action like Precise Edit, so Generate sends every box with it.
    fn boxes_per_job(&self, ai_box: &AiBox) -> usize {
        (self.plugins.manifest(&ai_box.plugin))
            .and_then(|m| m.action(&ai_box.action))
            .and_then(|a| a.regions_input())
            .map_or(1, region_limit)
    }

    /// The boxes Generate on box `index` sends, as groups of one job each.
    /// An action that takes one box runs the box alone (when it is ready).
    /// One that takes several gets every ready box of that action, in jobs of
    /// at most its limit; a job has one set of inputs, so boxes whose own
    /// values (model, quality, …) differ go in jobs of their own.
    fn ai_box_groups(&self, index: usize) -> Vec<Vec<usize>> {
        let Some(session) = self.session() else {
            return Vec::new();
        };
        let Some(chosen) = session.ai_boxes.get(index) else {
            return Vec::new();
        };
        let limit = self.boxes_per_job(chosen).max(1);
        if limit == 1 {
            return if self.box_ready(chosen) {
                vec![vec![index]]
            } else {
                Vec::new()
            };
        }
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, b) in session.ai_boxes.iter().enumerate() {
            if b.plugin != chosen.plugin || b.action != chosen.action || !self.box_ready(b) {
                continue;
            }
            let same = groups
                .iter_mut()
                .find(|group| group.len() < limit && session.ai_boxes[group[0]].values == b.values);
            match same {
                Some(group) => group.push(i),
                None => groups.push(vec![i]),
            }
        }
        groups
    }

    /// Run the box at `index` (with the other boxes of its action when that
    /// action takes several). Boxes without their prompt are left. A box
    /// leaves when its job starts; one waiting for a prompt (permission, send
    /// consent) or that cannot start stays, and stops the rest. Returns the
    /// jobs started.
    pub(super) fn generate_ai_boxes(&mut self, index: usize) -> usize {
        let groups = self.ai_box_groups(index);
        let Some(session) = self.session() else {
            return 0;
        };
        let boxes = session.ai_boxes.clone();
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
                boxes: picked.iter().map(|b| b.id).collect(),
            };
            // The boxes of a group share their values; see ai_box_groups.
            let first = picked[0];
            let regions = picked.iter().map(|b| b.region.clone()).collect();
            match self.run_from_surface(&first.plugin, &first.action, &first.values, regions, run) {
                SurfaceStart::Started => started += 1,
                SurfaceStart::Waiting | SurfaceStart::NotStarted => break,
            }
        }
        if started > 0 {
            if let Some(session) = self.session_mut() {
                session.ai_selected = None;
            }
            self.surface_popup = None;
        }
        started
    }

    /// The box's region fields and its action's inputs: basic first, then a
    /// collapsed "Advanced" section. Returns whether the box is ready to send.
    /// `focus` puts typing in its prompt.
    fn ai_box_form(&mut self, ui: &mut egui::Ui, index: usize, focus: bool) -> bool {
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
        let focused = match prompt_at(&spec) {
            PromptAt::Field(id) if focus => Some(input_id(
                fields.iter().find(|f| f.id == id).expect("prompt field"),
                (salt, "field"),
            )),
            PromptAt::Input(id) if focus => Some(input_id(
                inputs.iter().find(|i| i.id == id).expect("prompt input"),
                (salt, "input"),
            )),
            _ => None,
        };
        for field in fields.iter().filter(|f| !f.advanced) {
            let value = (ai_box.region.fields)
                .entry(field.id.clone())
                .or_insert_with(|| field.initial());
            input_widget(ui, field, value, (salt, "field"));
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
        if let Some(id) = focused {
            ui.memory_mut(|m| m.request_focus(id));
        }
        (self.session().and_then(|s| s.ai_boxes.get(index))).is_some_and(|b| self.box_ready(b))
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
            .map(|offered| {
                let verb = self.verb_of(&offered.plugin, &offered.action);
                (offered, verb)
            })
            .collect();
        let count = self
            .ai_box_groups(index)
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        let label = if count > 1 {
            tr("{verb} {count} boxes")
                .replace("{verb}", &self.verb_of(&ai_box.plugin, &ai_box.action))
                .replace("{count}", &count.to_string())
        } else {
            tr("Generate").to_owned()
        };
        let mut change = None;
        let mut generate = false;
        let mut delete = false;
        let fresh = self.surface_popup_fresh;
        let response = egui::Area::new(egui::Id::new(("region_popup", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain(true)
            .show(ctx, |ui| {
                theme::frame(&ctx.palette())
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        let current = (verbs.iter()).position(|(offered, _)| {
                            offered.plugin == ai_box.plugin && offered.action == ai_box.action
                        });
                        if verbs.len() > 1 {
                            let mut chosen = current.unwrap_or(0);
                            let options: Vec<(usize, &str)> = (verbs.iter().enumerate())
                                .map(|(index, (_, verb))| (index, verb.as_str()))
                                .collect();
                            let response = widgets::segmented(ui, &mut chosen, &options);
                            if Some(chosen) != current {
                                let offered = &verbs[chosen].0;
                                change = Some((offered.plugin.clone(), offered.action.clone()));
                            }
                            if let Some((offered, _)) = current.and_then(|i| verbs.get(i)) {
                                response.on_hover_text(&offered.source);
                            }
                            ui.add_space(6.0);
                        }
                        // Whose words these are: the action and its plugin.
                        if let Some((offered, _)) = current.and_then(|i| verbs.get(i)) {
                            ui.label(
                                egui::RichText::new(offered.attributed())
                                    .small()
                                    .color(ui.palette().muted),
                            )
                            .on_hover_text(&offered.source);
                            ui.add_space(2.0);
                        }
                        let ready = self.ai_box_form(ui, index, fresh);
                        self.surface_estimate_line(ui, &ai_box.plugin, &ai_box.action);
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
        self.surface_popup_fresh = false;
        // Clicks on the canvas with the tool pick or draw boxes themselves.
        let outside = !fresh
            && !self.surface_popup_picking
            && response.response.clicked_elsewhere()
            && !ctx
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| viewport.contains(p));
        if let Some((plugin, action)) = change {
            self.set_ai_box_action(index, &plugin, &action);
            self.request_surface_estimate(&plugin, &action);
        } else if delete {
            self.delete_ai_box(index);
        } else if generate {
            self.generate_ai_boxes(index);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || outside {
            self.surface_popup = None;
        }
    }
}

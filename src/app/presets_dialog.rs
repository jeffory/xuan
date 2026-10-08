//! Layer → Adjustment Presets…: save the selected adjustment layers under a name and add them to
//! any document again. The presets live in `adjustment-presets/` beside the configuration file
//! (see [`xuan::adjustment_presets`]); applying one is a single undo step.
use xuan::{
    adjustment_presets::{self, Library, Preset},
    i18n::tr,
};

use super::theme::PaletteExt as _;
use super::{Dialog, EditorApp, widgets};

/// The dialog's state, kept while it is open.
#[derive(Clone, Debug, Default)]
pub(super) struct PresetsEdit {
    /// The preset Apply and Delete use, by name.
    pub selected: Option<String>,
    /// The name to save the selected layers under.
    pub name: String,
    /// Why the last action failed.
    pub error: Option<String>,
}

enum Action {
    Apply(usize),
    Delete(usize),
    Save,
}

impl EditorApp {
    /// Read the presets beside the configuration file; files that fail are skipped and noted.
    pub(super) fn load_adjustment_presets(&mut self) {
        let folder = self
            .config_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|folder| folder.join(adjustment_presets::FOLDER));
        let (library, problems) = Library::open(folder);
        for problem in problems {
            eprintln!("Xuan: skipped an adjustment preset: {problem}");
        }
        self.adjustment_presets = library;
    }

    pub(super) fn open_adjustment_presets(&mut self) {
        if self.session().is_none() {
            return;
        }
        let taken = |name: &str| self.adjustment_presets.find(name).is_some();
        let name = (1..)
            .map(|n| format!("{} {n}", tr("Preset")))
            .find(|name| !taken(name))
            .unwrap_or_default();
        self.presets_edit = Some(PresetsEdit {
            selected: self
                .adjustment_presets
                .presets
                .first()
                .map(|s| s.preset.name.clone()),
            name,
            error: None,
        });
        self.dialog = Some(Dialog::AdjustmentPresets);
    }

    /// Add the preset at `index` above the active layer, as one undo step.
    pub(super) fn apply_adjustment_preset(&mut self, index: usize) {
        let Some(stored) = self.adjustment_presets.presets.get(index) else {
            return;
        };
        let preset = stored.preset.clone();
        self.edit(tr("Apply Adjustment Preset"), |document| {
            preset.apply(document).map(|_| ())
        });
    }

    /// Save the selected adjustment layers as a preset named `name`, replacing one of that name.
    pub(super) fn save_adjustment_preset(&mut self, name: &str) -> anyhow::Result<()> {
        let session = self
            .session()
            .ok_or_else(|| anyhow::anyhow!("Open a document first"))?;
        let preset = Preset::from_document(&session.document, name)?;
        self.adjustment_presets.save(preset)?;
        self.status = tr("Preset saved").into();
        Ok(())
    }

    pub(super) fn presets_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.presets_edit.take() else {
            self.dialog = None;
            return;
        };
        let presets: Vec<(String, usize)> = self
            .adjustment_presets
            .presets
            .iter()
            .map(|s| (s.preset.name.clone(), s.preset.layers.len()))
            .collect();
        let selected = edit
            .selected
            .as_ref()
            .and_then(|name| presets.iter().position(|p| &p.0 == name));
        let selected_layers = self.session().map_or(0, |s| {
            s.document
                .layers
                .iter()
                .filter(|l| s.document.selected.contains(&l.id) && l.adjustment.is_some())
                .count()
        });
        let mut open = true;
        let mut action = None;
        widgets::Window::new(tr("Adjustment Presets"))
            .id("adjustment_presets")
            .open(&mut open)
            .default_width(380.0)
            .show(ctx, |ui| {
                if presets.is_empty() {
                    ui.label(
                        egui::RichText::new(tr(
                            "No presets yet. Select adjustment layers and save them below.",
                        ))
                        .color(ui.palette().muted),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for (index, (name, layers)) in presets.iter().enumerate() {
                            let response = ui.selectable_label(selected == Some(index), name);
                            let response = response.on_hover_text(
                                tr("{} adjustment layers").replace("{}", &layers.to_string()),
                            );
                            if response.clicked() {
                                edit.selected = Some(name.clone());
                            }
                            if response.double_clicked() {
                                action = Some(Action::Apply(index));
                            }
                        }
                    });
                ui.add_enabled_ui(selected.is_some(), |ui| {
                    ui.horizontal(|ui| {
                        if widgets::button(ui, tr("Apply")).clicked() {
                            action = selected.map(Action::Apply);
                        }
                        if widgets::button(ui, tr("Delete")).clicked() {
                            action = selected.map(Action::Delete);
                        }
                    });
                });
                ui.separator();
                ui.label(if selected_layers == 0 {
                    tr("Select adjustment layers to save them as a preset.").to_owned()
                } else if selected_layers == 1 {
                    tr("Save the selected adjustment layer as a preset.").to_owned()
                } else {
                    tr("Save the {} selected adjustment layers as a preset.")
                        .replace("{}", &selected_layers.to_string())
                });
                ui.horizontal(|ui| {
                    let label = ui.label(tr("Name"));
                    ui.add(egui::TextEdit::singleline(&mut edit.name).desired_width(f32::INFINITY))
                        .labelled_by(label.id);
                });
                let valid = adjustment_presets::check_name(&edit.name).ok();
                if valid
                    .as_ref()
                    .is_some_and(|name| presets.iter().any(|p| &p.0 == name))
                {
                    ui.label(
                        egui::RichText::new(tr("Replaces the preset of this name."))
                            .small()
                            .color(ui.palette().muted),
                    );
                }
                let can_save = selected_layers > 0 && valid.is_some();
                if ui
                    .add_enabled_ui(can_save, |ui| widgets::button(ui, tr("Save Preset")))
                    .inner
                    .clicked()
                {
                    action = Some(Action::Save);
                }
                if let Some(error) = &edit.error {
                    ui.colored_label(ui.palette().error, error);
                }
            });
        match action {
            Some(Action::Apply(index)) => {
                edit.error = None;
                self.apply_adjustment_preset(index);
            }
            Some(Action::Delete(index)) => {
                edit.error = self
                    .adjustment_presets
                    .delete(index)
                    .err()
                    .map(|e| format!("{e:#}"));
                edit.selected = self
                    .adjustment_presets
                    .presets
                    .get(index.min(self.adjustment_presets.presets.len().saturating_sub(1)))
                    .map(|s| s.preset.name.clone());
            }
            Some(Action::Save) => match self.save_adjustment_preset(&edit.name) {
                Ok(()) => {
                    edit.error = None;
                    edit.selected = adjustment_presets::check_name(&edit.name).ok();
                }
                Err(error) => edit.error = Some(format!("{error:#}")),
            },
            None => {}
        }
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dialog = None;
        } else {
            self.presets_edit = Some(edit);
        }
    }
}

//! File → New's Preset menu (issue 140): pixel presets while the dialog shows pixels, physical
//! (paper) presets while it shows a print unit, each labelled with its size. The lists come from
//! `canvas-presets.toml` beside the configuration file when there is one, read again each time
//! File → New opens, and are the built-in ones otherwise (see [`xuan::canvas_presets`]).
//!
//! The menu also saves the current size as a preset, deletes the one shown, and opens Manage
//! presets…, which renames, reorders and deletes presets and restores the built-in lists. The
//! first change writes both lists to the file; closing it returns to File → New as it was.
use std::path::PathBuf;

use anyhow::Context as _;
use egui::RichText;
use xuan::{
    canvas_presets::{self, DEFAULT_GROUP, Kind, Preset, Presets, Size},
    i18n::{tr, tr_args},
};

use super::theme::PaletteExt as _;
use super::{Dialog, EditorApp, widgets};

/// The presets File → New offers and what reading them reported.
#[derive(Clone, Debug, Default)]
pub(super) struct CanvasPresets {
    pub list: Presets,
    /// Why the presets file could not be read; the built-in lists are offered meanwhile.
    pub error: Option<String>,
    /// The entries of the file that were skipped, and why.
    pub skipped: Vec<String>,
    /// Manage presets…, while it is open.
    pub edit: Option<PresetsEdit>,
}

/// Manage presets…'s state.
#[derive(Clone, Debug, Default)]
pub(super) struct PresetsEdit {
    /// The list shown: pixel or physical sizes.
    pub kind: Kind,
    pub selected: Option<usize>,
    /// The name and group Rename gives the selected preset, and Save current size the new one.
    pub name: String,
    pub group: String,
    /// Focus Name the next time the dialog shows.
    focus_name: bool,
    /// Scroll the selected preset into view the next time the dialog shows.
    reveal: bool,
    /// Why the last action failed.
    pub error: Option<String>,
}

/// What the Preset menu's commands asked for.
enum MenuAction {
    Save,
    Delete(usize),
    Manage,
}

/// What a button in Manage presets… asked for.
enum Action {
    Move(usize, bool),
    Delete(usize),
    Rename(usize),
    Save,
    Restore,
    Reveal,
}

/// A preset as the menu shows it: `Full High Definition (1920 × 1080)`, `A4 (210 × 297 mm)`.
/// Built-in names are translated; names the user typed show as typed.
pub(super) fn label(preset: &Preset) -> String {
    format!("{} ({})", tr(&preset.name), preset.size_text())
}

impl EditorApp {
    /// Where the presets are kept once changed: beside the configuration file.
    pub(super) fn canvas_presets_path(&self) -> Option<PathBuf> {
        Some(
            self.config_path
                .as_ref()?
                .with_file_name(canvas_presets::FILE),
        )
    }

    /// Reads the presets file again, so hand edits show; without one, the built-in lists. A file
    /// that cannot be read is reported and left as it is.
    pub(super) fn load_canvas_presets(&mut self) {
        let read = (self.canvas_presets_path())
            .map(|path| Presets::read(&path))
            .transpose();
        let state = &mut self.canvas_presets;
        state.error = None;
        state.skipped.clear();
        state.list = match read {
            Ok(Some(Some((list, skipped)))) => {
                for problem in &skipped {
                    eprintln!("Xuan: skipped a canvas preset: {problem}");
                }
                state.skipped = skipped;
                list
            }
            Ok(_) => Presets::default(),
            Err(error) => {
                eprintln!("Xuan: {error:#}");
                state.error = Some(format!("{error:#}"));
                Presets::default()
            }
        };
    }

    /// New canvas: the Preset menu. It shows the preset the fields match, or Custom.
    pub(super) fn preset_menu(&mut self, ui: &mut egui::Ui) {
        let kind = Kind::of(self.size_units.unit);
        let presets = &self.canvas_presets.list;
        let list = presets.list(kind);
        let current = presets.find(kind, self.dimensions, self.resolution);
        let mut picked = None;
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(tr("Preset"));
            widgets::PopUp::from_id_salt("new_canvas_preset")
                .selected_text(current.map_or_else(|| tr("Custom").to_owned(), |i| label(&list[i])))
                .width(290.0)
                .show_tall_ui(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .show(ui, |ui| {
                            // Custom keeps the fields as they are.
                            widgets::menu_choice(ui, &mut current.is_none(), true, tr("Custom"));
                            for (group, range) in presets.groups(kind) {
                                ui.label(
                                    RichText::new(tr(group)).small().color(ui.palette().muted),
                                );
                                for index in range {
                                    let mut selected = current == Some(index);
                                    let preset = &list[index];
                                    if widgets::menu_choice(ui, &mut selected, true, label(preset))
                                        .clicked()
                                    {
                                        picked = Some(preset.clone());
                                    }
                                }
                            }
                        });
                    ui.separator();
                    let commands = [
                        (tr("Save current size as preset…"), Some(MenuAction::Save)),
                        (tr("Delete preset"), current.map(MenuAction::Delete)),
                        (tr("Manage presets…"), Some(MenuAction::Manage)),
                    ];
                    for (text, command) in commands {
                        let enabled = command.is_some();
                        let chosen = ui
                            .add_enabled_ui(enabled, |ui| {
                                widgets::menu_choice(ui, &mut false, true, text).clicked()
                            })
                            .inner;
                        if chosen {
                            action = command;
                        }
                    }
                });
        });
        let state = &self.canvas_presets;
        if let Some(error) = &state.error {
            ui.label(RichText::new(error).small().color(ui.palette().error));
        } else if !state.skipped.is_empty() {
            ui.label(
                RichText::new(tr("Some canvas presets could not be read and were skipped"))
                    .small()
                    .color(ui.palette().muted),
            )
            .on_hover_text(state.skipped.join("\n"));
        }
        if let Some(preset) = picked {
            self.pick_canvas_preset(&preset);
        }
        match action {
            Some(MenuAction::Save) => self.open_canvas_presets(true),
            Some(MenuAction::Manage) => self.open_canvas_presets(false),
            Some(MenuAction::Delete(index)) => {
                if let Err(error) = self.change_canvas_presets(|presets| {
                    presets.remove(kind, index).context("No such preset")
                }) {
                    self.canvas_presets.error = Some(format!("{error:#}"));
                }
            }
            None => {}
        }
    }

    /// Fills the fields from `preset`. A physical preset switches the fields to its unit, sets
    /// its resolution if it has one, and keeps its exact print size through later changes of
    /// resolution.
    fn pick_canvas_preset(&mut self, preset: &Preset) {
        if let Size::Physical {
            unit, resolution, ..
        } = preset.size
        {
            if let Some(ppi) = resolution {
                self.resolution = ppi;
            }
            if self.size_units.unit != unit {
                self.size_units.unit = unit;
                self.remember_units();
            }
        }
        if let Some(size) = preset.pixels_at(self.resolution) {
            self.dimensions = size;
            self.ratio = size;
        }
        self.size_units.size_edited();
        if let Some(inches) = preset.inches() {
            self.size_units.keep_print_size(inches);
        }
    }
}

impl EditorApp {
    /// Applies `change` to the presets and writes both lists to the presets file. When either
    /// fails, the presets stay as they were.
    fn change_canvas_presets<T>(
        &mut self,
        change: impl FnOnce(&mut Presets) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut list = self.canvas_presets.list.clone();
        let result = change(&mut list)?;
        if let Some(path) = self.canvas_presets_path() {
            list.write(&path)?;
        }
        let state = &mut self.canvas_presets;
        state.list = list;
        // Entries that were skipped are not in the file any more.
        state.skipped.clear();
        state.error = None;
        Ok(result)
    }

    /// The current size as a preset of the list its unit offers: pixels, or the print size in
    /// the fields' unit, as they show it, at the current resolution.
    fn current_size_preset(&self, name: &str, group: &str) -> Preset {
        let unit = self.size_units.unit;
        if Kind::of(unit) == Kind::Physical {
            let side = |axis: usize| {
                let shown = self
                    .size_units
                    .shown(self.dimensions[axis], axis, self.resolution);
                canvas_presets::round_to_field(shown, unit)
            };
            Preset::physical(name, group, [side(0), side(1)], unit, Some(self.resolution))
        } else {
            Preset::pixels(name, group, self.dimensions)
        }
    }

    /// Opens Manage presets… over File → New: to save the current size (`save`, with Name
    /// focused) or with the preset the fields match selected.
    fn open_canvas_presets(&mut self, save: bool) {
        let kind = Kind::of(self.size_units.unit);
        let presets = &self.canvas_presets.list;
        let current = presets.find(kind, self.dimensions, self.resolution);
        let mut edit = PresetsEdit {
            kind,
            focus_name: save,
            reveal: true,
            ..Default::default()
        };
        match current.filter(|_| !save) {
            Some(index) => {
                let preset = &presets.list(kind)[index];
                edit.selected = Some(index);
                edit.name = preset.name.clone();
                edit.group = preset.group.clone();
            }
            None => {
                // The list holds at most MAX_PRESETS, so a free name is found.
                edit.name = (1..=canvas_presets::MAX_PRESETS + 1)
                    .map(|n| format!("{} {n}", tr("Preset")))
                    .find(|name| presets.position(kind, name).is_none())
                    .unwrap_or_default();
                edit.group = tr(DEFAULT_GROUP).to_owned();
            }
        }
        self.canvas_presets.edit = Some(edit);
        self.dialog = Some(Dialog::CanvasPresets);
    }

    /// File → New → Preset → Manage presets…. Done, Esc or closing returns to File → New.
    pub(super) fn canvas_presets_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.canvas_presets.edit.take() else {
            self.dialog = Some(Dialog::New);
            return;
        };
        let presets = self.canvas_presets.list.clone();
        let save_kind = Kind::of(self.size_units.unit);
        let path = self.canvas_presets_path();
        let current = self.current_size_preset("", "");
        let current_text = match current.size {
            Size::Pixels(_) => format!("{} px", current.size_text()),
            Size::Physical { .. } => format!(
                "{} · {} ppi",
                current.size_text(),
                canvas_presets::format_number(f64::from(self.resolution), 2)
            ),
        };
        let mut open = true;
        let mut done = false;
        let mut action = None;
        widgets::Window::new(tr("Canvas presets"))
            .id("canvas_presets")
            .open(&mut open)
            .default_width(420.0)
            .show_with_footer(
                ctx,
                |ui| {
                    let kind = edit.kind;
                    widgets::segmented(
                        ui,
                        &mut edit.kind,
                        &[
                            (Kind::Pixel, tr("Pixel sizes")),
                            (Kind::Physical, tr("Print sizes")),
                        ],
                    );
                    if edit.kind != kind {
                        edit.selected = None;
                    }
                    let list = presets.list(edit.kind);
                    edit.selected = edit.selected.filter(|index| *index < list.len());
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            if list.is_empty() {
                                ui.label(
                                    RichText::new(tr("No presets in this list."))
                                        .color(ui.palette().muted),
                                );
                            }
                            for (group, range) in presets.groups(edit.kind) {
                                ui.label(
                                    RichText::new(tr(group)).small().color(ui.palette().muted),
                                );
                                for index in range {
                                    let preset = &list[index];
                                    let selected = edit.selected == Some(index);
                                    let row = widgets::selected_row(ui, selected, label(preset));
                                    if selected && edit.reveal {
                                        row.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if row.clicked() {
                                        edit.selected = Some(index);
                                        edit.name = preset.name.clone();
                                        edit.group = preset.group.clone();
                                    }
                                }
                            }
                        });
                    edit.reveal = false;
                    ui.add_space(8.0);
                    let same_group = |index: Option<usize>, other: usize| {
                        index.is_some_and(|index| {
                            list.get(other)
                                .is_some_and(|other| other.group == list[index].group)
                        })
                    };
                    ui.horizontal(|ui| {
                        let selected = edit.selected;
                        let up = selected.is_some_and(|i| i > 0 && same_group(selected, i - 1));
                        let down = selected.is_some_and(|i| same_group(selected, i + 1));
                        let buttons = [
                            (tr("Move up"), up, selected.map(|i| Action::Move(i, true))),
                            (
                                tr("Move down"),
                                down,
                                selected.map(|i| Action::Move(i, false)),
                            ),
                            (
                                tr("Delete"),
                                selected.is_some(),
                                selected.map(Action::Delete),
                            ),
                        ];
                        for (text, enabled, chosen) in buttons {
                            if ui
                                .add_enabled_ui(enabled, |ui| widgets::button(ui, text))
                                .inner
                                .clicked()
                            {
                                action = chosen;
                            }
                        }
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    egui::Grid::new("canvas_preset_name")
                        .num_columns(2)
                        .show(ui, |ui| {
                            let name = ui.label(tr("Name"));
                            let field = ui
                                .add(
                                    egui::TextEdit::singleline(&mut edit.name)
                                        .char_limit(canvas_presets::MAX_NAME)
                                        .desired_width(260.0),
                                )
                                .labelled_by(name.id);
                            if edit.focus_name {
                                field.request_focus();
                                edit.focus_name = false;
                            }
                            ui.end_row();
                            let group = ui.label(tr("Group"));
                            ui.add(
                                egui::TextEdit::singleline(&mut edit.group)
                                    .char_limit(canvas_presets::MAX_NAME)
                                    .desired_width(260.0),
                            )
                            .labelled_by(group.id);
                            ui.end_row();
                        });
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(tr_args("Current size: {size}", &[("size", &current_text)]))
                            .color(ui.palette().muted),
                    );
                    let valid = canvas_presets::check_name(&edit.name).is_ok()
                        && canvas_presets::check_name(&edit.group).is_ok();
                    if valid && presets.position(save_kind, edit.name.trim()).is_some() {
                        ui.label(
                            RichText::new(tr("Saving replaces the preset of this name."))
                                .small()
                                .color(ui.palette().muted),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let rename = edit.selected.filter(|_| valid);
                        if ui
                            .add_enabled_ui(valid, |ui| {
                                widgets::button(ui, tr("Save current size"))
                            })
                            .inner
                            .clicked()
                        {
                            action = Some(Action::Save);
                        }
                        if ui
                            .add_enabled_ui(rename.is_some(), |ui| {
                                widgets::button(ui, tr("Rename"))
                            })
                            .inner
                            .clicked()
                        {
                            action = rename.map(Action::Rename);
                        }
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if widgets::button(ui, tr("Restore Defaults"))
                            .on_hover_text(tr(
                                "Delete the presets file and offer the built-in lists again",
                            ))
                            .clicked()
                        {
                            action = Some(Action::Restore);
                        }
                        if ui
                            .add_enabled_ui(path.is_some(), |ui| {
                                widgets::button(ui, tr("Open presets file"))
                            })
                            .inner
                            .on_hover_text(
                                path.as_ref()
                                    .map_or_else(String::new, |path| path.display().to_string()),
                            )
                            .clicked()
                        {
                            action = Some(Action::Reveal);
                        }
                    });
                    if let Some(error) = &edit.error {
                        ui.add_space(8.0);
                        ui.colored_label(ui.palette().error, error);
                    }
                },
                |ui, ()| {
                    done = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::single(tr("Done")),
                        |_| {},
                    )
                    .commit;
                },
            );
        if let Some(action) = action {
            edit.error = self
                .canvas_presets_action(&mut edit, action)
                .err()
                .map(|e| format!("{e:#}"));
        }
        if done || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dialog = Some(Dialog::New);
        } else {
            self.canvas_presets.edit = Some(edit);
        }
    }

    fn canvas_presets_action(
        &mut self,
        edit: &mut PresetsEdit,
        action: Action,
    ) -> anyhow::Result<()> {
        let kind = edit.kind;
        match action {
            Action::Move(index, up) => {
                edit.selected = self
                    .change_canvas_presets(|presets| {
                        (presets.move_within_group(kind, index, up))
                            .context("Cannot move the preset")
                    })
                    .map(Some)?;
            }
            Action::Delete(index) => {
                self.change_canvas_presets(|presets| {
                    presets.remove(kind, index).context("No such preset")
                })?;
                edit.selected = None;
            }
            Action::Rename(index) => {
                let group = self.stored_group(&edit.group);
                let name = edit.name.clone();
                edit.selected =
                    Some(self.change_canvas_presets(|presets| {
                        presets.rename(kind, index, &name, &group)
                    })?);
            }
            Action::Save => {
                let group = self.stored_group(&edit.group);
                let preset = self.current_size_preset(&edit.name, &group);
                let kind = preset.kind();
                let index = self.change_canvas_presets(|presets| presets.insert(preset))?;
                edit.kind = kind;
                edit.selected = Some(index);
                self.status = tr("Preset saved").into();
            }
            Action::Restore => {
                if let Some(path) = self.canvas_presets_path() {
                    canvas_presets::restore_defaults(&path)?;
                }
                let state = &mut self.canvas_presets;
                state.list = Presets::default();
                state.skipped.clear();
                state.error = None;
                edit.selected = None;
            }
            Action::Reveal => {
                let path = self
                    .canvas_presets_path()
                    .context("No configuration folder")?;
                // The file appears once the lists change; asked for, it is written now.
                if !path.exists() {
                    self.canvas_presets.list.write(&path)?;
                }
                super::tabs::show_in_folder(&path)
                    .with_context(|| tr("Could not show the file").to_owned())?;
            }
        }
        Ok(())
    }

    /// The group to keep for `typed`: a group already in the lists when `typed` is its
    /// translated name, so a translated Saved stays Saved; otherwise `typed` as it is.
    fn stored_group(&self, typed: &str) -> String {
        let typed = typed.trim();
        let presets = &self.canvas_presets.list;
        std::iter::once(DEFAULT_GROUP)
            .chain(
                [Kind::Pixel, Kind::Physical]
                    .into_iter()
                    .flat_map(|kind| presets.groups(kind).into_iter().map(|(group, _)| group)),
            )
            .find(|group| tr(group) == typed)
            .unwrap_or(typed)
            .to_owned()
    }
}

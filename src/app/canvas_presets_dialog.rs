//! File → New's Preset menu (issue 140): pixel presets while the dialog shows pixels, physical
//! (paper) presets while it shows a print unit, each labelled with its size. The lists come from
//! `canvas-presets.toml` beside the configuration file when there is one, read again each time
//! File → New opens, and are the built-in ones otherwise (see [`xuan::canvas_presets`]).
use std::path::PathBuf;

use egui::RichText;
use xuan::{
    canvas_presets::{self, Kind, Preset, Presets, Size},
    i18n::tr,
};

use super::theme::PaletteExt as _;
use super::{EditorApp, widgets};

/// The presets File → New offers and what reading them reported.
#[derive(Clone, Debug, Default)]
pub(super) struct CanvasPresets {
    pub list: Presets,
    /// Why the presets file could not be read; the built-in lists are offered meanwhile.
    pub error: Option<String>,
    /// The entries of the file that were skipped, and why.
    pub skipped: Vec<String>,
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

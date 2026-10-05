//! View → Grid Settings…, after Compositor's `UI/GridSettingsSheet.swift`.
//!
//! Every change shows on the canvas at once, with the grid shown while the dialog is open; Cancel
//! puts back what was there. OK keeps the settings for this project (saved in it) and as the
//! default for projects without their own grid.
use xuan::{
    i18n::tr,
    layout::{GridColor, GridSettings, GridStyle},
};

use super::{Dialog, EditorApp, theme, widgets};

/// The dialog's working copy.
#[derive(Clone, Copy, Debug)]
pub(super) struct GridEdit {
    pub draft: GridSettings,
}

impl EditorApp {
    /// The grid in effect: the dialog's draft, else the project's own, else the app default.
    pub(super) fn grid_settings(&self) -> GridSettings {
        if let Some(edit) = &self.grid_edit {
            return edit.draft;
        }
        self.session()
            .and_then(|s| s.document.grid)
            .unwrap_or(self.config.grid)
            .normalized()
    }

    /// Whether the layout grid is drawn; it always is while Grid Settings is open.
    pub(super) fn showing_grid(&self) -> bool {
        self.config.show_grid || self.grid_edit.is_some()
    }

    pub(super) fn open_grid_settings(&mut self) {
        if self.session().is_none() {
            return;
        }
        self.grid_edit = Some(GridEdit {
            draft: self.grid_settings(),
        });
        self.dialog = Some(Dialog::GridSettings);
    }

    /// Applies the dialog: the project records the grid (one undoable step) and the app keeps it as
    /// the default for other projects.
    fn apply_grid_settings(&mut self, grid: GridSettings) {
        if let Some(session) = self.session_mut()
            && session.document.grid != Some(grid)
        {
            session.history.commit();
            session
                .history
                .begin(tr("Grid Settings"), &session.document);
            session.document.grid = Some(grid);
            session.history.commit();
        }
        if self.config.grid != grid {
            self.set_view_option(|config| config.grid = grid);
        }
    }

    pub(super) fn grid_settings_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.grid_edit else {
            self.dialog = None;
            return;
        };
        let mut open = true;
        let mut ok = false;
        let mut cancel = false;
        let draft = &mut edit.draft;
        widgets::Window::new(tr("Grid"))
            .id("grid_settings")
            .open(&mut open)
            .default_width(360.0)
            .show(ctx, |ui| {
                egui::Grid::new("grid_settings_fields")
                    .num_columns(2)
                    .spacing(egui::vec2(12.0, 10.0))
                    .show(ui, |ui| {
                        ui.label(tr("Color"));
                        ui.horizontal(|ui| {
                            widgets::PopUp::from_id_salt("grid_color")
                                .selected_text(tr(draft.color.name()))
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for option in GridColor::ALL {
                                        widgets::menu_choice(
                                            ui,
                                            &mut draft.color,
                                            option,
                                            tr(option.name()),
                                        );
                                    }
                                });
                            // The swatch shows the color in use; picking one makes it Custom.
                            let [r, g, b] = draft.rgb();
                            let mut color = [r, g, b, 255];
                            widgets::color_well(ui, &mut color)
                                .on_hover_text(tr("Choose a custom grid color"));
                            if color[..3] != [r, g, b] {
                                draft.custom_color = [color[0], color[1], color[2]];
                                draft.color = GridColor::Custom;
                            }
                        });
                        ui.end_row();

                        ui.label(tr("Style"));
                        widgets::PopUp::from_id_salt("grid_style")
                            .selected_text(tr(draft.style.name()))
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for option in GridStyle::ALL {
                                    widgets::menu_choice(
                                        ui,
                                        &mut draft.style,
                                        option,
                                        tr(option.name()),
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label(tr("Opacity"));
                        ui.add(
                            widgets::Slider::new(&mut draft.opacity, GridSettings::OPACITY_RANGE)
                                .suffix("%"),
                        );
                        ui.end_row();

                        ui.label(tr("Gridline every"));
                        ui.horizontal(|ui| {
                            ui.add(
                                widgets::Number::new(&mut draft.spacing)
                                    .range(GridSettings::SPACING_RANGE)
                                    .speed(1.0),
                            );
                            ui.label(egui::RichText::new(tr("pixels")).color(theme::MUTED));
                        });
                        ui.end_row();

                        ui.label(tr("Subdivisions"));
                        ui.add(
                            widgets::Number::new(&mut draft.subdivisions)
                                .range(GridSettings::SUBDIVISION_RANGE)
                                .speed(0.2),
                        );
                        ui.end_row();
                    });
                ui.add_space(10.0);
                let valid = draft.is_valid();
                let note = if valid {
                    format!(
                        "{} {} {}",
                        tr("A subdivision every"),
                        format_step(draft.step()),
                        tr("pixels.")
                    )
                } else {
                    tr("Use no more subdivisions than the pixels between gridlines.").to_owned()
                };
                ui.add(
                    egui::Label::new(egui::RichText::new(note).color(if valid {
                        theme::MUTED
                    } else {
                        egui::Color32::from_rgb(255, 170, 60)
                    }))
                    .wrap(),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    cancel = widgets::button(ui, tr("Cancel")).clicked();
                    if widgets::button(ui, tr("Restore Defaults")).clicked() {
                        // The custom color is kept, so it's still there if Custom is chosen again.
                        draft.restore_defaults();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ok = ui
                            .add_enabled(valid, widgets::Button::new(tr("OK")).primary())
                            .clicked();
                    });
                });
            });
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.wants_keyboard_input();
        if (ok || enter) && edit.draft.is_valid() {
            self.grid_edit = None;
            self.dialog = None;
            self.apply_grid_settings(edit.draft);
        } else if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.grid_edit = None;
            self.dialog = None;
        } else {
            self.grid_edit = Some(edit);
        }
    }
}

/// "8", "3.33": up to two decimals, as upstream.
fn format_step(step: f32) -> String {
    let text = format!("{step:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_note_uses_up_to_two_decimals() {
        assert_eq!(format_step(8.0), "8");
        assert_eq!(format_step(10.0 / 3.0), "3.33");
        assert_eq!(format_step(2.5), "2.5");
    }
}

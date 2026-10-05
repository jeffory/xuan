use super::{EditorApp, theme, widgets};
use xuan::{
    config::{Config, Language, PIXEL_GRID_PERCENT_RANGE, TitleBar},
    i18n::{self, tr},
};

/// The pages of the Settings window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SettingsPage {
    #[default]
    General,
    Appearance,
    Keyboard,
}

const PAGE_ID: &str = "settings_page";

/// Shows `page` the next time Settings draws.
pub(super) fn show_settings_page(ctx: &egui::Context, page: SettingsPage) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PAGE_ID), page));
}

impl EditorApp {
    pub(super) fn load_config(&mut self) {
        match Config::path() {
            Ok(path) => {
                match Config::load(&path) {
                    Ok(config) => self.config = config,
                    Err(error) => {
                        self.error = Some(format!("{}\n\n{error:#}", tr("Could not load settings")))
                    }
                }
                self.config_path = Some(path);
            }
            Err(error) => {
                self.error = Some(format!("{}\n\n{error:#}", tr("Could not load settings")))
            }
        }
        i18n::set_language(self.config.language);
        for note in super::commands::override_problems(&self.config.keybindings) {
            eprintln!("Xuan: {note}");
        }
        self.rebuild_keymap();
    }

    /// View → Pixel Grid: apply immediately and remember the choice.
    pub(super) fn set_pixel_grid(&mut self, enabled: bool) {
        if self.config.pixel_grid == enabled {
            return;
        }
        let mut config = self.config.clone();
        config.pixel_grid = enabled;
        self.store_config(config);
    }

    /// A View menu preference (rulers, grid, guides, snapping): apply now and remember it.
    pub(super) fn set_view_option(&mut self, change: impl FnOnce(&mut Config)) {
        let mut config = self.config.clone();
        change(&mut config);
        if config != self.config {
            self.store_config(config);
        }
    }

    /// Keep `config` in memory and write it to the configuration file.
    fn store_config(&mut self, config: Config) {
        self.config = config;
        self.save_config();
    }

    /// Write the current preferences. Headless sessions (tests) have no path
    /// and keep them in memory, so they never touch the user's file.
    pub(super) fn save_config(&mut self) {
        if let Some(path) = &self.config_path
            && let Err(error) = self.config.save(path)
        {
            self.error = Some(format!("{}\n\n{error:#}", tr("Could not save settings")));
        }
    }

    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut done = false;
        let mut config = self.config.clone();
        // A numeric field is being dragged or typed into: apply, but do not write yet.
        let mut editing = false;
        let page_id = egui::Id::new(PAGE_ID);
        let mut page = ctx.data(|d| d.get_temp::<SettingsPage>(page_id).unwrap_or_default());
        let height = if page == SettingsPage::Keyboard {
            420.0
        } else {
            240.0
        };
        widgets::Window::new(tr("Settings"))
            .id("app_settings")
            .default_width(680.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.set_height(height);
                    ui.vertical(|ui| {
                        ui.set_width(125.0);
                        ui.set_min_height(height);
                        for (option, label, hint) in [
                            (
                                SettingsPage::General,
                                tr("General"),
                                tr("General preferences"),
                            ),
                            (
                                SettingsPage::Appearance,
                                tr("Appearance"),
                                tr("Window appearance"),
                            ),
                            (
                                SettingsPage::Keyboard,
                                tr("Keyboard Shortcuts"),
                                tr("Change the keys that run commands"),
                            ),
                        ] {
                            if ui
                                .selectable_label(page == option, label)
                                .on_hover_text(hint)
                                .clicked()
                            {
                                page = option;
                                self.key_editor.capture = None;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(465.0);
                        match page {
                            SettingsPage::General => general_settings(ui, &mut config),
                            SettingsPage::Appearance => {
                                editing = self.appearance_settings(ui, &mut config);
                            }
                            SettingsPage::Keyboard => super::keybindings::page(
                                ui,
                                &self.keymap,
                                &mut self.key_editor,
                                &mut config.keybindings,
                            ),
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary_button(ui, tr("Done")).clicked() {
                            done = true;
                        }
                    });
                });
            });
        ctx.data_mut(|d| d.insert_temp(page_id, page));
        if config != self.config {
            i18n::set_language(config.language);
            let keys_changed = config.keybindings != self.config.keybindings;
            let network_changed =
                config.disable_network_plugins != self.config.disable_network_plugins;
            self.config = config;
            if keys_changed {
                self.rebuild_keymap();
            }
            if network_changed {
                self.apply_network_plugins_setting();
            }
            self.config_dirty = true;
            ctx.request_repaint();
        }
        let closing = !open || done || ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if self.config_dirty && (!editing || closing) {
            self.config_dirty = false;
            self.save_config();
        }
        if closing {
            self.dialog = None;
            self.key_editor.capture = None;
            self.key_editor.conflict = None;
            self.key_editor.message = None;
        }
    }

    /// Returns whether a numeric field is mid-edit, so saving should wait.
    fn appearance_settings(&self, ui: &mut egui::Ui, config: &mut Config) -> bool {
        let mut editing = false;
        ui.heading(tr("Appearance"));
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(tr("Window title bar"));
            widgets::PopUp::from_id_salt("settings_title_bar")
                .selected_text(tr(config.title_bar.name()))
                .width(180.0)
                .show_ui(ui, |ui| {
                    for option in TitleBar::ALL {
                        widgets::menu_choice(ui, &mut config.title_bar, option, tr(option.name()));
                    }
                });
        });
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(match config.title_bar {
                TitleBar::System => tr("Use the title bar and window buttons of your desktop."),
                TitleBar::Compact => {
                    tr("Show the menus in the title bar, with window buttons on the right.")
                }
                TitleBar::MacOs => tr(
                    "Show the menus in the title bar, with macOS-style window buttons on the left.",
                ),
            })
            .wrap(),
        );
        ui.add_space(8.0);
        let note = if config.title_bar.client_side() && !self.transparent_window {
            tr("Rounded window corners appear after restarting Xuan.")
        } else {
            tr("Title bar changes apply immediately.")
        };
        ui.add(egui::Label::new(egui::RichText::new(note).color(theme::MUTED)).wrap());
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            widgets::checkbox(ui, &mut config.pixel_grid, tr("Pixel Grid"));
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(tr("Show pixel grid above"));
            let mut percent = config.pixel_grid_percent();
            let response = ui.add(
                egui::DragValue::new(&mut percent)
                    .range(PIXEL_GRID_PERCENT_RANGE)
                    .speed(10.0)
                    .suffix("%"),
            );
            editing = response.dragged() || response.has_focus();
            if percent != config.pixel_grid_percent() {
                config.pixel_grid_percent = percent;
            }
        });
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(tr(
                    "Outlines individual pixels when zoomed in past this level, between 200% and 6400%.",
                ))
                .color(theme::MUTED),
            )
            .wrap(),
        );
        editing
    }
}

fn general_settings(ui: &mut egui::Ui, config: &mut Config) {
    ui.heading(tr("General"));
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.label(tr("Language"));
        widgets::PopUp::from_id_salt("settings_language")
            .selected_text(config.language.name())
            .width(180.0)
            .show_ui(ui, |ui| {
                for option in [Language::English, Language::SimplifiedChinese] {
                    widgets::menu_choice(ui, &mut config.language, option, option.name());
                }
            });
    });
    ui.add_space(8.0);
    ui.label(tr("Language changes apply immediately."));
    ui.add_space(16.0);
    widgets::checkbox(
        ui,
        &mut config.disable_network_plugins,
        tr("Disable plugins that use the network"),
    );
    ui.add(
        egui::Label::new(
            egui::RichText::new(tr(
                "Plugins that declare network hosts do not start and their actions are unavailable. A plugin that declares none could still connect.",
            ))
            .small()
            .color(theme::MUTED),
        )
        .wrap(),
    );
    ui.add_space(20.0);
    ui.label(egui::RichText::new(tr("Configuration file")).color(theme::MUTED));
    if let Ok(path) = Config::path() {
        ui.add(egui::Label::new(path.display().to_string()).wrap());
    }
}

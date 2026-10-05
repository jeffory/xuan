use super::{EditorApp, theme, widgets};
use xuan::{
    config::{Config, Language, PIXEL_GRID_PERCENT_RANGE, TitleBar},
    i18n::{self, tr},
};

impl EditorApp {
    pub(super) fn load_config(&mut self) {
        match Config::path().and_then(|path| Config::load(&path)) {
            Ok(config) => self.config = config,
            Err(error) => {
                self.error = Some(format!("{}\n\n{error:#}", tr("Could not load settings")))
            }
        }
        i18n::set_language(self.config.language);
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
        self.config = config.clone();
        if let Err(error) = self.save_config(&config) {
            self.error = Some(format!("{}\n\n{error:#}", tr("Could not save settings")));
        }
    }

    fn save_config(&self, config: &Config) -> anyhow::Result<()> {
        match &self.config_path {
            Some(path) => config.save(path),
            None => Config::path().and_then(|path| config.save(&path)),
        }
    }

    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut done = false;
        let mut config = self.config.clone();
        // A numeric field is being dragged or typed into: apply, but do not write yet.
        let mut editing = false;
        let page_id = egui::Id::new("settings_page");
        let mut appearance = ctx.data(|d| d.get_temp::<bool>(page_id).unwrap_or(false));
        widgets::Window::new(tr("Settings"))
            .id("app_settings")
            .default_width(580.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.set_height(240.0);
                    ui.vertical(|ui| {
                        ui.set_width(125.0);
                        ui.set_min_height(240.0);
                        if ui
                            .selectable_label(!appearance, tr("General"))
                            .on_hover_text(tr("General preferences"))
                            .clicked()
                        {
                            appearance = false;
                        }
                        if ui
                            .selectable_label(appearance, tr("Appearance"))
                            .on_hover_text(tr("Window appearance"))
                            .clicked()
                        {
                            appearance = true;
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(365.0);
                        if appearance {
                            editing = self.appearance_settings(ui, &mut config);
                        } else {
                            general_settings(ui, &mut config);
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
        ctx.data_mut(|d| d.insert_temp(page_id, appearance));
        if config != self.config {
            i18n::set_language(config.language);
            self.config = config;
            self.config_dirty = true;
            ctx.request_repaint();
        }
        let closing = !open || done || ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if self.config_dirty && (!editing || closing) {
            self.config_dirty = false;
            if let Err(error) = self.save_config(&self.config) {
                self.error = Some(format!("{}\n\n{error:#}", tr("Could not save settings")));
            }
        }
        if closing {
            self.dialog = None;
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
    ui.add_space(28.0);
    ui.label(egui::RichText::new(tr("Configuration file")).color(theme::MUTED));
    if let Ok(path) = Config::path() {
        ui.add(egui::Label::new(path.display().to_string()).wrap());
    }
}

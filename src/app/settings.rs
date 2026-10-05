use super::{EditorApp, theme, widgets};
use xuan::{
    config::{Config, Language, TitleBar},
    i18n::{self, tr},
};

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
    }

    /// Write the current preferences. Headless sessions have no path and keep them in memory.
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
                            self.appearance_settings(ui, &mut config);
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
            self.save_config();
            ctx.request_repaint();
        }
        if !open || done || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dialog = None;
        }
    }

    fn appearance_settings(&self, ui: &mut egui::Ui, config: &mut Config) {
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

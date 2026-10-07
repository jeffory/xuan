use super::theme::PaletteExt as _;
use super::{EditorApp, widgets};
use xuan::{
    config::{Config, Language, PIXEL_GRID_PERCENT_RANGE, Theme, TitleBar},
    i18n::{self, tr},
    plugins::{manifest::Capability, sandbox},
};

/// The pages of the Settings window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SettingsPage {
    #[default]
    General,
    Appearance,
    /// Which algorithm Select Subject, Remove Background and Object mode use.
    Selection,
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
        let providers: Vec<_> = Capability::ALL
            .into_iter()
            .map(|capability| (capability, self.provider_choices(capability)))
            .collect();
        widgets::Window::new(tr("Settings"))
            .id("app_settings")
            .default_width(680.0)
            .open(&mut open)
            .show_with_footer(ctx, |ui| {
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
                                SettingsPage::Selection,
                                tr("Selection"),
                                tr("Choose the algorithm behind Select Subject and Remove Background"),
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
                            SettingsPage::Selection => {
                                selection_settings(ui, &mut config, &providers)
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
            }, |ui, ()| {
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
            let blocking_changed =
                config.block_undeclared_network() != self.config.block_undeclared_network();
            self.config = config;
            if keys_changed {
                self.rebuild_keymap();
            }
            if network_changed {
                self.apply_network_plugins_setting();
            }
            if blocking_changed {
                self.apply_block_network_setting();
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
            ui.label(tr("Theme"));
            widgets::PopUp::from_id_salt("settings_theme")
                .selected_text(tr(config.theme.name()))
                .width(180.0)
                .show_ui(ui, |ui| {
                    for option in Theme::ALL {
                        widgets::menu_choice(ui, &mut config.theme, option, tr(option.name()));
                    }
                });
        });
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(match config.theme {
                Theme::System => tr("Follow your desktop's light or dark setting."),
                Theme::Light => tr("Always use light colours."),
                Theme::Dark => tr("Always use dark colours."),
            })
            .wrap(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            widgets::checkbox(
                ui,
                &mut config.system_accent,
                tr("Use system accent colour"),
            );
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(tr(
                    "Highlights take your desktop's accent colour instead of Xuan's blue, when it has one.",
                ))
                .color(ui.palette().muted),
            )
            .wrap(),
        );
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
        #[cfg(target_os = "linux")]
        if config.title_bar == TitleBar::Compact {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(tr("Window buttons"));
                widgets::PopUp::from_id_salt("settings_window_buttons")
                    .selected_text(tr(config.window_buttons.name()))
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for option in xuan::config::WindowButtons::ALL {
                            widgets::menu_choice(
                                ui,
                                &mut config.window_buttons,
                                option,
                                tr(option.name()),
                            );
                        }
                    });
            });
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(match config.window_buttons {
                    xuan::config::WindowButtons::Theme => tr(
                        "Draw the buttons with the images of your desktop theme, falling back to the built-in ones when it has none.",
                    ),
                    xuan::config::WindowButtons::BuiltIn => {
                        tr("Always draw the buttons that come with Xuan.")
                    }
                })
                .wrap(),
            );
            if config.window_buttons == xuan::config::WindowButtons::Theme {
                let guard = self.window_theme.lock().unwrap_or_else(|e| e.into_inner());
                let found = guard
                    .as_ref()
                    .and_then(|theme| theme.resolved())
                    .map(|resolved| resolved.describe());
                let text = match found {
                    Some((source, Some(hover))) => format!(
                        "{} {}. {} {}",
                        tr("Images from"),
                        tr(source),
                        tr("Hovered close button:"),
                        hover.display()
                    ),
                    Some((source, None)) => format!("{} {}.", tr("Images from"), tr(source)),
                    None => tr("No theme images were found, so the built-in buttons are drawn.")
                        .to_string(),
                };
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(text).color(ui.palette().muted)).wrap(),
                );
            }
        }
        ui.add_space(8.0);
        let note = if config.title_bar.client_side() && !self.transparent_window {
            tr("Rounded window corners appear after restarting Xuan.")
        } else {
            tr("Title bar changes apply immediately.")
        };
        ui.add(egui::Label::new(egui::RichText::new(note).color(ui.palette().muted)).wrap());
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
                .color(ui.palette().muted),
            )
            .wrap(),
        );
        editing
    }
}

/// Settings → Selection: a provider for each replaceable algorithm.
fn selection_settings(
    ui: &mut egui::Ui,
    config: &mut Config,
    providers: &[(Capability, Vec<super::providers::Choice>)],
) {
    ui.heading(tr("Selection"));
    ui.add_space(16.0);
    egui::Grid::new("settings_providers")
        .num_columns(2)
        .spacing(egui::vec2(12.0, 10.0))
        .show(ui, |ui| {
            for (capability, choices) in providers {
                let current = config.providers.get(*capability).map(str::to_owned);
                // A chosen plugin that is no longer installed still shows by its id.
                let shown = choices.iter().find(|(id, _)| *id == current).map_or_else(
                    || current.clone().unwrap_or_default(),
                    |(_, label)| label.clone(),
                );
                ui.label(format!("{} {}", tr(capability.label()), tr("provider")));
                let mut chosen = current.clone();
                widgets::PopUp::from_id_salt(("settings_provider", capability.id()))
                    .selected_text(shown)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for (id, label) in choices {
                            widgets::menu_choice(ui, &mut chosen, id.clone(), label);
                        }
                    });
                if chosen != current {
                    config.providers.set(*capability, chosen);
                }
                ui.end_row();
            }
        });
    ui.add_space(12.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(tr(
                "Built-in uses Xuan's own classical segmentation, with no machine learning. A plugin that provides one of these, such as a segmentation model, can replace it; it runs under the plugin's usual permissions, and if it cannot run (disabled, or offline mode) the built-in algorithm is used with a notice.",
            ))
            .color(ui.palette().muted),
        )
        .wrap(),
    );
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
            egui::RichText::new(super::plugin_consent::offline_mode_note(config))
                .small()
                .color(ui.palette().muted),
        )
        .wrap(),
    );
    ui.add_space(12.0);
    let mut block = config.block_undeclared_network();
    let changed = ui
        .add_enabled_ui(sandbox::SUPPORTED, |ui| {
            widgets::checkbox(
                ui,
                &mut block,
                tr("Block network for plugins that don't declare it"),
            )
        })
        .inner
        .changed();
    if changed {
        config.block_undeclared_network = Some(block);
    }
    ui.add(
        egui::Label::new(
            egui::RichText::new(if sandbox::SUPPORTED {
                tr(
                    "Plugins that declare no network hosts cannot open network sockets, not even to this computer (localhost). Running plugins restart to apply it. Plugins that declare hosts are not blocked.",
                )
            } else {
                tr("Only available on Linux.")
            })
            .small()
            .color(ui.palette().muted),
        )
        .wrap(),
    );
    ui.add_space(20.0);
    ui.label(egui::RichText::new(tr("Configuration file")).color(ui.palette().muted));
    if let Ok(path) = Config::path() {
        ui.add(egui::Label::new(path.display().to_string()).wrap());
    }
}

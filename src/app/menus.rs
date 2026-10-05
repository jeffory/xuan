use std::collections::HashMap;

use egui::{Button, RichText};
use xuan::i18n::tr;
use xuan::{
    document::{Adjustment, Point},
    effects::Filter,
    plugins::manifest::Menu,
};

use super::{EditorApp, commands, theme, widgets};

fn menu_bar_button(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui)) {
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui.add(Button::new(label).fill(egui::Color32::TRANSPARENT));
    let rect = response.rect.expand2(egui::vec2(4.0, 0.0));
    let popup_id = egui::Popup::default_response_id(&response);
    let open_menu_id = ui.id().with("menu_bar_open_popup");
    let open_menu = ui
        .data(|data| data.get_temp::<egui::Id>(open_menu_id))
        .filter(|id| egui::Popup::is_id_open(ui.ctx(), *id));
    if response.hovered() && !response.clicked() && open_menu.is_some_and(|id| id != popup_id) {
        egui::Popup::open_id(ui.ctx(), popup_id);
    }

    let config = egui::containers::menu::MenuConfig::find(ui);
    egui::Popup::menu(&response)
        .anchor(rect)
        .close_behavior(config.close_behavior)
        .style(config.style)
        .show(content);
    let visuals = if egui::Popup::is_id_open(ui.ctx(), popup_id) {
        ui.data_mut(|data| data.insert_temp(open_menu_id, popup_id));
        &ui.visuals().widgets.open
    } else {
        ui.style().interact(&response)
    };

    // The dropdown and the expanded highlight share the same left edge.
    ui.painter().set(
        background,
        egui::epaint::RectShape::filled(rect, visuals.corner_radius, visuals.weak_bg_fill),
    );
}

/// Entries plugins added to a menu.
fn plugin_items(
    ui: &mut egui::Ui,
    items: Option<&Vec<super::plugins::PluginMenuItem>>,
    action: &mut Option<(String, String)>,
) {
    let Some(items) = items.filter(|items| !items.is_empty()) else {
        return;
    };
    ui.separator();
    for item in items {
        let mut button = Button::new(&item.label);
        // An empty shortcut would still add a space to the accessible name.
        if !item.shortcut.is_empty() {
            button = button.shortcut_text(&item.shortcut);
        }
        let response = ui
            .add_enabled(item.enabled, button)
            .on_hover_text(&item.source)
            .on_disabled_hover_text(&item.source);
        if response.clicked() {
            *action = Some((item.plugin.clone(), item.action.clone()));
            ui.close();
        }
    }
}

/// What the menus need from the command registry this frame: whether each command can run
/// and its shortcut hint.
pub(super) struct MenuItems(HashMap<&'static str, (bool, String)>);

impl MenuItems {
    pub(super) fn get(&self, command: &str) -> (bool, &str) {
        self.0
            .get(command)
            .map_or((false, ""), |(enabled, shortcut)| (*enabled, shortcut))
    }
}

/// A menu item for a registry command, with the registry's label.
fn item(
    ui: &mut egui::Ui,
    items: &MenuItems,
    command: &'static str,
    action: &mut Option<&'static str>,
) {
    let label = commands::find(command).map_or(command, |c| tr(c.label));
    labelled(ui, items, label, command, action);
}

/// A menu item for a registry command, with a label of its own.
fn labelled(
    ui: &mut egui::Ui,
    items: &MenuItems,
    label: &str,
    command: &'static str,
    action: &mut Option<&'static str>,
) {
    let (enabled, shortcut) = items.get(command);
    // Without a shortcut the accessible label is just the name, with no trailing space.
    let button = if shortcut.is_empty() {
        Button::new(label)
    } else {
        Button::new(label).shortcut_text(shortcut)
    };
    if ui.add_enabled(enabled, button).clicked() {
        *action = Some(command);
        ui.close();
    }
}

/// A menu toggle with a shortcut hint, run as a command so the key chord does the same thing.
fn check_item(
    ui: &mut egui::Ui,
    items: &MenuItems,
    checked: bool,
    label: Option<&str>,
    command: &'static str,
    action: &mut Option<&'static str>,
) {
    let label = label.unwrap_or_else(|| commands::find(command).map_or(command, |c| tr(c.label)));
    let (enabled, shortcut) = items.get(command);
    let response = ui.add_enabled_ui(enabled, |ui| {
        widgets::menu_check(ui, checked, label, shortcut)
    });
    if response.inner.clicked() {
        *action = Some(command);
        ui.close();
    }
}

pub(super) fn filter_menu(ui: &mut egui::Ui) -> Option<Filter> {
    let mut result = None;
    for filter in [
        Filter::GaussianBlur { radius: 4.0 },
        Filter::MotionBlur {
            distance: 15.0,
            angle: 0.0,
        },
        Filter::Noise {
            amount: 10.0,
            monochrome: true,
        },
        Filter::LensCorrection {
            distortion: 0.0,
            vignette: 0.0,
        },
    ] {
        if ui.button(tr(filter.name())).clicked() {
            result = Some(filter);
            ui.close();
        }
    }
    result
}

pub(super) fn adjustment_menu(ui: &mut egui::Ui) -> Option<Adjustment> {
    let mut result = None;
    for adjustment in [
        Adjustment::HueRanges {
            settings: Box::default(),
        },
        Adjustment::LevelsChannels {
            ranges: [xuan::color::DEFAULT_LEVELS; 4],
        },
        Adjustment::CurvesChannels {
            channels: std::array::from_fn(|_| vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)]),
        },
        Adjustment::Exposure {
            exposure: 0.0,
            offset: 0.0,
            gamma: 1.0,
        },
        Adjustment::GradientMap {
            shadows: [0, 0, 0, 255],
            highlights: [255; 4],
        },
        Adjustment::FilmGrain {
            amount: 10.0,
            size: 1.0,
            roughness: 50.0,
            seed: 1,
        },
        Adjustment::BLACK_WHITE,
        Adjustment::COLOR_BALANCE,
    ] {
        if ui.button(tr(adjustment.name())).clicked() {
            result = Some(adjustment);
            ui.close();
        }
    }
    result
}

impl EditorApp {
    pub(super) fn menu_items(&self) -> MenuItems {
        MenuItems(
            commands::COMMANDS
                .iter()
                .map(|c| {
                    (
                        c.id,
                        (self.command_enabled(c.id), self.keymap.shortcut(c.id)),
                    )
                })
                .collect(),
        )
    }

    pub(super) fn menus(&mut self, ctx: &egui::Context) {
        let items = self.menu_items();
        let mut action = None;
        let mut adjustment = None;
        let mut filter = None;
        let mut adjustment_layer = false;
        let mut filter_layer = false;
        let developing = self.develop.is_some();
        let has_doc = self.session().is_some() && !developing;
        let can_view = self.develop.as_ref().is_none_or(|d| d.ready());
        let pane_entries = self.pane_entries();
        let mut pane_toggle = None;
        let plugin_menu = self.plugin_menu_items();
        let mut plugin_action: Option<(String, String)> = None;
        let can_rerun = self
            .session()
            .and_then(|s| s.document.active())
            .is_some_and(|layer| layer.generated.is_some());
        let blocked = self.job.is_some()
            || self.dialog.is_some()
            || self.close_app
            || self.develop_close_requested.is_some()
            || self.close_tab.is_some();
        super::chrome::title_bar(self.window_corner_radius(ctx), "menubar").show(ctx, |ui| {
            egui::MenuBar::new()
                .config(egui::containers::menu::MenuConfig::new().style(theme::menu_style))
                .ui(ui, |ui| {
                    self.leading_window_controls(ui);
                    ui.add_enabled_ui(!blocked, |ui| {
                        // Leave a small gap between the expanded highlights.
                        ui.spacing_mut().item_spacing.x += 2.0;
                        menu_bar_button(ui, tr("File"), |ui| {
                            item(ui, &items, "new", &mut action);
                            item(ui, &items, "open", &mut action);
                            item(ui, &items, "open_clipboard", &mut action);
                            item(ui, &items, "open_comp", &mut action);
                            ui.add_enabled_ui(!developing, |ui| {
                                item(ui, &items, "import", &mut action);
                            });
                            ui.separator();
                            ui.add_enabled_ui(has_doc, |ui| {
                                item(ui, &items, "save", &mut action);
                                item(ui, &items, "save_as", &mut action);
                                item(ui, &items, "export", &mut action);
                                ui.separator();
                                item(ui, &items, "close", &mut action);
                            });
                            if developing {
                                labelled(ui, &items, tr("Close RAW Develop"), "close", &mut action);
                            }
                            ui.add_enabled_ui(has_doc, |ui| {
                                plugin_items(ui, plugin_menu.get(&Menu::File), &mut plugin_action);
                            });
                            ui.separator();
                            item(ui, &items, "quit", &mut action);
                        });
                        menu_bar_button(ui, tr("Edit"), |ui| {
                            item(ui, &items, "settings", &mut action);
                            plugin_items(ui, plugin_menu.get(&Menu::Edit), &mut plugin_action);
                            ui.separator();
                            let (undo, redo) = if let Some(d) = &self.develop {
                                (
                                    (d.ready() && !d.undo.is_empty())
                                        .then_some(tr("RAW adjustment")),
                                    (d.ready() && !d.redo.is_empty())
                                        .then_some(tr("RAW adjustment")),
                                )
                            } else {
                                (
                                    self.session().and_then(|s| s.history.undo_name()),
                                    self.session().and_then(|s| s.history.redo_name()),
                                )
                            };
                            ui.add_enabled_ui(undo.is_some(), |ui| {
                                labelled(
                                    ui,
                                    &items,
                                    &format!("{} {}", tr("Undo"), tr(undo.unwrap_or(""))),
                                    "undo",
                                    &mut action,
                                )
                            });
                            ui.add_enabled_ui(redo.is_some(), |ui| {
                                labelled(
                                    ui,
                                    &items,
                                    &format!("{} {}", tr("Redo"), tr(redo.unwrap_or(""))),
                                    "redo",
                                    &mut action,
                                )
                            });
                            ui.separator();
                            ui.add_enabled_ui(has_doc, |ui| {
                                item(ui, &items, "cut", &mut action);
                                item(ui, &items, "copy", &mut action);
                                item(ui, &items, "copy_merged", &mut action);
                            });
                            ui.add_enabled_ui(!developing, |ui| {
                                item(ui, &items, "paste", &mut action);
                            });
                            ui.add_enabled_ui(has_doc, |ui| {
                                ui.separator();
                                item(ui, &items, "fill_fg", &mut action);
                                item(ui, &items, "fill_bg", &mut action);
                                item(ui, &items, "clear", &mut action);
                                item(ui, &items, "content_fill", &mut action);
                            });
                        });
                        menu_bar_button(ui, tr("Image"), |ui| {
                            ui.add_enabled_ui(has_doc, |ui| {
                                ui.menu_button(tr("Adjustments"), |ui| {
                                    adjustment = adjustment_menu(ui);
                                    ui.separator();
                                    item(ui, &items, "invert", &mut action);
                                });
                                ui.separator();
                                item(ui, &items, "image_size", &mut action);
                                item(ui, &items, "canvas_size", &mut action);
                                ui.separator();
                                item(ui, &items, "flip_canvas_h", &mut action);
                                item(ui, &items, "flip_canvas_v", &mut action);
                                plugin_items(ui, plugin_menu.get(&Menu::Image), &mut plugin_action);
                            });
                        });
                        menu_bar_button(ui, tr("Layer"), |ui| {
                            ui.add_enabled_ui(has_doc, |ui| {
                                item(ui, &items, "new_layer", &mut action);
                                item(ui, &items, "duplicate", &mut action);
                                item(ui, &items, "delete_layer", &mut action);
                                ui.add_enabled_ui(
                                    self.session()
                                        .and_then(|s| s.document.active())
                                        .is_some_and(|l| l.raw.is_some() && !l.locked),
                                    |ui| {
                                        item(ui, &items, "develop", &mut action);
                                        item(ui, &items, "rasterize_raw", &mut action);
                                    },
                                );
                                item(ui, &items, "layer_effects", &mut action);
                                ui.separator();
                                item(ui, &items, "group", &mut action);
                                item(ui, &items, "ungroup", &mut action);
                                item(ui, &items, "merge", &mut action);
                                item(ui, &items, "flatten", &mut action);
                                ui.separator();
                                ui.menu_button(tr("New Adjustment Layer"), |ui| {
                                    adjustment = adjustment_menu(ui);
                                    adjustment_layer = true;
                                });
                                ui.menu_button(tr("New Filter Layer"), |ui| {
                                    filter = filter_menu(ui);
                                    filter_layer = true;
                                });
                                ui.menu_button(tr("Layer Mask"), |ui| {
                                    item(ui, &items, "new_mask_layer", &mut action);
                                    item(ui, &items, "mask", &mut action);
                                    labelled(
                                        ui,
                                        &items,
                                        tr("Enable / Disable"),
                                        "disable_mask",
                                        &mut action,
                                    );
                                    labelled(
                                        ui,
                                        &items,
                                        tr("Link / Unlink"),
                                        "link_mask",
                                        &mut action,
                                    );
                                    item(ui, &items, "delete_mask", &mut action);
                                });
                                item(ui, &items, "clip", &mut action);
                                ui.separator();
                                item(ui, &items, "flip_h", &mut action);
                                item(ui, &items, "flip_v", &mut action);
                                ui.separator();
                                ui.add_enabled_ui(can_rerun, |ui| {
                                    item(ui, &items, "rerun_plugin", &mut action);
                                });
                                plugin_items(ui, plugin_menu.get(&Menu::Layer), &mut plugin_action);
                            });
                        });
                        menu_bar_button(ui, tr("Select"), |ui| {
                            ui.add_enabled_ui(has_doc, |ui| {
                                labelled(ui, &items, tr("All"), "select_all", &mut action);
                                item(ui, &items, "deselect", &mut action);
                                labelled(
                                    ui,
                                    &items,
                                    tr("Inverse"),
                                    "invert_selection",
                                    &mut action,
                                );
                                item(ui, &items, "load_selection", &mut action);
                                item(ui, &items, "feather", &mut action);
                                plugin_items(
                                    ui,
                                    plugin_menu.get(&Menu::Select),
                                    &mut plugin_action,
                                );
                            });
                        });
                        menu_bar_button(ui, tr("Filter"), |ui| {
                            ui.add_enabled_ui(has_doc, |ui| {
                                item(ui, &items, "remove_background", &mut action);
                                ui.separator();
                                for f in [
                                    Filter::GaussianBlur { radius: 4.0 },
                                    Filter::MotionBlur {
                                        distance: 15.0,
                                        angle: 0.0,
                                    },
                                    Filter::Noise {
                                        amount: 10.0,
                                        monochrome: true,
                                    },
                                    Filter::LensCorrection {
                                        distortion: 0.0,
                                        vignette: 0.0,
                                    },
                                ] {
                                    if ui.button(format!("{}…", tr(f.name()))).clicked() {
                                        filter = Some(f);
                                        ui.close();
                                    }
                                }
                                plugin_items(
                                    ui,
                                    plugin_menu.get(&Menu::Filter),
                                    &mut plugin_action,
                                );
                            });
                        });
                        menu_bar_button(ui, tr("View"), |ui| {
                            ui.add_enabled_ui(can_view, |ui| {
                                item(ui, &items, "fit", &mut action);
                                item(ui, &items, "actual", &mut action);
                                item(ui, &items, "zoom_in", &mut action);
                                item(ui, &items, "zoom_out", &mut action);
                            });
                            ui.separator();
                            check_item(
                                ui,
                                &items,
                                self.config.pixel_grid,
                                None,
                                "toggle_pixel_grid",
                                &mut action,
                            );
                            check_item(
                                ui,
                                &items,
                                self.show_controls,
                                None,
                                "toggle_controls",
                                &mut action,
                            );
                            // Rulers, grid, guides and snapping, as upstream's View menu.
                            ui.separator();
                            ui.add_enabled_ui(has_doc, |ui| {
                                ui.menu_button(tr("Show"), |ui| {
                                    check_item(
                                        ui,
                                        &items,
                                        self.config.show_grid,
                                        Some(tr("Grid")),
                                        "toggle_grid",
                                        &mut action,
                                    );
                                    check_item(
                                        ui,
                                        &items,
                                        self.config.show_guides,
                                        Some(tr("Guides")),
                                        "toggle_guides",
                                        &mut action,
                                    );
                                });
                                item(ui, &items, "grid_settings", &mut action);
                                check_item(
                                    ui,
                                    &items,
                                    self.config.rulers,
                                    None,
                                    "toggle_rulers",
                                    &mut action,
                                );
                                ui.separator();
                                check_item(
                                    ui,
                                    &items,
                                    self.config.snap.enabled,
                                    None,
                                    "toggle_snap",
                                    &mut action,
                                );
                                let mut snap = self.config.snap;
                                ui.menu_button(tr("Snap To"), |ui| {
                                    ui.add_enabled_ui(snap.enabled, |ui| {
                                        for (value, label) in [
                                            (&mut snap.guides, tr("Guides")),
                                            (&mut snap.grid, tr("Grid")),
                                            (&mut snap.layers, tr("Layers")),
                                            (&mut snap.bounds, tr("Document Bounds")),
                                        ] {
                                            if widgets::menu_check(ui, *value, label, "").clicked()
                                            {
                                                *value = !*value;
                                            }
                                        }
                                    });
                                });
                                if snap != self.config.snap {
                                    self.set_view_option(|config| config.snap = snap);
                                }
                                ui.separator();
                                check_item(
                                    ui,
                                    &items,
                                    self.config.lock_guides,
                                    None,
                                    "lock_guides",
                                    &mut action,
                                );
                                let has_guides = self
                                    .session()
                                    .is_some_and(|s| !s.document.guides.is_empty());
                                ui.add_enabled_ui(has_guides, |ui| {
                                    item(ui, &items, "clear_guides", &mut action);
                                });
                            });
                        });
                        menu_bar_button(ui, tr("Plugins"), |ui| {
                            item(ui, &items, "plugins", &mut action);
                            item(ui, &items, "install_plugin", &mut action);
                            ui.add_enabled_ui(!developing, |ui| {
                                plugin_items(
                                    ui,
                                    plugin_menu.get(&Menu::Plugins),
                                    &mut plugin_action,
                                );
                            });
                        });
                        menu_bar_button(ui, tr("Window"), |ui| {
                            ui.add_enabled_ui(!developing, |ui| {
                                for (id, title, shown) in &pane_entries {
                                    let mut shown = *shown;
                                    if widgets::checkbox(ui, &mut shown, title).changed() {
                                        pane_toggle = Some((id.clone(), !shown));
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                item(ui, &items, "reset_panels", &mut action);
                            });
                        });
                        menu_bar_button(ui, tr("Help"), |ui| {
                            item(ui, &items, "command_palette", &mut action);
                            item(ui, &items, "shortcuts", &mut action);
                            item(ui, &items, "about", &mut action);
                        });
                    });
                    self.trailing_window_controls(ui);
                });
        });
        if let Some((id, hidden)) = pane_toggle {
            self.config.panes.set_hidden(&id, hidden);
            self.save_config();
        }
        if let Some((plugin, action)) = plugin_action {
            self.start_plugin_action(&plugin, &action);
        }
        if let Some(action) = action {
            self.run_command(action);
        }
        if let Some(adjustment) = adjustment {
            self.start_adjustment(adjustment, adjustment_layer);
        }
        if let Some(filter) = filter {
            if filter_layer {
                self.start_filter_layer(filter);
            } else {
                self.start_filter(filter);
            }
        }
    }

    pub(super) fn tabs(&mut self, ctx: &egui::Context) {
        let mut action = None;
        let mut switch = None;
        let mut close_project = None;
        let mut raw_switch = None;
        let mut raw_close = None;
        let mut copy = None;
        let developing = self.develop.is_some();
        let blocked = self.job.is_some()
            || self.dialog.is_some()
            || self.error.is_some()
            || self.close_app
            || self.close_tab.is_some()
            || self.develop_close_requested.is_some();
        egui::TopBottomPanel::top("project_tabs")
            .exact_height(46.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::TITLEBAR)
                    .inner_margin(egui::Margin::symmetric(14, 9)),
            )
            .show(ctx, |ui| {
                ui.add_enabled_ui(!blocked, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add(widgets::Button::new("+").min_size(egui::vec2(28.0, 28.0)))
                            .on_hover_text(match self.keymap.shortcut("new") {
                                shortcut if shortcut.is_empty() => tr("New canvas").to_owned(),
                                shortcut => format!("{} ({shortcut})", tr("New canvas")),
                            })
                            .clicked()
                        {
                            action = Some("new");
                        }
                        ui.separator();
                        let available = (ui.available_width() - 228.0).max(150.0);
                        ui.allocate_ui(egui::vec2(available, 28.0), |ui| {
                            egui::ScrollArea::horizontal()
                                .id_salt("tabs_scroll")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .show(ui, |ui| {
                                    ui.horizontal_centered(|ui| {
                                        if self.sessions.is_empty()
                                            && !developing
                                            && self.inactive_develop.is_empty()
                                        {
                                            ui.label(
                                                RichText::new(tr("Untitled")).color(theme::MUTED),
                                            );
                                        }
                                        for (index, session) in self.sessions.iter().enumerate() {
                                            let (response, close) = widgets::project_tab(
                                                ui,
                                                &session.title,
                                                session.history.dirty(),
                                                !developing && self.current == index,
                                            );
                                            if !developing {
                                                if let Some(layer) = response
                                                    .dnd_release_payload::<super::LayerDrag>()
                                                {
                                                    copy = Some((*layer, index));
                                                }
                                                if response
                                                    .dnd_hover_payload::<super::LayerDrag>()
                                                    .is_some()
                                                {
                                                    ui.painter().rect_stroke(
                                                        response.rect,
                                                        theme::BUTTON_RADIUS,
                                                        egui::Stroke::new(2.0_f32, theme::ACCENT),
                                                        egui::StrokeKind::Inside,
                                                    );
                                                }
                                            }
                                            if response.clicked() {
                                                switch = Some(index);
                                            }
                                            if close
                                                || response.clicked_by(egui::PointerButton::Middle)
                                            {
                                                close_project = Some(index);
                                            }
                                        }
                                        let mut raw_tabs: Vec<_> = self
                                            .develop
                                            .iter()
                                            .chain(&self.inactive_develop)
                                            .collect();
                                        raw_tabs.sort_by_key(|d| d.opened);
                                        for develop in raw_tabs {
                                            let selected = self
                                                .develop
                                                .as_ref()
                                                .is_some_and(|d| d.id == develop.id);
                                            let dirty =
                                                develop.asset.as_ref().is_some_and(|asset| {
                                                    asset.settings != develop.settings
                                                });
                                            let (response, close) = ui
                                                .push_id(develop.id, |ui| {
                                                    widgets::project_tab(
                                                        ui,
                                                        &format!("{} · RAW", develop.title),
                                                        dirty,
                                                        selected,
                                                    )
                                                })
                                                .inner;
                                            if response.clicked() {
                                                raw_switch = Some(develop.id);
                                            }
                                            if close
                                                || response.clicked_by(egui::PointerButton::Middle)
                                            {
                                                raw_close = Some(develop.id);
                                            }
                                        }
                                    });
                                });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_enabled_ui(
                                self.develop.as_ref().is_none_or(|d| d.ready()),
                                |ui| {
                                    if widgets::button(ui, "−")
                                        .on_hover_text(tr("Zoom out"))
                                        .clicked()
                                    {
                                        action = Some("zoom_out");
                                    }
                                    if widgets::button(ui, "+")
                                        .on_hover_text(tr("Zoom in"))
                                        .clicked()
                                    {
                                        action = Some("zoom_in");
                                    }
                                    if widgets::button(ui, "100%").clicked() {
                                        action = Some("actual");
                                    }
                                    if widgets::button(ui, tr("Fit")).clicked() {
                                        action = Some("fit");
                                    }
                                },
                            );
                        });
                    });
                });
            });
        if let Some((layer, destination)) = copy {
            self.copy_layer_to_project(layer, destination);
        }
        if let Some(index) = switch {
            self.cancel_gesture();
            self.suspend_develop();
            self.current = index;
            self.mask_target = false;
        }
        if let Some(id) = raw_switch.or(raw_close) {
            self.activate_develop(id);
        }
        if raw_close.is_some() {
            self.request_develop_close(super::develop::DevelopClose::Tab);
        }
        if let Some(index) = close_project {
            self.request_project_close(index);
        }
        if let Some(action) = action {
            self.command(action);
        }
    }
}

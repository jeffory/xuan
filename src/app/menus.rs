use super::theme::PaletteExt as _;

use egui::Button;
use xuan::i18n::tr;
use xuan::{
    document::{Adjustment, Point},
    effects::Filter,
};

use super::{
    EditorApp,
    commands::Chord,
    menu_model::{MenuAction, MenuEntry, MenuItem, MenuLayout},
    theme, widgets,
};

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

/// A menu row: the label and a right-aligned shortcut hint.
///
/// The hint sits on the accent fill while the row is highlighted, where egui's weak text colour
/// is unreadable, so its colour follows the row's state. That state is only known once the
/// button is added: it is read from the button's last response, with a repaint if it has
/// changed since.
pub(super) fn item_button(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    shortcut: &str,
) -> egui::Response {
    // Without a shortcut the accessible label is just the name, with no trailing space.
    if shortcut.is_empty() {
        return ui.add_enabled(enabled, Button::new(label));
    }
    let lit = |response: &egui::Response| {
        response.hovered()
            || response.has_focus()
            || response.highlighted()
            || response.is_pointer_button_down_on()
    };
    let id = ui.next_auto_id();
    let was_lit = enabled && ui.ctx().read_response(id).is_some_and(|r| lit(&r));
    let p = ui.palette();
    let hint = if was_lit { p.on_accent_muted } else { p.muted };
    let button = Button::new(label).shortcut_text(egui::RichText::new(shortcut).color(hint));
    let response = ui.add_enabled(enabled, button);
    if enabled && lit(&response) != was_lit {
        ui.ctx().request_repaint();
    }
    response
}

/// Draws a menu's entries and records the item chosen.
fn draw_entries(ui: &mut egui::Ui, entries: &[MenuEntry], chosen: &mut Option<MenuAction>) {
    for entry in entries {
        match entry {
            MenuEntry::Item(item) => draw_item(ui, item, chosen),
            MenuEntry::Submenu {
                label,
                enabled,
                entries,
            } => {
                ui.add_enabled_ui(*enabled, |ui| {
                    ui.menu_button(label.as_str(), |ui| draw_entries(ui, entries, chosen));
                });
            }
            MenuEntry::Separator => {
                ui.separator();
            }
            MenuEntry::Placeholder(text) => {
                ui.add_enabled(false, Button::new(text.as_str()));
            }
            // AppKit's own items exist only in the macOS menu bar.
            MenuEntry::System { .. } => {}
        }
    }
}

/// Draws one item: a toggle with its tick, a panel's checkbox, or a row with its shortcut hint.
fn draw_item(ui: &mut egui::Ui, item: &MenuItem, chosen: &mut Option<MenuAction>) {
    let shortcut = item.chord.map(Chord::label).unwrap_or_default();
    let pane = matches!(item.action, MenuAction::Pane { .. });
    let response = match item.checked {
        Some(mut checked) if pane => {
            ui.add_enabled_ui(item.enabled, |ui| {
                widgets::checkbox(ui, &mut checked, &item.label)
            })
            .inner
        }
        Some(checked) => {
            ui.add_enabled_ui(item.enabled, |ui| {
                widgets::menu_check(ui, checked, &item.label, &shortcut)
            })
            .inner
        }
        None => item_button(ui, item.enabled, &item.label, &shortcut),
    };
    let response = match &item.hover {
        Some(hover) => response.on_hover_text(hover).on_disabled_hover_text(hover),
        None => response,
    };
    let picked = if pane {
        response.changed()
    } else {
        response.clicked()
    };
    if picked {
        *chosen = Some(item.action.clone());
        // Snap To stays open, so several targets can be set in one go.
        if !matches!(item.action, MenuAction::Snap(_)) {
            ui.close();
        }
    }
}

pub(super) fn filter_menu(ui: &mut egui::Ui) -> Option<Filter> {
    let mut result = None;
    for filter in Filter::defaults() {
        if ui.button(tr(filter.name())).clicked() {
            result = Some(filter);
            ui.close();
        }
    }
    result
}

/// What an adjustment menu chose: an adjustment to set up, or a Color Lookup, whose table
/// is chosen first.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum AdjustmentChoice {
    Settings(Adjustment),
    ColorLookup,
}

/// The entries of an adjustment menu, in order, with their labels: each adjustment, then
/// Colour Lookup….
pub(super) fn adjustment_choices() -> Vec<(String, AdjustmentChoice)> {
    let mut choices: Vec<(String, AdjustmentChoice)> = [
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
    ]
    .into_iter()
    .map(|adjustment| {
        (
            tr(adjustment.name()).to_owned(),
            AdjustmentChoice::Settings(adjustment),
        )
    })
    .collect();
    choices.push((
        tr("Colour Lookup…").to_owned(),
        AdjustmentChoice::ColorLookup,
    ));
    choices
}

pub(super) fn adjustment_menu(ui: &mut egui::Ui) -> Option<AdjustmentChoice> {
    let mut result = None;
    for (label, choice) in adjustment_choices() {
        if ui.button(label).clicked() {
            result = Some(choice);
            ui.close();
        }
    }
    result
}

impl EditorApp {
    pub(super) fn menus(&mut self, ctx: &egui::Context) {
        let bar = self.menu_bar(MenuLayout::InWindow);
        let enabled = bar.menus.iter().all(|menu| menu.enabled);
        let mut chosen = None;
        super::chrome::title_bar(&ctx.palette(), self.window_corner_radius(ctx), "menubar").show(
            ctx,
            |ui| {
                egui::MenuBar::new()
                    .config(egui::containers::menu::MenuConfig::new().style({
                        let palette = ui.palette();
                        move |style: &mut egui::Style| theme::menu_style(style, &palette)
                    }))
                    .ui(ui, |ui| {
                        self.leading_window_controls(ui);
                        ui.add_enabled_ui(enabled, |ui| {
                            // Leave a small gap between the expanded highlights.
                            ui.spacing_mut().item_spacing.x += 2.0;
                            for menu in &bar.menus {
                                menu_bar_button(ui, &menu.label, |ui| {
                                    draw_entries(ui, &menu.entries, &mut chosen);
                                });
                            }
                        });
                        self.trailing_window_controls(ui);
                    });
            },
        );
        if let Some(action) = chosen {
            self.run_menu_action(ctx, action);
        }
    }
}

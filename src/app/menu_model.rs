//! The menus as data: what each menu holds, whether each item can run, and what choosing it
//! does. `menus.rs` draws this list inside the window. Choices run through
//! [`EditorApp::run_menu_action`].

use std::{collections::HashMap, path::PathBuf};

use xuan::{
    channels::Channels, effects::Filter, i18n::tr, layout::SnapSettings, plugins::manifest::Menu,
};

use super::{
    EditorApp,
    commands::{self, Chord},
    menus::{AdjustmentChoice, adjustment_choices},
    plugins::PluginMenuItem,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct MenuBar {
    pub menus: Vec<TopMenu>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct TopMenu {
    pub label: String,
    /// Whether the menu opens at all: not while a job, a dialog or a prompt is up. Its items
    /// are greyed then too.
    pub enabled: bool,
    pub entries: Vec<MenuEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum MenuEntry {
    Item(MenuItem),
    Submenu {
        label: String,
        enabled: bool,
        entries: Vec<MenuEntry>,
    },
    Separator,
    /// Greyed text, such as "No recent files".
    Placeholder(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MenuItem {
    pub label: String,
    /// Already includes every condition around the item.
    pub enabled: bool,
    /// A toggle's tick, or `None` for a plain item.
    pub checked: Option<bool>,
    /// The first binding: the hint egui shows, and the macOS key equivalent.
    pub chord: Option<Chord>,
    /// The tool tip in the window's menus: a plugin's source, a recent file's whole path.
    pub hover: Option<String>,
    pub action: MenuAction,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum MenuAction {
    Command(&'static str),
    Plugin {
        plugin: String,
        action: String,
    },
    Recent(PathBuf),
    Adjustment {
        choice: AdjustmentChoice,
        layer: bool,
    },
    Filter {
        filter: Filter,
        layer: bool,
    },
    /// Shows or hides a panel.
    Pane {
        id: String,
        hidden: bool,
    },
    /// Turns one View → Snap To target over.
    Snap(SnapTarget),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnapTarget {
    Guides,
    Grid,
    Layers,
    Bounds,
}

impl SnapTarget {
    fn flag(self, snap: &mut SnapSettings) -> &mut bool {
        match self {
            Self::Guides => &mut snap.guides,
            Self::Grid => &mut snap.grid,
            Self::Layers => &mut snap.layers,
            Self::Bounds => &mut snap.bounds,
        }
    }
}

/// What every menu needs to know this frame.
struct State<'a> {
    /// Nothing blocks the menus.
    enabled: bool,
    developing: bool,
    /// A document is open, outside Develop.
    has_doc: bool,
    plugins: HashMap<Menu, Vec<PluginMenuItem>>,
    recent: &'a [(PathBuf, bool)],
}

/// Builds one menu's entries. `enabled` is the AND of every condition around the entries
/// being added.
struct Entries<'a> {
    app: &'a EditorApp,
    enabled: bool,
    entries: Vec<MenuEntry>,
}

impl<'a> Entries<'a> {
    fn new(app: &'a EditorApp, enabled: bool) -> Self {
        Self {
            app,
            enabled,
            entries: Vec::new(),
        }
    }

    /// A registry command, with the registry's label.
    fn command(&mut self, id: &'static str) {
        let label = commands::find(id).map_or(id, |c| tr(c.label));
        self.labelled(label, id);
    }

    /// A registry command, with a label of its own.
    fn labelled(&mut self, label: &str, id: &'static str) {
        let item = self.command_item(label, id, None);
        self.entries.push(MenuEntry::Item(item));
    }

    /// A toggle command with its tick, run as a command so the key chord does the same thing.
    fn check(&mut self, checked: bool, label: Option<&str>, id: &'static str) {
        let label = label.unwrap_or_else(|| commands::find(id).map_or(id, |c| tr(c.label)));
        let item = self.command_item(label, id, Some(checked));
        self.entries.push(MenuEntry::Item(item));
    }

    fn command_item(&self, label: &str, id: &'static str, checked: Option<bool>) -> MenuItem {
        MenuItem {
            label: label.to_owned(),
            enabled: self.enabled && self.app.command_enabled(id),
            checked,
            chord: self.app.keymap.keys(id).first().copied(),
            hover: None,
            action: MenuAction::Command(id),
        }
    }

    /// An item that is not a registry command, so it has no shortcut.
    fn action(&mut self, label: &str, checked: Option<bool>, action: MenuAction) {
        self.entries.push(MenuEntry::Item(MenuItem {
            label: label.to_owned(),
            enabled: self.enabled,
            checked,
            chord: None,
            hover: None,
            action,
        }));
    }

    fn separator(&mut self) {
        self.entries.push(MenuEntry::Separator);
    }

    /// Entries added by `add` also need `condition`.
    fn when(&mut self, condition: bool, add: impl FnOnce(&mut Self)) {
        let outer = self.enabled;
        self.enabled &= condition;
        add(self);
        self.enabled = outer;
    }

    /// A submenu that opens only when `condition` holds; its entries need it too.
    fn submenu(&mut self, label: &str, condition: bool, add: impl FnOnce(&mut Self)) {
        let mut inner = Entries::new(self.app, self.enabled && condition);
        add(&mut inner);
        self.entries.push(MenuEntry::Submenu {
            label: label.to_owned(),
            enabled: inner.enabled,
            entries: inner.entries,
        });
    }

    /// Entries plugins added to a menu, after a separator.
    fn plugins(&mut self, items: Option<&Vec<PluginMenuItem>>) {
        let Some(items) = items.filter(|items| !items.is_empty()) else {
            return;
        };
        self.separator();
        for item in items {
            let id = format!("{}/{}", item.plugin, item.action);
            self.entries.push(MenuEntry::Item(MenuItem {
                label: item.label.clone(),
                enabled: self.enabled && item.enabled,
                checked: None,
                chord: self.app.keymap.keys(&id).first().copied(),
                hover: Some(item.source.clone()),
                action: MenuAction::Plugin {
                    plugin: item.plugin.clone(),
                    action: item.action.clone(),
                },
            }));
        }
    }

    /// The entries of File → Open Recent: a shortened path each, greyed when the file is gone.
    fn recent(&mut self, recent: &[(PathBuf, bool)]) {
        if recent.is_empty() {
            self.entries
                .push(MenuEntry::Placeholder(tr("No recent files").to_owned()));
            return;
        }
        for (path, exists) in recent {
            let full = super::recent::safe_path(path);
            let hover = if *exists {
                full
            } else {
                format!("{full}\n{}", tr("File not found"))
            };
            self.entries.push(MenuEntry::Item(MenuItem {
                label: super::recent::elide_path(path, super::recent::LABEL_CHARS),
                enabled: self.enabled && *exists,
                checked: None,
                chord: None,
                hover: Some(hover),
                action: MenuAction::Recent(path.clone()),
            }));
        }
    }

    /// Each adjustment, then Colour Lookup…, as an adjustment layer when `layer`.
    fn adjustments(&mut self, layer: bool) {
        for (label, choice) in adjustment_choices() {
            self.action(&label, None, MenuAction::Adjustment { choice, layer });
        }
    }

    /// The filters, each opening its dialog (`…`), or as filter layers when `layer`.
    fn filters(&mut self, layer: bool) {
        for filter in Filter::defaults() {
            let label = if layer {
                tr(filter.name()).to_owned()
            } else {
                format!("{}…", tr(filter.name()))
            };
            self.action(&label, None, MenuAction::Filter { filter, layer });
        }
    }

    fn finish(self) -> Vec<MenuEntry> {
        self.entries
    }
}

impl EditorApp {
    /// The menus as the menu bar shows them this frame.
    pub(super) fn menu_bar(&mut self) -> MenuBar {
        let recent = self.recent_status();
        self.build_menu_bar(&recent)
    }

    fn build_menu_bar(&self, recent: &[(PathBuf, bool)]) -> MenuBar {
        let developing = self.develop.is_some();
        let blocked = self.job.is_some()
            || self.dialog.is_some()
            || self.close_app
            || self.develop_close_requested.is_some()
            || self.close_tab.is_some();
        let state = State {
            enabled: !blocked,
            developing,
            has_doc: self.session().is_some() && !developing,
            plugins: self.plugin_menu_items(),
            recent,
        };
        let menu = |label: &str, entries: Vec<MenuEntry>| TopMenu {
            label: label.to_owned(),
            enabled: state.enabled,
            entries,
        };
        MenuBar {
            menus: vec![
                menu(tr("File"), self.file_entries(&state)),
                menu(tr("Edit"), self.edit_entries(&state)),
                menu(tr("Image"), self.image_entries(&state)),
                menu(tr("Layer"), self.layer_entries(&state)),
                menu(tr("Select"), self.select_entries(&state)),
                menu(tr("Filter"), self.filter_entries(&state)),
                menu(tr("View"), self.view_entries(&state)),
                menu(tr("Plugins"), self.plugins_entries(&state)),
                menu(tr("Window"), self.window_entries(&state)),
                menu(tr("Help"), self.help_entries(&state)),
            ],
        }
    }

    fn file_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.command("new");
        e.command("new_collage");
        e.command("open");
        e.command("open_clipboard");
        e.command("open_comp");
        e.when(!state.developing, |e| e.command("import"));
        e.submenu(tr("Open Recent"), self.command_enabled("open"), |e| {
            e.recent(state.recent);
            e.separator();
            e.command("clear_recent");
        });
        e.separator();
        e.when(state.has_doc, |e| {
            e.command("save");
            e.command("save_as");
            e.command("export");
            e.separator();
            e.command("close");
        });
        if state.developing {
            e.labelled(tr("Close RAW Develop"), "close");
        }
        e.when(state.has_doc, |e| e.plugins(state.plugins.get(&Menu::File)));
        e.separator();
        e.command("quit");
        e.finish()
    }

    fn edit_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.command("settings");
        e.plugins(state.plugins.get(&Menu::Edit));
        e.separator();
        let (undo, redo) = if let Some(d) = &self.develop {
            (
                (d.ready() && !d.undo.is_empty()).then_some(tr("RAW adjustment")),
                (d.ready() && !d.redo.is_empty()).then_some(tr("RAW adjustment")),
            )
        } else {
            (
                self.session().and_then(|s| s.history.undo_name()),
                self.session().and_then(|s| s.history.redo_name()),
            )
        };
        e.when(undo.is_some(), |e| {
            e.labelled(
                &format!("{} {}", tr("Undo"), tr(undo.unwrap_or(""))),
                "undo",
            );
        });
        e.when(redo.is_some(), |e| {
            e.labelled(
                &format!("{} {}", tr("Redo"), tr(redo.unwrap_or(""))),
                "redo",
            );
        });
        e.separator();
        e.when(state.has_doc, |e| {
            e.command("cut");
            e.command("copy");
            e.command("copy_merged");
        });
        e.when(!state.developing, |e| e.command("paste"));
        e.when(state.has_doc, |e| {
            e.separator();
            for id in ["fill_fg", "fill_bg", "clear", "stroke", "content_fill"] {
                e.command(id);
            }
        });
        e.finish()
    }

    fn image_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.when(state.has_doc, |e| {
            e.submenu(tr("Adjustments"), true, |e| {
                e.adjustments(false);
                e.separator();
                e.command("invert");
            });
            e.separator();
            for id in [
                "image_size",
                "canvas_size",
                "trim",
                "collage_layout",
                "crop_to_selection",
            ] {
                e.command(id);
            }
            e.separator();
            e.submenu(tr("Rotate Canvas"), true, |e| {
                for id in ["rotate_canvas_cw", "rotate_canvas_ccw", "rotate_canvas_180"] {
                    e.command(id);
                }
            });
            e.command("flip_canvas_h");
            e.command("flip_canvas_v");
            e.plugins(state.plugins.get(&Menu::Image));
        });
        e.finish()
    }

    fn layer_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        let active = self.session().and_then(|s| s.document.active());
        let raw_layer = active.is_some_and(|layer| layer.raw.is_some() && !layer.locked);
        let can_rerun = active.is_some_and(|layer| layer.generated.is_some());
        e.when(state.has_doc, |e| {
            e.command("new_layer");
            e.command("duplicate");
            e.command("delete_layer");
            e.when(raw_layer, |e| {
                e.command("develop");
                e.command("rasterize_raw");
            });
            e.command("layer_effects");
            e.separator();
            for id in ["group", "ungroup", "merge", "flatten"] {
                e.command(id);
            }
            e.separator();
            e.submenu(tr("New Adjustment Layer"), true, |e| e.adjustments(true));
            e.submenu(tr("New Filter Layer"), true, |e| e.filters(true));
            e.command("adjustment_presets");
            e.submenu(tr("Layer Mask"), true, |e| {
                e.command("new_mask_layer");
                e.command("mask");
                e.labelled(tr("Enable / Disable"), "disable_mask");
                e.labelled(tr("Link / Unlink"), "link_mask");
                e.command("delete_mask");
            });
            e.command("clip");
            e.separator();
            e.command("flip_h");
            e.command("flip_v");
            e.separator();
            e.when(can_rerun, |e| e.command("rerun_plugin"));
            e.plugins(state.plugins.get(&Menu::Layer));
        });
        e.finish()
    }

    fn select_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.when(state.has_doc, |e| {
            e.labelled(tr("All"), "select_all");
            e.command("deselect");
            e.labelled(tr("Inverse"), "invert_selection");
            e.labelled(tr("Layer's Pixels"), "select_layer_pixels");
            e.labelled(tr("Mask's Black Areas"), "select_mask_black");
            e.command("load_channel_selection");
            e.labelled(tr("Subject"), "select_subject");
            e.command("color_range");
            e.command("load_selection");
            e.separator();
            e.labelled(tr("Expand…"), "expand_selection");
            e.labelled(tr("Contract…"), "contract_selection");
            e.command("feather");
            e.separator();
            e.command("paths");
            e.plugins(state.plugins.get(&Menu::Select));
        });
        e.finish()
    }

    fn filter_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.when(state.has_doc, |e| {
            e.command("remove_background");
            e.command("remove_flat_background");
            e.separator();
            e.command("camera_raw_filter");
            e.separator();
            e.filters(false);
            e.plugins(state.plugins.get(&Menu::Filter));
        });
        e.finish()
    }

    fn view_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        let can_view = self.develop.as_ref().is_none_or(|d| d.ready());
        e.when(can_view, |e| {
            for id in ["fit", "actual", "zoom_in", "zoom_out"] {
                e.command(id);
            }
        });
        let targets = self.session().map_or(Channels::ALL, |s| s.channel_targets);
        e.submenu(tr("Channels"), state.has_doc, |e| {
            for id in [
                "channel_composite",
                "channel_red",
                "channel_green",
                "channel_blue",
                "channel_alpha",
            ] {
                let checked = match super::channel_pane::row(id) {
                    Some(None) => targets == Channels::ALL,
                    Some(Some(channel)) => targets.single() == Some(channel),
                    None => false,
                };
                e.check(checked, None, id);
            }
        });
        e.separator();
        e.check(self.config.pixel_grid, None, "toggle_pixel_grid");
        e.check(self.show_controls, None, "toggle_controls");
        // Rulers, grid, guides and snapping, as upstream's View menu.
        e.separator();
        let snap = self.config.snap;
        let has_guides = self
            .session()
            .is_some_and(|s| !s.document.guides.is_empty());
        e.when(state.has_doc, |e| {
            e.submenu(tr("Show"), true, |e| {
                e.check(self.config.show_grid, Some(tr("Grid")), "toggle_grid");
                e.check(self.config.show_guides, Some(tr("Guides")), "toggle_guides");
            });
            e.command("grid_settings");
            e.check(self.config.rulers, None, "toggle_rulers");
            e.separator();
            e.check(snap.enabled, None, "toggle_snap");
            e.submenu(tr("Snap To"), true, |e| {
                e.when(snap.enabled, |e| {
                    for (target, value, label) in [
                        (SnapTarget::Guides, snap.guides, tr("Guides")),
                        (SnapTarget::Grid, snap.grid, tr("Grid")),
                        (SnapTarget::Layers, snap.layers, tr("Layers")),
                        (SnapTarget::Bounds, snap.bounds, tr("Document Bounds")),
                    ] {
                        e.action(label, Some(value), MenuAction::Snap(target));
                    }
                });
            });
            e.separator();
            e.check(self.config.lock_guides, None, "lock_guides");
            e.when(has_guides, |e| e.command("clear_guides"));
        });
        e.finish()
    }

    fn plugins_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.command("plugins");
        e.command("install_plugin");
        e.when(!state.developing, |e| {
            e.plugins(state.plugins.get(&Menu::Plugins));
        });
        e.finish()
    }

    fn window_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.when(!state.developing, |e| {
            for (id, title, shown) in self.pane_entries() {
                e.action(&title, Some(shown), MenuAction::Pane { id, hidden: shown });
            }
            e.separator();
            e.command("reset_panels");
        });
        e.finish()
    }

    fn help_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.command("command_palette");
        e.command("shortcuts");
        e.separator();
        e.command("check_updates");
        e.command("about");
        e.finish()
    }

    /// Does what a menu item stands for.
    pub(super) fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Command(id) => self.run_command(id),
            MenuAction::Plugin { plugin, action } => self.start_plugin_action(&plugin, &action),
            MenuAction::Recent(path) => self.open_recent(&path),
            MenuAction::Adjustment { choice, layer } => self.choose_adjustment(choice, layer),
            MenuAction::Filter {
                filter,
                layer: true,
            } => self.start_filter_layer(filter),
            MenuAction::Filter {
                filter,
                layer: false,
            } => self.start_filter(filter),
            MenuAction::Pane { id, hidden } => {
                self.config.panes.set_hidden(&id, hidden);
                self.save_config();
            }
            MenuAction::Snap(target) => self.set_view_option(|config| {
                let flag = target.flag(&mut config.snap);
                *flag = !*flag;
            }),
        }
    }
}

#[cfg(test)]
impl MenuBar {
    /// Every item, in every menu and submenu, in order.
    pub(super) fn items(&self) -> Vec<&MenuItem> {
        fn walk<'a>(entries: &'a [MenuEntry], items: &mut Vec<&'a MenuItem>) {
            for entry in entries {
                match entry {
                    MenuEntry::Item(item) => items.push(item),
                    MenuEntry::Submenu { entries, .. } => walk(entries, items),
                    _ => {}
                }
            }
        }
        let mut items = Vec::new();
        for menu in &self.menus {
            walk(&menu.entries, &mut items);
        }
        items
    }

    /// The first item, in any menu or submenu, that runs this command.
    pub(super) fn find_command(&self, id: &str) -> Option<&MenuItem> {
        self.items()
            .into_iter()
            .find(|item| matches!(item.action, MenuAction::Command(command) if command == id))
    }
}

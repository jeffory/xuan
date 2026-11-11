# macOS Native Menu Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On macOS, show Xuan's menus in the system menu bar and put the document tabs in the
title row, while Windows and Linux keep the in-window menus unchanged.

**Architecture:** A new menu list (`menu_model.rs`) describes every menu as plain data, built each
frame from the app state. The egui renderer (`menus.rs`) draws it in the window; on macOS a muda
renderer (`macos_menu.rs`) shows it in the menu bar, using two pure helpers that are tested on
every platform: `menu_sync.rs` (what to change each frame) and `menu_keys.rs` (key equivalents,
and giving a key the menu took back to egui). Both renderers end in `run_menu_action`.

**Tech Stack:** Rust 1.99 (`rust-toolchain.toml`), egui/eframe 0.33.3, winit 0.30, egui_kittest,
muda 0.20.0, objc2 0.6 and objc2-app-kit 0.3 (macOS only).

**Spec:** `docs/superpowers/specs/2026-10-13-macos-native-menu-design.md`

## Global Constraints

- muda is pinned exactly: `muda = { version = "=0.20.0", default-features = false }`, in
  `[target.'cfg(target_os = "macos")'.dependencies]` only.
- objc2-app-kit is added for macOS only, with `default-features = false` and the features
  `std`, `NSApplication`, `NSEvent`, `NSResponder`. It is already in `Cargo.lock`.
- The only new crates in `Cargo.lock` are `muda`, `crossbeam-channel` and `keyboard-types`. Any
  other new crate stops the task.
- Windows and Linux behave exactly as before: same menus, same title row, same tab bar.
- ⌘M stays on Curves. Minimize has no key equivalent.
- On macOS Compact with the system menu bar, the tab bar is the top row and 32 px tall
  (`MAC_TITLE_HEIGHT = 32.0`). `TAB_HEIGHT` stays 36 px everywhere else.
- Every `tr("…")` string must be in `assets/locales/en.tsv`, and every key there must still be a
  string in `src/` (`src/i18n/source_keys.rs` checks both).
- `cargo clippy --locked --all-targets -- -D warnings` must pass on this Mac after every task.
  Linux CI runs the same command, so code used only on macOS needs the `cfg` attributes the tasks
  give.
- Conventional commit messages, with no "Fix #N" or other closing keywords (`AGENTS.md`).
- Commits are signed through 1Password. If signing fails with "failed to fill whole buffer",
  run the same `git commit` again. Never use `--no-gpg-sign`.
- Written docs use Australian English and no en or em dashes.
- Work stays on branch `feat/tabs-above-tool-options`. Do not push.

## Review Focus

1. A label with `&` (the adjustment "Black & White", a plugin's label) must show the `&` in the
   Mac menu. muda reads `&` as a mnemonic marker and drops it. Pinned by Task 3's `menu_text`
   test.
2. A toggle chosen from the Mac menu whose command then does not run (for example from its key
   while a text field has focus) must keep the true tick. muda turns a check item's tick over on
   its own. Pinned by the `recheck` logic and the manual check in Task 5, step 10.
3. ⌘V with only an image on the clipboard must still paste the image. The patched egui-winit
   sends an empty `Event::Paste`, and the replay must do the same. Pinned by Task 4's replay
   test.
4. Changing the language while Xuan runs must relabel the Mac menus, including AppKit's own
   items, without a restart. Pinned by Task 3's in-place test and Task 5, step 10.
5. In full screen, macOS hides the window buttons, so the tabs must move to the left edge.
   Pinned by Task 6's title row test.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/app/menu_model.rs` (new) | The menu list as data, built from the app state; `run_menu_action`. |
| `src/app/menus.rs` (changed) | Draws the list with egui; keeps `menu_bar_button`, `item_button`, `adjustment_menu`, `filter_menu`. |
| `src/app/menu_sync.rs` (new, macOS and tests) | `diff` (what the Mac menu must change) and `menu_text`. |
| `src/app/menu_keys.rs` (new, macOS and tests) | Key equivalents, `route` and `replay`. |
| `src/app/macos_menu.rs` (new, macOS only) | The muda menu, its events and its per-frame sync. |
| `src/app/chrome.rs` (changed) | `MAC_TITLE_HEIGHT`, `tabs_in_title_row`, `title_row_inset`, `title_row_gesture`. |
| `src/app/tabs.rs` (changed) | The tab bar as the title row on macOS. |
| `src/app/mod.rs` (changed) | Module declarations, two fields, startup and frame wiring. |
| `src/app/tests/menu_bar.rs`, `menu_changes.rs`, `mac_keys.rs`, `ui_mac_keys.rs` (new) | Tests. |

---

### Task 1: The menu list, drawn by the egui menus

Moves what the menus contain out of `menus.rs` into a data list, and makes the egui menus draw
that list. The in-window menus must look and behave exactly as before.

**Files:**
- Create: `src/app/menu_model.rs`
- Modify: `src/app/menus.rs` (lines 1-12 imports, 81-184 helpers, 197-244 adjustments, 246-752 `menus`)
- Modify: `src/app/mod.rs:38` (module list)
- Modify: `src/app/tests/keymap.rs:339-341`
- Create: `src/app/tests/menu_bar.rs`
- Modify: `src/app/tests.rs` (register the test module)

**Interfaces:**
- Produces:
  - `pub(super) struct MenuBar { pub menus: Vec<TopMenu> }` (derives `Clone, Debug, Default, PartialEq`)
  - `pub(super) struct TopMenu { pub label: String, pub enabled: bool, pub entries: Vec<MenuEntry> }`
  - `pub(super) enum MenuEntry { Item(MenuItem), Submenu { label: String, enabled: bool, entries: Vec<MenuEntry> }, Separator, Placeholder(String) }`
  - `pub(super) struct MenuItem { pub label: String, pub enabled: bool, pub checked: Option<bool>, pub chord: Option<Chord>, pub hover: Option<String>, pub action: MenuAction }`
  - `pub(super) enum MenuAction { Command(&'static str), Plugin { plugin: String, action: String }, Recent(PathBuf), Adjustment { choice: AdjustmentChoice, layer: bool }, Filter { filter: Filter, layer: bool }, Pane { id: String, hidden: bool }, Snap(SnapTarget) }`
  - `pub(super) enum SnapTarget { Guides, Grid, Layers, Bounds }`
  - `EditorApp::menu_bar(&mut self) -> MenuBar`
  - `EditorApp::run_menu_action(&mut self, action: MenuAction)`
  - Test only: `MenuBar::items(&self) -> Vec<&MenuItem>`, `MenuBar::find_command(&self, id: &str) -> Option<&MenuItem>`
  - `menus::adjustment_choices() -> Vec<(String, AdjustmentChoice)>`; `AdjustmentChoice` derives `Clone, Debug, PartialEq`

- [ ] **Step 1: Run the suite to record the starting point**

Run: `cargo test --locked 2>&1 | grep -E "^test result|FAILED|panicked" | head -20`
Expected: every `test result:` line says `0 failed`.

- [ ] **Step 2: Write the failing tests**

Create `src/app/tests/menu_bar.rs`:

```rust
//! The menu list (`menu_model.rs`): what each menu holds, and when its items can run.

use super::*;
use crate::app::menu_model::{MenuAction, MenuBar, MenuEntry, MenuItem, TopMenu};

/// A menu's entries as short lines: a command's id, `---` for a separator, `> Label` for a
/// submenu, `(text)` for a placeholder, and the label of any other item.
fn outline(entries: &[MenuEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry {
            MenuEntry::Item(MenuItem {
                action: MenuAction::Command(id),
                ..
            }) => (*id).to_owned(),
            MenuEntry::Item(item) => item.label.clone(),
            MenuEntry::Submenu { label, .. } => format!("> {label}"),
            MenuEntry::Separator => "---".to_owned(),
            MenuEntry::Placeholder(text) => format!("({text})"),
        })
        .collect()
}

fn menu<'a>(bar: &'a MenuBar, label: &str) -> &'a TopMenu {
    bar.menus
        .iter()
        .find(|menu| menu.label == label)
        .expect(label)
}

fn submenu<'a>(entries: &'a [MenuEntry], label: &str) -> &'a [MenuEntry] {
    entries
        .iter()
        .find_map(|entry| match entry {
            MenuEntry::Submenu {
                label: name,
                entries,
                ..
            } if name == label => Some(entries.as_slice()),
            _ => None,
        })
        .expect(label)
}

/// No separator first, last or next to another one, in any menu or submenu.
fn assert_tidy(entries: &[MenuEntry], name: &str) {
    let separator = |entry: Option<&MenuEntry>| matches!(entry, Some(MenuEntry::Separator));
    assert!(
        !separator(entries.first()) && !separator(entries.last()),
        "{name}: {:?}",
        outline(entries)
    );
    assert!(
        !entries
            .windows(2)
            .any(|pair| separator(pair.first()) && separator(pair.get(1))),
        "{name}: {:?}",
        outline(entries)
    );
    for entry in entries {
        if let MenuEntry::Submenu { label, entries, .. } = entry {
            assert_tidy(entries, label);
        }
    }
}

#[test]
fn the_menus_keep_their_order_and_contents() {
    let (_, mut app) = app_with_document();
    let bar = app.menu_bar();
    let labels: Vec<&str> = bar.menus.iter().map(|menu| menu.label.as_str()).collect();
    assert_eq!(
        labels,
        ["File", "Edit", "Image", "Layer", "Select", "Filter", "View", "Plugins", "Window", "Help"]
    );
    assert_eq!(
        outline(&menu(&bar, "File").entries),
        [
            "new", "new_collage", "open", "open_clipboard", "open_comp", "import",
            "> Open Recent", "---", "save", "save_as", "export", "---", "close", "---", "quit",
        ]
    );
    assert_eq!(
        outline(&menu(&bar, "Edit").entries),
        [
            "settings", "---", "undo", "redo", "---", "cut", "copy", "copy_merged", "paste",
            "---", "fill_fg", "fill_bg", "clear", "stroke", "content_fill",
        ]
    );
    assert_eq!(
        outline(submenu(&menu(&bar, "Layer").entries, "Layer Mask")),
        ["new_mask_layer", "mask", "disable_mask", "link_mask", "delete_mask"]
    );
    assert_eq!(
        outline(&menu(&bar, "Help").entries),
        ["command_palette", "shortcuts", "---", "check_updates", "about"]
    );
    let adjustments = outline(submenu(&menu(&bar, "Image").entries, "Adjustments"));
    assert_eq!(
        adjustments[adjustments.len() - 3..],
        ["Colour Lookup…", "---", "invert"]
    );
}

#[test]
fn items_without_a_document_are_greyed() {
    let (_, mut app) = app();
    let bar = app.menu_bar();
    assert!(bar.find_command("new").unwrap().enabled);
    assert!(!bar.find_command("save").unwrap().enabled);
    assert!(!bar.find_command("image_size").unwrap().enabled);
    assert!(matches!(
        menu(&bar, "Image").entries.first(),
        Some(MenuEntry::Submenu { enabled: false, .. })
    ));
}

#[test]
fn a_prompt_greys_every_menu() {
    let (_, mut app) = app_with_document();
    app.close_app = true;
    let bar = app.menu_bar();
    assert!(bar.menus.iter().all(|menu| !menu.enabled));
    assert!(bar.items().iter().all(|item| !item.enabled));
}

#[test]
fn undo_and_redo_name_the_step() {
    let (_, mut app) = app_with_document();
    let undo = app.menu_bar().find_command("undo").cloned().unwrap();
    assert_eq!((undo.label.as_str(), undo.enabled), ("Undo ", false));
    app.command("new_layer");
    let undo = app.menu_bar().find_command("undo").cloned().unwrap();
    assert!(
        undo.label.starts_with("Undo ") && undo.label.len() > "Undo ".len(),
        "{}",
        undo.label
    );
    assert!(undo.enabled);
}

#[test]
fn open_recent_lists_files_and_greys_missing_ones() {
    let directory = tempfile::tempdir().unwrap();
    let here = directory.path().join("here.png");
    std::fs::write(&here, b"").unwrap();
    let gone = directory.path().join("gone.png");
    let (_, mut app) = app();
    let bar = app.menu_bar();
    assert_eq!(
        outline(submenu(&menu(&bar, "File").entries, "Open Recent")),
        ["(No recent files)", "---", "clear_recent"]
    );

    app.config.recent_files = vec![here.clone(), gone.clone()];
    let bar = app.menu_bar();
    let entries = submenu(&menu(&bar, "File").entries, "Open Recent");
    let [MenuEntry::Item(first), MenuEntry::Item(second), ..] = entries else {
        panic!("{entries:?}");
    };
    assert_eq!((&first.action, first.enabled), (&MenuAction::Recent(here), true));
    assert_eq!((&second.action, second.enabled), (&MenuAction::Recent(gone), false));
    assert!(
        second
            .hover
            .as_deref()
            .is_some_and(|hover| hover.ends_with("File not found"))
    );
}

#[test]
fn toggles_show_their_ticks() {
    let (_, mut app) = app_with_document();
    app.config.rulers = true;
    app.config.snap.enabled = false;
    let bar = app.menu_bar();
    assert_eq!(bar.find_command("toggle_rulers").unwrap().checked, Some(true));
    assert_eq!(bar.find_command("toggle_snap").unwrap().checked, Some(false));
    assert_eq!(bar.find_command("channel_composite").unwrap().checked, Some(true));
    assert_eq!(bar.find_command("channel_red").unwrap().checked, Some(false));
    let snap_to = submenu(&menu(&bar, "View").entries, "Snap To");
    assert_eq!(snap_to.len(), 4);
    assert!(snap_to.iter().all(|entry| matches!(
        entry,
        MenuEntry::Item(MenuItem {
            enabled: false,
            checked: Some(_),
            action: MenuAction::Snap(_),
            ..
        })
    )));
}

#[test]
fn develop_offers_close_raw_develop_and_greys_the_editor_items() {
    let (context, mut app) = app();
    app.develop = Some(crate::app::develop::tests::ready(&context));
    let bar = app.menu_bar();
    let closes: Vec<&MenuItem> = menu(&bar, "File")
        .entries
        .iter()
        .filter_map(|entry| match entry {
            MenuEntry::Item(item) if item.action == MenuAction::Command("close") => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(closes.len(), 2);
    assert!(!closes[0].enabled);
    assert_eq!(closes[1].label, "Close RAW Develop");
    assert!(!bar.find_command("import").unwrap().enabled);
    assert!(!bar.find_command("reset_panels").unwrap().enabled);
}

#[test]
fn separators_never_start_end_or_double_up() {
    let (context, mut app) = app();
    for menu in app.menu_bar().menus {
        assert_tidy(&menu.entries, &menu.label);
    }
    app.dimensions = [20, 16];
    app.new_document();
    for menu in app.menu_bar().menus {
        assert_tidy(&menu.entries, &menu.label);
    }
    app.develop = Some(crate::app::develop::tests::ready(&context));
    for menu in app.menu_bar().menus {
        assert_tidy(&menu.entries, &menu.label);
    }
}
```

Register it in `src/app/tests.rs`, after the `update_check` module (around line 90):

```rust
#[path = "tests/menu_bar.rs"]
mod menu_bar;
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test --locked menu_bar 2>&1 | tail -5`
Expected: compile error, "unresolved import `crate::app::menu_model`".

- [ ] **Step 4: Share the adjustment list in `menus.rs`**

In `src/app/menus.rs`, replace `AdjustmentChoice` and `adjustment_menu` (lines 197-244) with:

```rust
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
    choices.push((tr("Colour Lookup…").to_owned(), AdjustmentChoice::ColorLookup));
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
```

- [ ] **Step 5: Write `src/app/menu_model.rs`**

```rust
//! The menus as data: what each menu holds, whether each item can run, and what choosing it
//! does. `menus.rs` draws this list inside the window. Choices run through
//! [`EditorApp::run_menu_action`].

use std::{collections::HashMap, path::PathBuf};

use xuan::{
    channels::Channels, effects::Filter, i18n::tr, layout::SnapSettings,
    plugins::manifest::Menu,
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
    Plugin { plugin: String, action: String },
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
    Pane { id: String, hidden: bool },
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
        let has_guides = self.session().is_some_and(|s| !s.document.guides.is_empty());
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
```

In `src/app/mod.rs`, add `mod menu_model;` after `mod menus;` (line 38).

- [ ] **Step 6: Run the model tests to see them pass**

Run: `cargo test --locked menu_bar 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: `test result: ok. 8 passed`. If the build fails because a field or method is private,
it is a typo: every field and method used here is already used by today's `menus.rs`. If
`separators_never_start_end_or_double_up` fails because Window starts with a separator (no panes
known in a bare app), bring Task 2's `tidy` (step 3.6) into this task.

- [ ] **Step 7: Make the egui menus draw the list**

In `src/app/menus.rs`:

1. Replace the imports (lines 1-12) with:

```rust
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
    menu_model::{MenuAction, MenuEntry, MenuItem},
    theme, widgets,
};
```

2. Delete `plugin_items`, `recent_items`, `MenuItems` and its `impl`, `item`, `labelled` and
   `check_item` (lines 81-184). In their place add:

```rust
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
```

3. Replace the whole `impl EditorApp { … menu_items … menus … }` block (lines 246-752) with:

```rust
impl EditorApp {
    pub(super) fn menus(&mut self, ctx: &egui::Context) {
        let bar = self.menu_bar();
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
            self.run_menu_action(action);
        }
    }
}
```

4. In `src/app/tests/keymap.rs`, replace lines 339-341:

```rust
    let bar = app.menu_bar();
    let merge = bar.find_command("merge").unwrap();
    assert_eq!(
        (merge.enabled, merge.chord.map(Chord::label).as_deref()),
        (true, Some("Ctrl+Shift+M"))
    );
    assert!(bar.find_command("invert_selection").unwrap().chord.is_none());
```

If `Chord` is not in scope there, add `use crate::app::commands::Chord;` at the top of the file.

- [ ] **Step 8: Run the whole suite**

Run: `cargo test --locked 2>&1 | grep -E "^test result|FAILED|panicked" | head -20`
Expected: every line says `0 failed`. The menu UI tests in `src/app/tests/ui.rs` (File, View,
recent files, plugin items) drive the new renderer, so a pass shows the in-window menus still
work. If one fails, compare the failing menu with the old code in `git show HEAD:src/app/menus.rs`
and fix the list, not the test.

- [ ] **Step 9: Lint**

Run: `cargo fmt && cargo clippy --locked --all-targets -- -D warnings 2>&1 | tail -3`
Expected: `Finished` with no warnings.

- [ ] **Step 10: Commit**

```bash
git add src/app/menu_model.rs src/app/menus.rs src/app/mod.rs src/app/tests.rs src/app/tests/menu_bar.rs src/app/tests/keymap.rs
git commit -m "refactor(menus): build the menus as a list the egui menu bar draws"
```

---

### Task 2: The Mac menu layout

Adds the macOS shape of the list: the "Xuan" app menu, the items that move into it, and the
Mac Window and Help menus.

**Files:**
- Modify: `src/app/menu_model.rs`
- Modify: `src/app/menus.rs` (`menus` and `draw_entries`)
- Modify: `assets/locales/en.tsv` (after `About Xuan`, line 114)
- Modify: `src/app/tests/menu_bar.rs`, `src/app/tests/keymap.rs`

**Interfaces:**
- Consumes: Task 1's types.
- Produces:
  - `pub(super) enum MenuLayout { Mac, InWindow }` (derives `Clone, Copy, Debug, PartialEq, Eq`)
  - `pub(super) enum Role { App, File, Edit, Image, Layer, Select, Filter, View, Plugins, Window, Help }` and `TopMenu.role: Role`
  - `MenuEntry::System { item: SystemItem, label: String }`
  - `pub(super) enum SystemItem { Services, Hide, HideOthers, ShowAll, Zoom, BringAllToFront }`
  - `MenuAction::Minimize`
  - `EditorApp::menu_bar(&mut self, layout: MenuLayout) -> MenuBar`
  - `EditorApp::run_menu_action(&mut self, ctx: &egui::Context, action: MenuAction)`

- [ ] **Step 1: Write the failing tests**

In `src/app/tests/menu_bar.rs`:

1. Change the import to
   `use crate::app::menu_model::{MenuAction, MenuBar, MenuEntry, MenuItem, MenuLayout, Role, TopMenu};`
2. Replace every `app.menu_bar()` with `app.menu_bar(MenuLayout::InWindow)`
   (`sed -i '' 's/menu_bar()/menu_bar(MenuLayout::InWindow)/g' src/app/tests/menu_bar.rs`).
3. Add an arm to `outline`, before the last one:
   `MenuEntry::System { item, .. } => format!("[{item:?}]"),`
4. Add:

```rust
fn top(bar: &MenuBar, role: Role) -> &[MenuEntry] {
    &bar.menus
        .iter()
        .find(|menu| menu.role == role)
        .expect("menu")
        .entries
}

#[test]
fn the_mac_layout_adds_the_app_menu_and_moves_its_items() {
    let (_, mut app) = app_with_document();
    let bar = app.menu_bar(MenuLayout::Mac);
    let roles: Vec<Role> = bar.menus.iter().map(|menu| menu.role).collect();
    assert_eq!(
        roles,
        [
            Role::App, Role::File, Role::Edit, Role::Image, Role::Layer, Role::Select,
            Role::Filter, Role::View, Role::Plugins, Role::Window, Role::Help,
        ]
    );
    assert_eq!(
        outline(top(&bar, Role::App)),
        [
            "about", "check_updates", "---", "settings", "---", "[Services]", "---", "[Hide]",
            "[HideOthers]", "[ShowAll]", "---", "quit",
        ]
    );
    assert_eq!(bar.find_command("quit").unwrap().label, "Quit Xuan");
    assert_eq!(
        outline(top(&bar, Role::File)).last().map(String::as_str),
        Some("close")
    );
    // Settings has gone, and so has the separator that followed it.
    assert_eq!(outline(top(&bar, Role::Edit))[..2], ["undo", "redo"]);
    assert_eq!(outline(top(&bar, Role::Help)), ["command_palette", "shortcuts"]);
    let window = outline(top(&bar, Role::Window));
    assert_eq!(window[..3], ["Minimize", "[Zoom]", "---"]);
    assert_eq!(
        window[window.len() - 3..],
        ["reset_panels", "---", "[BringAllToFront]"]
    );
}

#[test]
fn minimize_stays_on_while_a_prompt_greys_the_rest() {
    let (_, mut app) = app_with_document();
    app.close_app = true;
    let bar = app.menu_bar(MenuLayout::Mac);
    for item in bar.items() {
        assert_eq!(
            item.enabled,
            item.action == MenuAction::Minimize,
            "{}",
            item.label
        );
    }
}

#[test]
fn the_in_window_layout_has_no_mac_items() {
    let (_, mut app) = app_with_document();
    let bar = app.menu_bar(MenuLayout::InWindow);
    assert!(bar.menus.iter().all(|menu| menu.role != Role::App));
    assert!(bar.items().iter().all(|item| item.action != MenuAction::Minimize));
    assert!(
        bar.menus
            .iter()
            .flat_map(|menu| &menu.entries)
            .all(|entry| !matches!(entry, MenuEntry::System { .. }))
    );
}
```

5. In `separators_never_start_end_or_double_up`, check both layouts each time. Replace each
   `for menu in app.menu_bar(MenuLayout::InWindow).menus { … }` loop with:

```rust
    for layout in [MenuLayout::InWindow, MenuLayout::Mac] {
        for menu in app.menu_bar(layout).menus {
            assert_tidy(&menu.entries, &menu.label);
        }
    }
```

In `src/app/tests/keymap.rs`, change `app.menu_bar()` to
`app.menu_bar(crate::app::menu_model::MenuLayout::InWindow)`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test --locked menu_bar 2>&1 | tail -5`
Expected: compile errors: no `MenuLayout`, `Role` or `MenuEntry::System`.

- [ ] **Step 3: Add the Mac types to `menu_model.rs`**

After the module doc comment's imports, add:

```rust
/// Which shape of menus to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MenuLayout {
    /// The macOS menu bar: an app menu first, which takes About, Settings and Quit.
    // Built only by the macOS menu bar (`macos_menu.rs`) and the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Mac,
    /// The window's own menu row.
    InWindow,
}

/// What a top-level menu is, for the menus macOS treats specially.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    App,
    File,
    Edit,
    Image,
    Layer,
    Select,
    Filter,
    View,
    Plugins,
    Window,
    Help,
}

/// A macOS item that AppKit runs itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SystemItem {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Zoom,
    BringAllToFront,
}
```

Then:

1. Add to `TopMenu`, as its first field:

```rust
    // Read by the macOS menu bar (`macos_menu.rs`) and the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub role: Role,
```

2. Add to `MenuEntry`, after `Placeholder`:

```rust
    /// A macOS item that AppKit runs itself. Only in [`MenuLayout::Mac`].
    // Read by the macOS menu bar (`macos_menu.rs`) and the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    System { item: SystemItem, label: String },
```

3. Add to `MenuAction`, after `Snap`:

```rust
    /// Minimizes the window (the macOS Window menu).
    Minimize,
```

4. Add `mac: bool,` to `State`, with the doc comment `/// Building [`MenuLayout::Mac`].`

5. Add to `impl Entries`:

```rust
    /// One of AppKit's own items: always on.
    fn system(&mut self, item: SystemItem, label: &str) {
        self.entries.push(MenuEntry::System {
            item,
            label: label.to_owned(),
        });
    }
```

6. Replace `finish`, and the `push` at the end of `submenu`, so both tidy their entries:

```rust
    fn finish(self) -> Vec<MenuEntry> {
        tidy(self.entries)
    }
```

```rust
        self.entries.push(MenuEntry::Submenu {
            label: label.to_owned(),
            enabled: inner.enabled,
            entries: tidy(inner.entries),
        });
```

and add the free function:

```rust
/// Drops separators at the start or end of a list and next to another one: an item that moved
/// to the macOS app menu can leave one behind.
fn tidy(entries: Vec<MenuEntry>) -> Vec<MenuEntry> {
    let mut tidy: Vec<MenuEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let separator = matches!(entry, MenuEntry::Separator);
        if separator && matches!(tidy.last(), None | Some(MenuEntry::Separator)) {
            continue;
        }
        tidy.push(entry);
    }
    if matches!(tidy.last(), Some(MenuEntry::Separator)) {
        tidy.pop();
    }
    tidy
}
```

7. Replace `menu_bar` and the start of `build_menu_bar`:

```rust
    /// The menus as the menu bar shows them this frame.
    pub(super) fn menu_bar(&mut self, layout: MenuLayout) -> MenuBar {
        let recent = self.recent_status();
        self.build_menu_bar(layout, &recent)
    }

    fn build_menu_bar(&self, layout: MenuLayout, recent: &[(PathBuf, bool)]) -> MenuBar {
```

   In `build_menu_bar`, add `mac: layout == MenuLayout::Mac,` to `State`, give the `menu`
   closure a role, and put the app menu first on macOS:

```rust
        let menu = |role: Role, label: &str, entries: Vec<MenuEntry>| TopMenu {
            role,
            label: label.to_owned(),
            enabled: state.enabled,
            entries,
        };
        let mut menus = Vec::new();
        if state.mac {
            // macOS shows the app's own name as this menu's title.
            menus.push(menu(Role::App, "Xuan", self.app_entries(&state)));
        }
        menus.extend([
            menu(Role::File, tr("File"), self.file_entries(&state)),
            menu(Role::Edit, tr("Edit"), self.edit_entries(&state)),
            menu(Role::Image, tr("Image"), self.image_entries(&state)),
            menu(Role::Layer, tr("Layer"), self.layer_entries(&state)),
            menu(Role::Select, tr("Select"), self.select_entries(&state)),
            menu(Role::Filter, tr("Filter"), self.filter_entries(&state)),
            menu(Role::View, tr("View"), self.view_entries(&state)),
            menu(Role::Plugins, tr("Plugins"), self.plugins_entries(&state)),
            menu(Role::Window, tr("Window"), self.window_entries(&state)),
            menu(Role::Help, tr("Help"), self.help_entries(&state)),
        ]);
        MenuBar { menus }
```

8. Add the app menu:

```rust
    fn app_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        e.command("about");
        e.command("check_updates");
        e.separator();
        e.command("settings");
        e.separator();
        e.system(SystemItem::Services, tr("Services"));
        e.separator();
        e.system(SystemItem::Hide, tr("Hide Xuan"));
        e.system(SystemItem::HideOthers, tr("Hide Others"));
        e.system(SystemItem::ShowAll, tr("Show All"));
        e.separator();
        e.labelled(tr("Quit Xuan"), "quit");
        e.finish()
    }
```

9. In `file_entries`, wrap the last two lines before `e.finish()`:

```rust
        if !state.mac {
            e.separator();
            e.command("quit");
        }
```

10. In `edit_entries`, replace `e.command("settings");` with
    `if !state.mac { e.command("settings"); }` (rustfmt puts it on three lines).

11. In `help_entries`, wrap the last three commands:

```rust
        if !state.mac {
            e.separator();
            e.command("check_updates");
            e.command("about");
        }
```

12. Replace `window_entries`:

```rust
    fn window_entries(&self, state: &State) -> Vec<MenuEntry> {
        let mut e = Entries::new(self, state.enabled);
        if state.mac {
            // AppKit's own Minimize answers ⌘M, which Curves keeps (as in Photoshop), so this one
            // is Xuan's. Like AppKit's items it works while a prompt greys the rest.
            e.entries.push(MenuEntry::Item(MenuItem {
                label: tr("Minimize").to_owned(),
                enabled: true,
                checked: None,
                chord: None,
                hover: None,
                action: MenuAction::Minimize,
            }));
            e.system(SystemItem::Zoom, tr("Zoom"));
            e.separator();
        }
        e.when(!state.developing, |e| {
            for (id, title, shown) in self.pane_entries() {
                e.action(&title, Some(shown), MenuAction::Pane { id, hidden: shown });
            }
            e.separator();
            e.command("reset_panels");
        });
        if state.mac {
            e.separator();
            e.system(SystemItem::BringAllToFront, tr("Bring All to Front"));
        }
        e.finish()
    }
```

13. Give `run_menu_action` the context and the new arm:

```rust
    /// Does what a menu item stands for. The window's menus and the macOS menu bar both end
    /// here.
    pub(super) fn run_menu_action(&mut self, ctx: &egui::Context, action: MenuAction) {
        match action {
```

```rust
            MenuAction::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
```

14. In `MenuBar::items` (test only), nothing changes: `System` entries are not items.

- [ ] **Step 4: Update the egui renderer**

In `src/app/menus.rs`:
- Import `MenuLayout` with the other `menu_model` names.
- In `menus`, use `let bar = self.menu_bar(MenuLayout::InWindow);` and
  `self.run_menu_action(ctx, action);`.
- In `draw_entries`, add the arm:

```rust
            // AppKit's own items exist only in the macOS menu bar.
            MenuEntry::System { .. } => {}
```

- [ ] **Step 5: Add the new strings**

In `assets/locales/en.tsv`, after the line `About Xuan`, add one line each:

```text
Quit Xuan
Hide Xuan
Hide Others
Show All
Services
Minimize
Bring All to Front
```

(`Zoom` is already there.)

- [ ] **Step 6: Run the tests to see them pass**

Run: `cargo test --locked menu_bar 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: `test result: ok. 11 passed`.

Run: `cargo test --locked 2>&1 | grep -E "^test result|FAILED|panicked" | head -20`
Expected: every line says `0 failed`, including `src/i18n/source_keys.rs`.

- [ ] **Step 7: Lint and commit**

Run: `cargo fmt && cargo clippy --locked --all-targets -- -D warnings 2>&1 | tail -3`
Expected: no warnings.

```bash
git add src/app/menu_model.rs src/app/menus.rs assets/locales/en.tsv src/app/tests/menu_bar.rs src/app/tests/keymap.rs
git commit -m "feat(menus): add the macOS menu layout with the Xuan app menu"
```

---

### Task 3: What the Mac menu must change each frame

A pure function that compares the list the Mac menu shows with the new one, and the label
escaping muda needs.

**Files:**
- Create: `src/app/menu_sync.rs`
- Modify: `src/app/mod.rs` (module list)
- Create: `src/app/tests/menu_changes.rs`
- Modify: `src/app/tests.rs` (register the test module)

**Interfaces:**
- Consumes: `MenuBar`, `MenuEntry`, `TopMenu.role` from Task 2.
- Produces:
  - `pub(super) enum Sync { All, Menus(Vec<MenuChange>) }` (derives `Debug, PartialEq, Eq`)
  - `pub(super) enum MenuChange { Same, State, Rebuild }` (derives `Clone, Copy, Debug, PartialEq, Eq`)
  - `pub(super) fn diff(old: &MenuBar, new: &MenuBar) -> Sync`
  - `pub(super) fn menu_text(label: &str) -> String`

- [ ] **Step 1: Write the failing tests**

Create `src/app/tests/menu_changes.rs`:

```rust
//! What the macOS menu bar changes to show a new menu list (`menu_sync.rs`).

use super::*;
use crate::app::{
    menu_model::{MenuBar, MenuEntry, MenuLayout, Role},
    menu_sync::{MenuChange, Sync, diff, menu_text},
};

fn per_menu(old: &MenuBar, new: &MenuBar) -> Vec<MenuChange> {
    match diff(old, new) {
        Sync::Menus(changes) => changes,
        Sync::All => panic!("expected a change per menu"),
    }
}

fn position(bar: &MenuBar, role: Role) -> usize {
    bar.menus.iter().position(|menu| menu.role == role).unwrap()
}

/// Every menu but `index` is unchanged.
fn only(changes: &[MenuChange], index: usize) -> bool {
    changes
        .iter()
        .enumerate()
        .all(|(i, change)| i == index || *change == MenuChange::Same)
}

#[test]
fn an_unchanged_list_changes_nothing() {
    let (_, mut app) = app_with_document();
    let bar = app.menu_bar(MenuLayout::Mac);
    let changes = per_menu(&bar, &bar.clone());
    assert!(changes.iter().all(|change| *change == MenuChange::Same));
}

#[test]
fn a_first_list_or_other_top_menus_rebuild_everything() {
    let (_, mut app) = app_with_document();
    let mac = app.menu_bar(MenuLayout::Mac);
    assert_eq!(diff(&MenuBar::default(), &mac), Sync::All);
    assert_eq!(diff(&app.menu_bar(MenuLayout::InWindow), &mac), Sync::All);
}

#[test]
fn a_language_change_relabels_in_place() {
    let (_, mut app) = app_with_document();
    let old = app.menu_bar(MenuLayout::Mac);
    let file = position(&old, Role::File);
    let window = position(&old, Role::Window);
    let mut new = old.clone();
    new.menus[file].label = "Datei".into();
    if let MenuEntry::Item(item) = &mut new.menus[file].entries[0] {
        item.label = "Neu…".into();
    }
    // AppKit's own items are relabelled too.
    for entry in &mut new.menus[window].entries {
        if let MenuEntry::System { label, .. } = entry {
            *label = "Zoomen".into();
        }
    }
    let changes = per_menu(&old, &new);
    assert_eq!(changes[file], MenuChange::State);
    assert_eq!(changes[window], MenuChange::State);
}

#[test]
fn a_step_to_undo_changes_states_in_place() {
    let (_, mut app) = app_with_document();
    let old = app.menu_bar(MenuLayout::Mac);
    app.command("new_layer");
    let new = app.menu_bar(MenuLayout::Mac);
    let changes = per_menu(&old, &new);
    assert_eq!(changes[position(&old, Role::Edit)], MenuChange::State);
    assert!(!changes.contains(&MenuChange::Rebuild), "{changes:?}");
}

#[test]
fn added_items_rebuild_only_their_menu() {
    let directory = tempfile::tempdir().unwrap();
    let recent = directory.path().join("recent.png");
    std::fs::write(&recent, b"").unwrap();
    let (_, mut app) = app_with_document();
    let old = app.menu_bar(MenuLayout::Mac);
    app.config.recent_files = vec![recent];
    let new = app.menu_bar(MenuLayout::Mac);
    let file = position(&old, Role::File);
    let changes = per_menu(&old, &new);
    assert_eq!(changes[file], MenuChange::Rebuild);
    assert!(only(&changes, file), "{changes:?}");
}

/// The tick of the first toggle in a top-level menu.
fn first_toggle(bar: &mut MenuBar, menu: usize) -> &mut Option<bool> {
    bar.menus[menu]
        .entries
        .iter_mut()
        .find_map(|entry| match entry {
            MenuEntry::Item(item) if item.checked.is_some() => Some(&mut item.checked),
            _ => None,
        })
        .expect("a toggle")
}

#[test]
fn a_tick_changes_in_place_but_a_toggle_turning_plain_rebuilds() {
    let (_, mut app) = app_with_document();
    let old = app.menu_bar(MenuLayout::Mac);
    let view = position(&old, Role::View);
    let mut ticked = old.clone();
    let checked = first_toggle(&mut ticked, view);
    *checked = checked.map(|on| !on);
    assert_eq!(per_menu(&old, &ticked)[view], MenuChange::State);
    let mut plain = old.clone();
    *first_toggle(&mut plain, view) = None;
    assert_eq!(per_menu(&old, &plain)[view], MenuChange::Rebuild);
}

#[test]
fn an_ampersand_survives_muda() {
    assert_eq!(menu_text("Black & White"), "Black && White");
    assert_eq!(menu_text("Save"), "Save");
}
```

Register it in `src/app/tests.rs`, after `menu_bar`:

```rust
#[path = "tests/menu_changes.rs"]
mod menu_changes;
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test --locked menu_changes 2>&1 | tail -5`
Expected: compile error, "unresolved import `crate::app::menu_sync`".

- [ ] **Step 3: Write `src/app/menu_sync.rs`**

```rust
//! What the macOS menu bar (`macos_menu.rs`) must change to show a new menu list, and the text
//! it gives muda. Kept apart from AppKit so it is tested on every platform.

use super::menu_model::{MenuBar, MenuEntry};

/// What the menu bar must do.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Sync {
    /// The top-level menus are different: build the whole menu again.
    All,
    /// One change for each top-level menu, in order.
    Menus(Vec<MenuChange>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MenuChange {
    Same,
    /// Only labels, on and off, ticks or shortcuts differ: change the items in place.
    State,
    /// Entries were added, removed or changed kind: build this menu again.
    Rebuild,
}

pub(super) fn diff(old: &MenuBar, new: &MenuBar) -> Sync {
    if old.menus.len() != new.menus.len()
        || old
            .menus
            .iter()
            .zip(&new.menus)
            .any(|(old, new)| old.role != new.role)
    {
        return Sync::All;
    }
    Sync::Menus(
        old.menus
            .iter()
            .zip(&new.menus)
            .map(|(old, new)| {
                if old == new {
                    MenuChange::Same
                } else if same_shape(&old.entries, &new.entries) {
                    MenuChange::State
                } else {
                    MenuChange::Rebuild
                }
            })
            .collect(),
    )
}

/// Whether two lists differ only in what the menu bar can change in place. Items must do the
/// same thing, as the menu bar maps each item to its action when it builds it.
fn same_shape(old: &[MenuEntry], new: &[MenuEntry]) -> bool {
    old.len() == new.len()
        && old.iter().zip(new).all(|pair| match pair {
            (MenuEntry::Item(old), MenuEntry::Item(new)) => {
                old.action == new.action && old.checked.is_some() == new.checked.is_some()
            }
            (
                MenuEntry::Submenu { entries: old, .. },
                MenuEntry::Submenu { entries: new, .. },
            ) => same_shape(old, new),
            (MenuEntry::Separator, MenuEntry::Separator)
            | (MenuEntry::Placeholder(_), MenuEntry::Placeholder(_)) => true,
            (MenuEntry::System { item: old, .. }, MenuEntry::System { item: new, .. }) => {
                old == new
            }
            _ => false,
        })
}

/// A label as muda wants it. muda reads `&` as a mnemonic marker and drops it, so a literal
/// `&` (Black & White, a plugin's label) is written `&&`.
pub(super) fn menu_text(label: &str) -> String {
    label.replace('&', "&&")
}
```

In `src/app/mod.rs`, after `mod menu_model;`, add:

```rust
// The macOS menu bar uses it; until it exists, only the tests do.
#[cfg(test)]
mod menu_sync;
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test --locked menu_changes 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: `test result: ok. 7 passed`.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt && cargo clippy --locked --all-targets -- -D warnings 2>&1 | tail -3`
Expected: no warnings.

```bash
git add src/app/menu_sync.rs src/app/mod.rs src/app/tests.rs src/app/tests/menu_changes.rs
git commit -m "feat(menus): work out what the macOS menu bar changes each frame"
```

---

### Task 4: Mac key equivalents, and giving keys back to egui

**Files:**
- Create: `src/app/menu_keys.rs`
- Modify: `src/app/mod.rs` (module list)
- Create: `src/app/tests/mac_keys.rs`, `src/app/tests/ui_mac_keys.rs`
- Modify: `src/app/tests.rs`, `src/app/tests/ui.rs` (register the test modules)

**Interfaces:**
- Consumes: `Chord` (`commands.rs`), `MenuAction`, `MenuLayout`, `MenuBar::items`.
- Produces:
  - `pub(super) struct KeyEquivalent { pub key: String, pub command: bool, pub control: bool, pub option: bool, pub shift: bool }`
  - `pub(super) fn key_equivalent(chord: Chord) -> Option<KeyEquivalent>`
  - `pub(super) fn mac_modifiers(chord: Chord) -> egui::Modifiers`
  - `pub(super) fn replay(chord: Chord, clipboard: impl FnOnce() -> Option<String>) -> Vec<egui::Event>`
  - `pub(super) enum Route { Replay(Chord), Run(MenuAction) }` and `pub(super) fn route(from_key: bool, chord: Option<Chord>, action: MenuAction) -> Route`

- [ ] **Step 1: Write the failing unit tests**

Create `src/app/tests/mac_keys.rs`:

```rust
//! Key equivalents in the macOS menu bar, and keys it gives back to egui (`menu_keys.rs`).

use super::*;
use crate::app::{
    commands::Chord,
    menu_keys::{KeyEquivalent, Route, key_equivalent, replay, route},
    menu_model::{MenuAction, MenuLayout},
};
use egui::{Event, Key, Modifiers};

const fn chord(mods: Modifiers, key: Key) -> Chord {
    Chord { mods, key }
}

const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);

#[test]
fn key_equivalents_use_command_except_with_tab() {
    assert_eq!(
        key_equivalent(chord(CTRL_SHIFT, Key::S)),
        Some(KeyEquivalent {
            key: "s".into(),
            command: true,
            control: false,
            option: false,
            shift: true,
        })
    );
    // ⌘Tab switches apps, so the primary modifier is ⌃ Control with Tab.
    assert_eq!(
        key_equivalent(chord(Modifiers::CTRL, Key::Tab)),
        Some(KeyEquivalent {
            key: "\t".into(),
            command: false,
            control: true,
            option: false,
            shift: false,
        })
    );
    let key = |mods, key| key_equivalent(chord(mods, key)).map(|equivalent| equivalent.key);
    assert_eq!(key(Modifiers::ALT, Key::Backspace).as_deref(), Some("\u{8}"));
    assert_eq!(key(Modifiers::NONE, Key::Delete).as_deref(), Some("\u{f728}"));
    assert_eq!(key(Modifiers::SHIFT, Key::F5).as_deref(), Some("\u{f708}"));
    assert_eq!(key(Modifiers::CTRL, Key::Comma).as_deref(), Some(","));
    assert_eq!(key(Modifiers::CTRL, Key::BrowserBack), None);
}

#[test]
fn every_menu_shortcut_has_a_key_equivalent() {
    let (context, mut app) = app_with_document();
    let check = |app: &mut EditorApp| {
        for layout in [MenuLayout::Mac, MenuLayout::InWindow] {
            let bar = app.menu_bar(layout);
            for item in bar.items() {
                if let Some(chord) = item.chord {
                    assert!(key_equivalent(chord).is_some(), "{}: {chord:?}", item.label);
                }
            }
        }
    };
    check(&mut app);
    app.develop = Some(crate::app::develop::tests::ready(&context));
    check(&mut app);
}

#[test]
fn a_shortcut_goes_back_to_egui_and_a_click_runs() {
    let save = chord(Modifiers::CTRL, Key::S);
    let action = || MenuAction::Command("save");
    assert_eq!(route(true, Some(save), action()), Route::Replay(save));
    assert_eq!(route(false, Some(save), action()), Route::Run(action()));
    // Without a key equivalent no key could have chosen it.
    assert_eq!(route(true, None, action()), Route::Run(action()));
}

#[test]
fn replayed_keys_arrive_as_winit_sends_them() {
    let command = Modifiers {
        mac_cmd: true,
        command: true,
        ..Modifiers::NONE
    };
    let control = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    let key = |key, pressed, modifiers| Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    };
    assert_eq!(
        replay(chord(Modifiers::CTRL, Key::S), || None),
        [key(Key::S, true, command), key(Key::S, false, command)]
    );
    assert_eq!(
        replay(chord(Modifiers::CTRL, Key::Tab), || None),
        [key(Key::Tab, true, control), key(Key::Tab, false, control)]
    );
    // The clipboard chords become egui's clipboard events, with Shift or without.
    assert_eq!(replay(chord(Modifiers::CTRL, Key::C), || None), [Event::Copy]);
    assert_eq!(replay(chord(CTRL_SHIFT, Key::C), || None), [Event::Copy]);
    assert_eq!(replay(chord(Modifiers::CTRL, Key::X), || None), [Event::Cut]);
    assert_eq!(
        replay(chord(Modifiers::CTRL, Key::V), || Some("one\r\ntwo".into())),
        [Event::Paste("one\ntwo".into())]
    );
    // An image alone on the clipboard still pastes, as the patched egui-winit sends it.
    assert_eq!(
        replay(chord(Modifiers::CTRL, Key::V), || None),
        [Event::Paste(String::new())]
    );
}
```

Register it in `src/app/tests.rs`, after `menu_changes`:

```rust
#[path = "tests/mac_keys.rs"]
mod mac_keys;
```

- [ ] **Step 2: Write the failing UI tests**

Create `src/app/tests/ui_mac_keys.rs`:

```rust
//! A key the macOS menu bar took comes back to egui, which handles it as before.

use super::*;
use crate::app::{commands::Chord, menu_keys::replay};
use egui::{Key, Modifiers};

/// Sends a chord as the macOS menu bar gives it back.
fn replayed(ui: &mut UiTest, chord: Chord) {
    let events = replay(chord, || None);
    ui.harness.input_mut().events.extend(events);
    ui.settle();
}

#[test]
fn a_key_the_menu_bar_took_still_edits_a_text_field() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    ui.press(Modifiers::CTRL, Key::Comma);
    ui.click("Keyboard Shortcuts");
    ui.type_in_text_field("merge");
    ui.app_mut().command_trace = Some(Vec::new());
    // ⌘A selects the field's text, not the canvas.
    replayed(&mut ui, Chord {
        mods: Modifiers::CTRL,
        key: Key::A,
    });
    ui.type_keys("blur");
    assert_eq!(
        ui.harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .value()
            .as_deref(),
        Some("blur")
    );
    assert_eq!(ui.app().command_trace.as_deref(), Some(&[][..]), "no command ran");
}

#[test]
fn a_key_the_menu_bar_took_still_runs_its_command() {
    let mut ui = UiTest::with_document();
    ui.app_mut().command_trace = Some(Vec::new());
    let chord = ui.app().keymap.keys("new_layer")[0];
    replayed(&mut ui, chord);
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&["new_layer".to_owned()][..])
    );
}
```

Register it in `src/app/tests/ui.rs`, after the `paths` module (around line 95):

```rust
#[path = "ui_mac_keys.rs"]
mod mac_keys;
```

If `new_layer` has no default binding (the `[0]` panics), use `merge`, which
`src/app/tests/keymap.rs` shows is bound to Ctrl+E.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test --locked mac_keys 2>&1 | tail -5`
Expected: compile error, "unresolved import `crate::app::menu_keys`".

- [ ] **Step 4: Write `src/app/menu_keys.rs`**

```rust
//! Shortcuts in the macOS menu bar. AppKit offers a key press to the menu's key equivalents
//! before winit's view sees it, so an item chosen by its key equivalent gives the key back to
//! egui ([`route`], [`replay`]) instead of running: text fields, the command palette and the
//! shortcut scopes then handle it exactly as before. Kept apart from AppKit so it is tested on
//! every platform.

use egui::{Event, Key, Modifiers};

use super::{commands::Chord, menu_model::MenuAction};

/// A Mac key equivalent: the key's character and its modifiers, as `NSMenuItem` takes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct KeyEquivalent {
    pub key: String,
    pub command: bool,
    pub control: bool,
    pub option: bool,
    pub shift: bool,
}

/// The key equivalent that shows and answers `chord` in a Mac menu, if the key has one.
pub(super) fn key_equivalent(chord: Chord) -> Option<KeyEquivalent> {
    let modifiers = mac_modifiers(chord);
    Some(KeyEquivalent {
        key: mac_key(chord.key)?.to_owned(),
        command: modifiers.mac_cmd,
        control: modifiers.ctrl,
        option: modifiers.alt,
        shift: modifiers.shift,
    })
}

/// The modifiers macOS reports for `chord`. The primary modifier is ⌘ Command, except with Tab,
/// where it is ⌃ Control, as ⌘Tab switches apps (see `Chord::label_for`).
pub(super) fn mac_modifiers(chord: Chord) -> Modifiers {
    let control = chord.mods.ctrl && chord.key == Key::Tab;
    let command = chord.mods.ctrl && !control;
    Modifiers {
        alt: chord.mods.alt,
        ctrl: control,
        shift: chord.mods.shift,
        mac_cmd: command,
        command,
    }
}

/// The character AppKit uses for a key in a key equivalent.
fn mac_key(key: Key) -> Option<&'static str> {
    Some(match key {
        Key::A => "a",
        Key::B => "b",
        Key::C => "c",
        Key::D => "d",
        Key::E => "e",
        Key::F => "f",
        Key::G => "g",
        Key::H => "h",
        Key::I => "i",
        Key::J => "j",
        Key::K => "k",
        Key::L => "l",
        Key::M => "m",
        Key::N => "n",
        Key::O => "o",
        Key::P => "p",
        Key::Q => "q",
        Key::R => "r",
        Key::S => "s",
        Key::T => "t",
        Key::U => "u",
        Key::V => "v",
        Key::W => "w",
        Key::X => "x",
        Key::Y => "y",
        Key::Z => "z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::Minus => "-",
        Key::Plus => "+",
        Key::Equals => "=",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Semicolon => ";",
        Key::Colon => ":",
        Key::Quote => "'",
        Key::Slash => "/",
        Key::Backslash => "\\",
        Key::Pipe => "|",
        Key::Questionmark => "?",
        Key::Exclamationmark => "!",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::OpenCurlyBracket => "{",
        Key::CloseCurlyBracket => "}",
        Key::Backtick => "`",
        Key::Space => " ",
        Key::Tab => "\t",
        Key::Enter => "\r",
        Key::Escape => "\u{1b}",
        // NSBackspaceCharacter, shown as ⌫, and NSDeleteFunctionKey, shown as ⌦.
        Key::Backspace => "\u{8}",
        Key::Delete => "\u{f728}",
        Key::Insert => "\u{f727}",
        Key::Home => "\u{f729}",
        Key::End => "\u{f72b}",
        Key::PageUp => "\u{f72c}",
        Key::PageDown => "\u{f72d}",
        Key::ArrowUp => "\u{f700}",
        Key::ArrowDown => "\u{f701}",
        Key::ArrowLeft => "\u{f702}",
        Key::ArrowRight => "\u{f703}",
        Key::F1 => "\u{f704}",
        Key::F2 => "\u{f705}",
        Key::F3 => "\u{f706}",
        Key::F4 => "\u{f707}",
        Key::F5 => "\u{f708}",
        Key::F6 => "\u{f709}",
        Key::F7 => "\u{f70a}",
        Key::F8 => "\u{f70b}",
        Key::F9 => "\u{f70c}",
        Key::F10 => "\u{f70d}",
        Key::F11 => "\u{f70e}",
        Key::F12 => "\u{f70f}",
        Key::F13 => "\u{f710}",
        Key::F14 => "\u{f711}",
        Key::F15 => "\u{f712}",
        Key::F16 => "\u{f713}",
        Key::F17 => "\u{f714}",
        Key::F18 => "\u{f715}",
        Key::F19 => "\u{f716}",
        Key::F20 => "\u{f717}",
        _ => return None,
    })
}

/// What a menu bar event does.
#[derive(Debug, PartialEq)]
pub(super) enum Route {
    /// The item's key equivalent chose it: give the key back to egui.
    Replay(Chord),
    /// A click chose it: run it.
    Run(MenuAction),
}

pub(super) fn route(from_key: bool, chord: Option<Chord>, action: MenuAction) -> Route {
    match chord {
        Some(chord) if from_key => Route::Replay(chord),
        _ => Route::Run(action),
    }
}

/// The events winit would have sent egui for `chord`, had the menu not taken it. Like
/// egui-winit, ⌘X, ⌘C and ⌘V become egui's clipboard events, and a paste carries the
/// clipboard's text, empty when it holds only an image (Xuan's patched egui-winit).
pub(super) fn replay(chord: Chord, clipboard: impl FnOnce() -> Option<String>) -> Vec<Event> {
    let modifiers = mac_modifiers(chord);
    if modifiers.command {
        match chord.key {
            Key::X => return vec![Event::Cut],
            Key::C => return vec![Event::Copy],
            Key::V => {
                let text = clipboard().unwrap_or_default().replace("\r\n", "\n");
                return vec![Event::Paste(text)];
            }
            _ => {}
        }
    }
    [true, false]
        .map(|pressed| Event::Key {
            key: chord.key,
            physical_key: Some(chord.key),
            pressed,
            repeat: false,
            modifiers,
        })
        .to_vec()
}
```

In `src/app/mod.rs`, after `mod menu_model;`, add:

```rust
// The macOS menu bar uses it; until it exists, only the tests do.
#[cfg(test)]
mod menu_keys;
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test --locked mac_keys 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: two `test result: ok.` lines, 4 passed and 2 passed. If
`every_menu_shortcut_has_a_key_equivalent` names a key, add it to `mac_key` with AppKit's
character for it (the function-key range is U+F700 to U+F8FF, `NSUpArrowFunctionKey` onwards).

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt && cargo clippy --locked --all-targets -- -D warnings 2>&1 | tail -3`
Expected: no warnings.

```bash
git add src/app/menu_keys.rs src/app/mod.rs src/app/tests.rs src/app/tests/ui.rs src/app/tests/mac_keys.rs src/app/tests/ui_mac_keys.rs
git commit -m "feat(menus): add Mac key equivalents and give taken keys back to egui"
```

---

### Task 5: The macOS menu bar

Adds muda, the native renderer, and its wiring. Mac-only code cannot run in the unit tests
(muda needs the main thread), so this task ends with manual checks on this Mac.

**Files:**
- Modify: `Cargo.toml` (macOS dependencies, line 78), `Cargo.lock`
- Create: `licenses/muda-MIT.txt`
- Modify: `THIRD_PARTY.md` (after the notify entry)
- Create: `src/app/macos_menu.rs`
- Modify: `src/app/mod.rs` (modules, fields near lines 759 and 986, startup near line 821,
  `show_with_processor` near lines 2262 and 2308)
- Modify: `src/app/menu_model.rs` (three `cfg_attr`s)

**Interfaces:**
- Consumes: `diff`, `Sync`, `MenuChange`, `menu_text` (Task 3); `key_equivalent`, `route`,
  `Route`, `replay` (Task 4); `menu_bar(MenuLayout::Mac)`, `run_menu_action` (Task 2).
- Produces:
  - Field `system_menu_bar: bool` on `EditorApp` (all platforms; true when the menus are in the
    macOS menu bar).
  - Field `#[cfg(target_os = "macos")] native_menu: Option<macos_menu::NativeMenu>`.
  - `EditorApp::install_native_menu(&mut self, ctx: &egui::Context)`,
    `EditorApp::take_native_menu_input(&mut self, ctx: &egui::Context)`,
    `EditorApp::sync_native_menu(&mut self)` (all macOS only).

- [ ] **Step 1: Add the dependencies**

In `Cargo.toml`, under `[target.'cfg(target_os = "macos")'.dependencies]`, after the `objc2` line:

```toml
# The native menu bar (src/app/macos_menu.rs). Pinned: review the code before any bump.
muda = { version = "=0.20.0", default-features = false }
# The event behind a menu action (src/app/macos_menu.rs). Already built for winit and muda.
objc2-app-kit = { version = "0.3", default-features = false, features = ["std", "NSApplication", "NSEvent", "NSResponder"] }
```

Run: `cargo build 2>&1 | tail -2` (no `--locked`: this records the new crates).
Then run: `git diff Cargo.lock | grep '^+name' `
Expected: exactly `+name = "crossbeam-channel"`, `+name = "keyboard-types"` and `+name = "muda"`.
Anything else: stop and report it.

- [ ] **Step 2: Record the licence**

```bash
cp ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/muda-0.20.0/LICENSE-MIT licenses/muda-MIT.txt
```

In `THIRD_PARTY.md`, after the **notify 8.2** entry, add:

```markdown
- **muda 0.20.0** (macOS builds only) builds the native menu bar. Copyright 2022 Tauri Programme within The Commons Conservancy. Dual-licensed under Apache-2.0 or MIT; Xuan uses it under the [MIT license](licenses/muda-MIT.txt); see [upstream source](https://crates.io/crates/muda/0.20.0). It brings in **crossbeam-channel 0.5** and **keyboard-types 0.8** (MIT or Apache-2.0).
```

Check the copyright line against the first lines of `licenses/muda-MIT.txt` and correct it if
the file says otherwise.

- [ ] **Step 3: Compile the pure helpers on macOS too**

In `src/app/mod.rs`, change both `#[cfg(test)]` lines above `mod menu_sync;` and
`mod menu_keys;` (and their comments) to:

```rust
// The macOS menu bar's logic, tested on every platform.
#[cfg(any(target_os = "macos", test))]
```

and add, after them:

```rust
#[cfg(target_os = "macos")]
mod macos_menu;
```

In `src/app/menu_model.rs`, change the three `#[cfg_attr(not(test), allow(dead_code))]` lines (on
`MenuLayout::Mac`, `TopMenu::role` and `MenuEntry::System`) to
`#[cfg_attr(not(target_os = "macos"), allow(dead_code))]`, and their comments to
`// Used by the macOS menu bar (`macos_menu.rs`).`

- [ ] **Step 4: Write `src/app/macos_menu.rs`**

```rust
//! The macOS menu bar: the menu list (`menu_model.rs`) as a native menu, built with muda.
//!
//! AppKit offers a key press to the menu's key equivalents before winit's view sees it. An item
//! chosen by its key equivalent therefore gives the key back to egui ([`menu_keys::replay`]),
//! and a click runs the item. Each frame [`diff`] decides what to change: nothing, labels and
//! states in place, or one top-level menu rebuilt. If the menu cannot be built, the window shows
//! its own menus again.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use muda::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, PredefinedMenuItem, Submenu,
    accelerator::{Key, KeyAccelerator, Modifiers},
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEventType};

use super::{
    EditorApp,
    commands::Chord,
    menu_keys::{self, Route},
    menu_model::{MenuAction, MenuBar, MenuEntry, MenuLayout, Role, SystemItem},
    menu_sync::{MenuChange, Sync, diff, menu_text},
};

/// A native item, beside the entry it shows.
enum Native {
    Item(muda::MenuItem),
    Check(CheckMenuItem),
    Submenu(Submenu, Vec<Native>),
    /// AppKit's own items.
    System(PredefinedMenuItem),
    Separator(PredefinedMenuItem),
}

impl Native {
    fn as_item(&self) -> &dyn IsMenuItem {
        match self {
            Self::Item(item) => item,
            Self::Check(item) => item,
            Self::Submenu(item, _) => item,
            Self::System(item) | Self::Separator(item) => item,
        }
    }

    fn set_text(&self, text: &str) {
        let text = menu_text(text);
        match self {
            Self::Item(item) => item.set_text(text),
            Self::Check(item) => item.set_text(text),
            Self::Submenu(item, _) => item.set_text(text),
            Self::System(item) => item.set_text(text),
            Self::Separator(_) => {}
        }
    }

    fn set_enabled(&self, enabled: bool) {
        match self {
            Self::Item(item) => item.set_enabled(enabled),
            Self::Check(item) => item.set_enabled(enabled),
            Self::Submenu(item, _) => item.set_enabled(enabled),
            Self::System(_) | Self::Separator(_) => {}
        }
    }

    fn set_checked(&self, checked: bool) {
        if let Self::Check(item) = self {
            item.set_checked(checked);
        }
    }

    fn set_key(&self, chord: Option<Chord>) -> muda::Result<()> {
        match self {
            Self::Item(item) => item.set_key_accelerator(accelerator(chord)),
            Self::Check(item) => item.set_key_accelerator(accelerator(chord)),
            Self::Submenu(..) | Self::System(_) | Self::Separator(_) => Ok(()),
        }
    }
}

/// What the menu bar shows, and what its items stand for.
pub(super) struct NativeMenu {
    menu: Menu,
    /// Each top-level menu and its items, in the order of `shown.menus`.
    tops: Vec<(Submenu, Vec<Native>)>,
    /// The list the menu shows now.
    shown: MenuBar,
    /// Each item's action and key equivalent, by its muda id.
    actions: HashMap<MenuId, (MenuAction, Option<Chord>)>,
    /// Items chosen since the last frame, and whether a key press chose each.
    events: Arc<Mutex<Vec<(MenuId, bool)>>>,
    /// An item was chosen. muda turns a check item's tick over by itself when it is chosen,
    /// whether or not the action then runs, so every tick is set again from the list.
    recheck: bool,
}

impl NativeMenu {
    /// Makes `bar` the app's menu bar, in place of winit's default menu. Call on the main
    /// thread once the event loop runs.
    fn install(ctx: &egui::Context, bar: MenuBar) -> muda::Result<Self> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::clone(&events);
        let ctx = ctx.clone();
        // muda calls this inside AppKit's menu action, on the main thread, while the event
        // AppKit is handling still tells a key press from a click.
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let from_key = current_event_is_key_down();
            if let Ok(mut queue) = queue.lock() {
                queue.push((event.id, from_key));
            }
            ctx.request_repaint();
        }));
        let mut native = Self {
            menu: Menu::new(),
            tops: Vec::new(),
            shown: MenuBar::default(),
            actions: HashMap::new(),
            events,
            recheck: false,
        };
        // muda gives the Window and Help menus their roles only in the app's own menu.
        native.menu.init_for_nsapp();
        if let Err(error) = native.rebuild_all(bar) {
            native.menu.remove_for_nsapp();
            return Err(error);
        }
        Ok(native)
    }

    /// Builds every top-level menu again.
    fn rebuild_all(&mut self, bar: MenuBar) -> muda::Result<()> {
        for (submenu, _) in self.tops.drain(..) {
            self.menu.remove(&submenu)?;
        }
        for top in &bar.menus {
            let submenu = Submenu::new(menu_text(&top.label), true);
            let items = build(&submenu, &top.entries)?;
            self.menu.append(&submenu)?;
            match top.role {
                Role::Window => submenu.set_as_windows_menu_for_nsapp(),
                Role::Help => submenu.set_as_help_menu_for_nsapp(),
                _ => {}
            }
            self.tops.push((submenu, items));
        }
        self.shown = bar;
        self.map_actions();
        Ok(())
    }

    /// Brings the menu bar up to date with `bar`. Returns false when it cannot, after removing
    /// the menu, so the window shows its own menus again.
    fn sync(&mut self, bar: MenuBar) -> bool {
        let Err(error) = self.try_sync(&bar) else {
            return true;
        };
        eprintln!("Could not update the macOS menu bar, so it is rebuilt: {error}");
        if let Err(error) = self.rebuild_all(bar) {
            eprintln!("Could not build the macOS menu bar; the window shows its menus: {error}");
            self.menu.remove_for_nsapp();
            return false;
        }
        true
    }

    fn try_sync(&mut self, bar: &MenuBar) -> muda::Result<()> {
        let recheck = std::mem::take(&mut self.recheck);
        let changes = match diff(&self.shown, bar) {
            Sync::All => return self.rebuild_all(bar.clone()),
            Sync::Menus(changes) => changes,
        };
        if !recheck && changes.iter().all(|change| *change == MenuChange::Same) {
            return Ok(());
        }
        let mut rebuilt = false;
        for (index, change) in changes.into_iter().enumerate() {
            let (old, top) = (&self.shown.menus[index], &bar.menus[index]);
            let (submenu, items) = &mut self.tops[index];
            match change {
                MenuChange::Same => {}
                MenuChange::State => {
                    if old.label != top.label {
                        submenu.set_text(menu_text(&top.label));
                    }
                    update(items, &old.entries, &top.entries)?;
                }
                MenuChange::Rebuild => {
                    while submenu.remove_at(0).is_some() {}
                    submenu.set_text(menu_text(&top.label));
                    *items = build(submenu, &top.entries)?;
                    rebuilt = true;
                }
            }
            if recheck {
                recheck_ticks(items, &top.entries);
            }
        }
        self.shown = bar.clone();
        if rebuilt {
            self.map_actions();
        }
        Ok(())
    }

    /// Records each item's action and key equivalent by its muda id.
    fn map_actions(&mut self) {
        self.actions.clear();
        for ((_, items), top) in self.tops.iter().zip(&self.shown.menus) {
            map_items(items, &top.entries, &mut self.actions);
        }
    }

    /// What the menu bar was asked to do since the last frame, in order.
    fn take_routes(&mut self) -> Vec<Route> {
        let events = self
            .events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default();
        self.recheck |= !events.is_empty();
        events
            .into_iter()
            .filter_map(|(id, from_key)| {
                // An item of a menu rebuilt since it was chosen is gone: nothing to do.
                let (action, chord) = self.actions.get(&id)?.clone();
                Some(menu_keys::route(from_key, chord, action))
            })
            .collect()
    }
}

/// Appends `entries` to `parent` and returns their native items.
fn build(parent: &Submenu, entries: &[MenuEntry]) -> muda::Result<Vec<Native>> {
    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        let native = match entry {
            MenuEntry::Item(item) => {
                let text = menu_text(&item.label);
                let native = match item.checked {
                    Some(checked) => {
                        Native::Check(CheckMenuItem::new(text, item.enabled, checked, None))
                    }
                    None => Native::Item(muda::MenuItem::new(text, item.enabled, None)),
                };
                native.set_key(item.chord)?;
                native
            }
            MenuEntry::Submenu {
                label,
                enabled,
                entries,
            } => {
                let submenu = Submenu::new(menu_text(label), *enabled);
                let children = build(&submenu, entries)?;
                Native::Submenu(submenu, children)
            }
            MenuEntry::Separator => Native::Separator(PredefinedMenuItem::separator()),
            MenuEntry::Placeholder(text) => {
                Native::Item(muda::MenuItem::new(menu_text(text), false, None))
            }
            MenuEntry::System { item, label } => Native::System(system_item(*item, label)),
        };
        parent.append(native.as_item())?;
        items.push(native);
    }
    Ok(items)
}

/// One of AppKit's own items, with Xuan's label.
fn system_item(item: SystemItem, label: &str) -> PredefinedMenuItem {
    let text = menu_text(label);
    let text = Some(text.as_str());
    match item {
        SystemItem::Services => PredefinedMenuItem::services(text),
        SystemItem::Hide => PredefinedMenuItem::hide(text),
        SystemItem::HideOthers => PredefinedMenuItem::hide_others(text),
        SystemItem::ShowAll => PredefinedMenuItem::show_all(text),
        // AppKit's Zoom (`performZoom:`); muda calls it Maximize.
        SystemItem::Zoom => PredefinedMenuItem::maximize(text),
        SystemItem::BringAllToFront => PredefinedMenuItem::bring_all_to_front(text),
    }
}

/// Changes items in place: labels, on and off, ticks and key equivalents. `old` and `new` have
/// the same shape ([`diff`] said `State`).
fn update(items: &[Native], old: &[MenuEntry], new: &[MenuEntry]) -> muda::Result<()> {
    for ((native, old), new) in items.iter().zip(old).zip(new) {
        match (old, new) {
            (MenuEntry::Item(old), MenuEntry::Item(new)) => {
                if old.label != new.label {
                    native.set_text(&new.label);
                }
                if old.enabled != new.enabled {
                    native.set_enabled(new.enabled);
                }
                if old.checked != new.checked {
                    native.set_checked(new.checked == Some(true));
                }
                if old.chord != new.chord {
                    native.set_key(new.chord)?;
                }
            }
            (MenuEntry::Placeholder(old), MenuEntry::Placeholder(new))
            | (MenuEntry::System { label: old, .. }, MenuEntry::System { label: new, .. }) => {
                if old != new {
                    native.set_text(new);
                }
            }
            (
                MenuEntry::Submenu {
                    label: old_label,
                    enabled: old_enabled,
                    entries: old_entries,
                },
                MenuEntry::Submenu {
                    label,
                    enabled,
                    entries,
                },
            ) => {
                if old_label != label {
                    native.set_text(label);
                }
                if old_enabled != enabled {
                    native.set_enabled(*enabled);
                }
                if let Native::Submenu(_, children) = native {
                    update(children, old_entries, entries)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Sets every tick from the list again (see [`NativeMenu::recheck`]).
fn recheck_ticks(items: &[Native], entries: &[MenuEntry]) {
    for (native, entry) in items.iter().zip(entries) {
        match (native, entry) {
            (Native::Check(_), MenuEntry::Item(item)) => {
                native.set_checked(item.checked == Some(true));
            }
            (Native::Submenu(_, children), MenuEntry::Submenu { entries, .. }) => {
                recheck_ticks(children, entries);
            }
            _ => {}
        }
    }
}

fn map_items(
    items: &[Native],
    entries: &[MenuEntry],
    actions: &mut HashMap<MenuId, (MenuAction, Option<Chord>)>,
) {
    for (native, entry) in items.iter().zip(entries) {
        match (native, entry) {
            (Native::Item(native), MenuEntry::Item(item)) => {
                actions.insert(native.id().clone(), (item.action.clone(), item.chord));
            }
            (Native::Check(native), MenuEntry::Item(item)) => {
                actions.insert(native.id().clone(), (item.action.clone(), item.chord));
            }
            (Native::Submenu(_, children), MenuEntry::Submenu { entries, .. }) => {
                map_items(children, entries, actions);
            }
            _ => {}
        }
    }
}

/// The key equivalent for `chord`, as muda takes it.
fn accelerator(chord: Option<Chord>) -> Option<KeyAccelerator> {
    let key = menu_keys::key_equivalent(chord?)?;
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::META, key.command);
    modifiers.set(Modifiers::CONTROL, key.control);
    modifiers.set(Modifiers::ALT, key.option);
    modifiers.set(Modifiers::SHIFT, key.shift);
    Some(KeyAccelerator::new(modifiers, Key::Character(key.key)))
}

/// Whether AppKit is handling a key press: a menu item then ran from its key equivalent, as an
/// item runs during a key-down only through its own key equivalent.
fn current_event_is_key_down() -> bool {
    MainThreadMarker::new().is_some_and(|mtm| {
        NSApplication::sharedApplication(mtm)
            .currentEvent()
            .is_some_and(|event| event.r#type() == NSEventType::KeyDown)
    })
}

impl EditorApp {
    /// Moves the menus to the macOS menu bar. If that fails the window keeps its own menus.
    pub(super) fn install_native_menu(&mut self, ctx: &egui::Context) {
        let bar = self.menu_bar(MenuLayout::Mac);
        match NativeMenu::install(ctx, bar) {
            Ok(menu) => {
                self.native_menu = Some(menu);
                self.system_menu_bar = true;
            }
            Err(error) => {
                eprintln!("Could not build the macOS menu bar; the window shows its menus: {error}");
            }
        }
    }

    /// Does what the menu bar was asked to do since the last frame. Call before anything reads
    /// this frame's keys, so a key given back is handled like any other.
    pub(super) fn take_native_menu_input(&mut self, ctx: &egui::Context) {
        let routes = self
            .native_menu
            .as_mut()
            .map(NativeMenu::take_routes)
            .unwrap_or_default();
        for route in routes {
            match route {
                Route::Replay(chord) => {
                    let events = menu_keys::replay(chord, || self.clipboard_text());
                    // The key came before anything else in this frame's input.
                    ctx.input_mut(|input| {
                        let later = std::mem::replace(&mut input.events, events);
                        input.events.extend(later);
                    });
                }
                Route::Run(action) => self.run_menu_action(ctx, action),
            }
        }
    }

    /// Brings the menu bar up to date. If it cannot be, the window shows its own menus again.
    pub(super) fn sync_native_menu(&mut self) {
        let Some(mut menu) = self.native_menu.take() else {
            return;
        };
        let bar = self.menu_bar(MenuLayout::Mac);
        if menu.sync(bar) {
            self.native_menu = Some(menu);
        } else {
            self.system_menu_bar = false;
        }
    }

    fn clipboard_text(&mut self) -> Option<String> {
        self.connect_clipboard();
        self.system_clipboard.as_mut()?.get_text().ok()
    }
}
```

- [ ] **Step 5: Add the fields**

In `src/app/mod.rs`, in `struct EditorApp`, after `native_buttons: bool,` (line 759):

```rust
    /// Whether the menus are in the system menu bar, so the window draws none (macOS).
    system_menu_bar: bool,
    /// The macOS menu bar, while the menus are there.
    #[cfg(target_os = "macos")]
    native_menu: Option<macos_menu::NativeMenu>,
```

In `with_context`, after `native_buttons: chrome::NATIVE_BUTTONS && !cfg!(test),` (line 986):

```rust
            system_menu_bar: false,
            #[cfg(target_os = "macos")]
            native_menu: None,
```

- [ ] **Step 6: Wire it into startup and the frame**

1. After `macos::install_quit_handler(&cc.egui_ctx);` (line 821), add:

```rust
        #[cfg(target_os = "macos")]
        app.install_native_menu(&cc.egui_ctx);
```

2. Make this the first statement of `show_with_processor` (line 2263):

```rust
        // Keys the macOS menu bar took come back before anything reads this frame's input.
        #[cfg(target_os = "macos")]
        self.take_native_menu_input(ctx);
```

3. Replace `self.menus(ctx);` (line 2308) with:

```rust
        #[cfg(target_os = "macos")]
        self.sync_native_menu();
        if !self.system_menu_bar {
            self.menus(ctx);
        }
```

- [ ] **Step 7: Lint and test**

Run: `cargo fmt && cargo clippy --locked --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings. If a muda method name or argument differs from this plan, read its
signature in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/muda-0.20.0/src/items/`
and match it; do not change the behaviour.

Run: `XUAN_REQUIRE_RAW_FIXTURES=1 scripts/check.sh --gpu 2>&1 | tail -5`
Expected: exit 0.

- [ ] **Step 8: Check the build for Linux and Windows stays clean**

Run: `cargo clippy --locked --all-targets --target x86_64-unknown-linux-gnu -- -D warnings 2>&1 | tail -3`
Expected: no warnings. If the target is not installed (`rustup` is not on this Mac), skip this
step and say so in the report: Linux CI runs the same check on push.

- [ ] **Step 9: Launch it**

Run: `XUAN_PLUGIN_PATH="$PWD/plugins" cargo run --release --locked -- --demo`

- [ ] **Step 10: Check by hand, on this Mac**

Each line is one check, with what you should see:

1. The top bar shows `xuan`, File, Edit, Image, Layer, Select, Filter, View, Plugins, Window,
   Help. The window has no menu row.
2. The `xuan` menu: About Xuan, Check for Updates…, Settings… ⌘,, Services ▸, Hide Xuan ⌘H,
   Hide Others ⌥⌘H, Show All, Quit Xuan ⌘Q.
3. File → Save shows ⇧⌘S for Save As and ⌘S for Save. Clicking Save saves.
4. Image → Adjustments shows "Black & White" with its `&`.
5. With the Settings dialog open, the menu items are greyed. Closing it brings them back.
6. In Settings → Keyboard Shortcuts, type in the search field, then press ⌘A and type: the text
   is replaced. ⌘Z undoes the typing.
7. View → Show Rulers: click it, and the tick and the rulers change together.
8. Click in the Settings search field, then press the rulers shortcut (shown in the View menu):
   the rulers do not change, and the tick in View still matches them.
9. Settings → General → Language → 简体中文: the menu titles and AppKit's items (Hide, Show All,
   Zoom) change at once. Switch back to English.
10. Window → Minimize minimizes. Window → Zoom zooms. The window list shows the Xuan window.
11. With a change made, ⌘Q shows the unsaved-changes prompt. Cancel keeps Xuan open.
12. Open a file, then File → Open Recent lists it.

Report each check as passed or failed. A failure is a bug to fix in this task before the commit.

- [ ] **Step 11: Commit**

```bash
git add Cargo.toml Cargo.lock licenses/muda-MIT.txt THIRD_PARTY.md src/app/macos_menu.rs src/app/mod.rs src/app/menu_model.rs
git commit -m "feat(macos): show the menus in the system menu bar"
```

---

### Task 6: The tabs in the Mac title row

**Files:**
- Modify: `src/app/chrome.rs` (constants near line 19; `titlebar_drag`, lines 574-611)
- Modify: `src/app/tabs.rs` (`tabs`, lines 509-548; `tab_strip_ui`, lines 586, 624-638, 654, 813)
- Modify: `src/app/tests/ui_tabs.rs`

**Interfaces:**
- Consumes: `EditorApp::system_menu_bar` (Task 5), `native_buttons`, `title_bar_style()`.
- Produces:
  - `pub(super) const MAC_TITLE_HEIGHT: f32 = 32.0;` (`chrome.rs`)
  - `EditorApp::tabs_in_title_row(&self) -> bool`
  - `EditorApp::title_row_inset(&self, ctx: &egui::Context) -> f32`
  - `pub(super) fn title_row_gesture(ctx: &egui::Context, response: &egui::Response)` (`chrome.rs`)

- [ ] **Step 1: Write the failing tests**

Add to `src/app/tests/ui_tabs.rs`:

```rust
/// Three tabs on macOS, with the menus in the system menu bar.
fn mac_title_row() -> UiTest {
    let mut ui = three_tabs();
    ui.app_mut().native_buttons = true;
    ui.app_mut().system_menu_bar = true;
    ui.settle();
    ui
}

#[test]
fn with_the_mac_menu_bar_the_tabs_are_the_title_row() {
    let mut ui = mac_title_row();
    assert!(!ui.has("File"), "the menus are in the system menu bar");
    let alpha = tab_rect(&ui, "Alpha");
    assert!(alpha.top().abs() < 0.5, "the top row: {alpha:?}");
    assert!(
        (31.5..=32.5).contains(&alpha.height()),
        "32 px, centred on the window buttons: {alpha:?}"
    );
    assert!(alpha.left() >= 75.5, "after the window buttons: {alpha:?}");
    // In full screen macOS hides the buttons, and the tabs move left.
    ui.harness
        .input_mut()
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .fullscreen = Some(true);
    ui.settle();
    let alpha = tab_rect(&ui, "Alpha");
    assert!(alpha.left() < 1.0, "{alpha:?}");
}

#[test]
fn double_clicking_the_empty_title_row_zooms_the_window() {
    let mut ui = mac_title_row();
    ui.app_mut().command_trace = Some(Vec::new());
    let gamma = tab_rect(&ui, "Gamma");
    let pos = gamma.right_center() + egui::vec2(60.0, 0.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    ui.harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(pos));
    ui.harness.step();
    for pressed in [true, false, true, false] {
        ui.harness.input_mut().events.push(button(pressed));
    }
    ui.harness.step();
    let zoomed = ui.harness.output().viewport_output.values().any(|viewport| {
        viewport
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Maximized(true)))
    });
    assert!(zoomed, "a double-click zooms the window");
    ui.settle();
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&[][..]),
        "and starts no document"
    );
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test --locked title_row 2>&1 | grep -E "test result|panicked|assert"`
Expected: both fail. The first fails on the height (36 px) or the left edge (0), as the tab bar
is not yet the title row; the second because a double-click starts a document.

- [ ] **Step 3: Add the title row helpers to `chrome.rs`**

After `NATIVE_BUTTONS_WIDTH` (line 19), add:

```rust
/// The title row's height on macOS. The system centres its window buttons 16 px from the top
/// (measured on macOS 26), so a 32 px row lines up with them.
pub(super) const MAC_TITLE_HEIGHT: f32 = 32.0;

/// The title bar's left margin: the same as in [`title_bar`].
const TITLE_MARGIN: f32 = 14.0;
```

Add to the `impl EditorApp` block, after `trailing_window_controls`:

```rust
    /// Whether the tab bar is the window's top row: macOS's Compact style, with the menus in
    /// the system menu bar.
    pub(super) fn tabs_in_title_row(&self) -> bool {
        self.system_menu_bar
            && self.native_buttons
            && matches!(self.title_bar_style(), TitleBar::Compact)
    }

    /// Room the tab bar leaves at its start for the window buttons, when it is the top row.
    /// macOS hides them in full screen.
    pub(super) fn title_row_inset(&self, ctx: &egui::Context) -> f32 {
        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if self.tabs_in_title_row() && !fullscreen {
            TITLE_MARGIN + NATIVE_BUTTONS_WIDTH
        } else {
            0.0
        }
    }
```

Add a free function after `start_native_drag`:

```rust
/// Moves the window from a drag on an empty part of a title row, and zooms it on a
/// double-click.
pub(super) fn title_row_gesture(ctx: &egui::Context, response: &egui::Response) {
    if response.double_clicked() {
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if response.drag_started_by(egui::PointerButton::Primary) {
        // A native drag grabs the pointer, so let clicks finish before handing it off.
        start_native_drag(ctx, egui::ViewportCommand::StartDrag);
    }
}
```

In `titlebar_drag`, replace its final `if response.double_clicked() { … } else if … { … }`
(lines 602-610) with `title_row_gesture(ui.ctx(), &response);`.

- [ ] **Step 4: Make the tab bar the title row**

In `src/app/tabs.rs`:

1. Add to `impl EditorApp`, before `tabs`:

```rust
    /// The tab bar's height: the title row's on macOS, when the tabs are the top row.
    fn bar_height(&self) -> f32 {
        if self.tabs_in_title_row() {
            super::chrome::MAC_TITLE_HEIGHT
        } else {
            TAB_HEIGHT
        }
    }
```

2. In `tabs`, add `let height = self.bar_height();` before the panel, change
   `.exact_height(TAB_HEIGHT)` to `.exact_height(height)`, and in `horizontal_centered`, before
   `selected = self.tab_strip_ui(ui, &mut actions);`, add:

```rust
                        // The window buttons' corner moves the window, like the title row.
                        let inset = self.title_row_inset(ui.ctx());
                        if inset > 0.0 {
                            let (_, response) = ui.allocate_exact_size(
                                vec2(inset, height),
                                Sense::click_and_drag(),
                            );
                            super::chrome::title_row_gesture(ui.ctx(), &response);
                        }
```

3. In `tab_strip_ui`, add `let height = self.bar_height();` and
   `let title_row = self.tabs_in_title_row();` at the top, then:
   - line 586: `let new_button = height;`
   - line 624: `ui.allocate_exact_size(vec2(width, height), bar_sense);`, with
     `let bar_sense = if title_row { Sense::click_and_drag() } else { Sense::click() };` just above
   - the `empty` interaction (lines 626-636): `bar_sense` in place of `Sense::click()`
   - replace lines 637-639 with:

```rust
        if title_row {
            // The empty bar is the title row: it moves and zooms the window.
            super::chrome::title_row_gesture(ui.ctx(), &background);
            super::chrome::title_row_gesture(ui.ctx(), &empty);
        } else if background.double_clicked() || empty.double_clicked() {
            actions.command = Some("new");
        }
```

   and change the comment above `let empty` to "A double click on the bar's empty part starts a
   new document, as in browsers; as the macOS title row it zooms the window."
   - line 654: `vec2(strip.tab, height),`
   - line 813: `ui.allocate_exact_size(vec2(height, height), Sense::click());`

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test --locked ui_tabs 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: `test result: ok.`, with the two new tests and every existing tab test passing.

- [ ] **Step 6: Check the whole suite and lint**

Run: `XUAN_REQUIRE_RAW_FIXTURES=1 scripts/check.sh --gpu 2>&1 | tail -5`
Expected: exit 0.

- [ ] **Step 7: Check by hand, on this Mac**

Run: `XUAN_PLUGIN_PATH="$PWD/plugins" cargo run --release --locked -- --demo`

1. The tabs sit beside the red, yellow and green buttons, centred on them.
2. Dragging the empty part of the tab row moves the window. Double-clicking it zooms.
3. Dragging a tab still reorders the tabs.
4. Full screen (green button): the tabs start at the left edge.
5. Settings → Appearance → Window title bar → System, then restart Xuan: a normal macOS title
   bar, with the tabs under it at their usual height.

Report each check as passed or failed.

- [ ] **Step 8: Commit**

```bash
git add src/app/chrome.rs src/app/tabs.rs src/app/tests/ui_tabs.rs
git commit -m "feat(macos): put the document tabs in the title row"
```

---

### Task 7: Docs

**Files:**
- Modify: `docs/MACOS.md` (part 2 list, lines 33-50)
- Modify: `docs/USAGE.md` (line 108, lines 114-115, lines 767-771)

- [ ] **Step 1: Update `docs/MACOS.md`**

Replace the unticked native menu bar item (lines 39-40) with:

```markdown
- [x] **Native menu bar.** The menus are in the menu bar at the top of the screen, made from the
  same list as the in-window menus (`src/app/menu_model.rs`) with muda. The Xuan menu holds
  About, Check for Updates, Settings… (⌘,), Services, Hide, Hide Others, Show All and Quit; the
  Window menu adds Minimize, Zoom, Bring All to Front and the window list. An item chosen by
  its shortcut gives the key back to egui, so text fields and the command palette keep their
  keys. ⌘M stays on Curves, so Minimize has no shortcut. In the Compact style the document tabs
  take the title row beside the window buttons. If the menu bar cannot be built, the window
  keeps its own menus.
```

Add to the list under "Checking part 2 on a Mac":

```markdown
- Every menu opens from the menu bar and every item works. The ⌘ hints show, and items grey
  out as in the window's menus (no document, a dialog open, Develop).
- In a text field, ⌘C, ⌘X, ⌘V, ⌘A and ⌘Z edit the text.
- "Black & White" shows its `&`. Changing the language relabels the menus at once.
- Installing or turning off a plugin updates its menu items, and Open Recent updates after an
  open.
- The tab row lines up with the window buttons, moves the window, zooms on a double-click and
  starts at the left edge in full screen.
```

- [ ] **Step 2: Update `docs/USAGE.md`**

1. Line 108: after "The menu bar shares the titlebar with the window controls.", add "On macOS
   the menus are in the menu bar at the top of the screen, and the document tabs share the
   title row with the window buttons."
2. Lines 114-115: change "in a bar below the title bar and above the tool options." to "in a
   bar below the title bar and above the tool options (on macOS, in the title row itself)."
3. Lines 770-771: replace "On macOS the menus sit beside the system's own window buttons
   instead." with "On macOS the menus are in the menu bar at the top of the screen, and the
   document tabs sit beside the system's own window buttons instead."

- [ ] **Step 3: Check and commit**

Run: `grep -n "[–—]" docs/MACOS.md docs/USAGE.md | head`
Expected: no new lines from this task (older lines may already use dashes; leave them).

```bash
git add docs/MACOS.md docs/USAGE.md
git commit -m "docs(macos): describe the native menu bar and the tab title row"
```

- [ ] **Step 4: Final check**

Run: `XUAN_REQUIRE_RAW_FIXTURES=1 scripts/check.sh --gpu 2>&1 | tail -5`
Expected: exit 0.

Run: `git log --oneline 62bfa28..HEAD`
Expected: the design and plan commits, then one commit per task.

Do not push. AGENTS.md: once this reaches `develop` and CI passes, #128 gets a comment naming
the commits, keeps `needs-qa`, and lists the checks from Task 5 step 10 and Task 6 step 7.

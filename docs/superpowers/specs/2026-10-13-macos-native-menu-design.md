# macOS native menu bar and title row: design

- Date: 2026-10-13
- Issue: #128 (Mac keyboard and window conventions), the open "native menu bar" item in
  `docs/MACOS.md`
- Branch: `feat/tabs-above-tool-options`, after commit 62bfa28 (the tabs above the tool options)

## Goal

On macOS, Xuan's menus are in the system menu bar at the top of the screen. Today that bar shows
only winit's default app menu, and Xuan's menus are drawn inside the window. The document tabs then
move up into the title row, beside the red, yellow and green window buttons.

Success means:

- Every menu works from the top bar.
- The menus show ⌘ shortcuts.
- Items grey out in the same cases as now.
- Windows and Linux keep the in-window menus, unchanged.
- The Mac menus have the same items as the in-window menus, because both are made from one list.

## Agreed decisions

- Native menu bar on macOS only. Windows and Linux keep the egui menus.
- Built with the `muda` crate, pinned to `=0.20.0` (published 2026-09-11; 0.21.x was less than two
  weeks old when this was written).
- One shared menu list feeds both renderers.
- The "Xuan" app menu holds About, Check for Updates, Settings… (⌘,), Services, Hide, Hide Others,
  Show All and Quit.
- ⌘M stays on Curves, as in Photoshop. Minimize has no shortcut.
- In the Compact style on macOS, the tab bar is the title row, 32 px tall, so its centre lines up
  with the window buttons. On macOS 26.6 the buttons are centred 16 px from the top of a 32 px
  title bar (measured with a full-size content view and a hidden, transparent title bar).
- The System style on macOS keeps the standard title bar, with the tabs under it.

## Not in scope

- Double-clicking the title row follows the system's "Double-click a window's title bar to"
  setting. That stays a separate item in `docs/MACOS.md`; the title row keeps zooming.
- Tool tips in native menus. muda has no tool tip API.
- The app name in the menu bar. Until the app bundle exists (#129), macOS shows the executable's
  name, `xuan`, as the app menu's title.
- Moving the window buttons. The fixed 32 px row above lines up with them, so Xuan does not move
  them.

## Architecture

`src/app/menus.rs` today both decides what the menus contain and draws them with egui. It is split
into three units.

### 1. The menu list: `src/app/menu_model.rs`

A pure function of the app state, built every frame:

```rust
impl EditorApp {
    pub(super) fn menu_bar(&self, layout: MenuLayout) -> MenuBar;
}

/// Which shape of menus to build. `Mac` adds the app menu and moves items into it.
pub(super) enum MenuLayout { Mac, InWindow }

pub(super) struct MenuBar { pub menus: Vec<TopMenu> }

pub(super) struct TopMenu {
    pub role: Role, // App, File, Edit, Image, Layer, Select, Filter, View, Plugins, Window, Help
    pub label: String,
    pub entries: Vec<MenuEntry>,
}

pub(super) enum MenuEntry {
    Item(MenuItem),
    Submenu { label: String, enabled: bool, entries: Vec<MenuEntry> },
    Separator,
    /// Disabled text, such as "No recent files".
    Placeholder(String),
    /// A macOS item that AppKit runs itself: Services, Hide, Hide Others, Show All, Minimize,
    /// Zoom, Bring All to Front. Only in `MenuLayout::Mac`.
    System(SystemItem),
}

pub(super) struct MenuItem {
    pub label: String,
    /// Already includes every condition: the blocked state, an open document, Develop mode
    /// and `command_enabled`.
    pub enabled: bool,
    pub checked: Option<bool>,
    /// The first binding, for the native key equivalent.
    pub chord: Option<Chord>,
    /// The hint as egui shows it, from `Keymap::shortcut`.
    pub shortcut: String,
    /// egui only: a plugin's source, a recent file's full path.
    pub hover: Option<String>,
    pub action: MenuAction,
}

pub(super) enum MenuAction {
    Command(&'static str),
    Plugin { plugin: String, action: String },
    Recent(PathBuf),
    Adjustment { choice: AdjustmentChoice, layer: bool },
    Filter { filter: Filter, layer: bool },
    Pane { id: String, hidden: bool },
    Snap(SnapTarget), // Guides, Grid, Layers, Bounds
}
```

The types derive `PartialEq` so the Mac renderer can compare frames. Names that clash with muda's
types are qualified in `macos_menu.rs`.

The list keeps today's contents and order exactly, including the labels made in place ("Undo
{name}", "Close RAW Develop", "All", "Inverse", "Enable / Disable") and the nested conditions (Open
Recent follows Open; Develop and Rasterize RAW need an unlocked RAW layer; Clear Guides needs
guides; Snap To items need snapping on). An entry's `enabled` is the AND of every condition that
encloses it today.

`MenuLayout::Mac` differs from `MenuLayout::InWindow` in these ways only:

- A leading `Role::App` menu: About Xuan, Check for Updates…, separator, Settings…, separator,
  Services, separator, Hide Xuan, Hide Others, Show All, separator, Quit Xuan.
- File has no Quit, Edit has no Settings, and Help has no Check for Updates or About (nor the
  separator before them).
- Window starts with Minimize and Zoom and a separator, and ends with a separator and Bring All to
  Front.

The app menu's own items follow the blocked state like the rest of the bar. The `System` items are
always on.

### 2. The egui renderer: `src/app/menus.rs`

It walks the list with today's helpers (`menu_bar_button`, `item_button`, `widgets::menu_check`)
and returns the chosen `MenuAction`. `Pane` items keep the checkbox widget they use today. It is
used for `MenuLayout::InWindow` on every platform, and on macOS as the fallback (see Errors). The
in-window menus look and behave as they do now.

### 3. The Mac renderer: `src/app/macos_menu.rs` (`cfg(target_os = "macos")`)

It owns the muda `Menu`, a map from muda's `MenuId` to `MenuAction`, and the last `MenuBar` it
showed. Once per frame, with the new list:

- The same as last frame: nothing happens.
- Only labels, `enabled`, `checked` or key equivalents differ: those items are updated in place
  (`set_text`, `set_enabled`, `set_checked`, `set_accelerator`).
- Entries were added, removed or changed kind in a top-level menu (plugins, recent files, Develop
  mode): that top-level menu is rebuilt.

The decision is a pure function, `diff(old: &MenuBar, new: &MenuBar) -> Vec<Change>`, so it is
tested on every platform. Only applying the changes needs muda.

Setup happens where `macos::install_quit_handler` runs today (`src/app/mod.rs`, in the app's
creation, after NSApp has launched):

- `init_for_nsapp()` on the menu, which replaces winit's default menu.
- `set_as_windows_menu_for_nsapp()` on Window, so macOS adds its window list.
- `set_as_help_menu_for_nsapp()` on Help, so macOS adds its search field.
- `MenuEvent::set_event_handler`, which queues each event and calls `request_repaint`.

The event handler runs on the main thread, inside AppKit's menu action. At that moment it reads
`NSApp.currentEvent` and records whether the item ran from a key press (see Keys).

### Running a choice

`EditorApp::run_menu_action(MenuAction)` replaces the block at the end of today's `menus()`. Both
renderers end in it. A `Command` runs through `run_command` as today.

### Frame order

At the start of `update`, before any input is read, the Mac renderer drains its queue: key replays
go into `ctx.input_mut(|i| i.events)` and clicks go to `run_menu_action`. Then the Mac renderer
syncs the new list. `menus(ctx)` draws the egui bar only when the Mac menu is not in use.

## Keys

AppKit offers a key press to the main menu's key equivalents before winit's view sees it, and
winit's view does not take key equivalents itself. A native shortcut would therefore skip egui:
text fields would lose ⌘C, ⌘V, ⌘X, ⌘A and ⌘Z, and the command palette and Develop scopes would be
bypassed.

The rule:

- **From a key** (`NSApp.currentEvent` is a key-down that matches the item's key equivalent): the
  key press is replayed into egui at the start of the next frame, as winit would have sent it.
  ⌘C, ⌘X and ⌘V become `Event::Copy`, `Event::Cut` and `Event::Paste(clipboard text)`, as
  egui-winit makes them. egui then handles the key exactly as today: text fields, the palette,
  `shortcuts()` and its scopes.
- **From a click**: the item's `MenuAction` runs.

This is the pure function `route(from_key: Option<Chord>, action) -> Route` (`Replay(events)` or
`Run(action)`), tested on every platform. A disabled native item takes no key, so the press reaches
egui directly, which gives the same result.

Every item with a binding gets a key equivalent, so the menus show every hint. A chord that has no
muda `Accelerator` form gets no key equivalent and no hint on macOS; its key still works through
egui. The Control key forms (Control+S, Control+Tab) are not key equivalents and reach egui as
today.

Clashes with macOS: ⌘H (Hide) already has no command default on macOS (`MAC_SYSTEM`). ⌥⌘H (Hide
Others) is unused. ⌘M stays on Curves, so Minimize gets no key equivalent. If a user or plugin binds
a chord that a `System` item uses, AppKit's item wins, as in other Mac apps.

## Mac title row

Only with `title_bar_style() == Compact` and the native menu in use:

- No egui menu row. The tab bar is the top panel, `MAC_TITLE_HEIGHT = 32.0` px tall.
  `TAB_HEIGHT` stays 36 px everywhere else.
- The tabs start after the window buttons: the same leading space as today's
  `NATIVE_BUTTONS_WIDTH` plus the title bar's left margin. In full screen (`ViewportInfo::fullscreen`)
  macOS hides the buttons, and the space goes.
- The empty part of the bar (the area `tab_strip_ui` marks as `tab_bar_empty`, and the background
  behind the tabs) moves the window with a native drag and zooms on a double-click, as
  `titlebar_drag` does for today's title row. Elsewhere a double-click there still starts a new
  document.
- The zoom buttons (Fit, 100%, +, −) stay at the right of the bar.

With the System style, macOS draws its own title bar, there is no egui menu row, and the tab bar
keeps 36 px under the title bar.

## Errors

- If muda fails to build or attach the menu, Xuan logs the error with `eprintln!` (as
  `src/app/macos.rs` does) and falls back to the egui menu row and today's title row. The user is
  never left without menus.
- A failed in-place update or rebuild is logged, and the next frame rebuilds the whole menu from the
  list.
- A muda event whose id is not in the map (an item from a menu that was just rebuilt) is ignored.

## Testing

On every platform (no Mac needed):

- `menu_model` unit tests:
  - Both layouts: the app menu and the items that move.
  - The Undo and Redo labels.
  - The blocked state greys every item.
  - No document, Develop mode, the RAW layer conditions and Open Recent following Open.
  - Check states for channels, pixel grid, rulers, snapping and Snap To.
  - Plugin items keep the "· Plugin name" label.
  - Recent files, including a missing file and the empty placeholder.
- `diff` unit tests: no change, a state-only change, and a structure change in one menu.
- `route` unit tests: a key replay for ⌘S, the copy, cut and paste events, and a click.
- A kittest layout test with the title bar style forced to Compact and the native menu forced on:
  there is no menu bar panel, the tab bar is the top panel and 32 px tall, and the first tab starts
  after the window buttons' space.
- The existing menu UI tests pass unchanged. They show the egui renderer still draws the same menus.

On macOS CI (`.github/workflows/macos.yml`, which already runs the tests):

- Every default chord of every menu item converts to a muda `Accelerator`.

`scripts/check.sh --gpu` passes.

## Docs and the issue

- `docs/MACOS.md`: tick the native menu bar item, and add the Mac checks below to "Checking part 2
  on a Mac".
- `docs/USAGE.md`: on macOS the menus are in the menu bar, and in the Compact style the tabs share
  the row with the window buttons.
- When the work is on `develop` and CI passes, #128 gets a comment naming the commits and keeps the
  `needs-qa` label, with this "To test" list:
  - Every menu opens from the top bar and every item works.
  - The ⌘ hints show, and items grey out as in the window menus (no document, a dialog open,
    Develop mode).
  - In a text field (a layer name, Settings), ⌘C, ⌘X, ⌘V, ⌘A and ⌘Z edit the text.
  - ⌘Q with unsaved changes shows the prompt. Hide, Hide Others, Show All, Minimize, Zoom and the
    window list work.
  - Installing or turning off a plugin updates its menu items. Open Recent updates after an open.
  - The tab row lines up with the window buttons, moves the window, zooms on a double-click, and
    loses the button space in full screen.

## Security and supply chain

- New third-party code: `muda` 0.20.0 and its new dependencies `crossbeam-channel` and
  `keyboard-types`, all MIT or Apache-2.0. The other dependencies (`objc2`, `objc2-app-kit`,
  `objc2-foundation`, `png`, `dpi`, `thiserror`) are already in `Cargo.lock`.
- macOS only (`[target.'cfg(target_os = "macos")'.dependencies]`). Windows and Linux builds do not
  compile it.
- Pinned exactly, with `Cargo.lock` committed. Review the crate before the first build, and review
  any later bump the same way.
- No network or file access. Plugin labels from third-party manifests reach `NSMenuItem` titles as
  plain text, and keep the "· Plugin name" suffix, so a plugin item cannot pass for a built-in one.
- CI has no dependency audit (`cargo audit` or `cargo deny`). That is outside this change, but it
  is worth adding for the ISO 27001 and SOC 2 controls.

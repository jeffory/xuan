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
                eprintln!(
                    "Could not build the macOS menu bar; the window shows its menus: {error}"
                );
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

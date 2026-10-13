//! What the macOS menu bar changes to show a new menu list (`menu_sync.rs`).

use super::*;
use crate::app::{commands::Chord, menu_model::MenuAction};
use crate::app::{
    menu_model::{MenuBar, MenuEntry, MenuLayout, Role},
    menu_sync::{MenuChange, Sync, diff, item_at, menu_text},
};
use egui::{Key, Modifiers};

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

#[test]
fn showing_or_hiding_a_panel_changes_the_window_menu_in_place() {
    let (_, mut app) = app_with_document();
    let old = app.menu_bar(MenuLayout::Mac);
    let (id, _, shown) = app.pane_entries().into_iter().next().expect("a pane");
    app.config.panes.set_hidden(&id, shown);
    let new = app.menu_bar(MenuLayout::Mac);
    // A rebuild would scramble the window list AppKit keeps in this menu.
    assert_eq!(
        per_menu(&old, &new)[position(&old, Role::Window)],
        MenuChange::State
    );
}

/// The first item with a shortcut in a top-level menu.
fn first_shortcut(bar: &mut MenuBar, menu: usize) -> &mut Option<Chord> {
    bar.menus[menu]
        .entries
        .iter_mut()
        .find_map(|entry| match entry {
            MenuEntry::Item(item) if item.chord.is_some() => Some(&mut item.chord),
            _ => None,
        })
        .expect("a shortcut")
}

#[test]
fn a_changed_shortcut_changes_in_place_but_a_removed_one_rebuilds() {
    let (_, mut app) = app_with_document();
    let old = app.menu_bar(MenuLayout::Mac);
    let file = position(&old, Role::File);
    let mut changed = old.clone();
    *first_shortcut(&mut changed, file) = Some(Chord {
        mods: Modifiers::CTRL.plus(Modifiers::ALT),
        key: Key::N,
    });
    assert_eq!(per_menu(&old, &changed)[file], MenuChange::State);
    // muda cannot clear a key equivalent in place.
    let mut removed = old.clone();
    *first_shortcut(&mut removed, file) = None;
    assert_eq!(per_menu(&old, &removed)[file], MenuChange::Rebuild);
}

#[test]
fn an_item_is_found_by_its_path_with_its_current_shortcut() {
    let (_, mut app) = app_with_document();
    let mut bar = app.menu_bar(MenuLayout::Mac);
    let file = position(&bar, Role::File);
    assert_eq!(
        item_at(&bar, &[file, 0]).map(|item| &item.action),
        Some(&MenuAction::Command("new"))
    );
    let recent = bar.menus[file]
        .entries
        .iter()
        .position(|entry| matches!(entry, MenuEntry::Submenu { .. }))
        .unwrap();
    assert_eq!(
        item_at(&bar, &[file, recent, 2]).map(|item| &item.action),
        Some(&MenuAction::Command("clear_recent"))
    );
    // After an in-place change, the same path finds the new shortcut.
    let chord = Chord {
        mods: Modifiers::CTRL.plus(Modifiers::ALT),
        key: Key::N,
    };
    if let MenuEntry::Item(item) = &mut bar.menus[file].entries[0] {
        item.chord = Some(chord);
    }
    assert_eq!(item_at(&bar, &[file, 0]).unwrap().chord, Some(chord));
    assert!(item_at(&bar, &[file, 999]).is_none());
    assert!(item_at(&bar, &[file]).is_none(), "a menu is not an item");
    assert!(item_at(&bar, &[file, recent]).is_none(), "nor is a submenu");
}

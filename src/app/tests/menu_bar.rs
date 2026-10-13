//! The menu list (`menu_model.rs`): what each menu holds, and when its items can run.

use super::*;
use crate::app::menu_model::{MenuAction, MenuBar, MenuEntry, MenuItem, MenuLayout, Role, TopMenu};

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
            MenuEntry::System { item, .. } => format!("[{item:?}]"),
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
    let bar = app.menu_bar(MenuLayout::InWindow);
    let labels: Vec<&str> = bar.menus.iter().map(|menu| menu.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "File", "Edit", "Image", "Layer", "Select", "Filter", "View", "Plugins", "Window",
            "Help"
        ]
    );
    assert_eq!(
        outline(&menu(&bar, "File").entries),
        [
            "new",
            "new_collage",
            "open",
            "open_clipboard",
            "open_comp",
            "import",
            "> Open Recent",
            "---",
            "save",
            "save_as",
            "export",
            "---",
            "close",
            "---",
            "quit",
        ]
    );
    assert_eq!(
        outline(&menu(&bar, "Edit").entries),
        [
            "settings",
            "---",
            "undo",
            "redo",
            "---",
            "cut",
            "copy",
            "copy_merged",
            "paste",
            "---",
            "fill_fg",
            "fill_bg",
            "clear",
            "stroke",
            "content_fill",
        ]
    );
    assert_eq!(
        outline(submenu(&menu(&bar, "Layer").entries, "Layer Mask")),
        [
            "new_mask_layer",
            "mask",
            "disable_mask",
            "link_mask",
            "delete_mask"
        ]
    );
    assert_eq!(
        outline(&menu(&bar, "Help").entries),
        [
            "command_palette",
            "shortcuts",
            "---",
            "check_updates",
            "about"
        ]
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
    let bar = app.menu_bar(MenuLayout::InWindow);
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
    let bar = app.menu_bar(MenuLayout::InWindow);
    assert!(bar.menus.iter().all(|menu| !menu.enabled));
    assert!(bar.items().iter().all(|item| !item.enabled));
}

#[test]
fn undo_and_redo_name_the_step() {
    let (_, mut app) = app_with_document();
    let undo = app
        .menu_bar(MenuLayout::InWindow)
        .find_command("undo")
        .cloned()
        .unwrap();
    assert_eq!((undo.label.as_str(), undo.enabled), ("Undo ", false));
    app.command("new_layer");
    let undo = app
        .menu_bar(MenuLayout::InWindow)
        .find_command("undo")
        .cloned()
        .unwrap();
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
    let bar = app.menu_bar(MenuLayout::InWindow);
    assert_eq!(
        outline(submenu(&menu(&bar, "File").entries, "Open Recent")),
        ["(No recent files)", "---", "clear_recent"]
    );

    app.config.recent_files = vec![here.clone(), gone.clone()];
    let bar = app.menu_bar(MenuLayout::InWindow);
    let entries = submenu(&menu(&bar, "File").entries, "Open Recent");
    let [MenuEntry::Item(first), MenuEntry::Item(second), ..] = entries else {
        panic!("{entries:?}");
    };
    assert_eq!(
        (&first.action, first.enabled),
        (&MenuAction::Recent(here), true)
    );
    assert_eq!(
        (&second.action, second.enabled),
        (&MenuAction::Recent(gone), false)
    );
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
    let bar = app.menu_bar(MenuLayout::InWindow);
    assert_eq!(
        bar.find_command("toggle_rulers").unwrap().checked,
        Some(true)
    );
    assert_eq!(
        bar.find_command("toggle_snap").unwrap().checked,
        Some(false)
    );
    assert_eq!(
        bar.find_command("channel_composite").unwrap().checked,
        Some(true)
    );
    assert_eq!(
        bar.find_command("channel_red").unwrap().checked,
        Some(false)
    );
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
    let bar = app.menu_bar(MenuLayout::InWindow);
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
    for layout in [MenuLayout::InWindow, MenuLayout::Mac] {
        for menu in app.menu_bar(layout).menus {
            assert_tidy(&menu.entries, &menu.label);
        }
    }
    app.dimensions = [20, 16];
    app.new_document();
    for layout in [MenuLayout::InWindow, MenuLayout::Mac] {
        for menu in app.menu_bar(layout).menus {
            assert_tidy(&menu.entries, &menu.label);
        }
    }
    app.develop = Some(crate::app::develop::tests::ready(&context));
    for layout in [MenuLayout::InWindow, MenuLayout::Mac] {
        for menu in app.menu_bar(layout).menus {
            assert_tidy(&menu.entries, &menu.label);
        }
    }
}

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
            Role::App,
            Role::File,
            Role::Edit,
            Role::Image,
            Role::Layer,
            Role::Select,
            Role::Filter,
            Role::View,
            Role::Plugins,
            Role::Window,
            Role::Help,
        ]
    );
    assert_eq!(
        outline(top(&bar, Role::App)),
        [
            "about",
            "check_updates",
            "---",
            "settings",
            "---",
            "[Services]",
            "---",
            "[Hide]",
            "[HideOthers]",
            "[ShowAll]",
            "---",
            "quit",
        ]
    );
    assert_eq!(bar.find_command("quit").unwrap().label, "Quit Xuan");
    assert_eq!(
        outline(top(&bar, Role::File)).last().map(String::as_str),
        Some("close")
    );
    // Settings has gone, and so has the separator that followed it.
    assert_eq!(outline(top(&bar, Role::Edit))[..2], ["undo", "redo"]);
    assert_eq!(
        outline(top(&bar, Role::Help)),
        ["command_palette", "shortcuts"]
    );
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
    assert!(
        bar.items()
            .iter()
            .all(|item| item.action != MenuAction::Minimize)
    );
    assert!(
        bar.menus
            .iter()
            .flat_map(|menu| &menu.entries)
            .all(|entry| !matches!(entry, MenuEntry::System { .. }))
    );
}

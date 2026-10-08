//! The command palette, driven like a user: Ctrl+K, typing, arrows, Enter, Escape and clicks.

use super::*;
use crate::app::palette::{FILTER_ID, rank};
use egui::{Key, Modifiers};

/// Opens the palette over an editor with a document, keeping the configuration in a temporary
/// directory.
fn open() -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    ui.press(Modifiers::CTRL, Key::K);
    (directory, ui)
}

fn is_open(ui: &UiTest) -> bool {
    ui.app().palette.is_some()
}

fn selected(ui: &UiTest) -> usize {
    ui.app().palette.as_ref().unwrap().selected
}

fn filter_has_focus(ui: &UiTest) -> bool {
    ui.ctx()
        .memory(|memory| memory.has_focus(egui::Id::new(FILTER_ID)))
}

/// The ids the palette lists for its current filter.
fn listed(ui: &UiTest) -> Vec<String> {
    let app = ui.app();
    let query = &app.palette.as_ref().unwrap().query;
    rank(
        app.keymap.entries(),
        false,
        query,
        &app.config.recent_commands,
    )
    .iter()
    .map(|row| app.keymap.entries()[row.entry].id.clone())
    .collect()
}

#[test]
fn ctrl_k_opens_it_with_the_filter_focused_and_toggles_it_closed() {
    let (_directory, mut ui) = open();
    assert!(is_open(&ui));
    assert!(filter_has_focus(&ui));
    // Rows under the Edit header show the label alone.
    assert!(ui.has("Undo"));
    // Ctrl+K closes it even though the filter has focus, and opens it again.
    ui.press(Modifiers::CTRL, Key::K);
    assert!(!is_open(&ui));
    assert!(!ui.has_role(Role::Button, "Undo"));
    ui.press(Modifiers::CTRL, Key::K);
    assert!(is_open(&ui));
    assert!(filter_has_focus(&ui));
}

/// Under a category's header a row does not repeat the category ("New Canvas…  File" under
/// "File"); Recent rows, which mix categories, still name theirs (issue 91).
#[test]
fn rows_under_a_category_header_leave_the_category_out() {
    let (_directory, mut ui) = open();
    ui.app_mut().config.recent_commands = vec!["undo".into()];
    ui.press(Modifiers::CTRL, Key::K);
    ui.press(Modifiers::CTRL, Key::K);
    assert!(is_open(&ui));
    assert!(ui.has("Recent"));
    // The File header, beside the File menu.
    assert!(ui.harness.query_all_by_label("File").count() >= 2);
    assert!(ui.has("New Canvas…") && !ui.has("New Canvas…, File"));
    // Undo moves to Recent, which names its category; Redo stays under Edit, which does not.
    assert!(ui.has("Undo, Edit") && !ui.has("Undo"));
    assert!(ui.has("Redo") && !ui.has("Redo, Edit"));
}

#[test]
fn it_opens_from_the_help_menu() {
    let mut ui = UiTest::with_document();
    ui.open_menu("Help");
    ui.click("Command Palette… Ctrl+K");
    assert!(is_open(&ui));
}

#[test]
fn typing_filters_the_list() {
    let (_directory, mut ui) = open();
    assert!(ui.has("Swap Colours") && ui.has("Open…"));
    ui.type_keys("swap");
    // Search results have no headers: each names its category.
    assert!(ui.has("Swap Colours, Tools"));
    assert!(!ui.has("Open…") && !ui.has("Open…, File"));
    assert_eq!(listed(&ui).first().map(String::as_str), Some("swap_colors"));
    ui.type_keys("zzzz");
    assert!(ui.has("No matching commands"));
}

#[test]
fn enter_runs_the_selected_command_and_closes() {
    let (_directory, mut ui) = open();
    ui.app_mut().brush.color = [10, 20, 30, 255];
    ui.app_mut().background = [200, 190, 180, 255];
    ui.type_keys("swap col");
    ui.key(Key::Enter);
    assert!(!is_open(&ui));
    assert_eq!(ui.app().brush.color, [200, 190, 180, 255]);
    assert_eq!(ui.app().background, [10, 20, 30, 255]);
    assert_eq!(ui.app().config.recent_commands, ["swap_colors"]);
}

#[test]
fn arrows_and_pages_move_the_selection_and_enter_runs_that_row() {
    let (_directory, mut ui) = open();
    ui.type_keys("flip");
    let found = listed(&ui);
    assert!(found.len() >= 2, "{found:?}");
    ui.app_mut().command_trace = Some(Vec::new());
    ui.key(Key::ArrowDown);
    assert_eq!(selected(&ui), 1);
    ui.key(Key::ArrowDown);
    ui.key(Key::ArrowUp);
    assert_eq!(selected(&ui), 1);
    ui.key(Key::PageDown);
    assert_eq!(selected(&ui), found.len() - 1);
    ui.key(Key::PageUp);
    assert_eq!(selected(&ui), 0);
    ui.key(Key::ArrowUp);
    assert_eq!(selected(&ui), found.len() - 1);
    ui.key(Key::ArrowUp);
    let expected = found[found.len() - 2].clone();
    ui.key(Key::Enter);
    assert!(!is_open(&ui));
    assert_eq!(ui.app().command_trace.as_deref(), Some(&[expected][..]));
}

#[test]
fn a_new_filter_selects_the_best_match() {
    let (_directory, mut ui) = open();
    ui.key(Key::ArrowDown);
    ui.key(Key::ArrowDown);
    assert_eq!(selected(&ui), 2);
    ui.type_keys("u");
    assert_eq!(selected(&ui), 0);
}

#[test]
fn escape_and_clicking_outside_close_it() {
    let (_directory, mut ui) = open();
    ui.key(Key::Escape);
    assert!(!is_open(&ui));

    ui.press(Modifiers::CTRL, Key::K);
    assert!(is_open(&ui));
    // Below the list, over the canvas: closes and does not paint or select anything there.
    let undo = |ui: &UiTest| {
        ui.app()
            .session()
            .unwrap()
            .history
            .undo_name()
            .map(str::to_owned)
    };
    let before = undo(&ui);
    ui.click_at(egui::pos2(300.0, 800.0));
    assert!(!is_open(&ui));
    assert_eq!(undo(&ui), before);
    // Clicking inside the palette does not close it.
    ui.press(Modifiers::CTRL, Key::K);
    ui.click_at(egui::pos2(640.0, 90.0));
    assert!(is_open(&ui));
}

#[test]
fn clicking_a_row_runs_it_and_remembers_it() {
    let (directory, mut ui) = open();
    ui.app_mut().brush.color = [1, 2, 3, 255];
    ui.app_mut().background = [4, 5, 6, 255];
    ui.type_keys("swap");
    ui.click("Swap Colours, Tools");
    assert!(!is_open(&ui));
    assert_eq!(ui.app().brush.color, [4, 5, 6, 255]);
    let saved = std::fs::read_to_string(directory.path().join("config.toml")).unwrap();
    assert!(
        saved.contains("recent_commands") && saved.contains("swap_colors"),
        "{saved}"
    );

    // The next time it is the first row, under Recent.
    ui.press(Modifiers::CTRL, Key::K);
    assert_eq!(listed(&ui).first().map(String::as_str), Some("swap_colors"));
    assert!(ui.has("Recent"));
}

#[test]
fn it_does_not_open_over_a_dialog() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    ui.press(Modifiers::CTRL, Key::Comma);
    assert!(ui.app().dialog == Some(Dialog::Settings));
    ui.press(Modifiers::CTRL, Key::K);
    assert!(!is_open(&ui));
    assert!(ui.app().dialog == Some(Dialog::Settings));
    ui.key(Key::Escape);
    assert!(ui.app().dialog.is_none());
    ui.press(Modifiers::CTRL, Key::K);
    assert!(is_open(&ui));
}

#[test]
fn disabled_commands_are_greyed_and_do_not_run() {
    // No document: Merge Down needs one.
    let mut ui = UiTest::new();
    ui.press(Modifiers::CTRL, Key::K);
    ui.app_mut().command_trace = Some(Vec::new());
    ui.type_keys("merge down");
    assert!(ui.has("Merge Down / Selected, Layer"));
    assert!(!ui.enabled("Merge Down / Selected, Layer"));
    ui.key(Key::Enter);
    assert!(is_open(&ui), "Enter on a disabled command does nothing");
    ui.click("Merge Down / Selected, Layer");
    assert!(is_open(&ui));
    assert_eq!(ui.app().command_trace.as_deref(), Some(&[][..]));
    assert!(ui.app().config.recent_commands.is_empty());
}

#[test]
fn the_toggle_is_rebindable() {
    let mut ui = UiTest::new();
    ui.app_mut().config.keybindings.insert(
        "command_palette".into(),
        toml::Value::String("Ctrl+Shift+P".into()),
    );
    ui.app_mut().rebuild_keymap();
    ui.press(Modifiers::CTRL, Key::K);
    assert!(!is_open(&ui));
    let chord = Modifiers::CTRL | Modifiers::SHIFT;
    ui.press(chord, Key::P);
    assert!(is_open(&ui));
    assert!(ui.has("Command Palette…"));
    ui.press(chord, Key::P);
    assert!(!is_open(&ui));
}

#[test]
fn translated_labels_are_listed_and_searched() {
    let mut ui = UiTest::new();
    ui.app_mut().config.language = xuan::config::Language::new("zh-CN");
    ui.settle();
    ui.press(Modifiers::CTRL, Key::K);
    ui.type_keys("新建");
    assert!(ui.has("新建画布…, 文件"));
    ui.type_keys("x");
    assert!(!ui.has("新建画布…, 文件"));
}

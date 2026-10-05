//! Settings → Keyboard Shortcuts, driven like a user: search, click a shortcut, press keys.

use super::*;
use egui::{Key, Modifiers};

const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);
const MERGE: &str = "Merge Down / Selected";

/// Opens Settings on the Keyboard Shortcuts page, filtered to `search`.
fn keyboard_page(search: &str) -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    ui.press(Modifiers::CTRL, Key::Comma);
    assert!(ui.app().dialog == Some(Dialog::Settings));
    ui.click("Keyboard Shortcuts");
    ui.type_in_text_field(search);
    (directory, ui)
}

fn merge_keys(ui: &UiTest) -> Vec<String> {
    ui.app()
        .keymap
        .keys("merge")
        .iter()
        .map(|k| k.to_string())
        .collect()
}

fn saved(directory: &tempfile::TempDir) -> String {
    std::fs::read_to_string(directory.path().join("config.toml")).unwrap_or_default()
}

#[test]
fn capture_rebinds_and_escape_or_backspace_cancel_or_clear() {
    let (directory, mut ui) = keyboard_page("merge");
    assert!(ui.has(&format!("{MERGE}: Ctrl+E")));
    // Commands that do not match the search are hidden.
    assert!(!ui.has("Save: Ctrl+S"));

    // Escape cancels the capture without closing Settings.
    ui.click(&format!("{MERGE}: Ctrl+E"));
    assert!(ui.has("Press keys…"));
    ui.key(Key::Escape);
    assert!(ui.app().dialog == Some(Dialog::Settings));
    assert_eq!(merge_keys(&ui), ["Ctrl+E"]);

    // A new chord replaces the clicked one, is saved and dispatches.
    ui.click(&format!("{MERGE}: Ctrl+E"));
    ui.press(CTRL_SHIFT, Key::M);
    assert_eq!(merge_keys(&ui), ["Ctrl+Shift+M"]);
    assert!(ui.has(&format!("{MERGE}: Ctrl+Shift+M")));
    assert!(saved(&directory).contains("merge = \"Ctrl+Shift+M\""));

    // A plain letter is refused for a command that is not a tool.
    ui.click(&format!("Add shortcut: {MERGE}"));
    ui.key(Key::K);
    assert_eq!(merge_keys(&ui), ["Ctrl+Shift+M"]);
    assert!(ui.has("Single letters and digits are kept for tools. Add Ctrl or Alt."));

    // + adds a second shortcut; Backspace removes the clicked one.
    ui.click(&format!("Add shortcut: {MERGE}"));
    ui.press(Modifiers::CTRL | Modifiers::ALT, Key::M);
    assert_eq!(merge_keys(&ui), ["Ctrl+Shift+M", "Ctrl+Alt+M"]);
    ui.click(&format!("{MERGE}: Ctrl+Shift+M"));
    ui.key(Key::Backspace);
    assert_eq!(merge_keys(&ui), ["Ctrl+Alt+M"]);
    assert!(saved(&directory).contains("merge = \"Ctrl+Alt+M\""));

    // Escape now closes Settings, and the new binding runs Merge, as the menu shows.
    ui.key(Key::Escape);
    assert!(ui.app().dialog.is_none());
    ui.app_mut().dimensions = [20, 16];
    ui.app_mut().new_document();
    ui.settle();
    ui.app_mut().command_trace = Some(Vec::new());
    ui.press(Modifiers::CTRL, Key::E);
    ui.press(Modifiers::CTRL | Modifiers::ALT, Key::M);
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&["merge".to_owned()][..])
    );
    ui.app_mut().command_trace = None;
    ui.open_menu("Layer");
    assert!(ui.has(&format!("{MERGE} Ctrl+Alt+M")));
}

#[test]
fn a_used_chord_asks_to_reassign_or_cancel() {
    let (directory, mut ui) = keyboard_page("merge");
    ui.click(&format!("{MERGE}: Ctrl+E"));
    ui.press(Modifiers::CTRL, Key::S);
    // Nothing changes until the user answers.
    assert!(ui.has("Ctrl+S is already used by “Save”."));
    assert_eq!(merge_keys(&ui), ["Ctrl+E"]);
    ui.click("Cancel");
    assert!(!ui.has("Reassign"));
    assert_eq!(merge_keys(&ui), ["Ctrl+E"]);
    assert_eq!(ui.app().keymap.shortcut("save"), "Ctrl+S");

    // Escape also cancels the prompt, and leaves Settings open.
    ui.click(&format!("{MERGE}: Ctrl+E"));
    ui.press(Modifiers::CTRL, Key::S);
    ui.key(Key::Escape);
    assert!(!ui.has("Reassign"));
    assert!(ui.app().dialog == Some(Dialog::Settings));

    ui.click(&format!("{MERGE}: Ctrl+E"));
    ui.press(Modifiers::CTRL, Key::S);
    ui.click("Reassign");
    assert_eq!(merge_keys(&ui), ["Ctrl+S"]);
    assert!(ui.app().keymap.keys("save").is_empty());
    let text = saved(&directory);
    assert!(
        text.contains("merge = \"Ctrl+S\"") && text.contains("save = \"\""),
        "{text}"
    );
}

#[test]
fn reset_restores_one_command_or_all() {
    let (directory, mut ui) = keyboard_page("merge");
    ui.click(&format!("{MERGE}: Ctrl+E"));
    ui.press(CTRL_SHIFT, Key::M);
    ui.app_mut()
        .config
        .keybindings
        .insert("deselect".into(), toml::Value::String(String::new()));
    ui.app_mut().rebuild_keymap();
    ui.settle();
    assert_eq!(ui.app().config.keybindings.len(), 2);

    ui.click(&format!("Reset: {MERGE}"));
    assert_eq!(merge_keys(&ui), ["Ctrl+E"]);
    assert!(!ui.has(&format!("Reset: {MERGE}")));
    assert_eq!(ui.app().config.keybindings.len(), 1);

    ui.click("Reset All");
    assert!(ui.app().config.keybindings.is_empty());
    assert_eq!(ui.app().keymap.shortcut("deselect"), "Ctrl+D");
    assert!(!saved(&directory).contains("keybindings"));
    assert!(!ui.enabled("Reset All"));
}

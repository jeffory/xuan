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
    // No document, so the search field is the only text field.
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    ui.press(Modifiers::CTRL, Key::Comma);
    ui.click("Keyboard Shortcuts");
    ui.type_in_text_field("merge");
    ui.app_mut().command_trace = Some(Vec::new());
    // ⌘A selects the field's text.
    replayed(
        &mut ui,
        Chord {
            mods: Modifiers::CTRL,
            key: Key::A,
        },
    );
    ui.type_keys("blur");
    assert_eq!(
        ui.harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .value()
            .as_deref(),
        Some("blur")
    );
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&[][..]),
        "no command ran"
    );
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

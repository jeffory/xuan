//! With no document open the options bar, zoom controls and canvas hints give way to an invitation.

use super::*;
use egui::{Key, Modifiers};

const HINT: &str = "Drop an image here · Ctrl+O opens a file · Ctrl+N starts a new canvas";

#[test]
fn nothing_to_use_is_shown_until_a_document_opens() {
    let mut ui = UiTest::new();
    assert!(!ui.has("Auto Select"));
    assert!(!ui.has("Fit"));
    assert!(!ui.has("100%"));
    assert!(ui.has(HINT));
    assert!(!ui.has(Tool::Move.hint()));

    ui.app_mut().dimensions = [20, 16];
    ui.app_mut().new_document();
    ui.settle();
    assert!(ui.has("Auto Select"));
    assert!(ui.has("Fit"));
    assert!(ui.has("100%"));
    assert!(!ui.has(HINT));
    assert!(ui.has(Tool::Move.hint()));
}

#[test]
fn the_hint_follows_the_key_bindings() {
    let mut ui = UiTest::new();
    let directory = tempfile::tempdir().unwrap();
    ui.isolate_config(directory.path());
    let chord = commands::Chord::from_event(Modifiers::CTRL.plus(Modifiers::SHIFT), Key::J);
    let app = ui.app_mut();
    let mut overrides = std::mem::take(&mut app.config.keybindings);
    commands::set_override(&mut overrides, &app.keymap, "new", &[chord]);
    app.config.keybindings = overrides;
    app.rebuild_keymap();
    ui.settle();
    assert!(ui.has("Drop an image here · Ctrl+O opens a file · Ctrl+Shift+J starts a new canvas"));
}

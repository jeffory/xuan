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
    assert_eq!(
        key(Modifiers::ALT, Key::Backspace).as_deref(),
        Some("\u{8}")
    );
    assert_eq!(
        key(Modifiers::NONE, Key::Delete).as_deref(),
        Some("\u{f728}")
    );
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
    assert_eq!(
        replay(chord(Modifiers::CTRL, Key::C), || None),
        [Event::Copy]
    );
    assert_eq!(replay(chord(CTRL_SHIFT, Key::C), || None), [Event::Copy]);
    assert_eq!(
        replay(chord(Modifiers::CTRL, Key::X), || None),
        [Event::Cut]
    );
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

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

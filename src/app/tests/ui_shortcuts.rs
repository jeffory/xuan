//! Every shortcut documented in `docs/SHORTCUTS.md` must do what the document says.
//!
//! The document is parsed, each chord in its table is pressed as a real key event, and the test
//! checks the effect. A chord that is documented but has no entry in [`expected`], or an entry for
//! a chord that is no longer documented, fails the test, so the two cannot drift apart silently.

use super::*;
use egui::{Key, Modifiers};

const DOC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/SHORTCUTS.md"));

/// What pressing a chord must achieve.
enum Effect {
    /// The command is dispatched. Commands are only recorded, never run, because several of them
    /// open native file dialogs.
    Command(&'static str),
    /// The tool becomes active.
    Tool(Tool),
    /// Prepares the app, then checks state after the press.
    State(fn(&mut EditorApp), fn(&EditorApp) -> bool),
}

use Effect::{Command, State};

/// Shortcut cell text that is not a key chord. Pointer gestures and key groups are listed here and
/// checked elsewhere, so a new prose entry in the document also has to be acknowledged.
const PROSE: &[&str] = &[
    "0 for 100%",
    "Arrow keys",
    "Horizontal mouse wheel",
    "Number keys 1–9",
    "Shift+arrow keys",
    "Shift+wheel over the canvas",
    "Space-drag",
    "Wheel up",
    "down over the control",
    "middle-button drag",
    "mouse wheel",
];

fn expected(chord: &str) -> Option<Effect> {
    Some(match chord {
        "Ctrl+N" => Command("new"),
        "Ctrl+O" => Command("open"),
        "Ctrl+S" => Command("save"),
        "Ctrl+Shift+O" => Command("import"),
        "Ctrl+Shift+S" => Command("save_as"),
        "Ctrl+Alt+Shift+S" => Command("export"),
        "Ctrl+W" => Command("close"),
        "Ctrl+Q" => Command("quit"),
        "Ctrl+Z" => Command("undo"),
        "Ctrl+Shift+Z" | "Ctrl+Y" => Command("redo"),
        "Ctrl+J" => Command("duplicate"),
        "Ctrl+E" => Command("merge"),
        "Ctrl+G" => Command("group"),
        "Ctrl+Shift+G" => Command("ungroup"),
        "Ctrl+Alt+G" => Command("clip"),
        "Ctrl+Shift+N" => Command("new_layer"),
        "Ctrl+A" => Command("select_all"),
        "Ctrl+D" => Command("deselect"),
        "Ctrl+Shift+I" => Command("invert_selection"),
        "Ctrl+X" => Command("cut"),
        "Ctrl+C" => Command("copy"),
        "Ctrl+V" => Command("paste"),
        "Ctrl+Shift+C" => Command("copy_merged"),
        "Alt+Backspace" => Command("fill_fg"),
        "Ctrl+Backspace" => Command("fill_bg"),
        "Shift+F5" => Command("content_fill"),
        "Ctrl+L" => Command("levels"),
        "Ctrl+U" => Command("hue"),
        "Ctrl+M" => Command("curves"),
        "Ctrl+I" => Command("invert"),
        "Ctrl+0" => Command("fit"),
        "Ctrl+1" => Command("actual"),
        "Ctrl+Plus" => Command("zoom_in"),
        "Ctrl+Minus" => Command("zoom_out"),
        "F1" => Command("shortcuts"),

        "V" => Effect::Tool(Tool::Move),
        "M" => Effect::Tool(Tool::Marquee),
        "L" => Effect::Tool(Tool::Lasso),
        "W" => Effect::Tool(Tool::Wand),
        "C" => Effect::Tool(Tool::Crop),
        "B" => Effect::Tool(Tool::Brush),
        "E" => Effect::Tool(Tool::Erase),
        "J" => Effect::Tool(Tool::Heal),
        "S" => Effect::Tool(Tool::Clone),
        "R" => Effect::Tool(Tool::Blur),
        "G" => Effect::Tool(Tool::Gradient),
        "U" => Effect::Tool(Tool::Shape),
        "I" => Effect::Tool(Tool::Dropper),
        "H" => Effect::Tool(Tool::Hand),
        "Z" => Effect::Tool(Tool::Zoom),
        "T" => Effect::Tool(Tool::Text),
        "Shift+B" => State(|app| app.tool = Tool::Brush, |app| app.tool == Tool::Pencil),
        "Ctrl+T" => State(
            |app| app.tool = Tool::Zoom,
            |app| app.tool == Tool::Move && app.show_controls,
        ),
        "Ctrl+H" => State(|app| app.show_controls = true, |app| !app.show_controls),

        "[" => State(
            |app| app.brush.diameter = 40.0,
            |app| app.brush.diameter < 40.0,
        ),
        "]" => State(
            |app| app.brush.diameter = 40.0,
            |app| app.brush.diameter > 40.0,
        ),
        "Shift+[" => State(
            |app| app.brush.hardness = 0.5,
            |app| app.brush.hardness < 0.5,
        ),
        "Shift+]" => State(
            |app| app.brush.hardness = 0.5,
            |app| app.brush.hardness > 0.5,
        ),
        "X" => State(
            |app| {
                app.brush.color = [1, 2, 3, 255];
                app.background = [4, 5, 6, 255];
            },
            |app| app.brush.color == [4, 5, 6, 255] && app.background == [1, 2, 3, 255],
        ),
        "D" => State(
            |app| {
                app.brush.color = [1, 2, 3, 255];
                app.background = [4, 5, 6, 255];
            },
            |app| app.brush.color == [0, 0, 0, 255] && app.background == [255; 4],
        ),
        // Applies a crop, or abandons one, when a crop rectangle is waiting.
        "Enter" => State(
            |app| app.crop_rect = Some((Point::new(2.0, 2.0), Point::new(10.0, 10.0))),
            |app| app.crop_rect.is_none() && app.session().unwrap().document.width == 8,
        ),
        "Escape" => State(
            |app| app.crop_rect = Some((Point::new(2.0, 2.0), Point::new(10.0, 10.0))),
            |app| app.crop_rect.is_none(),
        ),
        _ => return None,
    })
}

/// Chords that belong to the text dialog and cannot be pressed without opening it.
const DIALOG_ONLY: &[&str] = &["Ctrl+Enter"];

fn key_named(name: &str) -> Option<Key> {
    Some(match name {
        "Backspace" => Key::Backspace,
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Plus" => Key::Plus,
        "Minus" => Key::Minus,
        "[" => Key::OpenBracket,
        "]" => Key::CloseBracket,
        "F1" => Key::F1,
        "F5" => Key::F5,
        "0" => Key::Num0,
        "1" => Key::Num1,
        single if single.len() == 1 => Key::from_name(single)?,
        _ => return None,
    })
}

fn parse_chord(token: &str) -> Option<(Modifiers, Key)> {
    let mut modifiers = Modifiers::NONE;
    let mut parts = token.split('+').peekable();
    // "Ctrl+Plus": the last part is the key, even when it is spelled "Plus".
    let mut key = None;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            key = key_named(part);
            break;
        }
        match part {
            "Ctrl" => modifiers |= Modifiers::CTRL,
            "Shift" => modifiers |= Modifiers::SHIFT,
            "Alt" => modifiers |= Modifiers::ALT,
            _ => return None,
        }
    }
    Some((modifiers, key?))
}

/// The shortcut column of the first table in the document, split into individual tokens.
fn documented_tokens() -> Vec<String> {
    let mut tokens = Vec::new();
    for row in DOC.lines().filter(|line| line.starts_with('|')).skip(2) {
        let cell = row.split('|').nth(2).expect("a two-column table").trim();
        // Notes in parentheses are prose, except "(or Ctrl+Y)" which names another chord.
        let mut flat = String::new();
        let mut rest = cell;
        while let Some(open) = rest.find('(') {
            flat.push_str(&rest[..open]);
            let close = rest[open..].find(')').map_or(rest.len(), |c| open + c + 1);
            let inner = &rest[open + 1..close - 1];
            if let Some(alternative) = inner.strip_prefix("or ") {
                flat.push_str(" / ");
                flat.push_str(alternative);
            }
            rest = &rest[close..];
        }
        flat.push_str(rest);
        for token in flat
            .replace(", or ", " / ")
            .replace(" or ", " / ")
            .replace(" and ", " / ")
            .replace(", ", " / ")
            .split(" / ")
        {
            let token = token.trim();
            if !token.is_empty() {
                tokens.push(token.to_owned());
            }
        }
    }
    tokens
}

#[test]
fn documented_shortcuts_trigger_their_commands() {
    let tokens = documented_tokens();
    assert!(tokens.len() > 60, "the table did not parse: {tokens:?}");
    let (chords, prose): (Vec<_>, Vec<_>) = tokens
        .iter()
        .map(String::as_str)
        .partition(|token| parse_chord(token).is_some());

    let mut prose = prose;
    prose.sort_unstable();
    prose.dedup();
    assert_eq!(
        prose, PROSE,
        "docs/SHORTCUTS.md gained or lost a non-chord entry; update PROSE and its test"
    );

    let mut ui = UiTest::with_document();
    ui.app_mut().command_trace = Some(Vec::new());
    let mut pressed = Vec::new();
    for chord in chords {
        if pressed.contains(&chord) {
            continue;
        }
        pressed.push(chord);
        if DIALOG_ONLY.contains(&chord) {
            continue;
        }
        let effect = expected(chord).unwrap_or_else(|| {
            panic!("{chord} is documented in docs/SHORTCUTS.md but has no expected effect")
        });
        let (modifiers, key) = parse_chord(chord).unwrap();
        ui.app_mut().command_trace = Some(Vec::new());
        // Start every tool test from a different tool, so a stuck tool cannot pass.
        let start = if matches!(effect, Effect::Tool(Tool::Zoom)) {
            Tool::Hand
        } else {
            Tool::Zoom
        };
        ui.app_mut().tool = start;
        if let State(setup, _) = &effect {
            setup(ui.app_mut());
        }
        ui.press(modifiers, key);
        let trace = ui.app().command_trace.clone().unwrap();
        match effect {
            Command(command) => assert_eq!(trace, [command], "{chord}"),
            Effect::Tool(tool) => {
                assert!(ui.app().tool == tool, "{chord} should select {tool:?}");
                assert!(trace.is_empty(), "{chord} also ran {trace:?}");
            }
            State(_, check) => assert!(check(ui.app()), "{chord} did not have its effect"),
        }
    }

    // Everything the test knows how to press is still documented.
    for chord in ["Ctrl+Y", "Shift+B", "Shift+[", "X", "Enter"] {
        assert!(pressed.contains(&chord), "{chord} left docs/SHORTCUTS.md");
    }
}

#[test]
fn number_keys_set_opacity_and_arrows_nudge() {
    let mut ui = UiTest::with_document();
    ui.app_mut().tool = Tool::Brush;
    for (digit, key) in [(1, Key::Num1), (5, Key::Num5), (10, Key::Num0)] {
        ui.key(key);
        assert!((ui.app().brush.opacity - digit as f32 / 10.0).abs() < 1e-6);
    }

    ui.app_mut().tool = Tool::Move;
    let x = |ui: &UiTest| {
        ui.app()
            .session()
            .unwrap()
            .document
            .active()
            .unwrap()
            .transform
            .x
    };
    let before = x(&ui);
    ui.key(Key::ArrowRight);
    assert_eq!(x(&ui), before + 1.0);
    ui.press(Modifiers::SHIFT, Key::ArrowRight);
    assert_eq!(x(&ui), before + 11.0);
}

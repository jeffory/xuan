//! Every shortcut documented in `docs/SHORTCUTS.md` must do what the document says.
//!
//! The command table in the document is generated from the command registry, and the test fails
//! when the two differ (`XUAN_UPDATE_DOCS=1` rewrites it). Every default binding of the registry
//! is then pressed as a real key event and its effect checked: a command is dispatched, a tool
//! becomes active, or, for the few key-only actions, a check in [`app_effect`] passes. The second,
//! hand-written table (keys the editor keeps for itself and pointer gestures) is parsed and
//! checked as before, so neither table can drift silently.

use super::*;
use commands::{Category, Keymap, Kind, Run, Scope};
use egui::{Key, Modifiers};

const DOC_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/SHORTCUTS.md");
const DOC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/SHORTCUTS.md"));

/// The document with LF line endings: a Windows checkout may have CRLF.
fn doc() -> String {
    DOC.replace("\r\n", "\n")
}
const BEGIN: &str = "<!-- BEGIN GENERATED from the command registry (src/app/commands.rs); refresh with XUAN_UPDATE_DOCS=1 cargo test documented_shortcuts -->\n";
const END: &str = "<!-- END GENERATED -->";
const OTHER_HEADING: &str = "## Other keys and pointer controls";

/// The command table: every built-in command with a default binding, by category.
fn generated_table() -> String {
    let keymap = Keymap::default();
    let mut table = String::from("| Category | Command | Shortcut |\n| --- | --- | --- |\n");
    for category in Category::ALL {
        for entry in keymap.entries() {
            let Kind::Builtin(command) = &entry.kind else {
                continue;
            };
            if command.category != category || command.keys.is_empty() {
                continue;
            }
            let keys: Vec<String> = command.keys.iter().map(|k| k.to_string()).collect();
            table.push_str(&format!(
                "| {} | {} | {} |\n",
                category.name(),
                command.label,
                keys.join(" / ")
            ));
        }
    }
    table
}

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

/// Checks for the registry entries that run something other than a command or a tool.
fn app_effect(id: &str) -> Option<Effect> {
    Some(match id {
        "command_palette" => State(|_| {}, |app| app.palette.is_some()),
        "show_transform" => State(
            |app| app.tool = Tool::Zoom,
            |app| app.tool == Tool::Move && app.show_controls,
        ),
        "toggle_controls" => State(|app| app.show_controls = true, |app| !app.show_controls),
        "tool_brush" => State(
            |app| app.brush_variant = Tool::Pencil,
            |app| app.tool == Tool::Pencil,
        ),
        "switch_brush" => State(|app| app.tool = Tool::Brush, |app| app.tool == Tool::Pencil),
        "tool_gradient" => State(
            |app| app.gradient_variant = Tool::Bucket,
            |app| app.tool == Tool::Bucket,
        ),
        "switch_gradient" => State(
            |app| app.tool = Tool::Gradient,
            |app| app.tool == Tool::Bucket,
        ),
        "switch_tone" => State(
            |app| {
                app.tool = Tool::Dodge;
                app.tone_mode = PaintMode::Burn;
            },
            |app| app.tool == Tool::Dodge && app.tone_mode == PaintMode::Sponge,
        ),
        "crop_mode" => State(
            |app| {
                app.tool = Tool::Crop;
                app.crop.perspective = false;
            },
            |app| app.tool == Tool::Crop && app.crop.perspective && app.crop.quad.is_some(),
        ),
        "marquee_shape" => State(
            |app| app.ellipse = false,
            |app| app.tool == Tool::Marquee && app.ellipse,
        ),
        "lasso_mode" => State(
            |app| app.polygonal = false,
            |app| app.tool == Tool::Lasso && app.polygonal,
        ),
        "shape_kind" => State(
            |app| app.shape_kind = xuan::paint::ShapeKind::Rectangle,
            |app| app.tool == Tool::Shape && app.shape_kind == xuan::paint::ShapeKind::Ellipse,
        ),
        "brush_smaller" => State(
            |app| app.brush.diameter = 40.0,
            |app| app.brush.diameter < 40.0,
        ),
        "brush_larger" => State(
            |app| app.brush.diameter = 40.0,
            |app| app.brush.diameter > 40.0,
        ),
        "brush_softer" => State(
            |app| app.brush.hardness = 0.5,
            |app| app.brush.hardness < 0.5,
        ),
        "brush_harder" => State(
            |app| app.brush.hardness = 0.5,
            |app| app.brush.hardness > 0.5,
        ),
        "swap_colors" => State(
            |app| {
                app.brush.color = [1, 2, 3, 255];
                app.background = [4, 5, 6, 255];
            },
            |app| app.brush.color == [4, 5, 6, 255] && app.background == [1, 2, 3, 255],
        ),
        "reset_colors" => State(
            |app| {
                app.brush.color = [1, 2, 3, 255];
                app.background = [4, 5, 6, 255];
            },
            |app| app.brush.color == [0, 0, 0, 255] && app.background == [255; 4],
        ),
        _ => return None,
    })
}

/// Shortcut cell text in the second table that is not a key chord. Pointer gestures and key
/// groups are listed here and checked elsewhere, so a new prose entry also has to be acknowledged.
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

/// Chords of the second table, which the editor handles itself.
fn other_effect(chord: &str) -> Option<Effect> {
    Some(match chord {
        // Applies a crop, or abandons one, when a crop rectangle is waiting.
        "Enter" => State(
            |app| app.crop.rect = Some(xuan::crop::CropBox::new(2, 2, 8, 8)),
            |app| app.crop.rect.is_none() && app.session().unwrap().document.width == 8,
        ),
        "Escape" => State(
            |app| app.crop.rect = Some(xuan::crop::CropBox::new(2, 2, 8, 8)),
            |app| app.crop.rect.is_none(),
        ),
        _ => return None,
    })
}

/// Chords that belong to the text dialog and cannot be pressed without opening it.
const DIALOG_ONLY: &[&str] = &["Ctrl+Enter"];

/// The shortcut column of the hand-written table, split into individual tokens.
fn other_tokens() -> Vec<String> {
    let doc = doc();
    let table = doc
        .split_once(OTHER_HEADING)
        .expect("docs/SHORTCUTS.md lost its second table")
        .1;
    let mut tokens = Vec::new();
    for row in table
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .skip(2)
    {
        let cell = row.split('|').nth(2).expect("a two-column table").trim();
        // Notes in parentheses are prose.
        let mut flat = String::new();
        let mut rest = cell;
        while let Some(open) = rest.find('(') {
            flat.push_str(&rest[..open]);
            let close = rest[open..].find(')').map_or(rest.len(), |c| open + c + 1);
            rest = &rest[close..];
        }
        flat.push_str(rest);
        for token in flat
            .replace(", or ", " / ")
            .replace(" or ", " / ")
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

/// Presses `chord` with `effect`'s setup and checks the effect.
fn press_and_check(ui: &mut UiTest, name: &str, chord: commands::Chord, effect: &Effect) {
    ui.app_mut().command_trace = Some(Vec::new());
    // Start every tool test from a different tool, so a stuck tool cannot pass.
    ui.app_mut().tool = if matches!(effect, Effect::Tool(Tool::Zoom)) {
        Tool::Hand
    } else {
        Tool::Zoom
    };
    if let State(setup, _) = effect {
        setup(ui.app_mut());
    }
    ui.press(chord.mods, chord.key);
    let trace = ui.app().command_trace.clone().unwrap();
    match effect {
        Command(command) => assert_eq!(trace, [*command], "{name} ({chord})"),
        Effect::Tool(tool) => {
            assert!(ui.app().tool == *tool, "{chord} should select {tool:?}");
            assert!(trace.is_empty(), "{chord} also ran {trace:?}");
        }
        State(_, check) => assert!(check(ui.app()), "{name} ({chord}) did not have its effect"),
    }
}

#[test]
fn documented_shortcuts_trigger_their_commands() {
    // The command table is the registry's.
    let generated = generated_table();
    let doc = doc();
    let documented = doc
        .split_once(BEGIN)
        .and_then(|(_, rest)| rest.split_once(END))
        .map(|(table, _)| table)
        .expect("docs/SHORTCUTS.md lost its generated block");
    if documented != generated {
        if std::env::var_os("XUAN_UPDATE_DOCS").is_some() {
            // Written with LF line endings, like the generated table.
            let text = doc.replacen(
                &format!("{BEGIN}{documented}{END}"),
                &format!("{BEGIN}{generated}{END}"),
                1,
            );
            std::fs::write(DOC_PATH, text).unwrap();
        } else {
            panic!(
                "docs/SHORTCUTS.md does not match the command registry; run \
                 XUAN_UPDATE_DOCS=1 cargo test documented_shortcuts to refresh it.\n\
                 Expected:\n{generated}"
            );
        }
    }

    // Every default binding in the editor does what the registry says.
    let mut ui = UiTest::with_document();
    // Something to undo and redo, so that every command with a key is enabled.
    for command in ["new_layer", "new_layer", "undo"] {
        ui.app_mut().command(command);
    }
    let keymap = Keymap::default();
    let mut pressed = 0;
    for entry in keymap.entries() {
        let Kind::Builtin(command) = &entry.kind else {
            continue;
        };
        if command.scope == Scope::Develop {
            continue;
        }
        for &chord in command.keys {
            let effect = match command.run {
                Run::Command => Command(command.id),
                Run::Tool(tool) => Effect::Tool(tool),
                Run::App(_) => app_effect(command.id)
                    .unwrap_or_else(|| panic!("{} has a key but no expected effect", command.id)),
            };
            // Clear Pixels' key deletes layers when nothing is selected.
            if command.id == "clear" {
                ui.app_mut().command_trace = None;
                ui.app_mut().command("select_all");
            }
            // An open palette would take the next chord.
            ui.app_mut().palette = None;
            press_and_check(&mut ui, command.id, chord, &effect);
            pressed += 1;
        }
    }
    assert!(pressed > 60, "only {pressed} bindings were pressed");

    // The keys and gestures the editor keeps for itself.
    let tokens = other_tokens();
    let (chords, prose): (Vec<_>, Vec<_>) = tokens
        .iter()
        .map(String::as_str)
        .partition(|token| commands::Chord::parse(token).is_some());
    let mut prose = prose;
    prose.sort_unstable();
    prose.dedup();
    assert_eq!(
        prose, PROSE,
        "docs/SHORTCUTS.md gained or lost a non-chord entry; update PROSE and its test"
    );
    let mut seen = Vec::new();
    for token in chords {
        if seen.contains(&token) {
            continue;
        }
        seen.push(token);
        let chord = commands::Chord::parse(token).unwrap();
        assert!(
            commands::reserved(chord).is_some(),
            "{token} is in the second table but commands can use it"
        );
        if DIALOG_ONLY.contains(&token) {
            continue;
        }
        let effect = other_effect(token).unwrap_or_else(|| {
            panic!("{token} is documented in docs/SHORTCUTS.md but has no expected effect")
        });
        press_and_check(&mut ui, token, chord, &effect);
    }
    for token in ["Enter", "Escape", "Ctrl+Enter"] {
        assert!(seen.contains(&token), "{token} left docs/SHORTCUTS.md");
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

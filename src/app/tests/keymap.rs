//! The command registry and key bindings: completeness, exact-modifier dispatch, overrides,
//! conflicts, persistence and plugin shortcuts.
use super::*;
use commands::{COMMANDS, Category, Chord, HostRun, Keymap, Refusal, Scope};
use egui::{Key, Modifiers};
use xuan::plugins::Manifest;

const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap_or_else(|| panic!("{text} is not a chord"))
}

fn overrides(pairs: &[(&str, &str)]) -> toml::Table {
    pairs
        .iter()
        .map(|(id, keys)| (id.to_string(), toml::Value::String(keys.to_string())))
        .collect()
}

/// Command ids in the menu source: `item(ui, &items, "id", …)`, `labelled(…, "id", …)`,
/// `check_item(…, "id", …)` and the tab bar's `action = Some("id")`.
pub(super) fn menu_commands() -> Vec<String> {
    let source = include_str!("../menus.rs");
    let mut ids = Vec::new();
    for start in ["item(", "labelled(", "check_item(", "action = Some("] {
        for (index, _) in source.match_indices(start) {
            let call = &source[index + start.len()..];
            let call = &call[..call.find(";").unwrap_or(call.len())];
            for literal in call.split('"').skip(1).step_by(2) {
                if !literal.is_empty()
                    && literal.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                {
                    ids.push(literal.to_owned());
                }
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

#[test]
fn every_menu_command_is_in_the_registry_and_every_label_is_translated() {
    let ids = menu_commands();
    assert!(ids.len() > 60, "the menu source did not parse: {ids:?}");
    for id in &ids {
        assert!(
            commands::find(id).is_some(),
            "menu command {id} is not registered"
        );
    }
    let mut seen = std::collections::HashSet::new();
    for command in COMMANDS {
        assert!(seen.insert(command.id), "duplicate command {}", command.id);
        assert!(
            !command.id.contains('/'),
            "{} looks like a plugin action",
            command.id
        );
    }
    // Every tool on the rail has a command.
    for tool in Tool::ALL {
        if tool != Tool::Region {
            let id = commands::tool_command(tool).unwrap();
            assert!(commands::find(id).is_some(), "{tool:?}: {id}");
        }
    }

    let catalog: std::collections::HashSet<&str> =
        include_str!("../../../assets/locales/zh-CN.tsv")
            .lines()
            .filter_map(|line| line.split_once('\t').map(|(key, _)| key))
            .collect();
    let labels = COMMANDS
        .iter()
        .map(|c| c.label)
        .chain(Category::ALL.iter().map(|c| c.name()));
    let missing: Vec<&str> = labels.filter(|label| !catalog.contains(label)).collect();
    assert!(missing.is_empty(), "untranslated: {missing:?}");
}

#[test]
fn default_bindings_are_unique_reserved_keys_free_and_letters_stay_with_tools() {
    let keymap = Keymap::default();
    for entry in keymap.entries() {
        for &key in &entry.keys {
            assert!(commands::reserved(key).is_none(), "{} uses {key}", entry.id);
            assert!(
                !key.is_plain_character() || entry.plain_keys_allowed(),
                "{} takes the plain key {key}",
                entry.id
            );
            // The defaults pass the same check the Settings page applies.
            assert_eq!(keymap.check(&entry.id, key), Ok(()), "{} {key}", entry.id);
        }
    }
}

#[test]
fn host_run_flags_keep_the_plugin_allow_lists() {
    // The sets `host/run` allowed before the registry; they must not grow by accident.
    let view = ["fit", "actual", "zoom_in", "zoom_out"];
    let edit = [
        "undo",
        "redo",
        "new_layer",
        "duplicate",
        "delete_layer",
        "group",
        "ungroup",
        "move_out",
        "merge",
        "flatten",
        "mask",
        "new_mask_layer",
        "delete_mask",
        "disable_mask",
        "link_mask",
        "clip",
        "select_all",
        "deselect",
        "invert_selection",
        "fill_fg",
        "fill_bg",
        "clear",
        "invert",
        "flip_h",
        "flip_v",
        "flip_canvas_h",
        "flip_canvas_v",
        "content_fill",
        "select_layer_pixels",
        "select_mask_black",
        "select_subject",
        "feather",
        "remove_background",
        "remove_flat_background",
    ];
    let with = |host: HostRun| {
        let mut ids: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| c.host == host)
            .map(|c| c.id)
            .collect();
        ids.sort_unstable();
        ids
    };
    let mut expected_view = view.to_vec();
    expected_view.sort_unstable();
    let mut expected_edit = edit.to_vec();
    expected_edit.sort_unstable();
    assert_eq!(with(HostRun::View), expected_view);
    assert_eq!(with(HostRun::Edit), expected_edit);
    assert_eq!(commands::host_run("save"), HostRun::Never);
    assert_eq!(commands::host_run("no_such_command"), HostRun::Never);
}

#[test]
fn chords_parse_print_and_label() {
    for (text, label) in [
        ("Ctrl+Alt+Shift+S", "Ctrl+Alt+Shift+S"),
        ("Ctrl+Plus", "Ctrl++"),
        ("Ctrl+Minus", "Ctrl+−"),
        ("Ctrl+;", "Ctrl+;"),
        ("Ctrl+'", "Ctrl+'"),
        ("Ctrl+,", "Ctrl+,"),
        ("Alt+Backspace", "Alt+Backspace"),
        ("Shift+[", "Shift+["),
        ("F1", "F1"),
        ("Delete", "Delete"),
    ] {
        let parsed = chord(text);
        assert_eq!(parsed.to_string(), text);
        assert_eq!(parsed.label(), label);
        assert_eq!(Chord::parse(&parsed.label()), Some(parsed), "{label}");
    }
    assert_eq!(chord("ctrl+shift+e"), chord("Ctrl+Shift+E"));
    assert_eq!(chord("Shift+Ctrl+E"), chord("Ctrl+Shift+E"));
    assert_eq!(chord("Cmd+Option+K"), chord("Ctrl+Alt+K"));
    for bad in ["", "Ctrl+", "Hyper+E", "Ctrl+NoSuchKey"] {
        assert_eq!(Chord::parse(bad), None, "{bad}");
    }
}

#[test]
fn dispatch_matches_modifiers_exactly() {
    let keymap = Keymap::default();
    let id = |mods, key| keymap.lookup(mods, key, false).map(|e| e.id.as_str());
    assert_eq!(id(Modifiers::CTRL, Key::E), Some("merge"));
    assert_eq!(id(CTRL_SHIFT, Key::E), None);
    assert_eq!(id(Modifiers::CTRL | Modifiers::ALT, Key::E), None);
    assert_eq!(id(Modifiers::CTRL, Key::I), Some("invert"));
    assert_eq!(id(CTRL_SHIFT, Key::I), Some("invert_selection"));
    assert_eq!(id(Modifiers::NONE, Key::B), Some("tool_brush"));
    assert_eq!(id(Modifiers::SHIFT, Key::B), Some("switch_brush"));
    // `+` and `'` need Shift on many layouts.
    assert_eq!(id(CTRL_SHIFT, Key::Plus), Some("zoom_in"));
    assert_eq!(id(CTRL_SHIFT, Key::Quote), Some("toggle_grid"));
    // Tools do not apply in Develop; commands for both do.
    assert_eq!(
        keymap.lookup(Modifiers::NONE, Key::B, true).map(|e| &e.id),
        None
    );
    assert_eq!(
        keymap
            .lookup(Modifiers::CTRL, Key::Z, true)
            .map(|e| e.id.as_str()),
        Some("undo")
    );

    // Pressed in the editor, Ctrl+Shift+E reaches nothing and Ctrl+E merges.
    let (context, mut app) = app_with_document();
    app.command_trace = Some(Vec::new());
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(Key::E, CTRL_SHIFT)],
        CTRL_SHIFT,
    );
    assert_eq!(app.command_trace.as_deref(), Some(&[][..]));
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(Key::E, Modifiers::CTRL)],
        Modifiers::CTRL,
    );
    assert_eq!(
        app.command_trace.as_deref(),
        Some(&["merge".to_owned()][..])
    );
}

#[test]
fn an_override_changes_dispatch_menu_label_and_resets() {
    let (context, mut app) = app_with_document();
    app.config.keybindings = overrides(&[("merge", "Ctrl+Shift+M"), ("invert_selection", "")]);
    app.rebuild_keymap();
    assert_eq!(app.keymap.shortcut("merge"), "Ctrl+Shift+M");
    assert!(app.keymap.keys("invert_selection").is_empty());
    let items = app.menu_items();
    assert_eq!(items.get("merge"), (true, "Ctrl+Shift+M"));
    assert_eq!(items.get("invert_selection").1, "");

    app.command_trace = Some(Vec::new());
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(Key::M, CTRL_SHIFT)],
        CTRL_SHIFT,
    );
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(Key::E, Modifiers::CTRL)],
        Modifiers::CTRL,
    );
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(Key::I, CTRL_SHIFT)],
        CTRL_SHIFT,
    );
    assert_eq!(
        app.command_trace.as_deref(),
        Some(&["merge".to_owned()][..])
    );

    // Resetting one command restores its defaults; the others keep their overrides.
    app.config.keybindings.remove("merge");
    app.rebuild_keymap();
    assert_eq!(app.keymap.shortcut("merge"), "Ctrl+E");
    assert!(!app.keymap.get("merge").unwrap().customised());
    assert!(app.keymap.get("invert_selection").unwrap().customised());
    // Setting the defaults back stores nothing.
    let keymap = Keymap::default();
    let mut table = app.config.keybindings.clone();
    commands::set_override(
        &mut table,
        &keymap,
        "invert_selection",
        &[chord("Ctrl+Shift+I")],
    );
    assert!(table.is_empty());
    commands::set_override(&mut table, &keymap, "redo", &[chord("Ctrl+Y"), chord("F2")]);
    assert_eq!(
        table["redo"],
        toml::Value::Array(vec!["Ctrl+Y".into(), "F2".into()])
    );
}

#[test]
fn conflicts_are_detected_within_overlapping_scopes() {
    let keymap = Keymap::default();
    assert_eq!(
        keymap.check("merge", chord("Ctrl+S")),
        Err(Refusal::Conflict("save".into()))
    );
    // Undo applies in Develop too, so a Develop command cannot take its key.
    assert_eq!(
        keymap.check("develop_split", chord("Ctrl+Z")),
        Err(Refusal::Conflict("undo".into()))
    );
    // Develop commands may share keys with editor tools; plain letters are fine there.
    assert_eq!(keymap.check("develop_split", chord("V")), Ok(()));
    assert_eq!(keymap.get("develop_split").unwrap().scope(), Scope::Develop);
    // Plain letters are for tools.
    assert_eq!(
        keymap.check("merge", chord("K")),
        Err(Refusal::NeedsModifier)
    );
    assert_eq!(
        keymap.check("merge", chord("Shift+K")),
        Err(Refusal::NeedsModifier)
    );
    assert_eq!(keymap.check("merge", chord("7")), Err(Refusal::Reserved));
    assert_eq!(keymap.check("tool_move", chord("K")), Ok(()));
    assert_eq!(
        keymap.check("tool_move", chord("B")),
        Err(Refusal::Conflict("tool_brush".into()))
    );
    // Keys the editor keeps.
    assert_eq!(
        keymap.check("merge", chord("Ctrl+Enter")),
        Err(Refusal::Reserved)
    );
    assert_eq!(
        keymap.check("merge", chord("Escape")),
        Err(Refusal::Reserved)
    );
    // `Ctrl+Shift++` is the same press as `Ctrl++`.
    assert_eq!(
        keymap.check("merge", chord("Ctrl+Shift+Plus")),
        Err(Refusal::Conflict("zoom_in".into()))
    );
    // A command's own key is no conflict, and free chords pass.
    assert_eq!(keymap.check("merge", chord("Ctrl+E")), Ok(()));
    assert_eq!(keymap.check("merge", chord("Ctrl+Alt+M")), Ok(()));
}

#[test]
fn overrides_persist_and_bad_entries_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("xuan/config.toml");
    let (_, mut app) = app();
    app.config_path = Some(path.clone());
    let keymap = Keymap::default();
    let mut table = toml::Table::new();
    commands::set_override(&mut table, &keymap, "merge", &[chord("Ctrl+Alt+M")]);
    commands::set_override(&mut table, &keymap, "deselect", &[]);
    app.config.keybindings = table;
    app.save_config();
    assert!(app.error.is_none(), "{:?}", app.error);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[keybindings]"), "{text}");
    assert!(text.contains("merge = \"Ctrl+Alt+M\"") && text.contains("deselect = \"\""));
    // Only the changes are stored.
    assert!(!text.contains("Ctrl+N"));

    let loaded = xuan::config::Config::load(&path).unwrap();
    let (keymap, errors) = Keymap::build(&loaded.keybindings, &[]);
    assert!(errors.is_empty());
    assert_eq!(keymap.shortcut("merge"), "Ctrl+Alt+M");
    assert!(keymap.keys("deselect").is_empty());
    assert_eq!(keymap.shortcut("save"), "Ctrl+S");

    // Unknown commands and values that are not shortcuts keep the defaults, with a note.
    let table = overrides(&[
        ("no_such_command", "Ctrl+K"),
        ("merge", "Hyper+E"),
        ("someplugin/action", "Ctrl+Alt+P"),
    ]);
    let notes = commands::override_problems(&table);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].contains("Hyper+E") || notes[1].contains("Hyper+E"));
    assert!(notes.iter().any(|note| note.contains("no_such_command")));
    let (keymap, _) = Keymap::build(&table, &[]);
    assert_eq!(keymap.shortcut("merge"), "Ctrl+E");
    assert!(keymap.get("no_such_command").is_none());
}

const PLUGIN: &str = r#"
[plugin]
id = "mock"
name = "Mock"
version = "0.1.0"
command = ["sh", "plugin.sh"]

[[actions]]
id = "echo"
label = "Echo Source…"
menu = "Filter"
shortcut = "Ctrl+Shift+E"
"#;

#[test]
fn plugin_shortcuts_yield_to_user_bindings_and_can_be_rebound() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = Manifest::parse(PLUGIN, dir.path()).unwrap();
    let manifests = std::slice::from_ref(&manifest);

    let (keymap, errors) = Keymap::build(&toml::Table::new(), manifests);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(keymap.shortcut("mock/echo"), "Ctrl+Shift+E");
    let entry = keymap.lookup(CTRL_SHIFT, Key::E, false).unwrap();
    assert_eq!(entry.id, "mock/echo");
    assert_eq!(entry.category(), Category::Plugins);
    assert!(keymap.lookup(CTRL_SHIFT, Key::E, true).is_none());

    // The user gave Merge the plugin's chord: the plugin's shortcut is left out and reported.
    let (keymap, errors) = Keymap::build(&overrides(&[("merge", "Ctrl+Shift+E")]), manifests);
    assert!(keymap.keys("mock/echo").is_empty());
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].error.contains("Ctrl+Shift+E") && errors[0].error.contains("merge"),
        "{}",
        errors[0].error
    );
    assert_eq!(
        keymap
            .lookup(CTRL_SHIFT, Key::E, false)
            .map(|e| e.id.as_str()),
        Some("merge")
    );

    // A built-in chord the user freed becomes available to a plugin.
    let taken = PLUGIN.replace("Ctrl+Shift+E", "Ctrl+Shift+I");
    let manifest = Manifest::parse(&taken, dir.path()).unwrap();
    let (keymap, errors) = Keymap::build(&toml::Table::new(), std::slice::from_ref(&manifest));
    assert!(keymap.keys("mock/echo").is_empty() && errors.len() == 1);
    let (keymap, errors) = Keymap::build(
        &overrides(&[("invert_selection", "Ctrl+Alt+I")]),
        std::slice::from_ref(&manifest),
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(keymap.shortcut("mock/echo"), "Ctrl+Shift+I");

    // Plugin actions are rebound like any command; an override silences the collision.
    let (keymap, errors) = Keymap::build(
        &overrides(&[("mock/echo", "Ctrl+Alt+Y")]),
        std::slice::from_ref(&manifest),
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(keymap.shortcut("mock/echo"), "Ctrl+Alt+Y");
    assert_eq!(
        keymap.check("mock/echo", chord("K")),
        Err(Refusal::NeedsModifier)
    );

    // In the editor, the plugin's errors show the collision until the user moves Merge back.
    let (_, mut app) = app();
    app.config.keybindings = overrides(&[("merge", "Ctrl+Shift+E")]);
    let manifest = Manifest::parse(PLUGIN, dir.path()).unwrap();
    app.install_plugins(vec![manifest], vec![]);
    assert_eq!(app.plugins.errors.len(), 1);
    app.config.keybindings.clear();
    app.rebuild_keymap();
    assert!(app.plugins.errors.is_empty());
    assert_eq!(app.keymap.shortcut("mock/echo"), "Ctrl+Shift+E");
}

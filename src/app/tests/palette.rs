//! The command palette's matcher, ranking and state machine. The overlay itself is driven like a
//! user in `ui_palette.rs`.
use super::*;
use crate::app::palette::{
    Group, MAX_RECENT, Move, Palette, Row, fuzzy_match, match_command, push_recent, rank,
};
use commands::Keymap;
use xuan::{config::Language, i18n};

fn score(query: &str, text: &str) -> i32 {
    fuzzy_match(query, text)
        .unwrap_or_else(|| panic!("{query:?} should match {text:?}"))
        .score
}

fn ids(keymap: &Keymap, rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| keymap.entries()[row.entry].id.clone())
        .collect()
}

fn find(keymap: &Keymap, developing: bool, query: &str, recent: &[&str]) -> Vec<String> {
    let recent: Vec<String> = recent.iter().map(|s| s.to_string()).collect();
    ids(keymap, &rank(keymap.entries(), developing, query, &recent))
}

#[test]
fn matcher_needs_the_characters_in_order_and_ignores_case() {
    assert!(fuzzy_match("xyz", "Gaussian Blur").is_none());
    assert!(fuzzy_match("rub", "Gaussian Blur").is_none(), "wrong order");
    assert!(fuzzy_match("blurry", "Blur").is_none(), "longer than text");
    assert!(fuzzy_match("GAUSS", "gaussian blur").is_some());
    let empty = fuzzy_match("", "anything").unwrap();
    assert_eq!((empty.score, empty.indices), (0, vec![]));
}

#[test]
fn matcher_reports_the_characters_it_used() {
    assert_eq!(fuzzy_match("gb", "Gaussian Blur").unwrap().indices, [0, 9]);
    assert_eq!(
        fuzzy_match("blur", "Gaussian Blur").unwrap().indices,
        [9, 10, 11, 12]
    );
    // The better alignment wins over the first one found: the word start, not the "b" in "Cab".
    assert_eq!(fuzzy_match("b", "Cab Blur").unwrap().indices, [4]);
}

#[test]
fn matcher_prefers_prefixes_word_starts_and_runs() {
    // A prefix beats the same letters later in the text.
    assert!(score("blur", "Blur") > score("blur", "Gaussian Blur"));
    // Consecutive characters beat scattered ones.
    assert!(score("lay", "Layer") > score("lay", "Lock All Years"));
    // Word starts beat the middle of a word.
    assert!(score("mc", "Merge Chain") > score("mc", "Hammock"));
    // An exact match beats a longer text it prefixes.
    assert!(score("undo", "Undo") > score("undo", "Undo History"));
    // Fewer gaps are better.
    assert!(score("ab", "a_b") > score("ab", "a____b"));
}

#[test]
fn matcher_works_on_cjk_and_mixed_text() {
    let found = fuzzy_match("新建", "新建画布…").unwrap();
    assert_eq!(found.indices, [0, 1]);
    // Indices are characters, not bytes.
    assert_eq!(fuzzy_match("画布", "新建画布…").unwrap().indices, [2, 3]);
    assert_eq!(fuzzy_match("建布", "新建画布…").unwrap().indices, [1, 3]);
    assert!(score("新建", "新建画布…") > score("新建", "画布新建"));
    assert_eq!(fuzzy_match("l图", "Layer 图层").unwrap().indices, [0, 6]);
    assert!(fuzzy_match("新建", "新画布").is_none());
}

#[test]
fn commands_match_on_aliases_ids_categories_and_every_word() {
    let others = [
        ("Hue / Saturation", 2),
        ("hsl", 4),
        ("hue_saturation", 6),
        ("Image", 10),
    ];
    // The alias finds it, with nothing to highlight in the label.
    let found = match_command("hsl", "Hue / Saturation…", &others).unwrap();
    assert!(found.indices.is_empty());
    // A word in the label highlights it.
    assert_eq!(
        match_command("sat", "Hue / Saturation…", &others)
            .unwrap()
            .indices,
        [6, 7, 8]
    );
    // Every word must match: one in the label, one in the category.
    assert!(match_command("sat image", "Hue / Saturation…", &others).is_some());
    assert!(match_command("sat layer", "Hue / Saturation…", &others).is_none());
    // A label match scores above the same text found in an alias.
    let label = match_command("hue", "Hue", &[("hue", 4)]).unwrap();
    let alias = match_command("hue", "Tint", &[("hue", 4)]).unwrap();
    assert!(label.score > alias.score);
}

#[test]
fn registry_aliases_ids_and_categories_find_commands() {
    let keymap = Keymap::default();
    assert!(
        find(&keymap, false, "pan", &[]).contains(&"tool_hand".to_owned()),
        "alias"
    );
    assert!(
        find(&keymap, false, "tool_brush", &[]).contains(&"tool_brush".to_owned()),
        "id"
    );
    assert_eq!(
        find(&keymap, false, "Tools brush", &[])
            .first()
            .map(String::as_str),
        Some("tool_brush"),
        "category and label"
    );
    assert!(find(&keymap, false, "qqqqqq", &[]).is_empty());
}

#[test]
fn equal_scores_are_ordered_by_recency_then_by_registry_order() {
    let keymap = Keymap::default();
    let order = |recent: &[&str]| {
        let found = find(&keymap, false, "flip", recent);
        (
            found.iter().position(|id| id == "flip_h").unwrap(),
            found.iter().position(|id| id == "flip_v").unwrap(),
        )
    };
    let (h, v) = order(&[]);
    assert!(h < v, "registry order");
    let (h, v) = order(&["flip_v"]);
    assert!(v < h, "recent first");
    let (h, v) = order(&["flip_v", "flip_h"]);
    assert!(v < h, "most recent first");
    // Recency never beats a better score.
    let found = find(&keymap, false, "flip horizontal", &["flip_v"]);
    assert_eq!(found[0], "flip_h");
}

#[test]
fn empty_filter_lists_recent_commands_then_every_category() {
    let keymap = Keymap::default();
    let recent: Vec<String> = ["undo", "swap_colors", "no_such_command", "undo"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let rows = rank(keymap.entries(), false, "  ", &recent);
    let found = ids(&keymap, &rows);
    assert_eq!(&found[..2], ["undo", "swap_colors"]);
    assert!(rows[..2].iter().all(|r| r.group == Group::Recent));
    // Nothing is listed twice.
    let mut unique = found.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), found.len());
    // The rest follows the category order.
    let categories: Vec<commands::Category> = rows[2..]
        .iter()
        .map(|r| match r.group {
            Group::Category(category) => category,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(categories.is_sorted());
    assert_eq!(categories.first(), Some(&commands::Category::File));
}

#[test]
fn the_palette_lists_the_commands_of_the_current_scope() {
    let keymap = Keymap::default();
    let editor = find(&keymap, false, "", &[]);
    let develop = find(&keymap, true, "", &[]);
    assert!(editor.contains(&"new_layer".to_owned()));
    assert!(!editor.contains(&"develop_split".to_owned()));
    assert!(develop.contains(&"develop_split".to_owned()));
    assert!(develop.contains(&"undo".to_owned()), "commands for both");
    assert!(!develop.contains(&"new_layer".to_owned()));
    // Filtering respects the scope too.
    assert!(find(&keymap, false, "split view", &[]).is_empty());
    assert!(!find(&keymap, true, "split view", &[]).is_empty());
}

#[test]
fn every_registry_entry_is_reachable_from_the_palette() {
    let keymap = Keymap::default();
    let mut listed = std::collections::HashSet::new();
    for developing in [false, true] {
        listed.extend(find(&keymap, developing, "", &[]));
        // Typing a command's label or id finds it in the scope where it applies.
        for entry in keymap.entries() {
            if !entry.scope().active(developing) {
                continue;
            }
            for query in [entry.label(), entry.id.as_str()] {
                let found = find(&keymap, developing, query, &[]);
                assert!(
                    found.contains(&entry.id),
                    "{query:?} does not find {}",
                    entry.id
                );
            }
        }
    }
    let missing: Vec<&str> = keymap
        .entries()
        .iter()
        .map(|e| e.id.as_str())
        .filter(|id| !listed.contains(*id))
        .collect();
    assert!(missing.is_empty(), "not in the palette: {missing:?}");
    // The palette's own toggle is one of them, and so is every menu command.
    assert!(listed.contains("command_palette"));
    for id in super::keymap::menu_commands() {
        assert!(
            listed.contains(id.as_str()),
            "menu command {id} is not in the palette"
        );
    }
}

#[test]
fn translated_labels_and_english_names_both_match() {
    let keymap = Keymap::default();
    i18n::set_language(&Language::new("zh-CN"));
    let chinese = find(&keymap, false, "新建画布", &[]);
    let english_id = find(&keymap, false, "new_layer", &[]);
    let english_label = find(&keymap, false, "New Canvas", &[]);
    let alias = find(&keymap, false, "hotkeys", &[]);
    let rows = rank(keymap.entries(), false, "新建画布", &[]);
    i18n::set_language(&Language::english());
    assert_eq!(chinese.first().map(String::as_str), Some("new"));
    assert!(english_id.contains(&"new_layer".to_owned()));
    assert_eq!(english_label.first().map(String::as_str), Some("new"));
    assert!(alias.contains(&"shortcuts".to_owned()));
    assert_eq!(
        rows[0].indices,
        [0, 1, 2, 3],
        "highlights the translated label"
    );
}

#[test]
fn recent_commands_move_to_the_front_and_are_capped() {
    let mut recent = Vec::new();
    for n in 0..15 {
        push_recent(&mut recent, &format!("c{n}"));
    }
    assert_eq!(recent.len(), MAX_RECENT);
    assert_eq!(recent[0], "c14");
    push_recent(&mut recent, "c10");
    assert_eq!(&recent[..2], ["c10", "c14"]);
    assert_eq!(recent.iter().filter(|id| *id == "c10").count(), 1);
    assert_eq!(recent.len(), MAX_RECENT);
}

#[test]
fn recent_commands_are_optional_in_the_configuration_file() {
    let old: xuan::config::Config = toml::from_str("pixel_grid = false\n").unwrap();
    assert!(old.recent_commands.is_empty());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let mut config = xuan::config::Config::default();
    config.save(&path).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("recent"));
    config.recent_commands = vec!["undo".into(), "merge".into()];
    config.save(&path).unwrap();
    assert_eq!(xuan::config::Config::load(&path).unwrap(), config);
}

#[test]
fn selection_moves_wraps_and_stops_at_the_ends() {
    let mut palette = Palette::default();
    palette.step(Move::Up, 20);
    assert_eq!(palette.selected, 19, "Up from the top wraps");
    palette.step(Move::Down, 20);
    assert_eq!(palette.selected, 0, "Down from the bottom wraps");
    palette.step(Move::PageDown, 20);
    assert_eq!(palette.selected, 8);
    palette.step(Move::PageDown, 20);
    palette.step(Move::PageDown, 20);
    assert_eq!(palette.selected, 19, "pages stop at the end");
    palette.step(Move::PageUp, 20);
    assert_eq!(palette.selected, 11);
    palette.selected = 3;
    palette.step(Move::PageUp, 20);
    assert_eq!(palette.selected, 0, "pages stop at the start");
    assert!(palette.reveal);
    // A shorter list pulls the selection in; an empty one has none.
    palette.selected = 15;
    palette.step(Move::Down, 5);
    assert_eq!(palette.selected, 0);
    palette.step(Move::Down, 0);
    assert_eq!(palette.selected, 0);
    palette.selected = 7;
    palette.filter_changed();
    assert_eq!(palette.selected, 0, "a new filter starts at the best match");
}

//! The tool rail: every tool and both colour swatches stay reachable, in columns if need be.

use super::*;

const TOOLS: [&str; 18] = [
    "Move / Transform",
    "Marquee",
    "Lasso",
    "Magic Wand",
    "Crop",
    "Brush",
    "Pencil",
    "Eraser",
    "Spot Healing",
    "Clone Stamp",
    "Blur / Smudge",
    "Gradient",
    "Shape",
    "Pen",
    "Text",
    "Eyedropper",
    "Hand",
    "Zoom",
];

fn at_size(width: f32, height: f32) -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    ui.harness.set_size(Vec2::new(width, height));
    ui.harness.run_steps(5);
    (directory, ui)
}

fn button_rect(ui: &UiTest, label: &str) -> egui::Rect {
    ui.harness.get_by_role_and_label(Role::Button, label).rect()
}

fn swatch_rect(ui: &UiTest, label: &str) -> egui::Rect {
    ui.harness
        .get_by_role_and_label(Role::ColorWell, label)
        .rect()
}

fn columns(ui: &UiTest) -> usize {
    let mut lefts: Vec<i32> = TOOLS
        .iter()
        .map(|label| button_rect(ui, label).left().round() as i32)
        .collect();
    lefts.sort_unstable();
    lefts.dedup();
    lefts.len()
}

#[test]
fn every_tool_and_swatch_fits_in_the_window_at_the_minimum_size() {
    let (_directory, ui) = at_size(850.0, 560.0);
    let window = ui.harness.ctx.content_rect();
    for label in TOOLS {
        let rect = button_rect(&ui, label);
        assert!(window.contains_rect(rect), "{label} at {rect:?}");
    }
    for label in ["Foreground color", "Background color"] {
        let rect = swatch_rect(&ui, label);
        assert!(window.contains_rect(rect), "{label} at {rect:?}");
    }
    assert!(columns(&ui) >= 2);
}

#[test]
fn the_rail_is_one_column_in_a_large_window() {
    let (_directory, ui) = at_size(1280.0, 1080.0);
    assert_eq!(columns(&ui), 1);
    assert!(button_rect(&ui, "Zoom").right() <= 56.0);
}

#[test]
fn clicking_a_tool_in_a_later_column_selects_it() {
    let (_directory, mut ui) = at_size(850.0, 560.0);
    let first = button_rect(&ui, TOOLS[0]).left();
    let (label, tool) = [("Zoom", Tool::Zoom), ("Eraser", Tool::Erase)]
        .into_iter()
        .find(|(label, _)| button_rect(&ui, label).left() > first + 1.0)
        .expect("a tool outside the first column");
    ui.click(label);
    assert_eq!(ui.app().tool, tool);
}

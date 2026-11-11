//! Select → Paths… with no paths: the empty state's actions, and SVG entry under Advanced.

use super::*;

fn empty_paths(select_all: bool) -> UiTest {
    let mut ui = UiTest::with_document();
    if select_all {
        ui.app_mut().command("select_all");
    }
    ui.app_mut().command("paths");
    ui.settle();
    ui
}

#[test]
fn no_paths_shows_only_the_ways_to_make_one() {
    let ui = empty_paths(false);
    assert!(ui.has("No paths yet"));
    assert!(ui.enabled("Draw with Pen"));
    // Tracing needs a selection.
    assert!(!ui.enabled("From selection"));
    for hidden in ["Fill Path", "Stroke Path", "Rename", "Make Selection"] {
        assert!(!ui.has(hidden), "{hidden} has no path to act on");
    }
    // SVG entry waits under Advanced.
    assert!(ui.has("Advanced"));
    assert!(!ui.has("Add Path"));
}

#[test]
fn draw_with_pen_switches_to_the_pen_and_closes_the_dialog() {
    let mut ui = empty_paths(false);
    ui.click("Draw with Pen");
    assert_eq!(ui.app().tool, Tool::Pen);
    assert_eq!(ui.app().dialog, None);
    assert!(ui.app().pen_target().is_none());
}

#[test]
fn from_selection_traces_a_path_and_shows_its_actions() {
    let mut ui = empty_paths(true);
    assert!(ui.enabled("From selection"));
    ui.click("From selection");
    let paths = &ui.app().session().unwrap().document.paths;
    assert_eq!(paths.len(), 1);
    assert_eq!(ui.app().dialog, Some(Dialog::Paths));
    assert!(!ui.has("No paths yet"));
    assert!(ui.has("Fill Path"));
}

#[test]
fn advanced_adds_a_path_from_svg_data() {
    let mut ui = empty_paths(false);
    ui.click("Advanced");
    assert!(ui.has("Add Path"));
    ui.harness.get_by_role(Role::MultilineTextInput).click();
    ui.harness.step();
    (ui.harness.get_by_role(Role::MultilineTextInput)).type_text("M 2 2 L 10 2 L 10 10 Z");
    ui.settle();
    ui.click("Add Path");
    assert_eq!(ui.app().session().unwrap().document.paths.len(), 1);
    assert!(ui.has("Fill Path"));
}

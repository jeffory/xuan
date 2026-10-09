//! Image → Canvas Size…: the anchor picker's cells are radio buttons with comfortable hit areas.

use super::*;

fn canvas_size() -> UiTest {
    let mut ui = UiTest::with_document();
    ui.app_mut().command("canvas_size");
    ui.settle();
    assert_eq!(ui.app().dialog, Some(Dialog::CanvasSize));
    ui
}

fn selected(ui: &UiTest, label: &str) -> bool {
    ui.harness
        .get_by_role_and_label(Role::RadioButton, label)
        .accesskit_node()
        .toggled()
        == Some(egui::accesskit::Toggled::True)
}

const CELLS: [&str; 9] = [
    "Top left",
    "Top",
    "Top right",
    "Left",
    "Centre",
    "Right",
    "Bottom left",
    "Bottom",
    "Bottom right",
];

#[test]
fn anchor_cells_are_large_named_and_start_centred() {
    let ui = canvas_size();
    for label in CELLS {
        let rect = ui
            .harness
            .get_by_role_and_label(Role::RadioButton, label)
            .rect();
        assert!(
            rect.width() >= 24.0 && rect.height() >= 24.0,
            "{label}: {rect:?}"
        );
        assert_eq!(selected(&ui, label), label == "Centre", "{label}");
    }
}

#[test]
fn clicking_a_cell_sets_the_anchor_and_canvas_size_uses_it() {
    let mut ui = canvas_size();
    ui.click_role(Role::RadioButton, "Bottom right");
    assert_eq!(ui.app().anchor, [1.0, 1.0]);
    assert!(selected(&ui, "Bottom right"));
    assert!(!selected(&ui, "Centre"));
    ui.click_role(Role::RadioButton, "Top");
    assert_eq!(ui.app().anchor, [0.5, 0.0]);
    assert!(selected(&ui, "Top"));
}

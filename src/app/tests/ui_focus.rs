//! Keyboard focus (#81): the tool rail, layer-row icons, document tabs and number fields take the
//! focus, and show the shared accent ring while they have it.

use super::*;
use crate::app::widgets::FOCUS_RING_WIDTH;
use egui::{Key, Modifiers};

/// Every rectangle painted in the last frame, nested shapes included.
fn painted_rects(ui: &UiTest) -> Vec<egui::epaint::RectShape> {
    fn collect(shape: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
        match shape {
            egui::Shape::Rect(rect) => out.push(rect.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &ui.harness.output().shapes {
        collect(&clipped.shape, &mut out);
    }
    out
}

/// Whether a focus ring was painted on or just round `rect`: a 2-point stroke in the accent,
/// centred on it, no smaller and at most a few points larger.
fn ring_round(ui: &UiTest, rect: egui::Rect) -> bool {
    let accent = ui.ctx().palette().accent;
    painted_rects(ui).iter().any(|shape| {
        shape.stroke.width == FOCUS_RING_WIDTH
            && shape.stroke.color == accent
            && (shape.rect.center() - rect.center()).length() < 1.5
            && shape.rect.width() >= rect.width() - 1.0
            && shape.rect.width() <= rect.width() + 6.0
            && shape.rect.height() >= rect.height() - 1.0
            && shape.rect.height() <= rect.height() + 6.0
    })
}

/// Focuses the node, as assistive technology or `request_focus` would, and settles.
fn focus(ui: &mut UiTest, role: Role, label: &str) {
    ui.harness.get_by_role_and_label(role, label).focus();
    ui.settle();
}

#[track_caller]
fn assert_focused_with_ring(ui: &UiTest, role: Role, label: &str) {
    let node = ui.harness.get_by_role_and_label(role, label);
    assert!(node.is_focused(), "{label} has the focus");
    let rect = node.rect();
    assert!(ring_round(ui, rect), "a focus ring round {label} at {rect:?}");
}

#[test]
fn a_tool_rail_button_shows_focus_and_tab_moves_it_on() {
    let mut ui = UiTest::with_document();
    let rect = ui
        .harness
        .get_by_role_and_label(Role::Button, "Marquee")
        .rect();
    assert!(!ring_round(&ui, rect), "no ring before it has the focus");
    focus(&mut ui, Role::Button, "Marquee");
    assert_focused_with_ring(&ui, Role::Button, "Marquee");
    // Tab moves to the next tool, and the ring with it.
    ui.key(Key::Tab);
    assert_focused_with_ring(&ui, Role::Button, "Lasso");
    assert!(!ring_round(&ui, rect), "the ring left Marquee");
}

#[test]
fn a_layer_eye_shows_focus_and_space_toggles_it() {
    let mut ui = UiTest::with_document();
    focus(&mut ui, Role::CheckBox, "Toggle visibility");
    assert_focused_with_ring(&ui, Role::CheckBox, "Toggle visibility");
    let visible = |ui: &UiTest| {
        ui.app()
            .session()
            .unwrap()
            .document
            .layers
            .iter()
            .all(|layer| layer.visible)
    };
    assert!(visible(&ui));
    ui.key(Key::Space);
    assert!(!visible(&ui), "Space works the focused eye");
}

#[test]
fn layer_actions_are_named_and_show_focus() {
    let mut ui = UiTest::with_document();
    for label in [
        "New layer (Ctrl+Shift+N)",
        "Group layers (Ctrl+G)",
        "New adjustment layer",
        "New filter layer",
        "Delete layer",
    ] {
        assert!(ui.has_role(Role::Button, label), "{label}");
    }
    focus(&mut ui, Role::Button, "Delete layer");
    assert_focused_with_ring(&ui, Role::Button, "Delete layer");
}

#[test]
fn a_document_tab_shows_focus() {
    let mut ui = UiTest::with_document();
    ui.app_mut().session_mut().unwrap().title = "Alpha".into();
    ui.settle();
    let tab = ui.harness.get_by_label("Alpha").rect();
    assert!(!ring_round(&ui, tab));
    ui.harness.get_by_label("Alpha").focus();
    ui.settle();
    assert!(ui.harness.get_by_label("Alpha").is_focused());
    assert!(ring_round(&ui, tab), "a focus ring inside the tab at {tab:?}");
    // The tab's close button follows it in the Tab order, and shows while focused.
    focus(&mut ui, Role::Button, "Close Alpha");
    assert_focused_with_ring(&ui, Role::Button, "Close Alpha");
    assert!(ui.has_role(Role::Button, "New canvas"), "the + button is named");
}

#[test]
fn a_number_field_shows_focus() {
    let mut ui = UiTest::with_document();
    ui.press(Modifiers::COMMAND, Key::N);
    let field = ui
        .harness
        .get_all_by_role(Role::SpinButton)
        .next()
        .expect("the New canvas dialog's width field");
    let rect = field.rect();
    field.focus();
    ui.settle();
    let focused = ui
        .harness
        .get_all_by_role(Role::SpinButton)
        .find(|node| node.rect().center() == rect.center())
        .map(|node| node.is_focused());
    // egui swaps a focused DragValue for its text editor, whose node may take the role.
    let editing = ui.harness.get_all_by_role(Role::TextInput).any(|node| {
        node.is_focused() && (node.rect().center() - rect.center()).length() < 2.0
    });
    assert!(focused == Some(true) || editing, "the field has the focus");
    assert!(ring_round(&ui, rect), "a focus ring round the field at {rect:?}");
}

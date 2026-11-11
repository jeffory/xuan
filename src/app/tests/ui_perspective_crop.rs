//! The Crop tool's Perspective mode in the options bar: the Box / Perspective switch, the size
//! fields and Auto, the corners' state, and Reset, Cancel and Apply.
use super::*;
use xuan::crop::perspective::{self, Quad};

/// A 120 × 100 document with the Crop tool switched to Perspective in its options bar.
fn perspective_ui() -> UiTest {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [120, 100];
    ui.app_mut().new_document();
    ui.settle();
    ui.key(egui::Key::C);
    assert_eq!(ui.app().tool, Tool::Crop);
    assert!(!ui.app().crop.perspective);
    ui.click("Perspective");
    assert!(ui.app().crop.perspective);
    ui
}

/// Drags on the canvas between two points in document pixels.
fn drag_canvas(ui: &mut UiTest, from: Point, to: Point) {
    let rect = ui.app().canvas_rect.unwrap();
    let zoom = ui.app().session().unwrap().zoom;
    let at = |p: Point| rect.min + Vec2::new(p.x, p.y) * zoom;
    ui.drag(at(from), at(to));
}

/// Pulls in the top corners, as for a page shot from below.
fn tilted() -> UiTest {
    let mut ui = perspective_ui();
    drag_canvas(&mut ui, Point::new(12.0, 10.0), Point::new(25.0, 12.0));
    drag_canvas(&mut ui, Point::new(108.0, 10.0), Point::new(95.0, 14.0));
    ui
}

/// What straightening changes: the canvas size, each layer's transform, and the history.
fn straightened(ui: &UiTest) -> (u32, u32, Vec<String>, Vec<String>) {
    let session = ui.app().session().unwrap();
    let document = &session.document;
    (
        document.width,
        document.height,
        document
            .layers
            .iter()
            .map(|l| format!("{:?}", l.transform))
            .collect(),
        session.history.names().map(str::to_owned).collect(),
    )
}

fn quad(ui: &UiTest) -> Quad {
    ui.app().crop.quad.unwrap()
}

#[test]
fn the_bar_switches_modes_and_shows_the_size_from_the_corners() {
    let mut ui = perspective_ui();
    // The ratio menu belongs to the box.
    assert!(!ui.has_role(Role::ComboBox, "Free"));
    assert!(ui.has("Size"));
    assert!(ui.has("Drag the corners onto the edges, then press Enter to apply"));
    assert!(ui.enabled("Reset") && ui.enabled("Cancel") && ui.enabled("Apply"));
    assert!(!ui.enabled("Auto"));
    let values: Vec<String> = ui
        .harness
        .query_all_by_role(Role::SpinButton)
        .filter_map(|node| node.value())
        .collect();
    assert!(values.iter().any(|v| v == "96"), "{values:?}");
    assert!(values.iter().any(|v| v == "80"), "{values:?}");
    // A typed size stays until Auto puts the corners' size back.
    ui.app_mut().crop.size = Some([40, 50]);
    ui.settle();
    assert!(ui.enabled("Auto"));
    ui.click("Auto");
    assert_eq!(ui.app().crop.size, None);
    ui.click("Box");
    assert!(!ui.app().crop.perspective && ui.app().crop.quad.is_none());
    assert!(ui.has_role(Role::ComboBox, "Free"));
}

#[test]
fn apply_and_enter_straighten_the_same_way() {
    let mut by_button = tilted();
    let corners = quad(&by_button);
    by_button.click("Apply");
    let mut by_key = tilted();
    by_key.key(egui::Key::Enter);
    let result = straightened(&by_button);
    assert_eq!([result.0, result.1], perspective::output_size(corners));
    assert_eq!(result.3, ["Perspective Crop"]);
    assert_eq!(result, straightened(&by_key));
    assert!(by_button.app().crop.quad.is_none() && by_key.app().crop.quad.is_none());
    // Nothing left to apply; Reset starts again on the new canvas.
    assert!(!by_button.enabled("Apply") && !by_button.enabled("Cancel"));
    assert!(by_button.has("Click four corners, or press Reset to start from the canvas"));
    by_button.click("Reset");
    assert!(by_button.app().crop.quad.is_some());
    assert!(by_button.app().crop.perspective);
}

#[test]
fn cancel_and_escape_discard_the_corners() {
    let mut ui = tilted();
    let before = straightened(&ui);
    ui.click("Cancel");
    assert!(ui.app().crop.quad.is_none());
    assert_eq!(straightened(&ui), before);
    ui.click("Reset");
    ui.key(egui::Key::Escape);
    assert!(ui.app().crop.quad.is_none());
    assert_eq!(straightened(&ui), before);
    assert!(before.3.is_empty());
}

#[test]
fn reset_puts_moved_corners_back() {
    let mut ui = tilted();
    ui.click("Reset");
    assert_eq!(quad(&ui), perspective::initial([120, 100], None));
}

#[test]
fn corners_that_cannot_be_straightened_say_why() {
    let mut ui = perspective_ui();
    let [a, b, c, d] = quad(&ui);
    ui.app_mut().crop.quad = Some([a, b, d, c]);
    ui.settle();
    let message = perspective::Degenerate::SelfIntersecting.message();
    assert!(ui.has(message));
    ui.click("Apply");
    assert!(straightened(&ui).3.is_empty());
    assert!(ui.app().crop.quad.is_some());
}

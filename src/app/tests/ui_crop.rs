//! The Crop tool's options bar: the Ratio menu, Custom W : H, Swap, the size readout, and Cancel
//! and Apply beside them.
use super::*;
use xuan::crop::{CropBox, Ratio};

/// A 120 × 100 document with the Crop tool picked by its key.
fn crop_ui() -> UiTest {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [120, 100];
    ui.app_mut().new_document();
    ui.settle();
    ui.key(egui::Key::C);
    assert_eq!(ui.app().tool, Tool::Crop);
    ui
}

/// Drags on the canvas between two points in document pixels.
fn drag_canvas(ui: &mut UiTest, from: Point, to: Point) {
    let rect = ui.app().canvas_rect.unwrap();
    let zoom = ui.app().session().unwrap().zoom;
    let at = |p: Point| rect.min + Vec2::new(p.x, p.y) * zoom;
    ui.drag(at(from), at(to));
}

fn pick_ratio(ui: &mut UiTest, current: &str, choice: &str) {
    ui.harness
        .get_by_role_and_label(Role::ComboBox, current)
        .click();
    ui.settle();
    ui.click(choice);
}

/// Draws a 9:16 box, adjusts it by a handle and moves it, as both apply tests do.
fn adjusted_box() -> UiTest {
    let mut ui = crop_ui();
    pick_ratio(&mut ui, "Free", "9:16");
    drag_canvas(&mut ui, Point::new(10.0, 10.0), Point::new(40.0, 60.0));
    drag_canvas(&mut ui, Point::new(37.0, 58.0), Point::new(46.0, 74.0));
    drag_canvas(&mut ui, Point::new(30.0, 40.0), Point::new(35.0, 45.0));
    assert_eq!(ui.app().crop.rect, Some(CropBox::new(15, 15, 36, 64)));
    ui
}

/// What a crop changes: the canvas size and where each layer sits on it.
fn cropped(ui: &UiTest) -> (u32, u32, Vec<String>, Vec<String>) {
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

#[test]
fn ratio_menu_lists_the_presets_and_shows_the_choice() {
    let mut ui = crop_ui();
    assert!(ui.has_role(Role::ComboBox, "Free"));
    assert!(!ui.enabled("Swap"));
    ui.harness
        .get_by_role_and_label(Role::ComboBox, "Free")
        .click();
    ui.settle();
    for label in [
        "Original (6:5)",
        "1:1",
        "4:3",
        "3:4",
        "3:2",
        "2:3",
        "16:9",
        "9:16",
        "9:20",
        "5:4",
        "4:5",
        "Custom",
    ] {
        assert!(ui.has(label), "missing {label}");
    }
    ui.click("9:16");
    assert_eq!(ui.app().crop.ratio, Ratio::Preset([9, 16]));
    assert!(ui.has_role(Role::ComboBox, "9:16"));
    // Swap turns it into 16:9.
    ui.click("Swap");
    assert_eq!(ui.app().crop.ratio, Ratio::Preset([16, 9]));
    assert!(ui.has_role(Role::ComboBox, "16:9"));
}

#[test]
fn custom_shows_its_fields_and_swap_exchanges_them() {
    let mut ui = crop_ui();
    let fields = |ui: &UiTest| ui.harness.query_all_by_role(Role::SpinButton).count();
    let before = fields(&ui);
    pick_ratio(&mut ui, "Free", "Custom");
    assert_eq!(ui.app().crop.ratio, Ratio::Custom);
    assert_eq!(fields(&ui), before + 2);
    ui.app_mut().crop.custom = [9, 20];
    ui.settle();
    drag_canvas(&mut ui, Point::new(10.0, 10.0), Point::new(60.0, 95.0));
    let rect = ui.app().crop.rect.unwrap();
    assert_eq!((rect.width, rect.height), (36, 80));
    assert!(ui.has("36 × 80 px"));
    ui.click("Swap");
    assert_eq!(ui.app().crop.custom, [20, 9]);
    let rect = ui.app().crop.rect.unwrap();
    assert_eq!(rect.width * 9, rect.height * 20, "{rect:?}");
}

#[test]
fn cancel_discards_the_box_and_is_disabled_without_one() {
    let mut ui = crop_ui();
    assert!(!ui.enabled("Cancel"));
    assert!(!ui.enabled("Apply"));
    drag_canvas(&mut ui, Point::new(10.0, 10.0), Point::new(50.0, 30.0));
    assert!(ui.has("40 × 20 px"));
    assert!(ui.enabled("Cancel") && ui.enabled("Apply"));
    ui.click("Cancel");
    assert!(ui.app().crop.rect.is_none());
    assert_eq!(cropped(&ui).0, 120);
    assert!(cropped(&ui).3.is_empty());
}

#[test]
fn apply_and_enter_crop_the_same_way() {
    let mut by_button = adjusted_box();
    by_button.click("Apply");
    let mut by_key = adjusted_box();
    by_key.key(egui::Key::Enter);
    let result = cropped(&by_button);
    assert_eq!(result.0, 36);
    assert_eq!(result.1, 64);
    assert_eq!(result.3, ["Crop"]);
    assert_eq!(result, cropped(&by_key));
    assert!(by_button.app().crop.rect.is_none() && by_key.app().crop.rect.is_none());
    // The ratio is still chosen for the next crop.
    assert!(by_button.has_role(Role::ComboBox, "9:16"));
}

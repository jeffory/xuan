//! File → New Collage… and Image → Collage Layout… as the user sees them.
use super::*;
use xuan::collage::{self, Template};

/// The New Collage dialog, opened from the File menu, for a small 200 × 160 canvas.
fn new_collage() -> UiTest {
    let mut ui = UiTest::new();
    ui.open_menu("File");
    ui.click("New Collage…");
    assert!(ui.app().dialog == Some(Dialog::Collage));
    assert!(ui.has("New collage"));
    ui.app_mut().collage.as_mut().unwrap().size = [200, 160];
    ui.settle();
    ui
}

fn cells(ui: &UiTest) -> usize {
    collage::cells(&ui.app().session().unwrap().document).len()
}

#[test]
fn new_collage_creates_a_two_by_two_collage() {
    let mut ui = new_collage();
    assert!(ui.has("Columns") && ui.has("Rows"));
    assert!(ui.has("Choose photos…"));
    assert!(ui.has("No photos yet: drop or import images into the cells later."));
    ui.click("Create collage");
    assert!(ui.app().dialog.is_none());
    assert_eq!(ui.app().sessions.len(), 1);
    assert_eq!(cells(&ui), 4);
    let document = &ui.app().session().unwrap().document;
    assert_eq!((document.width, document.height), (200, 160));
}

#[test]
fn choosing_a_template_hides_the_grid_size() {
    let mut ui = new_collage();
    ui.click("Grid");
    ui.click("One large, two small");
    let edit = ui.app().collage.as_ref().unwrap();
    assert_eq!(edit.layout.template, Template::LargeLeft);
    assert!(!ui.has("Columns"));
    ui.click("Create collage");
    assert_eq!(cells(&ui), 3);
}

#[test]
fn a_layout_without_room_is_explained_and_cannot_be_created() {
    let mut ui = new_collage();
    ui.app_mut().collage.as_mut().unwrap().layout.border = 100;
    ui.settle();
    assert!(ui.has("The border and spacing leave no room for the photos"));
    assert!(!ui.enabled("Create collage"));
    ui.key(egui::Key::Escape);
    assert!(ui.app().dialog.is_none());
    assert!(ui.app().sessions.is_empty());
}

#[test]
fn more_photos_than_cells_are_noted() {
    let mut ui = new_collage();
    let photos = vec![PathBuf::from("a.png"); 5];
    ui.app_mut().collage.as_mut().unwrap().photos = photos;
    ui.settle();
    assert!(ui.has("5 photos; the first 4 fill the cells"));
    ui.click("Clear photos");
    assert!(ui.app().collage.as_ref().unwrap().photos.is_empty());
}

#[test]
fn collage_layout_from_the_image_menu_applies_in_one_undo_step() {
    let mut ui = new_collage();
    ui.click("Create collage");
    let before = ui.app().session().unwrap().history.revision;
    ui.open_menu("Image");
    ui.click("Collage Layout…");
    assert!(ui.has("Collage layout"));
    // The canvas keeps its size, and there are no photos to choose.
    assert!(!ui.has("Width"));
    assert!(!ui.has("Choose photos…"));
    ui.app_mut().collage.as_mut().unwrap().layout.columns = 3;
    ui.settle();
    ui.click("Apply");
    assert!(ui.app().dialog.is_none());
    assert_eq!(cells(&ui), 6);
    assert_eq!(ui.app().session().unwrap().history.revision, before + 1);
}

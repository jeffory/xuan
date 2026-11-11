//! Color Lookup adjustment layers: offered by the adjustment menus, added by dropping a `.cube`
//! file, set up in the adjustment dialog, and refused with a reason when the file is broken.
use super::*;
use xuan::{
    document::Adjustment,
    lut::{Dimension, Interpolation, Lut},
};

/// A 3D table that swaps red and blue, written as `name` in `directory`.
fn write_table(directory: &Path, name: &str) -> PathBuf {
    let mut table = Lut::identity(Dimension::Three, 3);
    table.title = "Swap".into();
    for entry in &mut table.table {
        entry.swap(0, 2);
    }
    let path = directory.join(name);
    std::fs::write(&path, table.to_cube()).unwrap();
    path
}

fn active(ui: &UiTest) -> &Layer {
    ui.app().session().unwrap().document.active().unwrap()
}

#[test]
fn the_adjustment_menus_offer_color_lookup() {
    let mut ui = UiTest::with_document();
    ui.open_menu("Layer");
    ui.click("New Adjustment Layer ⏵");
    assert!(ui.has("Colour Lookup…"));
    ui.key(egui::Key::Escape);
    ui.open_menu("Image");
    ui.click("Adjustments ⏵");
    assert!(ui.has("Colour Lookup…"));
}

#[test]
fn a_dropped_cube_file_becomes_a_color_lookup_layer() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_table(directory.path(), "swap.cube");
    let mut ui = UiTest::with_document();
    let layers = ui.app().session().unwrap().document.layers.len();
    ui.drop_files(&[&path]);
    assert!(ui.app().dialog == Some(Dialog::Effect));
    assert!(ui.has("swap.cube"));
    assert!(ui.has("Swap · 3D table, 3 points a side"));
    assert!(ui.has("Non-destructive effect layer"));
    ui.click("Trilinear");
    ui.click("Apply");
    assert!(ui.app().dialog.is_none());

    let session = ui.app().session().unwrap();
    assert_eq!(session.document.layers.len(), layers + 1);
    let layer = active(&ui);
    assert_eq!(layer.name, "Color Lookup");
    let Some(Adjustment::ColorLookup {
        name,
        interpolation,
        table,
    }) = &layer.adjustment
    else {
        panic!("{:?}", layer.adjustment);
    };
    assert_eq!(name, "swap.cube");
    assert_eq!(*interpolation, Interpolation::Trilinear);
    assert_eq!(**table, Lut::load(&path).unwrap());

    // One undo step, which takes the layer away again.
    ui.press(egui::Modifiers::CTRL, egui::Key::Z);
    assert_eq!(ui.app().session().unwrap().document.layers.len(), layers);
}

#[test]
fn a_1d_table_has_no_interpolation_to_choose() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("curve.CUBE");
    std::fs::write(&path, Lut::identity(Dimension::One, 16).to_cube()).unwrap();
    let mut ui = UiTest::with_document();
    ui.drop_files(&[&path]);
    assert!(ui.has("1D table, 16 entries"));
    assert!(!ui.enabled("Trilinear"));
    ui.click("Apply");
    assert!(matches!(
        active(&ui).adjustment,
        Some(Adjustment::ColorLookup { .. })
    ));
}

#[test]
fn a_broken_cube_file_is_refused_and_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("truncated.cube");
    std::fs::write(&path, "LUT_3D_SIZE 17\n0 0 0\n0.5 0.5 0.5\n").unwrap();
    let mut ui = UiTest::with_document();
    let before = ui.app().session().unwrap().document.layers.len();
    let revision = ui.app().session().unwrap().history.revision;
    ui.drop_files(&[&path]);
    let error = ui.app().error.clone().expect("the file is refused");
    assert!(
        error.contains("Could not load the colour lookup table")
            && error.contains("incomplete: 2 of 4913 entries"),
        "{error}"
    );
    assert!(ui.app().dialog.is_none());
    let session = ui.app().session().unwrap();
    assert_eq!(session.document.layers.len(), before);
    assert_eq!(session.history.revision, revision);
}

#[test]
fn a_cube_file_needs_an_open_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_table(directory.path(), "swap.cube");
    let mut ui = UiTest::new();
    ui.drop_files(&[&path]);
    assert!(ui.app().sessions.is_empty());
    assert_eq!(
        ui.app().error.as_deref(),
        Some("Open an image before adding a colour lookup table")
    );
}

//! File → New's size presets (issue 140): pixel presets in pixels, paper sizes in print units,
//! and the lists read from `canvas-presets.toml`.

use super::units::{choose, commit, document, editor, enter};
use super::*;

/// Opens File → New on `size` at 72 ppi, with preferences in a temporary folder.
fn new_canvas(size: [u32; 2]) -> (tempfile::TempDir, UiTest) {
    let (directory, mut ui) = editor(None, 72.0);
    ui.app_mut().dimensions = size;
    ui.app_mut().command("new");
    ui.settle();
    (directory, ui)
}

fn presets_file(directory: &tempfile::TempDir) -> std::path::PathBuf {
    directory.path().join(xuan::canvas_presets::FILE)
}

#[test]
fn paper_sizes_are_offered_in_print_units() {
    let (directory, mut ui) = new_canvas([1920, 1080]);
    assert!(ui.has_role(Role::ComboBox, "Full High Definition (1920 × 1080)"));
    choose(&mut ui, "Pixels", "Millimetres");
    // The size stays, and it is no paper size.
    assert_eq!(ui.app().dimensions, [1920, 1080]);
    choose(&mut ui, "Custom", "A4 (210 × 297 mm)");
    assert_eq!(ui.app().resolution, 300.0);
    assert_eq!(ui.app().dimensions, [2480, 3508]);
    assert!(ui.has_role(Role::ComboBox, "A4 (210 × 297 mm)"));
    assert!(ui.has("2480 × 3508 px · 8.7 MP"));
    // The menu has the paper groups and no pixel presets.
    ui.harness
        .get_by_role_and_label(Role::ComboBox, "A4 (210 × 297 mm)")
        .click();
    ui.settle();
    assert!(ui.has("ISO A") && ui.has("ISO B") && ui.has("US"));
    assert!(!ui.has("Full High Definition (1920 × 1080)"));
    // Letter is in inches: picking it switches the unit.
    ui.click("Letter (8.5 × 11 in)");
    assert!(ui.has_role(Role::ComboBox, "Inches"));
    assert_eq!(ui.app().dimensions, [2550, 3300]);
    commit(&mut ui, "Create canvas");
    assert_eq!(document(&ui), (2550, 3300, 300.0));
    // The unit is remembered; no presets file is written for using a preset.
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    assert_eq!(saved.units.size, xuan::units::Unit::Inches);
    assert!(!presets_file(&directory).exists());
}

#[test]
fn a_paper_size_keeps_its_print_size_and_units_round_trip() {
    let (_directory, mut ui) = new_canvas([800, 600]);
    choose(&mut ui, "Pixels", "Millimetres");
    choose(&mut ui, "Custom", "A4 (210 × 297 mm)");
    // A new resolution keeps the paper size, so the pixels change.
    enter(&mut ui, 2, "150");
    assert_eq!(ui.app().dimensions, [1240, 1754]);
    assert!(ui.has_role(Role::ComboBox, "A4 (210 × 297 mm)"));
    enter(&mut ui, 2, "300");
    assert_eq!(ui.app().dimensions, [2480, 3508]);
    // Switching units back and forth never changes the pixels.
    for (from, to) in [
        ("Millimetres", "Inches"),
        ("Inches", "Pixels"),
        ("Pixels", "Centimetres"),
        ("Centimetres", "Millimetres"),
        ("Millimetres", "Pixels"),
    ] {
        choose(&mut ui, from, to);
        assert_eq!(ui.app().dimensions, [2480, 3508], "{to}");
    }
    // In pixels the pixel presets are offered, and A4 is Custom there.
    assert!(ui.has_role(Role::ComboBox, "Custom"));
    // A new resolution in pixels keeps the pixels.
    enter(&mut ui, 2, "72");
    assert_eq!(ui.app().dimensions, [2480, 3508]);
    // A pixel preset in pixels leaves the unit and resolution alone.
    choose(&mut ui, "Custom", "Square post (1080 × 1080)");
    assert_eq!(ui.app().dimensions, [1080, 1080]);
    assert_eq!(ui.app().resolution, 72.0);
    assert!(ui.has_role(Role::ComboBox, "Pixels"));
}

#[test]
fn hand_edited_presets_are_read_each_time_the_dialog_opens() {
    let (directory, mut ui) = new_canvas([800, 600]);
    let path = presets_file(&directory);
    let edited = "\
# My sizes
[[pixel]]
group = \"Mine\"
name = \"Banner ad\" # for the shop
width = 800
height = 600

[[pixel]]
name = \"Broken\"
width = -1
height = 600
";
    std::fs::write(&path, edited).unwrap();
    // Read again when File → New opens.
    ui.key(egui::Key::Escape);
    ui.app_mut().command("new");
    ui.settle();
    assert!(ui.has_role(Role::ComboBox, "Banner ad (800 × 600)"));
    // The bad entry is skipped with a note; the physical list is the built-in one.
    assert!(ui.has("Some canvas presets could not be read and were skipped"));
    assert_eq!(ui.app().canvas_presets.skipped.len(), 1);
    assert!(ui.app().canvas_presets.skipped[0].starts_with("pixel preset 2:"));
    assert_eq!(
        ui.app()
            .canvas_presets
            .list
            .list(xuan::canvas_presets::Kind::Physical)
            .len(),
        16
    );
    commit(&mut ui, "Create canvas");
    // Nothing rewrote the file.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
}

#[test]
fn a_broken_presets_file_is_reported_and_left_alone() {
    let (directory, mut ui) = new_canvas([1920, 1080]);
    let path = presets_file(&directory);
    std::fs::write(&path, "[[pixel]\nname = ").unwrap();
    ui.key(egui::Key::Escape);
    ui.app_mut().command("new");
    ui.settle();
    let error = ui.app().canvas_presets.error.clone().unwrap();
    assert!(error.contains("Cannot read"), "{error}");
    assert!(ui.has(&error));
    // The built-in lists are offered meanwhile.
    assert!(ui.has_role(Role::ComboBox, "Full High Definition (1920 × 1080)"));
    commit(&mut ui, "Create canvas");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[[pixel]\nname = ");
}

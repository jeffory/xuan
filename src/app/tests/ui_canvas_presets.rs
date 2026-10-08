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

/// Opens the Preset menu, which shows `current`, and picks its command `command`.
fn preset_command(ui: &mut UiTest, current: &str, command: &str) {
    ui.harness
        .get_by_role_and_label(Role::ComboBox, current)
        .click();
    ui.settle();
    ui.click(command);
}

/// Replaces the text of the field labelled `field`.
fn fill(ui: &mut UiTest, field: &str, text: &str) {
    ui.click_role(Role::TextInput, field);
    ui.press(egui::Modifiers::COMMAND, egui::Key::A);
    ui.type_keys(text);
}

fn names(ui: &UiTest, kind: xuan::canvas_presets::Kind) -> Vec<String> {
    let list = ui.app().canvas_presets.list.list(kind);
    list.iter().map(|p| p.name.clone()).collect()
}

#[test]
fn the_current_size_is_saved_as_a_preset_that_survives_a_restart() {
    let (directory, mut ui) = new_canvas([1000, 700]);
    let path = presets_file(&directory);
    preset_command(&mut ui, "Custom", "Save current size as preset…");
    assert_eq!(ui.app().dialog, Some(Dialog::CanvasPresets));
    assert!(ui.has("Current size: 1000 × 700 px"));
    let edit = ui.app().canvas_presets.edit.clone().unwrap();
    assert_eq!(
        (edit.name.as_str(), edit.group.as_str()),
        ("Preset 1", "Saved")
    );
    fill(&mut ui, "Name", "Poster");
    fill(&mut ui, "Group", "Mine");
    // Nothing is written until the list changes.
    assert!(!path.exists());
    ui.click("Save current size");
    assert!(path.exists());
    assert!(
        ui.app()
            .canvas_presets
            .edit
            .as_ref()
            .unwrap()
            .error
            .is_none()
    );
    assert!(ui.has("Mine") && ui.has("Poster (1000 × 700)"));
    ui.click("Done");
    // Back in File → New, as it was, with the new preset shown.
    assert_eq!(ui.app().dialog, Some(Dialog::New));
    assert_eq!(ui.app().dimensions, [1000, 700]);
    assert!(ui.has_role(Role::ComboBox, "Poster (1000 × 700)"));
    // Another start reads it from the file.
    let mut again = UiTest::new();
    again.isolate_config(directory.path());
    again.app_mut().dimensions = [700, 1000];
    again.app_mut().command("new");
    again.settle();
    assert!(again.has_role(Role::ComboBox, "Poster (1000 × 700)"));
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.starts_with("# Xuan canvas presets"));
    assert!(
        saved.contains(
            "[[pixel]]\ngroup = \"Mine\"\nname = \"Poster\"\nwidth = 1000\nheight = 700\n"
        )
    );
}

#[test]
fn a_print_size_is_saved_in_its_unit_and_resolution() {
    let (directory, mut ui) = new_canvas([800, 600]);
    choose(&mut ui, "Pixels", "Millimetres");
    choose(&mut ui, "Custom", "A4 (210 × 297 mm)");
    enter(&mut ui, 0, "100");
    assert_eq!(ui.app().dimensions, [1181, 3508]);
    preset_command(&mut ui, "Custom", "Save current size as preset…");
    assert!(ui.has("Current size: 100 × 297 mm · 300 ppi"));
    fill(&mut ui, "Name", "Card");
    ui.click("Save current size");
    let physical = xuan::canvas_presets::Kind::Physical;
    let list = ui.app().canvas_presets.list.list(physical);
    let card = list.iter().find(|p| p.name == "Card").unwrap();
    assert_eq!(card.group, "Saved");
    assert_eq!(
        card.size,
        xuan::canvas_presets::Size::Physical {
            width: 100.0,
            height: 297.0,
            unit: xuan::units::Unit::Millimeters,
            resolution: Some(300.0),
        }
    );
    // It went to the print sizes; the pixel list is as it was.
    assert_eq!(names(&ui, xuan::canvas_presets::Kind::Pixel).len(), 12);
    let saved = std::fs::read_to_string(presets_file(&directory)).unwrap();
    assert!(
        saved.contains(
            "name = \"Card\"\nwidth = 100\nheight = 297\nunit = \"mm\"\nresolution = 300\n"
        )
    );
    ui.key(egui::Key::Escape);
    assert_eq!(ui.app().dialog, Some(Dialog::New));
    assert!(ui.has_role(Role::ComboBox, "Card (100 × 297 mm)"));
}

#[test]
fn presets_are_deleted_renamed_moved_and_restored() {
    let (directory, mut ui) = new_canvas([1500, 500]);
    let path = presets_file(&directory);
    let pixel = xuan::canvas_presets::Kind::Pixel;
    // Delete in the menu removes the preset shown.
    preset_command(&mut ui, "Banner (1500 × 500)", "Delete preset");
    assert!(ui.has_role(Role::ComboBox, "Custom"));
    assert!(!names(&ui, pixel).contains(&"Banner".to_owned()));
    assert!(!std::fs::read_to_string(&path).unwrap().contains("Banner"));
    // Delete is offered only for a preset.
    ui.harness
        .get_by_role_and_label(Role::ComboBox, "Custom")
        .click();
    ui.settle();
    assert!(!ui.enabled("Delete preset"));
    ui.click("Manage presets…");
    assert_eq!(ui.app().dialog, Some(Dialog::CanvasPresets));
    assert!(!ui.enabled("Delete") && !ui.enabled("Rename"));
    ui.click("Square post (1080 × 1080)");
    // The first of its group moves only down.
    assert!(!ui.enabled("Move up"));
    ui.click("Move down");
    assert_eq!(names(&ui, pixel)[5..7], ["Portrait post", "Square post"]);
    assert!(ui.enabled("Move up"));
    fill(&mut ui, "Name", "Square");
    fill(&mut ui, "Group", "Mine");
    ui.click("Rename");
    assert_eq!(names(&ui, pixel).last().unwrap(), "Square");
    assert!(ui.has("Square (1080 × 1080)"));
    // A name another preset has is refused.
    fill(&mut ui, "Name", "Quad HD");
    assert!(ui.has("Saving replaces the preset of this name."));
    ui.click("Rename");
    let error = ui.app().canvas_presets.edit.as_ref().unwrap().error.clone();
    assert_eq!(error.as_deref(), Some("Another preset has this name"));
    assert_eq!(names(&ui, pixel).last().unwrap(), "Square");
    // The print sizes are their own list.
    ui.click("Print sizes");
    ui.click("A4 (210 × 297 mm)");
    ui.click("Delete");
    let physical = xuan::canvas_presets::Kind::Physical;
    assert_eq!(names(&ui, physical).len(), 15);
    let saved = xuan::canvas_presets::Presets::read(&path)
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(saved, ui.app().canvas_presets.list);
    // Restore Defaults deletes the file.
    ui.click("Restore Defaults");
    assert!(!path.exists());
    assert_eq!(
        ui.app().canvas_presets.list,
        xuan::canvas_presets::Presets::default()
    );
    // Open presets file writes it to show it.
    ui.click("Open presets file");
    assert!(path.exists());
    ui.click("Done");
    assert!(ui.has_role(Role::ComboBox, "Banner (1500 × 500)"));
}

#[test]
fn edits_never_overwrite_a_broken_presets_file() {
    let (directory, mut ui) = new_canvas([1920, 1080]);
    let path = presets_file(&directory);
    std::fs::write(&path, "version = 99\n").unwrap();
    ui.key(egui::Key::Escape);
    ui.app_mut().command("new");
    ui.settle();
    assert!(ui.app().canvas_presets.error.is_some());
    // Deleting from the menu is refused and reported.
    preset_command(
        &mut ui,
        "Full High Definition (1920 × 1080)",
        "Delete preset",
    );
    assert!(ui.has_role(Role::ComboBox, "Full High Definition (1920 × 1080)"));
    let error = ui.app().canvas_presets.error.clone().unwrap();
    assert!(
        error.contains("Unsupported canvas presets version 99"),
        "{error}"
    );
    // So is saving.
    preset_command(
        &mut ui,
        "Full High Definition (1920 × 1080)",
        "Save current size as preset…",
    );
    ui.click("Save current size");
    let error = ui.app().canvas_presets.edit.as_ref().unwrap().error.clone();
    assert!(error.unwrap().contains("Cannot read"));
    assert_eq!(names(&ui, xuan::canvas_presets::Kind::Pixel).len(), 12);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "version = 99\n");
}

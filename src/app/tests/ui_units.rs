//! Physical units in File → New, Canvas Size and Image Size (issue 94), driven through the
//! dialogs' fields and menus.

use super::*;

/// An editor whose preferences live in a temporary folder, with a `size` document at `ppi`
/// when `size` is given.
fn editor(size: Option<[u32; 2]>, ppi: f32) -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    if let Some(size) = size {
        ui.app_mut().dimensions = size;
        ui.app_mut().new_document();
        ui.app_mut().session_mut().unwrap().document.resolution = ppi;
    }
    ui.settle();
    (directory, ui)
}

/// The open size dialog's window.
fn dialog_rect(ui: &UiTest) -> egui::Rect {
    let title = match ui.app().dialog {
        Some(Dialog::New) => "New canvas",
        Some(Dialog::CanvasSize) => "Canvas size",
        Some(Dialog::ImageSize) => "Image size",
        other => panic!("no size dialog: {other:?}"),
    };
    ui.ctx()
        .memory(|memory| memory.area_rect(egui::Id::new(title)))
        .unwrap_or_else(|| panic!("no {title} window"))
}

/// The dialog's number fields, top to bottom and left to right: Width, Height, Resolution.
fn fields(ui: &UiTest) -> Vec<egui_kittest::Node<'_>> {
    let dialog = dialog_rect(ui);
    let mut fields: Vec<_> = (ui.harness.query_all_by_role(Role::SpinButton))
        .chain(ui.harness.query_all_by_role(Role::TextInput))
        // The empty-state screen's taller fields can sit behind File → New.
        .filter(|node| dialog.contains_rect(node.rect()) && node.rect().height() < 30.0)
        .collect();
    fields.sort_by(|a, b| {
        (a.rect().top().round(), a.rect().left())
            .partial_cmp(&(b.rect().top().round(), b.rect().left()))
            .unwrap()
    });
    fields
}

/// Focuses the dialog's `index`th number field, as a click would, so typing replaces its text.
fn focus_field(ui: &mut UiTest, index: usize) {
    fields(ui)[index].focus();
    ui.settle();
}

/// Types `text` into the `index`th field and leaves it, as Tab would.
fn enter(ui: &mut UiTest, index: usize, text: &str) {
    focus_field(ui, index);
    ui.type_keys(text);
    ui.ctx().memory_mut(|memory| {
        if let Some(focused) = memory.focused() {
            memory.surrender_focus(focused);
        }
    });
    ui.settle();
}

/// Picks `choice` from the combo box now showing `current`.
fn choose(ui: &mut UiTest, current: &str, choice: &str) {
    ui.harness
        .get_by_role_and_label(Role::ComboBox, current)
        .click();
    ui.settle();
    ui.click(choice);
}

fn commit(ui: &mut UiTest, label: &str) {
    ui.harness
        .query_all_by_role_and_label(Role::Button, label)
        .last()
        .unwrap()
        .click();
    ui.settle();
}

fn document(ui: &UiTest) -> (u32, u32, f32) {
    let d = &ui.app().session().unwrap().document;
    (d.width, d.height, d.resolution)
}

#[test]
fn new_canvas_in_millimetres_at_300_ppi_makes_a4() {
    let (directory, mut ui) = editor(None, 72.0);
    ui.app_mut().dimensions = [1920, 1080];
    ui.app_mut().command("new");
    ui.settle();
    choose(&mut ui, "Pixels", "Millimetres");
    // Switching the unit leaves the pixels alone.
    assert_eq!(ui.app().dimensions, [1920, 1080]);
    enter(&mut ui, 2, "300");
    assert_eq!(ui.app().resolution, 300.0);
    enter(&mut ui, 0, "210");
    enter(&mut ui, 1, "297");
    assert_eq!(ui.app().dimensions, [2480, 3508]);
    assert!(ui.has("2480 × 3508 px · 8.7 MP"));
    commit(&mut ui, "Create canvas");
    assert_eq!(document(&ui), (2480, 3508, 300.0));
    // The unit and resolution are offered next time.
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    assert_eq!(saved.units.size, xuan::units::Unit::Millimeters);
    assert_eq!(saved.units.new_canvas_resolution, Some(300.0));
    ui.app_mut().resolution = 72.0;
    ui.app_mut().command("new");
    ui.settle();
    assert_eq!(ui.app().resolution, 300.0);
    assert!(ui.has_role(Role::ComboBox, "Millimetres"));
}

#[test]
fn new_canvas_keeps_the_print_size_when_the_resolution_changes() {
    let (_directory, mut ui) = editor(None, 72.0);
    ui.app_mut().dimensions = [720, 360];
    ui.app_mut().command("new");
    ui.settle();
    choose(&mut ui, "Pixels", "Inches");
    // 10 × 5 in at 72 ppi.
    enter(&mut ui, 2, "144");
    assert_eq!(ui.app().dimensions, [1440, 720]);
    // In pixels, a new resolution only relabels the canvas.
    choose(&mut ui, "Inches", "Pixels");
    enter(&mut ui, 2, "300");
    assert_eq!(ui.app().dimensions, [1440, 720]);
    // Pixels per centimetre shows the same resolution another way.
    choose(&mut ui, "Pixels/inch", "Pixels/cm");
    assert_eq!(ui.app().resolution, 300.0);
    enter(&mut ui, 2, "100");
    assert!((ui.app().resolution - 254.0).abs() < 1e-3);
}

#[test]
fn typed_units_convert_and_bad_values_are_refused() {
    let (_directory, mut ui) = editor(None, 72.0);
    ui.app_mut().dimensions = [800, 600];
    ui.app_mut().command("new");
    ui.settle();
    enter(&mut ui, 2, "300");
    // Pixels are shown; a size typed in centimetres becomes pixels at 300 ppi.
    enter(&mut ui, 0, "10cm");
    assert_eq!(ui.app().dimensions, [1181, 600]);
    enter(&mut ui, 1, "1 in");
    assert_eq!(ui.app().dimensions, [1181, 300]);
    // Zero, negative and non-numeric sizes and resolutions leave the values as they were.
    for text in ["0", "-5", "abc", "10 furlongs"] {
        enter(&mut ui, 0, text);
        assert_eq!(ui.app().dimensions, [1181, 300], "{text}");
        enter(&mut ui, 2, text);
        assert_eq!(ui.app().resolution, 300.0, "{text}");
    }
    // The dialog is still usable.
    let create = ui
        .harness
        .query_all_by_role_and_label(Role::Button, "Create canvas")
        .last()
        .unwrap();
    assert!(!create.accesskit_node().is_disabled());
    enter(&mut ui, 0, "640");
    commit(&mut ui, "Create canvas");
    assert_eq!(document(&ui), (640, 300, 300.0));
}

#[test]
fn image_size_without_resample_changes_only_the_print_size() {
    let (directory, mut ui) = editor(Some([3000, 20]), 300.0);
    ui.app_mut().command("image_size");
    ui.settle();
    choose(&mut ui, "Pixels", "Inches");
    ui.click("Resample");
    assert!(!ui.app().size_units.resample);
    enter(&mut ui, 2, "150");
    // 3000 px at 150 ppi print 20 in wide; the pixels stay.
    assert_eq!(ui.app().dimensions, [3000, 20]);
    let shown = ui.app().size_units.shown(3000, 0, ui.app().resolution);
    assert_eq!(shown, 20.0);
    // Typing a print width sets the resolution instead: 3000 px over 15 in is 200 ppi.
    enter(&mut ui, 0, "15");
    assert_eq!(ui.app().resolution, 200.0);
    assert_eq!(ui.app().dimensions, [3000, 20]);
    enter(&mut ui, 2, "150");
    commit(&mut ui, "Resize");
    assert_eq!(document(&ui), (3000, 20, 150.0));
    // One undo step puts the old resolution back.
    ui.app_mut().command("undo");
    assert_eq!(document(&ui), (3000, 20, 300.0));
    ui.app_mut().command("redo");
    // Exported PNG and JPEG carry the new resolution.
    let document = ui.app().session().unwrap().document.clone();
    for extension in ["png", "jpg"] {
        let path = directory.path().join(format!("print.{extension}"));
        io::export(&document, &path, &io::ExportOptions::default()).unwrap();
        let (_, resolution) = io::import_image_with_resolution(&path).unwrap();
        assert_eq!(resolution, Some(150.0), "{extension}");
    }
}

#[test]
fn image_size_without_resample_needs_a_print_unit() {
    let (_directory, mut ui) = editor(Some([300, 20]), 72.0);
    ui.app_mut().command("image_size");
    ui.settle();
    ui.app_mut().dimensions = [200, 20];
    ui.settle();
    ui.click("Resample");
    // Turning Resample off puts the document's pixels back, and pixel fields are disabled.
    assert_eq!(ui.app().dimensions, [300, 20]);
    let disabled = fields(&ui)
        .iter()
        .filter(|node| node.accesskit_node().is_disabled())
        .count();
    assert_eq!(disabled, 2);
    assert!(!ui.has("Keep aspect ratio"));
}

#[test]
fn image_size_with_resample_sets_pixels_from_a_print_size() {
    let (_directory, mut ui) = editor(Some([600, 20]), 300.0);
    ui.app_mut().command("image_size");
    ui.settle();
    choose(&mut ui, "Pixels", "Inches");
    enter(&mut ui, 0, "10");
    assert_eq!(ui.app().dimensions, [3000, 20]);
    assert!(ui.has("3000 × 20 px · 0.1 MP · 1 MiB"));
    commit(&mut ui, "Resize");
    assert_eq!(document(&ui), (3000, 20, 300.0));
}

#[test]
fn canvas_size_adds_a_relative_percentage() {
    let (_directory, mut ui) = editor(Some([20, 16]), 72.0);
    ui.app_mut().command("canvas_size");
    ui.settle();
    choose(&mut ui, "Pixels", "Percent");
    assert!(ui.has_role(Role::ComboBox, "Percent"));
    ui.click("Relative");
    enter(&mut ui, 0, "50");
    enter(&mut ui, 1, "-25");
    assert_eq!(ui.app().dimensions, [30, 12]);
    // A size typed in another unit is relative too.
    enter(&mut ui, 0, "6px");
    assert_eq!(ui.app().dimensions, [26, 12]);
    commit(&mut ui, "Resize");
    assert_eq!(document(&ui), (26, 12, 72.0));
    // Percent is not offered by File → New, which shows pixels instead.
    ui.app_mut().command("new");
    ui.settle();
    assert!(ui.has_role(Role::ComboBox, "Pixels"));
}

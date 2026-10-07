//! Image → Rotate Canvas, Crop to Selection and Trim….
use super::*;

fn size(app: &EditorApp) -> (u32, u32) {
    let d = &app.session().unwrap().document;
    (d.width, d.height)
}

fn enabled(app: &EditorApp, id: &str) -> bool {
    (commands::find(id).unwrap().enabled)(app)
}

fn revision(app: &EditorApp) -> u64 {
    app.session().unwrap().history.revision as u64
}

#[test]
fn rotate_commands_are_one_undo_step_each() {
    let (_, mut app) = app();
    app.dimensions = [6, 4];
    app.new_document();
    for id in ["rotate_canvas_cw", "rotate_canvas_ccw", "rotate_canvas_180"] {
        assert!(enabled(&app, id), "{id}");
        let before = revision(&app);
        let was = size(&app);
        app.command(id);
        assert_eq!(revision(&app), before + 1, "{id}");
        let expected = if id == "rotate_canvas_180" {
            was
        } else {
            (was.1, was.0)
        };
        assert_eq!(size(&app), expected, "{id}");
        app.command("undo");
        assert_eq!(size(&app), was, "{id}");
    }
}

#[test]
fn crop_to_selection_needs_a_selection_and_is_one_undo_step() {
    let (_, mut app) = app();
    app.dimensions = [8, 6];
    app.new_document();
    assert!(!enabled(&app, "crop_to_selection"));
    app.edit_selection("Rect", |doc| {
        let mut mask = GrayImage::new(8, 6);
        for y in 1..3 {
            for x in 2..5 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        doc.selection = Some(Arc::new(mask));
    });
    assert!(enabled(&app, "crop_to_selection"));
    let before = revision(&app);
    app.command("crop_to_selection");
    assert_eq!(size(&app), (3, 2));
    assert_eq!(revision(&app), before + 1);
    app.command("undo");
    assert_eq!(size(&app), (8, 6));
}

#[test]
fn trim_dialog_trims_in_one_undo_step() {
    let (context, mut app) = app();
    app.dimensions = [7, 5];
    app.new_document();
    app.edit("Paint", |doc| {
        let mut pixels = image::RgbaImage::new(7, 5);
        pixels.put_pixel(3, 2, image::Rgba([255, 0, 0, 255]));
        doc.layers[0].pixels = Some(Arc::new(pixels));
        Ok(())
    });
    app.command("trim");
    assert_eq!(app.dialog, Some(Dialog::Trim));
    context.begin_pass(egui::RawInput::default());
    app.trim_dialog(&context);
    let _ = context.end_pass();
    let before = revision(&app);
    app.dialog = None;
    app.trim_canvas(app.trim_settings);
    assert_eq!(size(&app), (1, 1));
    assert_eq!(revision(&app), before + 1);
    // Nothing more to trim: no new step.
    app.trim_canvas(app.trim_settings);
    assert_eq!(revision(&app), before + 1);
    app.command("undo");
    assert_eq!(size(&app), (7, 5));
}

//! Edit → Stroke…: when it is available, its live preview, Apply as one undo step and Cancel.
use super::*;
use xuan::selection_ops::StrokeLocation;

fn enabled(app: &EditorApp, id: &str) -> bool {
    (commands::find(id).unwrap().enabled)(app)
}

fn revision(app: &EditorApp) -> u64 {
    app.session().unwrap().history.revision
}

fn layer_pixels(app: &EditorApp) -> RgbaImage {
    app.session()
        .unwrap()
        .document
        .active()
        .unwrap()
        .pixels
        .as_deref()
        .cloned()
        .unwrap_or_default()
}

/// A 20 × 16 document with a transparent pixel layer and x 6..14, y 4..12 selected.
fn selected() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [20, 16];
    app.new_document();
    app.edit("Pixels", |doc| {
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::new(20, 16)));
        Ok(())
    });
    app.edit_selection("Rect", |doc| {
        let mask = GrayImage::from_fn(20, 16, |x, y| {
            image::Luma([if (6..14).contains(&x) && (4..12).contains(&y) {
                255
            } else {
                0
            }])
        });
        doc.selection = Some(Arc::new(mask));
    });
    (context, app)
}

fn pass(context: &egui::Context, app: &mut EditorApp, events: Vec<egui::Event>) {
    context.begin_pass(egui::RawInput {
        events,
        ..Default::default()
    });
    app.stroke_dialog(context);
    let _ = context.end_pass();
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn stroke_needs_a_selection_and_a_pixel_layer() {
    let (_, mut app) = app();
    assert!(!enabled(&app, "stroke"));
    app.dimensions = [8, 8];
    app.new_document();
    // No selection.
    assert!(!enabled(&app, "stroke"));
    app.command("stroke");
    assert_eq!(app.dialog, None);
    app.command("select_all");
    assert!(enabled(&app, "stroke"));
    // Not on a locked layer, a text layer, a folder or while the mask is edited.
    app.session_mut().unwrap().document.layers[0].locked = true;
    assert!(!enabled(&app, "stroke"));
    app.session_mut().unwrap().document.layers[0].locked = false;
    app.session_mut().unwrap().document.layers[0].text = Some(Default::default());
    assert!(!enabled(&app, "stroke"));
    app.session_mut().unwrap().document.layers[0].text = None;
    app.session_mut().unwrap().document.layers[0].group = true;
    assert!(!enabled(&app, "stroke"));
    app.session_mut().unwrap().document.layers[0].group = false;
    app.session_mut().unwrap().document.layers[0].mask = Some(xuan::document::Mask::white());
    app.mask_target = true;
    assert!(app.editing_mask());
    assert!(!enabled(&app, "stroke"));
    app.mask_target = false;
    assert!(enabled(&app, "stroke"));
    // No layer at all.
    app.session_mut().unwrap().document.active = None;
    assert!(!enabled(&app, "stroke"));
}

/// Runs the dialog until the canvas shows its settings.
fn settle(context: &egui::Context, app: &mut EditorApp) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while app.stroke.as_ref().is_some_and(|edit| edit.busy()) {
        assert!(
            std::time::Instant::now() < deadline,
            "the preview did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
        pass(context, app, Vec::new());
    }
}

#[test]
fn stroke_previews_live_and_applies_as_one_undo_step() {
    let (context, mut app) = selected();
    app.brush.color = [10, 200, 30, 255];
    let before = layer_pixels(&app);
    let start = revision(&app);
    app.command("stroke");
    assert_eq!(app.dialog, Some(Dialog::Stroke));
    // The remembered settings and the foreground colour.
    let edit = app.stroke.as_ref().unwrap();
    assert_eq!(edit.outline.color, [10, 200, 30]);
    assert_eq!(edit.outline.width, 4);
    assert_eq!(edit.outline.location, StrokeLocation::Outside);
    // The line shows before anything is applied.
    settle(&context, &mut app);
    assert_eq!(app.dialog, Some(Dialog::Stroke));
    assert_eq!(layer_pixels(&app).get_pixel(5, 8).0, [10, 200, 30, 255]);
    assert_eq!(layer_pixels(&app).get_pixel(2, 8).0, [10, 200, 30, 255]);
    assert_eq!(layer_pixels(&app).get_pixel(1, 8)[3], 0);
    assert_eq!(revision(&app), start);

    // A new width and location make a new line, painted from the original pixels.
    let edit = app.stroke.as_mut().unwrap();
    edit.outline.width = 2;
    edit.outline.location = StrokeLocation::Inside;
    assert!(edit.busy());
    settle(&context, &mut app);
    let shown = layer_pixels(&app);
    assert_eq!(shown.get_pixel(2, 8)[3], 0);
    assert_eq!(shown.get_pixel(5, 8)[3], 0);
    assert_eq!(shown.get_pixel(6, 8).0, [10, 200, 30, 255]);
    assert_eq!(shown.get_pixel(7, 8).0, [10, 200, 30, 255]);
    assert_eq!(shown.get_pixel(8, 8)[3], 0);
    // A new colour paints the same line again at once.
    app.stroke.as_mut().unwrap().outline.color = [0, 0, 255];
    pass(&context, &mut app, Vec::new());
    assert!(!app.stroke.as_ref().unwrap().busy());
    assert_eq!(layer_pixels(&app).get_pixel(6, 8).0, [0, 0, 255, 255]);
    app.stroke.as_mut().unwrap().outline.color = [10, 200, 30];
    pass(&context, &mut app, Vec::new());
    assert_eq!(layer_pixels(&app), shown);

    // Enter applies: one undo step, and the settings are remembered.
    pass(&context, &mut app, vec![key(egui::Key::Enter)]);
    assert_eq!(app.dialog, None);
    assert!(app.stroke.is_none());
    assert_eq!(revision(&app), start + 1);
    assert_eq!(layer_pixels(&app), shown);
    assert_eq!(app.stroke_settings.width, 2);
    assert_eq!(app.stroke_settings.location, StrokeLocation::Inside);
    assert_eq!(
        app.session().unwrap().history.undo_name(),
        Some(xuan::i18n::tr("Stroke"))
    );
    app.command("undo");
    assert_eq!(layer_pixels(&app), before);

    // Reopening starts from the remembered settings.
    app.command("stroke");
    let edit = app.stroke.as_ref().unwrap();
    assert_eq!(
        (edit.outline.width, edit.outline.location),
        (2, StrokeLocation::Inside)
    );
}

#[test]
fn apply_waits_for_the_line_for_the_latest_settings() {
    let (context, mut app) = selected();
    let start = revision(&app);
    app.command("stroke");
    // Apply straight after a change: the dialog stays until the new line is painted, then
    // keeps it.
    app.stroke.as_mut().unwrap().outline.width = 1;
    pass(&context, &mut app, vec![key(egui::Key::Enter)]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while app.dialog.is_some() {
        assert!(std::time::Instant::now() < deadline, "Apply did not finish");
        std::thread::sleep(std::time::Duration::from_millis(1));
        pass(&context, &mut app, Vec::new());
    }
    assert_eq!(revision(&app), start + 1);
    let pixels = layer_pixels(&app);
    assert_eq!(pixels.get_pixel(5, 8)[3], 255);
    assert_eq!(pixels.get_pixel(4, 8)[3], 0);
    assert_eq!(app.stroke_settings.width, 1);
}

#[test]
fn cancelling_the_stroke_restores_the_layer() {
    let (context, mut app) = selected();
    let before = layer_pixels(&app);
    let start = revision(&app);
    let undo = app
        .session()
        .unwrap()
        .history
        .undo_name()
        .map(str::to_owned);
    app.command("stroke");
    settle(&context, &mut app);
    assert_ne!(layer_pixels(&app), before);
    pass(&context, &mut app, vec![key(egui::Key::Escape)]);
    assert_eq!(app.dialog, None);
    assert_eq!(layer_pixels(&app), before);
    assert_eq!(revision(&app), start);
    assert_eq!(
        app.session()
            .unwrap()
            .history
            .undo_name()
            .map(str::to_owned),
        undo
    );
    // Cancelled settings are not remembered, even with a line still being made.
    app.command("stroke");
    app.stroke.as_mut().unwrap().outline.width = 9;
    pass(&context, &mut app, vec![key(egui::Key::Escape)]);
    assert_eq!(app.stroke_settings.width, 4);
    assert_eq!(layer_pixels(&app), before);
}

#[test]
fn a_stroke_the_layer_cannot_grow_to_hold_reports_an_error_and_restores() {
    let (context, mut app) = selected();
    // One layer pixel squeezed into a ten-thousandth of a canvas pixel: holding the line
    // would take a layer far wider than the size limit.
    let layer = &mut app.session_mut().unwrap().document.layers[0];
    layer.pixels = Some(Arc::new(RgbaImage::new(1, 1)));
    layer.transform = xuan::document::Transform {
        x: 10.0,
        y: 8.0,
        width: 0.0001,
        height: 1.0,
        ..xuan::document::Transform::new(1, 1)
    };
    let document = app.session().unwrap().document.clone();
    let start = revision(&app);
    app.command("stroke");
    settle(&context, &mut app);
    assert!(app.error.is_some());
    assert_eq!(app.dialog, None);
    assert!(app.stroke.is_none());
    assert_eq!(revision(&app), start);
    assert!(app.session().unwrap().history.undo_name() != Some(xuan::i18n::tr("Stroke")));
    let layer = &app.session().unwrap().document.layers[0];
    assert_eq!(layer.pixels, document.layers[0].pixels);
    assert_eq!(layer.transform, document.layers[0].transform);
}

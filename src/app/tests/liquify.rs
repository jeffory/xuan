//! Liquify, the Blur / Smudge tool's forward warp: a stroke pushes pixels as
//! one undo step, a stroke that moves nothing adds none, locked layers and
//! masks are refused, and Shift+R cycles Blur, Smudge and Liquify.
use super::*;

/// A 60 × 40 layer, black left of x = 30 and white from it, with the Blur /
/// Smudge tool in Liquify mode.
fn liquify_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [60, 40];
    app.new_document();
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().pixels =
        Some(Arc::new(RgbaImage::from_fn(60, 40, |x, _| {
            if x < 30 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        })));
    session.history = History::default();
    app.set_tool(Tool::Blur);
    app.blur_mode = PaintMode::Liquify;
    app.brush.diameter = 24.0;
    app.brush.hardness = 0.5;
    app.brush.opacity = 1.0;
    (context, app)
}

fn layer_pixels(app: &EditorApp) -> RgbaImage {
    let layer = app.session().unwrap().document.active().unwrap();
    (**layer.pixels.as_ref().unwrap()).clone()
}

fn steps(app: &EditorApp) -> usize {
    app.session().unwrap().history.names().count()
}

/// Pushes the edge to the right along the middle row.
fn push(context: &egui::Context, app: &mut EditorApp) {
    drag(
        context,
        app,
        Point::new(24.0, 20.0),
        Point::new(36.0, 20.0),
        egui::Modifiers::NONE,
    );
}

#[test]
fn a_push_moves_the_edge_as_one_undo_step_that_undoes_exactly() {
    let (context, mut app) = liquify_app();
    let before = layer_pixels(&app);
    push(&context, &mut app);
    assert!(app.error.is_none(), "{:?}", app.error);
    let after = layer_pixels(&app);
    // Black now reaches past the old edge on the stroke's row, not far above it.
    assert!(
        after.get_pixel(33, 20)[0] < 60,
        "{:?}",
        after.get_pixel(33, 20)
    );
    assert_eq!(after.get_pixel(33, 2).0, [255; 4]);
    assert!(after.pixels().all(|p| p[3] == 255));
    assert_eq!(steps(&app), 1);
    assert_eq!(app.session().unwrap().history.undo_name(), Some("Liquify"));
    app.command("undo");
    assert_eq!(layer_pixels(&app), before);
    app.command("redo");
    assert_eq!(layer_pixels(&app), after);
}

#[test]
fn a_stroke_that_moves_nothing_adds_no_undo_step() {
    let (context, mut app) = liquify_app();
    let before = layer_pixels(&app);
    // A push across a flat colour, where nothing visibly moves.
    drag(
        &context,
        &mut app,
        Point::new(5.0, 20.0),
        Point::new(12.0, 20.0),
        egui::Modifiers::NONE,
    );
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(layer_pixels(&app), before);
    assert_eq!(steps(&app), 0);
    assert!(!app.session().unwrap().history.dirty());
}

#[test]
fn the_selection_limits_the_push() {
    let (context, mut app) = liquify_app();
    let before = layer_pixels(&app);
    app.session_mut().unwrap().document.selection =
        Some(Arc::new(GrayImage::from_fn(60, 40, |_, y| {
            image::Luma([if y < 20 { 255 } else { 0 }])
        })));
    push(&context, &mut app);
    let after = layer_pixels(&app);
    assert!(after.get_pixel(33, 18)[0] < 60);
    for y in 20..40 {
        for x in 0..60 {
            assert_eq!(after.get_pixel(x, y), before.get_pixel(x, y), "{x},{y}");
        }
    }
}

#[test]
fn locked_layers_and_masks_are_refused_with_a_message() {
    let (context, mut app) = liquify_app();
    let before = layer_pixels(&app);
    app.session_mut()
        .unwrap()
        .document
        .active_mut()
        .unwrap()
        .locked = true;
    push(&context, &mut app);
    assert!(
        app.error.as_deref().is_some_and(|e| e.contains("unlocked")),
        "{:?}",
        app.error
    );
    assert_eq!(layer_pixels(&app), before);
    assert_eq!(steps(&app), 0);

    let (context, mut app) = liquify_app();
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_pixel(60, 40, image::Luma([90]))),
        ..Mask::white()
    });
    app.mask_target = true;
    push(&context, &mut app);
    assert!(app.status.contains("not masks"), "{}", app.status);
    let layer = app.session().unwrap().document.active().unwrap();
    assert!(
        layer
            .mask
            .as_ref()
            .unwrap()
            .pixels
            .pixels()
            .all(|p| p[0] == 90)
    );
    assert_eq!(layer_pixels(&app), before);
    assert_eq!(steps(&app), 0);
}

#[test]
fn r_and_shift_r_cycle_blur_smudge_and_liquify() {
    let (context, mut app) = app();
    app.dimensions = [16, 16];
    app.new_document();
    app.set_tool(Tool::Zoom);
    let press = |app: &mut EditorApp, modifiers: egui::Modifiers| {
        let events = vec![text_key(egui::Key::R, modifiers)];
        keyboard_frame(&context, app, events, modifiers);
    };
    press(&mut app, egui::Modifiers::NONE);
    assert_eq!((app.tool, app.blur_mode), (Tool::Blur, PaintMode::Blur));
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.blur_mode, PaintMode::Smudge);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.blur_mode, PaintMode::Liquify);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.blur_mode, PaintMode::Blur);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.blur_mode, PaintMode::Smudge);
    // From another tool, R and Shift+R come back to the mode used last.
    app.set_tool(Tool::Brush);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!((app.tool, app.blur_mode), (Tool::Blur, PaintMode::Smudge));
    app.set_tool(Tool::Zoom);
    press(&mut app, egui::Modifiers::NONE);
    assert_eq!((app.tool, app.blur_mode), (Tool::Blur, PaintMode::Smudge));
}

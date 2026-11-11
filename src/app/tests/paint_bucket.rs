//! The Paint Bucket tool: clicks, undo steps and the G / Shift+G cycle with the Gradient.
use super::*;

const WHITE: image::Rgba<u8> = image::Rgba([255; 4]);
const BLACK: image::Rgba<u8> = image::Rgba([0, 0, 0, 255]);
const RED: [u8; 4] = [255, 0, 0, 255];

/// A 40 × 30 document whose layer is white with a black column at x = 20.
fn bucket_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [40, 30];
    app.new_document();
    let pixels = RgbaImage::from_fn(40, 30, |x, _| if x == 20 { BLACK } else { WHITE });
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().pixels = Some(Arc::new(pixels));
    session.history = History::default();
    app.set_tool(Tool::Bucket);
    app.brush.color = RED;
    app.brush.opacity = 1.0;
    (context, app)
}

fn layer_pixel(app: &EditorApp, x: u32, y: u32) -> [u8; 4] {
    let layer = app.session().unwrap().document.active().unwrap();
    layer.pixels.as_ref().unwrap().get_pixel(x, y).0
}

fn steps(app: &EditorApp) -> usize {
    app.session().unwrap().history.names().count()
}

#[test]
fn clicking_fills_the_area_as_one_undo_step() {
    let (context, mut app) = bucket_app();
    click_canvas(
        &context,
        &mut app,
        Point::new(5.5, 5.5),
        egui::Modifiers::NONE,
    );
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(layer_pixel(&app, 0, 0), RED);
    assert_eq!(layer_pixel(&app, 19, 29), RED);
    assert_eq!(layer_pixel(&app, 20, 0), BLACK.0);
    assert_eq!(layer_pixel(&app, 21, 0), WHITE.0);
    assert_eq!(steps(&app), 1);
    assert_eq!(
        app.session().unwrap().history.undo_name(),
        Some("Paint Bucket")
    );

    app.command("undo");
    assert_eq!(layer_pixel(&app, 0, 0), WHITE.0);
    assert_eq!(steps(&app), 0);
}

#[test]
fn options_reach_the_fill() {
    let (context, mut app) = bucket_app();
    app.bucket.contiguous = false;
    app.brush.opacity = 0.5;
    click_canvas(
        &context,
        &mut app,
        Point::new(5.5, 5.5),
        egui::Modifiers::NONE,
    );
    // Both sides of the line, half covered.
    assert_eq!(layer_pixel(&app, 0, 0), [255, 128, 128, 255]);
    assert_eq!(layer_pixel(&app, 39, 0), [255, 128, 128, 255]);
    assert_eq!(steps(&app), 1);
}

#[test]
fn locked_layers_and_misses_leave_no_undo_step() {
    let (context, mut app) = bucket_app();
    app.session_mut()
        .unwrap()
        .document
        .active_mut()
        .unwrap()
        .locked = true;
    click_canvas(
        &context,
        &mut app,
        Point::new(5.5, 5.5),
        egui::Modifiers::NONE,
    );
    assert!(app.error.take().is_some());
    assert_eq!(layer_pixel(&app, 0, 0), WHITE.0);
    assert_eq!(steps(&app), 0);

    // A selection elsewhere leaves nothing to fill.
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().locked = false;
    let mut selection = GrayImage::new(40, 30);
    selection.put_pixel(30, 10, image::Luma([255]));
    session.document.selection = Some(Arc::new(selection));
    click_canvas(
        &context,
        &mut app,
        Point::new(5.5, 5.5),
        egui::Modifiers::NONE,
    );
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(layer_pixel(&app, 0, 0), WHITE.0);
    assert_eq!(steps(&app), 0);
    assert!(!app.session().unwrap().history.dirty());
}

#[test]
fn dragging_does_not_fill_or_add_a_step() {
    let (context, mut app) = bucket_app();
    drag(
        &context,
        &mut app,
        Point::new(5.0, 5.0),
        Point::new(15.0, 15.0),
        egui::Modifiers::NONE,
    );
    assert_eq!(layer_pixel(&app, 0, 0), WHITE.0);
    assert_eq!(steps(&app), 0);
}

#[test]
fn with_the_mask_targeted_the_click_paints_the_mask() {
    let (context, mut app) = bucket_app();
    let session = app.session_mut().unwrap();
    let layer = session.document.active_mut().unwrap();
    layer.mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_pixel(40, 30, image::Luma([255]))),
        ..Mask::white()
    });
    app.mask_target = true;
    app.brush.color = [0, 0, 0, 255];
    click_canvas(
        &context,
        &mut app,
        Point::new(5.5, 5.5),
        egui::Modifiers::NONE,
    );
    let layer = app.session().unwrap().document.active().unwrap();
    // The mask is all white, so it all fills; the pixels stay as they were.
    assert!(
        layer
            .mask
            .as_ref()
            .unwrap()
            .pixels
            .pixels()
            .all(|p| p[0] == 0)
    );
    assert_eq!(layer_pixel(&app, 0, 0), WHITE.0);
    assert_eq!(steps(&app), 1);
}

#[test]
fn g_and_shift_g_cycle_gradient_and_paint_bucket() {
    let (context, mut app) = app();
    app.dimensions = [16, 16];
    app.new_document();
    app.set_tool(Tool::Zoom);
    let press = |app: &mut EditorApp, modifiers: egui::Modifiers| {
        let events = vec![text_key(egui::Key::G, modifiers)];
        keyboard_frame(&context, app, events, modifiers);
    };
    press(&mut app, egui::Modifiers::NONE);
    assert_eq!(app.tool, Tool::Gradient);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.tool, Tool::Bucket);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.tool, Tool::Gradient);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.tool, Tool::Bucket);
    // G comes back to whichever of the two was used last.
    app.set_tool(Tool::Zoom);
    press(&mut app, egui::Modifiers::NONE);
    assert_eq!(app.tool, Tool::Bucket);
    // From another tool, Shift+G starts at the Gradient.
    app.set_tool(Tool::Brush);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.tool, Tool::Gradient);
    press(&mut app, egui::Modifiers::NONE);
    assert_eq!(app.tool, Tool::Gradient);
}

#[test]
fn number_keys_set_the_bucket_opacity() {
    let (context, mut app) = bucket_app();
    let events = vec![text_key(egui::Key::Num5, egui::Modifiers::NONE)];
    keyboard_frame(&context, &mut app, events, egui::Modifiers::NONE);
    assert!((app.brush.opacity - 0.5).abs() < 1e-6);
    // The layer's own opacity is not changed.
    let layer = app.session().unwrap().document.active().unwrap();
    assert_eq!(layer.opacity, 1.0);
}

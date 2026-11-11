//! The Dodge / Burn tool: strokes as undo steps, the mask refusal, the
//! number keys and the O / Shift+O cycle through Dodge, Burn and Sponge.
use super::*;

const GREY: [u8; 4] = [128, 128, 128, 255];

/// A 60 × 40 mid-grey document with the Dodge / Burn tool at full exposure.
fn tone_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [60, 40];
    app.new_document();
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().pixels =
        Some(Arc::new(RgbaImage::from_pixel(60, 40, image::Rgba(GREY))));
    session.history = History::default();
    app.set_tool(Tool::Dodge);
    app.brush.diameter = 10.0;
    app.brush.hardness = 1.0;
    app.brush.opacity = 1.0;
    app.brush.tone.exposure = 1.0;
    (context, app)
}

fn layer_pixel(app: &EditorApp, x: u32, y: u32) -> [u8; 4] {
    let layer = app.session().unwrap().document.active().unwrap();
    layer.pixels.as_ref().unwrap().get_pixel(x, y).0
}

fn steps(app: &EditorApp) -> usize {
    app.session().unwrap().history.names().count()
}

fn stroke(context: &egui::Context, app: &mut EditorApp) {
    drag(
        context,
        app,
        Point::new(10.0, 20.0),
        Point::new(50.0, 20.0),
        egui::Modifiers::NONE,
    );
}

#[test]
fn a_stroke_dodges_burns_or_sponges_as_one_undo_step() {
    for (mode, lighter) in [(PaintMode::Dodge, true), (PaintMode::Burn, false)] {
        let (context, mut app) = tone_app();
        app.tone_mode = mode;
        stroke(&context, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        let pixel = layer_pixel(&app, 30, 20);
        assert_eq!(pixel[0] > 128, lighter, "{mode:?}: {pixel:?}");
        assert_ne!(pixel[0], 128, "{mode:?}");
        assert_eq!(pixel[3], 255);
        assert_eq!(layer_pixel(&app, 30, 5), GREY);
        assert_eq!(steps(&app), 1);
        assert_eq!(
            app.session().unwrap().history.undo_name(),
            Some("Dodge / Burn")
        );
        app.command("undo");
        assert_eq!(layer_pixel(&app, 30, 20), GREY);
        assert_eq!(steps(&app), 0);
    }
    // Sponge desaturates a colour towards grey.
    let (context, mut app) = tone_app();
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_pixel(
        60,
        40,
        image::Rgba([200, 100, 50, 255]),
    )));
    app.tone_mode = PaintMode::Sponge;
    stroke(&context, &mut app);
    let pixel = layer_pixel(&app, 30, 20);
    assert!(pixel[0] < 200 && pixel[2] > 50, "{pixel:?}");
    assert_eq!(steps(&app), 1);
}

#[test]
fn a_stroke_that_changes_nothing_adds_no_undo_step() {
    let (context, mut app) = tone_app();
    app.brush.tone.exposure = 0.0;
    stroke(&context, &mut app);
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(layer_pixel(&app, 30, 20), GREY);
    assert_eq!(steps(&app), 0);
    assert!(!app.session().unwrap().history.dirty());

    // Over transparent pixels only, nothing changes either.
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::new(60, 40)));
    app.brush.tone.exposure = 1.0;
    stroke(&context, &mut app);
    assert_eq!(steps(&app), 0);
}

#[test]
fn the_mask_target_is_refused_with_a_message() {
    let (context, mut app) = tone_app();
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_pixel(60, 40, image::Luma([90]))),
        ..Mask::white()
    });
    app.mask_target = true;
    stroke(&context, &mut app);
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
    assert_eq!(layer_pixel(&app, 30, 20), GREY);
    assert_eq!(steps(&app), 0);
}

#[test]
fn shift_click_draws_a_straight_line() {
    let (context, mut app) = tone_app();
    app.tone_mode = PaintMode::Burn;
    click_canvas(
        &context,
        &mut app,
        Point::new(10.0, 20.0),
        egui::Modifiers::NONE,
    );
    assert_eq!(layer_pixel(&app, 30, 20), GREY);
    click_canvas(
        &context,
        &mut app,
        Point::new(50.0, 20.0),
        egui::Modifiers::SHIFT,
    );
    assert!(layer_pixel(&app, 30, 20)[0] < 128);
    assert_eq!(steps(&app), 2);
}

#[test]
fn number_keys_set_the_exposure() {
    let (context, mut app) = tone_app();
    let events = vec![text_key(egui::Key::Num3, egui::Modifiers::NONE)];
    keyboard_frame(&context, &mut app, events, egui::Modifiers::NONE);
    assert!((app.brush.tone.exposure - 0.3).abs() < 1e-6);
    // Neither the brush's nor the layer's opacity changes.
    assert_eq!(app.brush.opacity, 1.0);
    let layer = app.session().unwrap().document.active().unwrap();
    assert_eq!(layer.opacity, 1.0);
}

#[test]
fn o_and_shift_o_cycle_dodge_burn_and_sponge() {
    let (context, mut app) = app();
    app.dimensions = [16, 16];
    app.new_document();
    app.set_tool(Tool::Zoom);
    let press = |app: &mut EditorApp, modifiers: egui::Modifiers| {
        let events = vec![text_key(egui::Key::O, modifiers)];
        keyboard_frame(&context, app, events, modifiers);
    };
    // The defaults: Dodge, Midtones, 50%.
    assert_eq!(app.brush.tone, xuan::paint::Tone::default());
    assert_eq!(app.brush.tone.range, xuan::paint::ToneRange::Midtones);
    assert_eq!(app.brush.tone.exposure, 0.5);
    press(&mut app, egui::Modifiers::NONE);
    assert_eq!((app.tool, app.tone_mode), (Tool::Dodge, PaintMode::Dodge));
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.tone_mode, PaintMode::Burn);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.tone_mode, PaintMode::Sponge);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!(app.tone_mode, PaintMode::Dodge);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!((app.tool, app.tone_mode), (Tool::Dodge, PaintMode::Burn));
    // From another tool, O and Shift+O come back to the mode used last.
    app.set_tool(Tool::Brush);
    press(&mut app, egui::Modifiers::SHIFT);
    assert_eq!((app.tool, app.tone_mode), (Tool::Dodge, PaintMode::Burn));
    app.set_tool(Tool::Zoom);
    press(&mut app, egui::Modifiers::NONE);
    assert_eq!((app.tool, app.tone_mode), (Tool::Dodge, PaintMode::Burn));
}

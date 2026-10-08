//! The Crop tool on the canvas: drawing a box in a ratio, adjusting it by its handles, moving it,
//! and applying or discarding it.
use super::*;
use xuan::crop::{CropBox, Ratio};

/// A 120 × 100 document with the Crop tool picked and nothing selected.
fn crop_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [120, 100];
    app.new_document();
    app.set_tool(Tool::Crop);
    frame(&context, &mut app);
    (context, app)
}

fn crop_box(app: &EditorApp) -> CropBox {
    app.crop.rect.expect("a crop box")
}

fn steps(app: &EditorApp) -> usize {
    app.session().unwrap().history.names().count()
}

fn size(app: &EditorApp) -> (u32, u32) {
    let d = &app.session().unwrap().document;
    (d.width, d.height)
}

fn press(context: &egui::Context, app: &mut EditorApp, key: egui::Key) {
    keyboard_frame(
        context,
        app,
        vec![text_key(key, egui::Modifiers::NONE)],
        egui::Modifiers::NONE,
    );
}

/// Both sides the same whole multiple of `a`:`b`.
#[track_caller]
fn assert_exact(rect: CropBox, [a, b]: [u32; 2]) {
    assert!(!rect.is_empty(), "{rect:?}");
    assert_eq!(rect.width % a, 0, "{rect:?} is not {a}:{b}");
    assert_eq!(rect.width / a * b, rect.height, "{rect:?} is not {a}:{b}");
}

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

const NONE: egui::Modifiers = egui::Modifiers::NONE;

#[test]
fn drawing_adjusting_and_moving_keep_nine_by_sixteen_then_enter_crops() {
    let (context, mut app) = crop_app();
    app.crop.ratio = Ratio::Preset([9, 16]);
    // 30 across is 3.3 units of 9, more than 50 down's 3.1 units of 16: three units.
    drag(&context, &mut app, p(10.0, 10.0), p(40.0, 60.0), NONE);
    assert_eq!(crop_box(&app), CropBox::new(10, 10, 27, 48));
    // The bottom-right handle out by one unit.
    drag(&context, &mut app, p(37.0, 58.0), p(46.0, 74.0), NONE);
    assert_eq!(crop_box(&app), CropBox::new(10, 10, 36, 64));
    // The right edge out by one unit: the height follows about the middle (y 42).
    drag(&context, &mut app, p(46.0, 42.0), p(55.0, 42.0), NONE);
    assert_eq!(crop_box(&app), CropBox::new(10, 2, 45, 80));
    assert_exact(crop_box(&app), [9, 16]);
    // Dragging inside moves it, as far as the canvas allows.
    drag(&context, &mut app, p(30.0, 40.0), p(40.0, 70.0), NONE);
    assert_eq!(crop_box(&app), CropBox::new(20, 20, 45, 80));
    // Nothing is an undo step until the crop is applied.
    assert_eq!(steps(&app), 0);
    assert_eq!(size(&app), (120, 100));

    press(&context, &mut app, egui::Key::Enter);
    assert!(app.crop.rect.is_none());
    assert_eq!(size(&app), (45, 80));
    let layer = app.session().unwrap().document.active().unwrap().transform;
    assert_eq!((layer.x, layer.y), (-20.0, -20.0));
    assert_eq!(steps(&app), 1);
    assert_eq!(app.session().unwrap().history.undo_name(), Some("Crop"));
    app.command("undo");
    assert_eq!(size(&app), (120, 100));
}

#[test]
fn every_drag_direction_keeps_the_ratio_to_the_pixel() {
    for (ratio, custom, terms) in [
        (Ratio::Preset([9, 16]), [1, 1], [9, 16]),
        (Ratio::Custom, [9, 20], [9, 20]),
        (Ratio::Original, [1, 1], [6, 5]),
        (Ratio::Preset([16, 9]), [1, 1], [16, 9]),
    ] {
        let (context, mut app) = crop_app();
        app.crop.ratio = ratio;
        app.crop.custom = custom;
        for to in [p(97.3, 81.1), p(13.6, 77.9), p(91.2, 4.4), p(5.5, 9.9)] {
            // A press on the last box would adjust it rather than draw a new one.
            app.crop.rect = None;
            drag(&context, &mut app, p(55.0, 45.0), to, NONE);
            let rect = crop_box(&app);
            assert_exact(rect, terms);
            assert!(rect.right() <= 120 && rect.bottom() <= 100, "{rect:?}");
            // The press stays a corner of the box.
            assert!(
                (rect.x == 55 || rect.right() == 55) && (rect.y == 45 || rect.bottom() == 45),
                "{ratio:?} {rect:?}"
            );
        }
    }
}

#[test]
fn shift_draws_a_square_when_free_and_free_is_any_shape() {
    let (context, mut app) = crop_app();
    drag(&context, &mut app, p(10.0, 10.0), p(50.0, 30.0), NONE);
    assert_eq!(crop_box(&app), CropBox::new(10, 10, 40, 20));
    drag(
        &context,
        &mut app,
        p(70.0, 50.0),
        p(100.0, 70.0),
        egui::Modifiers::SHIFT,
    );
    assert_eq!(crop_box(&app), CropBox::new(70, 50, 30, 30));
}

#[test]
fn escape_discards_the_box_and_leaves_the_canvas() {
    let (context, mut app) = crop_app();
    drag(&context, &mut app, p(10.0, 10.0), p(50.0, 30.0), NONE);
    assert!(app.crop.rect.is_some());
    press(&context, &mut app, egui::Key::Escape);
    assert!(app.crop.rect.is_none());
    assert_eq!(size(&app), (120, 100));
    assert_eq!(steps(&app), 0);
    // Enter with nothing to crop does nothing.
    press(&context, &mut app, egui::Key::Enter);
    assert_eq!(size(&app), (120, 100));
    assert_eq!(steps(&app), 0);
}

#[test]
fn a_click_without_a_drag_keeps_the_box() {
    let (context, mut app) = crop_app();
    drag(&context, &mut app, p(10.0, 10.0), p(50.0, 30.0), NONE);
    click_canvas(&context, &mut app, p(90.0, 80.0), NONE);
    assert_eq!(crop_box(&app), CropBox::new(10, 10, 40, 20));
}

#[test]
fn pressing_c_with_a_selection_starts_at_its_bounds() {
    let select = |app: &mut EditorApp| {
        app.edit_selection("Rect", |doc| {
            let mut mask = GrayImage::new(120, 100);
            for y in 30..90 {
                for x in 20..120 {
                    mask.put_pixel(x, y, image::Luma([255]));
                }
            }
            doc.selection = Some(Arc::new(mask));
        });
    };
    let (context, mut app) = app();
    app.dimensions = [120, 100];
    app.new_document();
    select(&mut app);
    frame(&context, &mut app);
    press(&context, &mut app, egui::Key::C);
    assert_eq!(app.tool, Tool::Crop);
    assert_eq!(crop_box(&app), CropBox::new(20, 30, 100, 60));
    // In a ratio, the largest box of it inside the bounds, centred.
    app.set_tool(Tool::Move);
    app.crop.ratio = Ratio::Preset([9, 16]);
    press(&context, &mut app, egui::Key::C);
    assert_eq!(crop_box(&app), CropBox::new(56, 36, 27, 48));
    // Applying crops to it in one step.
    press(&context, &mut app, egui::Key::Enter);
    assert_eq!(size(&app), (27, 48));
    assert_eq!(steps(&app), 2);
}

#[test]
fn the_ratio_stays_chosen_across_tools_and_documents() {
    let (context, mut app) = crop_app();
    app.set_crop_ratio(Ratio::Custom, [9, 20], false);
    drag(&context, &mut app, p(10.0, 10.0), p(50.0, 90.0), NONE);
    app.set_tool(Tool::Move);
    assert!(app.crop.rect.is_none());
    app.new_document();
    app.set_tool(Tool::Crop);
    assert_eq!((app.crop.ratio, app.crop.custom), (Ratio::Custom, [9, 20]));
}

#[test]
fn changing_and_swapping_the_ratio_refits_the_box() {
    let (context, mut app) = crop_app();
    drag(&context, &mut app, p(10.0, 10.0), p(100.0, 90.0), NONE);
    app.set_crop_ratio(Ratio::Preset([9, 16]), app.crop.custom, false);
    let rect = crop_box(&app);
    assert_exact(rect, [9, 16]);
    assert_eq!(rect.height, 80);
    // Swap turns it about its middle into 16:9.
    let (ratio, custom) = app.crop.ratio.swapped([120, 100], app.crop.custom);
    app.set_crop_ratio(ratio, custom, true);
    assert_eq!(app.crop.ratio, Ratio::Preset([16, 9]));
    let turned = crop_box(&app);
    assert_exact(turned, [16, 9]);
    assert_eq!((turned.width, turned.height), (80, 45));
}

#[test]
fn arrow_keys_move_the_box_not_the_layer() {
    let (context, mut app) = crop_app();
    drag(&context, &mut app, p(10.0, 10.0), p(50.0, 30.0), NONE);
    press(&context, &mut app, egui::Key::ArrowRight);
    press(&context, &mut app, egui::Key::ArrowDown);
    assert_eq!(crop_box(&app), CropBox::new(11, 11, 40, 20));
    let layer = app.session().unwrap().document.active().unwrap().transform;
    assert_eq!((layer.x, layer.y), (0.0, 0.0));
    assert_eq!(steps(&app), 0);
}

#[test]
fn a_box_left_on_a_canvas_that_shrank_is_cut_to_it() {
    let (context, mut app) = crop_app();
    drag(&context, &mut app, p(60.0, 10.0), p(110.0, 90.0), NONE);
    app.command("rotate_canvas_cw");
    assert_eq!(size(&app), (100, 120));
    frame(&context, &mut app);
    press(&context, &mut app, egui::Key::Enter);
    assert_eq!(size(&app), (40, 80));
}

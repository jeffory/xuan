//! The Crop tool's Perspective mode on the canvas: Shift+C, the corners it starts with, dragging
//! and clicking them, and straightening, cancelling or refusing them.
use super::*;
use xuan::crop::perspective::{self, Quad};

/// A 120 × 100 document with two layers at different places, and the Crop tool switched to
/// Perspective mode by Shift+C.
fn perspective_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [120, 100];
    app.new_document();
    let mut layer = Layer::image(
        "Offset",
        RgbaImage::from_pixel(30, 20, image::Rgba([9, 9, 9, 255])),
    );
    layer.transform.x = 40.0;
    layer.transform.y = 30.0;
    layer.transform.rotation = 10.0;
    app.session_mut().unwrap().document.insert(layer);
    app.set_tool(Tool::Crop);
    frame(&context, &mut app);
    shift_c(&context, &mut app);
    assert_eq!(app.tool, Tool::Crop);
    assert!(app.crop.perspective);
    (context, app)
}

fn shift_c(context: &egui::Context, app: &mut EditorApp) {
    keyboard_frame(
        context,
        app,
        vec![text_key(egui::Key::C, egui::Modifiers::SHIFT)],
        egui::Modifiers::SHIFT,
    );
}

fn press(context: &egui::Context, app: &mut EditorApp, key: egui::Key) {
    keyboard_frame(
        context,
        app,
        vec![text_key(key, egui::Modifiers::NONE)],
        egui::Modifiers::NONE,
    );
}

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

fn quad(app: &EditorApp) -> Quad {
    app.crop.quad.expect("perspective corners")
}

#[track_caller]
fn assert_near(actual: Quad, expected: Quad) {
    for (a, e) in actual.iter().zip(expected) {
        assert!(a.distance(e) < 0.05, "{actual:?} != {expected:?}");
    }
}

fn steps(app: &EditorApp) -> usize {
    app.session().unwrap().history.names().count()
}

/// The canvas size and every layer's transform.
fn state(app: &EditorApp) -> (u32, u32, String) {
    let document = &app.session().unwrap().document;
    let transforms: Vec<_> = document.layers.iter().map(|l| l.transform).collect();
    (document.width, document.height, format!("{transforms:?}"))
}

const INSET: Quad = [
    Point::new(12.0, 10.0),
    Point::new(108.0, 10.0),
    Point::new(108.0, 90.0),
    Point::new(12.0, 90.0),
];

const NONE: egui::Modifiers = egui::Modifiers::NONE;

#[test]
fn shift_c_switches_between_the_box_and_perspective_corners() {
    let (context, mut app) = perspective_app();
    // The corners start inset from the canvas edges.
    assert_eq!(quad(&app), INSET);
    assert!(app.crop.rect.is_none());
    // Again: back to the box, with no corners left.
    shift_c(&context, &mut app);
    assert_eq!(app.tool, Tool::Crop);
    assert!(!app.crop.perspective && app.crop.quad.is_none());
    shift_c(&context, &mut app);
    assert!(app.crop.perspective);
    // From another tool, C and Shift+C both pick Crop in the mode used last.
    app.set_tool(Tool::Move);
    assert!(app.crop.quad.is_none());
    press(&context, &mut app, egui::Key::C);
    assert_eq!((app.tool, app.crop.perspective), (Tool::Crop, true));
    assert_eq!(quad(&app), INSET);
    app.set_tool(Tool::Brush);
    shift_c(&context, &mut app);
    assert_eq!((app.tool, app.crop.perspective), (Tool::Crop, true));
}

#[test]
fn dragging_a_corner_moves_it_alone_and_enter_straightens_in_one_undo_step() {
    let (context, mut app) = perspective_app();
    let before = state(&app);
    drag(&context, &mut app, p(12.0, 10.0), p(24.0, 16.0), NONE);
    let mut expected = INSET;
    expected[0] = p(24.0, 16.0);
    assert_near(quad(&app), expected);
    // Dragging inside moves all four.
    drag(&context, &mut app, p(60.0, 50.0), p(62.0, 49.0), NONE);
    let moved = perspective::moved(expected, p(2.0, -1.0));
    assert_near(quad(&app), moved);
    // Arrow keys nudge them too, and nothing is an undo step yet.
    press(&context, &mut app, egui::Key::ArrowLeft);
    let nudged = perspective::moved(moved, p(-1.0, 0.0));
    assert_near(quad(&app), nudged);
    assert_eq!(steps(&app), 0);
    assert_eq!(state(&app), before);

    let corners = quad(&app);
    let size = perspective::output_size(corners);
    press(&context, &mut app, egui::Key::Enter);
    assert!(app.crop.quad.is_none());
    let (width, height, _) = state(&app);
    assert_eq!([width, height], size);
    assert_eq!(steps(&app), 1);
    assert_eq!(
        app.session().unwrap().history.undo_name(),
        Some("Perspective Crop")
    );
    // Both layers took the same map, so they stay registered.
    let map = perspective::rectify(corners, size).unwrap();
    let document = &app.session().unwrap().document;
    let offset = document.layers.iter().find(|l| l.name == "Offset").unwrap();
    let original = Transform {
        x: 40.0,
        y: 30.0,
        width: 30.0,
        height: 20.0,
        rotation: 10.0,
        ..Transform::new(1, 1)
    };
    for unit in [p(0.0, 0.0), p(1.0, 0.0), p(0.5, 0.5), p(1.0, 1.0)] {
        let expected = map.map(original.point(unit));
        assert!(offset.transform.point(unit).distance(expected) < 0.01);
    }
    app.command("undo");
    assert_eq!(state(&app), before);
}

#[test]
fn escape_discards_the_corners_and_reset_brings_them_back() {
    let (context, mut app) = perspective_app();
    drag(&context, &mut app, p(108.0, 90.0), p(100.0, 70.0), NONE);
    press(&context, &mut app, egui::Key::Escape);
    assert!(app.crop.quad.is_none());
    // Enter with nothing to straighten does nothing.
    press(&context, &mut app, egui::Key::Enter);
    assert_eq!((state(&app).0, steps(&app)), (120, 0));
    app.reset_quad();
    assert_eq!(quad(&app), INSET);
    // A typed size goes with Reset as well.
    app.crop.size = Some([10, 10]);
    app.reset_quad();
    assert_eq!(app.crop.size, None);
}

#[test]
fn four_clicks_place_the_corners_in_order() {
    let (context, mut app) = perspective_app();
    press(&context, &mut app, egui::Key::Escape);
    for point in [p(100.0, 85.0), p(20.0, 15.0), p(15.0, 80.0)] {
        click_canvas(&context, &mut app, point, NONE);
        assert!(app.crop.quad.is_none());
    }
    assert_eq!(app.crop.placing.len(), 3);
    click_canvas(&context, &mut app, p(105.0, 12.0), NONE);
    assert!(app.crop.placing.is_empty());
    assert_near(
        quad(&app),
        [p(20.0, 15.0), p(105.0, 12.0), p(100.0, 85.0), p(15.0, 80.0)],
    );
    assert_eq!(steps(&app), 0);
    // A click outside the corners leaves them alone.
    click_canvas(&context, &mut app, p(2.0, 2.0), NONE);
    assert_near(
        quad(&app),
        [p(20.0, 15.0), p(105.0, 12.0), p(100.0, 85.0), p(15.0, 80.0)],
    );
}

#[test]
fn crossed_or_dented_corners_are_refused_with_the_reason() {
    let (context, mut app) = perspective_app();
    let before = state(&app);
    // Drag the bottom-right corner over to the left: the sides cross.
    drag(&context, &mut app, p(108.0, 90.0), p(5.0, 60.0), NONE);
    press(&context, &mut app, egui::Key::Enter);
    assert!(app.status.contains("sides cross"), "{}", app.status);
    assert!(app.crop.quad.is_some(), "the corners stay to be fixed");
    // A corner pulled inside: not convex.
    app.crop.quad = Some([INSET[0], INSET[1], p(40.0, 40.0), INSET[3]]);
    app.apply_perspective_crop();
    assert!(app.status.contains("convex"), "{}", app.status);
    assert_eq!(state(&app), before);
    assert_eq!(steps(&app), 0);
    assert!(app.error.is_none());
}

#[test]
fn a_typed_size_and_a_selection_are_used() {
    let (context, mut app) = perspective_app();
    app.crop.size = Some([50, 70]);
    press(&context, &mut app, egui::Key::Enter);
    assert_eq!(state(&app).0, 50);
    assert_eq!(state(&app).1, 70);
    // Typed sizes are for one crop.
    assert_eq!(app.crop.size, None);
    app.edit_selection("Rect", |doc| {
        let mut mask = GrayImage::new(50, 70);
        for y in 10..30 {
            for x in 5..45 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        doc.selection = Some(Arc::new(mask));
    });
    app.set_tool(Tool::Move);
    press(&context, &mut app, egui::Key::C);
    assert_eq!(
        quad(&app),
        [p(5.0, 10.0), p(45.0, 10.0), p(45.0, 30.0), p(5.0, 30.0)]
    );
}

#[test]
fn the_corners_stay_with_their_document() {
    let (context, mut app) = perspective_app();
    app.new_document();
    frame(&context, &mut app);
    assert!(app.crop.quad.is_none());
    press(&context, &mut app, egui::Key::Enter);
    app.current = 0;
    frame(&context, &mut app);
    assert!(app.crop.quad.is_none());
    assert_eq!((state(&app).0, steps(&app)), (120, 0));
}

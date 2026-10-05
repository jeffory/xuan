//! View → Snap To applied to real canvas drags, and guides dragged with snapping.
use super::*;
use xuan::layout::{GridSettings, Guide, GuideAxis};

/// A 64x48 document at 200% (so the 10 point snap reach is 5 pixels) with an opaque 20x16 "Box"
/// layer at (5, 8), selected. The canvas centre is (32, 24).
fn snapping_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [64, 48];
    app.new_document();
    app.session_mut()
        .unwrap()
        .document
        .insert(opaque("Box", 5.0, 8.0, 20, 16));
    frame(&context, &mut app);
    let session = app.session_mut().unwrap();
    session.zoom = 2.0;
    session.pan = Vec2::ZERO;
    session.fit = false;
    frame(&context, &mut app);
    (context, app)
}

fn opaque(name: &str, x: f32, y: f32, width: u32, height: u32) -> Layer {
    let mut layer = Layer::image(
        name,
        RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255])),
    );
    layer.transform.x = x;
    layer.transform.y = y;
    layer
}

fn add_guide(app: &mut EditorApp, axis: GuideAxis, position: f32) {
    app.session_mut()
        .unwrap()
        .document
        .guides
        .push(Guide::new(axis, position));
}

fn active_transform(app: &EditorApp) -> Transform {
    app.session().unwrap().document.active().unwrap().transform
}

fn drag_x(
    context: &egui::Context,
    app: &mut EditorApp,
    from: Point,
    dx: f32,
    mods: egui::Modifiers,
) {
    drag(context, app, from, Point::new(from.x + dx, from.y), mods);
}

#[test]
fn moving_a_layer_snaps_its_edges_to_a_guide_unless_ctrl_is_held() {
    let (context, mut app) = snapping_app();
    add_guide(&mut app, GuideAxis::Vertical, 20.0);
    // Moved by 13.5 the left edge lands at 18.5, 1.5 from the guide.
    drag_x(
        &context,
        &mut app,
        Point::new(10.0, 14.0),
        13.5,
        egui::Modifiers::NONE,
    );
    let t = active_transform(&app);
    assert_eq!((t.x, t.y), (20.0, 8.0));
    assert!(app.snap_lines.is_empty(), "feedback ends with the drag");

    // Ctrl drags freely: the left edge stops at 24.3, within reach of the guide.
    drag_x(
        &context,
        &mut app,
        Point::new(25.0, 12.0),
        4.3,
        egui::Modifiers::CTRL,
    );
    let t = active_transform(&app);
    assert!((t.x - 24.3).abs() < 1e-3, "{t:?}");

    // With Snap To → Guides (and the canvas) off, nothing pulls.
    app.config.snap.guides = false;
    app.config.snap.bounds = false;
    drag_x(
        &context,
        &mut app,
        Point::new(30.0, 12.0),
        -4.0,
        egui::Modifiers::NONE,
    );
    assert!((active_transform(&app).x - 20.3).abs() < 1e-3);
}

#[test]
fn moving_a_layer_snaps_to_the_canvas_and_other_layers() {
    let (context, mut app) = snapping_app();
    let from = Point::new(10.0, 14.0);
    // The box's centre (15) moved by 16 lands at 31, a pixel from the canvas centre.
    drag_x(&context, &mut app, from, 16.0, egui::Modifiers::NONE);
    assert_eq!(active_transform(&app).x, 22.0);
    app.command("undo");
    app.config.snap.bounds = false;
    drag_x(&context, &mut app, from, 16.0, egui::Modifiers::NONE);
    assert_eq!(active_transform(&app).x, 21.0);
    app.command("undo");

    // Another layer's left edge at 45 pulls the box's right edge from 44.
    let document = &mut app.session_mut().unwrap().document;
    let box_id = document.active.unwrap();
    document.insert(opaque("Other", 45.0, 30.0, 10, 10));
    document.select(box_id, false);
    drag_x(&context, &mut app, from, 19.0, egui::Modifiers::NONE);
    assert_eq!(active_transform(&app).x, 25.0);
    app.command("undo");
    app.config.snap.layers = false;
    drag_x(&context, &mut app, from, 19.0, egui::Modifiers::NONE);
    assert_eq!(active_transform(&app).x, 24.0);
    app.command("undo");

    // The master toggle turns everything off.
    app.config.snap.layers = true;
    app.config.snap.bounds = true;
    app.command("toggle_snap");
    assert!(!app.config.snap.enabled);
    drag_x(&context, &mut app, from, 16.0, egui::Modifiers::NONE);
    assert_eq!(active_transform(&app).x, 21.0);
}

#[test]
fn resize_handles_snap_the_dragged_edge() {
    let (context, mut app) = snapping_app();
    add_guide(&mut app, GuideAxis::Vertical, 30.0);
    app.lock_ratio = false;
    // The right-middle handle sits at (25, 16); dragged to 28.5 its edge is 1.5 from the guide.
    drag_x(
        &context,
        &mut app,
        Point::new(25.0, 16.0),
        3.5,
        egui::Modifiers::NONE,
    );
    let t = active_transform(&app);
    assert!(
        (t.x - 5.0).abs() < 1e-3 && (t.width - 25.0).abs() < 1e-3 && t.height == 16.0,
        "{t:?}"
    );
}

#[test]
fn marquee_corners_and_selection_moves_snap() {
    let (context, mut app) = snapping_app();
    app.config.snap.layers = false;
    add_guide(&mut app, GuideAxis::Vertical, 20.0);
    add_guide(&mut app, GuideAxis::Horizontal, 30.0);
    app.set_tool(Tool::Marquee);
    drag(
        &context,
        &mut app,
        Point::new(10.0, 10.0),
        Point::new(18.5, 28.0),
        egui::Modifiers::NONE,
    );
    let bounds = |app: &EditorApp| {
        xuan::selection::bounds(app.session().unwrap().document.selection.as_ref().unwrap())
    };
    assert_eq!(bounds(&app), Some((10, 10, 20, 30)));

    // Moving the outline by 4 brings its middle (19) next to the guide at 20.
    drag_x(
        &context,
        &mut app,
        Point::new(15.0, 15.0),
        3.5,
        egui::Modifiers::NONE,
    );
    assert_eq!(bounds(&app), Some((15, 10, 25, 30)));

    // The grid pulls once it is shown and Snap To → Grid is on.
    app.config.show_grid = true;
    app.config.snap.grid = true;
    app.config.grid = GridSettings {
        spacing: 8,
        subdivisions: 1,
        ..GridSettings::default()
    };
    app.command("deselect");
    drag(
        &context,
        &mut app,
        Point::new(41.0, 33.0),
        Point::new(54.6, 38.6),
        egui::Modifiers::NONE,
    );
    assert_eq!(bounds(&app), Some((40, 32, 56, 40)));
}

#[test]
fn shapes_start_and_end_on_targets() {
    let (context, mut app) = snapping_app();
    add_guide(&mut app, GuideAxis::Vertical, 40.0);
    add_guide(&mut app, GuideAxis::Horizontal, 20.0);
    app.set_tool(Tool::Shape);
    drag(
        &context,
        &mut app,
        Point::new(38.0, 21.0),
        Point::new(54.0, 36.0),
        egui::Modifiers::NONE,
    );
    let t = active_transform(&app);
    assert_eq!(
        (t.x, t.y, t.width, t.height),
        (40.0, 20.0, 14.0, 16.0),
        "{t:?}"
    );
}

#[test]
fn guides_snap_while_dragged_and_follow_undo() {
    let (_context, mut app) = snapping_app();
    // A new guide 1.5 px from the box's left edge (5) lands on it.
    app.begin_guide_creation(GuideAxis::Vertical, 6.5, false);
    app.finish_guide_drag(false);
    assert_eq!(app.session().unwrap().document.guides[0].position, 5.0);
    // Ctrl places it freely; a drop on a ruler abandons a new one.
    app.begin_guide_creation(GuideAxis::Vertical, 6.5, true);
    app.finish_guide_drag(false);
    app.begin_guide_creation(GuideAxis::Horizontal, 3.0, false);
    app.finish_guide_drag(true);
    let guides = &app.session().unwrap().document.guides;
    assert_eq!(guides.len(), 2);
    assert_eq!(guides[1].position, 6.5);
    // Moving one snaps to the other guide.
    let second = guides[1];
    app.config.snap.layers = false;
    app.config.snap.bounds = false;
    app.begin_guide_move(second);
    app.move_guide_drag(11.0, false);
    assert_eq!(app.displayed_guides()[1].position, 11.0);
    app.move_guide_drag(6.0, false);
    assert_eq!(app.displayed_guides()[1].position, 5.0);
    app.cancel_guide_drag();
    assert_eq!(app.session().unwrap().document.guides[1].position, 6.5);

    app.command("undo");
    assert_eq!(app.session().unwrap().document.guides.len(), 1);
    app.command("clear_guides");
    assert!(app.session().unwrap().document.guides.is_empty());
    app.command("undo");
    assert_eq!(app.session().unwrap().document.guides.len(), 1);
}

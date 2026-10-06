//! The Pen tool: drawing paths into the document, editing their anchors and handles, and
//! editing path shape layers' outlines, each change one undo step.
use super::*;
use kurbo::{ParamCurve, ParamCurveNearest, Point as KPoint};
use pen_tool::PenTarget;
use xuan::path_edit::{EditPath, Side};

const NONE: egui::Modifiers = egui::Modifiers::NONE;
const ALT: egui::Modifiers = egui::Modifiers::ALT;

/// A 100x80 document at 4x zoom with the Pen selected.
fn pen_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [100, 80];
    app.new_document();
    app.set_tool(Tool::Pen);
    let session = app.session_mut().unwrap();
    session.zoom = 4.0;
    session.fit = false;
    frame(&context, &mut app);
    (context, app)
}

fn screen(app: &EditorApp, x: f64, y: f64) -> Pos2 {
    app.canvas_rect.unwrap().min + Vec2::new(x as f32, y as f32) * app.session().unwrap().zoom
}

fn pen_click(context: &egui::Context, app: &mut EditorApp, x: f64, y: f64, mods: egui::Modifiers) {
    let pos = screen(app, x, y);
    pointer_frame(context, app, pos, None, mods);
    pointer_frame(context, app, pos, Some(true), mods);
    pointer_frame(context, app, pos, Some(false), mods);
}

fn pen_drag(
    context: &egui::Context,
    app: &mut EditorApp,
    from: (f64, f64),
    to: (f64, f64),
    mods: egui::Modifiers,
) {
    let a = screen(app, from.0, from.1);
    let b = screen(app, to.0, to.1);
    pointer_frame(context, app, a, None, mods);
    pointer_frame(context, app, a, Some(true), mods);
    for step in 1..=4 {
        pointer_frame(context, app, a + (b - a) * (step as f32 / 4.0), None, mods);
    }
    pointer_frame(context, app, b, Some(false), mods);
}

fn press_key(context: &egui::Context, app: &mut EditorApp, key: egui::Key) {
    keyboard_frame(context, app, vec![text_key(key, NONE)], NONE);
}

fn paths(app: &EditorApp) -> &[xuan::vector::NamedPath] {
    &app.session().unwrap().document.paths
}

fn edit_path(app: &EditorApp, index: usize) -> EditPath {
    EditPath::from_vector(&paths(app)[index].d)
}

fn revision(app: &EditorApp) -> u64 {
    app.session().unwrap().history.revision
}

fn near(a: KPoint, b: (f64, f64)) -> bool {
    a.distance(KPoint::new(b.0, b.1)) < 0.01
}

fn points(path: &EditPath) -> Vec<KPoint> {
    path.subpaths[0].anchors.iter().map(|a| a.point).collect()
}

/// Every point sampled along `a` lies on `b`.
fn lies_on(a: &kurbo::BezPath, b: &kurbo::BezPath) -> bool {
    a.segments().all(|segment| {
        (0..=20).all(|i| {
            let p = segment.eval(f64::from(i) / 20.0);
            b.segments()
                .any(|s| s.nearest(p, 1e-9).distance_sq.sqrt() < 1e-3)
        })
    })
}

#[test]
fn p_selects_the_pen() {
    let (context, mut app) = pen_app();
    app.set_tool(Tool::Move);
    press_key(&context, &mut app, egui::Key::P);
    assert!(app.tool == Tool::Pen);
    assert_eq!(commands::tool_command(Tool::Pen), Some("tool_pen"));
}

#[test]
fn clicks_and_enter_make_an_open_polyline_path() {
    let (context, mut app) = pen_app();
    let start = revision(&app);
    for (x, y) in [(10.0, 10.0), (50.0, 10.0), (50.0, 40.0)] {
        pen_click(&context, &mut app, x, y, NONE);
    }
    assert!(paths(&app).is_empty(), "nothing is added while drawing");
    assert_eq!(app.pen.draft.as_ref().unwrap().subpath.anchors.len(), 3);
    // The draft and its rubber band are drawn.
    frame(&context, &mut app);
    press_key(&context, &mut app, egui::Key::Enter);
    assert!(app.pen.draft.is_none());
    assert_eq!(paths(&app).len(), 1);
    assert_eq!(paths(&app)[0].name, "Path 1");
    let path = edit_path(&app, 0);
    assert!(!path.subpaths[0].closed);
    let anchors = points(&path);
    for (anchor, expected) in anchors
        .iter()
        .zip([(10.0, 10.0), (50.0, 10.0), (50.0, 40.0)])
    {
        assert!(near(*anchor, expected), "{anchors:?}");
    }
    assert!(
        path.subpaths[0]
            .anchors
            .iter()
            .all(|a| !a.has_in() && !a.has_out())
    );
    assert!(!paths(&app)[0].d.to_svg().contains('C'));
    assert_eq!(revision(&app), start + 1, "one undo step");
    // It is shown for editing.
    assert_eq!(app.pen_target(), Some(PenTarget::Path(paths(&app)[0].id)));

    // A second path is Path 2; Escape finishes it too.
    pen_click(&context, &mut app, 80.0, 70.0, NONE);
    pen_click(&context, &mut app, 90.0, 70.0, NONE);
    press_key(&context, &mut app, egui::Key::Escape);
    assert_eq!(paths(&app).len(), 2);
    assert_eq!(paths(&app)[1].name, "Path 2");

    app.command("undo");
    app.command("undo");
    assert!(paths(&app).is_empty());
}

#[test]
fn dragging_makes_a_smooth_anchor_with_symmetric_handles() {
    let (context, mut app) = pen_app();
    pen_click(&context, &mut app, 10.0, 40.0, NONE);
    pen_drag(&context, &mut app, (50.0, 40.0), (60.0, 30.0), NONE);
    pen_click(&context, &mut app, 90.0, 40.0, NONE);
    // Switching tools finishes the path.
    app.set_tool(Tool::Move);
    assert_eq!(paths(&app).len(), 1);
    let path = edit_path(&app, 0);
    let smooth = path.subpaths[0].anchors[1];
    assert!(near(smooth.point, (50.0, 40.0)));
    assert!(near(smooth.handle_out, (60.0, 30.0)), "{smooth:?}");
    assert!(near(smooth.handle_in, (40.0, 50.0)), "{smooth:?}");
    assert!(smooth.smooth);
    assert!(paths(&app)[0].d.to_svg().contains('C'));
}

#[test]
fn clicking_the_first_anchor_closes_the_path() {
    let (context, mut app) = pen_app();
    for (x, y) in [(10.0, 10.0), (50.0, 10.0), (30.0, 40.0)] {
        pen_click(&context, &mut app, x, y, NONE);
    }
    // Within a few screen pixels of the first anchor.
    pen_click(&context, &mut app, 10.5, 10.5, NONE);
    assert!(app.pen.draft.is_none());
    assert_eq!(paths(&app).len(), 1);
    let path = edit_path(&app, 0);
    assert!(path.subpaths[0].closed);
    assert_eq!(path.subpaths[0].anchors.len(), 3);
    assert!(paths(&app)[0].d.to_svg().ends_with('Z'));
}

/// A document with the path `d`, shown for editing.
fn editing(d: &str) -> (egui::Context, EditorApp) {
    let (context, mut app) = pen_app();
    let id = app.add_path(d).unwrap();
    app.pen_select(Some(PenTarget::Path(id)));
    frame(&context, &mut app);
    (context, app)
}

#[test]
fn dragging_an_anchor_moves_it_as_one_undo_step() {
    let (context, mut app) = editing("M 10 10 L 50 10 L 50 40");
    let start = revision(&app);
    pen_drag(&context, &mut app, (50.0, 10.0), (60.0, 20.0), NONE);
    let anchors = points(&edit_path(&app, 0));
    assert!(near(anchors[1], (60.0, 20.0)), "{anchors:?}");
    assert!(near(anchors[2], (50.0, 40.0)));
    assert_eq!(revision(&app), start + 1);
    assert_eq!(app.pen.selected, Some((0, 1)));
    app.command("undo");
    assert!(near(points(&edit_path(&app, 0))[1], (50.0, 10.0)));
}

#[test]
fn handles_move_symmetrically_and_alt_breaks_the_symmetry() {
    let (context, mut app) = editing("M 10 40 C 10 20 30 10 50 10 C 70 10 90 20 90 40");
    assert!(edit_path(&app, 0).subpaths[0].anchors[1].smooth);
    let start = revision(&app);
    pen_drag(&context, &mut app, (70.0, 10.0), (70.0, 0.0), NONE);
    let anchor = edit_path(&app, 0).subpaths[0].anchors[1];
    assert!(near(anchor.handle_out, (70.0, 0.0)), "{anchor:?}");
    // The in handle turned to stay opposite, keeping its length.
    let (a, b) = (
        anchor.handle_in - anchor.point,
        anchor.handle_out - anchor.point,
    );
    assert!(a.cross(b).abs() < 1e-3 && a.dot(b) < 0.0, "{anchor:?}");
    assert!((a.hypot() - 20.0).abs() < 1e-3);
    assert!(anchor.smooth);
    assert_eq!(revision(&app), start + 1);

    let before = anchor.handle_in;
    pen_drag(&context, &mut app, (70.0, 0.0), (75.0, 15.0), ALT);
    let anchor = edit_path(&app, 0).subpaths[0].anchors[1];
    assert!(near(anchor.handle_out, (75.0, 15.0)), "{anchor:?}");
    assert!(
        anchor.handle_in.distance(before) < 1e-3,
        "the in handle stays"
    );
    assert!(!anchor.smooth);
    assert_eq!(revision(&app), start + 2);

    app.command("undo");
    let anchor = edit_path(&app, 0).subpaths[0].anchors[1];
    assert!(near(anchor.handle_out, (70.0, 0.0)));
    app.command("undo");
    let anchor = edit_path(&app, 0).subpaths[0].anchors[1];
    assert!(near(anchor.handle_out, (70.0, 10.0)));
    assert_eq!(anchor.handle(Side::In), KPoint::new(30.0, 10.0));
}

#[test]
fn clicking_a_segment_adds_an_anchor_that_keeps_the_curve() {
    let (context, mut app) = editing("M 10 60 C 10 10 90 10 90 60");
    let before = paths(&app)[0].d.bez().clone();
    let on = before.segments().next().unwrap().eval(0.37);
    let start = revision(&app);
    pen_click(&context, &mut app, on.x, on.y, NONE);
    let path = edit_path(&app, 0);
    assert_eq!(path.subpaths[0].anchors.len(), 3);
    assert!(path.subpaths[0].anchors[1].point.distance(on) < 0.5);
    let after = paths(&app)[0].d.bez().clone();
    assert!(lies_on(&before, &after) && lies_on(&after, &before));
    assert_eq!(revision(&app), start + 1);
    // Delete removes the anchor just added.
    assert_eq!(app.pen.selected, Some((0, 1)));
    press_key(&context, &mut app, egui::Key::Delete);
    assert_eq!(edit_path(&app, 0).subpaths[0].anchors.len(), 2);
    assert_eq!(revision(&app), start + 2);
    app.command("undo");
    app.command("undo");
    assert_eq!(paths(&app)[0].d.bez(), &before);
}

#[test]
fn clicking_an_anchor_deletes_it_and_alt_click_converts_it() {
    let (context, mut app) = editing("M 10 10 L 50 10 L 90 10 L 90 50");
    let start = revision(&app);
    pen_click(&context, &mut app, 50.0, 10.0, ALT);
    let anchor = edit_path(&app, 0).subpaths[0].anchors[1];
    assert!(
        anchor.smooth && anchor.has_in() && anchor.has_out(),
        "{anchor:?}"
    );
    pen_click(&context, &mut app, 50.0, 10.0, ALT);
    let anchor = edit_path(&app, 0).subpaths[0].anchors[1];
    assert!(!anchor.smooth && !anchor.has_in() && !anchor.has_out());
    assert_eq!(revision(&app), start + 2);

    pen_click(&context, &mut app, 50.0, 10.0, NONE);
    let anchors = points(&edit_path(&app, 0));
    assert_eq!(anchors.len(), 3);
    assert!(near(anchors[1], (90.0, 10.0)));
    assert_eq!(revision(&app), start + 3);
    app.command("undo");
    assert_eq!(points(&edit_path(&app, 0)).len(), 4);
}

#[test]
fn clicking_another_path_shows_it_and_escape_hides_it() {
    let (context, mut app) = editing("M 10 10 L 50 10");
    let first = paths(&app)[0].id;
    let second = app.add_path("M 10 60 L 50 60").unwrap();
    let start = revision(&app);
    pen_click(&context, &mut app, 30.0, 60.0, NONE);
    assert_eq!(app.pen_target(), Some(PenTarget::Path(second)));
    assert_eq!(revision(&app), start, "picking a path changes nothing");
    pen_click(&context, &mut app, 30.0, 10.0, NONE);
    assert_eq!(app.pen_target(), Some(PenTarget::Path(first)));
    press_key(&context, &mut app, egui::Key::Escape);
    assert_eq!(app.pen_target(), None);
}

#[test]
fn the_paths_dialog_shows_its_selected_path_for_the_pen() {
    let (context, mut app) = pen_app();
    let first = app.add_path("M 10 10 L 50 10").unwrap();
    app.command("paths");
    assert_eq!(app.pen_target(), Some(PenTarget::Path(first)));
    // Drawn on the canvas while the dialog is open.
    frame(&context, &mut app);
    assert_eq!(app.dialog, Some(Dialog::Paths));
}

#[test]
fn path_shape_layer_outlines_are_edited_in_place() {
    let (context, mut app) = pen_app();
    let outline = xuan::vector::VectorPath::parse("M 10 10 L 50 10 L 50 40 Z").unwrap();
    let layer =
        xuan::paint::path_shape(&outline, xuan::vector::FillRule::Nonzero, [255, 0, 0, 255])
            .unwrap();
    let id = layer.id;
    app.session_mut().unwrap().document.insert(layer);
    frame(&context, &mut app);
    assert_eq!(app.pen_target(), Some(PenTarget::Layer(id)));
    let start = revision(&app);
    pen_drag(&context, &mut app, (50.0, 40.0), (70.0, 60.0), NONE);
    assert_eq!(revision(&app), start + 1);
    let document = &app.session().unwrap().document;
    let layer = document.layers.iter().find(|l| l.id == id).unwrap();
    let shape = layer.shape.as_ref().unwrap().path.as_ref().unwrap();
    // Still box-local: the box grew to the new bounds and the layer with it.
    assert_eq!((shape.width, shape.height), (60.0, 50.0));
    assert_eq!((layer.transform.x, layer.transform.y), (10.0, 10.0));
    assert_eq!(
        (layer.transform.width, layer.transform.height),
        (60.0, 50.0)
    );
    assert_eq!(layer.pixels.as_ref().unwrap().dimensions(), (60, 50));
    let anchors = points(&pen_tool::layer_outline(layer).unwrap());
    assert!(near(anchors[2], (70.0, 60.0)), "{anchors:?}");
    assert!(near(anchors[0], (10.0, 10.0)), "{anchors:?}");
    // The new corner is filled.
    assert!(layer.pixels.as_ref().unwrap().get_pixel(58, 47)[3] > 100);

    app.command("undo");
    let document = &app.session().unwrap().document;
    let layer = document.layers.iter().find(|l| l.id == id).unwrap();
    assert_eq!(layer.transform.width, 40.0);
    assert_eq!(layer.pixels.as_ref().unwrap().dimensions(), (40, 30));
}

#[test]
fn a_selection_becomes_a_path() {
    let (_context, mut app) = pen_app();
    assert!(app.path_from_selection().is_err(), "no selection");
    let mut mask = image::GrayImage::new(100, 80);
    for y in 10..30 {
        for x in 20..60 {
            mask.put_pixel(x, y, image::Luma([255]));
        }
    }
    app.session_mut().unwrap().document.selection = Some(std::sync::Arc::new(mask));
    let start = revision(&app);
    let id = app.path_from_selection().unwrap();
    assert_eq!(revision(&app), start + 1);
    let path = edit_path(&app, 0);
    assert_eq!(paths(&app)[0].id, id);
    assert_eq!(path.subpaths.len(), 1);
    let mut anchors = points(&path);
    anchors.sort_by(|a, b| (a.x, a.y).partial_cmp(&(b.x, b.y)).unwrap());
    assert_eq!(
        anchors,
        [(20.0, 10.0), (20.0, 30.0), (60.0, 10.0), (60.0, 30.0)].map(|(x, y)| KPoint::new(x, y))
    );
}

#[test]
fn stroke_path_can_paint_with_the_pencil() {
    let (_context, mut app) = pen_app();
    let id = app.add_path("M 10 10 C 30 0 50 40 70 30").unwrap();
    app.brush.color = [0, 0, 255, 255];
    app.brush.diameter = 3.0;
    app.brush.hardness = 0.3;
    let edit = paths_dialog::PathsEdit {
        pencil: true,
        ..Default::default()
    };
    app.path_action(id, paths_dialog::PathAction::Stroke, &edit)
        .unwrap();
    let document = &app.session().unwrap().document;
    let pixels = document.active().unwrap().pixels.clone().unwrap();
    let alphas: Vec<u8> = pixels.pixels().map(|p| p[3]).collect();
    assert!(alphas.contains(&255));
    assert!(
        alphas.iter().all(|&a| a == 0 || a == 255),
        "the Pencil paints hard pixels"
    );
}

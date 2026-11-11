//! Select → Paths…: adding paths and filling, stroking, selecting and making shape layers
//! from them, each one undo step.
use super::*;
use paths_dialog::{PathAction, PathsEdit};

fn pixels(app: &EditorApp) -> RgbaImage {
    let document = &app.session().unwrap().document;
    (**document.active().unwrap().pixels.as_ref().unwrap()).clone()
}

#[test]
fn paths_are_added_and_filled_stroked_selected_and_shaped() {
    let (context, mut app) = app();
    app.dimensions = [40, 30];
    app.new_document();
    assert!((commands::find("paths").unwrap().enabled)(&app));
    app.command("paths");
    assert_eq!(app.dialog, Some(Dialog::Paths));
    // The dialog draws without a path and with one.
    context.begin_pass(egui::RawInput::default());
    app.paths_dialog(&context);
    let _ = context.end_pass();
    assert_eq!(app.dialog, Some(Dialog::Paths));

    // A malformed path is explained and adds nothing.
    let error = app.add_path("M 0 0 L 5").unwrap_err().to_string();
    assert!(
        error.contains("expected the y of the line's end"),
        "{error}"
    );
    assert!(app.session().unwrap().document.paths.is_empty());

    let square = app.add_path("M 5 5 H 15 V 15 H 5 Z").unwrap();
    let curve = app.add_path("M 20 25 C 20 5 38 5 38 25").unwrap();
    let paths = &app.session().unwrap().document.paths;
    assert_eq!(
        paths.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Path 1", "Path 2"]
    );
    context.begin_pass(egui::RawInput::default());
    app.paths_dialog(&context);
    let _ = context.end_pass();

    let edit = PathsEdit::default();
    app.brush.color = [255, 0, 0, 255];
    let revision = app.session().unwrap().history.revision;
    app.path_action(square, PathAction::Fill, &edit).unwrap();
    assert_eq!(pixels(&app).get_pixel(10, 10).0, [255, 0, 0, 255]);
    assert_eq!(pixels(&app).get_pixel(20, 10)[3], 0);
    assert_eq!(app.session().unwrap().history.revision, revision + 1);

    app.brush.color = [0, 0, 255, 255];
    app.brush.diameter = 3.0;
    app.brush.hardness = 1.0;
    app.path_action(curve, PathAction::Stroke, &edit).unwrap();
    // On the curve (its top is at y = 10), not along the chord between its ends.
    let on_curve = pixels(&app).get_pixel(29, 10).0;
    assert!(on_curve[2] == 255 && on_curve[3] > 200, "{on_curve:?}");
    assert_eq!(pixels(&app).get_pixel(29, 24)[3], 0);

    let feathered = PathsEdit {
        feather: 2.0,
        mode: SelectionMode::Add,
        ..PathsEdit::default()
    };
    app.path_action(square, PathAction::Select, &feathered)
        .unwrap();
    let selection = app.session().unwrap().document.selection.clone().unwrap();
    assert!(selection.get_pixel(10, 10)[0] > 240);
    let edge = selection.get_pixel(5, 10)[0];
    assert!(edge > 0 && edge < 255, "feathered edge {edge}");

    let layers = app.session().unwrap().document.layers.len();
    app.path_action(curve, PathAction::ShapeLayer, &edit)
        .unwrap();
    let document = &app.session().unwrap().document;
    assert_eq!(document.layers.len(), layers + 1);
    let shape = document.active().unwrap().shape.as_ref().unwrap();
    assert_eq!(shape.kind, xuan::paint::ShapeKind::Path);
    assert_eq!(shape.color, [0, 0, 255, 255]);

    let renamed = PathsEdit {
        name: "Hill".into(),
        ..PathsEdit::default()
    };
    app.path_action(curve, PathAction::Rename, &renamed)
        .unwrap();
    assert_eq!(app.session().unwrap().document.paths[1].name, "Hill");
    app.path_action(square, PathAction::Delete, &edit).unwrap();
    assert_eq!(app.session().unwrap().document.paths.len(), 1);
    // Undo brings the deleted path back.
    app.command("undo");
    assert_eq!(app.session().unwrap().document.paths.len(), 2);

    // Escape closes the dialog.
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(egui::Key::Escape, egui::Modifiers::NONE)],
        egui::Modifiers::NONE,
    );
    assert_eq!(app.dialog, None);
}

/// The Text window sets text along a document path, edits how it follows it and puts it
/// back in a box, each applied as one undo step.
#[test]
fn text_is_set_along_a_document_path_in_the_text_window() {
    let (context, mut app) = app();
    app.dimensions = [320, 200];
    app.new_document();
    // Without paths, the window says where to make one.
    app.start_text(None, Point::new(20.0, 20.0));
    // Windows are measured on their first frame and drawn from the second.
    frame(&context, &mut app);
    let output = frame(&context, &mut app);
    let texts = shown_text(&output);
    assert!(
        texts.iter().any(|t| t.contains("Select → Paths…")),
        "{texts:?}"
    );
    app.finish_text(false);

    let d = "M 20 150 C 80 40 240 40 300 150";
    app.add_path(d).unwrap();
    app.start_text(None, Point::new(20.0, 20.0));
    let id = app.text_edit.as_ref().unwrap().target;
    app.attach_text_path(Some(0));
    {
        let style = &mut app.text_edit.as_mut().unwrap().style;
        style.content = "Over the hill".into();
        let options = &mut style.path.as_mut().unwrap().options;
        options.start_offset = 50.0;
        options.align = xuan::text::PathAlign::Center;
        options.size_end = Some(20.0);
    }
    app.preview_text();
    frame(&context, &mut app);
    let output = frame(&context, &mut app);
    assert!(app.text_edit.as_ref().unwrap().error.is_none());
    let texts = shown_text(&output);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Turn letters with the path")),
        "{texts:?}"
    );
    app.finish_text(true);
    let layer = |app: &EditorApp| {
        app.session()
            .unwrap()
            .document
            .layers
            .iter()
            .find(|l| l.id == id)
            .unwrap()
            .clone()
    };
    let placed = layer(&app);
    let path = placed.text.as_ref().unwrap().path.clone().unwrap();
    let bounds = path
        .in_document(placed.transform)
        .unwrap()
        .bounds()
        .unwrap();
    let expected = xuan::vector::VectorPath::parse(d)
        .unwrap()
        .bounds()
        .unwrap();
    assert!((bounds.x0 - expected.x0).abs() < 0.01 && (bounds.y1 - expected.y1).abs() < 0.01);
    assert_eq!(path.options.size_end, Some(20.0));
    // New text still starts in a box.
    assert!(app.text_style.path.is_none());
    assert_eq!(app.session().unwrap().history.names().count(), 2);

    // Editing it again keeps the path; detaching puts the text in a box.
    app.start_text(Some(id), Point::default());
    app.text_edit
        .as_mut()
        .unwrap()
        .style
        .path
        .as_mut()
        .unwrap()
        .options
        .side = xuan::text::PathSide::Right;
    app.preview_text();
    app.finish_text(true);
    let flipped = layer(&app);
    assert_ne!(flipped.pixels, placed.pixels);
    app.start_text(Some(id), Point::default());
    app.attach_text_path(None);
    app.preview_text();
    app.finish_text(true);
    assert!(layer(&app).text.as_ref().unwrap().path.is_none());
    app.command("undo");
    assert_eq!(layer(&app).pixels, flipped.pixels);
    app.command("undo");
    assert_eq!(layer(&app).pixels, placed.pixels);
    assert_eq!(layer(&app).text, placed.text);
}

fn shown_text(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

//! Plugin edits that take SVG path data: `select_path`, `stroke` with `path`, `fill_path`,
//! `add_shape_layer` with the Path shape and `add_path`.
use super::*;

fn run(document: &mut Document, edits: &[Value]) -> Result<Vec<Uuid>> {
    let edits = parse_edits(Value::Array(edits.to_vec()))?;
    apply(
        document,
        &edits,
        &mut Reader::new(Access::anywhere(), crate::document::MAX_LAYERS),
    )
}

fn error(document: &Document, edit: Value) -> String {
    format!("{:#}", run(&mut document.clone(), &[edit]).unwrap_err())
}

/// A 40 × 30 document with one transparent canvas-sized pixel layer.
fn canvas() -> Document {
    let mut document = Document::new(40, 30).unwrap();
    document.layers[0].pixels = Some(Arc::new(RgbaImage::new(40, 30)));
    document
}

fn selection(document: &Document) -> &GrayImage {
    document.selection.as_deref().unwrap()
}

#[test]
fn select_path_selects_inside_a_closed_path_with_antialiased_curves() {
    let mut document = canvas();
    run(
        &mut document,
        &[json!({"op": "select_path", "path": "M 5 5 H 15 V 15 H 5 Z"})],
    )
    .unwrap();
    let mask = selection(&document);
    assert_eq!(mask.get_pixel(10, 10)[0], 255, "inside");
    assert_eq!(mask.get_pixel(4, 10)[0], 0, "outside");
    assert_eq!(mask.get_pixel(15, 10)[0], 0, "outside");
    // A circle: its edge pixels are partly selected.
    run(
        &mut document,
        &[json!({"op": "select_path", "path": "M 10 15 a 10 10 0 1 0 20 0 a 10 10 0 1 0 -20 0 z"})],
    )
    .unwrap();
    let mask = selection(&document);
    assert_eq!(mask.get_pixel(20, 15)[0], 255);
    assert_eq!(mask.get_pixel(2, 2)[0], 0);
    let partial = mask.pixels().filter(|p| p[0] > 0 && p[0] < 255).count();
    assert!(partial > 20, "{partial} antialiased edge pixels");

    // Combined with the selection, like the other shapes.
    run(
        &mut document,
        &[json!({"op": "select_path", "path": "M 0 0 H 20 V 30 H 0 Z", "mode": "subtract"})],
    )
    .unwrap();
    let mask = selection(&document);
    assert_eq!(mask.get_pixel(15, 15)[0], 0);
    assert_eq!(mask.get_pixel(25, 15)[0], 255);
    run(
        &mut document,
        &[json!({"op": "select_path", "path": "M 0 0 H 40 V 15 H 0 Z", "mode": "intersect"})],
    )
    .unwrap();
    assert_eq!(selection(&document).get_pixel(25, 20)[0], 0);
    assert_eq!(selection(&document).get_pixel(25, 10)[0], 255);
    run(
        &mut document,
        &[json!({"op": "select_path", "path": "M 0 0 H 4 V 4 H 0 Z", "mode": "add"})],
    )
    .unwrap();
    assert_eq!(selection(&document).get_pixel(1, 1)[0], 255);
    assert_eq!(selection(&document).get_pixel(25, 10)[0], 255);

    // Even-odd cuts the inner square out.
    run(
        &mut document,
        &[json!({"op": "select_path", "path": "M 0 0 H 30 V 30 H 0 Z M 10 10 H 20 V 20 H 10 Z", "fill_rule": "evenodd"})],
    )
    .unwrap();
    assert_eq!(selection(&document).get_pixel(15, 15)[0], 0);
    assert_eq!(selection(&document).get_pixel(5, 5)[0], 255);
}

#[test]
fn feather_softens_only_the_new_shape() {
    for op in [
        json!({"op": "select_path", "path": "M 20 0 H 40 V 30 H 20 Z", "mode": "add", "feather": 3}),
        json!({"op": "select_rect", "x": 20, "y": 0, "width": 20, "height": 30, "mode": "add", "feather": 3}),
        json!({"op": "select_polygon", "points": [[20, 0], [40, 0], [40, 30], [20, 30]], "mode": "add", "feather": 3}),
    ] {
        let mut document = canvas();
        // A hard-edged selection on the left stays hard.
        run(
            &mut document,
            &[json!({"op": "select_rect", "x": 0, "y": 0, "width": 8, "height": 30})],
        )
        .unwrap();
        run(&mut document, std::slice::from_ref(&op)).unwrap();
        let mask = selection(&document);
        assert_eq!(mask.get_pixel(7, 15)[0], 255, "{op}");
        assert_eq!(mask.get_pixel(8, 15)[0], 0, "{op}");
        // The new edge at x = 20 is soft.
        let edge = mask.get_pixel(19, 15)[0];
        assert!(edge > 0 && edge < 255, "{op}: {edge}");
        assert!(mask.get_pixel(21, 15)[0] < 255, "{op}");
        assert_eq!(mask.get_pixel(30, 15)[0], 255, "{op}");
    }
    let document = canvas();
    assert!(
        error(
            &document,
            json!({"op": "select_path", "path": "M 0 0 H 5 V 5 Z", "feather": 300})
        )
        .contains("feather radius must be between 0 and 256")
    );
}

#[test]
fn path_errors_are_clear_and_name_the_edit() {
    let document = canvas();
    assert_eq!(
        error(
            &document,
            json!({"op": "select_path", "path": "M 0 0 L 10"})
        ),
        "SVG path: expected the y of the line's end for `L` at character 11, found the end of the path"
    );
    let two = run(
        &mut document.clone(),
        &[
            json!({"op": "select_rect", "x": 0, "y": 0, "width": 5, "height": 5}),
            json!({"op": "fill_path", "path": "Q 1 1 2 2", "color": "#ff0000"}),
        ],
    )
    .unwrap_err();
    assert_eq!(
        format!("{two:#}"),
        "Edit 2 (fill_path): SVG path: a path must start with a move (`M x y`), found `Q` at character 1"
    );
}

#[test]
fn a_stroke_follows_a_path() {
    let mut document = canvas();
    // A curve from (5, 25) up through the top and down to (35, 25).
    run(
        &mut document,
        &[json!({
            "op": "stroke", "path": "M 5 25 C 5 0 35 0 35 25", "size": 3, "hardness": 1,
            "color": "#ff0000",
        })],
    )
    .unwrap();
    let pixels = document.layers[0].pixels.as_ref().unwrap();
    // The curve's middle is at (20, 6.25); the straight chord between its ends is not painted.
    assert!(pixels.get_pixel(20, 6)[3] > 200, "on the curve");
    assert_eq!(pixels.get_pixel(20, 25)[3], 0, "the chord is not painted");
    assert!(pixels.get_pixel(5, 24)[3] > 200, "its start");
    assert!(pixels.get_pixel(35, 24)[3] > 200, "its end");
    assert_eq!(pixels.get_pixel(20, 15)[3], 0, "inside is not filled");

    // The same stroke from `points` and from `path` paints the same pixels.
    let mut from_points = canvas();
    let mut from_path = canvas();
    run(
        &mut from_points,
        &[json!({"op": "stroke", "points": [[2, 2], [30, 2], [30, 20]], "size": 4})],
    )
    .unwrap();
    run(
        &mut from_path,
        &[json!({"op": "stroke", "path": "M 2 2 H 30 V 20", "size": 4})],
    )
    .unwrap();
    assert_eq!(
        from_points.layers[0].pixels.as_ref().unwrap().as_raw(),
        from_path.layers[0].pixels.as_ref().unwrap().as_raw()
    );

    // Brush dynamics apply to path strokes: a taper thins the ends.
    let mut tapered = canvas();
    run(
        &mut tapered,
        &[json!({"op": "stroke", "path": "M 2 15 H 38", "size": 9, "hardness": 1, "taper_in": 15, "taper_out": 15})],
    )
    .unwrap();
    let pixels = tapered.layers[0].pixels.as_ref().unwrap();
    assert!(pixels.get_pixel(20, 18)[3] > 0, "full width in the middle");
    assert_eq!(pixels.get_pixel(4, 18)[3], 0, "thin at the start");

    for (bad, says) in [
        (
            json!({"op": "stroke", "path": "M 0 0 L 5 5", "points": [[1, 1]]}),
            "Give a stroke `points` or a `path`, not both",
        ),
        (
            json!({"op": "stroke", "path": "M 0 0 L 5 5 M 9 9 L 10 10"}),
            "this path has 2 (each M or m starts one)",
        ),
        (
            json!({"op": "stroke", "path": "M 0 0 L 5 x"}),
            "SVG path: expected the y of the line's end",
        ),
        (json!({"op": "stroke"}), "A stroke needs at least one point"),
    ] {
        let message = error(&canvas(), bad.clone());
        assert!(message.contains(says), "{bad}: {message}");
    }
    // The flattened path counts against the stroke-length budget.
    let long = format!("M 0 0{}", " L 30000 0 L 0 0".repeat(5));
    let cost = cost(
        &canvas(),
        &parse_edits(json!([{"op": "stroke", "path": long}])).unwrap(),
    );
    assert_eq!(cost.stroke, 300_000.0);
}

#[test]
fn filling_many_tall_edges_counts_against_the_work_budget() {
    // A comb of 5,000 teeth as tall as the canvas: small in area, slow to fill.
    let comb: String = (0..5000)
        .map(|i| format!(" L {i} 30000 L {i}.5 0"))
        .collect();
    let tall = Document::new(100, 30_000).unwrap();
    let path = format!("M 0 0{comb} Z");
    for op in [
        json!({"op": "select_path", "path": path}),
        json!({"op": "fill_path", "path": path, "color": "#000000"}),
        json!({"op": "add_shape_layer", "shape": "Path", "path": path}),
    ] {
        let edits = parse_edits(json!([op])).unwrap();
        assert!(cost(&tall, &edits).work > MAX_WORK, "{op}");
        // On a short canvas only the rows filled count; a shape layer is as tall as its path.
        if op["op"] != "add_shape_layer" {
            assert!(cost(&canvas(), &edits).work < MAX_WORK / 10, "{op}");
        }
    }
}

#[test]
fn fill_path_fills_the_inside_within_the_selection() {
    let mut document = canvas();
    run(
        &mut document,
        &[json!({"op": "fill_path", "path": "M 5 5 H 15 V 15 H 5 Z", "color": "#00ff00"})],
    )
    .unwrap();
    let pixels = document.layers[0].pixels.as_ref().unwrap();
    assert_eq!(pixels.get_pixel(10, 10).0, [0, 255, 0, 255]);
    assert_eq!(pixels.get_pixel(16, 10)[3], 0);
    // Within a selection, only where both are; the selection is kept.
    let mut document = canvas();
    run(
        &mut document,
        &[
            json!({"op": "select_rect", "x": 10, "y": 0, "width": 30, "height": 30}),
            json!({"op": "fill_path", "path": "M 5 5 H 15 V 15 H 5 Z", "color": "#0000ff"}),
        ],
    )
    .unwrap();
    let pixels = document.layers[0].pixels.as_ref().unwrap();
    assert_eq!(pixels.get_pixel(7, 10)[3], 0);
    assert_eq!(pixels.get_pixel(12, 10).0, [0, 0, 255, 255]);
    assert_eq!(selection(&document).get_pixel(30, 25)[0], 255);
    // A curved edge is antialiased.
    let mut document = canvas();
    run(
        &mut document,
        &[json!({"op": "fill_path", "path": "M 5 15 a 10 10 0 1 0 20 0 a 10 10 0 1 0 -20 0", "color": "#000000"})],
    )
    .unwrap();
    let pixels = document.layers[0].pixels.as_ref().unwrap();
    assert!(pixels.pixels().any(|p| p[3] > 0 && p[3] < 255));
}

#[test]
fn path_shape_layers_are_live_antialiased_and_described() {
    let mut document = canvas();
    let d = "M 10 20 C 10 5 30 5 30 20 Z";
    let added = run(
        &mut document,
        &[json!({"op": "add_shape_layer", "shape": "Path", "path": d, "color": "#ff000080", "name": "Dome"})],
    )
    .unwrap();
    let layer = document.layers.iter().find(|l| l.id == added[0]).unwrap();
    assert_eq!(layer.name, "Dome");
    // The layer covers the curve's whole-pixel bounds: y from 8.75 (the curve's top) to 20.
    assert_eq!(
        (
            layer.transform.x,
            layer.transform.y,
            layer.transform.width,
            layer.transform.height
        ),
        (10.0, 8.0, 20.0, 12.0)
    );
    let pixels = layer.pixels.as_ref().unwrap();
    assert_eq!(pixels.get_pixel(10, 10).0, [255, 0, 0, 128]);
    assert_eq!(pixels.get_pixel(0, 0)[3], 0);
    assert!(
        pixels.pixels().any(|p| p[3] > 0 && p[3] < 128),
        "antialiased"
    );
    let described = describe(&document)["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == json!(added[0]))
        .cloned()
        .unwrap();
    assert_eq!(described["kind"], "shape");
    assert_eq!(described["shape"]["shape"], "Path");
    assert_eq!(described["shape"]["fill_rule"], "nonzero");
    assert_eq!(described["shape"]["path"], "M10,20 C10,5 30,5 30,20 Z");
    assert_eq!(described["shape"]["local_size"], json!([20.0, 12.0]));

    // Moved and stretched, it is redrawn from its outline, and described where it is now.
    run(
        &mut document,
        &[json!({"op": "transform", "layer": added[0], "x": 0, "y": 0, "width": 40, "height": 24})],
    )
    .unwrap();
    let layer = document.layers.iter().find(|l| l.id == added[0]).unwrap();
    assert_eq!(layer.pixels.as_ref().unwrap().dimensions(), (40, 24));
    assert!(layer.shape.is_some(), "still a live shape");
    let described = describe(&document)["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == json!(added[0]))
        .cloned()
        .unwrap();
    assert_eq!(described["shape"]["path"], "M0,24 C0,-6 40,-6 40,24 Z");

    for (bad, says) in [
        (
            json!({"op": "add_shape_layer", "shape": "Path"}),
            "A path shape needs `path`",
        ),
        (
            json!({"op": "add_shape_layer", "shape": "Path", "path": d, "x": 3}),
            "leave out x, y, width and height",
        ),
        (
            json!({"op": "add_shape_layer", "shape": "Ellipse", "path": d, "x": 0, "y": 0, "width": 5, "height": 5}),
            "`path` goes only with the Path shape",
        ),
        (
            json!({"op": "add_shape_layer", "shape": "Ellipse", "x": 0, "y": 0}),
            "needs x, y, width and height",
        ),
        (
            json!({"op": "add_shape_layer", "shape": "Path", "path": "M 0 0 L 10 0"}),
            "The path encloses no area",
        ),
        (
            json!({"op": "add_shape_layer", "shape": "Path", "path": "M 0 0 H 40000 V 10 Z"}),
            "Dimensions must be between 1 and 30000 pixels",
        ),
    ] {
        let message = error(&canvas(), bad.clone());
        assert!(message.contains(says), "{bad}: {message}");
    }
}

#[test]
fn paths_are_saved_with_the_document_and_described() {
    let mut document = canvas();
    run(
        &mut document,
        &[
            json!({"op": "add_path", "path": "M 0 0 L 10 10"}),
            json!({"op": "add_path", "path": "M 1 1 L 5 5", "name": "Hill"}),
            json!({"op": "add_path", "path": "M 2 2 L 6 6"}),
            // The same name replaces the path.
            json!({"op": "add_path", "path": "M 3 3 L 7 7", "name": "Hill"}),
        ],
    )
    .unwrap();
    let paths = describe(&document)["paths"].clone();
    let names: Vec<_> = paths
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].clone(), p["path"].clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            (json!("Path 1"), json!("M0,0 L10,10")),
            (json!("Hill"), json!("M3,3 L7,7")),
            (json!("Path 2"), json!("M2,2 L6,6")),
        ]
    );
    assert!(
        error(
            &document,
            json!({"op": "add_path", "path": "M 0 0", "name": " "})
        )
        .contains("1 to 256 bytes")
    );
    // Paths follow the content when the canvas changes, as guides do.
    run(
        &mut document,
        &[
            json!({"op": "crop", "x": 1, "y": 1, "width": 20, "height": 20}),
            json!({"op": "resize_image", "width": 40, "height": 20}),
        ],
    )
    .unwrap();
    assert_eq!(document.paths[0].d.to_svg(), "M-2,-1 L18,9");
    crate::operations::flip_canvas(&mut document, true);
    assert_eq!(document.paths[0].d.to_svg(), "M42,-1 L22,9");
}

/// The layer `id` as `describe` reports it.
fn described(document: &Document, id: Uuid) -> Value {
    describe(document)["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == json!(id))
        .cloned()
        .unwrap()
}

/// Whether the SVG path data `a` and `b` bound the same area, within a hundredth of a pixel.
fn same_bounds(a: &Value, b: &str) -> bool {
    let a = VectorPath::parse(a.as_str().unwrap())
        .unwrap()
        .bounds()
        .unwrap();
    let b = VectorPath::parse(b).unwrap().bounds().unwrap();
    [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)]
        .iter()
        .all(|(x, y)| (x - y).abs() < 0.01)
}

#[test]
fn text_layers_follow_paths_and_change_their_options() {
    let mut document = Document::new(300, 200).unwrap();
    let arch = "M 20 150 C 80 40 220 40 280 150";
    let added = run(
        &mut document,
        &[json!({"op": "add_text_layer", "text": "Up and over", "size": 24, "path": arch,
                 "path_options": {"start_offset": 50, "align": "center", "letter_spacing": 2,
                                  "size_end": 12, "opacity_start": 0.95, "opacity_end": 0.55}})],
    )
    .unwrap();
    let layer = document.layers.iter().find(|l| l.id == added[0]).unwrap();
    let stored = layer.text.as_ref().unwrap().path.as_ref().unwrap();
    assert_eq!(stored.options.align, crate::text::PathAlign::Center);
    assert_eq!(stored.options.size_end, Some(12.0));
    assert!(stored.options.rotate, "defaults fill the options left out");
    assert_eq!(layer.name, "Up and over");
    let text = &described(&document, added[0])["text"];
    assert_eq!(described(&document, added[0])["kind"], "text");
    assert_eq!(text["text"], "Up and over");
    assert_eq!(text["size"], 24.0);
    assert!(same_bounds(&text["path"], arch), "{}", text["path"]);
    assert_eq!(text["path_options"]["letter_spacing"], 2.0);
    assert_eq!(text["path_options"]["side"], "left");
    let first = layer.pixels.clone().unwrap();

    // Partial options keep the rest; the path stays where it was.
    run(
        &mut document,
        &[json!({"op": "set_text", "layer": added[0], "path_options": {"side": "right"}})],
    )
    .unwrap();
    let layer = document.layers.iter().find(|l| l.id == added[0]).unwrap();
    let options = &layer.text.as_ref().unwrap().path.as_ref().unwrap().options;
    assert_eq!(options.side, crate::text::PathSide::Right);
    assert_eq!((options.letter_spacing, options.size_end), (2.0, Some(12.0)));
    assert_ne!(layer.pixels.as_ref().unwrap(), &first);
    assert!(same_bounds(
        &described(&document, added[0])["text"]["path"],
        arch
    ));

    // A new path and text move the text; `null` puts it back in a box.
    let line = "M 10 180 L 290 180";
    run(
        &mut document,
        &[json!({"op": "set_text", "layer": added[0], "path": line, "text": "Flat"})],
    )
    .unwrap();
    let text = &described(&document, added[0])["text"];
    assert!(same_bounds(&text["path"], line), "{}", text["path"]);
    assert_eq!(text["text"], "Flat");
    assert_eq!(text["path_options"]["side"], "right", "options carry over");
    run(
        &mut document,
        &[json!({"op": "set_text", "layer": added[0], "path": null})],
    )
    .unwrap();
    let text = &described(&document, added[0])["text"];
    assert!(text.get("path").is_none() && text["text"] == "Flat");

    // Box text can be put on a path, with options of its own.
    let boxed = run(
        &mut document,
        &[json!({"op": "add_text_layer", "text": "Boxed", "x": 40, "y": 20})],
    )
    .unwrap()[0];
    run(
        &mut document,
        &[json!({"op": "set_text", "layer": boxed, "path": arch, "path_options": {"rotate": false}})],
    )
    .unwrap();
    let text = &described(&document, boxed)["text"];
    assert!(same_bounds(&text["path"], arch));
    assert_eq!(text["path_options"]["rotate"], false);
    assert_eq!(text["path_options"]["align"], "start");

    let image = document.layers[0].id;
    for (bad, says) in [
        (
            json!({"op": "add_text_layer", "text": "x", "path": arch, "x": 3}),
            "leave out x and y",
        ),
        (
            json!({"op": "add_text_layer", "text": "x", "path_options": {"align": "end"}}),
            "only with `path`",
        ),
        (
            json!({"op": "add_text_layer", "text": "x", "path": "M 0 0 L"}),
            "SVG path",
        ),
        (
            json!({"op": "add_text_layer", "text": "x", "path": arch, "path_options": {"opacity_end": 2}}),
            "Opacity",
        ),
        (
            json!({"op": "set_text", "layer": added[0], "path_options": {"align": "end"}}),
            "not on a path",
        ),
        (
            json!({"op": "set_text", "layer": boxed, "path_options": {"align": "middle"}}),
            "Invalid `path_options`",
        ),
        (
            json!({"op": "set_text", "layer": image, "text": "x"}),
            "not a text layer",
        ),
    ] {
        let error = error(&document, bad.clone());
        assert!(error.contains(says), "{bad}: {error}");
    }
    document
        .layers
        .iter_mut()
        .find(|l| l.id == boxed)
        .unwrap()
        .locked = true;
    let error = error(
        &document,
        json!({"op": "set_text", "layer": boxed, "text": "y"}),
    );
    assert!(error.contains("locked"), "{error}");
}

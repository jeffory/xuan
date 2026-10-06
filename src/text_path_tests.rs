//! Text on a path: layout along straight lines, arcs and closed paths, the options, and
//! keeping the path with the layer.
use super::*;
use crate::{document::Document, io, render};

fn renderer() -> TextRenderer {
    let mut db = cosmic_text::fontdb::Database::new();
    db.load_font_data(include_bytes!("../assets/fonts/InterVariable.ttf").to_vec());
    TextRenderer::with_fonts(FontSystem::new_with_locale_and_db("en-US".into(), db))
}

fn path(d: &str) -> VectorPath {
    VectorPath::parse(d).unwrap()
}

/// `content` at `size` laid out with `options`.
fn style(content: &str, size: f32, options: PathTextOptions) -> TextStyle {
    TextStyle {
        content: content.into(),
        size,
        path: Some(TextPath {
            d: path("M 0 0 L 1 0"),
            width: 1.0,
            height: 1.0,
            options,
        }),
        ..Default::default()
    }
}

/// The glyphs of the first line laid out on a straight line, and its width.
fn straight(renderer: &mut TextRenderer, style: &TextStyle) -> (Vec<cosmic_text::LayoutGlyph>, f64) {
    let buffer = renderer.shape(style).unwrap();
    let run = buffer.layout_runs().next().unwrap();
    (run.glyphs.to_vec(), f64::from(run.line_w))
}

/// Where the glyph's line would start: its point on the path less its offset into the line.
fn line_start(placed: &PathGlyph) -> f64 {
    placed.on_path.x - f64::from(placed.glyph.x + placed.glyph.w / 2.0)
}

/// The bounds of the pixels with any ink and their total alpha.
fn ink(image: &RgbaImage) -> ((u32, u32, u32, u32), u64) {
    let (mut x0, mut y0, mut x1, mut y1, mut sum) = (u32::MAX, u32::MAX, 0, 0, 0_u64);
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] > 0 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            sum += u64::from(pixel[3]);
        }
    }
    ((x0, y0, x1, y1), sum)
}

#[test]
fn glyphs_on_a_straight_path_sit_where_text_in_a_box_does() {
    let mut renderer = renderer();
    let style = style("Wavy fj Å!", 40.0, PathTextOptions::default());
    let line = path("M 10 100 L 900 100");
    let glyphs = renderer.layout_on_path(&style, &line).unwrap();
    let (laid, _) = straight(&mut renderer, &style);
    assert_eq!(glyphs.len(), laid.len());
    for (placed, glyph) in glyphs.iter().zip(&laid) {
        assert!((placed.on_path.x - (10.0 + f64::from(glyph.x + glyph.w / 2.0))).abs() < 1e-3);
        assert_eq!(placed.on_path.y, 100.0);
        assert_eq!((placed.angle, placed.size, placed.opacity), (0.0, 40.0, 1.0));
        // The pen origin lands on the baseline where cosmic-text put it.
        let pen = placed.transform * kurbo::Point::ZERO;
        assert!((pen.x - (10.0 + f64::from(glyph.x))).abs() < 1e-3, "{pen:?}");
        assert!((pen.y - 100.0).abs() < 1e-3, "{pen:?}");
    }

    // The ink matches text in a box: the same size and weight.
    let boxed = renderer.render(&style).unwrap();
    let (pixels, origin) = renderer.render_on_path(&style, &line).unwrap();
    let ((bx0, by0, bx1, by1), boxed_sum) = ink(&boxed);
    let ((px0, py0, px1, py1), path_sum) = ink(&pixels);
    assert!((bx1 - bx0).abs_diff(px1 - px0) <= 1, "{bx0}..{bx1} vs {px0}..{px1}");
    assert!((by1 - by0).abs_diff(py1 - py0) <= 1, "{by0}..{by1} vs {py0}..{py1}");
    let ratio = path_sum as f64 / boxed_sum as f64;
    assert!((0.97..1.03).contains(&ratio), "{ratio}");
    // Its pixels start at the ink: capitals reach about 0.73 em above the baseline.
    let cap_top = origin[1] + py0 as i32;
    assert!((100 - 32..100 - 26).contains(&cap_top), "{cap_top}");
    assert!((origin[0] + px0 as i32 - 10).abs() <= 4, "{origin:?}");
}

#[test]
fn glyphs_on_an_arc_turn_to_its_tangent_and_stand_outside_it() {
    let mut renderer = renderer();
    let style = style("Around the arc", 30.0, PathTextOptions::default());
    // The upper half of a circle of radius 200 around (300, 300), left to right.
    let arc = path("M 100 300 A 200 200 0 0 1 500 300");
    let glyphs = renderer.layout_on_path(&style, &arc).unwrap();
    assert_eq!(glyphs.len(), straight(&mut renderer, &style).0.len());
    let centre = kurbo::Point::new(300.0, 300.0);
    let mut previous = -180.0;
    for placed in &glyphs {
        let r = placed.on_path - centre;
        assert!((r.hypot() - 200.0).abs() < 0.1, "{r:?}");
        // Clockwise around the centre, the tangent is the radius turned a quarter.
        let tangent = r.x.atan2(-r.y).to_degrees();
        assert!((placed.angle - tangent).abs() < 0.5, "{} vs {tangent}", placed.angle);
        assert_eq!(placed.angle, placed.path_angle);
        assert!(placed.angle > previous);
        previous = placed.angle;
        // Above the baseline is outside the circle.
        let above = placed.transform * kurbo::Point::new(0.0, -15.0);
        assert!((above - centre).hypot() > 213.0);
    }
    assert!(glyphs[0].angle < -50.0, "{}", glyphs[0].angle);

    // Upright letters keep angle 0 on the same points.
    let mut upright = style.clone();
    upright.path.as_mut().unwrap().options.rotate = false;
    let still = renderer.layout_on_path(&upright, &arc).unwrap();
    for (a, b) in glyphs.iter().zip(&still) {
        assert_eq!(a.on_path, b.on_path);
        assert_eq!(b.angle, 0.0);
        assert_eq!(a.path_angle, b.path_angle);
    }
    let (pixels, _) = renderer.render_on_path(&style, &arc).unwrap();
    assert!(pixels.pixels().any(|p| p[3] == 255));
}

#[test]
fn alignment_and_start_offset_place_the_line() {
    let mut renderer = renderer();
    let line = path("M 0 50 L 1000 50");
    let (_, width) = straight(&mut renderer, &style("Align me", 24.0, Default::default()));
    for (align, offset, start) in [
        (PathAlign::Start, 10.0, 100.0),
        (PathAlign::Center, 50.0, 500.0 - width / 2.0),
        (PathAlign::End, 90.0, 900.0 - width),
        (PathAlign::Start, 0.0, 0.0),
    ] {
        let options = PathTextOptions {
            align,
            start_offset: offset,
            ..Default::default()
        };
        let glyphs = renderer
            .layout_on_path(&style("Align me", 24.0, options), &line)
            .unwrap();
        assert_eq!(glyphs.len(), 8);
        for placed in &glyphs {
            assert!((line_start(placed) - start).abs() < 1e-3, "{align:?}");
        }
    }

    // Letter spacing adds after each letter.
    let spaced = PathTextOptions {
        letter_spacing: 5.0,
        ..Default::default()
    };
    let glyphs = renderer
        .layout_on_path(&style("Align me", 24.0, spaced), &line)
        .unwrap();
    for (i, placed) in glyphs.iter().enumerate() {
        assert!((line_start(placed) - 5.0 * i as f64).abs() < 1e-3);
    }
    // Ending at the path's start, nothing is left on an open path.
    let before = PathTextOptions {
        align: PathAlign::End,
        ..Default::default()
    };
    let style = style("Align me", 24.0, before);
    assert!(renderer.layout_on_path(&style, &line).unwrap().is_empty());
    // Nothing to draw still makes a pixel, at the path's start.
    let (pixels, origin) = renderer.render_on_path(&style, &line).unwrap();
    assert_eq!((pixels.dimensions(), origin), ((1, 1), [0, 50]));
}

#[test]
fn flipping_runs_the_text_back_along_the_other_side() {
    let mut renderer = renderer();
    let line = path("M 0 50 L 1000 50");
    let options = PathTextOptions {
        side: PathSide::Right,
        ..Default::default()
    };
    let glyphs = renderer
        .layout_on_path(&style("Flip", 30.0, options), &line)
        .unwrap();
    assert_eq!(glyphs.len(), 4);
    let first = &glyphs[0];
    let centre = f64::from(first.glyph.x + first.glyph.w / 2.0);
    assert!((first.on_path.x - (1000.0 - centre)).abs() < 1e-3);
    for pair in glyphs.windows(2) {
        assert!(pair[1].on_path.x < pair[0].on_path.x);
    }
    for placed in &glyphs {
        assert!((placed.angle.abs() - 180.0).abs() < 1e-9);
        // Upside down, under the line.
        assert!((placed.transform * kurbo::Point::new(0.0, -15.0)).y > 60.0);
    }

    // A baseline shift lifts letters off the path along its normal, here downwards.
    let shifted = PathTextOptions {
        side: PathSide::Right,
        baseline_shift: 8.0,
        ..Default::default()
    };
    let lifted = renderer
        .layout_on_path(&style("Flip", 30.0, shifted), &line)
        .unwrap();
    let pen = |p: &PathGlyph| p.transform * kurbo::Point::ZERO;
    assert!((pen(&lifted[0]).y - 58.0).abs() < 1e-9);
    assert!((pen(&glyphs[0]).y - 50.0).abs() < 1e-9);
}

#[test]
fn size_and_opacity_ramp_from_the_first_letter_to_the_last() {
    let mut renderer = renderer();
    let line = path("M 0 100 L 2000 100");
    // The issue's picture: 17 letters shrinking from 17 to 7 px and fading from 95% to 55%.
    let options = PathTextOptions {
        size_end: Some(7.0),
        opacity_start: 0.95,
        opacity_end: 0.55,
        ..Default::default()
    };
    let style = style("ABCDEFGHIJKLMNOPQ", 17.0, options);
    let glyphs = renderer.layout_on_path(&style, &line).unwrap();
    assert_eq!(glyphs.len(), 17);
    for pair in glyphs.windows(2) {
        assert!(pair[1].size < pair[0].size);
        assert!(pair[1].opacity < pair[0].opacity);
        assert!(pair[1].on_path.x > pair[0].on_path.x);
    }
    let (first, last) = (&glyphs[0], &glyphs[16]);
    assert!((16.0..17.0).contains(&first.size), "{}", first.size);
    assert!((7.0..8.0).contains(&last.size), "{}", last.size);
    assert!((0.92..0.95).contains(&first.opacity), "{}", first.opacity);
    assert!((0.55..0.58).contains(&last.opacity), "{}", last.opacity);
    // The letters close up as they shrink: the line ends at the integral of the scale, the
    // average of 17 and 7 px over 17 px of the straight line's width.
    let (_, width) = straight(&mut renderer, &style);
    let end = last.on_path.x + f64::from(last.glyph.w / 2.0) * f64::from(last.size / 17.0);
    assert!((end - width * 12.0 / 17.0).abs() < 1e-3, "{end} vs {width}");
    // Drawn, the first letters are more opaque than the last.
    let (pixels, _) = renderer.render_on_path(&style, &line).unwrap();
    let strongest = |range: std::ops::Range<u32>| {
        pixels
            .enumerate_pixels()
            .filter(|(x, ..)| range.contains(x))
            .map(|(.., p)| p[3])
            .max()
            .unwrap()
    };
    let w = pixels.width();
    let (left, right) = (strongest(0..w / 4), strongest(w * 3 / 4..w));
    assert!((200..=243).contains(&left), "{left}");
    assert!(right <= 153 && right < left, "{right}");
}

#[test]
fn letters_past_the_end_of_an_open_path_are_hidden() {
    let mut renderer = renderer();
    let short = path("M 0 50 L 120 50");
    let style = style("This text is far too long for its path", 20.0, Default::default());
    let total = straight(&mut renderer, &style).0.len();
    let glyphs = renderer.layout_on_path(&style, &short).unwrap();
    assert!(!glyphs.is_empty() && glyphs.len() < total / 2);
    for placed in &glyphs {
        assert!((0.0..=120.0).contains(&placed.on_path.x));
    }
    let (pixels, origin) = renderer.render_on_path(&style, &short).unwrap();
    assert!(origin[0] + pixels.width() as i32 <= 132, "{origin:?} {}", pixels.width());
}

#[test]
fn text_wraps_around_a_closed_path() {
    let mut renderer = renderer();
    let circle = "M 150 100 A 50 50 0 1 1 50 100 A 50 50 0 1 1 150 100";
    let options = PathTextOptions {
        start_offset: 75.0,
        ..Default::default()
    };
    let style = style("wrapping around a circle", 16.0, options);
    let (laid, width) = straight(&mut renderer, &style);
    let length = std::f64::consts::TAU * 50.0;
    assert!(0.75 * length + width > length * 1.2, "the text must pass the start");
    let closed = renderer
        .layout_on_path(&style, &path(&format!("{circle} Z")))
        .unwrap();
    assert_eq!(closed.len(), laid.len());
    let centre = kurbo::Point::new(100.0, 100.0);
    for placed in &closed {
        assert!(((placed.on_path - centre).hypot() - 50.0).abs() < 0.1);
    }
    // The same circle left open ends where it starts, so the letters past it are hidden.
    let open = renderer.layout_on_path(&style, &path(circle)).unwrap();
    assert!(open.len() < closed.len());
    for (a, b) in open.iter().zip(&closed) {
        assert!((a.on_path - b.on_path).hypot() < 1e-6);
    }
}

#[test]
fn later_lines_run_beside_the_first() {
    let mut renderer = renderer();
    let line = path("M 0 100 L 1000 100");
    let glyphs = renderer
        .layout_on_path(&style("ab\ncd", 20.0, Default::default()), &line)
        .unwrap();
    assert_eq!(glyphs.len(), 4);
    let pen = |p: &PathGlyph| p.transform * kurbo::Point::ZERO;
    assert!((pen(&glyphs[0]).y - 100.0).abs() < 1e-6);
    assert!((pen(&glyphs[2]).y - 126.0).abs() < 1e-3, "{:?}", pen(&glyphs[2]));
    assert!((glyphs[2].on_path.x - glyphs[0].on_path.x).abs() < 3.0);
}

fn assert_same_path(a: &VectorPath, b: &VectorPath) {
    let (a, b) = (a.bounds().unwrap(), b.bounds().unwrap());
    for (x, y) in [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)] {
        assert!((x - y).abs() < 0.01, "{a:?} vs {b:?}");
    }
}

#[test]
fn the_path_stays_put_when_the_text_changes_and_moves_with_the_layer() {
    let mut renderer = renderer();
    let d = path("M 20 120 Q 150 20 280 120");
    let base = TextStyle {
        content: "Round trip".into(),
        size: 24.0,
        color: [10, 80, 160, 255],
        ..Default::default()
    };
    let options = PathTextOptions {
        align: PathAlign::Center,
        start_offset: 50.0,
        ..Default::default()
    };
    let mut layer = path_layer(&mut renderer, base.clone(), &d, options.clone()).unwrap();
    let stored = |layer: &Layer| layer.text.as_ref().unwrap().path.clone().unwrap();
    assert_same_path(&stored(&layer).in_document(layer.transform).unwrap(), &d);
    let (w, h) = layer.pixels.as_ref().unwrap().dimensions();
    assert_eq!((stored(&layer).width, stored(&layer).height), (w as f32, h as f32));
    // The layer is placed by its ink, which the text overhangs the path's ends with.
    assert!(layer.transform.x > 20.0 && layer.transform.y < 90.0);

    // Longer text grows the layer, but the path stays where it is in the document.
    let mut longer = layer.text.clone().unwrap();
    longer.content = "Round trip, and longer".into();
    restyle_layer(&mut renderer, &mut layer, longer).unwrap();
    assert!(layer.pixels.as_ref().unwrap().width() > w);
    assert_same_path(&stored(&layer).in_document(layer.transform).unwrap(), &d);

    // Moving, scaling and turning the layer takes its path along.
    layer.transform.x += 30.0;
    layer.transform.width *= 1.5;
    layer.transform.rotation = 20.0;
    let moved = stored(&layer).in_document(layer.transform).unwrap();
    let before = moved.bounds().unwrap();
    let style = layer.text.clone().unwrap();
    restyle_layer(&mut renderer, &mut layer, style).unwrap();
    assert_same_path(&stored(&layer).in_document(layer.transform).unwrap(), &moved);
    assert!(before.width() > d.bounds().unwrap().width());
    // A path given in document coordinates is kept in the turned layer's box.
    let attached = TextPath::from_document(
        &d,
        layer.transform,
        layer.pixels.as_ref().unwrap().dimensions(),
        options,
    )
    .unwrap();
    assert_same_path(&attached.in_document(layer.transform).unwrap(), &d);
    let mut warped = layer.transform;
    warped.warp = Some(crate::document::Transform::new(1, 1).corners());
    assert!(attached.in_document(warped).is_none());

    // Back in a box, the text keeps the layer's corner.
    let mut boxed = layer.text.clone().unwrap();
    boxed.path = None;
    let corner = layer.transform.point(Point::default());
    restyle_layer(&mut renderer, &mut layer, boxed).unwrap();
    assert!(layer.transform.point(Point::default()).distance(corner) < 0.01);
    assert!(layer.text.as_ref().unwrap().path.is_none());
}

#[test]
fn text_on_a_path_round_trips_through_xuan_files() {
    let mut renderer = renderer();
    let options = PathTextOptions {
        start_offset: 10.0,
        side: PathSide::Right,
        letter_spacing: 1.5,
        rotate: false,
        baseline_shift: -3.0,
        size_end: Some(9.0),
        opacity_start: 0.9,
        opacity_end: 0.4,
        align: PathAlign::Center,
    };
    let style = TextStyle {
        content: "Saved on a curve".into(),
        size: 20.0,
        color: [120, 30, 60, 230],
        bold: true,
        underline: true,
        ..Default::default()
    };
    let mut layer = path_layer(
        &mut renderer,
        style,
        &path("M 10 150 C 60 40 200 40 250 150"),
        options,
    )
    .unwrap();
    layer.transform.rotation = 12.0;
    let mut document = Document::new(300, 200).unwrap();
    document.insert(layer);
    let file = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    io::save(&document, &file).unwrap();
    let loaded = io::load(&file).unwrap();
    let (saved, read) = (document.active().unwrap(), loaded.active().unwrap());
    assert_eq!(read.text, saved.text);
    assert_eq!(read.pixels, saved.pixels);
    assert_eq!(render::render(&loaded), render::render(&document));
    // Drawn again from what was read, it is the same text.
    let mut redrawn = read.clone();
    restyle_layer(&mut renderer, &mut redrawn, read.text.clone().unwrap()).unwrap();
    assert_eq!(redrawn.pixels, saved.pixels);
    assert!((redrawn.transform.x - saved.transform.x).abs() < 1e-3);
    assert!((redrawn.transform.y - saved.transform.y).abs() < 1e-3);
}

#[test]
fn path_options_are_checked_and_default_when_left_out() {
    let stored: TextPath =
        serde_json::from_value(serde_json::json!({"d": "M0,0 L10,0", "width": 10, "height": 2}))
            .unwrap();
    assert_eq!(stored.options, PathTextOptions::default());
    assert!(stored.options.rotate);
    let json = serde_json::to_value(&stored).unwrap();
    assert_eq!(json["align"], "start");
    assert_eq!(json["side"], "left");
    assert!(json.get("size_end").is_none());
    stored.validate().unwrap();
    for change in [
        serde_json::json!({"start_offset": 101}),
        serde_json::json!({"letter_spacing": 2000}),
        serde_json::json!({"size_end": 0}),
        serde_json::json!({"opacity_end": 1.5}),
        serde_json::json!({"width": 0}),
        serde_json::json!({"d": ""}),
    ] {
        let mut json = json.clone();
        for (key, value) in change.as_object().unwrap() {
            json[key] = value.clone();
        }
        let bad: TextPath = serde_json::from_value(json).unwrap();
        let style = TextStyle {
            path: Some(bad),
            ..Default::default()
        };
        assert!(style.validate().is_err(), "{change}");
    }
}

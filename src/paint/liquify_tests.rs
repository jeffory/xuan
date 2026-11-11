//! Liquify: the falloff and displacement field, sub-steps that keep the
//! field from folding, and strokes that push pixels without tearing.
use super::liquify::{Dab, MAX_HARDNESS, MAX_STEPS, dabs, falloff, max_step};
use super::*;

fn options(mask_target: bool) -> StrokeOptions<'static> {
    StrokeOptions {
        mode: PaintMode::Liquify,
        mask_target,
        source: None,
        clone_offset: Point::default(),
    }
}

/// A 100 × 60 opaque layer whose red rises by 2 per column, so a pixel's
/// red says which column its colour came from.
fn ramp() -> Document {
    let mut document = Document::new(100, 60).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_fn(100, 60, |x, y| {
        Rgba([(x * 2) as u8, (y * 3) as u8, 90, 255])
    })));
    document
}

fn brush(diameter: f32, hardness: f32, strength: f32) -> Brush {
    Brush {
        diameter,
        hardness,
        opacity: strength,
        ..Brush::default()
    }
}

fn pixels(document: &Document) -> &RgbaImage {
    document.active().unwrap().pixels.as_ref().unwrap()
}

fn push(document: &mut Document, from: Point, to: Point, brush: &Brush) -> Result<()> {
    stroke(document, from, to, brush, options(false))
}

#[test]
fn the_falloff_is_full_inside_the_hardness_and_none_at_the_rim() {
    assert_eq!(falloff(0.0, 0.5), 1.0);
    assert_eq!(falloff(0.5, 0.5), 1.0);
    assert_eq!(falloff(1.0, 0.5), 0.0);
    assert_eq!(falloff(3.0, 0.5), 0.0);
    // Halfway through the soft edge, smoothstep gives exactly a half.
    assert!((falloff(0.75, 0.5) - 0.5).abs() < 1e-6);
    // It falls steadily from the centre to the rim.
    let mut last = 1.0;
    for i in 0..=100 {
        let value = falloff(i as f32 / 100.0, 0.2);
        assert!(value <= last, "{i}: {value} > {last}");
        last = value;
    }
    // A harder brush is capped, so the edge never becomes a step.
    assert_eq!(falloff(0.9, 1.0), falloff(0.9, MAX_HARDNESS));
    assert!(falloff(0.9, 1.0) > 0.0);
    assert_eq!(falloff(f32::NAN, 0.5), 0.0);
}

#[test]
fn the_displacement_scales_with_strength_and_fades_to_the_rim() {
    let dab = Dab {
        center: Point::new(50.0, 50.0),
        delta: Point::new(4.0, -2.0),
        radius: 10.0,
        hardness: 0.5,
        strength: 0.5,
        tilt: [0.0; 2],
    };
    let centre = dab.displacement(Point::new(52.0, 50.0));
    assert!((centre.x - 2.0).abs() < 1e-6 && (centre.y + 1.0).abs() < 1e-6);
    let outside = dab.displacement(Point::new(61.0, 50.0));
    assert_eq!((outside.x, outside.y), (0.0, 0.0));
    let edge = dab.displacement(Point::new(57.5, 50.0));
    assert!((edge.x - 1.0).abs() < 1e-5, "{edge:?}");
    let still = Dab {
        strength: 0.0,
        ..dab
    };
    let none = still.displacement(Point::new(50.0, 50.0));
    assert_eq!((none.x, none.y), (0.0, 0.0));
}

#[test]
fn long_segments_are_cut_into_steps_that_cannot_fold() {
    let soft = brush(40.0, 0.5, 1.0);
    // 20 × 0.5 / 3 = 3.33 units per step at most.
    assert!((max_step(20.0, 0.5, 1.0) - 10.0 / 3.0).abs() < 1e-5);
    let steps = dabs(Point::new(0.0, 0.0), Point::new(10.0, 0.0), &soft, &soft);
    assert_eq!(steps.len(), 3);
    let last = steps.last().unwrap();
    assert!((last.center.x - 10.0).abs() < 1e-5);
    let moved: f32 = steps.iter().map(|d| d.delta.x).sum();
    assert!((moved - 10.0).abs() < 1e-5);
    for dab in &steps {
        // The displacement's slope stays at a half or less, so no two pixels swap.
        let slope = dab.delta.x.hypot(dab.delta.y) * dab.strength * 1.5
            / (dab.radius * (1.0 - dab.hardness.min(MAX_HARDNESS)));
        assert!(slope <= 0.5 + 1e-5, "{slope}");
    }
    // A short move is one step; a huge jump is bounded.
    assert_eq!(
        dabs(Point::new(0.0, 0.0), Point::new(1.0, 0.0), &soft, &soft).len(),
        1
    );
    let tiny = brush(1.0, 1.0, 1.0);
    assert_eq!(
        dabs(Point::new(0.0, 0.0), Point::new(1e7, 0.0), &tiny, &tiny).len(),
        MAX_STEPS
    );
    // No movement, no strength or bad input: nothing to do.
    assert!(dabs(Point::new(3.0, 3.0), Point::new(3.0, 3.0), &soft, &soft).is_empty());
    let still = brush(40.0, 0.5, 0.0);
    assert!(dabs(Point::new(0.0, 0.0), Point::new(9.0, 0.0), &still, &still).is_empty());
    assert!(
        dabs(
            Point::new(f32::NAN, 0.0),
            Point::new(9.0, 0.0),
            &soft,
            &soft
        )
        .is_empty()
    );
    let broken = brush(f32::NAN, f32::NAN, f32::NAN);
    assert!(dabs(Point::new(0.0, 0.0), Point::new(9.0, 0.0), &broken, &broken).is_empty());
}

#[test]
fn a_push_moves_the_pixels_under_the_brush_by_the_drag() {
    let mut document = ramp();
    let before = pixels(&document).clone();
    // Radius 30 at hardness 0.5 takes a 10 unit move in one step, and
    // everything within 15 of the end moves by the whole drag.
    push(
        &mut document,
        Point::new(40.0, 30.0),
        Point::new(50.0, 30.0),
        &brush(60.0, 0.5, 1.0),
    )
    .unwrap();
    let after = pixels(&document);
    for x in 40..60 {
        assert_eq!(after.get_pixel(x, 30), before.get_pixel(x - 10, 30), "{x}");
    }
    // Rows near the brush's centre move sideways only.
    assert_eq!(after.get_pixel(50, 35), before.get_pixel(40, 35));
    // Beyond the rim nothing changes.
    for (x, y) in [(10, 30), (90, 30), (50, 0), (50, 59), (85, 59)] {
        assert_eq!(after.get_pixel(x, y), before.get_pixel(x, y), "{x},{y}");
    }
}

#[test]
fn a_long_drag_pushes_an_edge_smoothly_without_holes() {
    let mut document = Document::new(120, 60).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_fn(120, 60, |x, _| {
        if x < 40 {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    })));
    let soft = brush(40.0, 0.3, 1.0);
    let points = [20.0, 27.0, 41.0, 48.0, 63.0, 70.0].map(|x| Point::new(x, 30.0));
    let mut gesture = Stroke::default();
    for pair in points.windows(2) {
        gesture
            .segment(
                &mut document,
                pair[0],
                pair[1],
                &soft,
                &soft,
                options(false),
            )
            .unwrap();
    }
    let after = pixels(&document);
    // The black has been pushed past the old edge along the stroke…
    assert!(
        after.get_pixel(55, 30)[0] < 40,
        "{:?}",
        after.get_pixel(55, 30)
    );
    // …but not far from it.
    assert_eq!(after.get_pixel(55, 5).0, [255; 4]);
    assert_eq!(after.get_pixel(30, 5).0, [0, 0, 0, 255]);
    for y in 0..60 {
        let mut last = 0;
        for x in 0..120 {
            let pixel = after.get_pixel(x, y);
            // No holes: the layer stays opaque…
            assert_eq!(pixel[3], 255, "{x},{y}");
            // …and grey only, between the two colours.
            assert!(pixel[0] == pixel[1] && pixel[1] == pixel[2], "{x},{y}");
            // No tearing or folds: along a row the push only ever moves
            // the edge, so it still rises from black to white.
            assert!(
                u16::from(pixel[0]) + 2 >= last,
                "{x},{y}: {} after {last}",
                pixel[0]
            );
            last = last.max(u16::from(pixel[0]));
        }
    }
}

#[test]
fn zero_strength_changes_nothing() {
    let mut document = ramp();
    let before = pixels(&document).clone();
    let pointer = Arc::as_ptr(document.active().unwrap().pixels.as_ref().unwrap());
    push(
        &mut document,
        Point::new(20.0, 30.0),
        Point::new(60.0, 30.0),
        &brush(40.0, 0.5, 0.0),
    )
    .unwrap();
    assert_eq!(pixels(&document), &before);
    // The buffer isn't even copied.
    assert_eq!(
        Arc::as_ptr(document.active().unwrap().pixels.as_ref().unwrap()),
        pointer
    );
    // Nor does a click without movement change anything.
    push(
        &mut document,
        Point::new(20.0, 30.0),
        Point::new(20.0, 30.0),
        &brush(40.0, 0.5, 1.0),
    )
    .unwrap();
    assert_eq!(pixels(&document), &before);
}

#[test]
fn samples_past_the_layer_edge_take_the_edge_pixel() {
    let mut document = ramp();
    let before = pixels(&document).clone();
    // Pull from the left edge into the layer: the pixels near the edge take
    // their colour from beyond it, which clamps to column 0.
    push(
        &mut document,
        Point::new(0.0, 30.0),
        Point::new(8.0, 30.0),
        &brush(60.0, 0.5, 1.0),
    )
    .unwrap();
    let after = pixels(&document);
    for x in 0..8 {
        assert_eq!(after.get_pixel(x, 30), before.get_pixel(0, 30), "{x}");
    }
    assert_eq!(after.get_pixel(12, 30), before.get_pixel(4, 30));
    assert!(after.pixels().all(|p| p[3] == 255));
    // A brush wholly off the layer changes nothing.
    let mut document = ramp();
    push(
        &mut document,
        Point::new(-200.0, -200.0),
        Point::new(-190.0, -200.0),
        &brush(40.0, 0.5, 1.0),
    )
    .unwrap();
    assert_eq!(pixels(&document), &before);
}

#[test]
fn the_selection_limits_the_warp() {
    let mut document = ramp();
    let before = pixels(&document).clone();
    document.selection = Some(Arc::new(GrayImage::from_fn(100, 60, |_, y| {
        Luma([if y < 30 { 255 } else { 0 }])
    })));
    push(
        &mut document,
        Point::new(40.0, 30.0),
        Point::new(50.0, 30.0),
        &brush(60.0, 0.5, 1.0),
    )
    .unwrap();
    let after = pixels(&document);
    assert_eq!(after.get_pixel(50, 29), before.get_pixel(40, 29));
    for y in 30..60 {
        for x in 0..100 {
            assert_eq!(after.get_pixel(x, y), before.get_pixel(x, y), "{x},{y}");
        }
    }
}

#[test]
fn transparent_pixels_move_with_their_neighbours() {
    // A dot on a transparent layer is pushed, colour and alpha alike,
    // without dark fringes from the transparent pixels' colour.
    let mut document = Document::new(80, 40).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::from_fn(80, 40, |x, y| {
        let distance = (x as f32 - 30.0).hypot(y as f32 - 20.0);
        if distance < 6.0 {
            Rgba([250, 40, 10, 255])
        } else {
            Rgba([0, 0, 0, 0])
        }
    })));
    push(
        &mut document,
        Point::new(30.0, 20.0),
        Point::new(40.0, 20.0),
        &brush(60.0, 0.5, 1.0),
    )
    .unwrap();
    let after = pixels(&document);
    assert_eq!(after.get_pixel(40, 20).0, [250, 40, 10, 255]);
    assert_eq!(after.get_pixel(28, 20)[3], 0);
    for pixel in after.pixels().filter(|p| p[3] > 0) {
        assert_eq!(&pixel.0[..3], &[250, 40, 10], "{pixel:?}");
    }
}

#[test]
fn a_scaled_and_turned_layer_warps_in_document_space() {
    let mut document = Document::new(100, 100).unwrap();
    let layer = document.active_mut().unwrap();
    layer.pixels = Some(Arc::new(RgbaImage::from_fn(50, 50, |x, y| {
        Rgba([(x * 5) as u8, (y * 5) as u8, 0, 255])
    })));
    layer.transform.rotation = 30.0;
    let before = pixels(&document).clone();
    push(
        &mut document,
        Point::new(45.0, 50.0),
        Point::new(55.0, 50.0),
        &brush(30.0, 0.5, 1.0),
    )
    .unwrap();
    let after = pixels(&document);
    assert_eq!(after.dimensions(), (50, 50));
    assert_ne!(after, &before);
    // Corners, far from the brush, are untouched.
    for (x, y) in [(0, 0), (49, 0), (0, 49), (49, 49)] {
        assert_eq!(after.get_pixel(x, y), before.get_pixel(x, y));
    }
}

#[test]
fn locked_layers_and_masks_are_refused() {
    let mut document = ramp();
    document.active_mut().unwrap().locked = true;
    let error = push(
        &mut document,
        Point::new(40.0, 30.0),
        Point::new(50.0, 30.0),
        &brush(40.0, 0.5, 1.0),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unlocked"), "{error}");

    let mut document = ramp();
    let before = pixels(&document).clone();
    document.active_mut().unwrap().mask = Some(Mask::white());
    let error = stroke(
        &mut document,
        Point::new(40.0, 30.0),
        Point::new(50.0, 30.0),
        &brush(40.0, 0.5, 1.0),
        options(true),
    )
    .unwrap_err();
    assert!(error.to_string().contains("not masks"), "{error}");
    assert_eq!(pixels(&document), &before);
}

#[test]
fn a_shape_layer_is_rasterized_by_its_first_push_and_keeps_its_effects() {
    let mut document = Document::new(100, 60).unwrap();
    let mut layer = shape(
        Point::new(20.0, 10.0),
        Point::new(60.0, 50.0),
        ShapeKind::Rectangle,
        [200, 30, 30, 255],
        0.0,
    )
    .unwrap();
    layer.effects = Some(crate::layer_effects::LayerEffects {
        stroke: Some(Default::default()),
        ..Default::default()
    });
    document.insert(layer);
    // Zero strength leaves it live.
    push(
        &mut document,
        Point::new(55.0, 30.0),
        Point::new(70.0, 30.0),
        &brush(30.0, 0.5, 0.0),
    )
    .unwrap();
    assert!(document.active().unwrap().shape.is_some());
    push(
        &mut document,
        Point::new(55.0, 30.0),
        Point::new(70.0, 30.0),
        &brush(30.0, 0.5, 1.0),
    )
    .unwrap();
    let layer = document.active().unwrap();
    assert!(layer.shape.is_none());
    assert!(layer.effects.as_ref().unwrap().stroke.is_some());
}

#[test]
fn a_huge_brush_on_a_tiny_layer_is_bounded() {
    let mut document = Document::new(8, 8).unwrap();
    document.active_mut().unwrap().pixels =
        Some(Arc::new(RgbaImage::from_pixel(8, 8, Rgba([9, 9, 9, 255]))));
    push(
        &mut document,
        Point::new(-1e6, 4.0),
        Point::new(1e6, 4.0),
        &brush(1e9, 0.5, 1.0),
    )
    .unwrap();
    // Uniform pixels stay uniform however far they are pushed.
    assert!(pixels(&document).pixels().all(|p| p.0 == [9, 9, 9, 255]));
}

//! Edit → Stroke… on the active layer: colour, opacity, Preserve Transparency and layers that
//! are moved, scaled or smaller than the canvas.
use std::sync::atomic::AtomicBool;

use super::outline::{self, Outline};
use super::*;
use crate::{document::Transform, selection_ops::StrokeLocation};

const RED: [u8; 3] = [200, 30, 10];

fn outline(width: u32, location: StrokeLocation) -> Outline {
    Outline {
        width,
        location,
        color: RED,
        opacity: 1.0,
        preserve_transparency: false,
    }
}

/// A canvas with a transparent full-size layer and the rectangle `[left, right) × [top,
/// bottom)` selected.
fn document(size: (u32, u32), rect: [u32; 4]) -> Document {
    let mut document = Document::new(size.0, size.1).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::new(size.0, size.1)));
    let [left, top, right, bottom] = rect;
    document.selection = Some(Arc::new(GrayImage::from_fn(size.0, size.1, |x, y| {
        Luma([
            if (left..right).contains(&x) && (top..bottom).contains(&y) {
                255
            } else {
                0
            },
        ])
    })));
    document
}

fn pixels(document: &Document) -> &RgbaImage {
    document.active().unwrap().pixels.as_deref().unwrap()
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn outside_stroke_paints_the_colour_beside_the_selection() {
    let mut document = document((40, 30), [10, 10, 30, 20]);
    outline::apply(
        &mut document,
        &outline(3, StrokeLocation::Outside),
        &never(),
    )
    .unwrap();
    let image = pixels(&document);
    for x in 0..40 {
        let expected = if (7..10).contains(&x) || (30..33).contains(&x) {
            [RED[0], RED[1], RED[2], 255]
        } else {
            [0; 4]
        };
        assert_eq!(image.get_pixel(x, 15).0, expected, "x = {x}");
    }
    // The selection itself is untouched, as is the layer's size.
    assert_eq!(image.dimensions(), (40, 30));
    assert_eq!(image.get_pixel(20, 15).0, [0; 4]);
}

#[test]
fn opacity_blends_the_stroke_over_the_layer() {
    let mut document = document((20, 10), [5, 2, 15, 8]);
    let white = RgbaImage::from_pixel(20, 10, Rgba([255; 4]));
    document.active_mut().unwrap().pixels = Some(Arc::new(white));
    let half = Outline {
        opacity: 0.5,
        ..outline(2, StrokeLocation::Inside)
    };
    outline::apply(&mut document, &half, &never()).unwrap();
    let image = pixels(&document);
    let mix = |c: u8| ((f32::from(c) + 255.0) / 2.0).round() as u8;
    assert_eq!(
        image.get_pixel(5, 5).0,
        [mix(RED[0]), mix(RED[1]), mix(RED[2]), 255]
    );
    assert_eq!(image.get_pixel(8, 5).0, [255; 4]);
    assert_eq!(image.get_pixel(4, 5).0, [255; 4]);
    // On a transparent layer the stroke is half transparent.
    let mut clear = self::document((20, 10), [5, 2, 15, 8]);
    outline::apply(&mut clear, &half, &never()).unwrap();
    assert_eq!(
        pixels(&clear).get_pixel(5, 5).0,
        [RED[0], RED[1], RED[2], 128]
    );
    // No opacity paints nothing.
    let mut none = self::document((20, 10), [5, 2, 15, 8]);
    let before = pixels(&none).clone();
    let zero = Outline {
        opacity: 0.0,
        ..half
    };
    outline::apply(&mut none, &zero, &never()).unwrap();
    assert_eq!(pixels(&none), &before);
}

#[test]
fn preserve_transparency_paints_only_what_the_layer_shows() {
    let mut document = document((20, 10), [4, 2, 16, 8]);
    // Opaque blue on the left half, a half-transparent pixel, nothing on the right.
    let mut image = RgbaImage::from_fn(20, 10, |x, _| {
        if x < 10 {
            Rgba([0, 0, 255, 255])
        } else {
            Rgba([0; 4])
        }
    });
    image.put_pixel(5, 5, Rgba([0, 0, 255, 100]));
    document.active_mut().unwrap().pixels = Some(Arc::new(image));
    let keep = Outline {
        preserve_transparency: true,
        ..outline(4, StrokeLocation::Center)
    };
    outline::apply(&mut document, &keep, &never()).unwrap();
    let image = pixels(&document);
    assert_eq!(image.get_pixel(3, 5).0, [RED[0], RED[1], RED[2], 255]);
    // The half-transparent pixel takes the colour and keeps its alpha.
    assert_eq!(image.get_pixel(5, 5).0, [RED[0], RED[1], RED[2], 100]);
    // Transparent pixels stay transparent, colour and all.
    assert_eq!(image.get_pixel(15, 5).0, [0; 4]);
    assert_eq!(image.get_pixel(17, 5).0, [0; 4]);
    // Away from the line the layer is as it was.
    assert_eq!(image.get_pixel(8, 5).0, [0, 0, 255, 255]);
}

#[test]
fn a_moved_layer_takes_the_stroke_where_it_shows_and_grows_to_hold_it() {
    // A 10 × 10 layer at (20, 10) on a 50 × 40 canvas; the selection is canvas (22..28,
    // 12..18), inside the layer, so an outside stroke of 4 reaches past the layer's edges.
    let mut document = document((50, 40), [22, 12, 28, 18]);
    let layer = document.active_mut().unwrap();
    layer.pixels = Some(Arc::new(RgbaImage::new(10, 10)));
    layer.transform = Transform {
        x: 20.0,
        y: 10.0,
        ..Transform::new(10, 10)
    };
    outline::apply(
        &mut document,
        &outline(4, StrokeLocation::Outside),
        &never(),
    )
    .unwrap();
    let layer = document.active().unwrap();
    // The layer grew left and up by two pixels to hold the line (canvas 18..32, 8..22).
    assert_eq!(
        (layer.transform.x.round(), layer.transform.y.round()),
        (18.0, 8.0)
    );
    assert_eq!(pixels(&document).dimensions(), (14, 14));
    let canvas = |x: u32, y: u32| pixels(&document).get_pixel(x - 18, y - 8).0;
    assert_eq!(canvas(18, 15), [RED[0], RED[1], RED[2], 255]);
    assert_eq!(canvas(21, 15), [RED[0], RED[1], RED[2], 255]);
    assert_eq!(canvas(22, 15), [0; 4]);
    assert_eq!(canvas(31, 15), [RED[0], RED[1], RED[2], 255]);
    // The composite shows the stroke on the canvas where the selection says.
    let shown = render::render(&document);
    assert_eq!(shown.get_pixel(18, 15).0, [RED[0], RED[1], RED[2], 255]);
    assert_eq!(shown.get_pixel(17, 15)[3], 0);
    assert_eq!(shown.get_pixel(25, 15)[3], 0);
}

#[test]
fn a_scaled_layer_samples_the_stroke_in_canvas_space() {
    // A 10 × 10 layer stretched over the canvas's 20 × 20 pixels: each layer pixel covers
    // two canvas pixels a side. An inside stroke of 4 is two layer pixels wide.
    let mut document = document((20, 20), [0, 0, 20, 20]);
    let layer = document.active_mut().unwrap();
    layer.pixels = Some(Arc::new(RgbaImage::new(10, 10)));
    layer.transform = Transform::new(20, 20);
    outline::apply(&mut document, &outline(4, StrokeLocation::Inside), &never()).unwrap();
    let image = pixels(&document);
    assert_eq!(image.dimensions(), (10, 10));
    let alpha: Vec<u8> = (0..10).map(|x| image.get_pixel(x, 5)[3]).collect();
    assert_eq!(alpha, [255, 255, 0, 0, 0, 0, 0, 0, 255, 255]);
}

#[test]
fn stroke_needs_a_selection_and_a_paintable_layer() {
    let mut document = document((10, 10), [2, 2, 8, 8]);
    document.selection = None;
    assert!(outline::apply(&mut document, &outline(2, StrokeLocation::Center), &never()).is_err());
    let mut locked = self::document((10, 10), [2, 2, 8, 8]);
    locked.active_mut().unwrap().locked = true;
    assert!(outline::apply(&mut locked, &outline(2, StrokeLocation::Center), &never()).is_err());
    let mut cancelled = self::document((10, 10), [2, 2, 8, 8]);
    assert!(
        outline::apply(
            &mut cancelled,
            &outline(2, StrokeLocation::Center),
            &AtomicBool::new(true)
        )
        .is_err()
    );
    // A new layer without pixels yet gets them.
    let mut empty = self::document((10, 10), [2, 2, 8, 8]);
    empty.active_mut().unwrap().pixels = None;
    outline::apply(&mut empty, &outline(1, StrokeLocation::Outside), &never()).unwrap();
    assert_eq!(
        pixels(&empty).get_pixel(1, 5).0,
        [RED[0], RED[1], RED[2], 255]
    );
}

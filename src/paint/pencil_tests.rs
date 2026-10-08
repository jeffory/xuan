use std::collections::BTreeSet;

use super::*;
use crate::history::History;

fn pencil(size: f32, opacity: f32, square: bool) -> Brush {
    Brush {
        diameter: size,
        opacity,
        color: [200, 40, 40, 255],
        square,
        ..Default::default()
    }
}

fn options(mask_target: bool) -> StrokeOptions<'static> {
    StrokeOptions {
        mode: PaintMode::Pencil,
        mask_target,
        source: None,
        clone_offset: Point::default(),
    }
}

fn blank(size: u32) -> Document {
    let mut document = Document::new(size, size).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::new(size, size)));
    document
}

fn painted(document: &Document) -> BTreeSet<(u32, u32)> {
    let pixels = document.active().unwrap().pixels.as_ref().unwrap();
    pixels
        .enumerate_pixels()
        .filter(|(_, _, p)| p[3] > 0)
        .map(|(x, y, _)| (x, y))
        .collect()
}

fn dab(size: f32, square: bool, at: Point) -> BTreeSet<(u32, u32)> {
    let mut document = blank(12);
    let brush = pencil(size, 1.0, square);
    stroke(&mut document, at, at, &brush, options(false)).unwrap();
    painted(&document)
}

fn set(pixels: &[(u32, u32)]) -> BTreeSet<(u32, u32)> {
    pixels.iter().copied().collect()
}

fn block(x: u32, y: u32, w: u32, h: u32) -> BTreeSet<(u32, u32)> {
    (y..y + h)
        .flat_map(|y| (x..x + w).map(move |x| (x, y)))
        .collect()
}

#[test]
fn dab_coverage_per_size_and_shape() {
    // Odd sizes centre on the pixel under the pointer; even sizes on its nearest corner.
    let odd = Point::new(5.3, 5.7);
    let even = Point::new(6.2, 5.8);
    assert_eq!(dab(1.0, false, odd), set(&[(5, 5)]));
    assert_eq!(dab(1.0, true, odd), set(&[(5, 5)]));
    assert_eq!(dab(2.0, false, even), block(5, 5, 2, 2));
    assert_eq!(dab(2.0, true, even), block(5, 5, 2, 2));
    assert_eq!(dab(3.0, true, odd), block(4, 4, 3, 3));
    assert_eq!(dab(3.0, false, odd), block(4, 4, 3, 3));
    assert_eq!(dab(4.0, true, even), block(4, 4, 4, 4));
    let mut round4 = block(4, 4, 4, 4);
    for corner in [(4, 4), (7, 4), (4, 7), (7, 7)] {
        round4.remove(&corner);
    }
    assert_eq!(dab(4.0, false, even), round4);
}

#[test]
fn tip_offsets_match_painted_dabs() {
    for size in 1..=6u32 {
        for square in [false, true] {
            // The pointer sits just inside pixel (6, 6) or just before corner (6, 6).
            let at = if size % 2 == 1 {
                Point::new(6.25, 6.25)
            } else {
                Point::new(5.9, 5.9)
            };
            let expected: BTreeSet<_> = tip_offsets(size, square)
                .into_iter()
                .map(|(x, y)| ((6 + x) as u32, (6 + y) as u32))
                .collect();
            assert_eq!(
                dab(size as f32, square, at),
                expected,
                "size {size} square {square}"
            );
        }
    }
}

#[test]
fn pencil_pixels_are_fully_opaque_without_partial_edges() {
    let mut document = blank(32);
    let brush = pencil(7.0, 1.0, false);
    stroke(
        &mut document,
        Point::new(5.0, 6.0),
        Point::new(25.0, 20.0),
        &brush,
        options(false),
    )
    .unwrap();
    let pixels = document.active().unwrap().pixels.as_ref().unwrap();
    assert!(!painted(&document).is_empty());
    for pixel in pixels.pixels() {
        assert!(pixel[3] == 0 || *pixel == Rgba([200, 40, 40, 255]));
    }
}

#[test]
fn diagonal_size_one_stroke_is_gap_free_and_one_pixel_thick() {
    let mut document = blank(16);
    let brush = pencil(1.0, 1.0, false);
    stroke(
        &mut document,
        Point::new(2.5, 2.5),
        Point::new(12.5, 12.5),
        &brush,
        options(false),
    )
    .unwrap();
    assert_eq!(painted(&document), (2..=12).map(|i| (i, i)).collect());

    // A shallow line has exactly one pixel per column and no gaps between neighbours.
    let mut document = blank(48);
    stroke(
        &mut document,
        Point::new(8.5, 10.5),
        Point::new(34.5, 19.5),
        &brush,
        options(false),
    )
    .unwrap();
    let pixels = painted(&document);
    assert_eq!(pixels.len(), 27);
    let rows: Vec<_> = (8..=34)
        .map(|x| {
            let column: Vec<_> = pixels.iter().filter(|p| p.0 == x).collect();
            assert_eq!(column.len(), 1, "column {x}");
            column[0].1 as i32
        })
        .collect();
    assert!(rows.windows(2).all(|w| (w[1] - w[0]).abs() <= 1));
}

#[test]
fn overlapping_dabs_do_not_accumulate_within_a_stroke() {
    let mut document = blank(24);
    let brush = pencil(5.0, 0.5, false);
    let mut coverage = Stroke::default();
    let mut previous = Point::new(3.5, 12.5);
    // Many overlapping segments, plus a return pass over the same pixels.
    for x in (4..=20).chain((3..=19).rev()) {
        let next = Point::new(x as f32 + 0.5, 12.5);
        coverage
            .segment(
                &mut document,
                previous,
                next,
                &brush,
                &brush,
                options(false),
            )
            .unwrap();
        previous = next;
    }
    let pixels = document.active().unwrap().pixels.as_ref().unwrap();
    let alphas: BTreeSet<_> = pixels.pixels().map(|p| p[3]).filter(|a| *a > 0).collect();
    assert_eq!(alphas, BTreeSet::from([128]));

    // A second stroke applies on top of the first.
    let mut coverage = Stroke::default();
    let at = Point::new(10.5, 12.5);
    coverage
        .segment(&mut document, at, at, &brush, &brush, options(false))
        .unwrap();
    let pixels = document.active().unwrap().pixels.as_ref().unwrap();
    assert!(pixels.get_pixel(10, 12)[3] > 128);
}

#[test]
fn respects_selection_and_locked_layers() {
    let mut document = blank(24);
    document.selection = Some(Arc::new(GrayImage::from_fn(24, 24, |x, _| {
        Luma([if x < 12 { 255 } else { 0 }])
    })));
    let brush = pencil(4.0, 1.0, true);
    stroke(
        &mut document,
        Point::new(6.0, 12.0),
        Point::new(18.0, 12.0),
        &brush,
        options(false),
    )
    .unwrap();
    let pixels = painted(&document);
    assert!(!pixels.is_empty());
    assert!(pixels.iter().all(|(x, _)| *x < 12));

    let mut locked = blank(16);
    locked.active_mut().unwrap().locked = true;
    let at = Point::new(8.0, 8.0);
    assert!(stroke(&mut locked, at, at, &brush, options(false)).is_err());
    assert!(painted(&locked).is_empty());
}

#[test]
fn paints_layer_masks() {
    let mut document = blank(16);
    let brush = Brush {
        color: [0, 0, 0, 255],
        ..pencil(3.0, 1.0, true)
    };
    let at = Point::new(8.5, 8.5);
    stroke(&mut document, at, at, &brush, options(true)).unwrap();
    let mask = &document.active().unwrap().mask.as_ref().unwrap().pixels;
    let hidden: BTreeSet<_> = mask
        .enumerate_pixels()
        .filter(|(_, _, p)| p[0] == 0)
        .map(|(x, y, _)| (x, y))
        .collect();
    assert_eq!(hidden, block(7, 7, 3, 3));
    assert!(mask.pixels().all(|p| p[0] == 0 || p[0] == 255));
}

#[test]
fn undo_restores_the_layer() {
    let mut document = blank(16);
    let before = document.active().unwrap().pixels.clone().unwrap();
    let mut history = History::default();
    history.begin("Pencil", &document);
    let brush = pencil(3.0, 1.0, false);
    stroke(
        &mut document,
        Point::new(2.0, 2.0),
        Point::new(12.0, 9.0),
        &brush,
        options(false),
    )
    .unwrap();
    history.commit();
    assert!(!painted(&document).is_empty());
    assert!(history.undo(&mut document));
    assert_eq!(
        **document.active().unwrap().pixels.as_ref().unwrap(),
        *before
    );
}

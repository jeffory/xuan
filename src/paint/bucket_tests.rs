use super::*;
use crate::document::Transform;

const RED: [u8; 4] = [255, 0, 0, 255];
const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);
const BLACK: Rgba<u8> = Rgba([0, 0, 0, 255]);

/// White paper with an anti-aliased black ring of radius 12 around (20, 20): solid
/// within half a pixel of the radius, fading to white over the next pixel.
fn line_art() -> RgbaImage {
    RgbaImage::from_fn(40, 40, |x, y| {
        let ink = (1.5 - (ring_distance(x, y) - 12.0).abs()).clamp(0.0, 1.0);
        let value = (255.0 * (1.0 - ink)).round() as u8;
        Rgba([value, value, value, 255])
    })
}

fn ring_distance(x: u32, y: u32) -> f32 {
    (x as f32 + 0.5 - 20.0).hypot(y as f32 + 0.5 - 20.0)
}

/// A document whose only layer holds `pixels`.
fn layer_document(pixels: RgbaImage) -> Document {
    let mut document = Document::new(pixels.width(), pixels.height()).unwrap();
    document.active_mut().unwrap().pixels = Some(Arc::new(pixels));
    document
}

fn pixels(document: &Document) -> &RgbaImage {
    document.active().unwrap().pixels.as_deref().unwrap()
}

fn options() -> BucketOptions {
    BucketOptions {
        color: RED,
        ..Default::default()
    }
}

fn fill_line_art(anti_alias: bool) -> RgbaImage {
    let mut document = layer_document(line_art());
    let filled = bucket(
        &mut document,
        Point::new(20.0, 20.0),
        BucketOptions {
            anti_alias,
            ..options()
        },
    )
    .unwrap();
    assert!(filled);
    pixels(&document).clone()
}

#[test]
fn line_art_fills_up_to_the_lines_without_a_halo() {
    let image = fill_line_art(true);
    for (x, y, pixel) in image.enumerate_pixels() {
        let distance = ring_distance(x, y);
        if distance < 10.5 {
            assert_eq!(pixel.0, RED, "inside at ({x}, {y})");
        } else if distance < 12.0 {
            // Paper showing between the fill and the line is light in every channel;
            // the fill has no green, so green measures what is left of the paper.
            assert!(pixel[1] <= 64, "halo at ({x}, {y}): {:?}", pixel.0);
        } else if distance > 13.5 {
            assert_eq!(*pixel, WHITE, "outside at ({x}, {y}) changed");
        }
    }
    // The solid line is untouched.
    assert_eq!(image.get_pixel(32, 20), line_art().get_pixel(32, 20));
    assert!(image.get_pixel(32, 20)[0] < 8);
}

#[test]
fn without_anti_alias_the_blended_line_edge_keeps_the_paper_colour() {
    let image = fill_line_art(false);
    // A pixel that is partly ink and partly paper is left as it was, a light ring.
    let halo = image
        .enumerate_pixels()
        .filter(|(x, y, _)| ring_distance(*x, *y) < 12.0)
        .any(|(_, _, pixel)| pixel[1] > 128 && pixel[1] < 255);
    assert!(halo);
    // The fill itself is all or nothing.
    assert!(
        image
            .pixels()
            .all(|p| p.0 == RED || p[0] == p[1] && p[1] == p[2])
    );
}

/// Two white squares on black, apart, with a grey square nearly white.
fn islands() -> RgbaImage {
    RgbaImage::from_fn(12, 4, |x, _| match x {
        0..=2 => WHITE,
        4..=6 => WHITE,
        8..=10 => Rgba([240, 240, 240, 255]),
        _ => BLACK,
    })
}

fn filled_columns(image: &RgbaImage) -> Vec<u32> {
    (0..image.width())
        .filter(|&x| image.get_pixel(x, 0).0 == RED)
        .collect()
}

#[test]
fn contiguous_fills_only_the_connected_area() {
    let mut document = layer_document(islands());
    let options = BucketOptions {
        anti_alias: false,
        ..options()
    };
    bucket(&mut document, Point::new(1.5, 1.5), options).unwrap();
    assert_eq!(filled_columns(pixels(&document)), [0, 1, 2]);
}

#[test]
fn non_contiguous_fills_every_matching_pixel_on_the_layer() {
    let mut document = layer_document(islands());
    let options = BucketOptions {
        anti_alias: false,
        contiguous: false,
        ..options()
    };
    bucket(&mut document, Point::new(1.5, 1.5), options).unwrap();
    // The grey square is within the tolerance of white; the black is not.
    assert_eq!(
        filled_columns(pixels(&document)),
        [0, 1, 2, 4, 5, 6, 8, 9, 10]
    );

    // At a lower tolerance the grey is left out.
    let mut document = layer_document(islands());
    let options = BucketOptions {
        tolerance: 4,
        ..options
    };
    bucket(&mut document, Point::new(1.5, 1.5), options).unwrap();
    assert_eq!(filled_columns(pixels(&document)), [0, 1, 2, 4, 5, 6]);
}

#[test]
fn anti_aliased_non_contiguous_fill_softens_every_area_edge() {
    let image = RgbaImage::from_fn(5, 1, |x, _| match x {
        0 | 4 => WHITE,
        2 => BLACK,
        _ => Rgba([128, 128, 128, 255]),
    });
    let coverage = bucket_coverage(&image, Point::new(0.5, 0.5), 32, false, true);
    // Mid grey next to an area is half covered; black, touching none, is not at all.
    let half = ((255.0 - 127.0) / (255.0 - 32.0) * 255.0_f32).round() as u8;
    assert_eq!(coverage.as_raw(), &[255, half, 0, half, 255]);
}

#[test]
fn sampling_all_layers_fills_up_to_lines_on_another_layer() {
    let mut document = layer_document(line_art());
    document.insert(Layer::image("Colour", RgbaImage::new(40, 40)));
    let current = BucketOptions {
        anti_alias: false,
        ..options()
    };

    // The empty layer alone has nothing to stop the fill.
    let mut alone = document.clone();
    bucket(&mut alone, Point::new(20.0, 20.0), current).unwrap();
    assert!(pixels(&alone).pixels().all(|p| p.0 == RED));

    let all = BucketOptions {
        all_layers: true,
        ..current
    };
    bucket(&mut document, Point::new(20.0, 20.0), all).unwrap();
    let image = pixels(&document);
    assert_eq!(image.get_pixel(20, 20).0, RED);
    assert_eq!(image.get_pixel(20, 31).0, [0; 4], "the line stops it");
    assert_eq!(image.get_pixel(1, 1).0, [0; 4], "outside the line");
    // The line art itself is untouched.
    assert_eq!(document.layers[0].pixels.as_deref(), Some(&line_art()));
}

#[test]
fn the_selection_clips_the_fill() {
    let mut document = layer_document(RgbaImage::from_pixel(8, 4, WHITE));
    let mut selection = GrayImage::new(8, 4);
    for y in 0..4 {
        for x in 0..4 {
            selection.put_pixel(x, y, Luma([255]));
        }
    }
    selection.put_pixel(4, 0, Luma([128]));
    document.selection = Some(Arc::new(selection));
    // A click outside the selection still fills the selected part of the area.
    bucket(&mut document, Point::new(6.5, 2.5), options()).unwrap();
    let image = pixels(&document);
    assert_eq!(image.get_pixel(0, 0).0, RED);
    assert_eq!(image.get_pixel(3, 3).0, RED);
    assert_eq!(image.get_pixel(4, 0).0, [255, 127, 127, 255]);
    assert_eq!(*image.get_pixel(4, 1), WHITE);
    assert_eq!(*image.get_pixel(7, 3), WHITE);
}

#[test]
fn opacity_blends_the_fill_over_the_layer() {
    let mut document = layer_document(RgbaImage::from_pixel(4, 4, WHITE));
    let options = BucketOptions {
        opacity: 0.5,
        ..options()
    };
    bucket(&mut document, Point::new(1.0, 1.0), options).unwrap();
    assert!(
        pixels(&document)
            .pixels()
            .all(|p| p.0 == [255, 128, 128, 255])
    );
}

#[test]
fn painting_the_mask_fills_mask_areas_by_their_greys() {
    let mut layer = Layer::image("Photo", RgbaImage::from_pixel(8, 4, WHITE));
    // A black bar splits the mask at column 3.
    let mut mask = GrayImage::from_pixel(8, 4, Luma([255]));
    for y in 0..4 {
        mask.put_pixel(3, y, Luma([0]));
    }
    layer.mask = Some(Mask {
        pixels: Arc::new(mask),
        ..Mask::white()
    });
    let mut document = Document::new(8, 4).unwrap();
    document.insert(layer);
    let options = BucketOptions {
        color: [0, 0, 0, 255],
        mask_target: true,
        all_layers: false,
        ..options()
    };
    assert!(bucket(&mut document, Point::new(6.5, 1.5), options).unwrap());
    let layer = document.active().unwrap();
    let mask = &layer.mask.as_ref().unwrap().pixels;
    let row: Vec<u8> = (0..8).map(|x| mask.get_pixel(x, 2)[0]).collect();
    assert_eq!(row, [255, 255, 255, 0, 0, 0, 0, 0]);
    // The layer's pixels are not painted.
    assert!(layer.pixels.as_ref().unwrap().pixels().all(|p| *p == WHITE));
}

#[test]
fn locked_layers_refuse_the_fill() {
    let mut document = layer_document(RgbaImage::from_pixel(4, 4, WHITE));
    document.active_mut().unwrap().locked = true;
    let before = pixels(&document).clone();
    assert!(bucket(&mut document, Point::new(1.0, 1.0), options()).is_err());
    assert_eq!(*pixels(&document), before);

    let options = BucketOptions {
        mask_target: true,
        ..options()
    };
    assert!(bucket(&mut document, Point::new(1.0, 1.0), options).is_err());
}

#[test]
fn clicks_map_through_the_layer_transform() {
    let mut document = Document::new(40, 40).unwrap();
    let mut layer = Layer::image("Offset", RgbaImage::from_pixel(10, 10, WHITE));
    layer.set_transform(Transform {
        x: 20.0,
        y: 10.0,
        ..Transform::new(10, 10)
    });
    // A black column at the layer's x = 4, canvas x = 24.
    let pixels_mut = Arc::make_mut(layer.pixels.as_mut().unwrap());
    for y in 0..10 {
        pixels_mut.put_pixel(4, y, BLACK);
    }
    document.insert(layer);
    let options = BucketOptions {
        anti_alias: false,
        ..options()
    };

    // Off the layer there is nothing to fill.
    assert!(!bucket(&mut document, Point::new(5.0, 5.0), options).unwrap());
    assert!(bucket(&mut document, Point::new(27.5, 12.5), options).unwrap());
    let image = pixels(&document);
    assert_eq!(*image.get_pixel(3, 0), WHITE);
    assert_eq!(*image.get_pixel(4, 0), BLACK);
    assert_eq!(image.get_pixel(5, 0).0, RED);
    assert_eq!(image.get_pixel(9, 9).0, RED);
}

#[test]
fn a_transparent_seed_fills_by_alpha_alone() {
    // Transparent paper whose colour channels differ, then faint and solid ink.
    let image = RgbaImage::from_fn(4, 1, |x, _| match x {
        0 => Rgba([255, 255, 255, 0]),
        1 => Rgba([0, 0, 0, 0]),
        2 => Rgba([0, 0, 0, 128]),
        _ => BLACK,
    });
    let coverage = bucket_coverage(&image, Point::new(0.5, 0.5), 32, true, true);
    let half = ((255.0 - 128.0) / (255.0 - 32.0) * 255.0_f32).round() as u8;
    assert_eq!(coverage.as_raw(), &[255, 255, half, 0]);
    // A click off the image covers nothing.
    let none = bucket_coverage(&image, Point::new(9.0, 0.5), 32, true, true);
    assert!(none.as_raw().iter().all(|&v| v == 0));
}

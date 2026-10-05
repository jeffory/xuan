use super::*;
use image::Rgba;

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

/// A repeatable pseudo-random number in 0..limit for a pixel.
fn noise(x: u32, y: u32, salt: u32) -> u32 {
    let mut v = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(salt.wrapping_mul(2_246_822_519));
    v = (v ^ (v >> 13)).wrapping_mul(1_274_126_177);
    v ^ (v >> 16)
}

fn jitter(value: u8, x: u32, y: u32, salt: u32, amount: u32) -> u8 {
    let offset = (noise(x, y, salt) % (2 * amount + 1)) as i32 - amount as i32;
    (i32::from(value) + offset).clamp(0, 255) as u8
}

/// Intersection over union of the mask (≥ 128) with `truth`.
fn iou(mask: &GrayImage, truth: impl Fn(u32, u32) -> bool) -> f64 {
    let (mut both, mut either) = (0u64, 0u64);
    for (x, y, p) in mask.enumerate_pixels() {
        let (a, b) = (p[0] >= 128, truth(x, y));
        both += u64::from(a && b);
        either += u64::from(a || b);
    }
    both as f64 / either.max(1) as f64
}

fn in_disc(x: u32, y: u32, cx: f32, cy: f32, r: f32) -> bool {
    (x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2) <= r * r
}

/// An orange disc on a mottled green and brown texture.
fn disc_on_texture(width: u32, height: u32) -> (RgbaImage, impl Fn(u32, u32) -> bool) {
    let (cx, cy, r) = (width as f32 * 0.5, height as f32 * 0.5, height as f32 * 0.3);
    let image = RgbaImage::from_fn(width, height, |x, y| {
        if in_disc(x, y, cx, cy, r) {
            Rgba([
                jitter(235, x, y, 1, 12),
                jitter(140, x, y, 2, 12),
                jitter(40, x, y, 3, 12),
                255,
            ])
        } else {
            let patch = noise(x / 6, y / 6, 9) % 3;
            let base = [[60, 110, 50], [110, 85, 55], [80, 130, 90]][patch as usize];
            Rgba([
                jitter(base[0], x, y, 4, 20),
                jitter(base[1], x, y, 5, 20),
                jitter(base[2], x, y, 6, 20),
                255,
            ])
        }
    });
    (image, move |x, y| in_disc(x, y, cx, cy, r))
}

#[test]
fn a_disc_on_a_textured_background() {
    let (image, truth) = disc_on_texture(320, 240);
    let result = segment(&image, &Seeds::subject(), &|_| {}, &never()).unwrap();
    let score = iou(&result.mask, truth);
    assert!(score >= 0.95, "IoU {score}");
    assert_eq!(result.work, (320, 240));
}

#[test]
fn a_two_colour_subject_on_a_gradient() {
    let (width, height) = (300, 220);
    let inside = |x: u32, y: u32| (90..210).contains(&x) && (50..180).contains(&y);
    let image = RgbaImage::from_fn(width, height, |x, y| {
        if inside(x, y) {
            // Red above, blue below.
            if y < 115 {
                Rgba([jitter(200, x, y, 1, 8), 30, 40, 255])
            } else {
                Rgba([30, 50, jitter(190, x, y, 2, 8), 255])
            }
        } else {
            let level = (60.0 + 140.0 * x as f32 / width as f32) as u8;
            Rgba([
                level,
                jitter(level, x, y, 3, 4),
                level.saturating_add(10),
                255,
            ])
        }
    });
    let result = segment(&image, &Seeds::subject(), &|_| {}, &never()).unwrap();
    let score = iou(&result.mask, inside);
    assert!(score >= 0.95, "IoU {score}");
}

#[test]
fn a_subject_running_off_the_bottom_edge_reaches_it() {
    let (width, height) = (260, 200);
    let inside = |x: u32, y: u32| {
        (x as f32 - 130.0).powi(2) / 70f32.powi(2) + (y as f32 - 200.0).powi(2) / 120f32.powi(2)
            <= 1.0
    };
    let image = RgbaImage::from_fn(width, height, |x, y| {
        if inside(x, y) {
            Rgba([jitter(40, x, y, 1, 10), jitter(60, x, y, 2, 10), 200, 255])
        } else {
            Rgba([jitter(220, x, y, 3, 10), jitter(210, x, y, 4, 10), 180, 255])
        }
    });
    let result = segment(&image, &Seeds::subject(), &|_| {}, &never()).unwrap();
    let score = iou(&result.mask, inside);
    assert!(score >= 0.95, "IoU {score}");
    // The bottom row under the subject is kept, though the border seeds background.
    assert_eq!(result.mask.get_pixel(130, height - 1)[0], 255);
    assert_eq!(result.mask.get_pixel(5, height - 1)[0], 0);
}

#[test]
fn soft_edges_only_near_the_boundary_and_the_graph_stays_bounded() {
    let (image, truth) = disc_on_texture(1200, 900);
    let result = segment(&image, &Seeds::subject(), &|_| {}, &never()).unwrap();
    assert_eq!(result.work, (512, 384));
    assert!(iou(&result.mask, &truth) >= 0.95);
    let (cx, cy, r) = (600.0f32, 450.0f32, 270.0f32);
    let mut soft = 0;
    for (x, y, p) in result.mask.enumerate_pixels() {
        if p[0] != 0 && p[0] != 255 {
            soft += 1;
            let distance = ((x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) - r).abs();
            assert!(
                distance <= 12.0,
                "soft pixel {x},{y} is {distance} px from the edge"
            );
        }
    }
    // The edge is anti-aliased: a soft ring of roughly its circumference.
    assert!(soft > 1000, "{soft} soft pixels");
}

#[test]
fn a_large_image_is_cut_on_a_bounded_graph() {
    let (image, truth) = disc_on_texture(4000, 3000);
    let reported = std::sync::Mutex::new(Vec::new());
    let result = segment(
        &image,
        &Seeds::subject(),
        &|p| reported.lock().unwrap().push(p),
        &never(),
    )
    .unwrap();
    assert_eq!(result.work, (512, 384));
    assert_eq!(result.nodes, 512 * 384);
    // Four links per pixel at most: left, upper-left, up and upper-right.
    assert!(result.edges <= 4 * result.nodes);
    assert!(result.iterations >= 1);
    assert!(iou(&result.mask, truth) >= 0.95);
    let reported = reported.into_inner().unwrap();
    assert!(reported.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(reported.last(), Some(&1.0));
}

#[test]
fn cancelling_stops_the_segmentation() {
    let (image, _) = disc_on_texture(200, 160);
    assert!(segment(&image, &Seeds::subject(), &|_| {}, &AtomicBool::new(true)).is_none());
}

#[test]
fn a_click_keeps_only_the_object_under_it() {
    let (width, height) = (300, 200);
    let left = |x: u32, y: u32| in_disc(x, y, 90.0, 100.0, 45.0);
    let right = |x: u32, y: u32| in_disc(x, y, 210.0, 100.0, 45.0);
    let image = RgbaImage::from_fn(width, height, |x, y| {
        if left(x, y) || right(x, y) {
            Rgba([jitter(220, x, y, 1, 8), 40, 40, 255])
        } else {
            Rgba([40, jitter(160, x, y, 2, 8), 60, 255])
        }
    });
    let seeds = Seeds {
        points: vec![(210.0, 100.0)],
        rect: None,
    };
    let result = segment(&image, &seeds, &|_| {}, &never()).unwrap();
    assert!(iou(&result.mask, right) >= 0.95);
    // A rectangle bounds the object too.
    let seeds = Seeds {
        points: vec![],
        rect: Some([30.0, 40.0, 150.0, 160.0]),
    };
    let result = segment(&image, &seeds, &|_| {}, &never()).unwrap();
    assert!(iou(&result.mask, left) >= 0.95);
}

#[test]
fn transparent_pixels_are_background_and_flat_images_do_not_panic() {
    let image = RgbaImage::from_fn(120, 100, |x, y| {
        if in_disc(x, y, 60.0, 50.0, 30.0) {
            Rgba([200, 30, 30, 255])
        } else {
            Rgba([0, 0, 0, 0])
        }
    });
    let result = segment(&image, &Seeds::subject(), &|_| {}, &never()).unwrap();
    assert!(iou(&result.mask, |x, y| in_disc(x, y, 60.0, 50.0, 30.0)) >= 0.95);
    for (width, height) in [(1, 1), (1, 40), (40, 1), (3, 3)] {
        let flat = RgbaImage::from_pixel(width, height, Rgba([9, 9, 9, 255]));
        let result = segment(&flat, &Seeds::subject(), &|_| {}, &never()).unwrap();
        assert_eq!(result.mask.dimensions(), (width, height));
    }
}

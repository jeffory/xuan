//! The sample composition opened by `xuan --demo`.
//!
//! It lives in the library so the golden-image tests render exactly the
//! document the app shows, without starting the UI.

use image::{Rgba, RgbaImage};

use crate::{
    document::{Document, Layer, Point},
    paint::{self, ShapeKind},
};

/// Title of the demo document's tab.
pub const TITLE: &str = "Dune study";

/// A 1200 × 900 desert scene: a grained paper gradient, a sun and three ridges.
/// The sun layer is selected. Every pixel is a pure function of its position.
pub fn document() -> Document {
    let mut document = Document::new(1200, 900).unwrap();
    document.layers.clear();
    let sky = RgbaImage::from_fn(1200, 900, |x, y| {
        let t = y as f32 / 900.0;
        let grain = ((x.wrapping_mul(73) ^ y.wrapping_mul(137)) % 7) as f32 - 3.0;
        Rgba([
            (221.0 - t * 53.0 + grain) as u8,
            (183.0 - t * 65.0 + grain) as u8,
            (143.0 - t * 56.0 + grain) as u8,
            255,
        ])
    });
    document.layers.push(Layer::image("Warm paper", sky));
    document.layers.push(
        paint::shape(
            Point::new(758.0, 142.0),
            Point::new(944.0, 328.0),
            ShapeKind::Ellipse,
            [248, 222, 162, 255],
            0.0,
        )
        .unwrap(),
    );
    document.layers.last_mut().unwrap().name = "Afternoon sun".into();
    for (name, base, amplitude, phase, color) in [
        ("Distant ridge", 435.0, 80.0, 0.4, [173, 115, 84, 255]),
        ("Sandstone", 550.0, 130.0, 2.6, [137, 80, 60, 255]),
        ("Foreground dune", 695.0, 105.0, 4.4, [84, 58, 53, 255]),
    ] {
        let pixels = RgbaImage::from_fn(1200, 900, |x, y| {
            let line = base + (x as f32 / 420.0 + phase).sin() * amplitude;
            let mut c = color;
            c[3] = ((y as f32 - line).clamp(0.0, 1.0) * 255.0) as u8;
            Rgba(c)
        });
        document.layers.push(Layer::image(name, pixels));
    }
    document.select(document.layers[1].id, false);
    document
}

//! A layer's Fill: it fades the layer's own pixels but not its effects, and changes the blend of
//! the eight special modes rather than fading them.
use super::*;
use crate::{
    layer_effects::{EffectKind, LayerEffects, ShadowEffect, StrokeEffect},
    text::{TextRenderer, TextStyle},
};

fn white(width: u32, height: u32) -> Layer {
    Layer::image(
        "White",
        RgbaImage::from_pixel(width, height, Rgba([255; 4])),
    )
}

fn document(layers: Vec<Layer>) -> Document {
    let mut document = Document::new(layers[0].transform.width as u32, 1).unwrap();
    document.height = layers[0].transform.height as u32;
    document.active = layers.last().map(|l| l.id);
    document.layers = layers;
    document.validate().unwrap();
    document
}

fn stroke(size: f32, color: [u8; 3]) -> LayerEffects {
    LayerEffects {
        stroke: Some(StrokeEffect {
            size,
            color,
            ..StrokeEffect::default()
        }),
        ..LayerEffects::default()
    }
}

/// A text layer with a stroke at Fill 0% shows only the stroke.
#[test]
fn a_text_layer_with_a_stroke_at_fill_0_shows_only_the_stroke() {
    let style = TextStyle {
        content: "Fill".into(),
        size: 32.0,
        color: [0, 0, 255, 255],
        ..Default::default()
    };
    let pixels = TextRenderer::default().render(&style).unwrap();
    let mut text = Layer::image("Fill", pixels);
    text.transform.x = 6.0;
    text.transform.y = 6.0;
    text.text = Some(style);
    text.effects = Some(stroke(2.0, [255, 0, 0]));
    let (width, height) = (
        text.transform.width as u32 + 12,
        text.transform.height as u32 + 12,
    );
    let mut document = document(vec![white(width, height), text]);
    let blue = |image: &RgbaImage| {
        image
            .pixels()
            .filter(|p| p[2] > p[1].saturating_add(40))
            .count()
    };
    let red = |image: &RgbaImage| {
        image
            .pixels()
            .filter(|p| p[0] > p[2].saturating_add(40))
            .count()
    };
    let full = render(&document);
    assert!(blue(&full) > 50 && red(&full) > 50);

    document.layers[1].fill = 0.0;
    let empty = render(&document);
    // White and red only: the glyphs are holes in the stroke that show the backdrop.
    assert_eq!(blue(&empty), 0);
    for pixel in empty.pixels() {
        assert!(
            pixel[0] == 255 && pixel[1].abs_diff(pixel[2]) <= 1,
            "{pixel:?}"
        );
    }
    // The stroke is the same as at 100%, outside the glyphs (where they are not even faintly
    // under it).
    assert!(red(&empty) >= red(&full));
    let ring = full
        .pixels()
        .zip(empty.pixels())
        .filter(|(full, _)| full[0] > 250 && full[1] < 5 && full[2] < 5)
        .find(|(full, empty)| full.0.iter().zip(empty.0).any(|(a, b)| a.abs_diff(b) > 2));
    assert!(ring.is_none(), "{ring:?}");
}

/// Fill fades the pixels and leaves the effects; Opacity fades both; the drop shadow stays
/// knocked out under the faded pixels.
#[test]
fn fill_fades_the_pixels_and_opacity_the_effects_too() {
    let mut square = Layer::image("Blue", RgbaImage::from_pixel(8, 8, Rgba([0, 0, 255, 255])));
    square.transform.x = 8.0;
    square.transform.y = 8.0;
    let mut effects = stroke(2.0, [255, 0, 0]);
    effects.drop_shadow = Some(ShadowEffect {
        distance: 4.0,
        blur: 0.0,
        opacity: 1.0,
        angle: 0.0,
        ..ShadowEffect::DROP
    });
    square.effects = Some(effects);
    let mut document = document(vec![white(28, 24), square]);
    let at = |document: &Document, x, y| render(document).get_pixel(x, y).0;
    // Inside, on the stroke, and on the shadow beyond the stroke on the left (the light comes
    // from the right).
    let (inside, ring, shadow) = ((11, 11), (6, 11), (19, 11));
    assert_eq!(at(&document, inside.0, inside.1), [0, 0, 255, 255]);
    assert_eq!(at(&document, ring.0, ring.1), [255, 0, 0, 255]);
    let full_shadow = at(&document, shadow.0, shadow.1);

    document.layers[1].fill = 0.5;
    let half = at(&document, inside.0, inside.1);
    assert!(half[0].abs_diff(128) <= 1 && half[1].abs_diff(128) <= 1 && half[2] == 255);
    assert_eq!(at(&document, ring.0, ring.1), [255, 0, 0, 255]);
    assert_eq!(at(&document, shadow.0, shadow.1), full_shadow);

    document.layers[1].fill = 1.0;
    document.layers[1].opacity = 0.5;
    let faded = at(&document, ring.0, ring.1);
    assert!(faded[0] == 255 && faded[1].abs_diff(128) <= 1 && faded[2].abs_diff(128) <= 1);

    // At Fill 0% the inside shows the backdrop, not the shadow the pixels hid.
    document.layers[1].opacity = 1.0;
    document.layers[1].fill = 0.0;
    document.layers[1]
        .effects
        .as_mut()
        .unwrap()
        .drop_shadow
        .as_mut()
        .unwrap()
        .distance = 2.0;
    assert_eq!(at(&document, inside.0, inside.1), [255; 4]);
    // A color overlay still covers the whole shape.
    let effects = document.layers[1].effects.as_mut().unwrap();
    effects.add(EffectKind::ColorOverlay, [0, 255, 0]);
    assert_eq!(at(&document, inside.0, inside.1), [0, 255, 0, 255]);
}

/// Each of the eight special modes at Fill 50% differs from Opacity 50%, and matches
/// `blend::composite_filled`; Multiply (like every other mode) does not.
#[test]
fn special_modes_at_half_fill_differ_from_half_opacity() {
    let backdrop = Layer::image(
        "Ramp",
        RgbaImage::from_fn(32, 1, |x, _| {
            Rgba([(x * 8) as u8, 200 - x as u8 * 6, 90, 255])
        }),
    );
    let layer = Layer::image(
        "Layer",
        RgbaImage::from_fn(32, 1, |x, _| {
            Rgba([255 - (x * 8) as u8, 128, 40 + x as u8, 255])
        }),
    );
    for mode in BlendMode::ALL {
        if mode == BlendMode::Dissolve {
            continue;
        }
        let mut filled = layer.clone();
        filled.blend = mode;
        filled.fill = 0.5;
        let mut faded = filled.clone();
        faded.fill = 1.0;
        faded.opacity = 0.5;
        let filled_image = render(&document(vec![backdrop.clone(), filled.clone()]));
        let faded_image = render(&document(vec![backdrop.clone(), faded]));
        if mode.fill_is_special() {
            assert_ne!(filled_image, faded_image, "{}", mode.name());
        } else {
            assert_eq!(filled_image, faded_image, "{}", mode.name());
        }
        for x in [3, 15, 28] {
            let unit = |p: [u8; 4]| p.map(|v| v as f32 / 255.0);
            let expected = crate::blend::composite_filled(
                unit(backdrop.pixels.as_ref().unwrap().get_pixel(x, 0).0),
                unit(layer.pixels.as_ref().unwrap().get_pixel(x, 0).0),
                mode,
                0.5,
            )
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            assert_eq!(
                filled_image.get_pixel(x, 0).0,
                expected,
                "{} {x}",
                mode.name()
            );
        }
    }
}

/// Fill thins Dissolve's pattern as opacity does; a clipping base at Fill 0% still shapes the
/// layers clipped to it, as in Photoshop.
#[test]
fn dissolve_and_clipping_bases_take_fill() {
    let mut dissolve = Layer::image("Dots", RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 255])));
    dissolve.blend = BlendMode::Dissolve;
    dissolve.fill = 0.0;
    let mut dots = document(vec![white(64, 64), dissolve]);
    assert!(render(&dots).pixels().all(|p| p.0 == [255; 4]));
    dots.layers[1].fill = 0.5;
    let kept = render(&dots).pixels().filter(|p| p[0] == 0).count();
    assert!((1700..2400).contains(&kept), "{kept}");

    let mut base = Layer::image(
        "Base",
        RgbaImage::from_fn(4, 1, |x, _| Rgba([0, 0, 0, if x < 2 { 255 } else { 0 }])),
    );
    base.fill = 0.0;
    let mut clipped = Layer::image("Red", RgbaImage::from_pixel(4, 1, Rgba([255, 0, 0, 255])));
    clipped.clip_to = Some(base.id);
    let document = document(vec![white(4, 1), base, clipped]);
    let image = render(&document);
    assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(3, 0).0, [255; 4]);
}

/// Duplicates and pasted copies keep the fill.
#[test]
fn copies_keep_the_fill() {
    let mut layer = white(4, 4);
    layer.fill = 0.3;
    let mut source = document(vec![white(4, 4), layer]);
    let id = source.layers[1].id;
    source.select(id, false);
    crate::operations::duplicate(&mut source);
    assert_eq!(source.layers.len(), 3);
    assert_eq!(source.layers[2].fill, 0.3);
    let mut destination = Document::new(4, 4).unwrap();
    crate::operations::paste_layers(&source, &[id], &mut destination).unwrap();
    let pasted = destination.active().unwrap();
    assert_ne!(pasted.id, id);
    assert_eq!(pasted.fill, 0.3);
}

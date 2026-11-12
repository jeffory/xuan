use super::*;
use image::Rgba;

/// A `size`-pixel opaque square of `color` in the middle of a transparent `side` raster.
fn square(side: u32, size: u32, color: [u8; 3]) -> RgbaImage {
    let start = (side - size) / 2;
    RgbaImage::from_fn(side, side, |x, y| {
        if (start..start + size).contains(&x) && (start..start + size).contains(&y) {
            Rgba([color[0], color[1], color[2], 255])
        } else {
            Rgba([0; 4])
        }
    })
}

fn only(kind: EffectKind) -> LayerEffects {
    let mut effects = LayerEffects::default();
    effects.add(kind, [255, 0, 0]);
    effects
}

#[test]
fn spread_matches_a_direct_search() {
    let (w, h) = (23, 17);
    let plane: Vec<f32> = (0..w * h)
        .map(|i| ((i * 7919) % 101) as f32 / 100.0)
        .collect();
    for reach in [1, 2, 5, 30] {
        for smallest in [false, true] {
            let fast = spread(&plane, w, h, reach, smallest);
            for y in 0..h {
                for x in 0..w {
                    // Past the edge there is nothing: zero.
                    let at = |x: i64, y: i64| {
                        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                            0.0
                        } else {
                            plane[y as usize * w + x as usize]
                        }
                    };
                    let mut best: f32 = if smallest { 1.0 } else { 0.0 };
                    let r = reach as i64;
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let v = at(x as i64 + dx, y as i64 + dy);
                            best = if smallest { best.min(v) } else { best.max(v) };
                        }
                    }
                    assert_eq!(fast[y * w + x], best, "{x},{y} reach {reach} {smallest}");
                }
            }
        }
    }
}

#[test]
fn an_outside_stroke_rings_the_shape_and_an_inside_one_covers_its_edge() {
    let source = square(20, 10, [0, 0, 255]);
    let mut effects = LayerEffects {
        stroke: Some(StrokeEffect {
            size: 2.0,
            color: [255, 0, 0],
            ..StrokeEffect::default()
        }),
        ..LayerEffects::default()
    };
    let result = render_cpu(&source, &effects, 1.0);
    // Square from 5 to 14; the stroke reaches two pixels out, square, not round.
    assert_eq!(result.get_pixel(3, 3).0, [255, 0, 0, 255]);
    assert_eq!(result.get_pixel(4, 9).0, [255, 0, 0, 255]);
    assert_eq!(result.get_pixel(2, 9).0[3], 0);
    assert_eq!(result.get_pixel(5, 5).0, [0, 0, 255, 255]);
    effects.stroke.as_mut().unwrap().inside = true;
    let result = render_cpu(&source, &effects, 1.0);
    assert_eq!(result.get_pixel(4, 9).0[3], 0);
    assert_eq!(result.get_pixel(6, 9).0, [255, 0, 0, 255]);
    assert_eq!(result.get_pixel(7, 9).0, [0, 0, 255, 255]);
}

#[test]
fn a_drop_shadow_falls_away_from_the_light() {
    let source = square(40, 10, [255; 3]);
    let effects = LayerEffects {
        drop_shadow: Some(ShadowEffect {
            angle: 90.0,
            distance: 8.0,
            blur: 0.0,
            opacity: 1.0,
            ..ShadowEffect::DROP
        }),
        ..LayerEffects::default()
    };
    let result = render_cpu(&source, &effects, 1.0);
    // Light from above: the shadow drops straight down by eight pixels.
    // The square covers 15–24; its shadow 23–32.
    assert_eq!(result.get_pixel(20, 30).0, [0, 0, 0, 255]);
    assert_eq!(result.get_pixel(20, 33).0[3], 0);
    assert_eq!(result.get_pixel(20, 12).0[3], 0);
    assert_eq!(result.get_pixel(20, 20).0, [255, 255, 255, 255]);
    // Blurred, it fades toward its edge, and a 0° light throws it to the left.
    let soft = LayerEffects {
        drop_shadow: Some(ShadowEffect {
            angle: 0.0,
            ..ShadowEffect::DROP
        }),
        ..LayerEffects::default()
    };
    let result = render_cpu(&square(80, 10, [255; 3]), &soft, 1.0);
    let left = result.get_pixel(20, 40).0[3];
    let right = result.get_pixel(55, 40).0[3];
    assert!(left > 10 && right == 0, "{left} {right}");
}

#[test]
fn color_overlay_keeps_the_alpha_of_semi_transparent_pixels() {
    for alpha in [1u8, 64, 128, 200, 254] {
        let source = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, alpha]));
        let mut effects = LayerEffects::default();
        effects.add(EffectKind::ColorOverlay, [255, 255, 255]);
        let result = render_cpu(&source, &effects, 1.0);
        assert_eq!(
            result.get_pixel(1, 1).0,
            [255, 255, 255, alpha],
            "alpha {alpha}"
        );
        for opacity in [0.5f32, 0.25] {
            effects.color_overlay.as_mut().unwrap().opacity = opacity;
            let result = render_cpu(&source, &effects, 1.0);
            let p = result.get_pixel(1, 1).0;
            let expected = (opacity * 255.0).round() as i32;
            assert!((p[0] as i32 - expected).abs() <= 1, "{p:?} at {opacity}");
            assert_eq!(p[3], alpha);
        }
    }
}

#[test]
fn overlay_and_inner_effects_stay_inside_the_shape() {
    let source = square(40, 20, [0, 0, 255]);
    let overlay = render_cpu(&source, &only(EffectKind::ColorOverlay), 1.0);
    assert_eq!(overlay.get_pixel(20, 20).0, [255, 0, 0, 255]);
    assert_eq!(overlay.get_pixel(5, 5).0[3], 0);
    for kind in [EffectKind::InnerShadow, EffectKind::InnerGlow] {
        let mut effects = only(kind);
        effects
            .inner_glow
            .iter_mut()
            .for_each(|g| g.color = [255, 0, 0]);
        let result = render_cpu(&source, &effects, 1.0);
        assert!(
            result
                .pixels()
                .zip(source.pixels())
                .all(|(r, s)| r[3] == s[3])
        );
        assert_ne!(result.get_pixel(10, 20).0, [0, 0, 255, 255], "{kind:?}");
    }
    // The inner glow is strongest at the edge and fades inward.
    let result = render_cpu(&source, &only(EffectKind::InnerGlow), 1.0);
    assert!(result.get_pixel(10, 20)[0] > result.get_pixel(14, 20)[0]);
}

#[test]
fn an_outer_glow_surrounds_the_shape_without_covering_it() {
    let source = square(60, 10, [0, 0, 255]);
    let result = render_cpu(&source, &only(EffectKind::OuterGlow), 1.0);
    assert_eq!(result.get_pixel(30, 30).0, [0, 0, 255, 255]);
    let near = result.get_pixel(23, 30);
    let far = result.get_pixel(12, 30);
    assert_eq!(&near.0[..3], &[255, 255, 255]);
    assert!(near[3] > far[3] && far[3] > 0, "{near:?} {far:?}");
}

#[test]
fn margins_ranges_and_defaults_follow_upstream() {
    assert_eq!(margin(&only(EffectKind::Stroke)), 6);
    assert_eq!(margin(&only(EffectKind::DropShadow)), 20 + 60 + 2);
    assert_eq!(margin(&only(EffectKind::OuterGlow)), 62);
    assert_eq!(margin(&only(EffectKind::InnerGlow)), 2);
    let mut hidden = only(EffectKind::OuterGlow);
    hidden.set_enabled(EffectKind::OuterGlow, false);
    assert!(hidden.visible().is_empty() && !hidden.is_empty());
    assert!(only(EffectKind::DropShadow).validate().is_ok());
    let mut wrong = only(EffectKind::Stroke);
    wrong.stroke.as_mut().unwrap().size = 501.0;
    assert!(wrong.validate().is_err());
    let mut wrong = only(EffectKind::DropShadow);
    wrong.drop_shadow.as_mut().unwrap().distance = f32::NAN;
    assert!(wrong.validate().is_err());
    // Missing fields take the defaults, so hand-written or older records stay readable.
    let parsed: LayerEffects =
        serde_json::from_str(r#"{"outer_glow": {"size": 5}, "stroke": {}}"#).unwrap();
    assert_eq!(parsed.outer_glow.unwrap().opacity, 0.75);
    assert_eq!(parsed.stroke.unwrap(), StrokeEffect::default());
    let [dx, dy] = ShadowEffect::DROP.offset();
    assert!(dx.abs() < 1e-4 && (dy - 20.0).abs() < 1e-4);
}

fn document_with(layer: Layer) -> Document {
    let mut document = Document::new(40, 40).unwrap();
    document.layers = vec![layer];
    document
}

fn red_square() -> Layer {
    let mut layer = Layer::image("Square", square(20, 20, [0, 0, 255]));
    layer.transform.x = 10.0;
    layer.transform.y = 10.0;
    layer.effects = Some(LayerEffects {
        stroke: Some(StrokeEffect {
            size: 3.0,
            color: [255, 0, 0],
            ..StrokeEffect::default()
        }),
        ..LayerEffects::default()
    });
    layer
}

#[test]
fn rendering_draws_effects_around_the_layer_at_its_opacity() {
    let mut document = document_with(red_square());
    let image = crate::render::render(&document);
    assert_eq!(image.get_pixel(8, 20).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(20, 20).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(5, 20).0[3], 0);
    // Opacity dims the stroke too (upstream draws the whole result at the layer's opacity),
    // and so does a folder's.
    document.layers[0].opacity = 0.5;
    assert_eq!(crate::render::render(&document).get_pixel(8, 20).0[3], 128);
    // Hidden effects leave the layer as it is.
    document.layers[0]
        .effects
        .as_mut()
        .unwrap()
        .set_enabled(EffectKind::Stroke, false);
    assert_eq!(crate::render::render(&document).get_pixel(8, 20).0[3], 0);
    // The source pixels are never changed.
    assert_eq!(
        document.layers[0].pixels.as_ref().unwrap().dimensions(),
        (20, 20)
    );
}

#[test]
fn effects_follow_the_mask_and_are_clipped_with_the_layer() {
    // The mask hides the right half, so the stroke follows the left half's edge.
    let mut layer = red_square();
    layer.mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_fn(2, 1, |x, _| {
            image::Luma([if x == 0 { 255 } else { 0 }])
        })),
        ..Mask::white()
    });
    let image = crate::render::render(&document_with(layer));
    assert_eq!(image.get_pixel(15, 20).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(25, 20).0[3], 0);
    // The stroke along the mask's edge, inside the layer's own bounds.
    assert_eq!(image.get_pixel(21, 20).0, [255, 0, 0, 255]);

    // Clipped to a base covering only the top half, the stroke is cut there too.
    let base = Layer::image(
        "Base",
        RgbaImage::from_fn(40, 40, |_, y| {
            Rgba([255, 255, 255, if y < 20 { 255 } else { 0 }])
        }),
    );
    let mut clipped = red_square();
    clipped.clip_to = Some(base.id);
    let mut document = document_with(base);
    document.layers.push(clipped);
    let image = crate::render::render(&document);
    assert_eq!(image.get_pixel(8, 15).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(8, 25).0[3], 0);
}

#[test]
fn a_clipping_base_with_effects_lends_them_its_shape() {
    // Upstream clips to the base as drawn, effects included.
    let base = red_square();
    let mut top = Layer::image("Top", RgbaImage::from_pixel(40, 40, Rgba([0, 255, 0, 255])));
    top.clip_to = Some(base.id);
    let mut document = document_with(base);
    document.layers.push(top);
    let image = crate::render::render(&document);
    assert_eq!(image.get_pixel(8, 20).0, [0, 255, 0, 255]);
    assert_eq!(image.get_pixel(4, 20).0[3], 0);
}

/// On a 40 × 40 canvas: a text layer with a stroke and a sharp drop shadow, and a shape with an
/// outer glow, both clear of the canvas edges.
fn styled_document() -> Document {
    let mut text = red_square();
    text.name = "Text".into();
    text.text = Some(crate::text::TextStyle {
        content: "Hello".into(),
        ..Default::default()
    });
    text.effects.as_mut().unwrap().drop_shadow = Some(ShadowEffect {
        distance: 2.0,
        blur: 0.0,
        opacity: 1.0,
        ..ShadowEffect::DROP
    });
    let mut shape = Layer::image("Shape", square(4, 4, [0, 255, 0]));
    shape.transform.x = 24.0;
    shape.transform.y = 14.0;
    shape.shape = Some(crate::document::ShapeStyle {
        kind: crate::paint::ShapeKind::Rectangle,
        color: [0, 255, 0, 255],
        corner_radius: 0.0,
        path: None,
    });
    shape.effects = Some(LayerEffects {
        outer_glow: Some(GlowEffect {
            size: 3.0,
            ..GlowEffect::OUTER
        }),
        ..LayerEffects::default()
    });
    let mut document = document_with(text);
    document.layers.push(shape);
    let id = document.layers[0].id;
    document.select(id, false);
    document
}

/// Asserts that `after` shows `before` moved by whole pixels: `after`'s pixel (x, y) is
/// `before`'s (x + left, y + top), or transparent where that is off `before`.
fn assert_moved(name: &str, before: &RgbaImage, after: &RgbaImage, left: i64, top: i64) {
    for (x, y, pixel) in after.enumerate_pixels() {
        let (sx, sy) = (i64::from(x) + left, i64::from(y) + top);
        let expected = if (0..i64::from(before.width())).contains(&sx)
            && (0..i64::from(before.height())).contains(&sy)
        {
            *before.get_pixel(sx as u32, sy as u32)
        } else {
            Rgba([0; 4])
        };
        assert_eq!(*pixel, expected, "{name} at {x}, {y}");
    }
}

/// Upstream Compositor#180: Canvas Size, Image Size, Crop and Trim dropped every layer's
/// effects and turned text and shapes into plain pixels. Xuan's canvas edits only move, scale,
/// turn or flip layers, so each layer comes through with its effects, its text or shape and its
/// own pixels; the edits that only move layers draw exactly what was there before, effects and
/// all (issue 122).
#[test]
fn canvas_edits_keep_effects_and_live_text() {
    use crate::operations::{self, CanvasRotation, TrimBasis, TrimSides};
    let document = styled_document();
    let before = crate::render::render(&document);
    let trimmed = operations::trim_bounds(&before, TrimBasis::Transparent, TrimSides::default())
        .expect("the effects stay clear of the canvas edges");
    type Edit = (&'static str, fn(&mut Document));
    let edits: [Edit; 7] = [
        ("Canvas Size", |d| {
            operations::canvas_size(d, 60, 50, [0.5, 0.5]).unwrap()
        }),
        ("Image Size", |d| operations::image_size(d, 80, 60).unwrap()),
        ("Crop", |d| {
            operations::crop(d, Point::new(4.0, 4.0), Point::new(36.0, 30.0)).unwrap()
        }),
        ("Trim", |d| {
            assert!(operations::trim(d, TrimBasis::Transparent, TrimSides::default()).unwrap())
        }),
        ("Crop to Selection", |d| {
            d.selection = Some(Arc::new(GrayImage::from_fn(40, 40, |x, y| {
                image::Luma([if (2..38).contains(&x) && (6..34).contains(&y) {
                    255
                } else {
                    0
                }])
            })));
            operations::crop_to_selection(d).unwrap()
        }),
        ("Rotate Canvas", |d| {
            operations::rotate_canvas(d, CanvasRotation::Clockwise)
        }),
        ("Flip Canvas", |d| operations::flip_canvas(d, true)),
    ];
    for (name, edit) in edits {
        let mut edited = document.clone();
        edit(&mut edited);
        edited.validate().unwrap();
        assert_eq!(edited.layers.len(), document.layers.len(), "{name}");
        for (old, new) in document.layers.iter().zip(&edited.layers) {
            let layer = format!("{name}: {}", old.name);
            assert_eq!(new.effects, old.effects, "{layer}");
            assert_eq!(new.text, old.text, "{layer}");
            // `ShapeStyle` has no `PartialEq`.
            assert_eq!(
                format!("{:?}", new.shape),
                format!("{:?}", old.shape),
                "{layer}"
            );
            assert!(
                Arc::ptr_eq(old.pixels.as_ref().unwrap(), new.pixels.as_ref().unwrap()),
                "{layer} was redrawn"
            );
        }
        let after = crate::render::render(&edited);
        match name {
            "Canvas Size" => assert_moved(name, &before, &after, -10, -5),
            "Crop" => assert_moved(name, &before, &after, 4, 4),
            "Crop to Selection" => assert_moved(name, &before, &after, 2, 6),
            "Trim" => {
                let (left, top, right, bottom) = trimmed;
                assert_eq!(after.dimensions(), (right - left, bottom - top));
                assert_moved(name, &before, &after, left.into(), top.into());
            }
            _ => {}
        }
    }
}

/// The rows (or columns, `across`) of `image` along column (or row) `line` that are not
/// transparent: their first and last index.
fn drawn_span(image: &RgbaImage, line: u32, across: bool) -> (u32, u32) {
    let length = if across {
        image.width()
    } else {
        image.height()
    };
    let drawn: Vec<u32> = (0..length)
        .filter(|&i| {
            let pixel = if across {
                image.get_pixel(i, line)
            } else {
                image.get_pixel(line, i)
            };
            pixel[3] > 127
        })
        .collect();
    (*drawn.first().unwrap(), *drawn.last().unwrap())
}

/// Issue 122 asks whether Image Size scales effects with the image, as Photoshop's "Scale
/// Styles" does. Effect sizes are in layer pixels and Image Size scales the layer, so they do:
/// at twice the size, the stroke is twice as wide, and the shadow falls twice as far and is
/// twice as soft.
#[test]
fn image_size_scales_effects_with_the_layer() {
    let mut layer = red_square();
    layer.effects.as_mut().unwrap().drop_shadow = Some(ShadowEffect {
        distance: 4.0,
        blur: 0.0,
        opacity: 1.0,
        ..ShadowEffect::DROP
    });
    let sharp = document_with(layer);
    let mut doubled = sharp.clone();
    crate::operations::image_size(&mut doubled, 80, 80).unwrap();
    let (before, after) = (
        crate::render::render(&sharp),
        crate::render::render(&doubled),
    );
    // The square covers 10–29 and its 3-pixel stroke 7–32; the square's shadow drops 4 pixels,
    // to 33.
    assert_eq!(drawn_span(&before, 20, true), (7, 32));
    assert_eq!(drawn_span(&before, 20, false), (7, 33));
    let (left, right) = drawn_span(&after, 40, true);
    let (top, bottom) = drawn_span(&after, 40, false);
    for (side, value, expected) in [
        ("left", left, 14),
        ("right", right, 65),
        ("top", top, 14),
        ("bottom", bottom, 67),
    ] {
        assert!(
            value.abs_diff(expected) <= 1,
            "{side}: {value}, not {expected}"
        );
    }
    // A soft shadow fades from 90% to 10% over twice the rows.
    let soft = |document: &Document, line: u32| {
        let image = crate::render::render(document);
        (0..image.height())
            .filter(|&y| (26..=229).contains(&image.get_pixel(line, y)[3]))
            .count()
    };
    let mut blurred = sharp.clone();
    let shadow = blurred.layers[0].effects.as_mut().unwrap();
    shadow.drop_shadow.as_mut().unwrap().blur = 6.0;
    shadow.stroke = None;
    let mut doubled = blurred.clone();
    crate::operations::image_size(&mut doubled, 80, 80).unwrap();
    let (fade, scaled) = (soft(&blurred, 20), soft(&doubled, 40));
    assert!(fade > 3, "{fade}");
    assert!(
        scaled.abs_diff(fade * 2) <= 2,
        "{fade} rows became {scaled}"
    );
}

/// Upstream Compositor#180 also lost a layer's effects when Hue/Saturation, Levels or Invert was
/// applied to it directly. Here they change the layer's pixels and keep its effects.
#[test]
fn direct_adjustments_keep_the_layers_effects() {
    use crate::document::Adjustment;
    let mut hue = crate::color::HueSettings::default();
    hue.adjustments[0][0] = 120.0;
    let mut levels = [crate::color::DEFAULT_LEVELS; 4];
    levels[0][4] = 200.0;
    for adjustment in [
        Adjustment::HueRanges {
            settings: Box::new(hue),
        },
        Adjustment::LevelsChannels { ranges: levels },
        Adjustment::Invert,
    ] {
        let mut document = document_with(red_square());
        let id = document.layers[0].id;
        document.select(id, false);
        let (effects, pixels) = (
            document.layers[0].effects.clone(),
            document.layers[0].pixels.clone().unwrap(),
        );
        crate::effects::apply_adjustment(&mut document, &adjustment, false).unwrap();
        let layer = &document.layers[0];
        let name = adjustment.name();
        assert_ne!(
            **layer.pixels.as_ref().unwrap(),
            *pixels,
            "{name} changed nothing"
        );
        assert_eq!(layer.effects, effects, "{name}");
    }
}

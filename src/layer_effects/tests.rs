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
    let result = render_cpu(&source, &effects);
    // Square from 5 to 14; the stroke reaches two pixels out, square, not round.
    assert_eq!(result.get_pixel(3, 3).0, [255, 0, 0, 255]);
    assert_eq!(result.get_pixel(4, 9).0, [255, 0, 0, 255]);
    assert_eq!(result.get_pixel(2, 9).0[3], 0);
    assert_eq!(result.get_pixel(5, 5).0, [0, 0, 255, 255]);
    effects.stroke.as_mut().unwrap().inside = true;
    let result = render_cpu(&source, &effects);
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
    let result = render_cpu(&source, &effects);
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
    let result = render_cpu(&square(80, 10, [255; 3]), &soft);
    let left = result.get_pixel(20, 40).0[3];
    let right = result.get_pixel(55, 40).0[3];
    assert!(left > 10 && right == 0, "{left} {right}");
}

#[test]
fn overlay_and_inner_effects_stay_inside_the_shape() {
    let source = square(40, 20, [0, 0, 255]);
    let overlay = render_cpu(&source, &only(EffectKind::ColorOverlay));
    assert_eq!(overlay.get_pixel(20, 20).0, [255, 0, 0, 255]);
    assert_eq!(overlay.get_pixel(5, 5).0[3], 0);
    for kind in [EffectKind::InnerShadow, EffectKind::InnerGlow] {
        let mut effects = only(kind);
        effects
            .inner_glow
            .iter_mut()
            .for_each(|g| g.color = [255, 0, 0]);
        let result = render_cpu(&source, &effects);
        assert!(
            result
                .pixels()
                .zip(source.pixels())
                .all(|(r, s)| r[3] == s[3])
        );
        assert_ne!(result.get_pixel(10, 20).0, [0, 0, 255, 255], "{kind:?}");
    }
    // The inner glow is strongest at the edge and fades inward.
    let result = render_cpu(&source, &only(EffectKind::InnerGlow));
    assert!(result.get_pixel(10, 20)[0] > result.get_pixel(14, 20)[0]);
}

#[test]
fn an_outer_glow_surrounds_the_shape_without_covering_it() {
    let source = square(60, 10, [0, 0, 255]);
    let result = render_cpu(&source, &only(EffectKind::OuterGlow));
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

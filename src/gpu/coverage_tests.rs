//! `coverage.wgsl` cannot run without a GPU in CI's default jobs, so these tests check that
//! the shader still validates and run its program on the CPU, mirroring the shader line by
//! line, against the CPU renderer's clipping alpha. The ignored GPU-vs-CPU tests in
//! `gpu.rs` and the `clipping_folder` golden run the shader itself.
use super::*;
use crate::document::{Adjustment, Mask, Point};
use image::{GrayImage, Luma, Rgba, RgbaImage};
use wgpu::naga;

#[test]
fn coverage_shader_validates() {
    let module =
        naga::front::wgsl::parse_str(include_str!("coverage.wgsl")).expect("shader parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("shader validates");
}

/// `coverage_at` in coverage.wgsl, without the optional output transform.
fn evaluate(coverage: &Coverage, position: [u32; 2]) -> f32 {
    let config = &coverage.config;
    let point = [
        (position[0] as f32 + 0.5) / config[0][0] * config[0][2],
        (position[1] as f32 + 0.5) / config[0][1] * config[0][3],
    ];
    let mut stack = [0.0_f32; STACK];
    let mut top = 0;
    stack[0] = config[1][0];
    for i in 0..config[1][1] as usize {
        let base = 2 + i * 6;
        let parameter = config[base + 4][3];
        match config[base + 3][3] as u32 {
            1 => {
                top += 1;
                stack[top] = parameter;
            }
            2 => {
                let a = stack[top];
                top -= 1;
                stack[top] = a + stack[top] * (1.0 - a);
            }
            3 => stack[top] *= parameter,
            5 => {
                let a = stack[top];
                top -= 1;
                stack[top] *= a;
            }
            op => {
                let value = sample_source(coverage, point, base);
                if op == 4 {
                    stack[top] *= 1.0 - parameter * (1.0 - value);
                } else {
                    stack[top] *= value;
                }
            }
        }
    }
    assert_eq!(top, 0, "the program leaves one value");
    stack[0]
}

/// `sample_source` in coverage.wgsl.
fn sample_source(coverage: &Coverage, point: [f32; 2], base: usize) -> f32 {
    let config = &coverage.config;
    let [header, bounds, rotation] = [config[base], config[base + 1], config[base + 2]];
    let local = [
        point[0] - bounds[0] - bounds[2] * 0.5,
        point[1] - bounds[1] - bounds[3] * 0.5,
    ];
    let uv = [
        (local[0] * rotation[0] + local[1] * rotation[1]) / bounds[2] * rotation[2] + 0.5,
        (-local[0] * rotation[1] + local[1] * rotation[0]) / bounds[3] * rotation[3] + 0.5,
    ];
    let dot = |row: [f32; 4]| row[0] * uv[0] + row[1] * uv[1] + row[2];
    let w = dot(config[base + 5]);
    let uv = [dot(config[base + 3]) / w, dot(config[base + 4]) / w];
    if uv.iter().any(|v| !(0.0..1.0).contains(v)) {
        return 0.0;
    }
    let size = [header[0] as u32, header[1] as u32];
    let offset = header[2].to_bits() as usize * 4;
    if header[3] == 0.0 {
        let p = [(uv[0] * header[0]) as u32, (uv[1] * header[1]) as u32];
        let index = (p[1] * size[0] + p[0]) as usize;
        return coverage.pixels[offset + index] as f32 / 255.0;
    }
    let alpha_at = |x: i32, y: i32| {
        let x = x.clamp(0, size[0] as i32 - 1) as usize;
        let y = y.clamp(0, size[1] as i32 - 1) as usize;
        coverage.pixels[offset + (y * size[0] as usize + x) * 4 + 3] as f32 / 255.0
    };
    let p = [uv[0] * header[0] - 0.5, uv[1] * header[1] - 0.5];
    let low = [p[0].floor() as i32, p[1].floor() as i32];
    let f = [p[0] - p[0].floor(), p[1] - p[1].floor()];
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    mix(
        mix(alpha_at(low[0], low[1]), alpha_at(low[0] + 1, low[1]), f[0]),
        mix(
            alpha_at(low[0], low[1] + 1),
            alpha_at(low[0] + 1, low[1] + 1),
            f[0],
        ),
        f[1],
    )
}

fn disc(name: &str, size: u32) -> Layer {
    Layer::image(
        name,
        RgbaImage::from_fn(size, size, |x, y| {
            let d =
                (x as i32 * 2 + 1 - size as i32).pow(2) + (y as i32 * 2 + 1 - size as i32).pow(2);
            Rgba([
                255,
                255,
                255,
                if d < (size * size) as i32 { 230 } else { 0 },
            ])
        }),
    )
}

/// A folder base with everything that shapes it: moved, stretched and rotated layers, a
/// nested folder with opacity, a mask layer, a hidden layer, an adjustment, a layer clipped
/// inside the folder, the folder's opacity and mask, and two layers clipped on top, one
/// through a chain. Bottom to top; the folder is at index 9.
pub(crate) fn folder_base() -> Document {
    let mut doc = Document::new(48, 40).unwrap();
    let mut group = Layer::blank("Figure", 48, 40);
    group.group = true;
    group.opacity = 0.8;
    group.mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_fn(48, 40, |x, _| Luma([(x * 5) as u8]))),
        ..Mask::white()
    });
    let mut cloak = disc("Cloak", 20);
    cloak.parent = Some(group.id);
    cloak.transform.x = 4.0;
    cloak.transform.y = 6.0;
    cloak.opacity = 0.9;
    let mut hood = disc("Hood", 16);
    hood.parent = Some(group.id);
    hood.transform.x = 18.0;
    hood.transform.y = 3.0;
    hood.transform.width = 24.0;
    hood.transform.rotation = 30.0;
    let mut hood_shade = disc("Hood shade", 10);
    hood_shade.parent = Some(group.id);
    hood_shade.transform.x = 22.0;
    hood_shade.transform.y = 8.0;
    hood_shade.clip_to = Some(hood.id);
    let mut hidden = disc("Hidden", 40);
    hidden.parent = Some(group.id);
    hidden.visible = false;
    let mut invert = Layer::blank("Invert", 48, 40);
    invert.adjustment = Some(Adjustment::Invert);
    invert.parent = Some(group.id);
    let mut inner = Layer::blank("Hands", 48, 40);
    inner.group = true;
    inner.parent = Some(group.id);
    inner.opacity = 0.6;
    let mut hand = disc("Hand", 12);
    hand.parent = Some(inner.id);
    hand.transform.x = 30.0;
    hand.transform.y = 24.0;
    let mut fade = Layer::mask("Fade", 48, 40);
    fade.parent = Some(group.id);
    fade.opacity = 0.7;
    fade.mask.as_mut().unwrap().pixels = Arc::new(GrayImage::from_fn(48, 40, |_, y| {
        Luma([if y < 20 { 255 } else { 40 }])
    }));
    let mut book = disc("Book", 14);
    book.parent = Some(group.id);
    book.transform.x = 2.0;
    book.transform.y = 22.0;
    let mut shading = Layer::image(
        "Shading",
        RgbaImage::from_pixel(48, 40, Rgba([40, 20, 90, 255])),
    );
    shading.blend = crate::blend::BlendMode::Multiply;
    shading.clip_to = Some(group.id);
    let mut rim = Layer::image(
        "Rim",
        RgbaImage::from_pixel(48, 40, Rgba([255, 255, 0, 200])),
    );
    rim.clip_to = Some(shading.id);
    rim.mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_fn(48, 40, |x, y| {
            Luma([((x + y) * 3) as u8])
        })),
        ..Mask::white()
    });
    doc.layers = vec![
        cloak, hood, hood_shade, hidden, invert, inner, hand, fade, book, group, shading, rim,
    ];
    doc.validate().unwrap();
    doc
}

fn assert_matches(
    doc: &Document,
    layer: &Layer,
    mode: CoverageMode,
    expected: impl Fn(Point) -> f32,
) {
    let size = [doc.width, doc.height];
    let coverage = Coverage::new(doc, layer, size, mode);
    assert!(coverage.supported());
    let mut differences = 0;
    let mut total = 0.0;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let actual = evaluate(&coverage, [x, y]);
            let expected = expected(Point::new(x as f32 + 0.5, y as f32 + 0.5));
            total += expected;
            if (actual - expected).abs() > 0.002 {
                differences += 1;
            }
        }
    }
    assert!(total > 10.0, "the scene covers something");
    // Mask lookups take the nearest pixel, which may land across a pixel edge when the
    // shader and the CPU round a transformed position differently; more is a real bug.
    assert!(
        differences <= 4,
        "{differences} pixels differ for {}",
        layer.name
    );
}

#[test]
fn folder_base_programs_match_the_cpu_clipping_alpha() {
    let doc = folder_base();
    let group = &doc.layers[9];
    assert!(group.group);
    // The folder's own alpha, as clipping bakes and merges use it.
    assert_matches(&doc, group, CoverageMode::Alpha, |p| {
        render::layer_alpha(&doc, group, p, 0)
    });
    // The coverage of the layers clipped on top, as the compositor uses it.
    for layer in &doc.layers[10..] {
        assert_matches(&doc, layer, CoverageMode::Composite, |p| {
            let mut alpha = render::own_mask(layer, p) * render::inherited_coverage(&doc, layer, p);
            if let Some(base) = layer
                .clip_to
                .and_then(|id| doc.layers.iter().find(|l| l.id == id))
            {
                alpha *= render::layer_alpha(&doc, base, p, 0);
            }
            alpha
        });
    }
    // A source used twice (the clipped layer's base and its base's pixels) is uploaded once.
    let coverage = Coverage::new(&doc, &doc.layers[11], [48, 40], CoverageMode::Composite);
    let sources: usize = doc
        .layers
        .iter()
        .filter_map(|l| l.pixels.as_ref().map(|p| padded(p.as_raw()).len()))
        .sum::<usize>()
        + doc
            .layers
            .iter()
            .filter_map(|l| l.mask.as_ref().map(|m| padded(m.pixels.as_raw()).len()))
            .sum::<usize>();
    assert!(coverage.pixels.len() <= sources);
}

#[test]
fn layer_bases_keep_their_flat_program() {
    // Without a folder the program only multiplies, with opacities folded in.
    let mut doc = Document::new(8, 8).unwrap();
    let mut base = Layer::image("Base", RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255])));
    base.opacity = 0.5;
    let mut clipped = Layer::image("Clipped", RgbaImage::new(8, 8));
    clipped.clip_to = Some(base.id);
    doc.layers = vec![base, clipped];
    let coverage = Coverage::new(&doc, &doc.layers[1], [8, 8], CoverageMode::Composite);
    assert_eq!(coverage.config[1][0], 0.5);
    assert_eq!(coverage.instructions(), 1);
    assert_eq!(coverage.max_depth, 0);
    assert_eq!(evaluate(&coverage, [3, 3]), 0.5);
}

#[test]
fn programs_the_shader_cannot_hold_are_left_to_the_cpu() {
    // Folders nested deeper than the shader's stack: each level holds two values.
    let mut doc = Document::new(4, 4).unwrap();
    doc.layers.clear();
    let mut parent = None;
    for depth in 0..40 {
        let mut group = Layer::blank(format!("Folder {depth}"), 4, 4);
        group.group = true;
        group.parent = parent;
        parent = Some(group.id);
        doc.layers.push(group);
    }
    let mut leaf = Layer::image("Leaf", RgbaImage::new(4, 4));
    leaf.parent = parent;
    doc.layers.insert(0, leaf);
    let outer = doc.layers[1].clone();
    assert!(!Coverage::new(&doc, &outer, [4, 4], CoverageMode::Alpha).supported());
    let inner = doc.layers[35].clone();
    assert!(Coverage::new(&doc, &inner, [4, 4], CoverageMode::Alpha).supported());
}

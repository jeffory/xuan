//! Composition in tiles, for documents larger than the GPU's textures or readback buffers.
//! Each tile composites a window around the pixels it keeps, wide enough for the filter layers
//! that read across its edges, as a document of its own: its layers move into the window, and
//! upright layers keep only the pixels the window samples, so a layer larger than a texture
//! still uploads.
use std::sync::Arc;

use anyhow::{Result, ensure};
use image::RgbaImage;

use crate::{
    document::{Document, Layer, Point},
    effects::Filter,
    render,
};

/// A tile's kept pixels: big enough that margins cost little, small enough that the
/// compositor's half-float buffers stay near a hundred megabytes each.
pub(super) const TILE: u32 = 4096;
/// The smallest core worth compositing; below it, the CPU composites the document.
const MINIMUM_TILE: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The canvas pixels a tile keeps (`core`) and the larger area it composites (`window`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tile {
    pub core: Rect,
    pub window: Rect,
}

/// The limits a composition keeps within: the device's, or smaller ones in tests.
#[derive(Clone, Copy, Debug)]
pub(super) struct Tiling {
    /// The largest texture side.
    pub texture: u32,
    /// The largest buffer, which holds a readback.
    pub buffer: u64,
    /// A tile core's side.
    pub tile: u32,
}

impl Tiling {
    pub fn for_limits(limits: &wgpu::Limits) -> Self {
        Self {
            texture: limits.max_texture_dimension_2d,
            buffer: limits.max_buffer_size,
            tile: TILE,
        }
    }

    /// Whether a composition of `size` fits one texture and its readback one buffer.
    pub fn fits(&self, [width, height]: [u32; 2]) -> bool {
        let stride = (u64::from(width) * 4).div_ceil(256) * 256;
        width.max(height) <= self.texture && stride * u64::from(height) <= self.buffer
    }

    /// The largest square window: within a texture, and its readback within a buffer.
    fn window(&self) -> u32 {
        // Sides in multiples of 64 make rows of 256 bytes, which need no padding.
        let side = (self.buffer / 4).isqrt().min(u64::from(u32::MAX)) as u32 / 64 * 64;
        self.texture.min(side)
    }

    /// The core side for windows `margin` wider on each side, or `None` when not even one
    /// tile fits.
    pub fn core(&self, margin: u32) -> Option<u32> {
        let core = self
            .tile
            .min(self.window().saturating_sub(margin.saturating_mul(2)));
        (core >= MINIMUM_TILE).then_some(core)
    }
}

/// How far from the pixel it writes a filter layer reads, or `None` when it reads where on
/// the canvas a pixel is (Noise, Lens Correction, Vignette, Dither), which tiles would change.
pub(super) fn reach(filter: &Filter) -> Option<u32> {
    match *filter {
        Filter::GaussianBlur { radius }
        | Filter::Bloom { radius, .. }
        | Filter::TonalContrast { radius, .. } => {
            // The kernel's half, and a pixel to spare.
            Some(super::filter_layers::blur_length(radius) / 2 + 1)
        }
        // Samples reach half the distance either way, and one more texel for bilinear weights.
        Filter::MotionBlur { distance, .. } => {
            Some(((distance.abs() * 0.5).ceil() as u32).saturating_add(2))
        }
        Filter::Noise { .. }
        | Filter::LensCorrection { .. }
        | Filter::Vignette { .. }
        | Filter::Dither(_) => None,
    }
}

/// The margin every tile needs: filter layers stacked on each other read each other's
/// output, so their reaches add up. `None` when a filter layer can't be tiled.
pub(super) fn margin(document: &Document) -> Option<u32> {
    document
        .layers
        .iter()
        .filter(|layer| layer.visible)
        .filter_map(|layer| layer.filter.as_ref())
        .try_fold(0_u32, |total, filter| {
            Some(total.saturating_add(reach(filter)?))
        })
}

/// Tiles of `core` pixels covering `size`, each with a window `margin` wider on each side
/// within the canvas. Tiles of one window size come together, so the compositor keeps its
/// buffers from one tile to the next.
pub(super) fn grid([width, height]: [u32; 2], core: u32, margin: u32) -> Vec<Tile> {
    let core = core.max(1);
    let span = |start: u32, length: u32, total: u32| {
        let end = start.saturating_add(length).min(total);
        let from = start.saturating_sub(margin);
        (
            end - start,
            from,
            end.saturating_add(margin).min(total) - from,
        )
    };
    let mut tiles = Vec::new();
    for y in (0..height).step_by(core as usize) {
        for x in (0..width).step_by(core as usize) {
            let (core_width, window_x, window_width) = span(x, core, width);
            let (core_height, window_y, window_height) = span(y, core, height);
            tiles.push(Tile {
                core: Rect {
                    x,
                    y,
                    width: core_width,
                    height: core_height,
                },
                window: Rect {
                    x: window_x,
                    y: window_y,
                    width: window_width,
                    height: window_height,
                },
            });
        }
    }
    tiles.sort_by_key(|tile| (tile.window.width, tile.window.height));
    tiles
}

/// Copies `tile`'s core from its window's readback (rows `stride` bytes apart) into `output`.
pub(super) fn stitch(
    output: &mut RgbaImage,
    tile: &Tile,
    window: &[u8],
    stride: usize,
) -> Result<()> {
    let (core, outer) = (tile.core, tile.window);
    ensure!(
        core.x >= outer.x
            && core.y >= outer.y
            && core.x + core.width <= outer.x + outer.width
            && core.y + core.height <= outer.y + outer.height
            && core.x + core.width <= output.width()
            && core.y + core.height <= output.height()
            && stride >= outer.width as usize * 4
            && window.len() >= stride * outer.height as usize,
        "Tile outside its window"
    );
    let row = core.width as usize * 4;
    let left = (core.x - outer.x) as usize * 4;
    let output_stride = output.width() as usize * 4;
    let output = output.as_mut();
    for y in 0..core.height as usize {
        let from = (core.y - outer.y) as usize * stride + y * stride + left;
        let to = (core.y as usize + y) * output_stride + core.x as usize * 4;
        output[to..to + row].copy_from_slice(&window[from..from + row]);
    }
    Ok(())
}

/// `document` seen through `window`: a document of the window's size whose layers are moved
/// so the window's corner is the origin. Upright full-resolution layers keep only the pixels
/// the window samples; every other layer must fit a `texture`-sided texture whole.
pub(super) fn window_document(document: &Document, window: Rect, texture: u32) -> Result<Document> {
    let mut tile = document.clone();
    tile.width = window.width;
    tile.height = window.height;
    tile.selection = None;
    let [dx, dy] = [window.x as f32, window.y as f32];
    for layer in &mut tile.layers {
        if let Some(pixels) = layer.pixels.clone()
            && !crop(document, layer, &pixels, window)
        {
            let [width, height] =
                render::source_size(document, layer, [document.width, document.height]);
            ensure!(
                width.max(height) <= texture,
                "Layer exceeds GPU texture limits"
            );
        }
        layer.transform.x -= dx;
        layer.transform.y -= dy;
        if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            placement.x -= dx;
            placement.y -= dy;
        }
    }
    Ok(tile)
}

/// Keeps only the pixels of an upright, full-resolution `layer` that `window` samples, and
/// moves its bounds to match; a layer outside the window becomes one transparent pixel.
/// Returns whether `layer` could be cropped (it is unchanged when the window uses it whole).
fn crop(document: &Document, layer: &mut Layer, pixels: &Arc<RgbaImage>, window: Rect) -> bool {
    let t = layer.transform;
    let (width, height) = pixels.dimensions();
    // A layer drawn at half size or less is filtered down before upload; see `source_size`.
    // Within 1.9× the crop keeps that choice whatever the rounding of its new bounds.
    let full_resolution = |extent: f32, pixels: u32| {
        extent.is_finite() && extent > 0.0 && (pixels as f32) < extent * 1.9
    };
    // Filters leave an unwarped quad on the layers they draw into.
    let unwarped = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(x, y)| Point::new(x, y));
    if t.rotation != 0.0
        || t.warp.is_some_and(|quad| quad != unwarped)
        || !t.x.is_finite()
        || !t.y.is_finite()
        || !full_resolution(t.width, width)
        || !full_resolution(t.height, height)
        || render::source_size(document, layer, [document.width, document.height])
            != [width, height]
    {
        return false;
    }
    // The source pixels the window's pixel centres sample, and one more on each side for
    // bilinear weights, as `[first, end)`; then where those pixels lie on the canvas.
    let span = |start: f32, extent: f32, flip: bool, pixels: u32, from: u32, length: u32| {
        let [start, extent, n] = [f64::from(start), f64::from(extent), f64::from(pixels)];
        let source = |canvas: f64| {
            let s = (canvas - start) / extent * n;
            if flip { n - s } else { s }
        };
        let [a, b] = [
            source(f64::from(from) + 0.5),
            source(f64::from(from) + f64::from(length) - 0.5),
        ];
        let first = (a.min(b) - 1.0).floor().clamp(0.0, n) as u32;
        let end = (a.max(b) + 1.0).ceil().clamp(0.0, n) as u32;
        let offset = if flip { pixels - end.max(first) } else { first };
        let position = start + extent * f64::from(offset) / n;
        let size = extent * f64::from(end.saturating_sub(first)) / n;
        (first, end, position as f32, size as f32)
    };
    let (left, right, x, crop_width) = span(t.x, t.width, t.flip_x, width, window.x, window.width);
    let (top, bottom, y, crop_height) =
        span(t.y, t.height, t.flip_y, height, window.y, window.height);
    if [left, top, right, bottom] == [0, 0, width, height] {
        return true;
    }
    if layer
        .mask
        .as_ref()
        .is_some_and(|mask| mask.placement.is_none())
    {
        // The mask keeps the layer's original bounds.
        layer.mask.as_mut().unwrap().placement = Some(t);
    }
    if right <= left || bottom <= top {
        layer.pixels = Some(Arc::new(RgbaImage::new(1, 1)));
        return true;
    }
    layer.pixels = Some(Arc::new(
        image::imageops::crop_imm(pixels.as_ref(), left, top, right - left, bottom - top)
            .to_image(),
    ));
    layer.transform.x = x;
    layer.transform.y = y;
    layer.transform.width = crop_width;
    layer.transform.height = crop_height;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Mask, Transform};
    use image::{GrayImage, Luma, Rgba};

    #[test]
    fn grid_covers_the_canvas_once_with_windows_inside_it() {
        for (size, core, margin) in [
            ([300, 200], 64, 0),
            ([300, 200], 64, 10),
            ([300, 200], 100, 150),
            ([64, 64], 64, 7),
            ([1, 1], 64, 3),
            ([65_535, 3], 4096, 40),
        ] {
            let tiles = grid(size, core, margin);
            let mut covered = vec![0_u8; size[0] as usize * size[1] as usize];
            for tile in &tiles {
                let (c, w) = (tile.core, tile.window);
                assert!(c.width > 0 && c.height > 0 && c.width <= core && c.height <= core);
                assert!(w.x + w.width <= size[0] && w.y + w.height <= size[1]);
                // The margin is there wherever the canvas goes on.
                assert_eq!(c.x - w.x, c.x.min(margin));
                assert_eq!(c.y - w.y, c.y.min(margin));
                assert_eq!(
                    w.x + w.width - (c.x + c.width),
                    (size[0] - c.x - c.width).min(margin)
                );
                assert_eq!(
                    w.y + w.height - (c.y + c.height),
                    (size[1] - c.y - c.height).min(margin)
                );
                if size[0] < 1000 {
                    for y in c.y..c.y + c.height {
                        for x in c.x..c.x + c.width {
                            covered[(y * size[0] + x) as usize] += 1;
                        }
                    }
                }
            }
            if size[0] < 1000 {
                assert!(covered.iter().all(|&n| n == 1), "{size:?} {core} {margin}");
            }
            // Tiles of one window size are together.
            let sizes: Vec<_> = tiles
                .iter()
                .map(|t| (t.window.width, t.window.height))
                .collect();
            let mut groups = sizes.clone();
            groups.dedup();
            let mut distinct = groups.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(groups.len(), distinct.len());
        }
    }

    #[test]
    fn cores_fit_the_device_or_give_up() {
        let tiling = Tiling {
            texture: 16_384,
            buffer: 256 << 20,
            tile: TILE,
        };
        // A 256 MB readback holds an 8,192-pixel square window.
        assert_eq!(tiling.window(), 8192);
        assert_eq!(tiling.core(0), Some(TILE));
        assert_eq!(tiling.core(2000), Some(TILE));
        assert_eq!(tiling.core(3000), Some(8192 - 6000));
        assert_eq!(tiling.core(4070), None);
        assert_eq!(tiling.core(u32::MAX), None);
        let small = Tiling {
            texture: 100,
            buffer: u64::MAX,
            tile: 64,
        };
        assert_eq!(small.core(18), Some(64));
        assert_eq!(small.core(19), None);
        assert!(small.fits([100, 100]) && !small.fits([101, 1]));
        // Rows are padded to 256 bytes in the readback.
        let narrow = Tiling {
            texture: 1000,
            buffer: 256 * 10,
            tile: 64,
        };
        assert!(narrow.fits([1, 10]) && !narrow.fits([1, 11]) && !narrow.fits([65, 10]));
    }

    #[test]
    fn margins_add_up_and_canvas_wide_filters_refuse_tiles() {
        let mut document = Document::new(64, 64).unwrap();
        assert_eq!(margin(&document), Some(0));
        let mut blur = Layer::blank("Blur", 64, 64);
        blur.filter = Some(Filter::GaussianBlur { radius: 4.0 });
        let mut motion = Layer::blank("Motion", 64, 64);
        motion.filter = Some(Filter::MotionBlur {
            distance: 15.0,
            angle: 30.0,
        });
        document.layers.extend([blur, motion]);
        // A 4-pixel blur's kernel has 25 taps; 15 pixels of motion reach 8 either way. Each
        // has a pixel or two to spare.
        assert_eq!(margin(&document), Some(13 + 10));
        document.layers[1].visible = false;
        assert_eq!(margin(&document), Some(10));
        for filter in [
            Filter::Noise {
                amount: 10.0,
                monochrome: true,
            },
            Filter::VIGNETTE,
            Filter::LensCorrection {
                distortion: 10.0,
                vignette: 0.0,
            },
        ] {
            document.layers[2].filter = Some(filter);
            assert_eq!(margin(&document), None);
        }
        document.layers[2].filter = Some(Filter::MotionBlur {
            distance: f32::INFINITY,
            angle: 0.0,
        });
        assert_eq!(margin(&document), Some(u32::MAX));
    }

    #[test]
    fn stitching_copies_each_core_from_its_window() {
        let size = [150, 90];
        let canvas = RgbaImage::from_fn(size[0], size[1], |x, y| {
            Rgba([x as u8, y as u8, (x * y) as u8, 255])
        });
        let mut output = RgbaImage::new(size[0], size[1]);
        for tile in grid(size, 64, 9) {
            let w = tile.window;
            let stride = (w.width as usize * 4).div_ceil(256) * 256;
            let mut window = vec![0; stride * w.height as usize];
            for y in 0..w.height {
                for x in 0..w.width {
                    let at = y as usize * stride + x as usize * 4;
                    window[at..at + 4].copy_from_slice(&canvas.get_pixel(w.x + x, w.y + y).0);
                }
            }
            stitch(&mut output, &tile, &window, stride).unwrap();
            assert!(stitch(&mut output, &tile, &window[1..], stride).is_err());
        }
        assert_eq!(output, canvas);
    }

    /// A scene with a flipped, scaled, masked and clipped layer larger than the test texture
    /// limit, a rotated layer in Multiply, a layer outside some windows and an adjustment.
    fn scene() -> Document {
        let mut document = Document::new(300, 200).unwrap();
        let mut base = Layer::image(
            "Base",
            RgbaImage::from_fn(400, 220, |x, y| {
                Rgba([
                    (x * 3) as u8,
                    (y * 5) as u8,
                    (x + y) as u8,
                    200 + (x % 50) as u8,
                ])
            }),
        );
        base.transform = Transform {
            x: -37.3,
            y: -5.6,
            width: 380.0,
            height: 230.5,
            flip_x: true,
            ..Transform::new(400, 220)
        };
        base.mask = Some(Mask {
            pixels: Arc::new(GrayImage::from_fn(40, 22, |x, y| Luma([(x * 6 + y) as u8]))),
            ..Mask::white()
        });
        let mut rotated = Layer::image(
            "Rotated",
            RgbaImage::from_fn(60, 40, |x, y| {
                Rgba([250, (x * 4) as u8, (y * 6) as u8, 230])
            }),
        );
        rotated.transform.x = 130.0;
        rotated.transform.y = 70.0;
        rotated.transform.rotation = 21.0;
        rotated.blend = crate::blend::BlendMode::Multiply;
        let mut clipped = Layer::image(
            "Clipped",
            RgbaImage::from_fn(300, 200, |x, _| Rgba([20, 200, (x / 2) as u8, 255])),
        );
        clipped.clip_to = Some(base.id);
        clipped.opacity = 0.6;
        clipped.mask = Some(Mask {
            pixels: Arc::new(GrayImage::from_fn(300, 200, |x, y| {
                Luma([if (x / 30 + y / 30) % 2 == 0 { 255 } else { 90 }])
            })),
            placement: Some(Transform {
                x: 7.0,
                ..Transform::new(300, 200)
            }),
            ..Mask::white()
        });
        let mut outside = Layer::image(
            "Outside",
            RgbaImage::from_pixel(30, 30, Rgba([0, 0, 255, 255])),
        );
        outside.transform.x = 260.0;
        outside.transform.y = 160.0;
        let mut adjustment = Layer::blank("Invert", 300, 200);
        adjustment.adjustment = Some(crate::document::Adjustment::Invert);
        adjustment.opacity = 0.3;
        document.active = Some(base.id);
        document.layers = vec![base, rotated, clipped, outside, adjustment];
        document.validate().unwrap();
        document
    }

    /// The window documents render (on the CPU) to the canvas the whole document renders to.
    #[test]
    fn window_documents_render_their_part_of_the_canvas() {
        let document = scene();
        let expected = render::render(&document);
        for (core, margin) in [(64, 0), (100, 13)] {
            let mut output = RgbaImage::new(300, 200);
            for tile in grid([300, 200], core, margin) {
                let window = window_document(&document, tile.window, 128).unwrap();
                assert!(
                    window
                        .layers
                        .iter()
                        .filter_map(|l| l.pixels.as_ref())
                        .all(|p| p.width() <= 150 && p.height() <= 150)
                );
                let image = render::render(&window);
                stitch(
                    &mut output,
                    &tile,
                    image.as_raw(),
                    image.width() as usize * 4,
                )
                .unwrap();
            }
            for (index, (a, b)) in output.pixels().zip(expected.pixels()).enumerate() {
                assert!(
                    a.0.iter().zip(b.0).all(|(a, b)| a.abs_diff(b) <= 1),
                    "{core} {margin} pixel {index}: tiled {a:?}, whole {b:?}"
                );
            }
        }
        // Rotated layers are uploaded whole, so they must fit a texture.
        let window = Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        assert!(window_document(&document, window, 59).is_err());
        assert!(window_document(&document, window, 60).is_ok());
    }

    fn processor() -> Arc<super::super::Processor> {
        let instance = wgpu::Instance::new(&Default::default());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        super::super::Processor::new(device, queue)
    }

    /// Limits that make `scene_with_effects` tile: windows of 64-pixel cores and their margins.
    const SMALL: Tiling = Tiling {
        texture: 128,
        buffer: u64::MAX,
        tile: 64,
    };

    /// `scene` with what reaches across tile edges: Gaussian and Motion Blur filter layers, a
    /// Motion Blur attached to a layer, layer effects, and the canvas-fixed noise of Grain,
    /// Film Grain and Dissolve.
    fn scene_with_effects() -> Document {
        use crate::{
            document::Adjustment,
            layer_effects::{EffectKind, LayerEffects},
        };
        let mut document = scene();
        let mut shape = Layer::image(
            "Shape",
            RgbaImage::from_fn(90, 70, |x, y| {
                let inside = (x as i32 - 45).pow(2) + (y as i32 - 35).pow(2) < 900;
                Rgba([30, 90, 220, if inside { 255 } else { 0 }])
            }),
        );
        shape.transform.x = 40.0;
        shape.transform.y = 30.0;
        let mut effects = LayerEffects::default();
        for kind in [
            EffectKind::DropShadow,
            EffectKind::Stroke,
            EffectKind::OuterGlow,
        ] {
            effects.add(kind, [230, 60, 40]);
        }
        shape.effects = Some(effects);
        let mut attached = Layer::blank("Attached Motion Blur", 300, 200);
        attached.parent = Some(shape.id);
        attached.filter = Some(Filter::MotionBlur {
            distance: 21.0,
            angle: 70.0,
        });
        let mut dissolve = Layer::image(
            "Dissolve",
            RgbaImage::from_pixel(120, 90, Rgba([240, 220, 30, 255])),
        );
        dissolve.transform.x = 150.0;
        dissolve.transform.y = 90.0;
        dissolve.opacity = 0.5;
        dissolve.blend = crate::blend::BlendMode::Dissolve;
        let mut blur = Layer::blank("Blur", 300, 200);
        blur.filter = Some(Filter::GaussianBlur { radius: 4.0 });
        let mut motion = Layer::blank("Motion Blur", 300, 200);
        motion.filter = Some(Filter::MotionBlur {
            distance: 15.0,
            angle: 30.0,
        });
        motion.opacity = 0.8;
        let mut grain = Layer::blank("Grain", 300, 200);
        grain.adjustment = Some(Adjustment::Grain {
            amount: 20.0,
            monochrome: false,
            seed: 77,
        });
        let mut film = Layer::blank("Film Grain", 300, 200);
        film.adjustment = Some(Adjustment::FilmGrain {
            amount: 40.0,
            size: 2.5,
            roughness: 50.0,
            seed: 5,
        });
        document
            .layers
            .extend([shape, attached, dissolve, blur, motion, grain, film]);
        document.validate().unwrap();
        document
    }

    /// Visible pixels further apart than `tolerance` in any channel.
    fn differences(a: &RgbaImage, b: &RgbaImage, tolerance: u8) -> usize {
        a.pixels()
            .zip(b.pixels())
            .filter(|(a, b)| {
                (0..4).any(|c| (c == 3 || a[3] > 8 || b[3] > 8) && a[c].abs_diff(b[c]) > tolerance)
            })
            .count()
    }

    fn premultiplied(image: &RgbaImage) -> RgbaImage {
        let mut image = image.clone();
        for pixel in image.pixels_mut() {
            for c in 0..3 {
                pixel[c] = (u16::from(pixel[c]) * u16::from(pixel[3]) / 255) as u8;
            }
        }
        image
    }

    /// The pixels of the 300 × 200 scene that may switch on Dissolve's threshold: one in 200,
    /// as `gpu::processing_tests::processing_blend_modes_match_cpu` allows.
    const DISSOLVE_SWITCHES: usize = 300 * 200 / 200;

    #[test]
    #[ignore = "requires a Vulkan or OpenGL compute adapter; run explicitly for native verification"]
    fn tiles_match_one_pass_and_the_cpu_without_seams() {
        let gpu = processor();
        let document = scene_with_effects();
        let (width, height) = (document.width, document.height);
        let large = Tiling::for_limits(&gpu.device.limits());
        let margin = margin(&render::prepare_attachments(&document)).unwrap();
        assert_eq!(margin, 13 + 10);
        assert!(!SMALL.fits([width, height]) && SMALL.core(margin) == Some(64));
        let (whole, tiled) = super::super::scope(Some(gpu.clone()), || {
            (
                gpu.compose_within(&document, width, height, large).unwrap(),
                gpu.compose_within(&document, width, height, SMALL).unwrap(),
            )
        });
        // Bounds moved into each window round differently in the last bits, so bilinear
        // weights may move an eight-bit level; nothing more, and no seams at tile edges.
        assert_eq!(differences(&tiled, &whole, 1), 0);
        let expected = render::render(&document);
        // Against the CPU, faint pixels' colours compare premultiplied, as in `gpu::tests`.
        // Dissolve switches between two results, so a pixel on its threshold may switch
        // differently in the GPU's 16-bit float canvas (Metal does on a few hundred);
        // `processing_blend_modes_match_cpu` allows the same share.
        let cpu = differences(&premultiplied(&tiled), &premultiplied(&expected), 4);
        assert!(cpu <= DISSOLVE_SWITCHES, "{cpu} pixels differ from the CPU");
        // Folders with mask layers composite through the group buffers in each tile too.
        let mut grouped = document.clone();
        let mut folder = Layer::blank("Folder", width, height);
        folder.group = true;
        let mut mask = Layer::mask("Folder mask", width, height);
        mask.parent = Some(folder.id);
        mask.mask.as_mut().unwrap().pixels = Arc::new(GrayImage::from_fn(30, 20, |x, y| {
            Luma([if (x + y) % 3 == 0 { 40 } else { 255 }])
        }));
        for layer in &mut grouped.layers[1..4] {
            layer.parent = Some(folder.id);
        }
        grouped.layers.insert(4, mask);
        grouped.layers.insert(1, folder);
        grouped.validate().unwrap();
        let (whole, tiled) = super::super::scope(Some(gpu.clone()), || {
            (
                gpu.compose_within(&grouped, width, height, large).unwrap(),
                gpu.compose_within(&grouped, width, height, SMALL).unwrap(),
            )
        });
        assert_eq!(differences(&tiled, &whole, 1), 0);
        let expected = premultiplied(&render::render(&grouped));
        let cpu = differences(&premultiplied(&tiled), &expected, 4);
        assert!(
            cpu <= DISSOLVE_SWITCHES,
            "{cpu} pixels differ from the CPU in a folder"
        );
        // The windows' cropped layers do not stay on the GPU.
        let compositor = gpu.compositor.lock().unwrap();
        let sources = &compositor.as_ref().unwrap().sources;
        assert!(sources.values().all(|s| s.pixels.strong_count() > 0));
    }

    #[test]
    #[ignore = "requires a Vulkan or OpenGL compute adapter; run explicitly for native verification"]
    fn devices_that_hold_no_tile_leave_the_document_to_the_cpu() {
        let gpu = processor();
        let mut document = scene_with_effects();
        let (width, height) = (document.width, document.height);
        let compose = |document: &Document, tiling: Tiling| {
            super::super::scope(Some(gpu.clone()), || {
                gpu.compose_within(document, width, height, tiling)
            })
        };
        // 64-pixel cores and 23-pixel margins need 110-pixel windows.
        let tiny = Tiling {
            texture: 109,
            ..SMALL
        };
        let error = compose(&document, tiny).unwrap_err();
        assert!(error.to_string().contains("tile"), "{error}");
        // A filter layer that reads the whole canvas can't be tiled either.
        let index = document.layers.len() - 3;
        document.layers[index].filter = Some(Filter::VIGNETTE);
        assert!(compose(&document, SMALL).is_err());
        // Nor can a rotated layer larger than a texture.
        let mut document = scene();
        document.layers[1].pixels = Some(Arc::new(RgbaImage::from_pixel(
            150,
            40,
            Rgba([1, 2, 3, 255]),
        )));
        document.layers[1].transform.width = 150.0;
        assert!(compose(&document, SMALL).is_err());
        // Scaled compositions don't tile.
        assert!(
            super::super::scope(Some(gpu.clone()), || {
                gpu.compose_within(&scene(), 150, 100, SMALL)
            })
            .is_err()
        );
        // Each of these renders on the CPU instead, through `render::render`.
        let error = compose(&document, SMALL).unwrap_err();
        assert!(error.to_string().contains("texture"), "{error}");
    }

    #[test]
    #[ignore = "native GPU timing; run with XUAN_BENCHMARK=1 and --nocapture"]
    fn benchmark_tiled_composition() {
        use std::time::Instant;
        // `scripts/check.sh --gpu` runs every ignored GPU test; this one only when asked.
        if std::env::var_os("XUAN_BENCHMARK").is_none() {
            eprintln!("set XUAN_BENCHMARK=1 to time tiled composition");
            return;
        }
        let gpu = processor();
        let (width, height) = (9000, 6000);
        let mut document = Document::new(width, height).unwrap();
        document.layers = (0..3)
            .map(|i| {
                let mut layer = Layer::image(
                    "Photo",
                    RgbaImage::from_fn(width, height, |x, y| {
                        Rgba([
                            (x + i * 50) as u8,
                            (y * 3) as u8,
                            (x ^ y) as u8,
                            255 - i as u8 * 60,
                        ])
                    }),
                );
                layer.blend = crate::blend::BlendMode::ALL[i as usize * 3];
                layer
            })
            .collect();
        let mut blur = Layer::blank("Blur", width, height);
        blur.filter = Some(Filter::GaussianBlur { radius: 6.0 });
        blur.opacity = 0.5;
        document.layers.push(blur);
        document.active = Some(document.layers[0].id);
        document.validate().unwrap();
        // Lower the texture limit below the canvas so the GPU tiles it.
        let tiling = Tiling {
            texture: 8192,
            ..Tiling::for_limits(&gpu.device.limits())
        };
        let start = Instant::now();
        let tiled = super::super::scope(Some(gpu.clone()), || {
            gpu.compose_within(&document, width, height, tiling)
                .unwrap()
        });
        eprintln!("GPU tiles: {:.2} s", start.elapsed().as_secs_f64());
        let start = Instant::now();
        let cpu = render::render(&document);
        eprintln!("CPU: {:.2} s", start.elapsed().as_secs_f64());
        assert_eq!(
            differences(&premultiplied(&tiled), &premultiplied(&cpu), 4),
            0
        );
    }
}

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, bail, ensure};
use image::{GrayImage, Luma, RgbaImage};

mod heal;
pub use heal::{HealMode, coverage_bounds, spot_heal};

use crate::{
    blend::BlendMode,
    document::{Document, Layer, Mask, Point, Transform},
    paint::Brush,
    render, selection,
};

fn random(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// Fill selected pixels by matching nearby texture patches. Source candidates always
/// come from the original unselected image, so blemishes never propagate into the fill.
pub fn inpaint(image: &RgbaImage, mask: &GrayImage, cancel: &AtomicBool) -> Result<RgbaImage> {
    ensure!(
        image.dimensions() == mask.dimensions(),
        "Selection and image dimensions differ"
    );
    let (width, height) = image.dimensions();
    let index = |x: u32, y: u32| (y * width + x) as usize;
    let mut unknown: Vec<bool> = mask.pixels().map(|p| p[0] > 0).collect();
    // Reservoir sampling bounds the candidate pool independently of image size.
    let mut sources = Vec::with_capacity(65_536);
    let mut state = 0x7d41_397b;
    let mut seen = 0;
    for (x, y, pixel) in image.enumerate_pixels() {
        if !unknown[index(x, y)] && pixel[3] > 128 {
            seen += 1;
            if sources.len() < 65_536 {
                sources.push((x, y));
            } else {
                let slot = random(&mut state) as usize % seen;
                if slot < sources.len() {
                    sources[slot] = (x, y);
                }
            }
        }
    }
    ensure!(
        !sources.is_empty(),
        "Leave some opaque, unselected pixels to sample for Content-Aware Fill"
    );
    let mut queue = VecDeque::new();
    let mut queued = vec![false; unknown.len()];
    for y in 0..height {
        for x in 0..width {
            if !unknown[index(x, y)] {
                continue;
            }
            if neighbors(x, y, width, height).any(|(nx, ny)| !unknown[index(nx, ny)]) {
                queue.push_back((x, y));
                queued[index(x, y)] = true;
            }
        }
    }
    ensure!(
        !queue.is_empty(),
        "Select a region with unselected pixels around it"
    );
    let mut output = image.clone();
    let mut matches: HashMap<usize, (u32, u32)> = HashMap::new();
    while let Some((x, y)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let mut candidates = Vec::with_capacity(28);
        for (nx, ny) in neighbors(x, y, width, height) {
            if let Some(&(sx, sy)) = matches.get(&index(nx, ny)) {
                let sx = sx as i32 + x as i32 - nx as i32;
                let sy = sy as i32 + y as i32 - ny as i32;
                if sx >= 0 && sy >= 0 && sx < width as i32 && sy < height as i32 {
                    candidates.push((sx as u32, sy as u32));
                }
            } else if mask.get_pixel(nx, ny)[0] == 0 {
                candidates.push((nx, ny));
            }
        }
        for _ in 0..20 {
            candidates.push(sources[random(&mut state) as usize % sources.len()]);
        }
        let score = |sx: u32, sy: u32| -> f32 {
            if mask.get_pixel(sx, sy)[0] != 0 || image.get_pixel(sx, sy)[3] < 128 {
                return f32::MAX;
            }
            let mut error = 0.0;
            let mut count = 0;
            for oy in -2..=2 {
                for ox in -2..=2 {
                    let tx = x as i32 + ox;
                    let ty = y as i32 + oy;
                    let px = sx as i32 + ox;
                    let py = sy as i32 + oy;
                    if tx < 0
                        || ty < 0
                        || px < 0
                        || py < 0
                        || tx >= width as i32
                        || ty >= height as i32
                        || px >= width as i32
                        || py >= height as i32
                    {
                        continue;
                    }
                    if unknown[index(tx as u32, ty as u32)]
                        || mask.get_pixel(px as u32, py as u32)[0] != 0
                    {
                        continue;
                    }
                    let target = output.get_pixel(tx as u32, ty as u32);
                    let source = image.get_pixel(px as u32, py as u32);
                    if target[3] < 128 || source[3] < 128 {
                        continue;
                    }
                    for c in 0..3 {
                        error += (target[c] as f32 - source[c] as f32).powi(2);
                    }
                    count += 1;
                }
            }
            if count == 0 {
                1e8
            } else {
                error / count as f32
                    + ((sx as f32 - x as f32).abs() + (sy as f32 - y as f32).abs()) * 0.005
            }
        };
        let best = candidates
            .into_iter()
            .map(|candidate| (score(candidate.0, candidate.1), candidate))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, candidate)| candidate)
            .unwrap_or(sources[0]);
        output.put_pixel(x, y, *image.get_pixel(best.0, best.1));
        unknown[index(x, y)] = false;
        matches.insert(index(x, y), best);
        for (nx, ny) in neighbors(x, y, width, height) {
            if unknown[index(nx, ny)] && !queued[index(nx, ny)] {
                queued[index(nx, ny)] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let amount = mask.get_pixel(x, y)[0] as f32 / 255.0;
        let original = image.get_pixel(x, y);
        for i in 0..4 {
            pixel[i] =
                (original[i] as f32 * (1.0 - amount) + pixel[i] as f32 * amount).round() as u8;
        }
    }
    Ok(output)
}

fn neighbors(x: u32, y: u32, width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
    [
        (x.wrapping_sub(1), y),
        (x + 1, y),
        (x, y.wrapping_sub(1)),
        (x, y + 1),
    ]
    .into_iter()
    .filter(move |(x, y)| *x < width && *y < height)
}

fn raster_layer(document: &Document) -> Result<RgbaImage> {
    let layer = document
        .active()
        .ok_or_else(|| anyhow::anyhow!("Select a pixel layer"))?;
    ensure!(
        layer.raw.is_none(),
        "Rasterize the RAW layer before applying a pixel retouch operation"
    );
    ensure!(
        !layer.locked && !layer.group && layer.adjustment.is_none(),
        "Select an unlocked pixel layer"
    );
    let mut isolated = document.clone();
    isolated.layers = vec![Layer {
        parent: None,
        mask: None,
        clip_to: None,
        opacity: 1.0,
        blend: BlendMode::Normal,
        visible: true,
        effects: None,
        ..layer.clone()
    }];
    Ok(render::render(&isolated))
}

fn replace_raster(document: &mut Document, pixels: RgbaImage) {
    let transform = Transform::new(document.width, document.height);
    if let Some(layer) = document.active_mut() {
        if let Some(mask) = &mut layer.mask {
            mask.placement = Some(mask.placement.unwrap_or(layer.transform));
            mask.linked = false;
        }
        layer.shape = None;
        layer.text = None;
        layer.pixels = Some(Arc::new(pixels));
        layer.transform = transform;
    }
}

pub fn content_aware_fill(document: &mut Document, cancel: &AtomicBool) -> Result<()> {
    let selection = document
        .selection
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Select the area to fill first"))?
        .clone();
    let source = raster_layer(document)?;
    let result = inpaint(&source, &selection, cancel)?;
    replace_raster(document, result);
    Ok(())
}

pub fn heal_path(
    document: &mut Document,
    points: &[Point],
    brush: &Brush,
    mode: HealMode,
    cancel: &AtomicBool,
) -> Result<()> {
    heal_path_varying(
        document,
        points,
        &vec![brush.clone(); points.len()],
        mode,
        cancel,
    )
}

/// Spot Healing along a stroke (see `heal::spot_heal`). Works in the active layer's own
/// pixel space on the stroke's bounding box plus a search margin, so the layer keeps its
/// size, resolution and transform, and pixels outside the brush are never written.
pub fn heal_path_varying(
    document: &mut Document,
    points: &[Point],
    brushes: &[Brush],
    mode: HealMode,
    cancel: &AtomicBool,
) -> Result<()> {
    let selection = document.selection.clone();
    let layer = document
        .active_mut()
        .ok_or_else(|| anyhow::anyhow!("Select a pixel layer"))?;
    heal_layer(layer, selection.as_deref(), points, brushes, mode, cancel)?;
    Ok(())
}

/// Heals `layer` in place and returns the region of layer pixels the kernel worked on
/// (`[x0, y0, x1, y1]`), or `None` when the brush covered nothing.
fn heal_layer(
    layer: &mut Layer,
    selection: Option<&GrayImage>,
    points: &[Point],
    brushes: &[Brush],
    mode: HealMode,
    cancel: &AtomicBool,
) -> Result<Option<[u32; 4]>> {
    ensure!(
        points.len() == brushes.len(),
        "Each healing sample needs a brush"
    );
    if points.is_empty() {
        return Ok(None);
    }
    crate::paint::ensure_pixels(layer)?;
    let transform = layer.transform;
    let image = layer.pixels.as_ref().unwrap();
    let (width, height) = image.dimensions();
    let mut path: Vec<_> = points.iter().copied().zip(brushes).collect();
    if path.len() == 1 {
        path.push(path[0]);
    }
    // Each segment's bounds in layer pixels, and their union.
    let segment_bounds: Vec<Option<[u32; 4]>> = path
        .windows(2)
        .map(|segment| {
            let [(a, first), (b, brush)] = [segment[0], segment[1]];
            let radius = (first.diameter.max(brush.diameter) * 0.5).max(0.5) + 1.0;
            let min = Point::new(a.x.min(b.x) - radius, a.y.min(b.y) - radius);
            let max = Point::new(a.x.max(b.x) + radius, a.y.max(b.y) + radius);
            let local = [min, Point::new(max.x, min.y), max, Point::new(min.x, max.y)]
                .map(|p| transform.inverse(p));
            let extent = |values: [f32; 4], size: u32| {
                let lo = values.iter().copied().fold(f32::MAX, f32::min) * size as f32;
                let hi = values.iter().copied().fold(f32::MIN, f32::max) * size as f32;
                (
                    lo.floor().clamp(0.0, size as f32) as u32,
                    hi.ceil().clamp(0.0, size as f32) as u32,
                )
            };
            let (left, right) = extent(local.map(|p| p.x), width);
            let (top, bottom) = extent(local.map(|p| p.y), height);
            (right > left && bottom > top).then_some([left, top, right, bottom])
        })
        .collect();
    let Some(union) = segment_bounds.iter().flatten().copied().reduce(|a, b| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    }) else {
        return Ok(None);
    };
    // Brush coverage (shape, hardness, opacity and selection) in layer pixels.
    let (uw, uh) = (
        (union[2] - union[0]) as usize,
        (union[3] - union[1]) as usize,
    );
    let mut coverage = vec![0u8; uw * uh];
    for (segment, bounds) in path.windows(2).zip(&segment_bounds) {
        let Some([left, top, right, bottom]) = *bounds else {
            continue;
        };
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let [(a, first), (b, brush)] = [segment[0], segment[1]];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let length = (dx * dx + dy * dy).max(0.001);
        for y in top..bottom {
            for x in left..right {
                let p = transform.point(Point::new(
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                ));
                let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0.0, 1.0);
                let radius = (first.diameter + (brush.diameter - first.diameter) * t) * 0.5;
                let opacity = first.opacity + (brush.opacity - first.opacity) * t;
                let tilt =
                    [0, 1].map(|axis| first.tilt[axis] + (brush.tilt[axis] - first.tilt[axis]) * t);
                let distance = crate::paint::brush_distance(
                    Point::new(p.x - a.x - t * dx, p.y - a.y - t * dy),
                    radius,
                    tilt,
                );
                if distance > 1.0 {
                    continue;
                }
                let amount = ((1.0 - distance) / (1.0 - brush.hardness).max(0.001)).min(1.0)
                    * opacity
                    * selection::coverage(selection, p);
                let value = (amount * 255.0).round().clamp(0.0, 255.0) as u8;
                let cell = &mut coverage[(y - union[1]) as usize * uw + (x - union[0]) as usize];
                *cell = (*cell).max(value);
            }
        }
    }
    let Some(painted) = heal::coverage_bounds(&coverage, uw, uh) else {
        return Ok(None);
    };
    let painted = [
        painted[0] as u32 + union[0],
        painted[1] as u32 + union[1],
        painted[2] as u32 + union[0],
        painted[3] as u32 + union[1],
    ];
    // Room for the kernel's patch search, which looks up to about three spot-widths away.
    let reach = ((painted[2] - painted[0]).max(painted[3] - painted[1]) as f32 + 32.0) * 3.2;
    let region = [
        (painted[0] as f32 - reach).floor().max(0.0) as u32,
        (painted[1] as f32 - reach).floor().max(0.0) as u32,
        (painted[2] as f32 + reach).ceil().min(width as f32) as u32,
        (painted[3] as f32 + reach).ceil().min(height as f32) as u32,
    ];
    let (rw, rh) = (
        (region[2] - region[0]) as usize,
        (region[3] - region[1]) as usize,
    );
    // Premultiplied copy of the region, with the coverage placed alongside it.
    let mut rgba = vec![0u8; rw * rh * 4];
    for y in 0..rh {
        for x in 0..rw {
            let pixel = image
                .get_pixel(region[0] + x as u32, region[1] + y as u32)
                .0;
            let out = &mut rgba[(y * rw + x) * 4..(y * rw + x) * 4 + 4];
            for c in 0..3 {
                out[c] = premultiply(pixel[c], pixel[3]);
            }
            out[3] = pixel[3];
        }
    }
    let mut region_coverage = vec![0u8; rw * rh];
    for y in painted[1]..painted[3] {
        let from = (y - union[1]) as usize * uw;
        let to = (y - region[1]) as usize * rw;
        for x in painted[0]..painted[2] {
            region_coverage[to + (x - region[0]) as usize] =
                coverage[from + (x - union[0]) as usize];
        }
    }
    let original = rgba.clone();
    let seed = points.iter().fold(0x9e37_79b9u32, |seed, p| {
        (seed ^ p.x.to_bits())
            .rotate_left(13)
            .wrapping_mul(0x85eb_ca6b)
            ^ p.y.to_bits()
    });
    heal::spot_heal(&mut rgba, &region_coverage, rw, rh, 1.0, mode, seed, cancel)?;
    // Write back only the pixels the kernel changed, so everything else stays
    // byte-identical (un-premultiplying would round unchanged pixels).
    let pixels = Arc::make_mut(layer.pixels.as_mut().unwrap());
    for y in painted[1]..painted[3] {
        for x in painted[0]..painted[2] {
            let i = ((y - region[1]) as usize * rw + (x - region[0]) as usize) * 4;
            let healed = &rgba[i..i + 4];
            if healed == &original[i..i + 4] {
                continue;
            }
            let alpha = healed[3];
            pixels.get_pixel_mut(x, y).0 = if alpha == 0 {
                [0; 4]
            } else {
                [
                    unpremultiply(healed[0], alpha),
                    unpremultiply(healed[1], alpha),
                    unpremultiply(healed[2], alpha),
                    alpha,
                ]
            };
        }
    }
    Ok(Some(region))
}

fn premultiply(value: u8, alpha: u8) -> u8 {
    ((u32::from(value) * u32::from(alpha) + 127) / 255) as u8
}

fn unpremultiply(value: u8, alpha: u8) -> u8 {
    ((u32::from(value) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
}

/// The active layer, when Remove Background can mask it.
fn background_layer(document: &mut Document) -> Result<&mut Layer> {
    let layer = document
        .active_mut()
        .ok_or_else(|| anyhow::anyhow!("Select an image layer"))?;
    ensure!(
        !layer.locked && !layer.group,
        "Select an unlocked image layer"
    );
    ensure!(layer.pixels.is_some(), "The selected layer is empty");
    Ok(layer)
}

/// Filter → Remove Background: the classical subject segmentation of
/// [`crate::segment`] on the active layer's own pixels, as a layer mask that hides the
/// background (kept together with any mask the layer already has).
pub fn remove_background(
    document: &mut Document,
    progress: &(dyn Fn(f32) + Sync),
    cancel: &AtomicBool,
) -> Result<()> {
    let layer = background_layer(document)?;
    let image = layer.pixels.clone().unwrap();
    let Some(result) =
        crate::segment::segment(&image, &crate::segment::Seeds::subject(), progress, cancel)
    else {
        bail!("Cancelled");
    };
    apply_layer_matte(layer, result.mask);
    Ok(())
}

/// Masks `layer` with `matte` (white keeps, in the layer's own pixels), together with
/// the mask it already has: what either hides stays hidden.
pub fn apply_layer_matte(layer: &mut Layer, matte: GrayImage) {
    let mut matte = matte;
    let (width, height) = matte.dimensions();
    if let Some(result) = crate::gpu::bake_mask(layer, &matte) {
        matte = result;
    } else {
        for (x, y, pixel) in matte.enumerate_pixels_mut() {
            let point = layer.transform.point(Point::new(
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            ));
            pixel[0] = (pixel[0] as f32 * render::own_mask(layer, point)).round() as u8;
        }
    }
    layer.mask = Some(Mask {
        pixels: Arc::new(matte),
        ..Mask::white()
    });
}

/// Filter → Remove Flat Background: a portable edge-color matte. The seed color follows
/// each edge region, retaining foreground edges where the color distance crosses the
/// threshold. Exact at full resolution, so it keeps hairline detail on a flat
/// background (logos, line art, product shots on white) that the downscaled graph cut
/// of [`remove_background`] can lose.
pub fn remove_flat_background(
    document: &mut Document,
    tolerance: u8,
    cancel: &AtomicBool,
) -> Result<()> {
    let layer = background_layer(document)?;
    let image = layer.pixels.as_ref().unwrap();
    let (width, height) = image.dimensions();
    let mut matte = GrayImage::from_pixel(width, height, Luma([255]));
    let mut visited = vec![false; width as usize * height as usize];
    let mut queue = VecDeque::new();
    for x in 0..width {
        queue.push_back((x, 0, image.get_pixel(x, 0).0));
        queue.push_back((x, height - 1, image.get_pixel(x, height - 1).0));
    }
    for y in 0..height {
        queue.push_back((0, y, image.get_pixel(0, y).0));
        queue.push_back((width - 1, y, image.get_pixel(width - 1, y).0));
    }
    while let Some((x, y, seed)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let index = (y * width + x) as usize;
        if visited[index] {
            continue;
        }
        let pixel = image.get_pixel(x, y);
        if pixel[3] != 0
            && pixel.0[..3]
                .iter()
                .zip(seed)
                .any(|(a, b)| a.abs_diff(b) > tolerance)
        {
            continue;
        }
        visited[index] = true;
        matte.put_pixel(x, y, Luma([0]));
        for (nx, ny) in neighbors(x, y, width, height) {
            if !visited[(ny * width + nx) as usize] {
                queue.push_back((nx, ny, seed));
            }
        }
    }
    let matte = crate::gpu::blur_gray(&matte, 0.65);
    apply_layer_matte(layer, matte);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    const MODES: [HealMode; 3] = [
        HealMode::ContentAware,
        HealMode::CreateTexture,
        HealMode::ProximityMatch,
    ];

    fn hard_brush(diameter: f32) -> Brush {
        Brush {
            diameter,
            hardness: 1.0,
            opacity: 1.0,
            ..Brush::default()
        }
    }

    fn document_with(layer: Layer, width: u32, height: u32) -> Document {
        let mut document = Document::new(width, height).unwrap();
        document.layers = vec![layer];
        document.active = Some(document.layers[0].id);
        document
    }

    fn heal(document: &mut Document, points: &[Point], diameter: f32, mode: HealMode) {
        heal_path(
            document,
            points,
            &hard_brush(diameter),
            mode,
            &AtomicBool::new(false),
        )
        .unwrap();
    }

    fn pixels(document: &Document) -> &RgbaImage {
        document.layers[0].pixels.as_ref().unwrap()
    }

    /// Small deterministic noise in `-amplitude..=amplitude`.
    fn noise(x: u32, y: u32, salt: u32, amplitude: i32) -> i32 {
        let hash = super::heal::hash(x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ salt);
        (hash % (2 * amplitude as u32 + 1)) as i32 - amplitude
    }

    fn luminance(pixel: Rgba<u8>) -> f32 {
        0.3 * f32::from(pixel[0]) + 0.59 * f32::from(pixel[1]) + 0.11 * f32::from(pixel[2])
    }

    /// Layer pixels whose centres are farther than `radius` (document pixels) from every
    /// point of the stroke must be byte-identical.
    fn assert_untouched_outside(before: &RgbaImage, layer: &Layer, stroke: &[Point], radius: f32) {
        let after = layer.pixels.as_ref().unwrap();
        assert_eq!(before.dimensions(), after.dimensions());
        let (width, height) = after.dimensions();
        for (x, y, pixel) in after.enumerate_pixels() {
            let point = layer.transform.point(Point::new(
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            ));
            if stroke.iter().all(|p| p.distance(point) > radius) {
                assert_eq!(
                    pixel,
                    before.get_pixel(x, y),
                    "layer pixel ({x}, {y}) changed outside the brush"
                );
            }
        }
    }

    #[test]
    fn healing_a_dot_on_a_gradient_continues_the_gradient() {
        // Regression for #27: the old per-pixel fill copied gradient blocks from far away
        // (maximum error 54 under the brush).
        let clean = |x: u32, y: u32| Rgba([(2 * x) as u8, (2 * y) as u8, 64, 255]);
        for mode in MODES {
            let image = RgbaImage::from_fn(128, 96, |x, y| {
                let (dx, dy) = (x as f32 + 0.5 - 64.0, y as f32 + 0.5 - 48.0);
                if dx.hypot(dy) <= 5.0 {
                    Rgba([10, 10, 10, 255])
                } else {
                    clean(x, y)
                }
            });
            let mut document = document_with(Layer::image("Gradient", image.clone()), 128, 96);
            let center = Point::new(64.0, 48.0);
            heal(&mut document, &[center], 24.0, mode);
            let healed = pixels(&document);
            let mut worst = 0;
            for (x, y, pixel) in healed.enumerate_pixels() {
                let point = Point::new(x as f32 + 0.5, y as f32 + 0.5);
                if point.distance(center) <= 12.0 {
                    let expected = clean(x, y);
                    for c in 0..4 {
                        worst = worst.max(pixel[c].abs_diff(expected[c]));
                    }
                }
            }
            assert!(worst <= 2, "{mode:?}: error {worst} under the brush");
            assert_untouched_outside(&image, &document.layers[0], &[center], 13.0);
        }
    }

    /// Port of upstream `SpotHealingTests.healsTheBlemishUnderTheBrushAndNothingElse`.
    #[test]
    fn heals_the_blemish_under_the_brush_and_nothing_else() {
        let image = RgbaImage::from_fn(120, 80, |x, y| {
            let gray = if x % 4 < 2 { 100 } else { 112 };
            if (55..65).contains(&x) && (35..45).contains(&y) {
                Rgba([230, 20, 20, 255])
            } else {
                Rgba([gray, gray, gray, 255])
            }
        });
        for mode in MODES {
            let mut document = document_with(Layer::image("Surface", image.clone()), 120, 80);
            let stroke = [Point::new(60.0, 40.0), Point::new(60.5, 40.0)];
            heal(&mut document, &stroke, 24.0, mode);
            let after = pixels(&document);
            for (x, y) in [(60, 40), (56, 36), (64, 44)] {
                let healed = after.get_pixel(x, y);
                assert!(
                    i32::from(healed[0]) - i32::from(healed[1]) < 30
                        && (80..=130).contains(&healed[1])
                        && healed[3] == 255,
                    "{mode:?}: ({x}, {y}) is {healed:?}, not healed into the gray surface"
                );
            }
            for (x, y) in [(10, 10), (90, 40), (30, 40), (60, 10), (60, 70)] {
                assert_eq!(after.get_pixel(x, y), image.get_pixel(x, y), "{mode:?}");
            }
            assert_untouched_outside(&image, &document.layers[0], &stroke, 13.0);
        }
    }

    #[test]
    fn healing_keeps_texture_without_speckles() {
        // Skin-like grain with a saturated green region far from the spot.
        // Pores and mottling: noise interpolated over a 3 px lattice, plus fine grain.
        let mottle = |x: u32, y: u32| {
            let (fx, fy) = (x as f32 / 3.0, y as f32 / 3.0);
            let (ix, iy) = (fx as u32, fy as u32);
            let (tx, ty) = (fx.fract(), fy.fract());
            let at = |x: u32, y: u32| noise(x, y, 1, 6) as f32;
            let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
            let bottom = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
            (top * (1.0 - ty) + bottom * ty).round() as i32
        };
        let skin = |x: u32, y: u32| {
            let grain = mottle(x, y) + noise(x, y, 4, 1);
            Rgba([
                (200 + grain + noise(x, y, 2, 1)) as u8,
                (150 + grain + noise(x, y, 3, 1)) as u8,
                (130 + grain) as u8,
                255,
            ])
        };
        let image = RgbaImage::from_fn(200, 160, |x, y| {
            if x < 30 {
                Rgba([20, 230, 40, 255])
            } else {
                skin(x, y)
            }
        });
        let center = Point::new(130.0, 80.0);
        let radius = 8.0;
        let mut blemished = image.clone();
        for (x, y, pixel) in blemished.enumerate_pixels_mut() {
            if Point::new(x as f32 + 0.5, y as f32 + 0.5).distance(center) <= 4.0 {
                *pixel = Rgba([90, 40, 40, 255]);
            }
        }
        let ring: Vec<(u32, u32)> = image
            .enumerate_pixels()
            .filter(|(x, y, _)| {
                let d = Point::new(*x as f32 + 0.5, *y as f32 + 0.5).distance(center);
                d > radius + 1.0 && d <= radius + 16.0
            })
            .map(|(x, y, _)| (x, y))
            .collect();
        let roughness = |image: &RgbaImage, inside: &dyn Fn(u32, u32) -> bool| {
            let (mut sum, mut n) = (0.0, 0);
            for (x, y, pixel) in image.enumerate_pixels() {
                if x + 1 < image.width() && inside(x, y) && inside(x + 1, y) {
                    sum += (luminance(*pixel) - luminance(*image.get_pixel(x + 1, y))).abs();
                    n += 1;
                }
            }
            sum / n as f32
        };
        let ring_luma: Vec<f32> = ring
            .iter()
            .map(|&(x, y)| luminance(*image.get_pixel(x, y)))
            .collect();
        let ring_mean = ring_luma.iter().sum::<f32>() / ring_luma.len() as f32;
        let ring_max = ring_luma
            .iter()
            .map(|l| (l - ring_mean).abs())
            .fold(0.0, f32::max);
        let near = |x: u32, y: u32| {
            let d = Point::new(x as f32 + 0.5, y as f32 + 0.5).distance(center);
            d > radius + 1.0 && d <= radius + 16.0
        };
        let ring_roughness = roughness(&image, &near);
        for mode in [HealMode::ContentAware, HealMode::ProximityMatch] {
            let mut document = document_with(Layer::image("Skin", blemished.clone()), 200, 160);
            heal(&mut document, &[center], radius * 2.0, mode);
            let healed = pixels(&document);
            let hole = |x: u32, y: u32| {
                Point::new(x as f32 + 0.5, y as f32 + 0.5).distance(center) <= radius
            };
            // No speckles: healed pixels stay within the spread of the texture around the
            // spot. The membrane follows the per-pixel noise at the spot's edge, so a few
            // edge pixels may go slightly past the ring's extreme; distant colours (the old
            // fill's failure) would be tens of levels away.
            let (mut within, mut count) = (0, 0);
            for (x, y, pixel) in healed.enumerate_pixels() {
                if hole(x, y) {
                    let deviation = (luminance(*pixel) - ring_mean).abs();
                    assert!(
                        deviation <= ring_max * 1.5,
                        "{mode:?}: ({x}, {y}) {pixel:?} is {deviation} from the ring mean (max {ring_max})"
                    );
                    within += usize::from(deviation <= ring_max);
                    count += 1;
                }
            }
            assert!(
                within * 100 >= count * 95,
                "{mode:?}: {within} of {count} within the ring's spread"
            );
            let healed_roughness = roughness(healed, &hole);
            assert!(
                (healed_roughness - ring_roughness).abs() <= ring_roughness * 0.25,
                "{mode:?}: roughness {healed_roughness} vs {ring_roughness} around the spot"
            );
            assert_untouched_outside(&blemished, &document.layers[0], &[center], radius + 1.0);
        }
        // Create Texture synthesises matching grain rather than copying it.
        let mut document = document_with(Layer::image("Skin", blemished.clone()), 200, 160);
        heal(
            &mut document,
            &[center],
            radius * 2.0,
            HealMode::CreateTexture,
        );
        let healed = pixels(&document);
        let hole =
            |x: u32, y: u32| Point::new(x as f32 + 0.5, y as f32 + 0.5).distance(center) <= radius;
        let healed_roughness = roughness(healed, &hole);
        assert!(
            healed_roughness > ring_roughness * 0.5 && healed_roughness < ring_roughness * 1.5,
            "CreateTexture: roughness {healed_roughness} vs {ring_roughness}"
        );
        for (x, y, pixel) in healed.enumerate_pixels() {
            if hole(x, y) {
                assert!((luminance(*pixel) - ring_mean).abs() < 3.0 * ring_max);
                assert!(pixel[1] < 200, "green leaked into the heal at ({x}, {y})");
            }
        }
    }

    /// A long thin stroke takes its texture from alongside itself, not from a box-width
    /// away. Everything outside the strip around the stroke is a loud stripe pattern,
    /// which the box-based overlap test used to pick as the only candidate.
    #[test]
    fn long_thin_stroke_heals_from_alongside() {
        let (a, b) = (Point::new(200.0, 330.0), Point::new(440.0, 150.0));
        let distance_to_stroke = |x: u32, y: u32| {
            let p = Point::new(x as f32 + 0.5, y as f32 + 0.5);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            p.distance(Point::new(a.x + t * dx, a.y + t * dy))
        };
        let skin = |x: u32, y: u32| {
            let grain = noise(x / 2, y / 2, 11, 6) + noise(x, y, 12, 2);
            Rgba([
                (200 + grain) as u8,
                (150 + grain) as u8,
                (130 + grain) as u8,
                255,
            ])
        };
        let near = |x: u32, y: u32| (100..540).contains(&x) && (60..420).contains(&y);
        let clean = RgbaImage::from_fn(640, 480, |x, y| {
            if near(x, y) {
                skin(x, y)
            } else if (x / 6).is_multiple_of(2) {
                Rgba([40, 40, 40, 255])
            } else {
                Rgba([220, 220, 220, 255])
            }
        });
        let mut blemished = clean.clone();
        for (x, y, pixel) in blemished.enumerate_pixels_mut() {
            if distance_to_stroke(x, y) <= 2.0 {
                *pixel = Rgba([90, 40, 40, 255]);
            }
        }
        for mode in [HealMode::ContentAware, HealMode::ProximityMatch] {
            let mut document = document_with(Layer::image("Skin", blemished.clone()), 640, 480);
            heal(&mut document, &[a, b], 14.0, mode);
            let healed = pixels(&document);
            let (mut worst, mut sum, mut count) = (0u32, 0u64, 0u64);
            for (x, y, pixel) in healed.enumerate_pixels() {
                if distance_to_stroke(x, y) <= 7.0 {
                    let error = (luminance(*pixel) - luminance(*clean.get_pixel(x, y)))
                        .abs()
                        .round() as u32;
                    worst = worst.max(error);
                    sum += u64::from(error);
                    count += 1;
                }
            }
            let mean = sum as f64 / count as f64;
            assert!(
                worst < 40 && mean < 8.0,
                "{mode:?}: healed stroke is off by up to {worst} (mean {mean:.1}) from the skin"
            );
            for (x, y, pixel) in healed.enumerate_pixels() {
                if distance_to_stroke(x, y) > 8.0 {
                    assert_eq!(
                        pixel,
                        blemished.get_pixel(x, y),
                        "{mode:?}: ({x}, {y}) changed outside the stroke"
                    );
                }
            }
        }
    }

    fn checker_with_blemish(width: u32, height: u32, cell: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let (dx, dy) = (
                x as f32 - width as f32 / 2.0,
                y as f32 - height as f32 / 2.0,
            );
            if dx.hypot(dy) < cell as f32 {
                Rgba([250, 0, 0, 255])
            } else if (x / cell + y / cell).is_multiple_of(2) {
                Rgba([90, 110, 130, 255])
            } else {
                Rgba([110, 120, 100, 255])
            }
        })
    }

    #[test]
    fn healing_keeps_layer_geometry() {
        // Larger than the canvas and hanging off its left edge.
        let image = checker_with_blemish(200, 100, 4);
        let mut layer = Layer::image("Wide", image.clone());
        layer.transform.x = -50.0;
        let transform = layer.transform;
        let mut document = document_with(layer, 100, 100);
        let center = Point::new(50.0, 50.0);
        heal(&mut document, &[center], 20.0, HealMode::ContentAware);
        let layer = &document.layers[0];
        assert_eq!(layer.transform, transform);
        assert_eq!(pixels(&document).dimensions(), (200, 100));
        assert_ne!(pixels(&document).get_pixel(100, 50)[0], 250);
        assert_untouched_outside(&image, layer, &[center], 11.0);

        // Stored at four times the document resolution.
        let image = checker_with_blemish(400, 400, 16);
        let mut layer = Layer::image("Scaled", image.clone());
        layer.transform = Transform::new(100, 100);
        let transform = layer.transform;
        let mut document = document_with(layer, 100, 100);
        heal(&mut document, &[center], 20.0, HealMode::ContentAware);
        let layer = &document.layers[0];
        assert_eq!(layer.transform, transform);
        assert_eq!(pixels(&document).dimensions(), (400, 400));
        assert_ne!(pixels(&document).get_pixel(200, 200)[0], 250);
        assert_untouched_outside(&image, layer, &[center], 11.0);

        // Rotated and offset.
        let image = checker_with_blemish(120, 80, 4);
        let mut layer = Layer::image("Rotated", image.clone());
        layer.transform.x = -10.0;
        layer.transform.y = 10.0;
        layer.transform.rotation = 30.0;
        let transform = layer.transform;
        let center = transform.center();
        let mut document = document_with(layer, 100, 100);
        heal(&mut document, &[center], 20.0, HealMode::ContentAware);
        let layer = &document.layers[0];
        assert_eq!(layer.transform, transform);
        assert_eq!(pixels(&document).dimensions(), (120, 80));
        assert_ne!(pixels(&document).get_pixel(60, 40)[0], 250);
        assert_untouched_outside(&image, layer, &[center], 11.0);
    }

    #[test]
    fn healing_works_on_semi_transparent_layers() {
        let image = RgbaImage::from_fn(120, 80, |x, y| {
            let gray = if x % 4 < 2 { 100 } else { 112 };
            if (55..65).contains(&x) && (35..45).contains(&y) {
                Rgba([230, 20, 20, 120])
            } else {
                Rgba([gray, gray, gray, 120])
            }
        });
        for mode in MODES {
            let mut document = document_with(Layer::image("Glass", image.clone()), 120, 80);
            let center = Point::new(60.0, 40.0);
            heal(&mut document, &[center], 24.0, mode);
            let after = pixels(&document);
            for (x, y, pixel) in after.enumerate_pixels() {
                assert!(pixel[3].abs_diff(120) <= 1, "{mode:?}: alpha {pixel:?}");
                if Point::new(x as f32 + 0.5, y as f32 + 0.5).distance(center) <= 12.0 {
                    assert!(
                        pixel[0].abs_diff(pixel[1]) < 20 && (85..=125).contains(&pixel[1]),
                        "{mode:?}: ({x}, {y}) is {pixel:?}"
                    );
                }
            }
            assert_untouched_outside(&image, &document.layers[0], &[center], 13.0);
        }
    }

    #[test]
    fn healing_cost_follows_the_brush_not_the_layer() {
        let image = RgbaImage::from_fn(3000, 3000, |x, y| {
            Rgba([
                (x % 251) as u8,
                (y % 241) as u8,
                ((x + y) % 7) as u8 * 30,
                255,
            ])
        });
        let mut layer = Layer::image("Large", image);
        let started = std::time::Instant::now();
        let region = heal_layer(
            &mut layer,
            None,
            &[Point::new(1500.0, 1500.0)],
            &[hard_brush(20.0)],
            HealMode::ContentAware,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        let elapsed = started.elapsed();
        // A 20 px dab searches (20 + 32) × 3.2 px around itself, never the whole layer.
        let [x0, y0, x1, y1] = region;
        assert!(x1 - x0 < 400 && y1 - y0 < 400, "healed region {region:?}");
        assert!(x0 > 1000 && y0 > 1000 && x1 < 2000 && y1 < 2000);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "a small dab took {elapsed:?}"
        );
    }

    #[test]
    fn healing_respects_selection_lock_and_cancel() {
        let image = checker_with_blemish(60, 60, 4);
        let mut document = document_with(Layer::image("Layer", image.clone()), 60, 60);
        // Only the left half of the spot is selected.
        document.selection = Some(Arc::new(selection::rectangle(
            60,
            60,
            Point::new(0.0, 0.0),
            Point::new(30.0, 60.0),
            false,
        )));
        heal(
            &mut document,
            &[Point::new(30.0, 30.0)],
            12.0,
            HealMode::ContentAware,
        );
        let after = pixels(&document);
        for (x, y, pixel) in after.enumerate_pixels() {
            if x >= 30 {
                assert_eq!(pixel, image.get_pixel(x, y));
            }
        }
        assert_ne!(after.get_pixel(28, 30), image.get_pixel(28, 30));

        let mut locked = document_with(Layer::image("Layer", image.clone()), 60, 60);
        locked.layers[0].locked = true;
        assert!(
            heal_path(
                &mut locked,
                &[Point::new(30.0, 30.0)],
                &hard_brush(12.0),
                HealMode::ContentAware,
                &AtomicBool::new(false),
            )
            .is_err()
        );
        let mut cancelled = document_with(Layer::image("Layer", image), 60, 60);
        assert!(
            heal_path(
                &mut cancelled,
                &[Point::new(30.0, 30.0)],
                &hard_brush(12.0),
                HealMode::ContentAware,
                &AtomicBool::new(true),
            )
            .is_err()
        );
    }
    #[test]
    fn fill_removes_a_spot_without_changing_unselected_pixels() {
        let image = RgbaImage::from_fn(16, 16, |x, y| {
            if (6..10).contains(&x) && (6..10).contains(&y) {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([80, 120, 160, 255])
            }
        });
        let mask =
            selection::rectangle(16, 16, Point::new(6.0, 6.0), Point::new(10.0, 10.0), false);
        let result = inpaint(&image, &mask, &AtomicBool::new(false)).unwrap();
        assert!(result.pixels().all(|p| p.0 == [80, 120, 160, 255]));
        assert!(
            inpaint(
                &image,
                &GrayImage::from_pixel(16, 16, Luma([255])),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[test]
    fn background_removal_creates_an_editable_mask() {
        let mut doc = Document::new(20, 20).unwrap();
        let image = RgbaImage::from_fn(20, 20, |x, y| {
            if (5..15).contains(&x) && (5..15).contains(&y) {
                Rgba([50, 80, 130, 255])
            } else {
                Rgba([255; 4])
            }
        });
        doc.layers[0].pixels = Some(Arc::new(image));
        let original = doc.clone();
        remove_flat_background(&mut doc, 20, &AtomicBool::new(false)).unwrap();
        let mask = &doc.layers[0].mask.as_ref().unwrap().pixels;
        assert_eq!(mask.get_pixel(0, 0)[0], 0);
        assert_eq!(mask.get_pixel(10, 10)[0], 255);
        assert_eq!(
            doc.layers[0].pixels.as_ref().unwrap().get_pixel(0, 0)[3],
            255
        );
        // The graph cut finds the same square, and keeps an existing mask's holes.
        let mut doc = original;
        let mut hole = GrayImage::from_pixel(20, 20, Luma([255]));
        hole.put_pixel(9, 9, Luma([0]));
        doc.layers[0].mask = Some(Mask {
            pixels: Arc::new(hole),
            ..Mask::white()
        });
        remove_background(&mut doc, &|_| {}, &AtomicBool::new(false)).unwrap();
        let mask = &doc.layers[0].mask.as_ref().unwrap().pixels;
        assert_eq!(mask.get_pixel(0, 0)[0], 0);
        assert_eq!(mask.get_pixel(10, 10)[0], 255);
        assert_eq!(mask.get_pixel(9, 9)[0], 0);
        assert!(remove_background(&mut doc, &|_| {}, &AtomicBool::new(true)).is_err());
    }
}

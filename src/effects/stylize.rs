//! Vignette, Bloom / Glow and Tonal Contrast, ported from upstream Compositor's Filter menu
//! (`Document/Filters.swift`, with the pixel loops of `Rendering/AdjustPixels.c`). Colors are
//! straight sRGB in 0–1; `stylize.wgsl` mirrors every function here for the GPU.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;

/// Rec. 709 luminance of straight sRGB, as upstream's `rec709`.
pub(super) fn rec709(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Upstream's `vignette_mask_at`: the vignette's strength at `point` of a `size` frame, 0 at
/// its middle rising to 1 past its edges. Roundness 100 is an ellipse fitted to the frame and
/// −100 a rounded rectangle; Midpoint is how far out the falloff starts and Feather its width.
pub fn vignette_mask(
    point: [f32; 2],
    size: [f32; 2],
    midpoint: f32,
    roundness: f32,
    feather: f32,
) -> f32 {
    let nx = point[0] / size[0] * 2.0 - 1.0;
    let ny = point[1] / size[1] * 2.0 - 1.0;
    let square = nx.abs().max(ny.abs());
    let circle = nx.hypot(ny) / std::f32::consts::SQRT_2;
    let shape = (1.0 - roundness / 100.0) * 0.5;
    let distance = circle + (square - circle) * shape;
    let start = midpoint / 100.0 * 0.85;
    let soft = (feather / 100.0).max(0.05);
    smoothstep((distance - start) / soft)
}

/// Upstream's `adjust_colored_vignette` for one straight pixel: the color moves toward `color`
/// by `amount`% of the mask, eased off bright pixels by Highlights. `fills_clear` (a filter
/// layer, or upstream's vignette on an empty layer) paints transparent pixels too; otherwise
/// only the pixels that are there change color and their coverage stays as it was.
pub fn vignette_pixel(
    pixel: [f32; 4],
    mask: f32,
    amount: f32,
    highlights: f32,
    color: [f32; 3],
    fills_clear: bool,
) -> [f32; 4] {
    if mask <= 0.0 || amount <= 0.0 || (pixel[3] <= 0.0 && !fills_clear) {
        return pixel;
    }
    let alpha = pixel[3];
    let rgb = [pixel[0], pixel[1], pixel[2]];
    let bright = if alpha > 0.0 {
        ((rec709(rgb) - 0.45) / 0.55).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let effect = (amount / 100.0).clamp(0.0, 1.0) * mask * (1.0 - highlights / 100.0 * bright);
    if !fills_clear {
        let mixed: [f32; 3] = std::array::from_fn(|i| rgb[i] + (color[i] - rgb[i]) * effect);
        return [mixed[0], mixed[1], mixed[2], alpha];
    }
    // The color painted over the pixel at `effect`: an opaque pixel moves toward it, a clear
    // one takes it on.
    let out = alpha + effect * (1.0 - alpha);
    if out <= 0.0 {
        return pixel;
    }
    let mixed: [f32; 3] =
        std::array::from_fn(|i| (color[i] * effect + rgb[i] * alpha * (1.0 - effect)) / out);
    [mixed[0], mixed[1], mixed[2], out]
}

/// Bloom / Glow on premultiplied pixels: upstream runs Core Image's `CIBloom` with
/// `inputIntensity` = Amount / 50, whose kernel is not published. This uses the usual reading of
/// it: the blurred copy lightens the image where it is brighter (a per-channel maximum, which
/// also spreads the glow into transparent areas), mixed in at `intensity` (up to 2, so strong
/// settings push past the plain lighten).
pub fn bloom_pixel(source: [f32; 4], blurred: [f32; 4], intensity: f32) -> [f32; 4] {
    let mut out: [f32; 4] = std::array::from_fn(|i| {
        (source[i] + (source[i].max(blurred[i]) - source[i]) * intensity).clamp(0.0, 1.0)
    });
    for c in 0..3 {
        out[c] = out[c].min(out[3]);
    }
    out
}

/// Upstream's `adjust_tonal_contrast` for one straight pixel: its luminance is pushed away from
/// that of `base` (the image blurred at the detail radius), more in the tones the base belongs
/// to by the Shadows, Midtones and Highlights strengths, and less toward black and white.
pub fn tonal_contrast_pixel(
    pixel: [f32; 4],
    base: [f32; 4],
    amount: f32,
    [shadows, midtones, highlights]: [f32; 3],
) -> [f32; 4] {
    if pixel[3] <= 0.0 || base[3] <= 0.0 {
        return pixel;
    }
    let rgb = [pixel[0], pixel[1], pixel[2]];
    let luminance = rec709(rgb);
    let base_luminance = rec709([base[0], base[1], base[2]]);
    let tonal = |low: f32, high: f32| smoothstep((base_luminance - low) / (high - low));
    let shadow = 1.0 - tonal(0.15, 0.5);
    let highlight = tonal(0.5, 0.85);
    let midtone = 1.0 - shadow - highlight;
    let weight = (shadows * shadow + midtones * midtone + highlights * highlight) / 100.0;
    let detail = luminance - base_luminance;
    let delta = 0.18
        * (detail * 6.0).tanh()
        * weight
        * (amount / 50.0)
        * (4.0 * luminance * (1.0 - luminance));
    [
        (rgb[0] + delta).clamp(0.0, 1.0),
        (rgb[1] + delta).clamp(0.0, 1.0),
        (rgb[2] + delta).clamp(0.0, 1.0),
        pixel[3],
    ]
}

fn unit(pixel: &Rgba<u8>) -> [f32; 4] {
    pixel.0.map(|v| v as f32 / 255.0)
}

fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Straight 8-bit pixels with `map` applied to each, by position.
fn mapped(image: &RgbaImage, map: impl Fn(u32, u32, [f32; 4]) -> [f32; 4] + Sync) -> RgbaImage {
    let width = image.width().max(1);
    let mut result = image.clone();
    result
        .as_mut()
        .par_chunks_exact_mut(4)
        .enumerate()
        .for_each(|(index, pixel)| {
            let (x, y) = (index as u32 % width, index as u32 / width);
            let old = [pixel[0], pixel[1], pixel[2], pixel[3]].map(|v| v as f32 / 255.0);
            let new = map(x, y, old);
            if new != old {
                pixel.copy_from_slice(&new.map(byte));
            }
        });
    result
}

/// Vignette over the whole of `image`, which is its frame.
#[allow(clippy::too_many_arguments)]
pub(super) fn vignette(
    image: &RgbaImage,
    amount: f32,
    color: [u8; 3],
    midpoint: f32,
    roundness: f32,
    feather: f32,
    highlights: f32,
    fills_clear: bool,
) -> RgbaImage {
    let size = [image.width() as f32, image.height() as f32];
    let color = color.map(|v| v as f32 / 255.0);
    mapped(image, |x, y, pixel| {
        let point = [x as f32 + 0.5, y as f32 + 0.5];
        let mask = vignette_mask(point, size, midpoint, roundness, feather);
        vignette_pixel(pixel, mask, amount, highlights, color, fills_clear)
    })
}

pub(super) fn bloom(image: &RgbaImage, blurred: &RgbaImage, amount: f32) -> RgbaImage {
    let premultiplied = |p: [f32; 4]| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]];
    mapped(image, |x, y, pixel| {
        let glow = premultiplied(unit(blurred.get_pixel(x, y)));
        let out = bloom_pixel(premultiplied(pixel), glow, amount / 50.0);
        if out[3] <= 0.0 {
            return [0.0; 4];
        }
        [out[0] / out[3], out[1] / out[3], out[2] / out[3], out[3]]
    })
}

pub(super) fn tonal_contrast(
    image: &RgbaImage,
    blurred: &RgbaImage,
    amount: f32,
    strengths: [f32; 3],
) -> RgbaImage {
    if amount <= 0.0 || strengths == [0.0; 3] {
        return image.clone();
    }
    mapped(image, |x, y, pixel| {
        tonal_contrast_pixel(pixel, unit(blurred.get_pixel(x, y)), amount, strengths)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-4, "{actual} != {expected}");
    }

    /// Upstream's mask: nothing up to Midpoint · 0.85 of the way out, smoothstepped over
    /// Feather, measured on a circle (Roundness 100) or a square (−100) fitted to the frame.
    #[test]
    fn vignette_mask_follows_midpoint_feather_and_roundness() {
        let size = [200.0, 100.0];
        let at =
            |x: f32, y: f32, roundness: f32| vignette_mask([x, y], size, 50.0, roundness, 60.0);
        let expected = |distance: f32| {
            let t = ((distance - 0.425) / 0.6).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        assert_eq!(at(100.0, 50.0, 100.0), 0.0);
        // Halfway out along an axis is still inside the start on a circle (0.5 / √2).
        assert_eq!(at(150.0, 50.0, 100.0), 0.0);
        // 0.9 of the way out: a circle measures 0.9 / √2, a square 0.9, and Roundness 0
        // halfway between.
        let circle = 0.9 / std::f32::consts::SQRT_2;
        close(at(190.0, 50.0, 100.0), expected(circle));
        close(at(190.0, 50.0, -100.0), expected(0.9));
        close(at(190.0, 50.0, 0.0), expected((circle + 0.9) / 2.0));
        // On the diagonal both shapes measure the same.
        close(at(150.0, 75.0, -100.0), at(150.0, 75.0, 100.0));
        close(at(150.0, 75.0, 100.0), expected(0.5));
        // Corners are nearly at full strength.
        close(at(0.0, 0.0, 100.0), expected(1.0));
        assert!(at(0.0, 0.0, 100.0) > 0.99);
        // Feather has a floor of 5%, so even a hard edge is smoothstepped, between 0.425 and
        // 0.475 of the way out on the circle.
        let hard = |x| vignette_mask([x, 50.0], size, 50.0, 100.0, 0.0);
        assert_eq!(hard(160.0), 0.0);
        assert!(hard(164.0) > 0.0 && hard(164.0) < 1.0);
        assert_eq!(hard(168.0), 1.0);
    }

    #[test]
    fn vignette_pixel_recolors_eases_highlights_and_fills_clear_pixels() {
        let black = [0.0; 3];
        // Upstream's defaults at a full mask: 35% toward black.
        let gray = vignette_pixel([0.4, 0.4, 0.4, 1.0], 1.0, 35.0, 25.0, black, false);
        close(gray[0], 0.4 * 0.65);
        assert_eq!(gray[3], 1.0);
        // White is eased off by Highlights: bright 1, so 35% · (1 − 0.25).
        let white = vignette_pixel([1.0, 1.0, 1.0, 0.5], 1.0, 35.0, 25.0, black, false);
        close(white[0], 1.0 - 0.35 * 0.75);
        assert_eq!(white[3], 0.5);
        // A clear pixel stays clear on a layer, and takes the color on in a filter layer.
        assert_eq!(
            vignette_pixel([0.0; 4], 1.0, 35.0, 25.0, [1.0, 0.0, 0.0], false),
            [0.0; 4]
        );
        let painted = vignette_pixel([0.0; 4], 0.5, 80.0, 25.0, [1.0, 0.0, 0.0], true);
        close(painted[3], 0.4);
        close(painted[0], 1.0);
        // Half-covered: alpha + effect·(1 − alpha), color weighted by what each contributes.
        let mixed = vignette_pixel([0.0, 0.0, 1.0, 0.5], 1.0, 50.0, 0.0, [1.0, 0.0, 0.0], true);
        close(mixed[3], 0.75);
        close(mixed[0], 0.5 / 0.75);
        close(mixed[2], 0.25 / 0.75);
        // No mask or no amount, no change.
        let pixel = [0.3, 0.6, 0.9, 1.0];
        assert_eq!(vignette_pixel(pixel, 0.0, 35.0, 25.0, black, true), pixel);
        assert_eq!(vignette_pixel(pixel, 1.0, 0.0, 25.0, black, true), pixel);
    }

    #[test]
    fn bloom_lightens_toward_the_glow_and_spreads_it() {
        // Intensity 0.8 (Amount 40): 80% of the way to the brighter of the two.
        let out = bloom_pixel([0.2, 0.5, 0.1, 1.0], [0.6, 0.4, 0.1, 1.0], 0.8);
        close(out[0], 0.2 + 0.4 * 0.8);
        close(out[1], 0.5);
        close(out[3], 1.0);
        // Into a clear area, the glow arrives with its own coverage.
        let out = bloom_pixel([0.0; 4], [0.25, 0.25, 0.0, 0.5], 1.0);
        assert_eq!(out, [0.25, 0.25, 0.0, 0.5]);
        // Intensity 2 pushes past the glow, but never past full or past alpha.
        let out = bloom_pixel([0.5, 0.5, 0.5, 1.0], [0.9, 0.9, 0.9, 0.9], 2.0);
        close(out[0], 1.0);
        let out = bloom_pixel([0.1, 0.0, 0.0, 0.9], [0.8, 0.0, 0.0, 0.85], 2.0);
        assert!(out[0] <= out[3], "{out:?}");
        // Nothing brighter around it, nothing changes.
        let pixel = [0.4, 0.3, 0.2, 1.0];
        assert_eq!(bloom_pixel(pixel, [0.1, 0.1, 0.1, 1.0], 2.0), pixel);
    }

    #[test]
    fn tonal_contrast_pushes_detail_by_tone() {
        let defaults = [40.0, 60.0, 30.0];
        // A midtone base (0.5): all midtones, so 0.6 of the weight.
        let pixel = [0.6, 0.6, 0.6, 1.0];
        let out = tonal_contrast_pixel(pixel, [0.5, 0.5, 0.5, 1.0], 50.0, defaults);
        let expected = 0.6 + 0.18 * 0.6f32.tanh() * 0.6 * 1.0 * (4.0 * 0.6 * 0.4);
        close(out[0], expected);
        close(out[2], expected);
        // Darker than its surroundings: pushed darker.
        let out = tonal_contrast_pixel([0.4, 0.4, 0.4, 1.0], [0.5; 4], 100.0, defaults);
        assert!(out[0] < 0.4);
        // A shadow base (0.1) uses only Shadows; a highlight base (0.9) only Highlights.
        let out = tonal_contrast_pixel([0.2, 0.2, 0.2, 1.0], [0.1, 0.1, 0.1, 1.0], 50.0, defaults);
        close(out[0], 0.2 + 0.18 * 0.6f32.tanh() * 0.4 * (4.0 * 0.2 * 0.8));
        let out = tonal_contrast_pixel([0.8, 0.8, 0.8, 1.0], [0.9, 0.9, 0.9, 1.0], 50.0, defaults);
        close(
            out[0],
            0.8 + 0.18 * (-0.6f32).tanh() * 0.3 * (4.0 * 0.8 * 0.2),
        );
        // Black, white, flat areas and clear pixels stay.
        for pixel in [
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
            [0.3, 0.2, 0.1, 0.0],
        ] {
            assert_eq!(
                tonal_contrast_pixel(pixel, [0.5; 4], 100.0, defaults),
                pixel
            );
        }
        let flat = [0.3, 0.5, 0.7, 1.0];
        assert_eq!(tonal_contrast_pixel(flat, flat, 100.0, defaults), flat);
    }
}

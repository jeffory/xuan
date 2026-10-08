//! Select → Expand…, Contract…, Color Range… and Mask's Black Areas: classical
//! operations on Xuan's 256-level selection masks.
//!
//! Expand and Contract use a round (disc) reach, as Compositor's and Photoshop's do: a
//! pixel is within `radius` of another when the distance between their centres is at
//! most `radius`. Hard selections (every value 0 or 255) take an exact Euclidean
//! distance transform, linear in the number of pixels whatever the radius; soft
//! selections take a greyscale dilation or erosion with the same disc, which keeps a
//! feathered edge feathered.
use std::sync::atomic::{AtomicBool, Ordering};

use image::{GrayImage, Luma, RgbaImage};
use rayon::prelude::*;

/// The largest Expand / Contract amount, in pixels, as in Compositor.
pub const MAX_AMOUNT: u32 = 500;

/// Grows the selection by `radius` pixels with rounded corners, clipped to the canvas.
/// Returns `None` when cancelled.
pub fn expand(mask: &GrayImage, radius: u32, cancel: &AtomicBool) -> Option<GrayImage> {
    if radius == 0 {
        return Some(mask.clone());
    }
    if is_hard(mask) {
        let (width, height) = mask.dimensions();
        let distance =
            squared_distance(width, height, cancel, |x, y| mask.get_pixel(x, y)[0] != 0)?;
        let reach = f64::from(radius).powi(2);
        let mut output = GrayImage::new(width, height);
        for (pixel, d) in output.as_mut().iter_mut().zip(distance) {
            *pixel = if d <= reach { 255 } else { 0 };
        }
        Some(output)
    } else {
        morphology(mask, radius, true, cancel)
    }
}

/// Shrinks the selection by `radius` pixels, including away from the canvas edges (the
/// canvas is surrounded by unselected pixels), as Compositor's Contract does.
/// Returns `None` when cancelled.
pub fn contract(mask: &GrayImage, radius: u32, cancel: &AtomicBool) -> Option<GrayImage> {
    if radius == 0 {
        return Some(mask.clone());
    }
    if is_hard(mask) {
        let (width, height) = mask.dimensions();
        // Distances to the nearest unselected pixel, on a canvas padded with an
        // unselected border one pixel wide.
        let distance = squared_distance(width + 2, height + 2, cancel, |x, y| {
            x == 0 || y == 0 || x > width || y > height || mask.get_pixel(x - 1, y - 1)[0] == 0
        })?;
        let reach = f64::from(radius).powi(2);
        let stride = width as usize + 2;
        Some(GrayImage::from_fn(width, height, |x, y| {
            let d = distance[(y as usize + 1) * stride + x as usize + 1];
            Luma([if d > reach { 255 } else { 0 }])
        }))
    } else {
        morphology(mask, radius, false, cancel)
    }
}

fn is_hard(mask: &GrayImage) -> bool {
    mask.as_raw().iter().all(|&v| v == 0 || v == 255)
}

/// The squared Euclidean distance from every pixel to the nearest pixel where `seed` is
/// true (Felzenszwalb and Huttenlocher's two-pass transform). Pixels with no seed at all
/// get infinity. Columns, then rows, are worked out in parallel.
fn squared_distance(
    width: u32,
    height: u32,
    cancel: &AtomicBool,
    seed: impl Fn(u32, u32) -> bool + Sync,
) -> Option<Vec<f64>> {
    let (w, h) = (width as usize, height as usize);
    let scratch = |n: usize| move || (vec![0.0; n], vec![0usize; n], vec![0.0; n + 1]);
    // Each column on its own, into a column-major copy.
    let mut columns = vec![0.0; w * h];
    columns
        .par_chunks_mut(h)
        .enumerate()
        .for_each_init(scratch(h), |(f, v, z), (x, out)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            for (y, value) in f.iter_mut().enumerate() {
                *value = if seed(x as u32, y as u32) {
                    0.0
                } else {
                    f64::INFINITY
                };
            }
            transform_1d(f, out, v, z);
        });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut grid = vec![0.0; w * h];
    grid.par_chunks_mut(w)
        .enumerate()
        .for_each_init(scratch(w), |(f, v, z), (y, out)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            for (x, value) in f.iter_mut().enumerate() {
                *value = columns[x * h + y];
            }
            transform_1d(f, out, v, z);
        });
    (!cancel.load(Ordering::Relaxed)).then_some(grid)
}

/// One dimension of the squared distance transform: `d[q] = min_p f[p] + (q - p)²`.
fn transform_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    let Some(first) = f.iter().position(|value| value.is_finite()) else {
        d.fill(f64::INFINITY);
        return;
    };
    let mut k = 0;
    v[0] = first;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in first + 1..n {
        if !f[q].is_finite() {
            continue;
        }
        let qf = q as f64;
        // z[0] is minus infinity, so k never drops below 0.
        let s = loop {
            let p = v[k] as f64;
            let s = ((f[q] + qf * qf) - (f[v[k]] + p * p)) / (2.0 * (qf - p));
            if s <= z[k] {
                k -= 1;
            } else {
                break s;
            }
        };
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        let qf = q as f64;
        while z[k + 1] < qf {
            k += 1;
        }
        let p = v[k] as f64;
        *out = (qf - p) * (qf - p) + f[v[k]];
    }
}

/// Greyscale dilation (`grow`) or erosion with a disc of `radius`. Outside the canvas
/// counts as unselected, so erosion shrinks from the canvas edges too. Rows are worked
/// out in parallel, each from the `2 × radius + 1` input rows around it.
fn morphology(mask: &GrayImage, radius: u32, grow: bool, cancel: &AtomicBool) -> Option<GrayImage> {
    let (width, height) = mask.dimensions();
    let w = width as usize;
    let r = radius.min(width.max(height)) as i64;
    // Half-width of the disc on each row offset.
    let spans: Vec<usize> = (-r..=r)
        .map(|dy| ((r * r - dy * dy) as f64).sqrt().floor() as usize)
        .collect();
    let mut output = GrayImage::new(width, height);
    output.as_mut().par_chunks_mut(w).enumerate().for_each_init(
        || (vec![0u8; w], std::collections::VecDeque::with_capacity(w)),
        |(window, deque), (y, accumulated)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            accumulated.fill(if grow { 0 } else { 255 });
            for (i, dy) in (-r..=r).enumerate() {
                let sy = y as i64 + dy;
                if sy < 0 || sy >= i64::from(height) {
                    if !grow {
                        // An unselected row outside the canvas lies within reach.
                        accumulated.fill(0);
                    }
                    continue;
                }
                let row = &mask.as_raw()[sy as usize * w..(sy as usize + 1) * w];
                sliding_extreme(row, spans[i], grow, window, deque);
                for (a, &b) in accumulated.iter_mut().zip(window.iter()) {
                    *a = if grow { (*a).max(b) } else { (*a).min(b) };
                }
            }
        },
    );
    (!cancel.load(Ordering::Relaxed)).then_some(output)
}

/// The maximum (or minimum) of `row` over `x - half ..= x + half` for every `x`, with
/// zeros beyond the row's ends.
fn sliding_extreme(
    row: &[u8],
    half: usize,
    max: bool,
    out: &mut [u8],
    deque: &mut std::collections::VecDeque<usize>,
) {
    let n = row.len();
    deque.clear();
    let better = |a: u8, b: u8| if max { a >= b } else { a <= b };
    let mut next = 0;
    for (x, slot) in out.iter_mut().enumerate().take(n) {
        let end = (x + half).min(n - 1);
        while next <= end {
            while deque.back().is_some_and(|&i| better(row[next], row[i])) {
                deque.pop_back();
            }
            deque.push_back(next);
            next += 1;
        }
        while deque.front().is_some_and(|&i| i + half < x) {
            deque.pop_front();
        }
        let mut value = row[*deque.front().unwrap()];
        // The window reaches past an end of the row: outside counts as 0.
        if !max && (x < half || x + half >= n) {
            value = 0;
        }
        *slot = value;
    }
}

/// The widest Edit → Stroke… line, in pixels, as in Photoshop.
pub const MAX_STROKE_WIDTH: u32 = 250;

/// Where Edit → Stroke… draws its line against the selection's edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrokeLocation {
    /// Wholly inside the selection, its outer edge on the selection's.
    Inside,
    /// Straddling the edge, half on each side; an odd width puts the extra pixel outside.
    Center,
    /// Wholly outside the selection, its inner edge on the selection's.
    #[default]
    Outside,
}

/// Edit → Stroke…'s line along the edge of `mask`, as coverage on the same canvas: the
/// selection grown by the outside part of `width`, less the selection shrunk by the inside
/// part. Grows and shrinks reach as far as Expand and Contract do (a disc, so corners
/// round off), and the canvas edges count as an edge, as in Contract. The side of the line
/// on the selection's edge is that edge as it is, so a stroke meets a fill of the same
/// selection without a seam. On a hard selection (every value 0 or 255, as marquees and
/// lassos make) the far side is antialiased from the exact distance to the selection; a
/// soft one keeps its soft edge through the greyscale disc. Only the selection's bounds
/// and the reach around them are worked on. `None` when cancelled.
pub fn stroke_coverage(
    mask: &GrayImage,
    width: u32,
    location: StrokeLocation,
    cancel: &AtomicBool,
) -> Option<GrayImage> {
    let width = width.min(MAX_STROKE_WIDTH);
    let (outside, inside) = match location {
        StrokeLocation::Inside => (0, width),
        StrokeLocation::Center => (width - width / 2, width / 2),
        StrokeLocation::Outside => (width, 0),
    };
    let (canvas_width, canvas_height) = mask.dimensions();
    let mut output = GrayImage::new(canvas_width, canvas_height);
    let Some((left, top, right, bottom)) = crate::selection::bounds(mask).filter(|_| width > 0)
    else {
        return Some(output);
    };
    // A band one pixel wider than the reach is unselected all round (or is the canvas
    // edge), so the crop grows and shrinks exactly as the whole canvas would.
    let margin = width + 1;
    let (x0, y0) = (left.saturating_sub(margin), top.saturating_sub(margin));
    let x1 = right.saturating_add(margin).min(canvas_width);
    let y1 = bottom.saturating_add(margin).min(canvas_height);
    let crop = image::imageops::crop_imm(mask, x0, y0, x1 - x0, y1 - y0).to_image();
    let hard = is_hard(&crop);
    let grown = reach(&crop, outside, true, hard, cancel)?;
    let shrunk = reach(&crop, inside, false, hard, cancel)?;
    let crop_width = crop.width() as usize;
    for (i, (g, s)) in grown.into_iter().zip(shrunk).enumerate() {
        let (x, y) = ((i % crop_width) as u32, (i / crop_width) as u32);
        let value = ((g - s).clamp(0.0, 1.0) * 255.0).round() as u8;
        output.put_pixel(x0 + x, y0 + y, Luma([value]));
    }
    Some(output)
}

/// `mask` grown (or shrunk) by `radius` as coverage from 0 to 1. A hard mask takes the
/// distance transform with a one-pixel ramp past the reach, so the new edge is
/// antialiased; a whole-pixel distance is fully in or out, as Expand and Contract have
/// it. A soft mask takes the greyscale disc.
fn reach(
    mask: &GrayImage,
    radius: u32,
    grow: bool,
    hard: bool,
    cancel: &AtomicBool,
) -> Option<Vec<f32>> {
    let unit = |value: u8| f32::from(value) / 255.0;
    if radius == 0 {
        return Some(mask.as_raw().iter().map(|&v| unit(v)).collect());
    }
    if !hard {
        let moved = morphology(mask, radius, grow, cancel)?;
        return Some(moved.as_raw().iter().map(|&v| unit(v)).collect());
    }
    let (width, height) = mask.dimensions();
    let r = f64::from(radius);
    if grow {
        let distance =
            squared_distance(width, height, cancel, |x, y| mask.get_pixel(x, y)[0] != 0)?;
        Some(
            distance
                .into_iter()
                .map(|d| (r + 1.0 - d.sqrt()).clamp(0.0, 1.0) as f32)
                .collect(),
        )
    } else {
        // Distances to the nearest unselected pixel, on a canvas padded with an
        // unselected border one pixel wide, as in `contract`.
        let distance = squared_distance(width + 2, height + 2, cancel, |x, y| {
            x == 0 || y == 0 || x > width || y > height || mask.get_pixel(x - 1, y - 1)[0] == 0
        })?;
        let stride = width as usize + 2;
        Some(
            (0..(width * height) as usize)
                .map(|i| {
                    let (x, y) = (i % width as usize, i / width as usize);
                    let d = distance[(y + 1) * stride + x + 1];
                    (d.sqrt() - r).clamp(0.0, 1.0) as f32
                })
                .collect(),
        )
    }
}

/// Select → Color Range: how a pixel is matched against the sampled colours.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorRange {
    /// Colours to select, straight RGB.
    pub include: Vec<[u8; 3]>,
    /// Colours to take away from the selection.
    pub exclude: Vec<[u8; 3]>,
    /// How far, per channel (0–200), a colour may be from a sampled one and still be
    /// selected.
    pub fuzziness: u32,
    /// Select everything except those colours.
    pub invert: bool,
}

impl ColorRange {
    /// The largest fuzziness, as in Compositor.
    pub const MAX_FUZZINESS: u32 = 200;
    pub const DEFAULT_FUZZINESS: u32 = 40;

    /// How much a colour matches one sampled colour, from 1 (within `fuzziness / 2` on
    /// every channel) falling linearly to 0 at `fuzziness`, as Photoshop's Color Range
    /// fades its edge. Fuzziness 0 is an exact match.
    fn closeness(&self, rgb: [u8; 3], colors: &[[u8; 3]]) -> f32 {
        let fuzz = self.fuzziness.min(Self::MAX_FUZZINESS) as f32;
        let full = (fuzz / 2.0).floor();
        let mut best = 0.0f32;
        for color in colors {
            let distance = rgb
                .iter()
                .zip(color)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0) as f32;
            let value = if distance <= full {
                1.0
            } else if distance > fuzz {
                0.0
            } else {
                (fuzz + 1.0 - distance) / (fuzz + 1.0 - full)
            };
            best = best.max(value);
            if best >= 1.0 {
                break;
            }
        }
        best
    }

    /// The selection over `image` (straight RGBA): fully transparent pixels never match.
    pub fn mask(&self, image: &RgbaImage) -> GrayImage {
        use rayon::prelude::*;
        let mut output = GrayImage::new(image.width(), image.height());
        let width = image.width().max(1) as usize;
        output
            .as_mut()
            .par_chunks_mut(width)
            .zip(image.as_raw().par_chunks(width * 4))
            .for_each(|(row, pixels)| {
                for (out, pixel) in row.iter_mut().zip(pixels.as_chunks::<4>().0) {
                    *out = self.value(*pixel);
                }
            });
        output
    }

    /// The selection level of one straight RGBA pixel.
    pub fn value(&self, [r, g, b, a]: [u8; 4]) -> u8 {
        let mut value = if a == 0 || self.include.is_empty() {
            0.0
        } else {
            let rgb = [r, g, b];
            self.closeness(rgb, &self.include) * (1.0 - self.closeness(rgb, &self.exclude))
        };
        if self.invert {
            value = 1.0 - value;
        }
        (value * 255.0).round() as u8
    }
}

/// The straight colour under `(x, y)`, averaged over the 3 × 3 pixels around it and
/// weighted by alpha, as Compositor's Color Range eyedropper samples. `None` over
/// transparent pixels or outside the image.
pub fn sample_color(image: &RgbaImage, x: f32, y: f32) -> Option<[u8; 3]> {
    if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
        return None;
    }
    let (cx, cy) = (x as u32, y as u32);
    if cx >= image.width() || cy >= image.height() {
        return None;
    }
    let mut sums = [0u64; 4];
    for sy in cy.saturating_sub(1)..=(cy + 1).min(image.height() - 1) {
        for sx in cx.saturating_sub(1)..=(cx + 1).min(image.width() - 1) {
            let [r, g, b, a] = image.get_pixel(sx, sy).0;
            let a = u64::from(a);
            sums[0] += u64::from(r) * a;
            sums[1] += u64::from(g) * a;
            sums[2] += u64::from(b) * a;
            sums[3] += a;
        }
    }
    (sums[3] > 0).then(|| std::array::from_fn(|c| ((sums[c] + sums[3] / 2) / sums[3]) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn from_rows(rows: &[&str]) -> GrayImage {
        GrayImage::from_fn(rows[0].len() as u32, rows.len() as u32, |x, y| {
            Luma([if rows[y as usize].as_bytes()[x as usize] == b'#' {
                255
            } else {
                0
            }])
        })
    }

    fn rows(mask: &GrayImage) -> Vec<String> {
        (0..mask.height())
            .map(|y| {
                (0..mask.width())
                    .map(|x| match mask.get_pixel(x, y)[0] {
                        0 => '.',
                        255 => '#',
                        _ => '+',
                    })
                    .collect()
            })
            .collect()
    }

    /// The disc reference: selected when a selected pixel lies within `radius`.
    fn reference(mask: &GrayImage, radius: u32, grow: bool) -> GrayImage {
        let (w, h) = mask.dimensions();
        let r = radius as i64;
        GrayImage::from_fn(w, h, |x, y| {
            let mut found = false;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    let (sx, sy) = (x as i64 + dx, y as i64 + dy);
                    let inside = sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64;
                    let selected = inside && mask.get_pixel(sx as u32, sy as u32)[0] != 0;
                    if grow == selected {
                        found = true;
                    }
                }
            }
            Luma([if found == grow { 255 } else { 0 }])
        })
    }

    #[test]
    fn expand_grows_a_pixel_into_a_disc() {
        let mask = from_rows(&[
            ".......", ".......", ".......", "...#...", ".......", ".......", ".......",
        ]);
        let grown = expand(&mask, 2, &never()).unwrap();
        assert_eq!(
            rows(&grown),
            [
                ".......", "...#...", "..###..", ".#####.", "..###..", "...#...", "......."
            ]
        );
        let grown = expand(&mask, 3, &never()).unwrap();
        assert_eq!(
            rows(&grown),
            [
                "...#...", ".#####.", ".#####.", "#######", ".#####.", ".#####.", "...#..."
            ]
        );
    }

    #[test]
    fn contract_shrinks_from_holes_and_canvas_edges() {
        let all = GrayImage::from_pixel(6, 5, Luma([255]));
        assert_eq!(
            rows(&contract(&all, 1, &never()).unwrap()),
            ["......", ".####.", ".####.", ".####.", "......"]
        );
        let mask = from_rows(&[
            "#######", "#######", "#######", "###.###", "#######", "#######", "#######",
        ]);
        assert_eq!(
            rows(&contract(&mask, 1, &never()).unwrap()),
            [
                ".......", ".#####.", ".##.##.", ".#...#.", ".##.##.", ".#####.", "......."
            ]
        );
        // Contracting past the middle leaves an empty selection.
        assert!(
            contract(&mask, 4, &never())
                .unwrap()
                .as_raw()
                .iter()
                .all(|v| *v == 0)
        );
    }

    #[test]
    fn hard_and_soft_paths_agree_with_the_disc_reference() {
        let mut state = 7u32;
        let mask = GrayImage::from_fn(23, 17, |_, _| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            Luma([if state >> 28 == 0 { 255 } else { 0 }])
        });
        for radius in [1, 2, 3, 5, 8] {
            for grow in [true, false] {
                let expected = reference(&mask, radius, grow);
                let hard = if grow {
                    expand(&mask, radius, &never())
                } else {
                    contract(&mask, radius, &never())
                };
                assert_eq!(hard.unwrap(), expected, "hard r={radius} grow={grow}");
                let soft = morphology(&mask, radius, grow, &never()).unwrap();
                assert_eq!(soft, expected, "soft r={radius} grow={grow}");
            }
        }
    }

    #[test]
    fn soft_selections_keep_their_levels() {
        let mask = GrayImage::from_raw(5, 1, vec![0, 0, 128, 0, 0]).unwrap();
        assert_eq!(
            expand(&mask, 1, &never()).unwrap().as_raw(),
            &[0, 128, 128, 128, 0]
        );
        let mask = GrayImage::from_raw(5, 1, vec![255, 200, 100, 200, 255]).unwrap();
        // Rows above and below are outside the canvas: unselected.
        assert!(
            contract(&mask, 1, &never())
                .unwrap()
                .as_raw()
                .iter()
                .all(|v| *v == 0)
        );
        let mask = GrayImage::from_fn(5, 3, |x, _| Luma([[255, 200, 100, 200, 255][x as usize]]));
        assert_eq!(
            contract(&mask, 1, &never()).unwrap().as_raw()[5..10],
            [0, 100, 100, 100, 0]
        );
    }

    #[test]
    fn cancelled_morphology_returns_nothing() {
        let cancel = AtomicBool::new(true);
        let mask = GrayImage::from_pixel(10, 10, Luma([255]));
        assert!(expand(&mask, 3, &cancel).is_none());
        let soft = GrayImage::from_pixel(10, 10, Luma([3]));
        assert!(contract(&soft, 3, &cancel).is_none());
    }

    #[test]
    fn color_range_selects_exact_colours_and_fades_with_fuzziness() {
        let image = RgbaImage::from_fn(6, 1, |x, _| {
            image::Rgba(match x {
                0 => [200, 10, 10, 255],
                1 => [190, 10, 10, 255],
                2 => [180, 10, 10, 255],
                3 => [10, 200, 10, 255],
                4 => [200, 10, 10, 0],
                _ => [170, 10, 10, 255],
            })
        });
        let mut range = ColorRange {
            include: vec![[200, 10, 10]],
            fuzziness: 0,
            ..ColorRange::default()
        };
        assert_eq!(range.mask(&image).as_raw(), &[255, 0, 0, 0, 0, 0]);
        // Within fuzziness / 2 fully selected, then linear down to 0 past fuzziness.
        range.fuzziness = 20;
        assert_eq!(range.mask(&image).as_raw(), &[255, 255, 23, 0, 0, 0]);
        range.fuzziness = 40;
        let mask = range.mask(&image);
        assert_eq!(mask.as_raw()[..5], [255, 255, 255, 0, 0]);
        // Distance 30 with fuzziness 40: (41 - 30) / (41 - 20) of the way.
        assert_eq!(mask.as_raw()[5], (11.0f32 / 21.0 * 255.0).round() as u8);
        // Values fall monotonically with the distance.
        range.fuzziness = 200;
        let ramp = RgbaImage::from_fn(201, 1, |x, _| image::Rgba([x as u8, 0, 0, 255]));
        range.include = vec![[0, 0, 0]];
        let mask = range.mask(&ramp);
        assert!(mask.as_raw().windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(mask.as_raw()[100], 255);
        assert!(mask.as_raw()[200] > 0 && mask.as_raw()[200] < 10);
        // Invert, and excluded colours.
        range.fuzziness = 0;
        range.include = vec![[200, 10, 10], [10, 200, 10]];
        range.exclude = vec![[10, 200, 10]];
        assert_eq!(range.mask(&image).as_raw(), &[255, 0, 0, 0, 0, 0]);
        range.invert = true;
        assert_eq!(range.mask(&image).as_raw(), &[0, 255, 255, 255, 255, 255]);
    }

    /// A hard `width` × `height` canvas with the rectangle `[left, right) × [top, bottom)`
    /// selected.
    fn marquee(size: (u32, u32), left: u32, top: u32, right: u32, bottom: u32) -> GrayImage {
        GrayImage::from_fn(size.0, size.1, |x, y| {
            Luma([
                if (left..right).contains(&x) && (top..bottom).contains(&y) {
                    255
                } else {
                    0
                },
            ])
        })
    }

    /// The columns of row `y` the stroke fully covers, as runs `[start, end)`.
    fn covered_runs(coverage: &GrayImage, y: u32) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        let mut start = None;
        for x in 0..=coverage.width() {
            let full = x < coverage.width() && coverage.get_pixel(x, y)[0] == 255;
            match (full, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    runs.push((s, x));
                    start = None;
                }
                _ => {}
            }
        }
        runs
    }

    /// Column `x` laid out as a row, top first.
    fn column(coverage: &GrayImage, x: u32) -> GrayImage {
        GrayImage::from_fn(coverage.height(), 1, |y, _| *coverage.get_pixel(x, y))
    }

    #[test]
    fn stroke_widths_on_a_rectangle_are_exact() {
        // Selected: x 20..60, y 15..45 on an 80 × 60 canvas.
        let mask = marquee((80, 60), 20, 15, 60, 45);
        let stroke = |width, location| stroke_coverage(&mask, width, location, &never()).unwrap();
        let outside = stroke(10, StrokeLocation::Outside);
        // Exactly ten pixels beyond each side, nothing on or inside the edge.
        assert_eq!(covered_runs(&outside, 30), [(10, 20), (60, 70)]);
        assert_eq!(covered_runs(&column(&outside, 40), 0), [(5, 15), (45, 55)]);
        // Only the rounded corners are partly covered; the sides are crisp.
        for (x, y, p) in outside.enumerate_pixels() {
            if p[0] != 0 && p[0] != 255 {
                assert!(
                    !(20..60).contains(&x) && !(15..45).contains(&y),
                    "({x}, {y})"
                );
            }
        }
        // The corners round off with the disc: 8 px out diagonally is past the reach
        // (√128 > 10), 7 px is within it (√98 < 10).
        assert_eq!(outside.get_pixel(20 - 8, 15 - 8)[0], 0);
        assert_eq!(outside.get_pixel(20 - 7, 15 - 7)[0], 255);

        let inside = stroke(10, StrokeLocation::Inside);
        assert_eq!(covered_runs(&inside, 30), [(20, 30), (50, 60)]);
        assert_eq!(covered_runs(&column(&inside, 40), 0), [(15, 25), (35, 45)]);
        // Nothing outside the selection.
        assert!(
            inside
                .enumerate_pixels()
                .all(|(x, y, p)| p[0] == 0 || mask.get_pixel(x, y)[0] == 255)
        );
        // Inside corners stay square.
        assert_eq!(inside.get_pixel(20, 15)[0], 255);

        let center = stroke(10, StrokeLocation::Center);
        assert_eq!(covered_runs(&center, 30), [(15, 25), (55, 65)]);
        assert_eq!(covered_runs(&column(&center, 40), 0), [(10, 20), (40, 50)]);
        // An odd width puts the extra pixel outside.
        let odd = stroke(5, StrokeLocation::Center);
        assert_eq!(covered_runs(&odd, 30), [(17, 22), (58, 63)]);
    }

    #[test]
    fn stroke_meets_the_canvas_edge_and_nothing_strokes_nothing() {
        // Select All: an inside stroke borders the canvas, an outside one has nowhere to go.
        let all = GrayImage::from_pixel(12, 8, Luma([255]));
        let inside = stroke_coverage(&all, 2, StrokeLocation::Inside, &never()).unwrap();
        assert_eq!(covered_runs(&inside, 4), [(0, 2), (10, 12)]);
        assert_eq!(covered_runs(&inside, 0), [(0, 12)]);
        let outside = stroke_coverage(&all, 2, StrokeLocation::Outside, &never()).unwrap();
        assert!(outside.as_raw().iter().all(|&v| v == 0));
        // A selection touching the edge strokes as if the canvas went on.
        let edge = marquee((30, 10), 0, 0, 10, 10);
        let inside = stroke_coverage(&edge, 3, StrokeLocation::Inside, &never()).unwrap();
        assert_eq!(covered_runs(&inside, 5), [(0, 3), (7, 10)]);
        let empty = GrayImage::new(10, 10);
        let none = stroke_coverage(&empty, 4, StrokeLocation::Center, &never()).unwrap();
        assert!(none.as_raw().iter().all(|&v| v == 0));
        let zero = stroke_coverage(&edge, 0, StrokeLocation::Outside, &never()).unwrap();
        assert!(zero.as_raw().iter().all(|&v| v == 0));
        // Widths stop at the maximum.
        let wide = marquee((600, 3), 299, 1, 300, 2);
        let capped = stroke_coverage(&wide, 1000, StrokeLocation::Outside, &never()).unwrap();
        assert_eq!(
            covered_runs(&capped, 1),
            [(299 - MAX_STROKE_WIDTH, 299), (300, 300 + MAX_STROKE_WIDTH)]
        );
        let cancelled = AtomicBool::new(true);
        assert!(stroke_coverage(&edge, 3, StrokeLocation::Outside, &cancelled).is_none());
        let soft = GrayImage::from_pixel(10, 10, Luma([128]));
        assert!(stroke_coverage(&soft, 3, StrokeLocation::Outside, &cancelled).is_none());
    }

    #[test]
    fn stroke_on_an_ellipse_or_lasso_is_antialiased_and_round() {
        let ellipse = crate::selection::rectangle(
            100,
            100,
            crate::document::Point::new(20.0, 20.0),
            crate::document::Point::new(80.0, 80.0),
            true,
        );
        let ring = stroke_coverage(&ellipse, 6, StrokeLocation::Center, &never()).unwrap();
        let partial = ring
            .as_raw()
            .iter()
            .filter(|&&v| v != 0 && v != 255)
            .count();
        // Both edges of the line are antialiased, all the way round.
        assert!(partial > 150, "{partial} partly covered pixels");
        // The line is a ring of about the right area, π (33² − 27²): a hard selection has
        // no edge finer than its pixels, so on a curve each side may sit a fraction of a
        // pixel out.
        let area: f32 = ring.as_raw().iter().map(|&v| f32::from(v) / 255.0).sum();
        let expected = std::f32::consts::PI * (33.0f32.powi(2) - 27.0f32.powi(2));
        assert!((area / expected - 1.0).abs() < 0.08, "{area} vs {expected}");
        // Every pixel's coverage matches its distance from the circle's edge to within a
        // pixel: the outline is round, not squared off at 45°.
        for (x, y, p) in ring.enumerate_pixels() {
            let r = ((x as f32 + 0.5 - 50.0).powi(2) + (y as f32 + 0.5 - 50.0).powi(2)).sqrt();
            if (r - 30.0).abs() < 2.0 {
                assert_eq!(p[0], 255, "({x}, {y}) is on the circle");
            }
            if (r - 30.0).abs() > 4.5 {
                assert_eq!(p[0], 0, "({x}, {y}) is {r} from the centre");
            }
        }
        // An outside stroke ramps off over about a pixel on its outer edge.
        let outside = stroke_coverage(&ellipse, 6, StrokeLocation::Outside, &never()).unwrap();
        let rim: Vec<u8> = (80..90).map(|x| outside.get_pixel(x, 50)[0]).collect();
        assert_eq!(&rim[..6], &[255; 6]);
        assert_eq!(&rim[7..], &[0; 3]);

        let lasso = crate::selection::polygon(
            60,
            60,
            &[
                crate::document::Point::new(10.0, 10.0),
                crate::document::Point::new(50.0, 18.0),
                crate::document::Point::new(25.0, 50.0),
            ],
        );
        let ring = stroke_coverage(&lasso, 4, StrokeLocation::Outside, &never()).unwrap();
        let partial = ring
            .as_raw()
            .iter()
            .filter(|&&v| v != 0 && v != 255)
            .count();
        assert!(partial > 40, "{partial} partly covered pixels");
        assert!(
            ring.enumerate_pixels()
                .all(|(x, y, p)| p[0] == 0 || lasso.get_pixel(x, y)[0] == 0)
        );
    }

    #[test]
    fn soft_selections_give_soft_strokes() {
        // Half selected: the stroke is half strength.
        let mut half = marquee((40, 30), 10, 10, 30, 20);
        for value in half.as_mut() {
            *value /= 2;
        }
        let ring = stroke_coverage(&half, 3, StrokeLocation::Outside, &never()).unwrap();
        assert_eq!(ring.get_pixel(8, 15)[0], 127);
        assert_eq!(ring.get_pixel(7, 15)[0], 127);
        assert_eq!(ring.get_pixel(6, 15)[0], 0);
        assert_eq!(ring.get_pixel(15, 15)[0], 0);
        // A feathered edge stays feathered: the stroke ramps as the selection does.
        let feathered = crate::gpu::blur_gray(&marquee((60, 40), 20, 10, 40, 30), 3.0);
        let ring = stroke_coverage(&feathered, 6, StrokeLocation::Center, &never()).unwrap();
        let row: Vec<u8> = (0..30).map(|x| ring.get_pixel(x, 20)[0]).collect();
        let peak = row.iter().copied().max().unwrap();
        assert!(peak > 150, "{row:?}");
        assert!(
            row.iter().filter(|&&v| v > 0 && v < peak).count() >= 4,
            "{row:?}"
        );
    }

    #[test]
    fn eyedropper_averages_the_opaque_neighbours() {
        let image = RgbaImage::from_fn(3, 3, |x, y| {
            if (x, y) == (0, 0) {
                image::Rgba([255, 255, 255, 0])
            } else {
                image::Rgba([90, 30, 60, 255])
            }
        });
        assert_eq!(sample_color(&image, 1.5, 1.5), Some([90, 30, 60]));
        assert_eq!(sample_color(&image, 0.2, 0.2), Some([90, 30, 60]));
        assert_eq!(sample_color(&image, 3.0, 1.0), None);
        assert_eq!(sample_color(&RgbaImage::new(2, 2), 1.0, 1.0), None);
    }
}

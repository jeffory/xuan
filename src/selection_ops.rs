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
/// get infinity.
fn squared_distance(
    width: u32,
    height: u32,
    cancel: &AtomicBool,
    seed: impl Fn(u32, u32) -> bool,
) -> Option<Vec<f64>> {
    let (w, h) = (width as usize, height as usize);
    let mut grid = vec![f64::INFINITY; w * h];
    for y in 0..height {
        for x in 0..width {
            if seed(x, y) {
                grid[y as usize * w + x as usize] = 0.0;
            }
        }
    }
    let longest = w.max(h);
    let mut f = vec![0.0; longest];
    let mut d = vec![0.0; longest];
    let mut v = vec![0usize; longest];
    let mut z = vec![0.0; longest + 1];
    for x in 0..w {
        if x % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        transform_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        if y % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        f[..w].copy_from_slice(&grid[y * w..(y + 1) * w]);
        transform_1d(&f[..w], &mut grid[y * w..(y + 1) * w], &mut v, &mut z);
    }
    Some(grid)
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
/// counts as unselected, so erosion shrinks from the canvas edges too.
fn morphology(mask: &GrayImage, radius: u32, grow: bool, cancel: &AtomicBool) -> Option<GrayImage> {
    let (width, height) = mask.dimensions();
    let w = width as usize;
    let r = radius.min(width.max(height)) as i64;
    // Half-width of the disc on each row offset.
    let spans: Vec<usize> = (-r..=r)
        .map(|dy| ((r * r - dy * dy) as f64).sqrt().floor() as usize)
        .collect();
    let mut output = GrayImage::new(width, height);
    let mut window = vec![0u8; w];
    let mut accumulated = vec![0u8; w];
    let mut deque = std::collections::VecDeque::with_capacity(w);
    for y in 0..height as i64 {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        accumulated.fill(if grow { 0 } else { 255 });
        for (i, dy) in (-r..=r).enumerate() {
            let sy = y + dy;
            if sy < 0 || sy >= i64::from(height) {
                if !grow {
                    // An unselected row outside the canvas lies within reach.
                    accumulated.fill(0);
                }
                continue;
            }
            let row = &mask.as_raw()[sy as usize * w..(sy as usize + 1) * w];
            sliding_extreme(row, spans[i], grow, &mut window, &mut deque);
            for (a, &b) in accumulated.iter_mut().zip(&window) {
                *a = if grow { (*a).max(b) } else { (*a).min(b) };
            }
        }
        output.as_mut()[y as usize * w..(y as usize + 1) * w].copy_from_slice(&accumulated);
    }
    Some(output)
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
        let mut output = GrayImage::new(image.width(), image.height());
        for (pixel, out) in image.pixels().zip(output.as_mut()) {
            let [r, g, b, a] = pixel.0;
            let mut value = if a == 0 || self.include.is_empty() {
                0.0
            } else {
                let rgb = [r, g, b];
                self.closeness(rgb, &self.include) * (1.0 - self.closeness(rgb, &self.exclude))
            };
            if self.invert {
                value = 1.0 - value;
            }
            *out = (value * 255.0).round() as u8;
        }
        output
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

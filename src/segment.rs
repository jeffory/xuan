//! Classical foreground segmentation for Select → Subject, Remove Background and the
//! Magic tool's Object mode. No machine learning: plugins can replace it (see
//! `docs/PLUGINS.md`, "Providers").
//!
//! The algorithm is GrabCut (Rother, Kolmogorov and Blake, 2004):
//!
//! 1. The image is reduced so its longer side is at most [`WORK_SIDE`] pixels, which
//!    bounds the graph whatever the image size.
//! 2. A trimap seeds it: a thin band along the border is background with a strong prior
//!    (not a hard constraint, so subjects that run off the image keep their edge), a
//!    wider margin probably background and the centre probably foreground (Object mode: a
//!    rectangle's outside is background, its inside probable foreground, and a click is
//!    definitely foreground). Fully transparent pixels are background.
//! 3. Two colour models, Gaussian mixtures with five full-covariance components each,
//!    are learnt from the two sides; every pixel gets a cost for each side, and
//!    neighbours (8-connected) a cost for being cut apart that is high between similar
//!    colours. A minimum cut ([`maxflow`], Boykov–Kolmogorov) labels the probable
//!    pixels, the models are learnt again from the new labels, and this repeats.
//! 4. The hard mask is cleaned (specks dropped; Object mode keeps what was clicked),
//!    scaled back up and refined with a colour guided filter (He, Sun and Tang, 2010) on
//!    the full-resolution image, which pulls the edge onto the image's own edges and
//!    gives it a soft, anti-aliased alpha. The filter runs only in tiles that hold the
//!    boundary, so everything away from it stays exactly 0 or 255.
use std::sync::atomic::{AtomicBool, Ordering};

use image::{GrayImage, RgbaImage};
use rayon::prelude::*;

mod gmm;
pub mod maxflow;

use gmm::{COMPONENTS, Gmm};
use maxflow::{Cap, Graph, Side};

/// The longest side of the image the graph cut works on.
pub const WORK_SIDE: u32 = 512;
/// GrabCut iterations; it usually settles in fewer and stops then.
const ITERATIONS: usize = 5;
/// GrabCut's smoothness weight.
const GAMMA: f64 = 50.0;
/// Fixed-point scale of the graph's capacities.
const SCALE: f64 = 100.0;
/// The cost of going against a definite label: more than all of a pixel's neighbour
/// links together.
const HARD: Cap = (9.0 * GAMMA * SCALE) as Cap;
/// Data costs are capped so a colour no model explains cannot overflow a capacity.
const MAX_COST: f64 = 200.0;
/// How much more a pixel of the border band costs as foreground, in nats. The band
/// is not definitely background, so a subject that runs off the image (a portrait's
/// shoulders) can still reach the edge where its colour and its neighbours say so,
/// but it takes clear evidence.
const BORDER_PRIOR: f64 = 4.0;
/// The guided filter's regularisation, for colours in 0–1.
const EPSILON: f32 = 2e-3;

const BACKGROUND: u8 = 0;
const FOREGROUND: u8 = 1;
const PROBABLY_BACKGROUND: u8 = 2;
const PROBABLY_FOREGROUND: u8 = 3;

fn is_foreground(label: u8) -> bool {
    label == FOREGROUND || label == PROBABLY_FOREGROUND
}

/// Where the user pointed, in image pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seeds {
    /// Clicks on the object: definitely foreground, and only the parts connected to
    /// them are kept.
    pub points: Vec<(f32, f32)>,
    /// A rectangle around the object, `[left, top, right, bottom]`: everything outside
    /// is background.
    pub rect: Option<[f32; 4]>,
}

impl Seeds {
    /// Select Subject and Remove Background: no hints.
    pub fn subject() -> Self {
        Self::default()
    }
}

/// A segmentation and what it took.
#[derive(Clone, Debug)]
pub struct Segmentation {
    /// White over the foreground, at the image's size.
    pub mask: GrayImage,
    /// The size the graph cut worked at.
    pub work: (u32, u32),
    /// The graph's nodes and edges in its last iteration.
    pub nodes: usize,
    pub edges: usize,
    /// GrabCut iterations run.
    pub iterations: usize,
}

/// The working size for an image: the longer side at most [`WORK_SIDE`].
pub fn work_size(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    if longest <= WORK_SIDE {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(WORK_SIDE) / f64::from(longest);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

/// Segments the foreground of `image` (straight RGBA). `progress` receives 0–1;
/// returns `None` when `cancel` is set.
pub fn segment(
    image: &RgbaImage,
    seeds: &Seeds,
    progress: &(dyn Fn(f32) + Sync),
    cancel: &AtomicBool,
) -> Option<Segmentation> {
    let (width, height) = image.dimensions();
    let (w, h) = work_size(width, height);
    let small = if (w, h) == (width, height) {
        image.clone()
    } else {
        image::imageops::resize(image, w, h, image::imageops::FilterType::Triangle)
    };
    let (sx, sy) = (w as f32 / width as f32, h as f32 / height as f32);
    let colors: Vec<[f32; 3]> = small
        .pixels()
        .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
        .collect();
    let (mut labels, border) = trimap(&small, seeds, sx, sy);
    progress(0.05);
    let (mut nodes, mut edges, mut iterations) = (w as usize * h as usize, 0, 0);
    let has = |labels: &[u8], fg: bool| labels.iter().any(|&l| is_foreground(l) == fg);
    if w >= 2 && h >= 2 && has(&labels, true) && has(&labels, false) {
        let links = NeighbourLinks::new(&colors, w as usize, h as usize);
        let mut fg = Gmm::fit(&class(&colors, &labels, true));
        let mut bg = Gmm::fit(&class(&colors, &labels, false));
        for iteration in 0..ITERATIONS {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if iteration > 0 {
                fg = relearn(&colors, &labels, true, &fg);
                bg = relearn(&colors, &labels, false, &bg);
            }
            if fg.is_empty() || bg.is_empty() {
                break;
            }
            let mut graph = Graph::new(colors.len(), colors.len() * 4);
            for (i, (&label, color)) in labels.iter().zip(&colors).enumerate() {
                let (source, sink) = match label {
                    BACKGROUND => (0, HARD),
                    FOREGROUND => (HARD, 0),
                    _ if border[i] => (
                        capacity(bg.cost(*color)),
                        capacity(fg.cost(*color) + BORDER_PRIOR),
                    ),
                    _ => (capacity(bg.cost(*color)), capacity(fg.cost(*color))),
                };
                graph.add_terminal(i, source, sink);
            }
            links.add_to(&mut graph);
            graph.max_flow(cancel)?;
            nodes = graph.node_count();
            edges = graph.edge_count();
            iterations = iteration + 1;
            let mut changed = 0usize;
            for (i, label) in labels.iter_mut().enumerate() {
                if *label == PROBABLY_BACKGROUND || *label == PROBABLY_FOREGROUND {
                    let new = if graph.side(i) == Side::Source {
                        PROBABLY_FOREGROUND
                    } else {
                        PROBABLY_BACKGROUND
                    };
                    changed += usize::from(new != *label);
                    *label = new;
                }
            }
            progress(0.05 + 0.65 * (iteration + 1) as f32 / ITERATIONS as f32);
            if changed * 1000 < labels.len() && iteration > 0 {
                break;
            }
        }
    }
    let mut hard: Vec<bool> = labels.iter().map(|&l| is_foreground(l)).collect();
    let points: Vec<(usize, usize)> = seeds
        .points
        .iter()
        .map(|&(x, y)| {
            (
                ((x * sx) as usize).min(w as usize - 1),
                ((y * sy) as usize).min(h as usize - 1),
            )
        })
        .collect();
    clean(&mut hard, w as usize, h as usize, &points);
    progress(0.75);
    let mask = refine(image, &hard, w, h, cancel)?;
    progress(1.0);
    Some(Segmentation {
        mask,
        work: (w, h),
        nodes,
        edges,
        iterations,
    })
}

fn capacity(cost: f64) -> Cap {
    (cost.clamp(0.0, MAX_COST) * SCALE).round() as Cap
}

fn class(colors: &[[f32; 3]], labels: &[u8], foreground: bool) -> Vec<[f32; 3]> {
    colors
        .iter()
        .zip(labels)
        .filter(|(_, l)| is_foreground(**l) == foreground)
        .map(|(c, _)| *c)
        .collect()
}

/// GrabCut's model update: every pixel of a side takes its most likely component of
/// that side's mixture, and the mixture is learnt from those assignments.
fn relearn(colors: &[[f32; 3]], labels: &[u8], foreground: bool, model: &Gmm) -> Gmm {
    let samples = class(colors, labels, foreground);
    if samples.is_empty() {
        return Gmm::default();
    }
    let assigned: Vec<u8> = samples.iter().map(|c| model.component(*c)).collect();
    debug_assert!(assigned.iter().all(|&k| (k as usize) < COMPONENTS));
    Gmm::learn(&samples, &assigned)
}

/// The starting labels at the working size, and the border band.
fn trimap(small: &RgbaImage, seeds: &Seeds, sx: f32, sy: f32) -> (Vec<u8>, Vec<bool>) {
    let (w, h) = small.dimensions();
    let shortest = w.min(h) as f32;
    let band = (shortest * 0.01).round().max(1.0) as u32;
    let margin = ((shortest * 0.08).round() as u32).max(band + 1);
    let mut labels = vec![PROBABLY_FOREGROUND; w as usize * h as usize];
    let mut border = vec![false; labels.len()];
    for y in 0..h {
        for x in 0..w {
            let edge = x.min(y).min(w - 1 - x).min(h - 1 - y);
            let label = if let Some([left, top, right, bottom]) = seeds.rect {
                let (px, py) = ((x as f32 + 0.5) / sx, (y as f32 + 0.5) / sy);
                if px < left || px > right || py < top || py > bottom {
                    BACKGROUND
                } else {
                    PROBABLY_FOREGROUND
                }
            } else if edge < band {
                border[(y * w + x) as usize] = true;
                PROBABLY_BACKGROUND
            } else if edge < margin {
                PROBABLY_BACKGROUND
            } else {
                PROBABLY_FOREGROUND
            };
            labels[(y * w + x) as usize] = label;
        }
    }
    for (label, pixel) in labels.iter_mut().zip(small.pixels()) {
        if pixel[3] < 13 {
            *label = BACKGROUND;
        }
    }
    let radius = 2i64;
    for &(x, y) in &seeds.points {
        let (cx, cy) = ((x * sx) as i64, (y * sy) as i64);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let (px, py) = (cx + dx, cy + dy);
                if dx * dx + dy * dy <= radius * radius
                    && (0..i64::from(w)).contains(&px)
                    && (0..i64::from(h)).contains(&py)
                {
                    labels[(py as u32 * w + px as u32) as usize] = FOREGROUND;
                }
            }
        }
    }
    (labels, border)
}

/// The n-link weights of the 8-connected grid, computed once: each pixel links to its
/// left, upper-left, upper and upper-right neighbours.
struct NeighbourLinks {
    width: usize,
    height: usize,
    /// Per pixel, in that order; 0 where the neighbour is outside.
    weights: Vec<[Cap; 4]>,
}

const OFFSETS: [(isize, isize); 4] = [(-1, 0), (-1, -1), (0, -1), (1, -1)];

impl NeighbourLinks {
    fn new(colors: &[[f32; 3]], width: usize, height: usize) -> Self {
        let neighbour = |x: usize, y: usize, k: usize| -> Option<usize> {
            let (dx, dy) = OFFSETS[k];
            let (nx, ny) = (x as isize + dx, y as isize + dy);
            (nx >= 0 && ny >= 0 && (nx as usize) < width).then(|| ny as usize * width + nx as usize)
        };
        let diff = |a: [f32; 3], b: [f32; 3]| -> f64 {
            a.iter().zip(b).map(|(p, q)| f64::from(p - q).powi(2)).sum()
        };
        let (mut total, mut count) = (0.0, 0usize);
        for y in 0..height {
            for x in 0..width {
                for k in 0..4 {
                    if let Some(j) = neighbour(x, y, k) {
                        total += diff(colors[y * width + x], colors[j]);
                        count += 1;
                    }
                }
            }
        }
        let beta = if total > 0.0 {
            count as f64 / (2.0 * total)
        } else {
            0.0
        };
        let mut weights = vec![[0; 4]; width * height];
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                for (k, slot) in weights[i].iter_mut().enumerate() {
                    if let Some(j) = neighbour(x, y, k) {
                        let distance = if k % 2 == 1 {
                            std::f64::consts::SQRT_2
                        } else {
                            1.0
                        };
                        let weight = GAMMA / distance * (-beta * diff(colors[i], colors[j])).exp();
                        *slot = (weight * SCALE).round() as Cap;
                    }
                }
            }
        }
        Self {
            width,
            height,
            weights,
        }
    }

    fn add_to(&self, graph: &mut Graph) {
        for y in 0..self.height {
            for x in 0..self.width {
                let i = y * self.width + x;
                for (k, &(dx, dy)) in OFFSETS.iter().enumerate() {
                    let weight = self.weights[i][k];
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if weight > 0 && nx >= 0 && ny >= 0 && (nx as usize) < self.width {
                        graph.add_edge(i, ny as usize * self.width + nx as usize, weight, weight);
                    }
                }
            }
        }
    }
}

/// Drops specks: foreground regions under 1% of the largest one. With clicks, keeps
/// only the regions that hold a click (8-connected).
fn clean(mask: &mut [bool], width: usize, height: usize, points: &[(usize, usize)]) {
    let mut region = vec![u32::MAX; mask.len()];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || region[start] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        let mut size = 0usize;
        region[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i % width) as isize, (i / width) as isize);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx as usize >= width || ny as usize >= height {
                        continue;
                    }
                    let j = ny as usize * width + nx as usize;
                    if mask[j] && region[j] == u32::MAX {
                        region[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        sizes.push(size);
    }
    let keep: Vec<bool> = if points.is_empty() {
        let largest = sizes.iter().copied().max().unwrap_or(0);
        sizes.iter().map(|&s| s * 100 >= largest).collect()
    } else {
        let mut keep = vec![false; sizes.len()];
        for &(x, y) in points {
            if let Some(&id) = region.get(y * width + x).filter(|&&id| id != u32::MAX) {
                keep[id as usize] = true;
            }
        }
        keep
    };
    for (m, &id) in mask.iter_mut().zip(&region) {
        *m = id != u32::MAX && keep[id as usize];
    }
}

/// Tiles the guided filter works in, before their apron.
const TILE: u32 = 64;

/// Scales the working mask up to the image and refines its edge with a colour guided
/// filter, only in tiles near the boundary.
fn refine(
    image: &RgbaImage,
    hard: &[bool],
    w: u32,
    h: u32,
    cancel: &AtomicBool,
) -> Option<GrayImage> {
    let (width, height) = image.dimensions();
    let (sx, sy) = (w as f32 / width as f32, h as f32 / height as f32);
    // The upscaled mask: bilinear between working pixels, so 0 or 1 away from edges.
    let coverage = |x: u32, y: u32| -> f32 {
        let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: u32, y: u32| f32::from(u8::from(hard[(y * w + x) as usize]));
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    // The filter's radius: about one working pixel, at least two image pixels.
    let radius = ((1.0 / sx.min(sy)).ceil() as u32).max(2);
    let apron = 2 * radius;
    let tiles: Vec<(u32, u32)> = (0..height.div_ceil(TILE))
        .flat_map(|ty| (0..width.div_ceil(TILE)).map(move |tx| (tx * TILE, ty * TILE)))
        .collect();
    /// A tile's top-left corner and its values, or `None` when cancelled.
    type Tile = Option<((u32, u32), Vec<u8>)>;
    let results: Vec<Tile> = tiles
        .par_iter()
        .map(|&(x0, y0)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let (x1, y1) = ((x0 + TILE).min(width), (y0 + TILE).min(height));
            let (px0, py0) = (x0.saturating_sub(apron), y0.saturating_sub(apron));
            let (px1, py1) = ((x1 + apron).min(width), (y1 + apron).min(height));
            // Working pixels under the patch, with one more around for the bilinear
            // reach: if they all agree, the patch is uniform and needs no filter.
            let wx0 = ((px0 as f32 * sx).floor() as u32).saturating_sub(1);
            let wy0 = ((py0 as f32 * sy).floor() as u32).saturating_sub(1);
            let wx1 = ((px1 as f32 * sx).ceil() as u32 + 1).min(w - 1);
            let wy1 = ((py1 as f32 * sy).ceil() as u32 + 1).min(h - 1);
            let first = hard[(wy0 * w + wx0) as usize];
            let uniform =
                (wy0..=wy1).all(|y| (wx0..=wx1).all(|x| hard[(y * w + x) as usize] == first));
            let tile = if uniform {
                vec![if first { 255 } else { 0 }; ((x1 - x0) * (y1 - y0)) as usize]
            } else {
                guided_tile(
                    image,
                    &coverage,
                    [px0, py0, px1, py1],
                    [x0, y0, x1, y1],
                    radius,
                )
            };
            Some(((x0, y0), tile))
        })
        .collect();
    let mut mask = GrayImage::new(width, height);
    for result in results {
        let ((x0, y0), tile) = result?;
        let tw = (x0 + TILE).min(width) - x0;
        for (row, values) in tile.chunks(tw as usize).enumerate() {
            let start = ((y0 + row as u32) * width + x0) as usize;
            mask.as_mut()[start..start + tw as usize].copy_from_slice(values);
        }
    }
    Some(mask)
}

/// The colour guided filter over one patch, returning the tile inside it.
fn guided_tile(
    image: &RgbaImage,
    coverage: &(dyn Fn(u32, u32) -> f32 + Sync),
    [px0, py0, px1, py1]: [u32; 4],
    [x0, y0, x1, y1]: [u32; 4],
    radius: u32,
) -> Vec<u8> {
    let (pw, ph) = ((px1 - px0) as usize, (py1 - py0) as usize);
    let n = pw * ph;
    let mut guide = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    let mut p = vec![0f32; n];
    for y in 0..ph {
        for x in 0..pw {
            let (ix, iy) = (px0 + x as u32, py0 + y as u32);
            let pixel = image.get_pixel(ix, iy);
            let i = y * pw + x;
            for c in 0..3 {
                guide[c][i] = f32::from(pixel[c]) / 255.0;
            }
            p[i] = coverage(ix, iy);
        }
    }
    let r = radius as usize;
    let mean = |values: &[f32]| box_mean(values, pw, ph, r);
    let product =
        |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| x * y).collect() };
    let mi: [Vec<f32>; 3] = std::array::from_fn(|c| mean(&guide[c]));
    let mp = mean(&p);
    let mip: [Vec<f32>; 3] = std::array::from_fn(|c| mean(&product(&guide[c], &p)));
    let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let mii: [Vec<f32>; 6] =
        std::array::from_fn(|k| mean(&product(&guide[pairs[k].0], &guide[pairs[k].1])));
    let mut a = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    let mut b = vec![0f32; n];
    for i in 0..n {
        let m = [mi[0][i], mi[1][i], mi[2][i]];
        let v = |k: usize| f64::from(mii[k][i] - m[pairs[k].0] * m[pairs[k].1]);
        let e = f64::from(EPSILON);
        let sigma = [
            [v(0) + e, v(1), v(2)],
            [v(1), v(3) + e, v(4)],
            [v(2), v(4), v(5) + e],
        ];
        let cov = [0, 1, 2].map(|c| f64::from(mip[c][i] - m[c] * mp[i]));
        let (inverse, _) = gmm::invert(sigma);
        let mut bias = f64::from(mp[i]);
        for c in 0..3 {
            let value = inverse[c][0] * cov[0] + inverse[c][1] * cov[1] + inverse[c][2] * cov[2];
            a[c][i] = value as f32;
            bias -= value * f64::from(m[c]);
        }
        b[i] = bias as f32;
    }
    let ma: [Vec<f32>; 3] = std::array::from_fn(|c| mean(&a[c]));
    let mb = mean(&b);
    let mut out = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y - py0) as usize * pw + (x - px0) as usize;
            let q =
                ma[0][i] * guide[0][i] + ma[1][i] * guide[1][i] + ma[2][i] * guide[2][i] + mb[i];
            out.push((q.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    out
}

/// The mean over the `(2r+1)²` window around every pixel, the window cut to the patch.
fn box_mean(values: &[f32], width: usize, height: usize, r: usize) -> Vec<f32> {
    let stride = width + 1;
    let mut table = vec![0f64; stride * (height + 1)];
    for y in 0..height {
        let mut row = 0.0;
        for x in 0..width {
            row += f64::from(values[y * width + x]);
            table[(y + 1) * stride + x + 1] = table[y * stride + x + 1] + row;
        }
    }
    let mut out = vec![0f32; width * height];
    for y in 0..height {
        let (top, bottom) = (y.saturating_sub(r), (y + r + 1).min(height));
        for x in 0..width {
            let (left, right) = (x.saturating_sub(r), (x + r + 1).min(width));
            let sum = table[bottom * stride + right]
                - table[top * stride + right]
                - table[bottom * stride + left]
                + table[top * stride + left];
            out[y * width + x] = (sum / ((bottom - top) * (right - left)) as f64) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests;

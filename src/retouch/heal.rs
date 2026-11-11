//! Spot healing kernel, ported from upstream Compositor's `HealPixels.c`.
//!
//! One coherent source patch is chosen near the spot by matching the ring of pixels
//! around it, then the difference along the spot's edge is spread smoothly across the
//! spot (a membrane, or Laplace, fill) so the copied texture meets its surroundings
//! exactly.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail, ensure};

/// How Spot Healing rebuilds the painted area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HealMode {
    /// Copy texture from the nearby patch whose surrounding ring best matches the spot's.
    #[default]
    ContentAware,
    /// Fill smoothly from the spot's edges and add grain matching the detail around it.
    CreateTexture,
    /// Like Content-Aware, but prefer the closest good patch.
    ProximityMatch,
}

const OUTSIDE: u8 = 0;
const RING: u8 = 1;
const HOLE: u8 = 2;

/// Work-box pixels above which healing refuses rather than exhausting memory
/// (16 bytes of solver state per pixel).
const MAX_WORK_PIXELS: usize = 32 * 1024 * 1024;

/// Half-open `[x0, y0, x1, y1]` bounds of the nonzero bytes in a gray bitmap.
pub fn coverage_bounds(gray: &[u8], width: usize, height: usize) -> Option<[usize; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for y in 0..height {
        let row = &gray[y * width..(y + 1) * width];
        let Some(first) = row.iter().position(|&v| v != 0) else {
            continue;
        };
        let last = row.iter().rposition(|&v| v != 0).unwrap();
        x0 = x0.min(first);
        x1 = x1.max(last + 1);
        y0 = y0.min(y);
        y1 = y + 1;
    }
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
}

pub(super) fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

fn unit(key: u32) -> f64 {
    f64::from(hash(key) >> 8) / 16_777_216.0
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    Ok(())
}

/// Mean squared difference between the ring around the spot and the ring around the
/// patch offset by `(dx, dy)`. Infinite when the patch would touch the painted pixels or
/// leave the image.
struct Scorer<'a> {
    rgba: &'a [u8],
    /// Byte offsets of the ring pixels in `rgba`.
    ring: &'a [usize],
    /// Roles of the work box, row-major.
    role: &'a [u8],
    /// Work-box coordinates of every ring and hole pixel.
    cells: &'a [(i64, i64)],
    work: [i64; 4],
    size: [i64; 2],
}

impl Scorer<'_> {
    /// Whether the patch at `(dx, dy)` stays inside the image and off the painted pixels.
    /// A thin stroke's bounding box is mostly untouched skin, so the test is on the
    /// pixels themselves, not the box: a source two stroke-widths to the side is fine.
    fn usable(&self, dx: i64, dy: i64) -> bool {
        let [wx0, wy0, ww, wh] = self.work;
        let [w, h] = self.size;
        let box_inside = wx0 + dx >= 0 && wy0 + dy >= 0 && wx0 + ww + dx <= w && wy0 + wh + dy <= h;
        if box_inside && (dx.abs() >= ww || dy.abs() >= wh) {
            return true;
        }
        self.cells.iter().all(|&(x, y)| {
            let (sx, sy) = (x + dx, y + dy);
            if sx + wx0 < 0 || sy + wy0 < 0 || sx + wx0 >= w || sy + wy0 >= h {
                return false;
            }
            !(sx >= 0 && sy >= 0 && sx < ww && sy < wh)
                || self.role[(sy * ww + sx) as usize] != HOLE
        })
    }

    fn score(&self, dx: i64, dy: i64) -> f64 {
        if self.ring.is_empty() || !self.usable(dx, dy) {
            return f64::INFINITY;
        }
        let [w, _] = self.size;
        let shift = (dy * w + dx) * 4;
        let mut sum = 0u64;
        for &t in self.ring {
            let s = (t as i64 + shift) as usize;
            for c in 0..4 {
                let d = i64::from(self.rgba[t + c]) - i64::from(self.rgba[s + c]);
                sum += (d * d) as u64;
            }
        }
        sum as f64 / self.ring.len() as f64
    }
}

/// Solve for smooth values over HOLE pixels, fixed to the RING values around them. A
/// coarser copy is solved first and used as the starting point, so large spots settle in
/// few passes.
fn solve(
    value: &mut [f32],
    role: &[u8],
    w: usize,
    h: usize,
    depth: u32,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut iterations = 300;
    if w > 32 && h > 32 && depth < 16 {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut coarse = vec![0f32; cw * ch * 4];
        let mut coarse_role = vec![OUTSIDE; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let (mut known, mut hole) = (0, 0);
                let mut known_sum = [0f32; 4];
                let mut hole_sum = [0f32; 4];
                for j in 0..2 {
                    for i in 0..2 {
                        let (fx, fy) = (x * 2 + i, y * 2 + j);
                        if fx >= w || fy >= h {
                            continue;
                        }
                        let p = fy * w + fx;
                        let sum = match role[p] {
                            RING => {
                                known += 1;
                                &mut known_sum
                            }
                            HOLE => {
                                hole += 1;
                                &mut hole_sum
                            }
                            _ => continue,
                        };
                        for c in 0..4 {
                            sum[c] += value[p * 4 + c];
                        }
                    }
                }
                let q = y * cw + x;
                if known > 0 {
                    coarse_role[q] = RING;
                    for c in 0..4 {
                        coarse[q * 4 + c] = known_sum[c] / known as f32;
                    }
                } else if hole > 0 {
                    coarse_role[q] = HOLE;
                    for c in 0..4 {
                        coarse[q * 4 + c] = hole_sum[c] / hole as f32;
                    }
                }
            }
        }
        solve(&mut coarse, &coarse_role, cw, ch, depth + 1, cancel)?;
        for y in 0..h {
            for x in 0..w {
                let (p, q) = (y * w + x, (y / 2) * cw + x / 2);
                if role[p] == HOLE && coarse_role[q] == HOLE {
                    value[p * 4..p * 4 + 4].copy_from_slice(&coarse[q * 4..q * 4 + 4]);
                }
            }
        }
        iterations = 40;
    }
    const OMEGA: f32 = 1.8;
    for _ in 0..iterations {
        cancelled(cancel)?;
        for y in 0..h {
            for x in 0..w {
                let p = y * w + x;
                if role[p] != HOLE {
                    continue;
                }
                let mut sum = [0f32; 4];
                let mut n = 0;
                let neighbors = [
                    (x > 0).then(|| p - 1),
                    (x + 1 < w).then_some(p + 1),
                    (y > 0).then(|| p - w),
                    (y + 1 < h).then_some(p + w),
                ];
                for q in neighbors.into_iter().flatten() {
                    if role[q] == OUTSIDE {
                        continue;
                    }
                    for c in 0..4 {
                        sum[c] += value[q * 4 + c];
                    }
                    n += 1;
                }
                if n == 0 {
                    continue;
                }
                for c in 0..4 {
                    let v = &mut value[p * 4 + c];
                    *v += OMEGA * (sum[c] / n as f32 - *v);
                }
            }
        }
    }
    Ok(())
}

/// Spot healing, in place, over premultiplied RGBA (`width * 4` bytes per row).
/// `coverage` (`width * height` bytes, 0–255) marks what to heal; the result replaces
/// the original by coverage × opacity. Pixels with zero coverage are never written.
#[allow(clippy::too_many_arguments)]
pub fn spot_heal(
    rgba: &mut [u8],
    coverage: &[u8],
    width: usize,
    height: usize,
    opacity: f32,
    mode: HealMode,
    seed: u32,
    cancel: &AtomicBool,
) -> Result<()> {
    ensure!(
        rgba.len() == width * height * 4 && coverage.len() == width * height,
        "Healing buffers have mismatched sizes"
    );
    let (big_w, big_h) = (width as i64, height as i64);
    let Some(bounds) = coverage_bounds(coverage, width, height) else {
        return Ok(());
    };
    let [bx0, by0, bx1, by1] = bounds.map(|v| v as i64);
    let size = (bx1 - bx0).max(by1 - by0);
    let ring = (size / 8).clamp(2, 16);
    // Work box: the spot plus its ring, clipped to the image.
    let (wx0, wy0) = ((bx0 - ring).max(0), (by0 - ring).max(0));
    let (wx1, wy1) = ((bx1 + ring).min(big_w), (by1 + ring).min(big_h));
    let (ww, wh) = ((wx1 - wx0) as usize, (wy1 - wy0) as usize);
    let wn = ww * wh;
    ensure!(
        wn <= MAX_WORK_PIXELS,
        "The Spot Healing stroke is too large. Heal it in smaller strokes."
    );
    let at = |x: usize, y: usize| (wy0 as usize + y) * width + wx0 as usize + x;

    let mut role = vec![OUTSIDE; wn];
    for y in 0..wh {
        for x in 0..ww {
            if coverage[at(x, y)] != 0 {
                role[y * ww + x] = HOLE;
            }
        }
    }
    // The ring: pixels within `ring` of the spot (a square dilation, row pass then
    // column pass).
    let ring = ring as usize;
    let mut near = vec![false; wn];
    let mut prefix = vec![0usize; ww.max(wh) + 1];
    for y in 0..wh {
        for x in 0..ww {
            prefix[x + 1] = prefix[x] + usize::from(role[y * ww + x] == HOLE);
        }
        for x in 0..ww {
            let (lo, hi) = (x.saturating_sub(ring), (x + ring + 1).min(ww));
            near[y * ww + x] = prefix[hi] > prefix[lo];
        }
    }
    for x in 0..ww {
        for y in 0..wh {
            prefix[y + 1] = prefix[y] + usize::from(near[y * ww + x]);
        }
        for y in 0..wh {
            let (lo, hi) = (y.saturating_sub(ring), (y + ring + 1).min(wh));
            if role[y * ww + x] == OUTSIDE && prefix[hi] > prefix[lo] {
                role[y * ww + x] = RING;
            }
        }
    }
    drop(near);
    let ring_pixels: Vec<usize> = (0..wn)
        .filter(|&p| role[p] == RING)
        .map(|p| at(p % ww, p / ww) * 4)
        .collect();
    if ring_pixels.is_empty() {
        return Ok(());
    }
    cancelled(cancel)?;

    // Source patch for Content-Aware and Proximity Match.
    let mut offset = None;
    if mode != HealMode::CreateTexture {
        let cells: Vec<(i64, i64)> = (0..wn)
            .filter(|&p| role[p] != OUTSIDE)
            .map(|p| ((p % ww) as i64, (p / ww) as i64))
            .collect();
        let scorer = Scorer {
            rgba,
            ring: &ring_pixels,
            role: &role,
            cells: &cells,
            work: [wx0, wy0, ww as i64, wh as i64],
            size: [big_w, big_h],
        };
        let proximity = mode == HealMode::ProximityMatch;
        let step = if proximity { 0.6 } else { 0.1 };
        // Candidate distances, nearest first, each with the weight that makes nearer
        // patches win ties. Upstream's five are fractions of the work box, which for a
        // long stroke is a box-width away. The three before them are measured from the
        // painted area's own thickness, so a thin stroke can take its texture from
        // alongside itself: they only pass the overlap test when the painted area is
        // thin compared with its box, and for a dab the search is upstream's.
        let hole_count = role.iter().filter(|&&r| r == HOLE).count();
        let thickness = hole_count as f64 / size as f64 + 2.0 * ring as f64;
        const BOX_FACTORS: [f64; 5] = [1.05, 1.35, 1.75, 2.25, 2.8];
        const NEAR_FACTORS: [f64; 3] = [1.0, 1.5, 2.25];
        let near = NEAR_FACTORS.iter().enumerate().map(|(i, f)| {
            (
                1.0 / (1.0 + step * (NEAR_FACTORS.len() - i) as f64),
                f * thickness,
                f * thickness,
            )
        });
        let far = BOX_FACTORS
            .iter()
            .take(if proximity { 2 } else { 5 })
            .enumerate()
            .map(|(i, f)| (1.0 + step * i as f64, f * ww as f64, f * wh as f64));
        let mut best = f64::INFINITY;
        let (mut ox, mut oy) = (0, 0);
        for (weight, rx, ry) in near.chain(far) {
            for a in 0..24 {
                let angle = f64::from(a) * std::f64::consts::PI / 12.0;
                let dx = (angle.cos() * rx).round() as i64;
                let dy = (angle.sin() * ry).round() as i64;
                let score = scorer.score(dx, dy) * weight;
                if score < best {
                    best = score;
                    (ox, oy) = (dx, dy);
                }
            }
        }
        if best.is_finite() {
            // Fine-tune the alignment so repeating texture lines up.
            let (cx, cy) = (ox, oy);
            let mut refined = scorer.score(cx, cy);
            for j in -3..=3 {
                for i in -3..=3 {
                    let score = scorer.score(cx + i, cy + j);
                    if score < refined {
                        refined = score;
                        (ox, oy) = (cx + i, cy + j);
                    }
                }
            }
            offset = Some((oy * big_w + ox) * 4);
        }
    }
    cancelled(cancel)?;

    // Membrane: the edge difference between the original and the patch (or the original
    // itself for a smooth fill), spread across the spot.
    let mut value = vec![0f32; wn * 4];
    let mut mean = [0f64; 4];
    let mut detail = [0f64; 3];
    for p in 0..wn {
        if role[p] != RING {
            continue;
        }
        let (x, y) = (p % ww, p / ww);
        let t = at(x, y) * 4;
        let s = offset.map(|o| (t as i64 + o) as usize);
        for c in 0..4 {
            let v = f32::from(rgba[t + c]) - s.map_or(0.0, |s| f32::from(rgba[s + c]));
            value[p * 4 + c] = v;
            mean[c] += f64::from(v);
        }
        if s.is_none() {
            // Fine detail around the spot: each pixel against the average of its
            // neighbours.
            let (ix, iy) = (wx0 as usize + x, wy0 as usize + y);
            let neighbors = [
                (ix > 0).then(|| t - 4),
                (ix + 1 < width).then_some(t + 4),
                (iy > 0).then(|| t - width * 4),
                (iy + 1 < height).then_some(t + width * 4),
            ];
            for c in 0..3 {
                let (mut around, mut n) = (0.0, 0);
                for q in neighbors.into_iter().flatten() {
                    around += f64::from(rgba[q + c]);
                    n += 1;
                }
                if n > 0 {
                    let d = f64::from(rgba[t + c]) - around / f64::from(n);
                    detail[c] += d * d;
                }
            }
        }
    }
    let ring_count = ring_pixels.len() as f64;
    for m in &mut mean {
        *m /= ring_count;
    }
    for p in 0..wn {
        if role[p] == HOLE {
            for c in 0..4 {
                value[p * 4 + c] = mean[c] as f32;
            }
        }
    }
    solve(&mut value, &role, ww, wh, 0, cancel)?;
    let detail = detail.map(|d| (d / ring_count).sqrt() * 0.9);

    for p in 0..wn {
        if role[p] != HOLE {
            continue;
        }
        let (ix, iy) = (wx0 as usize + p % ww, wy0 as usize + p / ww);
        let index = iy * width + ix;
        let t = index * 4;
        let s = offset.map(|o| (t as i64 + o) as usize);
        let amount = f64::from(coverage[index]) / 255.0 * f64::from(opacity);
        let grain = if s.is_none() {
            let key = hash(
                seed ^ hash(
                    (iy as u32)
                        .wrapping_mul(width as u32)
                        .wrapping_add(ix as u32),
                ),
            );
            let (u1, u2) = (unit(key), unit(key ^ 0x68e3_1da4));
            (-2.0 * (1.0 - u1).ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        } else {
            0.0
        };
        let mut out = [0f64; 4];
        for c in 0..4 {
            let source = s.map_or(0.0, |s| f64::from(rgba[s + c]));
            let healed =
                source + f64::from(value[p * 4 + c]) + if c < 3 { grain * detail[c] } else { 0.0 };
            let original = f64::from(rgba[t + c]);
            out[c] = original + (healed - original) * amount;
        }
        let alpha = out[3].clamp(0.0, 255.0).round();
        rgba[t + 3] = alpha as u8;
        for c in 0..3 {
            rgba[t + c] = out[c].clamp(0.0, alpha).round() as u8;
        }
    }
    Ok(())
}

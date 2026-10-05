//! Gaussian mixture colour models with full covariances, as GrabCut uses them: one for
//! the foreground and one for the background, five components each.

/// Components per mixture, as in GrabCut.
pub const COMPONENTS: usize = 5;

/// Added to every variance (colour values are 0–255), so flat synthetic colours and
/// JPEG-smooth areas do not make a component singular.
const VARIANCE_FLOOR: f64 = 4.0;

#[derive(Clone, Debug)]
struct Component {
    mean: [f64; 3],
    inverse: [[f64; 3]; 3],
    /// `ln(weight) - ln(sqrt((2π)³ det Σ))`.
    log_norm: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Gmm {
    components: Vec<Component>,
}

/// Running sums for one component.
#[derive(Clone, Copy, Default)]
struct Sums {
    count: f64,
    sum: [f64; 3],
    products: [[f64; 3]; 3],
}

impl Sums {
    fn add(&mut self, c: [f32; 3]) {
        let c = c.map(f64::from);
        self.count += 1.0;
        for i in 0..3 {
            self.sum[i] += c[i];
            for j in 0..3 {
                self.products[i][j] += c[i] * c[j];
            }
        }
    }
}

impl Gmm {
    /// Learns the mixture from samples already assigned to components (`labels[i]` is
    /// the component of `samples[i]`). Empty components are dropped.
    pub fn learn(samples: &[[f32; 3]], labels: &[u8]) -> Self {
        let mut sums = [Sums::default(); COMPONENTS];
        for (sample, &label) in samples.iter().zip(labels) {
            sums[label as usize].add(*sample);
        }
        let total: f64 = sums.iter().map(|s| s.count).sum();
        let components = sums
            .iter()
            .filter(|s| s.count > 0.0)
            .map(|s| {
                let mean = s.sum.map(|v| v / s.count);
                let mut covariance = [[0.0; 3]; 3];
                for i in 0..3 {
                    for j in 0..3 {
                        covariance[i][j] = s.products[i][j] / s.count - mean[i] * mean[j];
                    }
                    covariance[i][i] += VARIANCE_FLOOR;
                }
                let (inverse, determinant) = invert(covariance);
                let weight = s.count / total;
                Component {
                    mean,
                    inverse,
                    log_norm: weight.ln()
                        - 0.5 * (determinant.ln() + 3.0 * (2.0 * std::f64::consts::PI).ln()),
                }
            })
            .collect();
        Self { components }
    }

    /// Clusters samples into [`COMPONENTS`] groups with k-means (deterministic seeding
    /// on the samples farthest apart) and learns the mixture from them.
    pub fn fit(samples: &[[f32; 3]]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let labels = kmeans(samples);
        Self::learn(samples, &labels)
    }

    fn log_component(component: &Component, c: [f32; 3]) -> f64 {
        let d = [
            f64::from(c[0]) - component.mean[0],
            f64::from(c[1]) - component.mean[1],
            f64::from(c[2]) - component.mean[2],
        ];
        let m = &component.inverse;
        let mahalanobis = d[0] * (m[0][0] * d[0] + m[0][1] * d[1] + m[0][2] * d[2])
            + d[1] * (m[1][0] * d[0] + m[1][1] * d[1] + m[1][2] * d[2])
            + d[2] * (m[2][0] * d[0] + m[2][1] * d[1] + m[2][2] * d[2]);
        component.log_norm - 0.5 * mahalanobis
    }

    /// The component that explains `c` best.
    pub fn component(&self, c: [f32; 3]) -> u8 {
        let mut best = (0, f64::NEG_INFINITY);
        for (k, component) in self.components.iter().enumerate() {
            let value = Self::log_component(component, c);
            if value > best.1 {
                best = (k, value);
            }
        }
        best.0 as u8
    }

    /// `-ln p(c)`: the data cost of `c` under this mixture.
    pub fn cost(&self, c: [f32; 3]) -> f64 {
        if self.components.is_empty() {
            return 0.0;
        }
        let logs: [f64; COMPONENTS] = std::array::from_fn(|k| {
            self.components
                .get(k)
                .map_or(f64::NEG_INFINITY, |component| {
                    Self::log_component(component, c)
                })
        });
        let max = logs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let sum: f64 = logs.iter().map(|v| (v - max).exp()).sum();
        -(max + sum.ln())
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

/// The inverse and determinant of a symmetric positive definite 3 × 3 matrix.
pub(super) fn invert(m: [[f64; 3]; 3]) -> ([[f64; 3]; 3], f64) {
    let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
    let c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
    let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
    let determinant = (m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02).max(1e-9);
    let inverse = [
        [
            c00 / determinant,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / determinant,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / determinant,
        ],
        [
            c01 / determinant,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / determinant,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / determinant,
        ],
        [
            c02 / determinant,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / determinant,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / determinant,
        ],
    ];
    (inverse, determinant)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// k-means labels for every sample. Centres are seeded farthest-first from the mean
/// (deterministic) on at most 20,000 evenly spaced samples, refined with ten Lloyd
/// iterations there, then every sample takes its nearest centre.
fn kmeans(samples: &[[f32; 3]]) -> Vec<u8> {
    let step = samples.len().div_ceil(20_000).max(1);
    let subset: Vec<[f32; 3]> = samples.iter().step_by(step).copied().collect();
    let mean = {
        let mut sum = [0.0f64; 3];
        for s in &subset {
            for c in 0..3 {
                sum[c] += f64::from(s[c]);
            }
        }
        sum.map(|v| (v / subset.len() as f64) as f32)
    };
    let mut centres: Vec<[f32; 3]> = Vec::with_capacity(COMPONENTS);
    let mut nearest: Vec<f32> = subset.iter().map(|s| distance(*s, mean)).collect();
    for _ in 0..COMPONENTS {
        let (index, &farthest) = nearest
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        if farthest <= 0.0 && !centres.is_empty() {
            break;
        }
        let centre = subset[index];
        centres.push(centre);
        for (n, s) in nearest.iter_mut().zip(&subset) {
            *n = n.min(distance(*s, centre));
        }
    }
    let assign = |s: [f32; 3], centres: &[[f32; 3]]| -> u8 {
        let mut best = (0, f32::INFINITY);
        for (k, c) in centres.iter().enumerate() {
            let d = distance(s, *c);
            if d < best.1 {
                best = (k, d);
            }
        }
        best.0 as u8
    };
    for _ in 0..10 {
        let mut sums = vec![([0.0f64; 3], 0usize); centres.len()];
        for s in &subset {
            let k = assign(*s, &centres) as usize;
            for (sum, value) in sums[k].0.iter_mut().zip(s) {
                *sum += f64::from(*value);
            }
            sums[k].1 += 1;
        }
        for (centre, (sum, count)) in centres.iter_mut().zip(sums) {
            if count > 0 {
                *centre = sum.map(|v| (v / count as f64) as f32);
            }
        }
    }
    samples.iter().map(|s| assign(*s, &centres)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mixture_prefers_its_own_colours() {
        let red: Vec<[f32; 3]> = (0..200)
            .map(|i| [200.0 + (i % 7) as f32, 20.0 + (i % 5) as f32, 20.0])
            .collect();
        let blue: Vec<[f32; 3]> = (0..200)
            .map(|i| [20.0, 30.0 + (i % 3) as f32, 210.0 - (i % 9) as f32])
            .collect();
        let mut both = red.clone();
        both.extend(&blue);
        let (red, mixed) = (Gmm::fit(&red), Gmm::fit(&both));
        assert!(red.cost([203.0, 22.0, 20.0]) < red.cost([20.0, 30.0, 205.0]));
        // Both colours are cheap under the mixture of both.
        assert!(mixed.cost([20.0, 31.0, 205.0]) < red.cost([20.0, 31.0, 205.0]));
        assert!(mixed.cost([203.0, 22.0, 20.0]).is_finite());
        // A single flat colour stays finite thanks to the variance floor.
        let flat = Gmm::fit(&[[9.0, 9.0, 9.0]; 50]);
        assert!(flat.cost([9.0, 9.0, 9.0]).is_finite());
        assert!(flat.cost([9.0, 9.0, 9.0]) < flat.cost([60.0, 9.0, 9.0]));
    }
}

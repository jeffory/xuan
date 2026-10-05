//! Film inversion in camera-linear space, before the ordinary Develop adjustments.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    DecodedRaw, DevelopSettings,
    process::{SourceMap, sample_camera_patch},
};
use crate::document::Point;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NegativeSettings {
    pub enabled: bool,
    /// Transmission through unexposed film, in normalized camera RGB.
    pub film_base: [f32; 3],
    /// Per-channel optical density range above the film base (log10 units).
    pub density_range: [f32; 3],
    pub black_point: f32,
    /// Converts normalized density into scene-linear positive light.
    pub gamma: f32,
    /// Positive-image channel compensation in stops.
    pub balance: [f32; 3],
}

impl Default for NegativeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            film_base: [1.0; 3],
            density_range: [2.0; 3],
            black_point: 0.0,
            gamma: 2.2,
            balance: [0.0; 3],
        }
    }
}

impl NegativeSettings {
    pub fn validate(&self) -> Result<()> {
        for (values, min, max) in [
            (&self.film_base[..], 0.00001, 16.0),
            (&self.density_range[..], 0.1, 6.0),
            (&self.balance[..], -3.0, 3.0),
            (&[self.black_point][..], -0.5, 0.5),
            (&[self.gamma][..], 0.5, 4.0),
        ] {
            ensure!(
                values
                    .iter()
                    .all(|v| v.is_finite() && (min..=max).contains(v)),
                "Invalid negative conversion setting"
            );
        }
        Ok(())
    }

    pub(crate) fn convert(&self, camera: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|c| {
            let density = (self.film_base[c] / camera[c].max(0.00001)).log10();
            let positive = ((density - self.black_point) / self.density_range[c]).clamp(0.0, 1.0);
            positive.powf(self.gamma) * 2.0_f32.powf(self.balance[c])
        })
    }
}

/// Estimate robust channel endpoints from the visible crop. A bounded, regular
/// grid avoids scanline aliasing and keeps analysis inexpensive on full RAW data.
/// This is a starting point: scenes without neutral extrema need manual balance.
pub fn analyze_negative(raw: &DecodedRaw, settings: &DevelopSettings) -> NegativeSettings {
    let mut channels: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::new());
    let [left, top, right, bottom] = settings.crop;
    let nx = ((right - left) * raw.camera.width() as f32)
        .ceil()
        .clamp(1.0, 256.0) as u32;
    let ny = ((bottom - top) * raw.camera.height() as f32)
        .ceil()
        .clamp(1.0, 256.0) as u32;
    let map = SourceMap::new(
        settings,
        raw.camera.width() as f32 / raw.camera.height() as f32,
    );
    for y in 0..ny {
        for x in 0..nx {
            let point = map.apply(Point::new(
                left + (x as f32 + 0.5) / nx as f32 * (right - left),
                top + (y as f32 + 0.5) / ny as f32 * (bottom - top),
            ));
            if !(0.0..1.0).contains(&point.x) || !(0.0..1.0).contains(&point.y) {
                continue;
            }
            let pixel = raw.camera.get_pixel(
                (point.x * raw.camera.width() as f32) as u32,
                (point.y * raw.camera.height() as f32) as u32,
            );
            if pixel.0.iter().all(|v| v.is_finite() && *v > 0.00001) {
                for c in 0..3 {
                    channels[c].push(pixel[c]);
                }
            }
        }
    }
    let mut result = NegativeSettings {
        enabled: true,
        ..Default::default()
    };
    for (c, values) in channels.iter_mut().enumerate() {
        if values.is_empty() {
            continue;
        }
        values.sort_unstable_by(f32::total_cmp);
        let low = values[values.len() / 100].max(0.00001);
        let high = values[(values.len() * 99 / 100).min(values.len() - 1)].clamp(0.00001, 16.0);
        result.film_base[c] = high;
        result.density_range[c] = (high / low).log10().clamp(0.1, 6.0);
    }
    result
}

/// Average a small patch of unexposed film. No white balance or camera matrix is
/// applied: all three channels must refer to the same linear signal as inversion.
pub fn sample_film_base(raw: &DecodedRaw, point: Point) -> Option<[f32; 3]> {
    // Inversion takes log10(base / signal), so only positive transmission counts.
    sample_camera_patch(raw, point, true).map(|mean| mean.map(|v| v.clamp(0.00001, 16.0)))
}

//! Dodge, Burn and Sponge: brush modes that lighten, darken or change the
//! saturation of the pixels under the brush instead of painting a colour.
//!
//! A stroke's coverage builds up in its [`super::Stroke`] as for the Brush,
//! and the effect is worked out once from each pixel's original value with
//! the strongest coverage so far, so overlapping dabs never compound. Alpha
//! is left as it is.
//!
//! The GPU stroke shader (`gpu/paint.wgsl`, `tone_pixel`) repeats these
//! formulas; keep the two in step.

use super::PaintMode;

/// Which tones Dodge and Burn change most. Sponge ignores it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToneRange {
    Shadows,
    #[default]
    Midtones,
    Highlights,
}

/// The Dodge, Burn and Sponge options a [`super::Brush`] carries; other
/// modes ignore them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub range: ToneRange,
    /// Strength, 0 to 1. At 1, full coverage on a pixel wholly in the range
    /// moves it [`REACH`] of the way to white (Dodge) or black (Burn), or
    /// doubles its saturation as far as it can go (Sponge, saturating).
    pub exposure: f32,
    /// Sponge: saturate instead of desaturate.
    pub saturate: bool,
}

impl Default for Tone {
    fn default() -> Self {
        Self {
            range: ToneRange::Midtones,
            exposure: 0.5,
            saturate: false,
        }
    }
}

/// How far one stroke at full exposure and coverage moves a pixel wholly in
/// the range towards white or black.
pub const REACH: f32 = 0.5;

/// Whether a mode changes tones rather than painting a colour.
pub fn is_tone(mode: PaintMode) -> bool {
    matches!(mode, PaintMode::Dodge | PaintMode::Burn | PaintMode::Sponge)
}

/// Luma of gamma-encoded RGB with the Rec. 709 weights, as the filters use.
pub fn luma(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much of a pixel with this luma (0 to 1) belongs to `range`, 0 to 1.
///
/// The three ranges approximate GIMP's Color Balance masks
/// (`gimp_operation_color_balance_map` in `gimpoperationcolorbalance.c`);
/// they are not an exact port, and the constants were not checked against
/// GIMP's source. Shadows fall and highlights rise over ramps a quarter
/// wide centred at one third and two thirds, and the midtones are what is
/// left between them. GIMP's ramps are linear and use HSL lightness with a
/// 0.7 scale; these are smoothstep on luma, unscaled, so the weight has no
/// corners:
///
/// ```text
/// shadows    = 1 − smoothstep(5/24, 11/24, L)
/// highlights = smoothstep(13/24, 19/24, L)
/// midtones   = 1 − shadows − highlights
/// ```
///
/// The ramps do not overlap, so the three always add up to 1: dodging all
/// three ranges in turn is the same as dodging everything once.
pub fn range_weight(range: ToneRange, luma: f32) -> f32 {
    let shadows = 1.0 - smoothstep(5.0 / 24.0, 11.0 / 24.0, luma);
    let highlights = smoothstep(13.0 / 24.0, 19.0 / 24.0, luma);
    match range {
        ToneRange::Shadows => shadows,
        ToneRange::Midtones => (1.0 - shadows - highlights).max(0.0),
        ToneRange::Highlights => highlights,
    }
}

/// `pixel` (straight RGBA, 0 to 1) after a tone `mode` with stroke
/// coverage `amount` (0 to 1).
///
/// - Dodge: `c + k·REACH·(1 − c)` and Burn: `c − k·REACH·c` on each colour
///   channel, with `k = amount · exposure · range_weight(range, luma)`.
///   The weight comes from the pixel's luma, so all channels move together
///   and the hue holds.
/// - Sponge scales each channel's distance from the luma by `1 − k`
///   (desaturate; grey at full strength) or `1 + k` (saturate; at most
///   double, and no further than the first channel reaching 0 or 1, so the
///   hue holds), with `k = amount · exposure`. The luma is unchanged.
///
/// Alpha is unchanged, and fully transparent pixels are returned as they are.
pub fn apply(pixel: [f32; 4], mode: PaintMode, tone: Tone, amount: f32) -> [f32; 4] {
    let strength = (amount * tone.exposure).clamp(0.0, 1.0);
    if pixel[3] <= 0.0 || strength <= 0.0 {
        return pixel;
    }
    let rgb = [pixel[0], pixel[1], pixel[2]];
    let y = luma(rgb);
    let out = match mode {
        PaintMode::Dodge => {
            let k = strength * range_weight(tone.range, y) * REACH;
            rgb.map(|c| c + k * (1.0 - c))
        }
        PaintMode::Burn => {
            let k = strength * range_weight(tone.range, y) * REACH;
            rgb.map(|c| c - k * c)
        }
        PaintMode::Sponge => {
            let factor = if tone.saturate {
                rgb.iter()
                    .map(|c| c - y)
                    .fold(1.0 + strength, |limit, d| {
                        if d > 1e-6 {
                            limit.min((1.0 - y) / d)
                        } else if d < -1e-6 {
                            limit.min(y / -d)
                        } else {
                            limit
                        }
                    })
                    .max(1.0)
            } else {
                1.0 - strength
            };
            rgb.map(|c| y + (c - y) * factor)
        }
        _ => rgb,
    };
    [
        out[0].clamp(0.0, 1.0),
        out[1].clamp(0.0, 1.0),
        out[2].clamp(0.0, 1.0),
        pixel[3],
    ]
}

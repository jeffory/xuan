use serde::{Deserialize, Serialize};

/// A layer's blend mode. Variants keep a fixed order (`ALL`), which is also
/// the mode's code in `composite.wgsl`; menus use `GROUPS`, Photoshop's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    Difference,
    ColorDodge,
    ColorBurn,
    Hue,
    Saturation,
    Color,
    Luminosity,
    // Added in .xuan format 7.
    Dissolve,
    LinearBurn,
    DarkerColor,
    LinearDodge,
    LighterColor,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Exclusion,
    Subtract,
    Divide,
}

impl BlendMode {
    /// Every mode in code order: the first 13 are the modes .xuan format 1–6 knew.
    pub const ALL: [Self; 27] = [
        Self::Normal,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Darken,
        Self::Lighten,
        Self::Difference,
        Self::ColorDodge,
        Self::ColorBurn,
        Self::Hue,
        Self::Saturation,
        Self::Color,
        Self::Luminosity,
        Self::Dissolve,
        Self::LinearBurn,
        Self::DarkerColor,
        Self::LinearDodge,
        Self::LighterColor,
        Self::SoftLight,
        Self::HardLight,
        Self::VividLight,
        Self::LinearLight,
        Self::PinLight,
        Self::HardMix,
        Self::Exclusion,
        Self::Subtract,
        Self::Divide,
    ];

    /// Photoshop's menu order and grouping, as upstream Compositor's
    /// `LayerBlendMode.groups` (Document/LayerAppearance.swift), plus the three
    /// modes upstream leaves out (Dissolve, Darker Color, Lighter Color) where
    /// Photoshop puts them. Menus draw a separator between groups.
    pub const GROUPS: [&'static [Self]; 6] = [
        &[Self::Normal, Self::Dissolve],
        &[
            Self::Darken,
            Self::Multiply,
            Self::ColorBurn,
            Self::LinearBurn,
            Self::DarkerColor,
        ],
        &[
            Self::Lighten,
            Self::Screen,
            Self::ColorDodge,
            Self::LinearDodge,
            Self::LighterColor,
        ],
        &[
            Self::Overlay,
            Self::SoftLight,
            Self::HardLight,
            Self::VividLight,
            Self::LinearLight,
            Self::PinLight,
            Self::HardMix,
        ],
        &[
            Self::Difference,
            Self::Exclusion,
            Self::Subtract,
            Self::Divide,
        ],
        &[Self::Hue, Self::Saturation, Self::Color, Self::Luminosity],
    ];

    /// The mode's code in `composite.wgsl`.
    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as u32
    }

    /// Whether .xuan format 6 and earlier can store this mode.
    pub fn is_legacy(self) -> bool {
        self.code() < 13
    }

    /// Menu order (`GROUPS` flattened).
    pub fn menu_order() -> impl Iterator<Item = Self> {
        Self::GROUPS.into_iter().flatten().copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Multiply => "Multiply",
            Self::Screen => "Screen",
            Self::Overlay => "Overlay",
            Self::Darken => "Darken",
            Self::Lighten => "Lighten",
            Self::Difference => "Difference",
            Self::ColorDodge => "Color Dodge",
            Self::ColorBurn => "Color Burn",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Color => "Color",
            Self::Luminosity => "Luminosity",
            Self::Dissolve => "Dissolve",
            Self::LinearBurn => "Linear Burn",
            Self::DarkerColor => "Darker Color",
            // Upstream's raw value, which its project files store.
            Self::LinearDodge => "Linear Dodge (Add)",
            Self::LighterColor => "Lighter Color",
            Self::SoftLight => "Soft Light",
            Self::HardLight => "Hard Light",
            Self::VividLight => "Vivid Light",
            Self::LinearLight => "Linear Light",
            Self::PinLight => "Pin Light",
            Self::HardMix => "Hard Mix",
            Self::Exclusion => "Exclusion",
            Self::Subtract => "Subtract",
            Self::Divide => "Divide",
        }
    }
}

/// Dissolve's per-pixel threshold in [0, 1), from a document pixel's integer
/// coordinates. `dissolve_value` in `gpu/adjustments.wgsl` is the same integer
/// hash, so the CPU and GPU keep the same pixels. The top 24 bits convert to
/// `f32` exactly on both.
pub fn dissolve_value(x: i32, y: i32) -> f32 {
    fn mix(input: u32) -> u32 {
        let value = (input ^ (input >> 16)).wrapping_mul(0x7feb_352d);
        let value = (value ^ (value >> 15)).wrapping_mul(0x846c_a68b);
        value ^ (value >> 16)
    }
    let hash =
        mix((x as u32).wrapping_mul(0x9e37_79b1) ^ mix((y as u32).wrapping_mul(0x85eb_ca77)));
    (hash >> 8) as f32 / 16_777_216.0
}

/// Dissolve draws a pixel fully opaque with probability `alpha`, or not at all.
/// The pattern is fixed to document pixels, so it scales with the zoom.
pub fn dissolve_alpha(alpha: f32, x: f32, y: f32) -> f32 {
    if dissolve_value(x.floor() as i32, y.floor() as i32) < alpha {
        1.0
    } else {
        0.0
    }
}

fn color_dodge(d: f32, s: f32) -> f32 {
    if d == 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (d / (1.0 - s)).min(1.0)
    }
}

fn color_burn(d: f32, s: f32) -> f32 {
    if d == 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - d) / s).min(1.0)
    }
}

/// One channel of a separable mode: `d` is the backdrop, `s` the layer. The
/// formulas are Photoshop's (Adobe's PDF blend modes for the W3C set; upstream
/// draws the rest through the Core Image filters of the same names, see
/// Rendering/SeparableBlend.swift). `blend()` in `gpu/adjustments.wgsl` mirrors
/// this function.
pub fn blend_channel(d: f32, s: f32, mode: BlendMode) -> f32 {
    match mode {
        BlendMode::Multiply => d * s,
        BlendMode::Screen => d + s - d * s,
        BlendMode::Overlay => {
            if d <= 0.5 {
                2.0 * d * s
            } else {
                1.0 - 2.0 * (1.0 - d) * (1.0 - s)
            }
        }
        BlendMode::Darken => d.min(s),
        BlendMode::Lighten => d.max(s),
        BlendMode::Difference => (d - s).abs(),
        BlendMode::ColorDodge => color_dodge(d, s),
        BlendMode::ColorBurn => color_burn(d, s),
        BlendMode::LinearBurn => (d + s - 1.0).max(0.0),
        BlendMode::LinearDodge => (d + s).min(1.0),
        // Photoshop's Soft Light (not the W3C variant): upstream's test
        // `softLightMatchesPhotoshop` expects 2·0.5·(1 − 0.9) + √0.5·(2·0.9 − 1).
        BlendMode::SoftLight => {
            if s <= 0.5 {
                2.0 * d * s + d * d * (1.0 - 2.0 * s)
            } else {
                2.0 * d * (1.0 - s) + d.sqrt() * (2.0 * s - 1.0)
            }
        }
        BlendMode::HardLight => {
            if s <= 0.5 {
                2.0 * d * s
            } else {
                1.0 - 2.0 * (1.0 - d) * (1.0 - s)
            }
        }
        BlendMode::VividLight => {
            if s <= 0.5 {
                color_burn(d, 2.0 * s)
            } else {
                color_dodge(d, 2.0 * s - 1.0)
            }
        }
        BlendMode::LinearLight => (d + 2.0 * s - 1.0).clamp(0.0, 1.0),
        BlendMode::PinLight => {
            if s <= 0.5 {
                d.min(2.0 * s)
            } else {
                d.max(2.0 * s - 1.0)
            }
        }
        // White where the two add up to at least one. Rounded to 8-bit levels
        // first, so the GPU's 16-bit float canvas lands on the same side.
        BlendMode::HardMix => {
            if ((d + s) * 255.0 + 0.5).floor() >= 255.0 {
                1.0
            } else {
                0.0
            }
        }
        BlendMode::Exclusion => d + s - 2.0 * d * s,
        BlendMode::Subtract => (d - s).max(0.0),
        // Dividing by black is white, except for black itself. Levels below
        // half an 8-bit step count as black, so a backdrop the GPU's 16-bit
        // float canvas rounds to zero divides the same way.
        BlendMode::Divide => {
            if s * 255.0 < 0.5 {
                if d * 255.0 >= 0.5 { 1.0 } else { 0.0 }
            } else {
                (d / s).min(1.0)
            }
        }
        _ => s,
    }
}

fn luminance(c: [f32; 3]) -> f32 {
    c[0] * 0.3 + c[1] * 0.59 + c[2] * 0.11
}
fn saturation(c: [f32; 3]) -> f32 {
    c.into_iter().fold(f32::MIN, f32::max) - c.into_iter().fold(f32::MAX, f32::min)
}

fn set_luminance(mut c: [f32; 3], value: f32) -> [f32; 3] {
    let delta = value - luminance(c);
    c = c.map(|v| v + delta);
    let min = c.into_iter().fold(f32::MAX, f32::min);
    let max = c.into_iter().fold(f32::MIN, f32::max);
    if min < 0.0 {
        c = c.map(|v| value + (v - value) * value / (value - min).max(1e-6));
    }
    if max > 1.0 {
        c = c.map(|v| value + (v - value) * (1.0 - value) / (max - value).max(1e-6));
    }
    c
}

fn set_saturation(c: [f32; 3], value: f32) -> [f32; 3] {
    let min = c.into_iter().fold(f32::MAX, f32::min);
    let max = c.into_iter().fold(f32::MIN, f32::max);
    if max <= min {
        [0.0; 3]
    } else {
        c.map(|v| (v - min) * value / (max - min))
    }
}

/// The blended color of a fully opaque backdrop `d` and layer `s`.
pub fn blend_color(d: [f32; 3], s: [f32; 3], mode: BlendMode) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_luminance(set_saturation(s, saturation(d)), luminance(d)),
        BlendMode::Saturation => set_luminance(set_saturation(d, saturation(s)), luminance(d)),
        BlendMode::Color => set_luminance(s, luminance(d)),
        BlendMode::Luminosity => set_luminance(d, luminance(s)),
        // Photoshop compares the composite brightness and keeps one whole color.
        BlendMode::DarkerColor => {
            if luminance(s) < luminance(d) {
                s
            } else {
                d
            }
        }
        BlendMode::LighterColor => {
            if luminance(s) > luminance(d) {
                s
            } else {
                d
            }
        }
        _ => std::array::from_fn(|i| blend_channel(d[i], s[i], mode)),
    }
}

/// W3C compositing formula, with straight sRGB inputs and outputs. Dissolve is
/// drawn as Normal here; the compositor first thresholds its alpha with
/// `dissolve_alpha`.
pub fn composite(dst: [f32; 4], src: [f32; 4], mode: BlendMode) -> [f32; 4] {
    let alpha = src[3] + dst[3] * (1.0 - src[3]);
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    let d = [dst[0], dst[1], dst[2]];
    let s = [src[0], src[1], src[2]];
    let mixed = blend_color(d, s, mode);
    let mut result = [0.0; 4];
    for i in 0..3 {
        result[i] = ((1.0 - src[3]) * dst[3] * dst[i]
            + (1.0 - dst[3]) * src[3] * src[i]
            + dst[3] * src[3] * mixed[i])
            / alpha;
    }
    result[3] = alpha;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_over_and_blend_on_transparency() {
        let result = composite(
            [0.0, 0.0, 1.0, 1.0],
            [1.0, 0.0, 0.0, 0.5],
            BlendMode::Normal,
        );
        assert_eq!(result, [0.5, 0.0, 0.5, 1.0]);
        for mode in BlendMode::ALL {
            let result = composite([0.0; 4], [0.2, 0.4, 0.8, 0.5], mode);
            assert_eq!(result, [0.2, 0.4, 0.8, 0.5]);
        }
    }

    fn close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "{what}: {actual} != {expected}"
        );
    }

    /// Known values from Photoshop's published formulas (and upstream's tests).
    #[test]
    fn separable_modes_match_photoshop() {
        use BlendMode::*;
        let cases: &[(BlendMode, f32, f32, f32)] = &[
            (LinearBurn, 0.6, 0.7, 0.3),
            (LinearBurn, 0.2, 0.3, 0.0),
            (LinearDodge, 0.25, 0.5, 0.75),
            (LinearDodge, 0.6, 0.7, 1.0),
            // Upstream's softLightMatchesPhotoshop: 0.666, 170 of 255.
            (SoftLight, 0.5, 0.9, 0.1 + 0.5f32.sqrt() * 0.8),
            (SoftLight, 0.5, 0.25, 0.25 + 0.125),
            (SoftLight, 0.3, 0.5, 0.3),
            (HardLight, 0.6, 0.25, 0.3),
            (HardLight, 0.4, 0.75, 0.7),
            (VividLight, 0.6, 0.25, 0.2),
            (VividLight, 0.4, 0.75, 0.8),
            (VividLight, 0.4, 0.0, 0.0),
            (VividLight, 0.4, 1.0, 1.0),
            (LinearLight, 0.5, 0.6, 0.7),
            (LinearLight, 0.2, 0.1, 0.0),
            (LinearLight, 0.9, 0.8, 1.0),
            (PinLight, 0.8, 0.25, 0.5),
            (PinLight, 0.2, 0.75, 0.5),
            (PinLight, 0.4, 0.25, 0.4),
            (HardMix, 0.5, 0.5, 1.0),
            (HardMix, 0.5, 0.4, 0.0),
            (HardMix, 128.0 / 255.0, 127.0 / 255.0, 1.0),
            (HardMix, 128.0 / 255.0, 126.0 / 255.0, 0.0),
            (Exclusion, 0.5, 0.5, 0.5),
            (Exclusion, 1.0, 0.25, 0.75),
            (Exclusion, 0.2, 0.0, 0.2),
            (Subtract, 0.6, 0.25, 0.35),
            (Subtract, 0.2, 0.5, 0.0),
            (Divide, 0.3, 0.6, 0.5),
            (Divide, 0.8, 0.4, 1.0),
            (Divide, 0.3, 0.0, 1.0),
            (Divide, 0.0, 0.0, 0.0),
        ];
        for &(mode, d, s, expected) in cases {
            close(
                blend_channel(d, s, mode),
                expected,
                &format!("{} d={d} s={s}", mode.name()),
            );
        }
        // 170 of 255, as upstream measures for Soft Light.
        let value = blend_channel(0.5, 0.9, SoftLight);
        assert_eq!((value * 255.0).round(), 170.0);
    }

    #[test]
    fn darker_and_lighter_color_keep_whole_colors() {
        let red = [1.0, 0.0, 0.0];
        let blue = [0.0, 0.0, 1.0];
        assert_eq!(blend_color(red, blue, BlendMode::DarkerColor), blue);
        assert_eq!(blend_color(blue, red, BlendMode::DarkerColor), blue);
        assert_eq!(blend_color(red, blue, BlendMode::LighterColor), red);
        assert_eq!(blend_color(blue, red, BlendMode::LighterColor), red);
        // Unlike Darken, channels are never mixed from both colors.
        assert_eq!(blend_color(red, blue, BlendMode::Darken), [0.0; 3]);
    }

    #[test]
    fn dissolve_keeps_a_deterministic_share_of_pixels() {
        assert_eq!(dissolve_alpha(1.0, 3.5, 7.5), 1.0);
        assert_eq!(dissolve_alpha(0.0, 3.5, 7.5), 0.0);
        let kept = (0..100)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .filter(|&(x, y)| dissolve_alpha(0.3, x as f32 + 0.5, y as f32 + 0.5) > 0.0)
            .count();
        assert!((2700..3300).contains(&kept), "{kept}");
        // Fixed values, shared with the shader's copy of the hash.
        assert_eq!(dissolve_value(0, 0), 0.0);
        assert_eq!(dissolve_value(-3, 5), dissolve_value(-3, 5));
        assert_ne!(dissolve_value(1, 0), dissolve_value(0, 1));
    }

    #[test]
    fn menu_groups_follow_photoshop_and_cover_every_mode() {
        let menu: Vec<_> = BlendMode::menu_order().collect();
        assert_eq!(menu.len(), BlendMode::ALL.len());
        for mode in BlendMode::ALL {
            assert!(menu.contains(&mode), "{}", mode.name());
        }
        let names: Vec<_> = menu.iter().map(|m| m.name()).collect();
        assert_eq!(
            names[..8],
            [
                "Normal",
                "Dissolve",
                "Darken",
                "Multiply",
                "Color Burn",
                "Linear Burn",
                "Darker Color",
                "Lighten"
            ]
        );
        assert_eq!(BlendMode::Luminosity.code(), 12);
        assert!(BlendMode::Luminosity.is_legacy());
        assert!(!BlendMode::Dissolve.is_legacy());
    }
}

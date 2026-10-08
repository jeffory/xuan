//! Filter → Dither, ported from upstream Compositor (`Document/Dither.swift` for the settings
//! and the chunky-pixel steps, `Rendering/DitherPixels.c` for the pixel loops). Colors are
//! straight sRGB; alpha is kept and fully transparent pixels are left alone. The ordered,
//! halftone, pattern and scanline styles also run on the GPU (`dither.wgsl`); error diffusion,
//! ASCII and the scanlines' glow are CPU only.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Result, ensure};
use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::within;

/// The looks, in the order upstream's panel lists them (and `DitherPixels.h` numbers them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DitherStyle {
    #[default]
    Atkinson,
    FloydSteinberg,
    Bayer2,
    Bayer4,
    Bayer8,
    HalftoneDots,
    HalftoneLines,
    HalftoneDiamonds,
    MacPatterns,
    Ascii,
    Scanlines,
}

impl DitherStyle {
    pub const ALL: [Self; 11] = [
        Self::Atkinson,
        Self::FloydSteinberg,
        Self::Bayer2,
        Self::Bayer4,
        Self::Bayer8,
        Self::HalftoneDots,
        Self::HalftoneLines,
        Self::HalftoneDiamonds,
        Self::MacPatterns,
        Self::Ascii,
        Self::Scanlines,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Atkinson => "Atkinson (Classic Mac)",
            Self::FloydSteinberg => "Floyd–Steinberg",
            Self::Bayer2 => "Bayer 2 × 2",
            Self::Bayer4 => "Bayer 4 × 4",
            Self::Bayer8 => "Bayer 8 × 8",
            Self::HalftoneDots => "Halftone Dots",
            Self::HalftoneLines => "Halftone Lines",
            Self::HalftoneDiamonds => "Halftone Diamonds",
            Self::MacPatterns => "Mac Patterns",
            Self::Ascii => "ASCII",
            Self::Scanlines => "Scanlines (CRT)",
        }
    }

    /// Upstream's style code, also the GPU's.
    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|s| *s == self).unwrap() as u32
    }

    /// Error diffusion: each pixel's rounding error is passed to its neighbors.
    pub fn diffuses(self) -> bool {
        matches!(self, Self::Atkinson | Self::FloydSteinberg)
    }

    /// Diffusion and ordered styles quantize to a number of tones; the rest draw marks in two.
    pub fn has_tones(self) -> bool {
        self.code() <= Self::Bayer8.code()
    }

    pub fn is_halftone(self) -> bool {
        matches!(
            self,
            Self::HalftoneDots | Self::HalftoneLines | Self::HalftoneDiamonds
        )
    }

    /// Halftone shapes, patterns and characters mark one tone on the other.
    pub fn draws_marks(self) -> bool {
        !self.has_tones() && self != Self::Scanlines
    }

    /// ASCII's characters and a CRT's lines are drawn at full resolution, not in chunky pixels.
    pub fn uses_pixel_size(self) -> bool {
        self != Self::Ascii && self != Self::Scanlines
    }
}

/// How a chunky pixel is drawn: a solid square, or a round dot with the dark color around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DitherPixelShape {
    #[default]
    Square,
    Dot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DitherColors {
    #[default]
    BlackWhite,
    TwoColors,
    Original,
}

/// Upstream's `DitherSettings`, with its defaults. Missing fields take those defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DitherSettings {
    pub style: DitherStyle,
    /// Each dithered pixel covers this many layer pixels on a side, 1–32.
    pub pixel_size: f32,
    pub pixel_shape: DitherPixelShape,
    /// Halftone screen cells, in dithered pixels, 4–64.
    pub cell_size: f32,
    /// ASCII's line height in layer pixels, 6–64.
    pub text_size: f32,
    /// Scanlines: the distance between lines in layer pixels, 2–32.
    pub line_spacing: f32,
    /// Scanlines: light blooming around the lines and how far they break into dots (0–100%),
    /// and how far they waver sideways (0–64 px).
    pub glow: f32,
    pub dots: f32,
    pub wobble: f32,
    /// Halftone screen angle in degrees, −90–90.
    pub angle: f32,
    /// Tones per channel for diffusion and ordered styles, 2–8; 2 is 1-bit.
    pub levels: f32,
    /// How much of the error diffusion passes on, 0–100%.
    pub diffusion: f32,
    /// −100–100: more ink (darker) or less, and flatter or punchier, before dithering.
    pub density: f32,
    pub contrast: f32,
    pub colors: DitherColors,
    pub dark: [u8; 3],
    pub light: [u8; 3],
    /// Marks stand for the light tones, drawn in the light color on the dark.
    pub light_on_dark: bool,
    /// ASCII's characters, any order: they are sorted by how much ink each one has.
    pub characters: String,
}

impl Default for DitherSettings {
    fn default() -> Self {
        Self {
            style: DitherStyle::Atkinson,
            pixel_size: 2.0,
            pixel_shape: DitherPixelShape::Square,
            cell_size: 8.0,
            text_size: 14.0,
            line_spacing: 4.0,
            glow: 35.0,
            dots: 0.0,
            wobble: 0.0,
            angle: 45.0,
            levels: 2.0,
            diffusion: 100.0,
            density: 0.0,
            contrast: 0.0,
            colors: DitherColors::BlackWhite,
            dark: [0; 3],
            light: [255; 3],
            light_on_dark: true,
            characters: DEFAULT_CHARACTERS.into(),
        }
    }
}

pub const DEFAULT_CHARACTERS: &str = " .:-=+*#%@";

impl DitherSettings {
    pub fn validate(&self) -> Result<()> {
        within("Dither.pixel_size", self.pixel_size, 1.0, 32.0)?;
        within("Dither.cell_size", self.cell_size, 4.0, 64.0)?;
        within("Dither.text_size", self.text_size, 6.0, 64.0)?;
        within("Dither.line_spacing", self.line_spacing, 2.0, 32.0)?;
        within("Dither.glow", self.glow, 0.0, 100.0)?;
        within("Dither.dots", self.dots, 0.0, 100.0)?;
        within("Dither.wobble", self.wobble, 0.0, 64.0)?;
        within("Dither.angle", self.angle, -90.0, 90.0)?;
        within("Dither.levels", self.levels, 2.0, 8.0)?;
        within("Dither.diffusion", self.diffusion, 0.0, 100.0)?;
        within("Dither.density", self.density, -100.0, 100.0)?;
        within("Dither.contrast", self.contrast, -100.0, 100.0)?;
        ensure!(
            self.characters.chars().count() <= 64 && !self.characters.contains(['\n', '\r']),
            "`Dither.characters` must be at most 64 characters on one line"
        );
        Ok(())
    }

    /// Sizes in layer pixels at `scale` (a smaller preview), kept whole and at least 1. Halftone
    /// cells are counted in chunky pixels, so they grow back by what rounding the pixel size
    /// took away.
    pub fn scaled(&self, scale: f32) -> Self {
        let size = |v: f32| (v * scale).round().max(1.0);
        let pixel_size = size(self.pixel_size);
        Self {
            pixel_size,
            cell_size: self.cell_size * self.pixel_size * scale / pixel_size,
            text_size: size(self.text_size),
            line_spacing: size(self.line_spacing),
            wobble: self.wobble * scale,
            ..self.clone()
        }
    }

    /// Whether the GPU can draw this: everything but error diffusion, ASCII, and scanlines'
    /// glow, which need a whole-image pass or glyphs.
    pub fn runs_on_gpu(&self) -> bool {
        !(self.style.diffuses()
            || self.style == DitherStyle::Ascii
            || (self.style == DitherStyle::Scanlines && self.glow > 0.0))
    }

    /// The chunky pixel side in layer pixels.
    pub fn block(&self) -> u32 {
        if self.style.uses_pixel_size() {
            self.pixel_size.round().max(1.0) as u32
        } else {
            1
        }
    }

    /// Density as a gamma (positive darkens) and contrast as a slope about mid gray.
    pub fn tone_curve(&self) -> (f32, f32) {
        let density = self.density / 100.0;
        let contrast = self.contrast / 100.0;
        (
            (density * 1.5).exp2(),
            if contrast >= 0.0 {
                1.0 / (1.0 - 0.95 * contrast)
            } else {
                1.0 + contrast
            },
        )
    }

    /// The dark and light colors the result is made of (black and white unless Two Colors).
    pub fn palette(&self) -> ([f32; 3], [f32; 3]) {
        if self.colors == DitherColors::TwoColors {
            (
                self.dark.map(|v| v as f32 / 255.0),
                self.light.map(|v| v as f32 / 255.0),
            )
        } else {
            ([0.0; 3], [1.0; 3])
        }
    }
}

/// Upstream's `adjust_tone`: density bends the tone as a gamma, so black and white stay put;
/// contrast pivots on mid gray.
pub fn adjust_tone(value: f32, gamma: f32, contrast: f32) -> f32 {
    ((value.clamp(0.0, 1.0).powf(gamma) - 0.5) * contrast + 0.5).clamp(0.0, 1.0)
}

fn quantize(value: f32, levels: u32) -> f32 {
    let steps = (levels - 1) as f32;
    (value.clamp(0.0, 1.0) * steps).round() / steps
}

const BAYER8: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60,
    28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15,
    47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

/// Upstream's `ordered_threshold`, in [0, 1).
pub fn ordered_threshold(style: DitherStyle, x: u32, y: u32) -> f32 {
    match style {
        DitherStyle::Bayer2 => {
            const M: [u8; 4] = [0, 2, 3, 1];
            (M[((y & 1) * 2 + (x & 1)) as usize] as f32 + 0.5) / 4.0
        }
        DitherStyle::Bayer4 => {
            const M: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
            (M[((y & 3) * 4 + (x & 3)) as usize] as f32 + 0.5) / 16.0
        }
        _ => (BAYER8[((y & 7) * 8 + (x & 7)) as usize] as f32 + 0.5) / 64.0,
    }
}

/// Upstream's `ordered`: the tone rounded down to one of `levels` after adding the threshold.
pub fn ordered(value: f32, threshold: f32, levels: u32) -> f32 {
    let steps = (levels - 1) as f32;
    (value.clamp(0.0, 1.0) * steps + threshold)
        .floor()
        .min(steps)
        / steps
}

/// Upstream's `spot`: how covered a halftone cell must be before the point at `u`, `v`
/// (−0.5–0.5 across the cell) is marked.
pub fn spot(style: DitherStyle, u: f32, v: f32) -> f32 {
    match style {
        DitherStyle::HalftoneDots => std::f32::consts::PI * (u * u + v * v),
        DitherStyle::HalftoneLines => v.abs() * 2.0,
        _ => u.abs() + v.abs(),
    }
}

/// Old Mac fill patterns, 8 × 8, one byte per row with the leftmost pixel in the top bit,
/// from sparsest to fullest.
pub const PATTERNS: [[u8; 8]; 17] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x80, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00],
    [0x88, 0x00, 0x22, 0x00, 0x88, 0x00, 0x22, 0x00],
    [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01],
    [0x88, 0x22, 0x88, 0x22, 0x88, 0x22, 0x88, 0x22],
    [0x00, 0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00],
    [0x11, 0x22, 0x44, 0x88, 0x11, 0x22, 0x44, 0x88],
    [0xAA, 0x00, 0xAA, 0x00, 0xAA, 0x00, 0xAA, 0x00],
    [0x88, 0x55, 0x22, 0x55, 0x88, 0x55, 0x22, 0x55],
    [0xFF, 0x80, 0x80, 0x80, 0xFF, 0x08, 0x08, 0x08],
    [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
    [0x81, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x81],
    [0x77, 0xAA, 0xDD, 0xAA, 0x77, 0xAA, 0xDD, 0xAA],
    [0xEE, 0xDD, 0xBB, 0x77, 0xEE, 0xDD, 0xBB, 0x77],
    [0x77, 0xFF, 0xDD, 0xFF, 0x77, 0xFF, 0xDD, 0xFF],
    [0x7F, 0xFF, 0xFF, 0xFF, 0xF7, 0xFF, 0xFF, 0xFF],
    [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
];

/// Whether the Mac pattern for `coverage` (0–1) marks the pixel at `x`, `y`.
pub fn pattern_mark(coverage: f32, x: u32, y: u32) -> bool {
    let index = (coverage * (PATTERNS.len() - 1) as f32).round() as usize;
    (PATTERNS[index.min(PATTERNS.len() - 1)][(y & 7) as usize] >> (7 - (x & 7))) & 1 == 1
}

/// How much a halftone mark covers the pixel at `x`, `y` (0 or 1) for tone `t`.
pub fn halftone_mark(style: DitherStyle, t: f32, x: u32, y: u32, cell: f32, angle: f32) -> f32 {
    let (sin, cos) = angle.sin_cos();
    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
    let mut u = (fx * cos + fy * sin) / cell;
    let mut v = (-fx * sin + fy * cos) / cell;
    u -= u.floor() + 0.5;
    v -= v.floor() + 0.5;
    if t > spot(style, u, v) { 1.0 } else { 0.0 }
}

/// Upstream's scanline: half the beam's height, thin in the shadows and most of the way across
/// in the highlights, and how much of the pixel `offset` from the line's middle (and `across`
/// it, for dots) it covers.
pub fn beam_cover(t: f32, middle: f32, offset: f32, across: f32) -> f32 {
    let beam = middle * (0.2 + 0.5 * t.clamp(0.0, 1.0).sqrt());
    let distance = (offset * offset + across * across).sqrt();
    (beam - distance + 0.5).clamp(0.0, 1.0)
}

/// Upstream's wobble: a slow wave down the screen with a quicker one over it.
pub fn wobble_shift(line: u32, wobble: f32) -> i64 {
    let line = line as f32;
    let wave = (line * 0.45).sin() * 0.7 + (line * 1.7 + 1.3).sin() * 0.3;
    (wobble * wave).round() as i64
}

/// One error-diffusion tap: neighbors to the right on this row and below, and its weight.
const ATKINSON: [(i64, i64, f32); 6] = [
    (1, 0, 1.0),
    (2, 0, 1.0),
    (-1, 1, 1.0),
    (0, 1, 1.0),
    (1, 1, 1.0),
    (0, 2, 1.0),
];
const FLOYD: [(i64, i64, f32); 4] = [(1, 0, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)];

/// Upstream's `diffuse`, in serpentine order so the error's drift doesn't streak to one side.
/// Atkinson passes on only six eighths of the error, which gives the Mac's crisp look.
fn diffuse(plane: &mut [f32], alpha: &[u8], width: usize, settings: &DitherSettings) {
    let (taps, divisor): (&[(i64, i64, f32)], f32) = if settings.style == DitherStyle::Atkinson {
        (&ATKINSON, 8.0)
    } else {
        (&FLOYD, 16.0)
    };
    let levels = settings.levels.round().clamp(2.0, 16.0) as u32;
    let diffusion = settings.diffusion / 100.0;
    let height = plane.len() / width.max(1);
    for y in 0..height {
        let reverse = y & 1 == 1;
        for i in 0..width {
            let x = if reverse { width - 1 - i } else { i };
            let at = y * width + x;
            if alpha[at] == 0 {
                continue;
            }
            let old = plane[at];
            let q = quantize(old, levels);
            plane[at] = q;
            let error = (old - q) * diffusion / divisor;
            for &(dx, dy, weight) in taps {
                let nx = x as i64 + if reverse { -dx } else { dx };
                let ny = y as i64 + dy;
                if nx < 0 || nx >= width as i64 || ny >= height as i64 {
                    continue;
                }
                plane[ny as usize * width + nx as usize] += error * weight;
            }
        }
    }
}

/// Characters drawn as coverage maps of one cell, from least ink to most (see `glyphs`).
pub struct Glyphs {
    pub width: u32,
    pub height: u32,
    pub maps: Vec<Vec<u8>>,
    pub coverage: Vec<f32>,
}

/// Each distinct character of `characters` drawn into a cell of monospaced text `line_height`
/// tall and one character wide, sorted from least ink to most. Upstream draws the system's bold
/// monospaced font at line height / 1.2; Xuan uses the Hack font bundled with the interface, so
/// ASCII looks the same on every platform. Results are kept for the next preview.
pub fn glyphs(characters: &str, line_height: u32) -> Arc<Glyphs> {
    type Cache = Mutex<HashMap<(String, u32), Arc<Glyphs>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let key = (characters.to_owned(), line_height);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().unwrap().get(&key) {
        return found.clone();
    }
    let made = Arc::new(draw_glyphs(characters, line_height));
    let mut cache = cache.lock().unwrap();
    if cache.len() > 16 {
        cache.clear();
    }
    cache.insert(key, made.clone());
    made
}

fn draw_glyphs(characters: &str, line_height: u32) -> Glyphs {
    use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
    let hack = egui::FontDefinitions::default()
        .font_data
        .get("Hack")
        .map(|data| data.font.to_vec())
        .unwrap_or_default();
    let mut fonts = FontSystem::new_with_locale_and_db("en-US".into(), {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(hack);
        db
    });
    let size = line_height as f32 / 1.2;
    let height = line_height.max(1);
    let attrs = Attrs::new().family(Family::Name("Hack"));
    let shape = |fonts: &mut FontSystem, text: &str| {
        let mut buffer = Buffer::new(fonts, Metrics::new(size, height as f32));
        buffer.set_size(fonts, None, None);
        buffer.set_text(fonts, text, &attrs, Shaping::Advanced);
        buffer
    };
    let width = shape(&mut fonts, "M")
        .layout_runs()
        .next()
        .map_or(1.0, |run| run.line_w)
        .round()
        .max(1.0) as u32;
    let mut cache = SwashCache::new();
    let mut seen = std::collections::HashSet::new();
    let mut drawn: Vec<(Vec<u8>, f32)> = Vec::new();
    for character in characters.chars().filter(|c| seen.insert(*c)) {
        let mut map = vec![0_u8; (width * height) as usize];
        let buffer = shape(&mut fonts, &character.to_string());
        for run in buffer.layout_runs() {
            let left = ((width as f32 - run.line_w) / 2.0).round() as i32;
            for glyph in run.glyphs {
                let physical = glyph.physical((left as f32, run.line_y), 1.0);
                cache.with_pixels(
                    &mut fonts,
                    physical.cache_key,
                    Color::rgb(255, 255, 255),
                    |x, y, color| {
                        let (px, py) = (physical.x + x, physical.y + y);
                        if (0..width as i32).contains(&px) && (0..height as i32).contains(&py) {
                            let at = (py as u32 * width + px as u32) as usize;
                            map[at] = map[at].max(color.a());
                        }
                    },
                );
            }
        }
        let ink = map.iter().map(|&v| v as f32).sum::<f32>() / (255.0 * map.len() as f32);
        drawn.push((map, ink));
    }
    drawn.sort_by(|a, b| a.1.total_cmp(&b.1));
    Glyphs {
        width,
        height,
        coverage: drawn.iter().map(|d| d.1).collect(),
        maps: drawn.into_iter().map(|d| d.0).collect(),
    }
}

/// Upstream's `dither_apply` on straight pixels, in place.
pub fn dither_pixels(image: &mut RgbaImage, settings: &DitherSettings) {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let count = width * height;
    if count == 0 {
        return;
    }
    let original = settings.colors == DitherColors::Original;
    let planes = if original { 3 } else { 1 };
    let (gamma, contrast) = settings.tone_curve();
    let alpha: Vec<u8> = image.pixels().map(|p| p[3]).collect();
    // The image's own colors, unadjusted: marks take them in Original mode.
    let source: Vec<[f32; 3]> = image
        .pixels()
        .map(|p| {
            if p[3] == 0 {
                [0.0; 3]
            } else {
                [p[0], p[1], p[2]].map(|v| v as f32 / 255.0)
            }
        })
        .collect();
    let mut tone = vec![0.0_f32; count * planes];
    for (at, c) in source.iter().enumerate() {
        if original {
            for p in 0..3 {
                tone[p * count + at] = adjust_tone(c[p], gamma, contrast);
            }
        } else {
            tone[at] = adjust_tone(super::stylize::rec709(*c), gamma, contrast);
        }
    }
    let (dark, light) = settings.palette();
    let levels = settings.levels.round().clamp(2.0, 16.0) as u32;
    let style = settings.style;
    let pixels = image.as_mut();
    let write = |pixels: &mut [u8], at: usize, rgb: [f32; 3]| {
        for c in 0..3 {
            pixels[at * 4 + c] = (rgb[c].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    };
    if style.has_tones() {
        for plane in tone.chunks_exact_mut(count) {
            if style.diffuses() {
                diffuse(plane, &alpha, width, settings);
            } else {
                for (at, value) in plane.iter_mut().enumerate() {
                    if alpha[at] != 0 {
                        let (x, y) = ((at % width) as u32, (at / width) as u32);
                        *value = ordered(*value, ordered_threshold(style, x, y), levels);
                    }
                }
            }
        }
        for at in (0..count).filter(|&at| alpha[at] != 0) {
            let rgb = if original {
                [tone[at], tone[count + at], tone[2 * count + at]]
            } else {
                let t = tone[at];
                std::array::from_fn(|c| dark[c] + (light[c] - dark[c]) * t)
            };
            write(pixels, at, rgb);
        }
        return;
    }
    if style == DitherStyle::Scanlines {
        // A CRT: each line scans the image, its tone along the line the average of the rows it
        // covers. The beam glows brighter and blooms thicker where the picture is light.
        let spacing = (settings.line_spacing.round() as usize).max(2);
        let middle = spacing as f32 / 2.0;
        let dots = (settings.dots / 100.0).clamp(0.0, 1.0);
        let lines: Vec<_> = (0..height.div_ceil(spacing))
            .into_par_iter()
            .map(|line| {
                let top = line * spacing;
                let bottom = (top + spacing).min(height);
                let shift = wobble_shift(line as u32, settings.wobble);
                let mut scan = vec![0.0_f32; width * planes];
                for x in 0..width {
                    let sx = x as i64 - shift;
                    let mut sum = [0.0; 3];
                    let mut n = 0;
                    if (0..width as i64).contains(&sx) {
                        for y in top..bottom {
                            let at = y * width + sx as usize;
                            if alpha[at] == 0 {
                                continue;
                            }
                            for (c, total) in sum.iter_mut().enumerate().take(planes) {
                                *total += tone[c * count + at];
                            }
                            n += 1;
                        }
                    }
                    for c in 0..planes {
                        scan[c * width + x] = if n > 0 { sum[c] / n as f32 } else { 0.0 };
                    }
                }
                scan
            })
            .collect();
        for at in (0..count).filter(|&at| alpha[at] != 0) {
            let (x, y) = (at % width, at / width);
            let (line, top) = (y / spacing, y / spacing * spacing);
            let scan = &lines[line];
            let offset = ((y - top) as f32 + 0.5 - middle).abs();
            // Dots: the line breaks into beads, each lit in the color at its middle.
            let along = (x as f32 + 0.5).rem_euclid(spacing as f32) - middle;
            let centered = (x as f32 - along * dots)
                .round()
                .clamp(0.0, width as f32 - 1.0) as usize;
            let (rgb, t) = if original {
                let rgb = [
                    scan[centered],
                    scan[width + centered],
                    scan[2 * width + centered],
                ];
                (rgb, super::stylize::rec709(rgb))
            } else {
                let t = scan[centered];
                (
                    std::array::from_fn(|c| dark[c] + (light[c] - dark[c]) * t),
                    t,
                )
            };
            // The beam is driven brighter than the picture, making up for the dark between.
            let rgb = rgb.map(|v| v * 1.35);
            let cover = beam_cover(t, middle, offset, along * dots);
            let screen = if original { [0.0; 3] } else { dark };
            write(
                pixels,
                at,
                std::array::from_fn(|c| screen[c] + (rgb[c] - screen[c]) * cover),
            );
        }
        return;
    }
    // Marks cover as much of each spot as the tone calls for. On light, they stand for darkness
    // and are drawn in the dark color; light on dark, the reverse.
    let marks: Vec<f32> = if original {
        (0..count)
            .map(|i| super::stylize::rec709([tone[i], tone[count + i], tone[2 * count + i]]))
            .collect()
    } else {
        tone[..count].to_vec()
    };
    let light_on_dark = settings.light_on_dark;
    let (ink, paper) = if light_on_dark {
        (light, dark)
    } else {
        (dark, light)
    };
    let cell = settings.cell_size.max(2.0);
    let angle = settings.angle.to_radians();
    let glyphs = (style == DitherStyle::Ascii).then(|| {
        let characters = if settings.characters.is_empty() {
            DEFAULT_CHARACTERS
        } else {
            &settings.characters
        };
        glyphs(characters, settings.text_size.round().max(1.0) as u32)
    });
    // ASCII: each cell shares one character, picked from the cell's average tone.
    let picked = glyphs.as_ref().filter(|g| !g.maps.is_empty()).map(|g| {
        let (gw, gh) = (g.width as usize, g.height as usize);
        let columns = width.div_ceil(gw);
        let rows = height.div_ceil(gh);
        let most = *g.coverage.last().unwrap();
        (0..rows * columns)
            .map(|cell| {
                let (row, column) = (cell / columns, cell % columns);
                let (mut sum, mut n) = (0.0, 0);
                for y in row * gh..((row + 1) * gh).min(height) {
                    for x in column * gw..((column + 1) * gw).min(width) {
                        if alpha[y * width + x] != 0 {
                            sum += marks[y * width + x];
                            n += 1;
                        }
                    }
                }
                let t = if n > 0 { sum / n as f32 } else { 1.0 };
                let wanted = if light_on_dark { t } else { 1.0 - t } * most;
                let mut best = 0;
                let mut best_distance = 2.0;
                for (index, coverage) in g.coverage.iter().enumerate() {
                    let distance = (coverage - wanted).abs();
                    if distance < best_distance {
                        best_distance = distance;
                        best = index;
                    }
                }
                best
            })
            .collect::<Vec<_>>()
    });
    // Original colors: marks take the pixel's own color, on black (light on dark) or white.
    let paper_original = if light_on_dark { 0.0 } else { 1.0 };
    for at in (0..count).filter(|&at| alpha[at] != 0) {
        let (x, y) = ((at % width) as u32, (at / width) as u32);
        let t = marks[at];
        let coverage = if light_on_dark { t } else { 1.0 - t };
        let amount = if let (Some(picked), Some(g)) = (&picked, &glyphs) {
            let columns = width.div_ceil(g.width as usize);
            let glyph = picked[(y / g.height) as usize * columns + (x / g.width) as usize];
            g.maps[glyph][((y % g.height) * g.width + x % g.width) as usize] as f32 / 255.0
        } else if style == DitherStyle::MacPatterns {
            f32::from(u8::from(pattern_mark(coverage, x, y)))
        } else if style.is_halftone() {
            halftone_mark(style, coverage, x, y, cell, angle)
        } else {
            // ASCII without characters to draw.
            0.0
        };
        let rgb = if original {
            source[at].map(|s| paper_original + (s - paper_original) * amount)
        } else {
            std::array::from_fn(|c| paper[c] + (ink[c] - paper[c]) * amount)
        };
        write(pixels, at, rgb);
    }
}

/// Averages each `block` × `block` square (what there is of it at the edges), weighting color
/// by coverage. Upstream draws the image at 1 / block with high-quality interpolation.
pub fn block_average(image: &RgbaImage, block: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    RgbaImage::from_fn(width.div_ceil(block), height.div_ceil(block), |bx, by| {
        let mut sum = [0.0_f32; 4];
        let mut n = 0.0;
        for y in by * block..((by + 1) * block).min(height) {
            for x in bx * block..((bx + 1) * block).min(width) {
                let p = image.get_pixel(x, y);
                let a = p[3] as f32;
                for c in 0..3 {
                    sum[c] += p[c] as f32 * a;
                }
                sum[3] += a;
                n += 1.0;
            }
        }
        if sum[3] <= 0.0 {
            return Rgba([0; 4]);
        }
        Rgba([
            (sum[0] / sum[3]).round() as u8,
            (sum[1] / sum[3]).round() as u8,
            (sum[2] / sum[3]).round() as u8,
            (sum[3] / n).round() as u8,
        ])
    })
}

/// Upstream's `dither_dots`: how much of a `block`-sized chunky pixel a round dot covers at
/// `x`, `y`; the rest shows the gap color.
pub fn dot_cover(x: u32, y: u32, block: u32) -> f32 {
    let radius = block as f32 * 0.42;
    let middle = block as f32 / 2.0;
    let dx = (x % block) as f32 + 0.5 - middle;
    let dy = (y % block) as f32 + 0.5 - middle;
    (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
}

/// Filter → Dither on straight pixels: chunky pixels are dithered on a copy averaged down by
/// the pixel size and blown back up without smoothing; scanlines may glow.
pub fn dither(image: &RgbaImage, settings: &DitherSettings) -> RgbaImage {
    let block = settings.block();
    let mut working = if block > 1 {
        block_average(image, block)
    } else {
        image.clone()
    };
    dither_pixels(&mut working, settings);
    if settings.style == DitherStyle::Scanlines && settings.glow > 0.0 {
        return glowing(&working, settings);
    }
    if block == 1 {
        return working;
    }
    let gap = if settings.colors == DitherColors::TwoColors {
        settings.dark.map(|v| v as f32)
    } else {
        [0.0; 3]
    };
    let dots = settings.pixel_shape == DitherPixelShape::Dot && block >= 2;
    RgbaImage::from_fn(image.width(), image.height(), |x, y| {
        let mut p = *working.get_pixel(x / block, y / block);
        if dots && p[3] > 0 {
            let cover = dot_cover(x, y, block);
            for c in 0..3 {
                p[c] = (p[c] as f32 * cover + gap[c] * (1.0 - cover)).round() as u8;
            }
        }
        p
    })
}

/// Upstream's `glowing` and `dither_glow`: the lines' light, blurred across a few line spacings
/// and added back over them, never past each pixel's own coverage.
fn glowing(image: &RgbaImage, settings: &DitherSettings) -> RgbaImage {
    let sigma = settings.line_spacing.round().max(2.0) * 3.0 + 3.0;
    let light = super::gaussian_blurred(image, sigma);
    let amount = settings.glow / 100.0 * 2.5;
    let mut result = image.clone();
    for (pixel, glow) in result.pixels_mut().zip(light.pixels()) {
        let a = pixel[3] as f32 / 255.0;
        if a <= 0.0 {
            continue;
        }
        for c in 0..3 {
            let premultiplied = pixel[c] as f32 / 255.0 * a;
            let added = glow[c] as f32 / 255.0 * glow[3] as f32 / 255.0 * amount * a;
            pixel[c] = ((premultiplied + added).min(a) / a * 255.0).round() as u8;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(style: DitherStyle) -> DitherSettings {
        DitherSettings {
            style,
            pixel_size: 1.0,
            ..DitherSettings::default()
        }
    }

    fn gray(width: u32, height: u32, value: u8) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba([value, value, value, 255]))
    }

    fn mean(image: &RgbaImage) -> f32 {
        image.pixels().map(|p| p[0] as f32).sum::<f32>() / (image.len() / 4) as f32 / 255.0
    }

    #[test]
    fn tone_curve_and_thresholds_follow_upstream() {
        let neutral = DitherSettings::default().tone_curve();
        assert_eq!(neutral, (1.0, 1.0));
        assert_eq!(adjust_tone(0.3, 1.0, 1.0), 0.3);
        // Density 100 is a gamma of 2^1.5: black and white stay, mid gray darkens.
        let dense = DitherSettings {
            density: 100.0,
            contrast: 100.0,
            ..Default::default()
        };
        let (gamma, contrast) = dense.tone_curve();
        assert!((gamma - 2.828_427).abs() < 1e-4);
        assert!((contrast - 20.0).abs() < 1e-3);
        assert_eq!(adjust_tone(1.0, gamma, 1.0), 1.0);
        assert!((adjust_tone(0.5, gamma, 1.0) - 0.5f32.powf(gamma)).abs() < 1e-6);
        let flat = DitherSettings {
            contrast: -100.0,
            ..Default::default()
        };
        assert_eq!(adjust_tone(0.9, 1.0, flat.tone_curve().1), 0.5);
        // Bayer thresholds are the matrix's ranks, centered in their step.
        assert_eq!(ordered_threshold(DitherStyle::Bayer2, 0, 0), 0.125);
        assert_eq!(ordered_threshold(DitherStyle::Bayer2, 1, 1), 0.375);
        assert_eq!(ordered_threshold(DitherStyle::Bayer4, 1, 0), 8.5 / 16.0);
        assert_eq!(ordered_threshold(DitherStyle::Bayer8, 7, 7), 21.5 / 64.0);
        assert_eq!(ordered_threshold(DitherStyle::Bayer8, 9, 8), 32.5 / 64.0);
        assert_eq!(ordered(0.5, 0.49, 2), 0.0);
        assert_eq!(ordered(0.5, 0.5, 2), 1.0);
        assert_eq!(ordered(1.0, 0.99, 3), 1.0);
        assert_eq!(ordered(0.4, 0.3, 3), 0.5);
        // Halftone spots: a dot grows from the middle; lines are horizontal bands.
        assert_eq!(spot(DitherStyle::HalftoneDots, 0.0, 0.0), 0.0);
        assert!(
            (spot(DitherStyle::HalftoneDots, 0.5, 0.0) - std::f32::consts::FRAC_PI_4).abs() < 1e-6
        );
        assert_eq!(spot(DitherStyle::HalftoneLines, 0.4, 0.25), 0.5);
        assert_eq!(spot(DitherStyle::HalftoneDiamonds, -0.25, 0.25), 0.5);
        // Patterns run from empty to full, the top-left pixel in the top bit.
        assert!(!pattern_mark(0.0, 0, 0));
        assert!(pattern_mark(1.0, 5, 3));
        assert!(pattern_mark(1.0 / 16.0, 0, 0));
        assert!(!pattern_mark(1.0 / 16.0, 1, 0));
        assert!(pattern_mark(1.0 / 16.0, 4, 4));
        // Scanline beam and dots.
        assert_eq!(beam_cover(0.0, 2.0, 1.5, 0.0), 0.0);
        assert!((beam_cover(1.0, 2.0, 0.5, 0.0) - 1.0).abs() < 1e-6);
        assert!((beam_cover(1.0, 2.0, 1.5, 0.0) - 0.4).abs() < 1e-5);
        assert_eq!(wobble_shift(0, 0.0), 0);
        assert_eq!(wobble_shift(3, 10.0), {
            let wave = (1.35f32).sin() * 0.7 + (6.4f32).sin() * 0.3;
            (10.0 * wave).round() as i64
        });
        assert!((dot_cover(1, 1, 4) - 1.0).abs() < 1e-6);
        assert!(dot_cover(0, 0, 4) < 0.5);
    }

    /// Diffusion keeps the average tone: a mid gray becomes about half black, half white.
    #[test]
    fn error_diffusion_keeps_the_average_and_atkinson_loses_some() {
        let image = gray(64, 64, 128);
        let floyd = dither(&image, &settings(DitherStyle::FloydSteinberg));
        assert!(floyd.pixels().all(|p| p[0] == 0 || p[0] == 255));
        assert!(
            (mean(&floyd) - 128.0 / 255.0).abs() < 0.02,
            "{}",
            mean(&floyd)
        );
        // Atkinson passes on 6/8 of the error, so a light gray comes out lighter: whiter
        // highlights, as on the classic Mac.
        let light = gray(64, 64, 200);
        let atkinson = dither(&light, &settings(DitherStyle::Atkinson));
        assert!(mean(&atkinson) > 200.0 / 255.0, "{}", mean(&atkinson));
        // Black and white stay as they are.
        assert!(
            dither(&gray(8, 8, 0), &settings(DitherStyle::Atkinson))
                .pixels()
                .all(|p| p[0] == 0)
        );
        assert!(
            dither(&gray(8, 8, 255), &settings(DitherStyle::Atkinson))
                .pixels()
                .all(|p| p[0] == 255)
        );
        // No diffusion is plain rounding: 128 / 255 rounds up everywhere.
        let none = DitherSettings {
            diffusion: 0.0,
            ..settings(DitherStyle::FloydSteinberg)
        };
        assert!(dither(&image, &none).pixels().all(|p| p[0] == 255));
        // Four levels: every value is one of 0, 85, 170 and 255.
        let four = DitherSettings {
            levels: 4.0,
            ..settings(DitherStyle::Atkinson)
        };
        assert!(dither(&image, &four).pixels().all(|p| p[0] % 85 == 0));
    }

    #[test]
    fn dither_keeps_alpha_and_skips_clear_pixels() {
        let mut image = gray(4, 4, 128);
        image.put_pixel(1, 1, Rgba([10, 20, 30, 0]));
        image.put_pixel(2, 1, Rgba([100, 100, 100, 77]));
        for style in DitherStyle::ALL {
            let out = dither(&image, &settings(style));
            assert_eq!(out.get_pixel(1, 1).0, [10, 20, 30, 0], "{style:?}");
            assert_eq!(out.get_pixel(2, 1)[3], 77, "{style:?}");
        }
    }

    #[test]
    fn ordered_dither_matches_the_matrix() {
        // A Bayer 2 × 2 of a 30% gray: only the cell with the lowest threshold (0.125) is lit
        // when 0.3 + threshold reaches 1, i.e. threshold ≥ 0.7: the 0.875 cell.
        let image = gray(4, 4, 77);
        let out = dither(&image, &settings(DitherStyle::Bayer2));
        for (x, y, p) in out.enumerate_pixels() {
            let lit = ordered_threshold(DitherStyle::Bayer2, x, y) + 77.0 / 255.0 >= 1.0;
            assert_eq!(p[0] == 255, lit, "({x}, {y})");
        }
        assert_eq!(out.pixels().filter(|p| p[0] == 255).count(), 4);
        // Two Colors maps black and white to the chosen pair.
        let two = DitherSettings {
            colors: DitherColors::TwoColors,
            dark: [20, 40, 60],
            light: [200, 180, 160],
            ..settings(DitherStyle::Bayer2)
        };
        let out = dither(&image, &two);
        assert!(
            out.pixels()
                .all(|p| p.0 == [20, 40, 60, 255] || p.0 == [200, 180, 160, 255])
        );
        // Original colors dither each channel on its own.
        let color = RgbaImage::from_pixel(8, 8, Rgba([255, 128, 0, 255]));
        let out = dither(
            &color,
            &DitherSettings {
                colors: DitherColors::Original,
                ..settings(DitherStyle::Bayer8)
            },
        );
        assert!(out.pixels().all(|p| p[0] == 255 && p[2] == 0));
        let half = out.pixels().filter(|p| p[1] == 255).count();
        assert_eq!(half, 32);
    }

    #[test]
    fn marks_follow_light_on_dark_and_cover_the_tone() {
        // Halftone dots of a white image, light on dark: dots as large as they get (a circle
        // of area 1 leaves the cell's corners dark) and nothing of a black one.
        let white = gray(32, 32, 255);
        let lit = |image: &RgbaImage| image.pixels().filter(|p| p[0] == 255).count();
        let dots = settings(DitherStyle::HalftoneDots);
        let full = lit(&dither(&white, &dots));
        assert!(full > 32 * 32 * 3 / 4 && full < 32 * 32, "{full}");
        assert_eq!(lit(&dither(&gray(32, 32, 0), &dots)), 0);
        // Dark on light, the marks are ink: white is all paper, black mostly ink.
        let ink = DitherSettings {
            light_on_dark: false,
            ..dots.clone()
        };
        assert_eq!(lit(&dither(&white, &ink)), 32 * 32);
        assert_eq!(lit(&dither(&gray(32, 32, 0), &ink)), 32 * 32 - full);
        // A mid gray covers about half of each cell, for every screen.
        for style in [
            DitherStyle::HalftoneDots,
            DitherStyle::HalftoneLines,
            DitherStyle::HalftoneDiamonds,
            DitherStyle::MacPatterns,
        ] {
            let out = dither(&gray(64, 64, 128), &settings(style));
            // Upstream's patterns step unevenly: the middle one is 24 of 64 pixels.
            assert!((mean(&out) - 0.5).abs() < 0.15, "{style:?} {}", mean(&out));
        }
    }

    #[test]
    fn chunky_pixels_and_dots() {
        let image = RgbaImage::from_fn(8, 8, |x, _| {
            let v = if x < 4 { 0 } else { 255 };
            Rgba([v, v, v, 255])
        });
        let chunky = DitherSettings {
            pixel_size: 4.0,
            ..settings(DitherStyle::Bayer2)
        };
        let out = dither(&image, &chunky);
        assert_eq!(out.get_pixel(0, 0)[0], 0);
        assert_eq!(out.get_pixel(7, 7)[0], 255);
        let dotted = DitherSettings {
            pixel_shape: DitherPixelShape::Dot,
            ..chunky.clone()
        };
        let out = dither(&image, &dotted);
        // The middle of a white chunky pixel stays lit; its corner shows the black gap.
        assert_eq!(out.get_pixel(5, 5)[0], 255);
        assert!(out.get_pixel(4, 4)[0] < 128);
        // A block over a partly covered edge averages what is there.
        let averaged = block_average(&gray(5, 1, 100), 4);
        assert_eq!(averaged.dimensions(), (2, 1));
        assert_eq!(averaged.get_pixel(1, 0).0, [100, 100, 100, 255]);
    }

    #[test]
    fn scanlines_draw_lit_lines_with_dark_between() {
        let lines = DitherSettings {
            glow: 0.0,
            ..settings(DitherStyle::Scanlines)
        };
        let out = dither(&gray(16, 16, 255), &lines);
        // Spacing 4: rows 1 and 2 of each line are lit (beam half-height 1.4, offsets 0.5),
        // rows 0 and 3 only partly.
        assert_eq!(out.get_pixel(3, 1)[0], 255);
        assert_eq!(out.get_pixel(3, 2)[0], 255);
        let edge = out.get_pixel(3, 0)[0];
        assert!(edge > 0 && edge < 255, "{edge}");
        // A black picture is dark screen.
        assert!(dither(&gray(16, 16, 0), &lines).pixels().all(|p| p[0] == 0));
        // Glow adds light between the lines.
        let glowing = dither(&gray(16, 16, 255), &settings(DitherStyle::Scanlines));
        assert!(glowing.get_pixel(3, 0)[0] > edge);
        assert!(lines.runs_on_gpu());
        assert!(!settings(DitherStyle::Scanlines).runs_on_gpu());
    }

    #[test]
    fn ascii_picks_more_ink_for_brighter_cells() {
        let glyphs = glyphs(DEFAULT_CHARACTERS, 14);
        assert_eq!(glyphs.maps.len(), 10);
        assert_eq!(glyphs.height, 14);
        assert!(glyphs.width >= 6 && glyphs.width <= 10, "{}", glyphs.width);
        // Space has no ink and comes first; the rest are sorted by ink.
        assert_eq!(glyphs.coverage[0], 0.0);
        assert!(glyphs.coverage.windows(2).all(|w| w[0] <= w[1]));
        let ascii = settings(DitherStyle::Ascii);
        assert!(dither(&gray(42, 28, 0), &ascii).pixels().all(|p| p[0] == 0));
        let bright = dither(&gray(42, 28, 255), &ascii);
        let lit = bright.pixels().filter(|p| p[0] > 0).count();
        assert!(lit > 42 * 28 / 8, "{lit}");
    }

    #[test]
    fn settings_validate_and_scale() {
        assert!(DitherSettings::default().validate().is_ok());
        let wrong = DitherSettings {
            levels: 9.0,
            ..Default::default()
        };
        assert!(format!("{:#}", wrong.validate().unwrap_err()).contains("Dither.levels"));
        let long = DitherSettings {
            characters: "x".repeat(65),
            ..Default::default()
        };
        assert!(long.validate().is_err());
        let half = DitherSettings::default().scaled(0.25);
        assert_eq!(half.pixel_size, 1.0);
        assert_eq!(half.text_size, 4.0);
        // 8 cells of 2 px are 16 px; at a quarter, 4 cells of 1 px.
        assert_eq!(half.cell_size, 4.0);
        assert_eq!(DitherStyle::Scanlines.code(), 10);
        let parsed: DitherSettings =
            serde_json::from_value(serde_json::json!({"style": "HalftoneDots"})).unwrap();
        assert_eq!(parsed.style, DitherStyle::HalftoneDots);
        assert_eq!(parsed.cell_size, 8.0);
    }
}

use std::sync::Arc;

use anyhow::{Context as _, Result, ensure};
use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight, Wrap,
};
use image::{Pixel, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use kurbo::{Affine, BezPath, Shape as _};

use crate::{
    document::{Layer, Point, Transform, validate_size},
    vector::{ArcLength, FillRule, VectorPath},
};

pub const MAX_TEXT_BYTES: usize = 16_384;
const FALLBACK_FAMILY: &str = "Inter Variable";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub content: String,
    pub family: String,
    pub size: f32,
    pub color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    /// The path the text follows instead of a box (format 11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<TextPath>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            content: "Text".into(),
            family: FALLBACK_FAMILY.into(),
            size: 48.0,
            color: [0, 0, 0, 255],
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            path: None,
        }
    }
}

impl TextStyle {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.content.len() <= MAX_TEXT_BYTES,
            "Text is limited to 16 KiB"
        );
        ensure!(
            !self.family.trim().is_empty() && self.family.len() <= 1024,
            "Invalid font family"
        );
        ensure!(
            self.size.is_finite() && (1.0..=1024.0).contains(&self.size),
            "Font size must be between 1 and 1024 pixels"
        );
        if let Some(path) = &self.path {
            path.validate()?;
        }
        Ok(())
    }

    pub fn layer_name(&self) -> String {
        let name: String = self
            .content
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(60)
            .collect();
        if name.is_empty() { "Text".into() } else { name }
    }
}

/// Where text on a path is anchored at its start offset, as SVG's `text-anchor`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathAlign {
    /// The text begins at the start offset.
    #[default]
    Start,
    /// The text is centred on it.
    Center,
    /// The text ends at it.
    End,
}

/// Which side of its path text on a path stands on, as SVG 2's `side`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathSide {
    /// On the left of the path's direction of travel: on top of a path drawn left to right.
    #[default]
    Left,
    /// On its other side, running the other way along it (Photoshop's flip).
    Right,
}

/// How text follows its path: where it starts, which way it runs, its spacing and how size
/// and opacity change from its first letter to its last.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathTextOptions {
    /// Where the text is anchored, in percent of the path's length from its start (−100–100).
    pub start_offset: f32,
    pub align: PathAlign,
    pub side: PathSide,
    /// Extra space after each letter, in pixels (negative to tighten).
    pub letter_spacing: f32,
    /// Whether letters turn to follow the path; otherwise they stay upright.
    pub rotate: bool,
    /// How far the letters are raised off the path, in pixels (negative lowers them).
    pub baseline_shift: f32,
    /// The font size at the end of the text, shrinking or growing from the style's size at its
    /// start; `None` keeps one size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_end: Option<f32>,
    /// Opacity (0–1) at the first and the last letter, blending between them.
    pub opacity_start: f32,
    pub opacity_end: f32,
}

impl Default for PathTextOptions {
    fn default() -> Self {
        Self {
            start_offset: 0.0,
            align: PathAlign::Start,
            side: PathSide::Left,
            letter_spacing: 0.0,
            rotate: true,
            baseline_shift: 0.0,
            size_end: None,
            opacity_start: 1.0,
            opacity_end: 1.0,
        }
    }
}

impl PathTextOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.start_offset.is_finite() && (-100.0..=100.0).contains(&self.start_offset),
            "The start offset must be between -100 and 100 percent"
        );
        ensure!(
            self.letter_spacing.is_finite() && self.letter_spacing.abs() <= 1000.0,
            "Letter spacing must be between -1000 and 1000 pixels"
        );
        ensure!(
            self.baseline_shift.is_finite() && self.baseline_shift.abs() <= 10_000.0,
            "The baseline shift must be between -10000 and 10000 pixels"
        );
        ensure!(
            self.size_end
                .is_none_or(|size| size.is_finite() && (1.0..=1024.0).contains(&size)),
            "The end size must be between 1 and 1024 pixels"
        );
        ensure!(
            [self.opacity_start, self.opacity_end]
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "Opacity must be between 0 and 1"
        );
        Ok(())
    }
}

/// The path a text layer follows: SVG path data in the coordinates of a `width` × `height`
/// box, which the layer's pixels cover, so the path moves and stretches with the layer, as a
/// path shape's outline does. Only its first subpath is followed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextPath {
    pub d: VectorPath,
    pub width: f32,
    pub height: f32,
    #[serde(flatten)]
    pub options: PathTextOptions,
}

impl TextPath {
    pub fn validate(&self) -> Result<()> {
        self.d.validate()?;
        ensure!(!self.d.is_empty(), "The text path has no points");
        ensure!(
            [self.width, self.height]
                .iter()
                .all(|v| v.is_finite() && (1.0..=300_000.0).contains(v)),
            "Invalid text path box"
        );
        self.options.validate()
    }

    /// The path in document coordinates for a layer placed by `transform`, unless the layer
    /// is warped.
    pub fn in_document(&self, transform: Transform) -> Option<VectorPath> {
        Some(
            self.d
                .transformed(transform.box_affine(self.width, self.height)?),
        )
    }

    /// The path in the pixels of a layer whose pixels are `width` × `height`.
    fn in_pixels(&self, width: u32, height: u32) -> VectorPath {
        self.d.transformed(Affine::scale_non_uniform(
            f64::from(width) / f64::from(self.width),
            f64::from(height) / f64::from(self.height),
        ))
    }

    /// `path`, given in document coordinates, kept in the box of a layer placed by
    /// `transform` whose pixels are `width` × `height`.
    pub fn from_document(
        path: &VectorPath,
        transform: Transform,
        (width, height): (u32, u32),
        options: PathTextOptions,
    ) -> Result<Self> {
        let to_document = transform
            .box_affine(width as f32, height as f32)
            .context("Remove the layer's warp before putting its text on a path")?;
        ensure!(
            to_document.determinant().abs() > 1e-12,
            "The layer is too thin to hold a path"
        );
        let path = Self {
            d: VectorPath::from_bez(to_document.inverse() * path.bez())?,
            width: width as f32,
            height: height as f32,
            options,
        };
        path.validate()?;
        Ok(path)
    }
}

/// One letter of text on a path, as laid out: where it sits and how it is drawn.
#[derive(Clone, Debug)]
pub struct PathGlyph {
    /// The glyph's index in the text's line, in visual order.
    pub index: usize,
    /// The point on the path under the middle of the glyph's advance.
    pub on_path: kurbo::Point,
    /// The path's direction there, in degrees clockwise from +x.
    pub path_angle: f64,
    /// The angle the glyph is drawn at: the path's, or 0 when letters stay upright.
    pub angle: f64,
    /// The font size the glyph is drawn at.
    pub size: f32,
    /// Its opacity along the ramp, 0–1.
    pub opacity: f32,
    /// The glyph's own coordinates (pen origin on the baseline, y down) to the drawing's.
    pub transform: Affine,
    glyph: cosmic_text::LayoutGlyph,
    /// Underline and strikethrough as (top, thickness) in the glyph's coordinates.
    rules: Vec<(f32, f32)>,
    /// Pixels of synthetic bold and whether italic is synthesised.
    embolden: i32,
    italic: bool,
}

/// One font database per editor, loaded lazily when the text tool is first used.
pub struct TextRenderer {
    fonts: FontSystem,
    families: Vec<String>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        let fonts = FontSystem::new_with_fonts([cosmic_text::fontdb::Source::Binary(Arc::new(
            include_bytes!("../assets/fonts/InterVariable.ttf").to_vec(),
        ))]);
        Self::with_fonts(fonts)
    }
}

impl TextRenderer {
    fn with_fonts(fonts: FontSystem) -> Self {
        let mut families: Vec<_> = fonts
            .db()
            .faces()
            .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
            .collect();
        families.sort_by_key(|name| name.to_lowercase());
        families.dedup();
        Self { fonts, families }
    }

    pub fn families(&self) -> &[String] {
        &self.families
    }

    pub fn has_family(&self, family: &str) -> bool {
        self.families
            .iter()
            .any(|name| name.eq_ignore_ascii_case(family))
    }

    /// Shape `style`'s text with the closest face of its family, unwrapped.
    fn shape(&mut self, style: &TextStyle) -> Result<Buffer> {
        style.validate()?;
        let line_height = style.size * 1.3;
        ensure!(
            (style.content.lines().count().max(1) as f32 * line_height) <= 30_000.0,
            "Text is too tall"
        );
        let family = if self.has_family(&style.family) {
            &style.family
        } else {
            FALLBACK_FAMILY
        };
        // Resolve the closest available face first. Requesting a missing weight or
        // style directly can substitute an unrelated family during shaping.
        let face = self
            .fonts
            .db()
            .query(&cosmic_text::fontdb::Query {
                families: &[Family::Name(family)],
                weight: if style.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                },
                style: if style.italic {
                    Style::Italic
                } else {
                    Style::Normal
                },
                ..Default::default()
            })
            .and_then(|id| self.fonts.db().face(id))
            .ok_or_else(|| anyhow::anyhow!("The font could not be loaded"))?;
        let attrs = Attrs::new()
            .family(Family::Name(family))
            .weight(face.weight)
            .style(face.style)
            .stretch(face.stretch);
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(style.size, line_height));
        buffer.set_wrap(&mut self.fonts, Wrap::None);
        buffer.set_size(&mut self.fonts, None, None);
        buffer.set_text(&mut self.fonts, &style.content, &attrs, Shaping::Advanced);
        Ok(buffer)
    }

    /// Whether a glyph needs synthetic italic, and how many pixels of synthetic bold, for a
    /// family without those faces.
    fn synthetic(&self, glyph: &cosmic_text::LayoutGlyph, style: &TextStyle) -> (bool, i32) {
        let face = self.fonts.db().face(glyph.font_id);
        let italic = style.italic && face.is_some_and(|face| face.style == Style::Normal);
        // Families without a bold face still have a visible bold style.
        let embolden = if style.bold && face.is_some_and(|face| face.weight < Weight::SEMIBOLD) {
            (style.size * 0.025).ceil() as i32
        } else {
            0
        };
        (italic, embolden)
    }

    /// Draw `style`'s text in a box, line by line, ignoring any path.
    pub fn render(&mut self, style: &TextStyle) -> Result<RgbaImage> {
        let line_height = style.size * 1.3;
        let buffer = self.shape(style)?;

        let mut right = 1.0_f32;
        let mut bottom = line_height;
        for run in buffer.layout_runs() {
            right = right.max(run.line_w);
            bottom = bottom.max(run.line_top + run.line_height);
        }
        validate_size(right.ceil() as u32, bottom.ceil() as u32)?;

        // Measure actual ink as well as advances so italic overhangs and combining
        // marks are preserved. Keep the glyph cache local to bound retained memory.
        let mut cache = SwashCache::new();
        let (mut left, mut top) = (0, 0);
        let (mut right, mut bottom) = (right.ceil() as i32, bottom.ceil() as i32);
        let mut glyphs = Vec::new();
        let mut rules = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let mut physical = glyph.physical((0.0, 0.0), 1.0);
                let (italic, embolden) = self.synthetic(glyph, style);
                if italic {
                    physical.cache_key.flags |= cosmic_text::CacheKeyFlags::FAKE_ITALIC;
                }
                let y = run.line_y as i32 + physical.y;
                if let Some(image) = cache.get_image(&mut self.fonts, physical.cache_key) {
                    let placement = image.placement;
                    let x = physical.x + placement.left;
                    let y = y - placement.top;
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x + placement.width as i32 + embolden);
                    bottom = bottom.max(y + placement.height as i32);
                }
                glyphs.push((physical, y, embolden));
            }
            let thickness = (style.size / 16.0).max(1.0);
            for (enabled, y) in [
                (style.underline, run.line_y + style.size * 0.1),
                (style.strikethrough, run.line_y - style.size * 0.3),
            ] {
                if enabled && run.line_w > 0.0 {
                    top = top.min(y.floor() as i32);
                    bottom = bottom.max((y + thickness).ceil() as i32);
                    rules.push((run.line_w, y, thickness));
                }
            }
        }
        let (width, height) = ((right - left) as u32, (bottom - top) as u32);
        validate_size(width, height)?;
        let mut pixels = RgbaImage::new(width, height);
        let color = Color::rgb(style.color[0], style.color[1], style.color[2]);
        for (glyph, baseline, embolden) in glyphs {
            cache.with_pixels(&mut self.fonts, glyph.cache_key, color, |x, y, color| {
                for offset in 0..=embolden {
                    if let Some(pixel) = pixels.get_pixel_mut_checked(
                        (glyph.x + x + offset - left) as u32,
                        (baseline + y - top) as u32,
                    ) {
                        pixel.blend(&Rgba(color.as_rgba()));
                    }
                }
            });
        }
        for (width, y, thickness) in rules {
            for py in y.floor() as i32..(y + thickness).ceil() as i32 {
                for px in 0..width.ceil() as i32 {
                    let coverage = (width - px as f32).min(1.0)
                        * ((y + thickness).min(py as f32 + 1.0) - y.max(py as f32));
                    let mut rgba = color.as_rgba();
                    rgba[3] = (coverage * 255.0).round() as u8;
                    pixels
                        .get_pixel_mut((px - left) as u32, (py - top) as u32)
                        .blend(&Rgba(rgba));
                }
            }
        }
        for pixel in pixels.pixels_mut() {
            pixel[3] = ((u16::from(pixel[3]) * u16::from(style.color[3]) + 127) / 255) as u8;
        }
        Ok(pixels)
    }

    /// Lay out `style`'s text along the first subpath of `path`, given in the drawing's
    /// pixels, with `style.path`'s options (the defaults without one). Each glyph keeps the
    /// position and advance cosmic-text gives it on a straight line, which become a distance
    /// along the path; it is drawn there, turned to the path's direction. Glyphs whose middle
    /// falls before the start or past the end of an open path are left out, as Photoshop
    /// hides them; on a closed path the text wraps around. Later lines run beside the first,
    /// a line height further from the path each.
    pub fn layout_on_path(
        &mut self,
        style: &TextStyle,
        path: &VectorPath,
    ) -> Result<Vec<PathGlyph>> {
        let options = style
            .path
            .as_ref()
            .map(|p| p.options.clone())
            .unwrap_or_default();
        options.validate()?;
        let measure = ArcLength::first(path).context("The text path has no points")?;
        let measure = match options.side {
            PathSide::Left => measure,
            PathSide::Right => measure.reversed(),
        };
        let buffer = self.shape(style)?;
        let anchor = f64::from(options.start_offset) / 100.0 * measure.length();
        let spacing = f64::from(options.letter_spacing);
        let end_scale = f64::from(options.size_end.unwrap_or(style.size) / style.size);
        let thickness = (style.size / 16.0).max(1.0);
        let mut placed = Vec::new();
        let mut first_baseline = None;
        for run in buffer.layout_runs() {
            let baseline = *first_baseline.get_or_insert(run.line_y);
            let line_offset = f64::from(run.line_y - baseline);
            let width = f64::from(run.line_w);
            let mut order: Vec<&cosmic_text::LayoutGlyph> = run.glyphs.iter().collect();
            order.sort_by(|a, b| a.x.total_cmp(&b.x));
            // How many letters (clusters) come before each glyph, for letter spacing.
            let mut letters = Vec::with_capacity(order.len());
            let mut count = 0_usize;
            for (i, glyph) in order.iter().enumerate() {
                if i > 0 && order[i - 1].start != glyph.start {
                    count += 1;
                }
                letters.push(count);
            }
            // The distance from the line's start to `x` once the size ramp has scaled the
            // letters: the integral of a scale that changes linearly across the line.
            let along = |x: f64| {
                if width > 0.0 {
                    x * (1.0 + (end_scale - 1.0) * x / (2.0 * width))
                } else {
                    x
                }
            };
            let total = along(width) + spacing * count as f64;
            let start = anchor
                - match options.align {
                    PathAlign::Start => 0.0,
                    PathAlign::Center => total / 2.0,
                    PathAlign::End => total,
                };
            for (index, (glyph, letter)) in order.into_iter().zip(letters).enumerate() {
                let centre = f64::from(glyph.x + glyph.w / 2.0);
                let t = if width > 0.0 {
                    (centre / width).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let scale = 1.0 + (end_scale - 1.0) * t;
                let distance = start + along(centre) + spacing * letter as f64;
                let Some((point, direction)) = measure.sample(distance) else {
                    continue;
                };
                let path_angle = direction.y.atan2(direction.x);
                let angle = if options.rotate { path_angle } else { 0.0 };
                // Later lines and the baseline shift move off the path along its normal.
                let shift = line_offset - f64::from(options.baseline_shift);
                let (sin, cos) = path_angle.sin_cos();
                let base = point + kurbo::Vec2::new(-sin * shift, cos * shift);
                let pen = (
                    f64::from(glyph.x + glyph.x_offset * glyph.font_size),
                    f64::from(glyph.y - glyph.y_offset * glyph.font_size),
                );
                let transform = Affine::translate(base.to_vec2())
                    * Affine::rotate(angle)
                    * Affine::scale(scale)
                    * Affine::translate((pen.0 - centre, pen.1));
                let mut rules = Vec::new();
                for (enabled, top) in [
                    (style.underline, style.size * 0.1),
                    (style.strikethrough, -style.size * 0.3),
                ] {
                    if enabled && glyph.w > 0.0 {
                        let x0 = f64::from(glyph.x) - pen.0;
                        let y0 = f64::from(top) - pen.1;
                        rules.push(kurbo::Rect::new(
                            x0,
                            y0,
                            x0 + f64::from(glyph.w),
                            y0 + f64::from(thickness),
                        ));
                    }
                }
                let (italic, embolden) = self.synthetic(glyph, style);
                placed.push(PathGlyph {
                    index,
                    on_path: point,
                    path_angle: path_angle.to_degrees(),
                    angle: angle.to_degrees(),
                    size: (f64::from(style.size) * scale) as f32,
                    opacity: options.opacity_start
                        + (options.opacity_end - options.opacity_start) * t as f32,
                    transform,
                    glyph: glyph.clone(),
                    rules,
                    embolden,
                    italic,
                });
            }
        }
        Ok(placed)
    }

    /// Draw `style`'s text along `path` (see [`Self::layout_on_path`]): the pixels, and where
    /// their top-left corner is in the path's coordinates.
    pub fn render_on_path(
        &mut self,
        style: &TextStyle,
        path: &VectorPath,
    ) -> Result<(RgbaImage, [i32; 2])> {
        let glyphs = self.layout_on_path(style, path)?;
        let mut cache = SwashCache::new();
        // Each glyph's ink in the drawing's coordinates, and its bounds.
        let mut inks = Vec::with_capacity(glyphs.len());
        let mut bounds: Option<kurbo::Rect> = None;
        for placed in &glyphs {
            let glyph = &placed.glyph;
            let (key, _, _) = cosmic_text::CacheKey::new(
                glyph.font_id,
                glyph.glyph_id,
                glyph.font_size,
                (0.0, 0.0),
                cosmic_text::CacheKeyFlags::empty(),
            );
            let mut ink = Ink {
                outline: None,
                bitmap: None,
                rules: BezPath::new(),
                opacity: placed.opacity,
            };
            if let Some(commands) = cache.get_outline_commands(&mut self.fonts, key) {
                let outline = outline_path(commands);
                // Synthetic italic leans 14° as cosmic-text's does; synthetic bold overlaps
                // copies a pixel apart, which fill as one shape.
                let lean = if placed.italic {
                    Affine::new([1.0, 0.0, -(14.0_f64.to_radians().tan()), 1.0, 0.0, 0.0])
                } else {
                    Affine::IDENTITY
                };
                let mut path = BezPath::new();
                for copy in 0..=placed.embolden {
                    let shifted = placed.transform
                        * Affine::translate((f64::from(copy), 0.0))
                        * lean
                        * &outline;
                    path.extend(shifted.elements().iter().copied());
                }
                if !path.elements().is_empty() {
                    ink.outline = Some(path);
                }
            } else if let Some(image) = cache.get_image(&mut self.fonts, key).clone()
                && image.placement.width > 0
                && image.placement.height > 0
            {
                // Colour glyphs without an outline are drawn from their bitmap.
                let to_drawing = placed.transform
                    * Affine::translate((
                        f64::from(image.placement.left),
                        -f64::from(image.placement.top),
                    ));
                ink.bitmap = Some((image, to_drawing));
            }
            for rule in &placed.rules {
                ink.rules
                    .extend((placed.transform * rule.to_path(0.1)).elements().iter().copied());
            }
            if let Some(area) = ink.bounds() {
                bounds = Some(bounds.map_or(area, |b| b.union(area)));
                inks.push(ink);
            }
        }
        let area = bounds.unwrap_or_else(|| {
            let start = ArcLength::first(path)
                .and_then(|m| m.sample(0.0))
                .map_or(kurbo::Point::ZERO, |(p, _)| p);
            kurbo::Rect::from_origin_size(start, (1.0, 1.0))
        });
        let (left, top) = (area.x0.floor(), area.y0.floor());
        let (width, height) = (
            ((area.x1.ceil() - left) as u32).max(1),
            ((area.y1.ceil() - top) as u32).max(1),
        );
        ensure!(
            left.abs() <= 1_000_000.0 && top.abs() <= 1_000_000.0,
            "The text path is too far away"
        );
        validate_size(width, height)?;
        let mut pixels = RgbaImage::new(width, height);
        let [r, g, b, a] = style.color;
        for ink in inks {
            let alpha = f32::from(a) / 255.0 * ink.opacity;
            ink.draw(&mut pixels, (left, top), [r, g, b], alpha)?;
        }
        Ok((pixels, [left as i32, top as i32]))
    }
}

/// A glyph's ink on a path: its outline or, for a colour glyph, its bitmap mapped into the
/// drawing; and its underline and strikethrough.
struct Ink {
    outline: Option<BezPath>,
    bitmap: Option<(cosmic_text::SwashImage, Affine)>,
    rules: BezPath,
    opacity: f32,
}

impl Ink {
    fn bounds(&self) -> Option<kurbo::Rect> {
        let mut parts = Vec::new();
        if let Some(outline) = &self.outline {
            parts.push(outline.bounding_box());
        }
        if let Some((image, to_drawing)) = &self.bitmap {
            let size = (
                f64::from(image.placement.width),
                f64::from(image.placement.height),
            );
            parts.push(to_drawing.transform_rect_bbox(kurbo::Rect::from_origin_size(
                kurbo::Point::ZERO,
                size,
            )));
        }
        if !self.rules.elements().is_empty() {
            parts.push(self.rules.bounding_box());
        }
        parts
            .into_iter()
            .filter(|r| r.is_finite())
            .reduce(|a, b| a.union(b))
    }

    /// Blend the ink onto `pixels`, whose top-left corner is at `origin` in the drawing.
    fn draw(
        &self,
        pixels: &mut RgbaImage,
        origin: (f64, f64),
        color: [u8; 3],
        alpha: f32,
    ) -> Result<()> {
        let Some(area) = self.bounds() else {
            return Ok(());
        };
        let x0 = (area.x0.floor() - origin.0).max(0.0) as u32;
        let y0 = (area.y0.floor() - origin.1).max(0.0) as u32;
        let x1 = ((area.x1.ceil() - origin.0) as u32).min(pixels.width());
        let y1 = ((area.y1.ceil() - origin.1) as u32).min(pixels.height());
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let to_grid = Affine::translate((-(origin.0 + f64::from(x0)), -(origin.1 + f64::from(y0))));
        let mut coverage = vec![0.0_f32; (w * h) as usize];
        for path in [self.outline.as_ref(), Some(&self.rules)].into_iter().flatten() {
            if path.elements().is_empty() {
                continue;
            }
            let part = VectorPath::from_bez(path.clone())?.coverage(FillRule::Nonzero, to_grid, w, h);
            for (value, part) in coverage.iter_mut().zip(part) {
                *value = value.max(part);
            }
        }
        for y in 0..h {
            for x in 0..w {
                let covered = coverage[(y * w + x) as usize];
                let mut source = Rgba([color[0], color[1], color[2], 0]);
                let mut weight = covered;
                if let Some((image, to_drawing)) = &self.bitmap {
                    // Sample the bitmap under this pixel's centre.
                    let at = to_drawing.inverse()
                        * kurbo::Point::new(
                            origin.0 + f64::from(x0 + x) + 0.5,
                            origin.1 + f64::from(y0 + y) + 0.5,
                        );
                    if let Some(sample) = sample_bitmap(image, at) {
                        let ink = f32::from(sample[3]) / 255.0;
                        if ink > weight {
                            source = Rgba([sample[0], sample[1], sample[2], 0]);
                            weight = ink;
                        }
                    }
                }
                if weight <= 0.0 {
                    continue;
                }
                source[3] = (weight * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                if source[3] > 0 {
                    pixels.get_pixel_mut(x0 + x, y0 + y).blend(&source);
                }
            }
        }
        Ok(())
    }
}

/// A glyph bitmap's colour at `at`, in its pixels, interpolated bilinearly; a mask bitmap
/// gives white with its coverage as alpha.
fn sample_bitmap(image: &cosmic_text::SwashImage, at: kurbo::Point) -> Option<[u8; 4]> {
    let (w, h) = (
        image.placement.width as i64,
        image.placement.height as i64,
    );
    let (fx, fy) = (at.x - 0.5, at.y - 0.5);
    if fx < -1.0 || fy < -1.0 || fx > w as f64 || fy > h as f64 {
        return None;
    }
    let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (fx - ix as f64, fy - iy as f64);
    let color = matches!(image.content, cosmic_text::SwashContent::Color);
    let texel = |x: i64, y: i64| -> [f64; 4] {
        if x < 0 || y < 0 || x >= w || y >= h {
            return [0.0; 4];
        }
        let i = (y * w + x) as usize;
        if color {
            let p = &image.data[i * 4..i * 4 + 4];
            let a = f64::from(p[3]);
            [f64::from(p[0]) * a, f64::from(p[1]) * a, f64::from(p[2]) * a, a]
        } else {
            let a = f64::from(image.data[i]);
            [255.0 * a, 255.0 * a, 255.0 * a, a]
        }
    };
    let mut sum = [0.0; 4];
    for (dx, dy, weight) in [
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (1, 0, tx * (1.0 - ty)),
        (0, 1, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ] {
        let t = texel(ix + dx, iy + dy);
        for (s, v) in sum.iter_mut().zip(t) {
            *s += v * weight;
        }
    }
    if sum[3] <= 0.0 {
        return None;
    }
    let unpremultiply = |v: f64| (v / sum[3]).round().clamp(0.0, 255.0) as u8;
    Some([
        unpremultiply(sum[0]),
        unpremultiply(sum[1]),
        unpremultiply(sum[2]),
        sum[3].round().clamp(0.0, 255.0) as u8,
    ])
}

/// A glyph outline from swash (y up, pen origin at 0, 0) as a path with y down.
fn outline_path(commands: &[cosmic_text::Command]) -> BezPath {
    use cosmic_text::Command;
    macro_rules! p {
        ($v:expr) => {
            kurbo::Point::new(f64::from($v.x), -f64::from($v.y))
        };
    }
    let mut path = BezPath::new();
    for command in commands {
        match command {
            Command::MoveTo(a) => path.move_to(p!(a)),
            Command::LineTo(a) => path.line_to(p!(a)),
            Command::QuadTo(a, b) => path.quad_to(p!(a), p!(b)),
            Command::CurveTo(a, b, c) => path.curve_to(p!(a), p!(b), p!(c)),
            Command::Close => path.close_path(),
        }
    }
    path
}

/// Redraw a text layer for `style`: in a box, keeping the layer's top-left corner, scale
/// and rotation; or along its path, which stays where it is in the document.
pub fn restyle_layer(renderer: &mut TextRenderer, layer: &mut Layer, style: TextStyle) -> Result<()> {
    let Some(path) = style.path.as_ref() else {
        let pixels = renderer.render(&style)?;
        return update_layer(layer, style, pixels);
    };
    let (width, height) = layer
        .pixels
        .as_ref()
        .map(|p| p.dimensions())
        .context("Text layer has no pixels")?;
    let in_pixels = path.in_pixels(width, height);
    let (pixels, origin) = renderer.render_on_path(&style, &in_pixels)?;
    let mut style = style;
    let stored = style.path.as_mut().unwrap();
    stored.d = in_pixels.transformed(Affine::translate((
        -f64::from(origin[0]),
        -f64::from(origin[1]),
    )));
    stored.width = pixels.width() as f32;
    stored.height = pixels.height() as f32;
    let anchor = Point::new(
        origin[0] as f32 / width as f32,
        origin[1] as f32 / height as f32,
    );
    place_layer(layer, style, pixels, anchor)
}

/// A new text layer whose text follows `path`, given in document coordinates, which also
/// places the layer.
pub fn path_layer(
    renderer: &mut TextRenderer,
    style: TextStyle,
    path: &VectorPath,
    options: PathTextOptions,
) -> Result<Layer> {
    let mut style = TextStyle {
        path: Some(TextPath {
            d: path.clone(),
            width: 1.0,
            height: 1.0,
            options,
        }),
        ..style
    };
    let (pixels, origin) = renderer.render_on_path(&style, path)?;
    let stored = style.path.as_mut().unwrap();
    stored.d = path.transformed(Affine::translate((
        -f64::from(origin[0]),
        -f64::from(origin[1]),
    )));
    stored.width = pixels.width() as f32;
    stored.height = pixels.height() as f32;
    style.validate()?;
    let mut layer = Layer::image(style.layer_name(), pixels);
    layer.transform.x = origin[0] as f32;
    layer.transform.y = origin[1] as f32;
    layer.text = Some(style);
    ensure!(layer.transform.valid(), "Invalid layer placement");
    Ok(layer)
}

/// Replace the text while retaining the layer's scale, rotation, and top-left anchor.
pub fn update_layer(layer: &mut Layer, style: TextStyle, pixels: RgbaImage) -> Result<()> {
    place_layer(layer, style, pixels, Point::new(0.0, 0.0))
}

/// Replace the text and pixels, keeping the layer's scale and rotation, with the new pixels'
/// top-left corner where the point `anchor` (in the old pixels, 0–1) was.
fn place_layer(layer: &mut Layer, style: TextStyle, pixels: RgbaImage, anchor: Point) -> Result<()> {
    style.validate()?;
    ensure!(
        !layer.locked && layer.text.is_some(),
        "Select an unlocked text layer"
    );
    let old = layer
        .pixels
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Text layer has no pixels"))?;
    let anchor = layer.transform.point(anchor);
    let mut transform = layer.transform;
    transform.width *= pixels.width() as f32 / old.width() as f32;
    transform.height *= pixels.height() as f32 / old.height() as f32;
    let moved = transform.point(Point::new(0.0, 0.0));
    transform.x += anchor.x - moved.x;
    transform.y += anchor.y - moved.y;
    ensure!(transform.valid(), "Text transform is too large");
    if layer
        .text
        .as_ref()
        .is_some_and(|old| old.layer_name() == layer.name)
    {
        layer.name = style.layer_name();
    }
    layer.set_transform(transform);
    layer.pixels = Some(Arc::new(pixels));
    layer.text = Some(style);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        document::{Document, Mask},
        io, paint, render,
    };

    fn renderer() -> TextRenderer {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(include_bytes!("../assets/fonts/InterVariable.ttf").to_vec());
        TextRenderer::with_fonts(FontSystem::new_with_locale_and_db("en-US".into(), db))
    }

    fn layer(renderer: &mut TextRenderer, style: TextStyle) -> Layer {
        let mut layer = Layer::image(style.layer_name(), renderer.render(&style).unwrap());
        layer.text = Some(style);
        layer
    }

    #[test]
    fn font_discovery_includes_installed_families_and_bundled_fallback() {
        let renderer = TextRenderer::default();
        assert!(renderer.has_family(FALLBACK_FAMILY));
        for face in renderer.fonts.db().faces() {
            for (family, _) in &face.families {
                assert!(renderer.has_family(family));
            }
        }
        let mut fallback = self::renderer();
        let missing = TextStyle {
            family: "Definitely unavailable Xuan test font".into(),
            ..Default::default()
        };
        assert_eq!(
            fallback.render(&missing).unwrap(),
            fallback.render(&TextStyle::default()).unwrap()
        );
    }

    #[test]
    fn styles_change_ink_and_preserve_color_alpha_and_multiline_layout() {
        let mut renderer = renderer();
        let style = TextStyle {
            content: "Office fj Å\nSecond line".into(),
            color: [30, 110, 190, 128],
            ..Default::default()
        };
        let plain = renderer.render(&style).unwrap();
        assert!(plain.height() >= (style.size * 2.6) as u32);
        assert!(plain.pixels().any(|pixel| pixel[3] == 128));
        assert!(plain.pixels().any(|pixel| (1..128).contains(&pixel[3])));
        for index in 0..4 {
            let mut decorated = style.clone();
            match index {
                0 => decorated.bold = true,
                1 => decorated.italic = true,
                2 => decorated.underline = true,
                _ => decorated.strikethrough = true,
            }
            let pixels = renderer.render(&decorated).unwrap();
            assert_ne!(
                pixels, plain,
                "Decoration {index} must affect rendered text"
            );
            for pixel in pixels.pixels().filter(|pixel| pixel[3] > 0) {
                assert!(pixel[3] <= 128);
                for channel in 0..3 {
                    assert!(
                        (i16::from(pixel[channel]) - i16::from(style.color[channel])).abs() <= 1
                    );
                }
            }
        }
    }

    #[test]
    fn empty_text_and_invalid_or_oversized_input_are_handled() {
        let mut renderer = renderer();
        let mut style = TextStyle {
            content: String::new(),
            ..Default::default()
        };
        assert!(
            renderer
                .render(&style)
                .unwrap()
                .pixels()
                .all(|pixel| pixel[3] == 0)
        );
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, 1025.0] {
            style.size = size;
            assert!(renderer.render(&style).is_err());
        }
        style.size = 1024.0;
        style.content = "W".repeat(100);
        assert!(renderer.render(&style).is_err());
        style.content = "line\n".repeat(100);
        assert!(renderer.render(&style).is_err());
        style.content = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(renderer.render(&style).is_err());
    }

    #[test]
    fn text_round_trips_with_pixels_and_remains_editable_after_transform() {
        let mut renderer = renderer();
        let mut layer = layer(&mut renderer, TextStyle::default());
        layer.transform.x = 20.0;
        layer.transform.y = 30.0;
        layer.transform.rotation = 25.0;
        layer.transform.width *= 1.5;
        layer.transform.height *= 0.75;
        layer.mask = Some(Mask::white());
        let anchor = layer.transform.point(Point::default());
        let mut style = layer.text.clone().unwrap();
        style.content = "Longer text\nwith decorations".into();
        style.underline = true;
        style.strikethrough = true;
        let pixels = renderer.render(&style).unwrap();
        let dimensions = pixels.dimensions();
        update_layer(&mut layer, style.clone(), pixels).unwrap();
        assert!(anchor.distance(layer.transform.point(Point::default())) < 0.001);
        assert!((layer.transform.width - dimensions.0 as f32 * 1.5).abs() < 0.001);
        assert!((layer.transform.height - dimensions.1 as f32 * 0.75).abs() < 0.001);
        let mut document = Document::new(600, 300).unwrap();
        document.insert(layer);
        // Close the destination handle so Windows can replace the file during save.
        let path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        io::save(&document, &path).unwrap();
        let loaded = io::load(&path).unwrap();
        assert_eq!(loaded.active().unwrap().text, Some(style));
        assert_eq!(
            loaded.active().unwrap().pixels,
            document.active().unwrap().pixels
        );
        assert_eq!(render::render(&loaded), render::render(&document));

        let old = document.active_mut().unwrap();
        paint::ensure_pixels(old).unwrap();
        assert!(old.text.is_none());
        let mut json = serde_json::to_value(old).unwrap();
        json.as_object_mut().unwrap().remove("text");
        assert!(
            serde_json::from_value::<Layer>(json)
                .unwrap()
                .text
                .is_none()
        );
    }
}

#[cfg(test)]
#[path = "text_path_tests.rs"]
mod path_tests;

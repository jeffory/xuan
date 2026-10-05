//! One-way import of Compositor `.comp` packages, format versions 1–11.
//!
//! A package is a directory holding `manifest.json` and `images/<layer UUID>.png` (plus
//! `.mask.png`) assets; upstream documents the format in `docs/project-format.md`. Parts of a
//! project Xuan cannot represent yet (layer effects, some blend modes and adjustment kinds, text
//! layout) are left out and counted in an [`ImportReport`], so the rest of the project still opens.
//! Damaged or hostile manifests and assets are rejected as a whole.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use uuid::Uuid;

use super::{MAX_ASSET, MAX_MANIFEST, decode_image, package_read};
use crate::{
    blend::BlendMode,
    document::{Adjustment, Document, Layer, Mask, Point, Transform},
    effects::Filter,
    i18n::tr,
    layer_effects::LayerEffects,
    layout::{Guide, GuideAxis, MAX_GUIDE_POSITION, MAX_GUIDES},
    text::{MAX_TEXT_BYTES, TextStyle},
};

/// The newest Compositor project version the importer reads (Compositor 1.4.5 writes 11).
pub const NEWEST_VERSION: u64 = 11;
/// Upstream's limit on text content, in UTF-16 units.
const MAX_TEXT_UNITS: usize = 100_000;
/// Upstream's limit on a per-letter font name, in characters.
const MAX_FONT_NAME: usize = 200;
/// Upstream's largest text layer paragraph box side and area, in layer pixels.
const MAX_BOX_SIDE: f64 = 30_000.0;
const MAX_BOX_AREA: f64 = 200_000_000.0;

/// Photoshop blend modes Xuan has that upstream does not (Document/LayerAppearance.swift leaves
/// them out); a package naming one is malformed.
const XUAN_ONLY_BLEND_MODES: [BlendMode; 3] = [
    BlendMode::Dissolve,
    BlendMode::DarkerColor,
    BlendMode::LighterColor,
];

/// Something in a Compositor project that Xuan imported only partly, or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dropped {
    /// Effects on a folder or an adjustment layer, which upstream never draws; left out.
    LayerEffect,
    /// A text layer whose alignment, tracking, leading or paragraph box Xuan ignores.
    TextLayout,
    /// A text layer with per-letter colors (version 10).
    TextColors,
    /// A text layer with per-letter fonts (version 11).
    TextFonts,
    /// A text layer beyond Xuan's text limits, imported as plain pixels.
    TextAsPixels,
    /// A live line shape, imported as plain pixels.
    LineShape,
    /// A blur or noise layer whose settings were brought into Xuan's range or approximated.
    FilterSettings,
    /// A clipping mask released because its layer or base became a filter or was left out.
    ClippingMask,
}

impl Dropped {
    fn label(self) -> String {
        match self {
            Self::LayerEffect => tr("Layer effects on folders or adjustment layers").into(),
            Self::TextLayout => {
                tr("Text alignment, spacing or paragraph box (kept until the text is edited)")
                    .into()
            }
            Self::TextColors => tr("Per-letter text colors (kept until the text is edited)").into(),
            Self::TextFonts => tr("Per-letter fonts (kept until the text is edited)").into(),
            Self::TextAsPixels => tr("Text beyond Xuan's limits (imported as pixels)").into(),
            Self::LineShape => tr("Live line shapes (imported as pixels)").into(),
            Self::FilterSettings => tr("Blur or noise settings adapted to Xuan").into(),
            Self::ClippingMask => tr("Clipping masks on blur or noise layers").into(),
        }
    }
}

/// What a Compositor import left out or changed. Empty for `.xuan` projects and for packages
/// Xuan represents completely.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    dropped: BTreeMap<Dropped, usize>,
}

impl ImportReport {
    fn add(&mut self, item: Dropped) {
        *self.dropped.entry(item).or_default() += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty()
    }

    /// How many times `item` occurred in the imported project.
    pub fn count(&self, item: Dropped) -> usize {
        self.dropped.get(&item).copied().unwrap_or(0)
    }

    /// A short, translated notice for the user, or `None` when nothing was lost.
    pub fn summary(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut text = tr(
            "Imported with changes. These parts of the Compositor project aren't supported yet:",
        )
        .to_owned();
        for (item, count) in &self.dropped {
            text.push_str(&format!("\n• {}: {count}", item.label()));
        }
        Some(text)
    }
}

fn number(value: &Value, key: &str, default: f32) -> f32 {
    value[key].as_f64().map_or(default, |v| v as f32)
}

/// A number in `range`, or `default` when missing. Anything else marks a damaged project.
fn ranged(
    value: &Value,
    key: &str,
    default: f64,
    range: std::ops::RangeInclusive<f64>,
) -> Result<f64> {
    let v = &value[key];
    if v.is_null() {
        return Ok(default);
    }
    let v = v
        .as_f64()
        .with_context(|| format!("Invalid Compositor value: {key}"))?;
    ensure!(
        v.is_finite() && range.contains(&v),
        "Compositor value out of range: {key}"
    );
    Ok(v)
}

fn identifier(value: &Value) -> Result<Option<Uuid>> {
    value
        .as_str()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(Into::into)
}

pub(super) fn comp_transform(value: &Value) -> Result<Transform> {
    let pair = |value: &Value, a: &str, b: &str| -> Result<(f32, f32)> {
        let (x, y) = if let Some(values) = value.as_array() {
            ensure!(values.len() == 2, "Invalid transform coordinates");
            (values[0].as_f64(), values[1].as_f64())
        } else {
            (value[a].as_f64(), value[b].as_f64())
        };
        Ok((
            x.context("Missing transform coordinate")? as f32,
            y.context("Missing transform coordinate")? as f32,
        ))
    };
    let (x, y) = pair(&value["origin"], "x", "y")?;
    let (width, height) = pair(&value["size"], "width", "height")?;
    let t = Transform {
        x,
        y,
        width,
        height,
        rotation: number(value, "rotation", 0.0),
        flip_x: value["flipX"].as_bool().unwrap_or(false),
        flip_y: value["flipY"].as_bool().unwrap_or(false),
        warp: None,
    };
    ensure!(t.valid(), "Invalid Compositor layer transform");
    Ok(t)
}

// Swift dictionaries with enum keys are encoded as alternating key/value arrays.
fn swift_dictionary_get<'a>(value: &'a Value, key: &str) -> &'a Value {
    if let Some(array) = value.as_array() {
        for pair in array.as_chunks::<2>().0 {
            if pair[0].as_str() == Some(key) {
                return &pair[1];
            }
        }
        &Value::Null
    } else {
        &value[key]
    }
}

/// What an upstream adjustment record becomes in Xuan.
#[derive(Debug, PartialEq)]
pub(super) enum ImportedAdjustment {
    Adjustment(Adjustment),
    /// Blur and noise adjustments sample neighbors; Xuan has them as filter layers.
    Filter(Filter),
}

pub(super) fn comp_adjustment(
    value: &Value,
    version: u64,
    report: &mut ImportReport,
) -> Result<ImportedAdjustment> {
    let kind = value["kind"]
        .as_str()
        .context("Adjustment kind is missing")?;
    let result = match kind {
        "Hue/Saturation" => {
            let hsv = &value["hsvSettings"];
            if hsv.is_null() {
                Adjustment::HueSaturation {
                    hue: number(value, "hue", 0.0),
                    saturation: number(value, "saturation", 0.0),
                    lightness: number(value, "lightness", 0.0),
                    colorize: value["colorize"].as_bool().unwrap_or(false),
                }
            } else {
                let mut settings = crate::color::HueSettings {
                    range: crate::color::HueSettings::RANGES
                        .iter()
                        .position(|name| Some(*name) == hsv["range"].as_str())
                        .unwrap_or(0),
                    colorize: hsv["colorize"].as_bool().unwrap_or(false),
                    invert_range: hsv["invertRange"].as_bool().unwrap_or(false),
                    ..Default::default()
                };
                for (index, name) in crate::color::HueSettings::RANGES.iter().enumerate() {
                    let adjustment = swift_dictionary_get(&hsv["adjustments"], name);
                    settings.adjustments[index] = [
                        number(adjustment, "hue", 0.0),
                        number(adjustment, "saturation", 0.0),
                        number(adjustment, "lightness", 0.0),
                    ];
                    let band = swift_dictionary_get(&hsv["bands"], name);
                    if !band.is_null() {
                        settings.bands[index] = [
                            number(band, "falloffStart", 0.0),
                            number(band, "rangeStart", 0.0),
                            number(band, "rangeEnd", 360.0),
                            number(band, "falloffEnd", 360.0),
                        ];
                    }
                }
                Adjustment::HueRanges {
                    settings: Box::new(settings),
                }
            }
        }
        "Levels" => {
            let input = value["levels"]["ranges"]
                .as_array()
                .context("Missing levels ranges")?;
            ensure!(input.len() == 4, "Invalid levels ranges");
            let ranges = std::array::from_fn(|i| {
                let r = &input[i];
                [
                    number(r, "black", 0.0),
                    number(r, "gamma", 1.0),
                    number(r, "white", 255.0),
                    number(r, "outputBlack", 0.0),
                    number(r, "outputWhite", 255.0),
                ]
            });
            Adjustment::LevelsChannels { ranges }
        }
        "Curves" => {
            let input = value["curves"]["channels"]
                .as_array()
                .context("Missing curve channels")?;
            ensure!(
                input.len() == 4 && input.iter().all(|c| c.is_array()),
                "Invalid curve channels"
            );
            let channels = std::array::from_fn(|i| {
                input[i]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| Point::new(number(p, "x", 0.0) / 255.0, number(p, "y", 0.0) / 255.0))
                    .collect()
            });
            Adjustment::CurvesChannels { channels }
        }
        "Exposure" => {
            let settings = &value["exposureSettings"];
            Adjustment::Exposure {
                exposure: number(settings, "exposure", 0.0),
                offset: number(settings, "offset", 0.0),
                gamma: number(settings, "gamma", 1.0),
            }
        }
        "Gradient Map" => {
            let reversed = value["gradientMapSettings"]["reversed"]
                .as_bool()
                .unwrap_or(false);
            let color = |key| {
                let c = &value["gradientMapSettings"][key];
                let default = if key == "shadows" { 0.0 } else { 1.0 };
                [
                    number(c, "red", default),
                    number(c, "green", default),
                    number(c, "blue", default),
                    1.0,
                ]
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            };
            let (shadows, highlights) = if reversed {
                ("highlights", "shadows")
            } else {
                ("shadows", "highlights")
            };
            Adjustment::GradientMap {
                shadows: color(shadows),
                highlights: color(highlights),
            }
        }
        "Grain" => {
            let settings = &value["grainSettings"];
            Adjustment::FilmGrain {
                amount: number(settings, "amount", 0.0),
                size: number(settings, "size", 1.0),
                roughness: number(settings, "roughness", 50.0),
                seed: settings["seed"].as_u64().unwrap_or(1) as u32,
            }
        }
        "Invert" => Adjustment::Invert,
        // Upstream's BlackWhiteSettings and ColorBalanceSettings (Document/ImageAdjustments.swift);
        // missing fields take upstream's defaults.
        "Black & White" => {
            let settings = &value["blackWhiteSettings"];
            let Adjustment::BlackWhite {
                weights: defaults, ..
            } = Adjustment::BLACK_WHITE
            else {
                unreachable!()
            };
            let keys = ["reds", "yellows", "greens", "cyans", "blues", "magentas"];
            Adjustment::BlackWhite {
                weights: std::array::from_fn(|i| number(settings, keys[i], defaults[i])),
                tint: settings["tint"].as_bool().unwrap_or(false),
                tint_hue: number(settings, "tintHue", 40.0),
                tint_saturation: number(settings, "tintSaturation", 20.0),
            }
        }
        "Color Balance" => {
            let settings = &value["colorBalanceSettings"];
            let tone = |prefix: &str| {
                ["CyanRed", "MagentaGreen", "YellowBlue"]
                    .map(|pair| number(settings, &format!("{prefix}{pair}"), 0.0))
            };
            Adjustment::ColorBalance {
                shadows: tone("shadow"),
                midtones: tone("mid"),
                highlights: tone("highlight"),
                preserve_luminosity: settings["preserveLuminosity"].as_bool().unwrap_or(true),
            }
        }
        "Gaussian Blur" | "Motion Blur" | "Add Noise" => {
            ensure!(version >= 9, "{kind} adjustments need project version 9");
            let filter = comp_sampling_adjustment(kind, value, report)?;
            filter.validate()?;
            return Ok(ImportedAdjustment::Filter(filter));
        }
        _ => bail!("Unsupported Compositor adjustment: {kind}"),
    };
    crate::effects::validate_adjustment(&result)?;
    Ok(ImportedAdjustment::Adjustment(result))
}

/// Version 9's blur and noise adjustments, checked against upstream's ranges and then brought
/// into the (narrower) ranges of Xuan's filters.
fn comp_sampling_adjustment(
    kind: &str,
    value: &Value,
    report: &mut ImportReport,
) -> Result<Filter> {
    let mut adapted = false;
    let mut limit = |v: f64, max: f64| {
        adapted |= v > max;
        v.min(max) as f32
    };
    let filter = match kind {
        "Gaussian Blur" => Filter::GaussianBlur {
            radius: limit(ranged(value, "blurRadius", 10.0, 0.1..=250.0)?, 100.0),
        },
        "Motion Blur" => {
            let angle = ranged(value, "motionAngle", 0.0, -90.0..=90.0)?;
            Filter::MotionBlur {
                distance: limit(ranged(value, "motionDistance", 10.0, 1.0..=2000.0)?, 200.0),
                // Upstream turns counterclockwise; Xuan's angle runs clockwise in image space.
                // A motion streak is symmetric, so negating it gives the same direction.
                angle: -angle as f32,
            }
        }
        _ => {
            let amount = limit(ranged(value, "noiseAmount", 10.0, 0.1..=400.0)?, 100.0);
            // Xuan's noise is uniform; a Gaussian distribution is approximated.
            adapted |= value["noiseGaussian"].as_bool() == Some(true);
            Filter::Noise {
                amount,
                monochrome: value["noiseMonochromatic"].as_bool().unwrap_or(false),
            }
        }
    };
    if adapted {
        report.add(Dropped::FilterSettings);
    }
    Ok(filter)
}

/// Font family, bold and italic from a PostScript name such as `HelveticaNeue-BoldItalic`.
fn font_from_postscript(name: &str) -> (String, bool, bool) {
    let (family, style) = name.split_once('-').unwrap_or((name, ""));
    let family = family
        .strip_suffix("PSMT")
        .or_else(|| family.strip_suffix("MT"))
        .unwrap_or(family);
    let mut spaced = String::with_capacity(family.len() + 4);
    let mut previous_lower = false;
    for c in family.chars() {
        if c.is_uppercase() && previous_lower {
            spaced.push(' ');
        }
        previous_lower = c.is_lowercase();
        spaced.push(c);
    }
    let style = style.to_ascii_lowercase();
    let bold = ["bold", "black", "heavy"].iter().any(|w| style.contains(w));
    let italic = style.contains("italic") || style.contains("oblique");
    (spaced, bold, italic)
}

/// Check upstream's `colorRuns` / `fontRuns`: sorted, not overlapping, non-empty, inside the text.
fn validate_runs(
    runs: &Value,
    units: usize,
    mut check: impl FnMut(&Value) -> Result<()>,
) -> Result<()> {
    let runs = runs.as_array().context("Invalid text runs")?;
    ensure!(!runs.is_empty() && runs.len() <= units, "Invalid text runs");
    let mut end = 0_u64;
    for run in runs {
        let location = run["location"].as_u64().context("Invalid text run")?;
        let length = run["length"].as_u64().context("Invalid text run")?;
        ensure!(location >= end && length > 0, "Overlapping text runs");
        end = location.checked_add(length).context("Invalid text run")?;
        check(run)?;
    }
    ensure!(end <= units as u64, "Text run outside the text");
    Ok(())
}

fn unit_color(value: &Value, key: &str) -> Result<u8> {
    Ok((ranged(value, key, 0.0, 0.0..=1.0)? * 255.0).round() as u8)
}

/// Upstream's editable text metadata, or `None` when Xuan keeps only the layer's pixels.
fn comp_text(value: &Value, version: u64, report: &mut ImportReport) -> Result<Option<TextStyle>> {
    ensure!(value.is_object(), "Invalid text metadata");
    let content = value["content"].as_str().unwrap_or("Text");
    let units = content.encode_utf16().count();
    ensure!(units <= MAX_TEXT_UNITS, "Text content is too long");
    let size = ranged(value, "fontSize", 72.0, 1.0..=2000.0)?;
    let color = [
        unit_color(value, "red")?,
        unit_color(value, "green")?,
        unit_color(value, "blue")?,
        255,
    ];
    let alignment = value["alignment"].as_str().unwrap_or("Left");
    ensure!(
        ["Left", "Center", "Right"].contains(&alignment),
        "Invalid text alignment"
    );
    let tracking = ranged(value, "tracking", 0.0, -100.0..=1000.0)?;
    let leading = ranged(value, "leading", 0.0, 0.0..=5000.0)?;
    let paragraph = &value["boxSize"];
    if !paragraph.is_null() {
        // CGSize is encoded as [width, height]; accept the keyed form too.
        let (w, h) = match paragraph.as_array() {
            Some(pair) if pair.len() == 2 => (pair[0].as_f64(), pair[1].as_f64()),
            _ => (paragraph["width"].as_f64(), paragraph["height"].as_f64()),
        };
        let (w, h) = (
            w.context("Invalid text box")?,
            h.context("Invalid text box")?,
        );
        ensure!(
            [w, h]
                .iter()
                .all(|v| v.is_finite() && (16.0..=MAX_BOX_SIDE).contains(v))
                && w * h <= MAX_BOX_AREA,
            "Invalid text box"
        );
    }
    let font_name = value["fontName"].as_str().unwrap_or("Helvetica");
    ensure!(font_name.len() <= 1024, "Invalid font name");
    if !value["colorRuns"].is_null() {
        ensure!(version >= 10, "Per-letter colors need project version 10");
        validate_runs(&value["colorRuns"], units, |run| {
            for key in ["red", "green", "blue"] {
                ensure!(run[key].is_number(), "Invalid text run color");
                unit_color(run, key)?;
            }
            Ok(())
        })?;
        report.add(Dropped::TextColors);
    }
    if !value["fontRuns"].is_null() {
        ensure!(version >= 11, "Per-letter fonts need project version 11");
        validate_runs(&value["fontRuns"], units, |run| {
            let name = run["fontName"].as_str().context("Invalid text run font")?;
            ensure!(
                !name.is_empty()
                    && name.chars().count() <= MAX_FONT_NAME
                    && !name.chars().any(|c| matches!(
                        c,
                        '\n' | '\r' | '\u{2028}' | '\u{2029}' | '\u{85}' | '\u{b}' | '\u{c}'
                    )),
                "Invalid text run font"
            );
            Ok(())
        })?;
        report.add(Dropped::TextFonts);
    }
    if alignment != "Left" || tracking != 0.0 || leading != 0.0 || !paragraph.is_null() {
        report.add(Dropped::TextLayout);
    }
    let (family, bold, italic) = font_from_postscript(font_name);
    let style = TextStyle {
        content: content.into(),
        family: if family.trim().is_empty() {
            TextStyle::default().family
        } else {
            family
        },
        size: size as f32,
        color,
        bold,
        italic,
        ..TextStyle::default()
    };
    if content.len() > MAX_TEXT_BYTES || style.validate().is_err() {
        report.add(Dropped::TextAsPixels);
        return Ok(None);
    }
    Ok(Some(style))
}

/// A layer's `effects` record (upstream's `LayerEffects`, Document/LayerEffects.swift): each
/// effect's settings, with upstream's defaults for missing fields and its ranges checked.
fn comp_effects(value: &Value) -> Result<LayerEffects> {
    use crate::layer_effects::{GlowEffect, OverlayEffect, ShadowEffect, StrokeEffect};
    let effects = value.as_object().context("Invalid layer effects")?;
    let record = |key: &str| -> Result<Option<&Value>> {
        match effects.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(record @ Value::Object(_)) => Ok(Some(record)),
            Some(_) => bail!("Invalid layer effect: {key}"),
        }
    };
    // Missing in older projects means visible.
    let enabled = |r: &Value| r["enabled"].as_bool().unwrap_or(true);
    let color = |r: &Value, default: f32| -> Result<[u8; 3]> {
        let mut rgb = [0; 3];
        for (channel, key) in rgb.iter_mut().zip(["red", "green", "blue"]) {
            let value = number(r, key, default);
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "Invalid layer effect color"
            );
            *channel = (value * 255.0).round() as u8;
        }
        Ok(rgb)
    };
    let shadow = |r: &Value, defaults: ShadowEffect| -> Result<ShadowEffect> {
        Ok(ShadowEffect {
            enabled: enabled(r),
            angle: number(r, "angle", defaults.angle),
            distance: number(r, "distance", defaults.distance),
            blur: number(r, "blur", defaults.blur),
            color: color(r, 0.0)?,
            opacity: number(r, "opacity", defaults.opacity),
        })
    };
    let glow = |r: &Value, defaults: GlowEffect| -> Result<GlowEffect> {
        Ok(GlowEffect {
            enabled: enabled(r),
            size: number(r, "size", defaults.size),
            color: color(r, 1.0)?,
            opacity: number(r, "opacity", defaults.opacity),
        })
    };
    let result = LayerEffects {
        stroke: match record("stroke")? {
            Some(r) => Some(StrokeEffect {
                enabled: enabled(r),
                size: number(r, "size", 4.0),
                color: color(r, 0.0)?,
                opacity: number(r, "opacity", 1.0),
                inside: r["inside"].as_bool().unwrap_or(false),
            }),
            None => None,
        },
        drop_shadow: match record("shadow")? {
            Some(r) => Some(shadow(r, ShadowEffect::DROP)?),
            None => None,
        },
        color_overlay: match record("colorOverlay")? {
            Some(r) => Some(OverlayEffect {
                enabled: enabled(r),
                color: color(r, 0.0)?,
                opacity: number(r, "opacity", 1.0),
            }),
            None => None,
        },
        inner_shadow: match record("innerShadow")? {
            Some(r) => Some(shadow(r, ShadowEffect::INNER)?),
            None => None,
        },
        outer_glow: match record("outerGlow")? {
            Some(r) => Some(glow(r, GlowEffect::OUTER)?),
            None => None,
        },
        inner_glow: match record("innerGlow")? {
            Some(r) => Some(glow(r, GlowEffect::INNER)?),
            None => None,
        },
    };
    result.validate()?;
    Ok(result)
}

/// Upstream stores a blend mode by its display name, which Xuan's `BlendMode::name` matches.
fn comp_blend(name: &str) -> Result<BlendMode> {
    BlendMode::ALL
        .into_iter()
        .find(|b| b.name() == name && !XUAN_ONLY_BLEND_MODES.contains(b))
        .with_context(|| format!("Unknown blend mode: {name}"))
}

fn comp_guides(value: &Value, version: u64) -> Result<Vec<Guide>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let records = value.as_array().context("Invalid guides")?;
    ensure!(
        version >= 8 || records.is_empty(),
        "Guides need project version 8"
    );
    ensure!(records.len() <= MAX_GUIDES, "Too many guides");
    let guides = records
        .iter()
        .map(|record| {
            let axis = match record["axis"].as_str() {
                Some("horizontal") => GuideAxis::Horizontal,
                Some("vertical") => GuideAxis::Vertical,
                _ => bail!("Invalid guide axis"),
            };
            let position = record["position"]
                .as_f64()
                .context("Missing guide position")?;
            ensure!(
                position.is_finite() && position.abs() <= f64::from(MAX_GUIDE_POSITION),
                "Invalid guide position"
            );
            Ok(Guide {
                id: identifier(&record["id"])?.context("Missing guide ID")?,
                axis,
                position: position as f32,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    crate::layout::validate_guides(&guides)?;
    Ok(guides)
}

/// Read a Compositor package, counting everything left out in the returned report.
pub fn load(path: &Path) -> Result<(Document, ImportReport)> {
    let manifest: Value = serde_json::from_slice(&package_read(
        path,
        Path::new("manifest.json"),
        MAX_MANIFEST,
    )?)?;
    ensure!(
        manifest["format"] == "com.compositor.project",
        "Not a Compositor project"
    );
    let version = manifest["version"]
        .as_u64()
        .context("Project version missing")?;
    ensure!(
        (1..=NEWEST_VERSION).contains(&version),
        "Unsupported Compositor project version {version} (Xuan reads 1–{NEWEST_VERSION})"
    );
    ensure!(
        manifest["colorSpace"].as_str().unwrap_or("sRGB") == "sRGB",
        "Unsupported color space"
    );
    let width = u32::try_from(manifest["width"].as_u64().context("Missing canvas width")?)?;
    let height = u32::try_from(
        manifest["height"]
            .as_u64()
            .context("Missing canvas height")?,
    )?;
    let mut report = ImportReport::default();
    let mut document = Document::new(width, height)?;
    document.id = identifier(&manifest["documentID"])?.context("Missing document ID")?;
    document.resolution = number(&manifest, "resolution", 72.0);
    // The layout grid is an app preference upstream, so packages carry no grid to import.
    document.guides = comp_guides(&manifest["guides"], version)?;
    document.layers.clear();
    let records = manifest["layers"].as_array().context("Missing layers")?;
    ensure!(
        records.len() <= crate::document::MAX_LAYERS,
        "Too many layers"
    );
    let mut image_pixels = 0;
    let mut mask_pixels = 0;
    for record in records {
        let id = identifier(&record["id"])?.context("Missing layer ID")?;
        let mut layer = Layer::blank(
            record["name"].as_str().context("Missing layer name")?,
            width,
            height,
        );
        layer.id = id;
        layer.visible = record["isVisible"].as_bool().unwrap_or(true);
        layer.transform = comp_transform(&record["transform"])?;
        layer.parent = identifier(&record["parentID"])?;
        layer.group = record["isGroup"].as_bool().unwrap_or(false);
        layer.opacity = number(record, "opacity", 1.0);
        layer.blend = comp_blend(record["blendMode"].as_str().unwrap_or("Normal"))?;
        if layer.group {
            // Folders are pass-through; their own opacity arrived in version 8.
            ensure!(
                layer.blend == BlendMode::Normal && (version >= 8 || layer.opacity == 1.0),
                "Invalid folder appearance"
            );
        }
        layer.clip_to = identifier(&record["maskSourceID"])?;
        let adjustment = if record["adjustment"].is_null() {
            None
        } else {
            ensure!(
                version >= 7 && !layer.group && record["imageFile"].is_null(),
                "Invalid adjustment layer"
            );
            Some(comp_adjustment(
                &record["adjustment"],
                version,
                &mut report,
            )?)
        };
        let effects = if record["effects"].is_null() {
            LayerEffects::default()
        } else {
            comp_effects(&record["effects"])?
        };
        if let Some(name) = record["imageFile"].as_str() {
            ensure!(
                name.eq_ignore_ascii_case(&format!("{id}.png")),
                "Unsafe layer asset path"
            );
            let bytes = package_read(path, &Path::new("images").join(name), MAX_ASSET)?;
            layer.pixels = Some(Arc::new(decode_image(bytes, &mut image_pixels)?.to_rgba8()));
        }
        if let Some(name) = record["maskFile"].as_str() {
            ensure!(
                name.eq_ignore_ascii_case(&format!("{id}.mask.png")),
                "Unsafe mask asset path"
            );
            let bytes = package_read(path, &Path::new("images").join(name), MAX_ASSET)?;
            layer.mask = Some(Mask {
                pixels: Arc::new(decode_image(bytes, &mut mask_pixels)?.to_luma8()),
                enabled: record["maskEnabled"].as_bool().unwrap_or(true),
                linked: record["maskLinked"].as_bool().unwrap_or(true),
                placement: if record["maskPlacement"].is_null() {
                    None
                } else {
                    Some(comp_transform(&record["maskPlacement"])?)
                },
            });
        }
        if !record["text"].is_null() {
            ensure!(
                layer.pixels.is_some() && !layer.group && adjustment.is_none(),
                "Invalid text layer"
            );
            layer.text = comp_text(&record["text"], version, &mut report)?;
        }
        if let Some(shape) = record["shape"].as_object()
            && layer.pixels.is_some()
            && layer.text.is_none()
        {
            let radius = shape
                .get("cornerRadius")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32;
            ensure!(radius.is_finite() && radius >= 0.0, "Invalid shape radius");
            let kind = match shape.get("kind").and_then(Value::as_str) {
                Some("Ellipse") => Some(crate::paint::ShapeKind::Ellipse),
                Some("Rectangle") if radius > 0.0 => {
                    Some(crate::paint::ShapeKind::RoundedRectangle)
                }
                Some("Rectangle") => Some(crate::paint::ShapeKind::Rectangle),
                // Xuan cannot redraw lines, so their pixels stay as they are.
                Some("Line") => None,
                _ => bail!("Invalid shape kind"),
            };
            let color =
                |key| (number(&record["shape"], key, 0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
            match kind {
                Some(kind) => {
                    layer.shape = Some(crate::document::ShapeStyle {
                        kind,
                        color: [color("red"), color("green"), color("blue"), 255],
                        corner_radius: radius,
                    })
                }
                None => report.add(Dropped::LineShape),
            }
        }
        match adjustment {
            Some(ImportedAdjustment::Adjustment(adjustment)) => layer.adjustment = Some(adjustment),
            Some(ImportedAdjustment::Filter(filter)) => {
                // Filter layers mix their result over the backdrop at their opacity.
                if layer.blend != BlendMode::Normal {
                    layer.blend = BlendMode::Normal;
                    report.add(Dropped::FilterSettings);
                }
                layer.filter = Some(filter);
            }
            None => {}
        }
        if !effects.is_empty() {
            if layer.group || layer.is_effect() {
                report.add(Dropped::LayerEffect);
            } else {
                layer.effects = Some(effects);
            }
        }
        document.layers.push(layer);
    }
    // Xuan's filter layers neither clip nor serve as clipping bases.
    let filters: HashSet<Uuid> = document
        .layers
        .iter()
        .filter(|l| l.filter.is_some())
        .map(|l| l.id)
        .collect();
    for layer in &mut document.layers {
        if let Some(base) = layer.clip_to
            && (layer.filter.is_some() || filters.contains(&base))
        {
            layer.clip_to = None;
            report.add(Dropped::ClippingMask);
        }
    }
    let ids: HashMap<Uuid, usize> = document
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l.id, i))
        .collect();
    let active = identifier(&manifest["activeLayerID"])?;
    ensure!(
        active.is_none_or(|id| ids.contains_key(&id)),
        "Missing active layer"
    );
    document.active = active
        .filter(|id| ids.contains_key(id))
        .or_else(|| document.layers.last().map(|l| l.id));
    document.selected = document.active.into_iter().collect();
    document.validate()?;
    Ok((document, report))
}

#[cfg(test)]
#[path = "compositor_tests.rs"]
mod tests;

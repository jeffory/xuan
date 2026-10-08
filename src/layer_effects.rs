//! Layer effects: stroke, drop shadow, color overlay, inner shadow, outer glow and inner glow.
//!
//! A port of upstream Compositor's `LayerEffects` (Document/LayerEffects.swift) and its GPU
//! renderer (Rendering/MetalLayerEffects.swift). Effects are stored on the layer and never touch
//! its pixels: each render draws the layer's pixels (through its own mask) with the effects
//! around them on a canvas grown by [`margin`] on every side, and that raster stands in for the
//! layer, so the layer's opacity, folders, blend mode and clipping apply to the whole result.
//! The raster is made on the GPU (`gpu::layer_effects`) when one is available, otherwise by
//! [`render_cpu`], with the same passes and arithmetic.

use std::sync::{Arc, Mutex, Weak};

use anyhow::{Result, ensure};
use image::{GrayImage, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::document::{Document, Layer, Mask, Point, Transform};

/// A line around what the layer shows, outside its edge or inside it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrokeEffect {
    pub enabled: bool,
    /// Width in layer pixels, 0–500.
    pub size: f32,
    pub color: [u8; 3],
    /// 0–1.
    pub opacity: f32,
    pub inside: bool,
}

impl Default for StrokeEffect {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 4.0,
            color: [0; 3],
            opacity: 1.0,
            inside: false,
        }
    }
}

/// A drop shadow (behind the layer) or an inner shadow (inside its edges).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShadowEffect {
    pub enabled: bool,
    /// Where the light comes from, in degrees counterclockwise from the right, as Photoshop's
    /// dial is: 90 is from straight above, which drops the shadow straight down. −360–360.
    pub angle: f32,
    /// 0–5000 layer pixels.
    pub distance: f32,
    /// Softness, 0–500 layer pixels (a Gaussian of half this standard deviation).
    pub blur: f32,
    pub color: [u8; 3],
    pub opacity: f32,
}

impl Default for ShadowEffect {
    fn default() -> Self {
        Self::DROP
    }
}

impl ShadowEffect {
    /// Upstream's drop shadow defaults.
    pub const DROP: Self = Self {
        enabled: true,
        angle: 90.0,
        distance: 20.0,
        blur: 20.0,
        color: [0; 3],
        opacity: 0.5,
    };
    /// Upstream's inner shadow defaults.
    pub const INNER: Self = Self {
        distance: 10.0,
        blur: 10.0,
        ..Self::DROP
    };

    /// Where the shadow falls, in layer pixels (y grows downward): away from the light.
    pub fn offset(&self) -> [f32; 2] {
        let (sin, cos) = self.angle.to_radians().sin_cos();
        [-cos * self.distance, sin * self.distance]
    }
}

/// A flat color over everything the layer shows.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayEffect {
    pub enabled: bool,
    pub color: [u8; 3],
    pub opacity: f32,
}

impl Default for OverlayEffect {
    fn default() -> Self {
        Self {
            enabled: true,
            color: [0; 3],
            opacity: 1.0,
        }
    }
}

/// A soft glow around the outside of the layer, or inward from its edges.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlowEffect {
    pub enabled: bool,
    /// 0–500 layer pixels.
    pub size: f32,
    pub color: [u8; 3],
    pub opacity: f32,
}

impl Default for GlowEffect {
    fn default() -> Self {
        Self::OUTER
    }
}

impl GlowEffect {
    pub const OUTER: Self = Self {
        enabled: true,
        size: 20.0,
        color: [255; 3],
        opacity: 0.75,
    };
    pub const INNER: Self = Self {
        size: 10.0,
        ..Self::OUTER
    };
}

/// What a layer draws around itself. `None` is an effect the layer does not have; a present but
/// disabled effect keeps its settings and draws nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerEffects {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke: Option<StrokeEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_shadow: Option<ShadowEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_overlay: Option<OverlayEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_shadow: Option<ShadowEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outer_glow: Option<GlowEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_glow: Option<GlowEffect>,
}

/// The six effects, in upstream's order (`LayerEffectKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    Stroke,
    DropShadow,
    ColorOverlay,
    InnerShadow,
    OuterGlow,
    InnerGlow,
}

impl EffectKind {
    pub const ALL: [Self; 6] = [
        Self::Stroke,
        Self::DropShadow,
        Self::ColorOverlay,
        Self::InnerShadow,
        Self::OuterGlow,
        Self::InnerGlow,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Stroke => "Stroke",
            Self::DropShadow => "Drop Shadow",
            Self::ColorOverlay => "Color Overlay",
            Self::InnerShadow => "Inner Shadow",
            Self::OuterGlow => "Outer Glow",
            Self::InnerGlow => "Inner Glow",
        }
    }
}

fn valid_color_opacity(opacity: f32) -> bool {
    opacity.is_finite() && (0.0..=1.0).contains(&opacity)
}

fn within(value: f32, low: f32, high: f32) -> bool {
    value.is_finite() && (low..=high).contains(&value)
}

impl LayerEffects {
    pub fn is_empty(&self) -> bool {
        EffectKind::ALL.iter().all(|kind| !self.contains(*kind))
    }

    pub fn contains(&self, kind: EffectKind) -> bool {
        match kind {
            EffectKind::Stroke => self.stroke.is_some(),
            EffectKind::DropShadow => self.drop_shadow.is_some(),
            EffectKind::ColorOverlay => self.color_overlay.is_some(),
            EffectKind::InnerShadow => self.inner_shadow.is_some(),
            EffectKind::OuterGlow => self.outer_glow.is_some(),
            EffectKind::InnerGlow => self.inner_glow.is_some(),
        }
    }

    pub fn is_enabled(&self, kind: EffectKind) -> bool {
        match kind {
            EffectKind::Stroke => self.stroke.is_some_and(|e| e.enabled),
            EffectKind::DropShadow => self.drop_shadow.is_some_and(|e| e.enabled),
            EffectKind::ColorOverlay => self.color_overlay.is_some_and(|e| e.enabled),
            EffectKind::InnerShadow => self.inner_shadow.is_some_and(|e| e.enabled),
            EffectKind::OuterGlow => self.outer_glow.is_some_and(|e| e.enabled),
            EffectKind::InnerGlow => self.inner_glow.is_some_and(|e| e.enabled),
        }
    }

    /// Add `kind` with upstream's defaults, unless the layer already has it. `color` is used
    /// for a new stroke or color overlay (upstream takes the background color).
    pub fn add(&mut self, kind: EffectKind, color: [u8; 3]) {
        match kind {
            EffectKind::Stroke => {
                self.stroke.get_or_insert(StrokeEffect {
                    color,
                    ..StrokeEffect::default()
                });
            }
            EffectKind::DropShadow => {
                self.drop_shadow.get_or_insert(ShadowEffect::DROP);
            }
            EffectKind::ColorOverlay => {
                self.color_overlay.get_or_insert(OverlayEffect {
                    color,
                    ..OverlayEffect::default()
                });
            }
            EffectKind::InnerShadow => {
                self.inner_shadow.get_or_insert(ShadowEffect::INNER);
            }
            EffectKind::OuterGlow => {
                self.outer_glow.get_or_insert(GlowEffect::OUTER);
            }
            EffectKind::InnerGlow => {
                self.inner_glow.get_or_insert(GlowEffect::INNER);
            }
        }
    }

    pub fn remove(&mut self, kind: EffectKind) {
        match kind {
            EffectKind::Stroke => self.stroke = None,
            EffectKind::DropShadow => self.drop_shadow = None,
            EffectKind::ColorOverlay => self.color_overlay = None,
            EffectKind::InnerShadow => self.inner_shadow = None,
            EffectKind::OuterGlow => self.outer_glow = None,
            EffectKind::InnerGlow => self.inner_glow = None,
        }
    }

    pub fn set_enabled(&mut self, kind: EffectKind, enabled: bool) {
        match kind {
            EffectKind::Stroke => self.stroke.iter_mut().for_each(|e| e.enabled = enabled),
            EffectKind::DropShadow => self
                .drop_shadow
                .iter_mut()
                .for_each(|e| e.enabled = enabled),
            EffectKind::ColorOverlay => self
                .color_overlay
                .iter_mut()
                .for_each(|e| e.enabled = enabled),
            EffectKind::InnerShadow => self
                .inner_shadow
                .iter_mut()
                .for_each(|e| e.enabled = enabled),
            EffectKind::OuterGlow => self.outer_glow.iter_mut().for_each(|e| e.enabled = enabled),
            EffectKind::InnerGlow => self.inner_glow.iter_mut().for_each(|e| e.enabled = enabled),
        }
    }

    /// Upstream's ranges (the `isValid` of each effect in Document/LayerEffects.swift).
    pub fn validate(&self) -> Result<()> {
        let valid = self
            .stroke
            .is_none_or(|e| within(e.size, 0.0, 500.0) && valid_color_opacity(e.opacity))
            && [self.drop_shadow, self.inner_shadow]
                .into_iter()
                .flatten()
                .all(|e| {
                    within(e.angle, -360.0, 360.0)
                        && within(e.distance, 0.0, 5000.0)
                        && within(e.blur, 0.0, 500.0)
                        && valid_color_opacity(e.opacity)
                })
            && self
                .color_overlay
                .is_none_or(|e| valid_color_opacity(e.opacity))
            && [self.outer_glow, self.inner_glow]
                .into_iter()
                .flatten()
                .all(|e| within(e.size, 0.0, 500.0) && valid_color_opacity(e.opacity));
        ensure!(valid, "Invalid layer effect settings");
        Ok(())
    }

    /// Only the effects that draw something: enabled, visible and of non-zero size.
    pub fn visible(&self) -> Self {
        Self {
            stroke: self
                .stroke
                .filter(|e| e.enabled && e.size > 0.0 && e.opacity > 0.0),
            drop_shadow: self.drop_shadow.filter(|e| e.enabled && e.opacity > 0.0),
            color_overlay: self.color_overlay.filter(|e| e.enabled && e.opacity > 0.0),
            inner_shadow: self.inner_shadow.filter(|e| e.enabled && e.opacity > 0.0),
            outer_glow: self
                .outer_glow
                .filter(|e| e.enabled && e.size > 0.0 && e.opacity > 0.0),
            inner_glow: self
                .inner_glow
                .filter(|e| e.enabled && e.size > 0.0 && e.opacity > 0.0),
        }
    }
}

/// The room effects need around the layer, in layer pixels (upstream's `margin(for:)`).
pub fn margin(effects: &LayerEffects) -> u32 {
    let effects = effects.visible();
    let mut margin: f32 = 0.0;
    if let Some(stroke) = effects.stroke
        && !stroke.inside
    {
        margin = margin.max(stroke.size);
    }
    if let Some(shadow) = effects.drop_shadow {
        margin = margin.max(shadow.distance + shadow.blur * 3.0);
    }
    if let Some(glow) = effects.outer_glow {
        margin = margin.max(glow.size * 3.0);
    }
    margin.ceil() as u32 + 2
}

/// The settings of one render, as plain numbers shared by the CPU and GPU passes.
pub(crate) struct Plan {
    /// Reach in pixels and whether the stroke is inside.
    pub stroke: Option<(u32, bool, [f32; 4])>,
    /// Offset, blur sigma and color for each shadow.
    pub drop_shadow: Option<([f32; 2], f32, [f32; 4])>,
    pub inner_shadow: Option<([f32; 2], f32, [f32; 4])>,
    pub overlay: Option<[f32; 4]>,
    /// Blur sigma and color for each glow.
    pub outer_glow: Option<(f32, [f32; 4])>,
    pub inner_glow: Option<(f32, [f32; 4])>,
}

fn color(rgb: [u8; 3], opacity: f32) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        opacity,
    ]
}

impl Plan {
    pub(crate) fn new(effects: &LayerEffects) -> Self {
        let effects = effects.visible();
        Self {
            stroke: effects.stroke.map(|e| {
                (
                    (e.size.round() as u32).max(1),
                    e.inside,
                    color(e.color, e.opacity),
                )
            }),
            drop_shadow: effects
                .drop_shadow
                .map(|e| (e.offset(), e.blur / 2.0, color(e.color, e.opacity))),
            inner_shadow: effects
                .inner_shadow
                .map(|e| (e.offset(), e.blur / 2.0, color(e.color, e.opacity))),
            overlay: effects.color_overlay.map(|e| color(e.color, e.opacity)),
            outer_glow: effects
                .outer_glow
                .map(|e| (e.size / 2.0, color(e.color, e.opacity))),
            inner_glow: effects
                .inner_glow
                .map(|e| (e.size / 2.0, color(e.color, e.opacity))),
        }
    }
}

/// The Gaussian's half width for `sigma`, as upstream: three standard deviations.
pub(crate) fn blur_radius(sigma: f32) -> u32 {
    ((sigma * 3.0).round() as u32).max(1)
}

/// Whether a blur of `sigma` does anything (upstream skips anything at or below 0.01).
pub(crate) fn blurs(sigma: f32) -> bool {
    sigma > 0.01
}

fn blur(plane: &[f32], width: usize, height: usize, sigma: f32) -> Vec<f32> {
    if !blurs(sigma) {
        return plane.to_vec();
    }
    let radius = blur_radius(sigma) as i64;
    let weights: Vec<f32> = (-radius..=radius)
        .map(|o| (-((o * o) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let pass = |source: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0; source.len()];
        out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            for (x, target) in row.iter_mut().enumerate() {
                let (mut total, mut sum) = (0.0f32, 0.0f32);
                for (k, weight) in weights.iter().enumerate() {
                    let offset = k as i64 - radius;
                    let value = if horizontal {
                        let sx = (x as i64 + offset).clamp(0, width as i64 - 1) as usize;
                        source[y * width + sx]
                    } else {
                        let sy = (y as i64 + offset).clamp(0, height as i64 - 1) as usize;
                        source[sy * width + x]
                    };
                    total += weight * value;
                    sum += weight;
                }
                *target = total / sum;
            }
        });
        out
    };
    pass(&pass(plane, true), false)
}

/// The shape moved by `offset`, sampled between pixels so it moves smoothly.
fn shift(plane: &[f32], width: usize, height: usize, offset: [f32; 2]) -> Vec<f32> {
    let mut out = vec![0.0; plane.len()];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, target) in row.iter_mut().enumerate() {
            let sx = x as f32 - offset[0];
            let sy = y as f32 - offset[1];
            if sx < 0.0 || sy < 0.0 || sx > (width - 1) as f32 || sy > (height - 1) as f32 {
                continue;
            }
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let top = mix(plane[y0 * width + x0], plane[y0 * width + x1], fx);
            let bottom = mix(plane[y1 * width + x0], plane[y1 * width + x1], fx);
            *target = mix(top, bottom, fy);
        }
    });
    out
}

/// The largest (or smallest) value within `reach` along each row and then each column; past
/// the edge there is nothing (zero). Sliding-window passes, so the cost does not grow with the
/// reach, as upstream's CPU `extreme`.
fn spread(plane: &[f32], width: usize, height: usize, reach: usize, smallest: bool) -> Vec<f32> {
    fn line(input: &[f32], output: &mut [f32], reach: usize, smallest: bool) {
        let count = input.len();
        let mut queue = std::collections::VecDeque::with_capacity(count);
        let mut next = 0;
        for (center, target) in output.iter_mut().enumerate() {
            while next < count && next <= center + reach {
                let value = input[next];
                while queue.back().is_some_and(|&i: &usize| {
                    let previous = input[i];
                    if smallest {
                        previous >= value
                    } else {
                        previous <= value
                    }
                }) {
                    queue.pop_back();
                }
                queue.push_back(next);
                next += 1;
            }
            while queue.front().is_some_and(|&i| i + reach < center) {
                queue.pop_front();
            }
            let outside = center < reach || center + reach >= count;
            *target = if smallest && outside {
                0.0
            } else {
                input[*queue.front().unwrap()]
            };
        }
    }
    let mut rows = vec![0.0; plane.len()];
    rows.par_chunks_mut(width)
        .zip(plane.par_chunks(width))
        .for_each(|(out, input)| line(input, out, reach, smallest));
    // Columns, through a transposed copy.
    let mut columns = vec![0.0; plane.len()];
    let transposed: Vec<f32> = (0..width * height)
        .map(|i| rows[(i % height) * width + i / height])
        .collect();
    let mut result = vec![0.0; plane.len()];
    columns
        .par_chunks_mut(height)
        .zip(transposed.par_chunks(height))
        .for_each(|(out, input)| line(input, out, reach, smallest));
    for (i, value) in columns.into_iter().enumerate() {
        result[(i % height) * width + i / height] = value;
    }
    result
}

/// Draws `padded` (the layer's pixels as they are shown, with room around them) with `effects`,
/// on the CPU: upstream's `effects_compose` order — the drop shadow behind, the outer glow over
/// it, an outside stroke over that, the pixels, then the color overlay, inner glow, inner shadow
/// and an inside stroke on top. `gpu/layer_effects.wgsl` runs the same passes.
pub fn render_cpu(padded: &RgbaImage, effects: &LayerEffects) -> RgbaImage {
    let (width, height) = padded.dimensions();
    let plan = Plan::new(effects);
    let (w, h) = (width as usize, height as usize);
    let shape: Vec<f32> = padded.pixels().map(|p| p[3] as f32 / 255.0).collect();
    let ring = plan.stroke.map(|(reach, inside, _)| {
        let moved = spread(&shape, w, h, reach as usize, inside);
        shape
            .iter()
            .zip(moved)
            .map(|(&s, m)| (if inside { s - m } else { m - s }).clamp(0.0, 1.0))
            .collect::<Vec<_>>()
    });
    let drop_shadow = plan
        .drop_shadow
        .map(|(offset, sigma, _)| blur(&shift(&shape, w, h, offset), w, h, sigma));
    let inner_shadow = plan.inner_shadow.map(|(offset, sigma, _)| {
        let moved = blur(&shift(&shape, w, h, offset), w, h, sigma);
        shape
            .iter()
            .zip(moved)
            .map(|(&s, m)| (s * (1.0 - m)).clamp(0.0, 1.0))
            .collect::<Vec<_>>()
    });
    let outer_glow = plan.outer_glow.map(|(sigma, _)| blur(&shape, w, h, sigma));
    let inner_glow = plan.inner_glow.map(|(sigma, _)| {
        let blurred = blur(&shape, w, h, sigma);
        shape
            .iter()
            .zip(blurred)
            .map(|(&s, b)| (s * (1.0 - b)).clamp(0.0, 1.0))
            .collect::<Vec<_>>()
    });
    let mut output = RgbaImage::new(width, height);
    output
        .as_mut()
        .par_chunks_exact_mut(4)
        .enumerate()
        .for_each(|(i, target)| {
            let raw = &padded.as_raw()[i * 4..i * 4 + 4];
            let p = [raw[0], raw[1], raw[2], raw[3]].map(|v| v as f32 / 255.0);
            let over = |color: &mut [f32; 3], alpha: &mut f32, paint: [f32; 4], coverage: f32| {
                let coverage = (coverage * paint[3]).clamp(0.0, 1.0);
                for c in 0..3 {
                    color[c] = paint[c] * coverage + color[c] * (1.0 - coverage);
                }
                *alpha = coverage + *alpha * (1.0 - coverage);
            };
            let mut color = [0.0f32; 3];
            let mut alpha = 0.0f32;
            if let (Some(plane), Some((_, _, paint))) = (&drop_shadow, plan.drop_shadow) {
                over(&mut color, &mut alpha, paint, plane[i]);
            }
            if let (Some(plane), Some((_, paint))) = (&outer_glow, plan.outer_glow) {
                over(&mut color, &mut alpha, paint, plane[i] * (1.0 - shape[i]));
            }
            if let (Some(plane), Some((_, false, paint))) = (&ring, plan.stroke) {
                over(&mut color, &mut alpha, paint, plane[i]);
            }
            // A color overlay recolors the layer's own pixels and keeps their alpha, as Photoshop
            // does, so a semi-transparent pixel stays as transparent as it was.
            let mut face = [p[0], p[1], p[2]];
            if let Some(paint) = plan.overlay {
                for c in 0..3 {
                    face[c] = face[c] + (paint[c] - face[c]) * paint[3].clamp(0.0, 1.0);
                }
            }
            for c in 0..3 {
                color[c] = face[c] * p[3] + color[c] * (1.0 - p[3]);
            }
            alpha = p[3] + alpha * (1.0 - p[3]);
            if let (Some(plane), Some((_, paint))) = (&inner_glow, plan.inner_glow) {
                over(&mut color, &mut alpha, paint, plane[i]);
            }
            if let (Some(plane), Some((_, _, paint))) = (&inner_shadow, plan.inner_shadow) {
                over(&mut color, &mut alpha, paint, plane[i]);
            }
            if let (Some(plane), Some((_, true, paint))) = (&ring, plan.stroke) {
                over(&mut color, &mut alpha, paint, plane[i]);
            }
            target.copy_from_slice(&straight(color, alpha));
        });
    output
}

/// Premultiplied floats to straight 8-bit RGBA, rounded as the shaders' `packed` does.
pub(crate) fn straight(color: [f32; 3], alpha: f32) -> [u8; 4] {
    let alpha = alpha.clamp(0.0, 1.0);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
    if alpha <= 0.0 {
        return [0; 4];
    }
    [
        byte(color[0] / alpha),
        byte(color[1] / alpha),
        byte(color[2] / alpha),
        byte(alpha),
    ]
}

/// `pixels` placed with `margin` transparent pixels on every side.
pub fn pad(pixels: &RgbaImage, margin: u32) -> RgbaImage {
    let mut padded = RgbaImage::new(pixels.width() + margin * 2, pixels.height() + margin * 2);
    image::imageops::replace(&mut padded, pixels, margin as i64, margin as i64);
    padded
}

/// `pixels` with `effects` around them, and the margin added on every side. `None` when nothing
/// would be drawn or the raster would exceed the document limits, so the layer draws as it is.
pub fn render(pixels: &RgbaImage, effects: &LayerEffects) -> Option<(RgbaImage, u32)> {
    let visible = effects.visible();
    if visible.is_empty() || visible.validate().is_err() {
        return None;
    }
    let margin = margin(&visible);
    let (width, height) = (pixels.width() + margin * 2, pixels.height() + margin * 2);
    // The effect raster is limited like any other layer.
    if crate::document::validate_size(width, height).is_err() {
        return None;
    }
    let padded = pad(pixels, margin);
    let result = crate::gpu::layer_effects(&padded, &visible)
        .unwrap_or_else(|| render_cpu(&padded, &visible));
    Some((result, margin))
}

/// The layer's pixels through its own mask, in its pixel grid (upstream's `masked`).
fn shown(layer: &Layer, pixels: &RgbaImage) -> RgbaImage {
    let Some(mask) = layer.mask.as_ref().filter(|m| m.enabled) else {
        return pixels.clone();
    };
    let (width, height) = pixels.dimensions();
    let mut result = pixels.clone();
    result
        .as_mut()
        .par_chunks_exact_mut(4)
        .enumerate()
        .for_each(|(index, pixel)| {
            let point = layer.transform.point(Point::new(
                ((index as u32 % width) as f32 + 0.5) / width as f32,
                ((index as u32 / width) as f32 + 0.5) / height as f32,
            ));
            let amount = crate::render::mask_sample(
                &mask.pixels,
                mask.placement.unwrap_or(layer.transform).inverse(point),
            );
            pixel[3] = (pixel[3] as f32 * amount).round() as u8;
        });
    result
}

/// Recent results, keyed by the pixels and mask they were made from and the settings, so the
/// canvas does not rebuild them on every redraw (upstream keeps a similar cache).
struct Entry {
    pixels: Weak<RgbaImage>,
    mask: Option<(Weak<GrayImage>, Option<Transform>, Transform)>,
    effects: LayerEffects,
    result: Arc<RgbaImage>,
    margin: u32,
}

static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
const CACHE_ENTRIES: usize = 8;
const CACHE_BYTES: usize = 256 * 1024 * 1024;

fn mask_key(layer: &Layer) -> Option<(Weak<GrayImage>, Option<Transform>, Transform)> {
    layer
        .mask
        .as_ref()
        .filter(|m| m.enabled)
        .map(|m: &Mask| (Arc::downgrade(&m.pixels), m.placement, layer.transform))
}

fn same_mask(
    a: &Option<(Weak<GrayImage>, Option<Transform>, Transform)>,
    b: &Option<(Weak<GrayImage>, Option<Transform>, Transform)>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some((a, ap, at)), Some((b, bp, bt))) => {
            a.ptr_eq(b)
                && a.strong_count() > 0
                && ap == bp
                // A linked mask moves with the layer, so its placement relative to the pixels
                // only changes when it has a placement of its own.
                && (ap.is_none() || at == bt)
        }
        _ => false,
    }
}

/// The layer drawn with its effects, as a layer that stands in for it: the grown raster, a
/// transform grown by the margin, and no mask (it is already applied). `None` when the layer
/// has no visible effects.
pub fn apply(layer: &Layer) -> Option<Layer> {
    let effects = layer.effects.as_ref()?;
    let pixels = layer.pixels.as_ref()?;
    if layer.group || layer.is_effect() || effects.visible().is_empty() {
        return None;
    }
    let mask = mask_key(layer);
    let visible = effects.visible();
    let cached = {
        let cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        cache
            .iter()
            .find(|e| {
                e.pixels.as_ptr() == Arc::as_ptr(pixels)
                    && e.pixels.strong_count() > 0
                    && same_mask(&e.mask, &mask)
                    && e.effects == visible
            })
            .map(|e| (e.result.clone(), e.margin))
    };
    let (result, margin) = match cached {
        Some(hit) => hit,
        None => {
            let (image, margin) = render(&shown(layer, pixels), &visible)?;
            let result = Arc::new(image);
            let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
            cache.retain(|e| e.pixels.strong_count() > 0);
            cache.push(Entry {
                pixels: Arc::downgrade(pixels),
                mask,
                effects: visible,
                result: result.clone(),
                margin,
            });
            let bytes = |e: &Entry| e.result.as_raw().len();
            while cache.len() > CACHE_ENTRIES
                || (cache.len() > 1 && cache.iter().map(bytes).sum::<usize>() > CACHE_BYTES)
            {
                cache.remove(0);
            }
            (result, margin)
        }
    };
    let (width, height) = (pixels.width() as f32, pixels.height() as f32);
    let (mx, my) = (margin as f32 / width, margin as f32 / height);
    let mut drawn = layer.clone();
    drawn.transform = if layer.transform.warp.is_none() {
        // Grown evenly about the same center: rotation and flips are unchanged, so
        // upstream's `placed` (scale the size, keep the center) is exact.
        let center = layer.transform.center();
        let mut grown = layer.transform;
        grown.width *= 1.0 + 2.0 * mx;
        grown.height *= 1.0 + 2.0 * my;
        grown.x = center.x - grown.width * 0.5;
        grown.y = center.y - grown.height * 0.5;
        grown
    } else {
        layer.transform.expanded(-mx, -my, 1.0 + mx, 1.0 + my)
    };
    drawn.pixels = Some(result);
    drawn.mask = None;
    drawn.effects = None;
    Some(drawn)
}

/// Replace every layer that has visible effects with its effected raster (see [`apply`]).
/// Returns `None` when no layer has any.
pub fn prepare(document: &Document) -> Option<Document> {
    if !document
        .layers
        .iter()
        .any(|l| l.effects.as_ref().is_some_and(|e| !e.visible().is_empty()))
    {
        return None;
    }
    let mut prepared = document.clone();
    for layer in &mut prepared.layers {
        if let Some(drawn) = apply(layer) {
            *layer = drawn;
        }
    }
    Some(prepared)
}

#[cfg(test)]
mod tests;

//! Camera Raw Filter: the Develop pipeline run on an ordinary 8-bit sRGB layer.
//!
//! The layer's pixels become linear sRGB "camera" values with an identity colour matrix and a
//! neutral as-shot white balance, so Develop's defaults leave them as they were and its
//! temperature control shifts the colour relative to daylight (D65).
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, ensure};
use image::{GrayImage, Rgb32FImage, RgbaImage};
use rawler::imgop::{matrix::pseudo_inverse, xyz::SRGB_TO_XYZ_D65};

use super::{DecodedRaw, DevelopSettings, NegativeSettings, RawMetadata};
use crate::document::{Layer, Transform, validate_size};

const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Temperature white balance measures a light's colour through this matrix: sRGB, with its
/// channels scaled so that 6500 K is exactly neutral. Temperature then shifts the colour
/// relative to the layer as it is, and lower temperatures cool the image, as in RAW Develop.
fn filter_xyz_to_camera() -> [[f32; 3]; 3] {
    let srgb = pseudo_inverse(SRGB_TO_XYZ_D65);
    let gains = super::process::temperature_gains(srgb, DAYLIGHT);
    std::array::from_fn(|row| srgb[row].map(|v| v * gains[row]))
}

/// The temperature at which the Camera Raw Filter's white balance changes nothing.
const DAYLIGHT: f32 = 6500.0;

/// The sRGB transfer function's inverse, the counterpart of the encoding in `process::tone`.
fn linear(value: u8) -> f32 {
    let v = f32::from(value) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

impl DevelopSettings {
    /// The Camera Raw Filter's starting point: every control neutral, so Apply without
    /// changes leaves the layer as it was. Unlike a RAW file, an 8-bit layer has already been
    /// sharpened and denoised, so those start at zero too.
    pub fn camera_raw_filter() -> Self {
        Self {
            color_noise: 0.0,
            sharpen: 0.0,
            ..Self::default()
        }
    }

    /// Whether the settings keep the layer's size and placement, as the Camera Raw Filter
    /// needs: no crop, rotation, perspective, distortion or film negative conversion.
    pub fn keeps_geometry(&self) -> bool {
        let neutral = Self::default();
        !self.negative.enabled
            && self.crop == neutral.crop
            && self.quarter_turns == 0
            && self.rotation == 0.0
            && self.perspective == neutral.perspective
            && self.distortion == 0.0
    }

    /// These settings with the controls the Camera Raw Filter does not have reset, for
    /// settings loaded from a file that was saved in RAW Develop.
    pub fn without_geometry(mut self) -> Self {
        let neutral = Self::default();
        self.negative = NegativeSettings::default();
        self.crop = neutral.crop;
        self.quarter_turns = 0;
        self.rotation = 0.0;
        self.perspective = neutral.perspective;
        self.distortion = 0.0;
        self
    }
}

/// Whether the Camera Raw Filter can change `layer`: an unlocked layer with pixels. RAW
/// layers open in Develop instead; folders, masks, adjustment and filter layers have no
/// pixels of their own to change. Text and shapes are rasterized, as other filters do.
pub fn can_filter(layer: &Layer) -> bool {
    !layer.locked && layer.pixels.is_some() && layer.raw.is_none() && layer.can_attach_effects()
}

/// The layer's pixels as Develop's input: linear sRGB with an identity colour matrix.
/// Transparency is not part of it; [`render_filter`] restores the layer's alpha.
pub fn filter_source(pixels: &RgbaImage) -> Result<DecodedRaw> {
    let (width, height) = pixels.dimensions();
    validate_size(width, height)?;
    let table: [f32; 256] = std::array::from_fn(|v| linear(v as u8));
    let camera = Rgb32FImage::from_raw(
        width,
        height,
        pixels
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                [
                    table[p[0] as usize],
                    table[p[1] as usize],
                    table[p[2] as usize],
                ]
            })
            .collect(),
    )
    .unwrap();
    Ok(DecodedRaw {
        camera,
        as_shot: [1.0; 3],
        camera_to_rgb: IDENTITY,
        xyz_to_camera: filter_xyz_to_camera(),
        metadata: RawMetadata {
            camera: String::new(),
            lens: String::new(),
            iso: None,
            aperture: None,
            shutter: None,
            focal_length: None,
            width,
            height,
            bits: 8,
        },
    })
}

/// Develop `source` (from [`filter_source`] on `original`) with `settings`, keeping the
/// layer's alpha. Fully transparent pixels keep their original values.
pub fn render_filter(
    source: &DecodedRaw,
    original: &RgbaImage,
    settings: &DevelopSettings,
    cancel: &AtomicBool,
) -> Result<RgbaImage> {
    ensure!(
        settings.keeps_geometry(),
        "The Camera Raw Filter cannot crop, rotate or reshape a layer"
    );
    ensure!(
        source.camera.dimensions() == original.dimensions(),
        "The Camera Raw Filter source does not match the layer"
    );
    let mut result = super::render(source, settings, cancel)?;
    ensure!(
        result.dimensions() == original.dimensions(),
        "The Camera Raw Filter changed the layer's size"
    );
    for (pixel, old) in result.pixels_mut().zip(original.pixels()) {
        if old[3] == 0 {
            *pixel = *old;
        } else {
            pixel[3] = old[3];
        }
    }
    Ok(result)
}

/// Keep the filtered `result` only where `selection` covers the layer placed at
/// `transform`, and the `original` pixels elsewhere.
pub fn within_selection(
    result: &mut RgbaImage,
    original: &RgbaImage,
    transform: Transform,
    selection: &GrayImage,
    cancel: &AtomicBool,
) -> Result<()> {
    ensure!(
        result.dimensions() == original.dimensions(),
        "The Camera Raw Filter changed the layer's size"
    );
    crate::effects::blend_selection(result, original, transform, selection, 0, cancel)
}

/// Replace the layer's pixels with the filtered `result`. `source` and `transform` are the
/// pixels and placement the result was made from: if the layer has changed since, nothing
/// is replaced.
pub fn apply_filter(
    layer: &mut Layer,
    source: &Arc<RgbaImage>,
    transform: Transform,
    result: RgbaImage,
) -> Result<()> {
    ensure!(can_filter(layer), "Select an unlocked pixel layer");
    ensure!(
        layer
            .pixels
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(p, source))
            && layer.transform == transform,
        "The layer changed while the Camera Raw Filter was open"
    );
    ensure!(
        result.dimensions() == source.dimensions(),
        "The Camera Raw Filter changed the layer's size"
    );
    layer.text = None;
    layer.shape = None;
    layer.pixels = Some(Arc::new(result));
    Ok(())
}

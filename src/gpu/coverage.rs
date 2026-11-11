use super::{
    Processor,
    processor::attempt,
    raster::{padded, transform_config},
};
use crate::{
    document::{Document, Layer, Transform},
    render,
};
use anyhow::{Result, ensure};
use std::{collections::HashMap, sync::Arc};

/// Coverage is a small stack program over sampled sources, evaluated per pixel by
/// `coverage.wgsl`. Each instruction takes six config rows: a sampled source's header and
/// transform, with the operation in row 3's `w` and its parameter in row 4's `w`. A plain
/// chain of layers only multiplies (`MUL_SAMPLE`, with opacities folded into the starting
/// value); a folder clipping base pushes its children's alphas and combines them as
/// [`crate::render::group_alpha`] does.
struct Coverage {
    config: Vec<[f32; 4]>,
    pixels: Vec<u8>,
    /// Sources already in `pixels`, by address and kind, so a base used twice is uploaded once.
    offsets: HashMap<(usize, bool), u32>,
    depth: usize,
    max_depth: usize,
}

/// Multiply the top of the stack by a sampled source.
const MUL_SAMPLE: f32 = 0.0;
/// Push the parameter.
const PUSH: f32 = 1.0;
/// Pop `a` and set the top to `a + top * (1 - a)`.
const UNION: f32 = 2.0;
/// Multiply the top by the parameter.
const MUL_SCALAR: f32 = 3.0;
/// Multiply the top by `1 - parameter * (1 - sample)`: a mask layer inside a folder.
const FADE_SAMPLE: f32 = 4.0;
/// Pop `a` and multiply the top by it.
const MUL: f32 = 5.0;
/// The stack size in `coverage.wgsl`.
const STACK: usize = 32;
/// Longer programs are left to the CPU.
const MAX_INSTRUCTIONS: usize = 4096;

impl Coverage {
    fn new(document: &Document, layer: &Layer, size: [u32; 2], mode: CoverageMode) -> Self {
        let stride = size[0].div_ceil(256) * 256;
        let mut coverage = Self {
            config: vec![
                [
                    size[0] as f32,
                    size[1] as f32,
                    document.width as f32,
                    document.height as f32,
                ],
                [1.0, 0.0, stride as f32, 0.0],
            ],
            pixels: Vec::new(),
            offsets: HashMap::new(),
            depth: 0,
            max_depth: 0,
        };
        if mode == CoverageMode::Composite && !layer.standalone_mask {
            coverage.config[1][0] = if layer.visible { 1.0 } else { 0.0 };
            let mut parent = layer.parent;
            for _ in 0..64 {
                let Some(group) = parent.and_then(|id| document.layers.iter().find(|l| l.id == id))
                else {
                    break;
                };
                coverage.config[1][0] *= if group.visible { group.opacity } else { 0.0 };
                coverage.mask(group);
                parent = group.parent;
            }
        }
        if mode != CoverageMode::Alpha {
            coverage.mask(layer);
        }
        let base = match mode {
            CoverageMode::Alpha => Some(layer),
            CoverageMode::Composite => layer
                .clip_to
                .and_then(|id| document.layers.iter().find(|l| l.id == id)),
            CoverageMode::Mask => None,
        };
        if let Some(base) = base {
            coverage.alpha(document, base, 0);
        }
        coverage
    }

    /// Multiply the top of the stack by [`crate::render::layer_alpha`] of `layer`.
    fn alpha(&mut self, document: &Document, layer: &Layer, depth: usize) {
        if depth > 256 || self.instructions() > MAX_INSTRUCTIONS {
            self.scalar(0.0);
            return;
        }
        if layer.group {
            self.op(PUSH, 0.0);
            for child in document
                .layers
                .iter()
                .filter(|l| l.parent == Some(layer.id) && l.visible)
            {
                if render::fades_group(child) {
                    let mask = child.mask.as_ref().unwrap();
                    self.source(
                        [mask.pixels.width(), mask.pixels.height()],
                        Arc::as_ptr(&mask.pixels) as usize,
                        mask.pixels.as_raw(),
                        mask.placement.unwrap_or(child.transform),
                        false,
                    );
                    self.set_op(FADE_SAMPLE, child.opacity);
                } else if render::adds_group_alpha(child) {
                    self.op(PUSH, 1.0);
                    self.alpha(document, child, depth + 1);
                    self.op(UNION, 0.0);
                }
            }
            self.scalar(layer.opacity);
            self.mask(layer);
            self.op(MUL, 0.0);
            return;
        }
        self.scalar(layer.opacity);
        if let Some(pixels) = &layer.pixels {
            self.source(
                [pixels.width(), pixels.height()],
                Arc::as_ptr(pixels) as usize,
                pixels.as_raw(),
                layer.transform,
                true,
            );
        } else {
            self.scalar(0.0);
        }
        self.mask(layer);
        if let Some(source) = layer
            .clip_to
            .and_then(|id| document.layers.iter().find(|l| l.id == id))
        {
            self.alpha(document, source, depth + 1);
        }
    }

    fn instructions(&self) -> usize {
        self.config[1][1] as usize
    }

    /// Whether the shader can run the program; otherwise the caller falls back to the CPU.
    fn supported(&self) -> bool {
        self.max_depth < STACK && self.instructions() <= MAX_INSTRUCTIONS
    }

    /// Multiply the top of the stack by `value`, folded into the start when nothing is pushed.
    fn scalar(&mut self, value: f32) {
        if self.depth == 0 {
            self.config[1][0] *= value;
        } else {
            self.op(MUL_SCALAR, value);
        }
    }

    /// An instruction that samples nothing.
    fn op(&mut self, op: f32, parameter: f32) {
        self.config.push([0.0; 4]);
        self.config.extend([[0.0; 4]; 5]);
        self.config[1][1] += 1.0;
        self.set_op(op, parameter);
    }

    /// Set the last instruction's operation, tracking the stack depth.
    fn set_op(&mut self, op: f32, parameter: f32) {
        let base = self.config.len() - 6;
        self.config[base + 3][3] = op;
        self.config[base + 4][3] = parameter;
        if op == PUSH {
            self.depth += 1;
            self.max_depth = self.max_depth.max(self.depth);
        } else if op == UNION || op == MUL {
            self.depth -= 1;
        }
    }

    fn encode(
        &self,
        gpu: &Processor,
        size: [u32; 2],
    ) -> Result<(wgpu::CommandEncoder, wgpu::Buffer)> {
        ensure!(
            self.supported(),
            "Clipping is nested too deeply for the GPU"
        );
        let stride = self.config[1][2] as u32;
        let pixels = gpu.buffer(&self.pixels)?;
        let result = gpu.empty(u64::from(stride) * u64::from(size[1]))?;
        let mut encoder = gpu.encoder();
        gpu.dispatch(
            &mut encoder,
            "layer_coverage",
            include_str!("coverage.wgsl"),
            [&pixels, &pixels, &result],
            &self.config,
            [stride / 4, size[1]],
        )?;
        Ok((encoder, result))
    }

    /// Multiply the top of the stack by a sampled source: `rgba` pixels' bilinear alpha, or
    /// a mask's nearest value.
    fn source(
        &mut self,
        size: [u32; 2],
        key: usize,
        bytes: &[u8],
        transform: Transform,
        rgba: bool,
    ) {
        let offset = *self.offsets.entry((key, rgba)).or_insert_with(|| {
            let offset = (self.pixels.len() / 4) as u32;
            self.pixels.extend(padded(bytes));
            offset
        });
        self.config.push([
            size[0] as f32,
            size[1] as f32,
            f32::from_bits(offset),
            if rgba { 1.0 } else { 0.0 },
        ]);
        let mut transform_rows = transform_config(transform);
        let inverse = transform
            .warp
            .and_then(crate::geometry::Homography::from_quad)
            .and_then(|h| h.inverse())
            .map_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], |h| h.0);
        for (row, matrix) in transform_rows[2..].iter_mut().zip(inverse) {
            *row = [matrix[0], matrix[1], matrix[2], 0.0];
        }
        self.config.extend(transform_rows);
        self.config[1][1] += 1.0;
        self.set_op(MUL_SAMPLE, 0.0);
    }
    fn mask(&mut self, layer: &Layer) {
        if let Some(mask) = layer.mask.as_ref().filter(|m| m.enabled) {
            self.source(
                [mask.pixels.width(), mask.pixels.height()],
                Arc::as_ptr(&mask.pixels) as usize,
                mask.pixels.as_raw(),
                mask.placement.unwrap_or(layer.transform),
                false,
            );
        }
    }
}
impl Processor {
    pub(super) fn coverage(
        &self,
        document: &Document,
        layer: &Layer,
        size: [u32; 2],
    ) -> Result<wgpu::Texture> {
        let stride = size[0].div_ceil(256) * 256;
        let coverage = Coverage::new(document, layer, size, CoverageMode::Composite);
        let (mut encoder, result) = coverage.encode(self, size)?;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer coverage"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &result,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(size[1]),
                },
            },
            texture.as_image_copy(),
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        Ok(texture)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CoverageMode {
    Composite,
    Mask,
    Alpha,
}

/// Materialize masks for selection, clipping bake and background removal. An
/// optional transform maps the output grid into a layer's source coordinates.
pub(crate) fn coverage_image(
    document: &Document,
    layer: &Layer,
    size: [u32; 2],
    mode: CoverageMode,
    transform: Option<Transform>,
) -> Option<image::GrayImage> {
    attempt(u64::from(size[0]) * u64::from(size[1]), 65_536, |gpu| {
        let mut coverage = Coverage::new(document, layer, size, mode);
        if let Some(transform) = transform {
            coverage.config[1][3] = coverage.config.len() as f32;
            coverage.config.extend(transform_config(transform));
        }
        let (encoder, result) = coverage.encode(gpu, size)?;
        let stride = coverage.config[1][2] as u64;
        let bytes = gpu.read(encoder, &result, stride * u64::from(size[1]))?;
        let mut image = image::GrayImage::new(size[0], size[1]);
        for (source, row) in bytes
            .chunks_exact(stride as usize)
            .zip(image.as_mut().chunks_exact_mut(size[0] as usize))
        {
            row.copy_from_slice(&source[..row.len()]);
        }
        Ok(image)
    })
}

pub(crate) fn bake_alpha(
    document: &Document,
    base: &Layer,
    pixels: &image::RgbaImage,
    transform: Transform,
) -> Option<image::RgbaImage> {
    let size = [pixels.width(), pixels.height()];
    bake(
        document,
        base,
        size,
        CoverageMode::Alpha,
        transform,
        pixels.as_raw(),
        1,
    )
    .map(|bytes| image::RgbaImage::from_raw(size[0], size[1], bytes).unwrap())
}
pub(crate) fn bake_mask(layer: &Layer, mask: &image::GrayImage) -> Option<image::GrayImage> {
    if layer.mask.as_ref().is_none_or(|m| !m.enabled) {
        return Some(mask.clone());
    }
    let size = [mask.width(), mask.height()];
    let document = Document::new(size[0], size[1]).ok()?;
    bake(
        &document,
        layer,
        size,
        CoverageMode::Mask,
        layer.transform,
        mask.as_raw(),
        2,
    )
    .map(|bytes| super::paint::gray(size, bytes))
}
fn bake(
    document: &Document,
    base: &Layer,
    size: [u32; 2],
    mode: CoverageMode,
    transform: Transform,
    original: &[u8],
    format: u32,
) -> Option<Vec<u8>> {
    attempt(u64::from(size[0]) * u64::from(size[1]), 65_536, |gpu| {
        let mut coverage = Coverage::new(document, base, size, mode);
        ensure!(
            coverage.supported(),
            "Clipping is nested too deeply for the GPU"
        );
        coverage.config[1][2] = size[0] as f32 * 4.0;
        coverage.config[1][3] = coverage.config.len() as f32;
        let mut mapping = transform_config(transform);
        mapping[2][3] = format as f32;
        coverage.config.extend(mapping);
        let source = gpu.buffer(&coverage.pixels)?;
        let original = gpu.buffer(&padded(original))?;
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
        let output = gpu.empty(bytes)?;
        let mut encoder = gpu.encoder();
        gpu.dispatch(
            &mut encoder,
            "layer_coverage",
            include_str!("coverage.wgsl"),
            [&source, &original, &output],
            &coverage.config,
            size,
        )?;
        gpu.read(encoder, &output, bytes)
    })
}

#[cfg(test)]
#[path = "coverage_tests.rs"]
pub(super) mod tests;

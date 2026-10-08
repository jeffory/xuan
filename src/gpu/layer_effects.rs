//! Layer effects on the GPU, with the passes of upstream's MetalLayerEffects
//! (Rendering/MetalLayerEffects.swift). `layer_effects::render_cpu` is the CPU fallback.

use super::{Processor, processor::attempt};
use crate::layer_effects::{LayerEffects, Plan, blur_radius, blurs};
use anyhow::Result;
use image::RgbaImage;

const SHADER: &str = concat!(
    include_str!("buffers.wgsl"),
    include_str!("layer_effects.wgsl")
);
/// Small rasters are quicker on the CPU than a round trip to the GPU.
const MINIMUM: u64 = 16_384;

/// `padded` drawn with `effects`, or `None` without a GPU (or for a small raster).
pub(crate) fn layer_effects(padded: &RgbaImage, effects: &LayerEffects) -> Option<RgbaImage> {
    let pixels = u64::from(padded.width()) * u64::from(padded.height());
    attempt(pixels, MINIMUM, |gpu| gpu.layer_effects(padded, effects))
}

/// One render's encoder and the buffers every pass shares.
struct Passes<'a> {
    gpu: &'a Processor,
    encoder: wgpu::CommandEncoder,
    none: wgpu::Buffer,
    size: [u32; 2],
    bytes: u64,
}

impl Passes<'_> {
    /// Runs `entry` over `from` (and `with`), into a new plane.
    fn run(
        &mut self,
        entry: &'static str,
        from: &wgpu::Buffer,
        with: Option<&wgpu::Buffer>,
        config: [f32; 4],
    ) -> Result<wgpu::Buffer> {
        let output = self.gpu.empty(self.bytes)?;
        self.gpu.dispatch(
            &mut self.encoder,
            entry,
            SHADER,
            [from, with.unwrap_or(&self.none), &output],
            &[config],
            self.size,
        )?;
        Ok(output)
    }

    fn config(&self, z: f32, w: f32) -> [f32; 4] {
        [self.size[0] as f32, self.size[1] as f32, z, w]
    }

    /// `from` softened by a Gaussian of `sigma` (or a copy, for none).
    fn blur(&mut self, from: &wgpu::Buffer, sigma: f32) -> Result<wgpu::Buffer> {
        if !blurs(sigma) {
            return self.run("fx_shift", from, None, self.config(0.0, 0.0));
        }
        let config = self.config(sigma, blur_radius(sigma) as f32);
        let rows = self.run("fx_blur_rows", from, None, config)?;
        self.run("fx_blur_columns", &rows, None, config)
    }

    /// `from` moved by `offset` and softened.
    fn shadow(
        &mut self,
        from: &wgpu::Buffer,
        offset: [f32; 2],
        sigma: f32,
    ) -> Result<wgpu::Buffer> {
        let moved = self.run("fx_shift", from, None, self.config(offset[0], offset[1]))?;
        if !blurs(sigma) {
            return Ok(moved);
        }
        self.blur(&moved, sigma)
    }
}

impl Processor {
    pub(super) fn layer_effects(
        &self,
        padded: &RgbaImage,
        effects: &LayerEffects,
    ) -> Result<RgbaImage> {
        let (width, height) = padded.dimensions();
        let plan = Plan::new(effects);
        let bytes = u64::from(width) * u64::from(height) * 4;
        let input = self.buffer(padded.as_raw())?;
        let mut passes = Passes {
            gpu: self,
            encoder: self.encoder(),
            none: self.empty(16)?,
            size: [width, height],
            bytes,
        };
        let shape = passes.run("fx_alpha", &input, None, passes.config(0.0, 0.0))?;
        let ring = match plan.stroke {
            Some((reach, inside, _)) => {
                let config = passes.config(reach as f32, if inside { 1.0 } else { 0.0 });
                let rows = passes.run("fx_spread_rows", &shape, None, config)?;
                let moved = passes.run("fx_spread_columns", &rows, None, config)?;
                Some(passes.run("fx_ring", &shape, Some(&moved), config)?)
            }
            None => None,
        };
        let drop_shadow = match plan.drop_shadow {
            Some((offset, sigma, _)) => Some(passes.shadow(&shape, offset, sigma)?),
            None => None,
        };
        let inside = |passes: &mut Passes, soft: wgpu::Buffer| {
            passes.run("fx_inside", &shape, Some(&soft), passes.config(0.0, 0.0))
        };
        let inner_shadow = match plan.inner_shadow {
            Some((offset, sigma, _)) => {
                let soft = passes.shadow(&shape, offset, sigma)?;
                Some(inside(&mut passes, soft)?)
            }
            None => None,
        };
        let outer_glow = match plan.outer_glow {
            Some((sigma, _)) => Some(passes.blur(&shape, sigma)?),
            None => None,
        };
        let inner_glow = match plan.inner_glow {
            Some((sigma, _)) => {
                let soft = passes.blur(&shape, sigma)?;
                Some(inside(&mut passes, soft)?)
            }
            None => None,
        };
        // The five planes side by side, for the one pass that reads them all.
        let planes = self.empty(bytes * 5)?;
        for (slot, plane) in [ring, drop_shadow, inner_shadow, outer_glow, inner_glow]
            .iter()
            .enumerate()
        {
            if let Some(plane) = plane {
                passes
                    .encoder
                    .copy_buffer_to_buffer(plane, 0, &planes, slot as u64 * bytes, bytes);
            }
        }
        let flag = |present: bool| if present { 1.0 } else { 0.0 };
        let paint = |color: Option<[f32; 4]>| color.unwrap_or([0.0; 4]);
        let config = [
            passes.config(0.0, 0.0),
            paint(plan.stroke.map(|s| s.2)),
            paint(plan.drop_shadow.map(|s| s.2)),
            paint(plan.overlay),
            paint(plan.inner_shadow.map(|s| s.2)),
            paint(plan.outer_glow.map(|g| g.1)),
            paint(plan.inner_glow.map(|g| g.1)),
            [
                flag(plan.stroke.is_some()),
                flag(plan.stroke.is_some_and(|s| s.1)),
                flag(plan.drop_shadow.is_some()),
                flag(plan.inner_shadow.is_some()),
            ],
            [
                flag(plan.overlay.is_some()),
                flag(plan.outer_glow.is_some()),
                flag(plan.inner_glow.is_some()),
                0.0,
            ],
        ];
        let output = self.empty(bytes)?;
        self.dispatch(
            &mut passes.encoder,
            "fx_compose",
            SHADER,
            [&input, &planes, &output],
            &config,
            [width, height],
        )?;
        let pixels = self.read(passes.encoder, &output, bytes)?;
        Ok(RgbaImage::from_raw(width, height, pixels).expect("raster size"))
    }
}

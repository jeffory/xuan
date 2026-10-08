use super::{Processor, processor::attempt};
use crate::raw::{DecodedRaw, DevelopSettings, Overlay, OverlayKind, SourceMap};
use anyhow::{Result, ensure};
use bytemuck::{Pod, Zeroable};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const SHADER: &str = concat!(
    include_str!("buffers.wgsl"),
    include_str!("raster.wgsl"),
    include_str!("raw.wgsl")
);

pub(crate) fn develop(
    raw: &DecodedRaw,
    s: &DevelopSettings,
    wb: [f32; 3],
    depth: u32,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    attempt(
        u64::from(raw.camera.width()) * u64::from(raw.camera.height()),
        16_384,
        |gpu| gpu.develop(raw, s, wb, depth, cancel),
    )
}

pub(super) struct RawBuffers {
    pub source: wgpu::Buffer,
    pub pixels: [wgpu::Buffer; 2],
    detail: Option<[wgpu::Buffer; 2]>,
}

impl RawBuffers {
    pub fn new(gpu: &Processor, raw: &DecodedRaw) -> Result<Self> {
        let bytes = u64::from(raw.camera.width()) * u64::from(raw.camera.height()) * 16;
        Ok(Self {
            source: gpu.buffer(bytemuck::cast_slice(raw.camera.as_raw()))?,
            pixels: [gpu.empty(bytes)?, gpu.empty(bytes)?],
            detail: None,
        })
    }
}

impl Processor {
    pub(super) fn develop(
        &self,
        raw: &DecodedRaw,
        s: &DevelopSettings,
        wb: [f32; 3],
        depth: u32,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        let mut work = RawBuffers::new(self, raw)?;
        let (mut encoder, current) = self.raw_passes(raw, s, wb, &mut work, cancel)?;
        let source = &work.source;
        let buffers = &work.pixels;
        let config = RawUniforms::new(
            raw,
            s,
            wb,
            Encoding {
                depth,
                row_pixels: 0,
                premultiply: false,
            },
        );
        let target = config.output_size();
        let bytes = u64::from(target[0]) * u64::from(target[1]) * u64::from(depth / 2);
        let output = self.empty(bytes)?;
        ensure!(!cancel.load(Ordering::Relaxed), "RAW development cancelled");
        self.dispatch(
            &mut encoder,
            "raw_encode",
            SHADER,
            [&buffers[current], source, &output],
            config.rows(),
            target,
        )?;
        let result = self.read(encoder, &output, bytes)?;
        ensure!(!cancel.load(Ordering::Relaxed), "RAW development cancelled");
        Ok(result)
    }

    pub(super) fn raw_passes(
        &self,
        raw: &DecodedRaw,
        s: &DevelopSettings,
        wb: [f32; 3],
        work: &mut RawBuffers,
        cancel: &AtomicBool,
    ) -> Result<(wgpu::CommandEncoder, usize)> {
        s.validate()?;
        let mut encoder = self.encoder();
        let size = [raw.camera.width(), raw.camera.height()];
        let source = &work.source;
        let buffers = &work.pixels;
        let uniforms = RawUniforms::new(raw, s, wb, Encoding::default());
        let config = uniforms.rows();
        ensure!(!cancel.load(Ordering::Relaxed), "RAW development cancelled");
        self.dispatch(
            &mut encoder,
            "raw_camera",
            SHADER,
            [source, source, &buffers[0]],
            config,
            size,
        )?;
        let mut current = 0;
        if s.luminance_noise > 0.0 || s.color_noise > 0.0 {
            self.dispatch(
                &mut encoder,
                "raw_denoise",
                SHADER,
                [&buffers[current], source, &buffers[1 - current]],
                config,
                size,
            )?;
            current = 1 - current;
        }
        for overlay in s.overlays.iter().filter(|o| o.enabled) {
            ensure!(!cancel.load(Ordering::Relaxed), "RAW development cancelled");
            let overlay = overlay_config(overlay, size, cancel)?;
            self.dispatch(
                &mut encoder,
                "raw_overlay",
                SHADER,
                [&buffers[current], source, &buffers[1 - current]],
                &overlay,
                size,
            )?;
            current = 1 - current;
        }
        self.dispatch(
            &mut encoder,
            "raw_tone",
            SHADER,
            [&buffers[current], source, &buffers[1 - current]],
            config,
            size,
        )?;
        current = 1 - current;
        let scale = size[0] as f32 / raw.metadata.width as f32;
        if s.clarity != 0.0 || s.texture != 0.0 || s.sharpen != 0.0 {
            if work.detail.is_none() {
                let bytes = u64::from(size[0]) * u64::from(size[1]) * 16;
                work.detail = Some([self.empty(bytes)?, self.empty(bytes)?]);
            }
            let [scratch, blurred] = work.detail.as_ref().unwrap();
            for (amount, radius, threshold) in [
                (s.clarity / 100.0, 24.0 * scale, 0.0),
                (s.texture / 100.0, 3.0 * scale, 0.0),
                (
                    s.sharpen / 100.0,
                    s.sharpen_radius * scale,
                    s.sharpen_threshold,
                ),
            ] {
                if amount == 0.0 {
                    continue;
                }
                ensure!(!cancel.load(Ordering::Relaxed), "RAW development cancelled");
                self.blur_passes(
                    &mut encoder,
                    &buffers[current],
                    scratch,
                    blurred,
                    size,
                    radius.max(0.3),
                )?;
                self.dispatch(
                    &mut encoder,
                    "raw_detail",
                    SHADER,
                    [&buffers[current], blurred, &buffers[1 - current]],
                    &[uniforms.size, [amount, threshold, 0.0, 0.0]],
                    size,
                )?;
                current = 1 - current;
            }
        }
        Ok((encoder, current))
    }
}

/// How `raw_encode` writes the final pixels.
#[derive(Clone, Copy, Debug)]
pub(super) struct Encoding {
    /// Bits per channel: 8 or 16.
    pub depth: u32,
    /// Output row length in pixels; 0 packs rows tightly.
    pub row_pixels: u32,
    /// Premultiply alpha, as egui textures expect.
    pub premultiply: bool,
}

impl Default for Encoding {
    fn default() -> Self {
        Self {
            depth: 8,
            row_pixels: 0,
            premultiply: false,
        }
    }
}

/// Configuration for the `raw_*` development passes in `raw.wgsl`, built once
/// for both the full-resolution and the resident-preview dispatch. Each field
/// is one `vec4<f32>` row of the shader's `config` array, which names the rows
/// with `RAW_*` index constants; `tests::wgsl_rows_match_uniform_layout` keeps
/// the two in step. Geometry, negative and crop coefficients come from the same
/// Rust values the CPU path evaluates (`SourceMap`, `NegativeInversion`,
/// `OutputMap`), so the shader applies them rather than re-deriving them.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(super) struct RawUniforms {
    /// Development width, height, aspect ratio.
    pub size: [f32; 4],
    /// Straightening cos, sin; perspective x, y.
    pub geometry: [f32; 4],
    /// Distortion, red and blue chromatic aberration, vignette.
    pub lens: [f32; 4],
    /// White-balance multipliers; exposure gain.
    pub white_balance: [f32; 4],
    pub camera_to_rgb: [[f32; 4]; 3],
    /// Luminance and color noise reduction.
    pub noise: [f32; 4],
    /// Shadows, highlights, whites, blacks.
    pub tone: [f32; 4],
    /// Dehaze, brightness, contrast, defringe.
    pub presence: [f32; 4],
    /// Saturation, vibrance, monochrome flag, tone balance.
    pub color: [f32; 4],
    pub bw_mix: [f32; 4],
    /// Shadow hue, amount; highlight hue, amount.
    pub split_tone: [f32; 4],
    /// Development pixel read by output (0, 0); output width, height.
    pub output: [f32; 4],
    /// Depth, row length in pixels, premultiply flag.
    pub encoding: [f32; 4],
    /// Row-major integer matrix mapping output to development pixel offsets.
    pub output_axes: [f32; 4],
    /// Per channel (master, red, green, blue): four knots, then the fifth.
    pub curves: [[f32; 4]; 8],
    /// Hue, saturation, lightness shift; band center hue.
    pub hsl: [[f32; 4]; 8],
    /// Film base; enabled flag.
    pub negative_base: [f32; 4],
    /// Density range; black point.
    pub negative_density: [f32; 4],
    /// Linear channel gain; gamma.
    pub negative_gain: [f32; 4],
}

// One `vec4<f32>` per field: no padding, so the struct is the row array.
const _: () = assert!(std::mem::size_of::<RawUniforms>().is_multiple_of(16));

const HSL_BANDS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

impl RawUniforms {
    pub fn new(raw: &DecodedRaw, s: &DevelopSettings, wb: [f32; 3], encoding: Encoding) -> Self {
        let size = [raw.camera.width(), raw.camera.height()];
        let geometry = SourceMap::new(s, size[0] as f32 / size[1] as f32);
        let output = s.output_map(size);
        let negative = s.negative.inversion();
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        let mut u = Self {
            size: [size[0] as f32, size[1] as f32, geometry.aspect, 0.0],
            geometry: [
                geometry.cos,
                geometry.sin,
                geometry.perspective[0],
                geometry.perspective[1],
            ],
            lens: [
                geometry.distortion,
                s.chromatic_red,
                s.chromatic_blue,
                s.vignette,
            ],
            white_balance: [wb[0], wb[1], wb[2], 2.0_f32.powf(s.exposure)],
            camera_to_rgb: raw.camera_to_rgb.map(|row| [row[0], row[1], row[2], 0.0]),
            noise: [s.luminance_noise, s.color_noise, 0.0, 0.0],
            tone: [s.shadows, s.highlights, s.whites, s.blacks],
            presence: [s.dehaze, s.brightness, s.contrast, s.defringe],
            color: [s.saturation, s.vibrance, flag(s.monochrome), s.tone_balance],
            bw_mix: [s.bw_mix[0], s.bw_mix[1], s.bw_mix[2], 0.0],
            split_tone: [
                s.shadow_tone[0],
                s.shadow_tone[1],
                s.highlight_tone[0],
                s.highlight_tone[1],
            ],
            output: [
                output.origin[0] as f32,
                output.origin[1] as f32,
                output.size[0] as f32,
                output.size[1] as f32,
            ],
            encoding: [
                encoding.depth as f32,
                encoding.row_pixels as f32,
                flag(encoding.premultiply),
                0.0,
            ],
            output_axes: [
                output.axes[0][0] as f32,
                output.axes[0][1] as f32,
                output.axes[1][0] as f32,
                output.axes[1][1] as f32,
            ],
            curves: [[0.0; 4]; 8],
            hsl: std::array::from_fn(|i| [s.hsl[i][0], s.hsl[i][1], s.hsl[i][2], HSL_BANDS[i]]),
            negative_base: [
                negative.film_base[0],
                negative.film_base[1],
                negative.film_base[2],
                flag(s.negative.enabled),
            ],
            negative_density: [
                negative.density_range[0],
                negative.density_range[1],
                negative.density_range[2],
                negative.black_point,
            ],
            negative_gain: [
                negative.gain[0],
                negative.gain[1],
                negative.gain[2],
                negative.gamma,
            ],
        };
        for (i, curve) in s.curves.iter().enumerate() {
            u.curves[i * 2].copy_from_slice(&curve[..4]);
            u.curves[i * 2 + 1][0] = curve[4];
        }
        u
    }

    /// The `config` array rows uploaded for the shader.
    pub fn rows(&self) -> &[[f32; 4]] {
        bytemuck::cast_slice(std::slice::from_ref(self))
    }

    /// Output dimensions after cropping and quarter turns.
    pub fn output_size(&self) -> [u32; 2] {
        [self.output[2] as u32, self.output[3] as u32]
    }
}

fn overlay_config(overlay: &Overlay, size: [u32; 2], cancel: &AtomicBool) -> Result<Vec<[f32; 4]>> {
    let mut p = vec![
        [size[0] as f32, size[1] as f32, 0.0, 0.0],
        [
            overlay.start.x,
            overlay.start.y,
            overlay.end.x,
            overlay.end.y,
        ],
        [
            match overlay.kind {
                OverlayKind::Linear => 0.0,
                OverlayKind::Radial => 1.0,
                OverlayKind::Brush => 2.0,
            },
            overlay.feather,
            if overlay.invert { 1.0 } else { 0.0 },
            overlay.radius,
        ],
        [overlay.exposure, overlay.warmth, overlay.saturation, 0.0],
        [size[0].div_ceil(32) as f32, 0.0, 0.0, 0.0],
    ];
    if overlay.kind == OverlayKind::Brush {
        // Bin dab centers once. Each GPU pixel visits only overlapping dabs.
        let grid = [size[0].div_ceil(32), size[1].div_ceil(32)];
        let mut tiles = vec![Vec::new(); (grid[0] * grid[1]) as usize];
        let radius = overlay.radius * size[1] as f32;
        // Bound the acceleration structure independently of image size. Very
        // broad, dense strokes must not exhaust host memory before GPU fallback.
        let mut records = 5 + tiles.len();
        for point in &overlay.points {
            ensure!(!cancel.load(Ordering::Relaxed), "RAW development cancelled");
            let x = point.x * size[0] as f32;
            let y = point.y * size[1] as f32;
            let left = ((x - radius).max(0.0) as u32 / 32).min(grid[0] - 1);
            let top = ((y - radius).max(0.0) as u32 / 32).min(grid[1] - 1);
            let right = ((x + radius).ceil() as u32 / 32).min(grid[0] - 1);
            let bottom = ((y + radius).ceil() as u32 / 32).min(grid[1] - 1);
            records += ((right - left + 1) * (bottom - top + 1)) as usize;
            ensure!(
                records <= 4_194_304,
                "RAW brush coverage exceeds the GPU tile budget"
            );
            for y in top..=bottom {
                for x in left..=right {
                    tiles[(y * grid[0] + x) as usize].push([point.x, point.y, 0.0, 0.0]);
                }
            }
        }
        p.resize(5 + tiles.len(), [0.0; 4]);
        for (i, tile) in tiles.into_iter().enumerate() {
            p[5 + i] = [p.len() as f32, tile.len() as f32, 0.0, 0.0];
            p.extend(tile);
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    //! Adapter-free checks that the shader receives exactly what the CPU path
    //! uses. Each test evaluates the corresponding `raw.wgsl` expression in Rust
    //! from the uniform rows alone; the ignored GPU-vs-CPU comparisons in
    //! `processing_tests.rs` cover the shader itself.
    use super::*;
    use crate::document::Point;
    use crate::raw::{NegativeSettings, RawMetadata};
    use std::mem::{offset_of, size_of};
    use wgpu::naga;

    fn raw(width: u32, height: u32) -> DecodedRaw {
        DecodedRaw {
            camera: image::Rgb32FImage::new(width, height),
            as_shot: [1.2, 1.0, 0.9],
            camera_to_rgb: [[1.1, -0.1, 0.0], [-0.05, 1.1, -0.05], [0.0, -0.1, 1.1]],
            xyz_to_camera: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            metadata: RawMetadata {
                width,
                height,
                ..Default::default()
            },
        }
    }

    fn settings() -> Vec<DevelopSettings> {
        let base = DevelopSettings {
            rotation: 7.0,
            perspective: [3.0, -5.0],
            distortion: 11.0,
            negative: NegativeSettings {
                enabled: true,
                film_base: [0.9, 0.5, 0.2],
                density_range: [1.7, 2.1, 2.4],
                black_point: -0.07,
                gamma: 1.8,
                balance: [0.3, -0.2, 0.1],
            },
            exposure: -0.3,
            ..Default::default()
        };
        let mut all = Vec::new();
        for quarter_turns in 0..4 {
            for crop in [
                [0.0, 0.0, 1.0, 1.0],
                [0.07, 0.11, 0.94, 0.92],
                [0.25, 0.125, 0.75, 1.0],
            ] {
                all.push(DevelopSettings {
                    quarter_turns,
                    crop,
                    ..base.clone()
                });
            }
        }
        all
    }

    #[test]
    fn uniform_rows_are_tightly_packed_vec4s() {
        assert_eq!(size_of::<RawUniforms>(), 35 * 16);
        assert!(std::mem::align_of::<RawUniforms>() <= 16);
        let u = RawUniforms::new(
            &raw(8, 6),
            &DevelopSettings::default(),
            [1.0; 3],
            Encoding::default(),
        );
        assert_eq!(u.rows().len(), 35);
        assert_eq!(u.rows()[0], u.size);
        assert_eq!(u.rows()[34], u.negative_gain);
    }

    /// Every `RAW_*` row index in raw.wgsl must equal the matching field offset,
    /// and the shader must still parse and validate.
    #[test]
    fn wgsl_rows_match_uniform_layout() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("raw shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("raw shader validates");
        let mut shader: Vec<(String, u32)> = module
            .constants
            .iter()
            .filter_map(|(_, c)| {
                let name = c.name.as_deref()?.strip_prefix("RAW_")?;
                match module.global_expressions[c.init] {
                    naga::Expression::Literal(naga::Literal::U32(v)) => Some((name.to_owned(), v)),
                    ref other => panic!("RAW_{name} is not a u32 literal: {other:?}"),
                }
            })
            .collect();
        shader.sort();
        macro_rules! rows {
            ($($name:literal => $field:ident),* $(,)?) => {
                vec![$(($name.to_owned(), (offset_of!(RawUniforms, $field) / 16) as u32)),*]
            };
        }
        let mut rust = rows![
            "SIZE" => size,
            "GEOMETRY" => geometry,
            "LENS" => lens,
            "WHITE_BALANCE" => white_balance,
            "CAMERA_TO_RGB" => camera_to_rgb,
            "NOISE" => noise,
            "TONE" => tone,
            "PRESENCE" => presence,
            "COLOR" => color,
            "BW_MIX" => bw_mix,
            "SPLIT_TONE" => split_tone,
            "OUTPUT" => output,
            "ENCODING" => encoding,
            "OUTPUT_AXES" => output_axes,
            "CURVES" => curves,
            "HSL" => hsl,
            "NEGATIVE_BASE" => negative_base,
            "NEGATIVE_DENSITY" => negative_density,
            "NEGATIVE_GAIN" => negative_gain,
        ];
        rust.push(("ROWS".to_owned(), (size_of::<RawUniforms>() / 16) as u32));
        rust.sort();
        assert_eq!(shader, rust);
    }

    /// `raw_encode` index arithmetic, evaluated from the uniform rows.
    fn shader_source_pixel(u: &RawUniforms, x: u32, y: u32) -> [u32; 2] {
        let axes = u.output_axes.map(|v| v as i32);
        let (x, y) = (x as i32, y as i32);
        [
            (u.output[0] as i32 + axes[0] * x + axes[1] * y) as u32,
            (u.output[1] as i32 + axes[2] * x + axes[3] * y) as u32,
        ]
    }

    #[test]
    fn shader_crop_and_quarter_turns_match_cpu_mappings() {
        let size = [64, 32];
        let image = raw(size[0], size[1]);
        for s in settings() {
            let u = RawUniforms::new(&image, &s, [1.0; 3], Encoding::default());
            let map = s.output_map(size);
            let [left, top, right, bottom] = s.crop_pixels(size);
            let [w, h] = [right - left, bottom - top];
            assert_eq!(u.output_size(), s.output_size(size));
            assert_eq!(u.output_size(), map.size);
            let [ow, oh] = u.output_size();
            for y in 0..oh {
                for x in 0..ow {
                    let actual = shader_source_pixel(&u, x, y);
                    assert_eq!(actual, map.source_pixel(x, y));
                    // The explicit table both encoders used before #15.
                    let [cx, cy] = match s.quarter_turns {
                        1 => [y, h - 1 - x],
                        2 => [w - 1 - x, h - 1 - y],
                        3 => [w - 1 - y, x],
                        _ => [x, y],
                    };
                    assert_eq!(actual, [left + cx, top + cy], "{s:?} at {x},{y}");
                    // The editor's display -> image mapping lands in the same
                    // pixel wherever the crop is pixel-exact (dyadic here).
                    if s.crop != [0.07, 0.11, 0.94, 0.92] {
                        let p = s.image_point(Point::new(
                            (x as f32 + 0.5) / ow as f32,
                            (y as f32 + 0.5) / oh as f32,
                        ));
                        let pixel = [
                            (p.x * size[0] as f32).floor() as u32,
                            (p.y * size[1] as f32).floor() as u32,
                        ];
                        assert_eq!(actual, pixel, "{s:?} at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn shader_geometry_matches_cpu_source_point() {
        let image = raw(89, 67);
        let aspect = 89.0 / 67.0;
        for s in settings() {
            let u = RawUniforms::new(&image, &s, [1.0; 3], Encoding::default());
            for (x, y) in [
                (0.0, 0.0),
                (0.13, 0.71),
                (0.5, 0.5),
                (0.97, 0.04),
                (1.0, 1.0),
            ] {
                // raw.wgsl `source_point`.
                let a = u.size[2];
                let [c, sn, px, py] = u.geometry;
                let p = [(x - 0.5) * 2.0, (y - 0.5) * 2.0 / a];
                let mut q = [c * p[0] + sn * p[1], (-sn * p[0] + c * p[1]) * a];
                let d = (1.0 + 0.004 * (px * q[0] + py * q[1])).max(0.2);
                q = q.map(|v| v / d);
                let k = 1.0 + u.lens[0] * 0.003 * (q[0] * q[0] + q[1] * q[1]) * 0.5;
                let shader = Point::new(0.5 + q[0] * k * 0.5, 0.5 + q[1] * k * 0.5);
                let cpu = crate::raw::source_point(Point::new(x, y), &s, aspect);
                assert!(shader.distance(cpu) < 1e-5, "{shader:?} vs {cpu:?}");
            }
        }
    }

    #[test]
    fn shader_negative_inversion_matches_cpu() {
        let image = raw(8, 6);
        for s in settings() {
            let u = RawUniforms::new(&image, &s, [1.0; 3], Encoding::default());
            assert_eq!(u.negative_base[3], 1.0);
            let cpu = s.negative.inversion();
            for camera in [[0.01_f32, 0.2, 0.5], [0.9, 0.5, 0.2], [0.0, 1.5, 0.05]] {
                // raw.wgsl `raw_camera`, negative branch.
                let shader: [f32; 3] = std::array::from_fn(|c| {
                    let density =
                        (u.negative_base[c] / camera[c].max(0.00001)).log2() / 10.0_f32.log2();
                    let positive =
                        ((density - u.negative_density[3]) / u.negative_density[c]).clamp(0.0, 1.0);
                    positive.powf(u.negative_gain[3]) * u.negative_gain[c] * u.white_balance[3]
                });
                let expected = cpu.convert(camera).map(|v| v * 2.0_f32.powf(s.exposure));
                for c in 0..3 {
                    assert!(
                        (shader[c] - expected[c]).abs() <= 1e-5 * expected[c].abs().max(1.0),
                        "{shader:?} vs {expected:?}"
                    );
                }
            }
        }
        let off = RawUniforms::new(
            &raw(8, 6),
            &DevelopSettings::default(),
            [1.0; 3],
            Encoding::default(),
        );
        assert_eq!(off.negative_base[3], 0.0);
    }
}

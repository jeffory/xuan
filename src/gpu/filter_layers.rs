//! Filter the resident composite without uploading or reading back a raster. Dither's error
//! diffusion, ASCII and glowing scanlines are the exception: they read the composite back, run
//! on the CPU and upload the result.
use wgpu::util::DeviceExt;

use super::target;
use crate::effects::Filter;

pub(super) struct FilterLayers {
    pipeline: wgpu::ComputePipeline,
    horizontal: wgpu::ComputePipeline,
    output: wgpu::Texture,
    scratch: Option<wgpu::Texture>,
    /// Bloom's and Tonal Contrast's blurred copy of the composite.
    blurred: Option<wgpu::Texture>,
}

impl FilterLayers {
    pub(super) fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let pipeline = |format, name: &str| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("filter layer inputs"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("filter layer pipeline"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("filter layers"),
                source: wgpu::ShaderSource::Wgsl(
                    concat!(
                        include_str!("filter_layers.wgsl"),
                        include_str!("stylize.wgsl"),
                        include_str!("dither.wgsl")
                    )
                    .replace("OUTPUT_FORMAT", name)
                    .into(),
                ),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("filter layer"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some("filter_layer"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            pipeline: pipeline(wgpu::TextureFormat::Rgba8Unorm, "rgba8unorm"),
            horizontal: pipeline(wgpu::TextureFormat::Rgba32Float, "rgba32float"),
            output: target(device, size, wgpu::TextureFormat::Rgba8Unorm, 1),
            scratch: None,
            blurred: None,
        }
    }

    /// Whether `render` can draw `filter`; otherwise `read` and `upload` take it to the CPU.
    pub(super) fn runs_on_gpu(filter: &Filter) -> bool {
        !matches!(filter, Filter::Dither(settings) if !settings.runs_on_gpu())
    }

    /// The composite in `source` as straight eight-bit pixels. Submits `encoder` and replaces
    /// it with a new one, so the frame continues after the readback.
    pub(super) fn read(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
    ) -> image::RgbaImage {
        let [width, height] = [source.width(), source.height()];
        self.resize(device, [width, height]);
        dispatch(
            device,
            encoder,
            &self.pipeline,
            [source, source],
            &self.output,
            &[[9.0, 0.0, 0.0, 0.0]],
        );
        let stride = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("filter layer readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            self.output.size(),
        );
        let finished = std::mem::replace(
            encoder,
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xuan composite frame"),
            }),
        );
        queue.submit([finished.finish()]);
        let (send, receive) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = send.send(result);
            });
        let mapped = device
            .poll(wgpu::PollType::wait_indefinitely())
            .ok()
            .and_then(|_| receive.recv().ok())
            .is_some_and(|result| result.is_ok());
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        if mapped {
            let bytes = buffer.slice(..).get_mapped_range();
            for row in bytes.chunks(stride as usize).take(height as usize) {
                pixels.extend_from_slice(&row[..(width * 4) as usize]);
            }
            drop(bytes);
            buffer.unmap();
        } else {
            pixels.resize((width * height * 4) as usize, 0);
        }
        image::RgbaImage::from_raw(width, height, pixels).unwrap()
    }

    /// Puts a CPU result where `render` leaves its own.
    pub(super) fn upload(
        &mut self,
        queue: &wgpu::Queue,
        image: &image::RgbaImage,
    ) -> &wgpu::Texture {
        queue.write_texture(
            self.output.as_image_copy(),
            image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width() * 4),
                rows_per_image: Some(image.height()),
            },
            self.output.size(),
        );
        &self.output
    }

    fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if size != [self.output.width(), self.output.height()] {
            self.output = target(device, size, wgpu::TextureFormat::Rgba8Unorm, 1);
            self.scratch = None;
            self.blurred = None;
        }
    }

    /// The Gaussian blur of `source` into `output`, as the CPU's `gaussian_blurred`.
    fn blur(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: [&wgpu::ComputePipeline; 2],
        source: &wgpu::Texture,
        scratch: &wgpu::Texture,
        output: &wgpu::Texture,
        sigma: f32,
    ) {
        let sigma = sigma.max(0.01);
        let length = blur_length(sigma);
        let mut weights: Vec<f32> = (0..length)
            .map(|i| (-0.5 * ((i as f32 - (length / 2) as f32) / sigma).powi(2)).exp())
            .collect();
        let total = weights.iter().sum::<f32>();
        weights.iter_mut().for_each(|w| *w /= total);
        let mut config = vec![[0.0, (length / 2) as f32, 0.0, 0.0]];
        config.extend(weights.iter().map(|w| [*w, 0.0, 0.0, 0.0]));
        dispatch(
            device,
            encoder,
            pipelines[1],
            [source, source],
            scratch,
            &config,
        );
        config[0][0] = 1.0;
        dispatch(
            device,
            encoder,
            pipelines[0],
            [scratch, scratch],
            output,
            &config,
        );
    }

    pub(super) fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        filter: &Filter,
    ) -> &wgpu::Texture {
        let size = [source.width(), source.height()];
        self.resize(device, size);
        let mut config = vec![[0.0; 4]];
        let mut second = source;
        match filter {
            &Filter::GaussianBlur { radius } => {
                let scratch = self.scratch.get_or_insert_with(|| {
                    target(device, size, wgpu::TextureFormat::Rgba32Float, 1)
                });
                Self::blur(
                    device,
                    encoder,
                    [&self.pipeline, &self.horizontal],
                    source,
                    scratch,
                    &self.output,
                    radius,
                );
                return &self.output;
            }
            &Filter::MotionBlur { distance, angle } => {
                let (sin, cos) = angle.to_radians().sin_cos();
                config[0] = [
                    2.0,
                    distance.ceil().clamp(1.0, 256.0),
                    distance * cos,
                    distance * sin,
                ];
            }
            &Filter::Noise { amount, monochrome } => {
                config[0] = [3.0, amount / 100.0, u32::from(monochrome) as f32, 0.0];
            }
            &Filter::LensCorrection {
                distortion,
                vignette,
            } => {
                config[0] = [4.0, distortion, vignette, 0.0];
            }
            // A filter layer frames the canvas and paints its transparent areas too.
            &Filter::Vignette {
                amount,
                color,
                midpoint,
                roundness,
                feather,
                highlights,
            } => {
                config[0] = [5.0, amount, highlights, 1.0];
                config.push([midpoint, roundness, feather, 0.0]);
                let [r, g, b] = color.map(|v| v as f32 / 255.0);
                config.push([r, g, b, 0.0]);
            }
            &Filter::Bloom { radius, .. } | &Filter::TonalContrast { radius, .. } => {
                let scratch = self.scratch.get_or_insert_with(|| {
                    target(device, size, wgpu::TextureFormat::Rgba32Float, 1)
                });
                let blurred = self.blurred.get_or_insert_with(|| {
                    target(device, size, wgpu::TextureFormat::Rgba8Unorm, 1)
                });
                Self::blur(
                    device,
                    encoder,
                    [&self.pipeline, &self.horizontal],
                    source,
                    scratch,
                    blurred,
                    radius,
                );
                second = blurred;
                match *filter {
                    Filter::Bloom { amount, .. } => config[0] = [6.0, amount / 50.0, 0.0, 0.0],
                    Filter::TonalContrast {
                        amount,
                        shadows,
                        midtones,
                        highlights,
                        ..
                    } => {
                        config[0] = [7.0, amount, 0.0, 0.0];
                        config.push([shadows, midtones, highlights, 0.0]);
                    }
                    _ => unreachable!(),
                }
            }
            Filter::Dither(settings) => {
                config[0] = [8.0, 0.0, 0.0, 0.0];
                config.extend(super::raster::dither_params(settings));
            }
        }
        dispatch(
            device,
            encoder,
            &self.pipeline,
            [source, second],
            &self.output,
            &config,
        );
        &self.output
    }
}

/// The taps of `FilterLayers::blur`'s kernel for `sigma`, an odd count.
pub(super) fn blur_length(sigma: f32) -> u32 {
    let sigma = sigma.max(0.01);
    ((((sigma - 0.8) / 0.3 + 1.0) * 2.0 + 1.0).max(3.0) as u32) | 1
}

/// `sources` are the filtered texture and a second input (the blurred copy, or the same).
fn dispatch(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    sources: [&wgpu::Texture; 2],
    output: &wgpu::Texture,
    config: &[[f32; 4]],
) {
    let parameters = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("filter layer parameters"),
        contents: bytemuck::cast_slice(config),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let views = [sources[0], output, sources[1]].map(|t| t.create_view(&Default::default()));
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("filter layer inputs"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: parameters.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&views[2]),
            },
        ],
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("filter layer"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind, &[]);
    pass.dispatch_workgroups(output.width().div_ceil(8), output.height().div_ceil(8), 1);
}

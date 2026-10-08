//! The golden scenes rendered on the GPU, compared with the CPU goldens.
//!
//! Scenes are built on the CPU (no processor is current) and then composited
//! or developed by calling the processor directly, so a GPU error fails the
//! test instead of silently falling back to the CPU reference.

use std::sync::atomic::AtomicBool;

use image::RgbaImage;

use super::Processor;
use crate::goldens::{Content, run};

#[test]
#[ignore = "requires a Vulkan, DirectX 12 or OpenGL compute adapter"]
fn scenes_match_cpu_goldens() {
    let instance = wgpu::Instance::new(&Default::default());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let gpu = Processor::new(device, queue);
    run(
        "gpu",
        false,
        |scene| scene.gpu,
        |content| match content {
            Content::Composite {
                document,
                width,
                height,
            } => {
                // Layer effects are drawn on this GPU too.
                super::scope(Some(gpu.clone()), || {
                    let prepared = crate::render::prepare_attachments(document);
                    gpu.compose(prepared.as_ref(), *width, *height).unwrap()
                })
            }
            Content::Develop { raw, settings } => {
                let [width, height] =
                    settings.output_size([raw.camera.width(), raw.camera.height()]);
                let wb = crate::raw::white_balance(raw, settings);
                let bytes = gpu
                    .develop(raw, settings, wb, 8, &AtomicBool::new(false))
                    .unwrap();
                RgbaImage::from_raw(width, height, bytes).unwrap()
            }
        },
    );
}

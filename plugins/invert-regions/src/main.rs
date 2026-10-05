//! Example Xuan plugin in Rust. Build with `cargo build --release`, then copy
//! or symlink this folder into the plugins directory.
use serde_json::{Value, json};
use xuan_plugin::{Output, Plugin, RpcError, ui};

fn main() {
    Plugin::new()
        .action("invert", |job| {
            let source = job
                .source_path()
                .ok_or_else(|| RpcError::new(xuan_plugin::codes::INVALID_PARAMS, "no source"))?;
            job.progress(Some(0.1), Some("reading"));
            let mut image = image::open(source)
                .map_err(RpcError::internal)?
                .to_rgba8();
            let keep_alpha: bool = job.input("keep_alpha").unwrap_or(true);
            let regions = job.regions();
            let (width, height) = image.dimensions();
            let covers = |x: u32, y: u32| -> Option<f32> {
                if regions.is_empty() {
                    return Some(1.0);
                }
                regions
                    .iter()
                    .find(|r| {
                        (x as f32) >= r.x
                            && (x as f32) < r.x + r.width
                            && (y as f32) >= r.y
                            && (y as f32) < r.y + r.height
                    })
                    .map(|r| {
                        r.fields
                            .get("strength")
                            .and_then(Value::as_f64)
                            .unwrap_or(1.0)
                            .clamp(0.0, 1.0) as f32
                    })
            };
            for y in 0..height {
                if y % 64 == 0 {
                    job.check_cancelled()?;
                    job.progress(Some(0.1 + 0.8 * y as f32 / height as f32), None);
                }
                for x in 0..width {
                    if let Some(strength) = covers(x, y) {
                        let pixel = image.get_pixel_mut(x, y);
                        for channel in &mut pixel.0[..3] {
                            let inverted = 255.0 - f32::from(*channel);
                            *channel = (f32::from(*channel) + (inverted - f32::from(*channel)) * strength)
                                .round() as u8;
                        }
                        if !keep_alpha {
                            pixel.0[3] = 255;
                        }
                    }
                }
            }
            let out = job.path("inverted.png");
            image.save(&out).map_err(RpcError::internal)?;
            job.progress(Some(1.0), Some("done"));
            Ok(vec![
                Output::image(out, Some("Inverted"), 0.0, 0.0),
                Output::text(format!("Inverted {} region(s)", regions.len().max(1))),
            ])
        })
        .pane("average", |pane| {
            if pane.document.is_null() {
                return Ok(ui::column(vec![ui::muted("Open a document to see its average colour.")]));
            }
            let export = pane.host.export_document(Some(128))?;
            let image = image::open(&export.path)
                .map_err(RpcError::internal)?
                .to_rgba8();
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for pixel in image.pixels() {
                if pixel.0[3] > 0 {
                    for (i, channel) in pixel.0[..3].iter().enumerate() {
                        sum[i] += u64::from(*channel);
                    }
                    count += 1;
                }
            }
            let average = sum.map(|v| (v / count.max(1)) as u8);
            let hex = format!("#{:02x}{:02x}{:02x}", average[0], average[1], average[2]);
            Ok(ui::column(vec![
                ui::swatches(None, &[&hex], None),
                ui::label(&hex),
                ui::muted(&format!("{} × {} px sampled", image.width(), image.height())),
                json!({"type": "row", "children": [ui::button("refresh", "Refresh")]}),
            ]))
        })
        .run();
}

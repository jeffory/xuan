//! Proof of concept: animated GIF export (and an import Xuan never calls). See README.md.
use std::{
    fs::File,
    io::BufWriter,
    sync::{Arc, Mutex},
};

use image::{
    codecs::gif::{GifDecoder, GifEncoder, Repeat},
    imageops, AnimationDecoder, Delay, Frame, RgbaImage,
};
use serde_json::{json, Value};
use xuan_plugin::{Host, Plugin, RpcError};

/// `"Walk 3 (120ms)"` → 120.
fn delay_from_name(name: &str) -> Option<u32> {
    let open = name.rfind('(')?;
    let inner = name[open + 1..].strip_suffix(')')?.trim();
    inner.strip_suffix("ms")?.trim().parse().ok()
}

fn frames_by_pixels(
    host: &Host,
    document: &Value,
    layers: &[&Value],
) -> Result<Vec<RgbaImage>, RpcError> {
    let (width, height) = (
        document["width"].as_u64().unwrap_or(1) as u32,
        document["height"].as_u64().unwrap_or(1) as u32,
    );
    let mut frames = Vec::new();
    for layer in layers {
        let id = layer["id"].as_str().unwrap_or_default();
        // layer/export gives the layer's own pixels: no group contents, masks,
        // effects, adjustments, flips, rotation or blend mode.
        let export = host.export_layer(id, "pixels", None)?;
        let pixels = image::open(&export.path)
            .map_err(RpcError::internal)?
            .to_rgba8();
        let mut canvas = RgbaImage::new(width, height);
        imageops::overlay(
            &mut canvas,
            &pixels,
            export.x.round() as i64,
            export.y.round() as i64,
        );
        frames.push(canvas);
    }
    Ok(frames)
}

fn frames_by_composite(host: &Host, layers: &[&Value]) -> Result<Vec<RgbaImage>, RpcError> {
    let ids: Vec<&str> = layers.iter().filter_map(|l| l["id"].as_str()).collect();
    let visible: Vec<bool> = layers
        .iter()
        .map(|l| l["visible"].as_bool().unwrap_or(true))
        .collect();
    let mut frames = Vec::new();
    for shown in &ids {
        // No way to render a subset of layers without editing the document:
        // every frame costs an undo step and marks the document modified.
        let edits = ids
            .iter()
            .map(|id| json!({"op": "set", "layer": id, "visible": id == shown}))
            .collect();
        host.edit("Show animation frame", edits)?;
        let export = host.export_document(None)?;
        frames.push(
            image::open(&export.path)
                .map_err(RpcError::internal)?
                .to_rgba8(),
        );
    }
    let restore = ids
        .iter()
        .zip(&visible)
        .map(|(id, visible)| json!({"op": "set", "layer": id, "visible": visible}))
        .collect();
    host.edit("Restore layer visibility", restore)?;
    Ok(frames)
}

fn main() {
    let settings = Arc::new(Mutex::new(Value::Null));
    let on_settings = settings.clone();
    Plugin::new()
        .on_settings(move |s| *on_settings.lock().unwrap() = s.settings.clone())
        .exporter("gif", move |host, path, _image, document| {
            let settings = settings.lock().unwrap().clone();
            let default_delay = settings["delay"].as_u64().unwrap_or(100) as u32;
            let mode = settings["frames"].as_str().unwrap_or("pixels").to_owned();
            let looping = settings["loop"].as_bool().unwrap_or(true);
            // `document` is the structure of the tab that was exported, but
            // layer/export and document/export read the *current* tab, which
            // may have changed since. Check rather than export the wrong one.
            let current = host.document()?;
            if current["id"] != document["id"] {
                return Err(RpcError::internal(
                    "The document changed tabs during export; try again",
                ));
            }
            // Bottom-to-top top-level layers are the frames, as in GIMP.
            let all = document["layers"].as_array().cloned().unwrap_or_default();
            let mut layers: Vec<&Value> = all.iter().filter(|l| l["parent"].is_null()).collect();
            // document/get lists layers bottom to top, so this is frame order.
            let skipped: Vec<String> = layers
                .iter()
                .filter(|l| mode == "pixels" && l["kind"] == "group")
                .map(|l| l["name"].as_str().unwrap_or_default().to_owned())
                .collect();
            if mode == "pixels" {
                layers.retain(|l| l["kind"] != "group");
            }
            if layers.is_empty() {
                return Err(RpcError::internal(
                    "No frames: the document has no top-level pixel layers",
                ));
            }
            let frames = match mode.as_str() {
                "composite" => frames_by_composite(host, &layers)?,
                _ => frames_by_pixels(host, document, &layers)?,
            };
            let file = File::create(path).map_err(RpcError::internal)?;
            let mut encoder = GifEncoder::new_with_speed(BufWriter::new(file), 10);
            if looping {
                encoder
                    .set_repeat(Repeat::Infinite)
                    .map_err(RpcError::internal)?;
            }
            for (pixels, layer) in frames.into_iter().zip(&layers) {
                let ms = delay_from_name(layer["name"].as_str().unwrap_or_default())
                    .unwrap_or(default_delay);
                let frame = Frame::from_parts(pixels, 0, 0, Delay::from_numer_denom_ms(ms, 1));
                encoder.encode_frame(frame).map_err(RpcError::internal)?;
            }
            if !skipped.is_empty() {
                host.status(format!(
                    "Skipped groups (no pixels of their own): {}",
                    skipped.join(", ")
                ));
            }
            Ok(())
        })
        .importer("gif", |_host, path, work_dir| {
            // Never called by Xuan 0.5: .gif is a built-in extension.
            let decoder = GifDecoder::new(std::io::BufReader::new(File::open(path)?))
                .map_err(RpcError::internal)?;
            let frames = decoder
                .into_frames()
                .collect_frames()
                .map_err(RpcError::internal)?;
            let (width, height) = frames.first().map_or((1, 1), |f| f.buffer().dimensions());
            let mut layers = Vec::new();
            for (index, frame) in frames.iter().enumerate() {
                let (numer, denom) = frame.delay().numer_denom_ms();
                let file = work_dir.join(format!("frame-{index}.png"));
                frame.buffer().save(&file).map_err(RpcError::internal)?;
                layers.push(json!({
                    "name": format!("Frame {} ({}ms)", index + 1, numer / denom.max(1)),
                    "image": file,
                    "visible": index == 0,
                }));
            }
            Ok(json!({"width": width, "height": height, "layers": layers}))
        })
        .run();
}

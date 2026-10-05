//! What plugins see of a document and how they change it: JSON descriptions,
//! PNG exports and batched edits that the editor records as one undo step.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    blend::BlendMode,
    document::{Document, Layer, Mask, Transform},
};

/// Describe a document for `document/get` and the `document` field of jobs.
pub fn describe(document: &Document) -> Value {
    let selection = document
        .selection
        .as_deref()
        .and_then(crate::selection::bounds)
        .map(|(x0, y0, x1, y1)| json!({"x": x0, "y": y0, "width": x1 - x0, "height": y1 - y0}));
    json!({
        "id": document.id,
        "width": document.width,
        "height": document.height,
        "resolution": document.resolution,
        "active": document.active,
        "selection": selection,
        "layers": document.layers.iter().map(describe_layer).collect::<Vec<_>>(),
    })
}

pub fn describe_layer(layer: &Layer) -> Value {
    let kind = if layer.group {
        "group"
    } else if layer.standalone_mask {
        "mask"
    } else if layer.adjustment.is_some() {
        "adjustment"
    } else if layer.filter.is_some() {
        "filter"
    } else if layer.text.is_some() {
        "text"
    } else if layer.raw.is_some() {
        "raw"
    } else {
        "image"
    };
    let (width, height) = layer
        .pixels
        .as_ref()
        .map_or((0, 0), |pixels| pixels.dimensions());
    json!({
        "id": layer.id,
        "name": layer.name,
        "kind": kind,
        "visible": layer.visible,
        "locked": layer.locked,
        "opacity": layer.opacity,
        "blend": layer.blend,
        "parent": layer.parent,
        "clip_to": layer.clip_to,
        "x": layer.transform.x,
        "y": layer.transform.y,
        "width": layer.transform.width,
        "height": layer.transform.height,
        "rotation": layer.transform.rotation,
        "pixel_width": width,
        "pixel_height": height,
        "has_mask": layer.mask.is_some(),
        "generated": layer.generated,
    })
}

/// A PNG written for a plugin, with where it sits in the document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub x: f32,
    pub y: f32,
    /// Export pixels per source pixel, below one when downscaled.
    pub scale: f32,
}

pub fn write_png(image: &RgbaImage, path: &Path) -> Result<()> {
    image
        .save_with_format(path, image::ImageFormat::Png)
        .with_context(|| format!("Cannot write {}", path.display()))
}

pub fn write_gray_png(image: &GrayImage, path: &Path) -> Result<()> {
    image
        .save_with_format(path, image::ImageFormat::Png)
        .with_context(|| format!("Cannot write {}", path.display()))
}

pub fn read_png(path: &Path) -> Result<RgbaImage> {
    let image = crate::io::import_image(path)?;
    crate::document::validate_size(image.width(), image.height())?;
    Ok(image)
}

pub fn read_gray_png(path: &Path) -> Result<GrayImage> {
    let image = read_png(path)?;
    Ok(image::DynamicImage::ImageRgba8(image).to_luma8())
}

/// Shrink so the longest side is at most `max_side`; returns the scale used.
pub fn fit(image: &RgbaImage, max_side: Option<u32>) -> (RgbaImage, f32) {
    let longest = image.width().max(image.height());
    match max_side {
        Some(max) if longest > max && max > 0 => {
            let scale = max as f32 / longest as f32;
            let width = ((image.width() as f32 * scale).round() as u32).max(1);
            let height = ((image.height() as f32 * scale).round() as u32).max(1);
            (
                crate::render::resize_quality(image, width, height),
                width as f32 / image.width() as f32,
            )
        }
        _ => (image.clone(), 1.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum What {
    Pixels,
    Mask,
}

/// Write a layer's source pixels (or its mask) as `name` in `dir`.
pub fn export_layer(
    document: &Document,
    layer: Uuid,
    what: What,
    max_side: Option<u32>,
    dir: &Path,
    name: &str,
) -> Result<Export> {
    let layer = find(document, layer)?;
    let path = dir.join(name);
    let (width, height, scale) = match what {
        What::Pixels => {
            let pixels = layer
                .pixels
                .as_deref()
                .with_context(|| format!("Layer {} has no pixels", layer.name))?;
            let (image, scale) = fit(pixels, max_side);
            write_png(&image, &path)?;
            (image.width(), image.height(), scale)
        }
        What::Mask => {
            let mask = layer
                .mask
                .as_ref()
                .with_context(|| format!("Layer {} has no mask", layer.name))?;
            write_gray_png(&mask.pixels, &path)?;
            (mask.pixels.width(), mask.pixels.height(), 1.0)
        }
    };
    Ok(Export {
        path,
        width,
        height,
        x: layer.transform.x,
        y: layer.transform.y,
        scale,
    })
}

/// Write the flattened document.
pub fn export_composite(
    document: &Document,
    max_side: Option<u32>,
    dir: &Path,
    name: &str,
) -> Result<Export> {
    let longest = document.width.max(document.height);
    let scale = match max_side {
        Some(max) if longest > max && max > 0 => max as f32 / longest as f32,
        _ => 1.0,
    };
    let width = ((document.width as f32 * scale).round() as u32).max(1);
    let height = ((document.height as f32 * scale).round() as u32).max(1);
    let image = if scale < 1.0 {
        crate::render::render_scaled(document, width, height)
    } else {
        crate::render::render(document)
    };
    let path = dir.join(name);
    write_png(&image, &path)?;
    Ok(Export {
        path,
        width: image.width(),
        height: image.height(),
        x: 0.0,
        y: 0.0,
        scale: width as f32 / document.width as f32,
    })
}

/// Write the selection mask, if there is one, cropped to its bounds.
pub fn export_selection(document: &Document, dir: &Path, name: &str) -> Result<Option<Export>> {
    let Some(selection) = document.selection.as_deref() else {
        return Ok(None);
    };
    let Some((x0, y0, x1, y1)) = crate::selection::bounds(selection) else {
        return Ok(None);
    };
    let cropped = image::imageops::crop_imm(selection, x0, y0, x1 - x0, y1 - y0).to_image();
    let path = dir.join(name);
    write_gray_png(&cropped, &path)?;
    Ok(Some(Export {
        path,
        width: cropped.width(),
        height: cropped.height(),
        x: x0 as f32,
        y: y0 as f32,
        scale: 1.0,
    }))
}

/// One change in a `document/edit` batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    AddLayer {
        image: PathBuf,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        height: Option<f32>,
        #[serde(default)]
        mask: Option<PathBuf>,
        #[serde(default)]
        above: Option<Uuid>,
        #[serde(default)]
        opacity: Option<f32>,
        #[serde(default)]
        blend: Option<BlendMode>,
    },
    ReplacePixels {
        layer: Uuid,
        image: PathBuf,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
    },
    Set {
        layer: Uuid,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        visible: Option<bool>,
        #[serde(default)]
        locked: Option<bool>,
        #[serde(default)]
        opacity: Option<f32>,
        #[serde(default)]
        blend: Option<BlendMode>,
    },
    RemoveLayer {
        layer: Uuid,
    },
    SetMask {
        layer: Uuid,
        #[serde(default)]
        mask: Option<PathBuf>,
    },
    SetSelection {
        #[serde(default)]
        mask: Option<PathBuf>,
    },
    Select {
        layer: Uuid,
    },
}

/// Apply a batch. The caller clones the document first and only keeps the
/// result when every edit succeeded, so a failing batch changes nothing.
pub fn apply(document: &mut Document, edits: &[Edit]) -> Result<Vec<Uuid>> {
    ensure!(edits.len() <= 1000, "Too many edits in one batch");
    let mut added = Vec::new();
    for edit in edits {
        match edit {
            Edit::AddLayer {
                image,
                name,
                x,
                y,
                width,
                height,
                mask,
                above,
                opacity,
                blend,
            } => {
                let pixels = read_png(image)?;
                let mut layer = Layer::image(
                    name.clone().unwrap_or_else(|| "Plugin layer".into()),
                    pixels,
                );
                layer.transform.x = x.unwrap_or(0.0);
                layer.transform.y = y.unwrap_or(0.0);
                if let Some(width) = width {
                    layer.transform.width = *width;
                }
                if let Some(height) = height {
                    layer.transform.height = *height;
                }
                ensure!(layer.transform.valid(), "Invalid layer placement");
                if let Some(mask) = mask {
                    layer.mask = Some(Mask {
                        pixels: Arc::new(read_gray_png(mask)?),
                        ..Mask::white()
                    });
                }
                if let Some(opacity) = opacity {
                    layer.opacity = valid_opacity(*opacity)?;
                }
                if let Some(blend) = blend {
                    layer.blend = *blend;
                }
                if let Some(above) = above {
                    find(document, *above)?;
                    document.select(*above, false);
                }
                added.push(layer.id);
                document.insert(layer);
            }
            Edit::ReplacePixels { layer, image, x, y } => {
                let pixels = read_png(image)?;
                let target = find_mut(document, *layer)?;
                ensure!(!target.locked, "Layer {} is locked", target.name);
                ensure!(
                    target.pixels.is_some(),
                    "Layer {} has no pixels",
                    target.name
                );
                target.raw = None;
                target.text = None;
                target.shape = None;
                if let Some(x) = x {
                    target.transform.x = *x;
                }
                if let Some(y) = y {
                    target.transform.y = *y;
                }
                target.pixels = Some(Arc::new(pixels));
            }
            Edit::Set {
                layer,
                name,
                visible,
                locked,
                opacity,
                blend,
            } => {
                let target = find_mut(document, *layer)?;
                if let Some(name) = name {
                    ensure!(name.len() <= 256, "Layer name too long");
                    target.name = name.clone();
                }
                if let Some(visible) = visible {
                    target.visible = *visible;
                }
                if let Some(locked) = locked {
                    target.locked = *locked;
                }
                if let Some(opacity) = opacity {
                    target.opacity = valid_opacity(*opacity)?;
                }
                if let Some(blend) = blend {
                    target.blend = *blend;
                }
            }
            Edit::RemoveLayer { layer } => {
                find(document, *layer)?;
                let removed = document.descendants(*layer);
                document
                    .layers
                    .retain(|l| l.id != *layer && !removed.contains(&l.id));
                for l in &mut document.layers {
                    if l.clip_to
                        .is_some_and(|id| id == *layer || removed.contains(&id))
                    {
                        l.clip_to = None;
                    }
                }
                if document.active == Some(*layer)
                    || document.active.is_some_and(|id| removed.contains(&id))
                {
                    document.active = document.layers.last().map(|l| l.id);
                }
                document
                    .selected
                    .retain(|id| id != layer && !removed.contains(id));
            }
            Edit::SetMask { layer, mask } => {
                let target = find_mut(document, *layer)?;
                ensure!(
                    !target.standalone_mask,
                    "Use replace_pixels for mask layers"
                );
                match mask {
                    Some(path) => {
                        let pixels = Arc::new(read_gray_png(path)?);
                        match &mut target.mask {
                            Some(mask) => mask.pixels = pixels,
                            None => {
                                target.mask = Some(Mask {
                                    pixels,
                                    ..Mask::white()
                                })
                            }
                        }
                    }
                    None => target.mask = None,
                }
            }
            Edit::SetSelection { mask } => {
                document.selection = match mask {
                    Some(path) => {
                        let image = read_gray_png(path)?;
                        ensure!(
                            image.dimensions() == (document.width, document.height),
                            "The selection mask must match the document size"
                        );
                        Some(Arc::new(image))
                    }
                    None => None,
                };
            }
            Edit::Select { layer } => {
                find(document, *layer)?;
                document.select(*layer, false);
            }
        }
    }
    document.validate()?;
    Ok(added)
}

fn valid_opacity(opacity: f32) -> Result<f32> {
    ensure!(
        opacity.is_finite() && (0.0..=1.0).contains(&opacity),
        "Opacity must be between 0 and 1"
    );
    Ok(opacity)
}

fn find(document: &Document, id: Uuid) -> Result<&Layer> {
    document
        .layers
        .iter()
        .find(|layer| layer.id == id)
        .with_context(|| format!("No layer {id}"))
}

fn find_mut(document: &mut Document, id: Uuid) -> Result<&mut Layer> {
    document
        .layers
        .iter_mut()
        .find(|layer| layer.id == id)
        .with_context(|| format!("No layer {id}"))
}

/// The transform covering a pixel rectangle of a layer's source image.
pub fn sub_transform(
    transform: Transform,
    source: (u32, u32),
    crop: (f32, f32, f32, f32),
) -> Transform {
    let (w, h) = (source.0 as f32, source.1 as f32);
    let (cx, cy, cw, ch) = crop;
    let unit = |x: f32, y: f32| crate::document::Point::new(x / w, y / h);
    let center = transform.point(unit(cx + cw * 0.5, cy + ch * 0.5));
    let mut result = transform;
    result.width = (transform.width * cw / w).max(1.0);
    result.height = (transform.height * ch / h).max(1.0);
    result.x = center.x - result.width * 0.5;
    result.y = center.y - result.height * 0.5;
    if transform.warp.is_some() {
        result.warp = None;
        let corners = [
            unit(cx, cy),
            unit(cx + cw, cy),
            unit(cx + cw, cy + ch),
            unit(cx, cy + ch),
        ]
        .map(|p| result.inverse(transform.point(p)));
        if crate::geometry::Homography::from_quad(corners).is_some() {
            result.warp = Some(corners);
        }
    }
    result
}

/// FNV-1a over the pixel bytes, for `Generated::source_hash`.
pub fn pixel_hash(image: &RgbaImage) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in image.as_raw() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("fnv1a:{hash:016x}")
}

pub fn blend_names() -> Vec<String> {
    BlendMode::ALL
        .iter()
        .map(|mode| {
            serde_json::to_value(mode)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Point;

    fn png(dir: &Path, name: &str, color: [u8; 4], size: u32) -> PathBuf {
        let path = dir.join(name);
        write_png(
            &RgbaImage::from_pixel(size, size, image::Rgba(color)),
            &path,
        )
        .unwrap();
        path
    }

    #[test]
    fn describes_layers_and_selection() {
        let mut document = Document::new(40, 30).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::new(40, 30)));
        let mut selection = GrayImage::new(40, 30);
        selection.put_pixel(10, 5, image::Luma([255]));
        selection.put_pixel(20, 15, image::Luma([255]));
        document.selection = Some(Arc::new(selection));
        let value = describe(&document);
        assert_eq!(value["width"], 40);
        assert_eq!(
            value["selection"],
            json!({"x": 10, "y": 5, "width": 11, "height": 11})
        );
        assert_eq!(value["layers"][0]["kind"], "image");
        assert_eq!(value["layers"][0]["pixel_width"], 40);
        assert_eq!(value["layers"][0]["blend"], "Normal");
        assert!(blend_names().contains(&"Multiply".to_owned()));
    }

    #[test]
    fn exports_fit_the_longest_side_and_report_scale() {
        let dir = tempfile::tempdir().unwrap();
        let mut document = Document::new(400, 200).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            400,
            200,
            image::Rgba([10, 20, 30, 255]),
        )));
        document.layers[0].transform.x = 7.0;
        let id = document.layers[0].id;
        let export =
            export_layer(&document, id, What::Pixels, Some(100), dir.path(), "l.png").unwrap();
        assert_eq!((export.width, export.height, export.x), (100, 50, 7.0));
        assert_eq!(export.scale, 0.25);
        assert_eq!(read_png(&export.path).unwrap().dimensions(), (100, 50));
        let composite = export_composite(&document, None, dir.path(), "c.png").unwrap();
        assert_eq!(
            (composite.width, composite.height, composite.scale),
            (400, 200, 1.0)
        );
        assert!(
            export_selection(&document, dir.path(), "s.png")
                .unwrap()
                .is_none()
        );
        assert!(export_layer(&document, id, What::Mask, None, dir.path(), "m.png").is_err());
        assert!(
            export_layer(
                &document,
                Uuid::new_v4(),
                What::Pixels,
                None,
                dir.path(),
                "x.png"
            )
            .is_err()
        );
    }

    #[test]
    fn edits_apply_in_order_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let mut document = Document::new(64, 64).unwrap();
        let base = document.layers[0].id;
        document.layers[0].pixels = Some(Arc::new(RgbaImage::new(64, 64)));
        let red = png(dir.path(), "red.png", [255, 0, 0, 255], 16);
        let mask = dir.path().join("mask.png");
        write_gray_png(&GrayImage::from_pixel(4, 4, image::Luma([128])), &mask).unwrap();
        let added = apply(
            &mut document,
            &[
                Edit::AddLayer {
                    image: red.clone(),
                    name: Some("Red".into()),
                    x: Some(10.0),
                    y: Some(20.0),
                    width: None,
                    height: None,
                    mask: Some(mask.clone()),
                    above: Some(base),
                    opacity: Some(0.5),
                    blend: Some(BlendMode::Multiply),
                },
                Edit::Set {
                    layer: base,
                    name: Some("Base".into()),
                    visible: Some(false),
                    locked: None,
                    opacity: None,
                    blend: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(added.len(), 1);
        let layer = document.layers.iter().find(|l| l.id == added[0]).unwrap();
        assert_eq!(layer.name, "Red");
        assert_eq!((layer.transform.x, layer.transform.y), (10.0, 20.0));
        assert_eq!(layer.opacity, 0.5);
        assert_eq!(layer.blend, BlendMode::Multiply);
        assert_eq!(layer.mask.as_ref().unwrap().pixels.get_pixel(0, 0)[0], 128);
        assert_eq!(document.active, Some(added[0]));
        assert_eq!(document.layers[0].name, "Base");
        assert!(!document.layers[0].visible);

        apply(
            &mut document,
            &[
                Edit::ReplacePixels {
                    layer: base,
                    image: red.clone(),
                    x: None,
                    y: Some(3.0),
                },
                Edit::SetMask {
                    layer: base,
                    mask: None,
                },
                Edit::Select { layer: base },
            ],
        )
        .unwrap();
        assert_eq!(
            document.layers[0].pixels.as_ref().unwrap().dimensions(),
            (16, 16)
        );
        assert_eq!(document.layers[0].transform.y, 3.0);
        assert_eq!(document.active, Some(base));

        assert!(
            apply(
                &mut document,
                &[Edit::Set {
                    layer: base,
                    name: None,
                    visible: None,
                    locked: None,
                    opacity: Some(2.0),
                    blend: None
                }]
            )
            .is_err()
        );
        assert!(
            apply(
                &mut document,
                &[Edit::RemoveLayer {
                    layer: Uuid::new_v4()
                }]
            )
            .is_err()
        );
        assert!(apply(&mut document, &[Edit::SetSelection { mask: Some(mask) }]).is_err());
        document.layers[0].locked = true;
        assert!(
            apply(
                &mut document,
                &[Edit::ReplacePixels {
                    layer: base,
                    image: red,
                    x: None,
                    y: None
                }]
            )
            .is_err()
        );
        apply(&mut document, &[Edit::RemoveLayer { layer: added[0] }]).unwrap();
        assert_eq!(document.layers.len(), 1);
        let selection = dir.path().join("sel.png");
        write_gray_png(
            &GrayImage::from_pixel(64, 64, image::Luma([255])),
            &selection,
        )
        .unwrap();
        apply(
            &mut document,
            &[Edit::SetSelection {
                mask: Some(selection),
            }],
        )
        .unwrap();
        assert!(document.selection.is_some());
        let parsed: Edit = serde_json::from_value(json!({"op": "select", "layer": base})).unwrap();
        assert_eq!(parsed, Edit::Select { layer: base });
    }

    #[test]
    fn sub_transforms_cover_the_cropped_source_area() {
        let mut transform = Transform::new(200, 100);
        transform.x = 10.0;
        transform.y = 20.0;
        let sub = sub_transform(transform, (200, 100), (50.0, 25.0, 100.0, 50.0));
        assert_eq!(
            (sub.x, sub.y, sub.width, sub.height),
            (60.0, 45.0, 100.0, 50.0)
        );
        assert!(sub.warp.is_none());
        let mut rotated = transform;
        rotated.rotation = 90.0;
        let sub = sub_transform(rotated, (200, 100), (0.0, 0.0, 100.0, 100.0));
        let expected = rotated.point(Point::new(0.25, 0.5));
        let actual = sub.center();
        assert!((expected.x - actual.x).abs() < 1e-3 && (expected.y - actual.y).abs() < 1e-3);
        assert_eq!(sub.rotation, 90.0);
        let hash = pixel_hash(&RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 4])));
        assert!(hash.starts_with("fnv1a:"));
        assert_ne!(hash, pixel_hash(&RgbaImage::new(2, 2)));
    }
}

//! Preparing what an action receives and fitting what it returns back into
//! the document. Plugins only see the exported source image; every scale,
//! crop and layer transform is undone here when the result comes back.
use std::{path::Path, sync::Arc};

use anyhow::{Context, Result, ensure};
use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{
    edits::{Export, fit, sub_transform, write_gray_png, write_png},
    manifest::{Source, SourceKind},
};
use crate::document::{Document, Layer, Mask, Point, Transform};

/// A region the user marked on the canvas, in document pixels.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Coverage for a selection-shaped region, at document size.
    #[serde(skip)]
    pub mask: Option<Arc<GrayImage>>,
    #[serde(default)]
    pub fields: Map<String, Value>,
}

impl Region {
    pub fn rect(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            mask: None,
            fields: Map::new(),
        }
    }

    pub fn from_mask(mask: Arc<GrayImage>) -> Option<Self> {
        let (x0, y0, x1, y1) = crate::selection::bounds(&mask)?;
        Some(Self {
            x: x0 as f32,
            y: y0 as f32,
            width: (x1 - x0) as f32,
            height: (y1 - y0) as f32,
            mask: Some(mask),
            fields: Map::new(),
        })
    }

    pub fn contains(&self, point: Point) -> bool {
        (self.x..self.x + self.width).contains(&point.x)
            && (self.y..self.y + self.height).contains(&point.y)
    }
}

/// The source sent to a plugin and everything needed to place results back.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub export: Option<Export>,
    /// The layer whose pixels were sent, for `layer` sources.
    pub layer: Option<Uuid>,
    /// Full size of the source raster the crop was taken from.
    pub source_size: (u32, u32),
    /// Crop in source pixels: x, y, width, height.
    pub crop: (f32, f32, f32, f32),
    /// Export pixels per source pixel.
    pub scale: f32,
    /// Document placement of the whole source raster.
    pub transform: Transform,
    /// Regions in export coordinates, as sent to the plugin.
    pub regions: Vec<Value>,
    pub hash: Option<String>,
}

impl Prepared {
    pub fn none() -> Self {
        Self {
            export: None,
            layer: None,
            source_size: (0, 0),
            crop: (0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
            transform: Transform::new(1, 1),
            regions: Vec::new(),
            hash: None,
        }
    }

    /// JSON for the `source` field of `action/run`.
    pub fn describe(&self) -> Value {
        match &self.export {
            Some(export) => json!({
                "path": export.path,
                "width": export.width,
                "height": export.height,
                "layer": self.layer,
                "scale": self.scale,
                "offset": {"x": self.crop.0, "y": self.crop.1},
                "document_x": export.x,
                "document_y": export.y,
            }),
            None => Value::Null,
        }
    }

    /// Map a point in export coordinates to the document.
    pub fn to_document(&self, x: f32, y: f32) -> Point {
        let (w, h) = (self.source_size.0 as f32, self.source_size.1 as f32);
        let sx = self.crop.0 + x / self.scale;
        let sy = self.crop.1 + y / self.scale;
        self.transform
            .point(Point::new(sx / w.max(1.0), sy / h.max(1.0)))
    }

    /// Map a document point to export coordinates.
    pub fn from_document(&self, point: Point) -> (f32, f32) {
        let unit = self.transform.inverse(point);
        let (w, h) = (self.source_size.0 as f32, self.source_size.1 as f32);
        (
            (unit.x * w - self.crop.0) * self.scale,
            (unit.y * h - self.crop.1) * self.scale,
        )
    }

    /// The document transform of an output covering export pixels
    /// `x, y, width, height`.
    pub fn placement(&self, x: f32, y: f32, width: f32, height: f32) -> Transform {
        sub_transform(
            self.transform,
            self.source_size,
            (
                self.crop.0 + x / self.scale,
                self.crop.1 + y / self.scale,
                width / self.scale,
                height / self.scale,
            ),
        )
    }
}

/// Export the source an action asked for and translate the regions into its
/// coordinates. Files go in `dir`.
pub fn prepare(
    document: &Document,
    source: &Source,
    regions: &[Region],
    dir: &Path,
) -> Result<Prepared> {
    let (pixels, layer, transform): (Arc<RgbaImage>, Option<Uuid>, Transform) = match source.from {
        SourceKind::None => return Ok(Prepared::none()),
        SourceKind::Layer => {
            let layer = document
                .active()
                .filter(|layer| layer.pixels.is_some() && !layer.group)
                .context("Select an image layer first")?;
            (
                layer.pixels.clone().unwrap(),
                Some(layer.id),
                layer.transform,
            )
        }
        SourceKind::Composite | SourceKind::Selection => (
            Arc::new(crate::render::render(document)),
            None,
            Transform::new(document.width, document.height),
        ),
    };
    let (w, h) = pixels.dimensions();
    let mut prepared = Prepared {
        export: None,
        layer,
        source_size: (w, h),
        crop: (0.0, 0.0, w as f32, h as f32),
        scale: 1.0,
        transform,
        regions: Vec::new(),
        hash: Some(super::edits::pixel_hash(&pixels)),
    };
    // Regions in source pixels: bounding boxes of the mapped corners.
    let boxes: Vec<(f32, f32, f32, f32)> = regions
        .iter()
        .map(|region| {
            let corners = [
                (region.x, region.y),
                (region.x + region.width, region.y),
                (region.x + region.width, region.y + region.height),
                (region.x, region.y + region.height),
            ]
            .map(|(x, y)| {
                let unit = transform.inverse(Point::new(x, y));
                (unit.x * w as f32, unit.y * h as f32)
            });
            let min_x = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
            let min_y = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min);
            let max_x = corners
                .iter()
                .map(|c| c.0)
                .fold(f32::NEG_INFINITY, f32::max);
            let max_y = corners
                .iter()
                .map(|c| c.1)
                .fold(f32::NEG_INFINITY, f32::max);
            (min_x, min_y, max_x - min_x, max_y - min_y)
        })
        .collect();
    let mut crop = prepared.crop;
    if source.from == SourceKind::Selection
        && let Some((x0, y0, x1, y1)) = document
            .selection
            .as_deref()
            .and_then(crate::selection::bounds)
    {
        crop = (x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32);
    } else if source.crop_to_regions && !boxes.is_empty() {
        let min_x = boxes.iter().map(|b| b.0).fold(f32::INFINITY, f32::min);
        let min_y = boxes.iter().map(|b| b.1).fold(f32::INFINITY, f32::min);
        let max_x = boxes
            .iter()
            .map(|b| b.0 + b.2)
            .fold(f32::NEG_INFINITY, f32::max);
        let max_y = boxes
            .iter()
            .map(|b| b.1 + b.3)
            .fold(f32::NEG_INFINITY, f32::max);
        let pad = source.padding * (max_x - min_x).max(max_y - min_y);
        let x0 = (min_x - pad).floor().max(0.0);
        let y0 = (min_y - pad).floor().max(0.0);
        let x1 = (max_x + pad).ceil().min(w as f32);
        let y1 = (max_y + pad).ceil().min(h as f32);
        if x1 > x0 && y1 > y0 {
            crop = (x0, y0, x1 - x0, y1 - y0);
        }
    }
    prepared.crop = crop;
    let cropped = if crop == (0.0, 0.0, w as f32, h as f32) {
        (*pixels).clone()
    } else {
        image::imageops::crop_imm(
            &*pixels,
            crop.0 as u32,
            crop.1 as u32,
            (crop.2 as u32).max(1),
            (crop.3 as u32).max(1),
        )
        .to_image()
    };
    let (exported, scale) = fit(&cropped, source.max_side);
    prepared.scale = scale;
    let path = dir.join("source.png");
    write_png(&exported, &path)?;
    let origin = transform.point(Point::new(crop.0 / w as f32, crop.1 / h as f32));
    prepared.export = Some(Export {
        path,
        width: exported.width(),
        height: exported.height(),
        x: origin.x,
        y: origin.y,
        scale,
    });
    for (index, (region, bounds)) in regions.iter().zip(&boxes).enumerate() {
        let x = (bounds.0 - crop.0) * scale;
        let y = (bounds.1 - crop.1) * scale;
        let width = bounds.2 * scale;
        let height = bounds.3 * scale;
        let mut mask_path = None;
        if let Some(mask) = &region.mask {
            let (ew, eh) = (exported.width(), exported.height());
            let projected = GrayImage::from_fn(ew, eh, |px, py| {
                let point = prepared.to_document(px as f32 + 0.5, py as f32 + 0.5);
                let coverage = crate::selection::coverage(Some(mask), point);
                image::Luma([(coverage * 255.0).round() as u8])
            });
            let path = dir.join(format!("region-{}.png", index + 1));
            write_gray_png(&projected, &path)?;
            mask_path = Some(path);
        }
        prepared.regions.push(json!({
            "index": index + 1,
            "x": x.round(),
            "y": y.round(),
            "width": width.round().max(1.0),
            "height": height.round().max(1.0),
            "mask": mask_path,
            "fields": region.fields,
        }));
    }
    Ok(prepared)
}

/// A new layer for an image the plugin returned at export position `x, y`.
pub fn place_layer(
    prepared: &Prepared,
    name: &str,
    image: RgbaImage,
    x: f32,
    y: f32,
    regions: Option<&[Region]>,
) -> Result<Layer> {
    ensure!(prepared.export.is_some(), "The action had no source image");
    let (width, height) = (image.width() as f32, image.height() as f32);
    let mut layer = Layer::image(name, image);
    layer.transform = prepared.placement(x, y, width, height);
    ensure!(
        layer.transform.valid(),
        "The result does not fit the document"
    );
    if let Some(regions) = regions
        && !regions.is_empty()
    {
        layer.mask = Some(Mask {
            pixels: Arc::new(region_mask(
                prepared,
                &layer.transform,
                (width as u32, height as u32),
                regions,
            )),
            ..Mask::white()
        });
    }
    Ok(layer)
}

/// A soft-edged mask covering the regions, in the output layer's pixel grid.
pub fn region_mask(
    prepared: &Prepared,
    transform: &Transform,
    size: (u32, u32),
    regions: &[Region],
) -> GrayImage {
    let (w, h) = size;
    let mut mask = GrayImage::from_fn(w, h, |px, py| {
        let point = transform.point(Point::new(
            (px as f32 + 0.5) / w as f32,
            (py as f32 + 0.5) / h as f32,
        ));
        let covered = regions.iter().any(|region| match &region.mask {
            Some(mask) => crate::selection::coverage(Some(mask), point) > 0.5,
            None => region.contains(point),
        });
        image::Luma([if covered { 255 } else { 0 }])
    });
    let _ = prepared;
    let radius = ((w.max(h) as f32) * 0.015).round().max(1.0) as u32;
    box_blur(&mut mask, radius);
    mask
}

/// Two passes of a separable box blur, enough for a soft mask edge.
pub fn box_blur(mask: &mut GrayImage, radius: u32) {
    let (w, h) = mask.dimensions();
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    for _ in 0..2 {
        let source = mask.clone();
        for y in 0..h {
            let mut sum = 0u32;
            let mut count = 0u32;
            for x in 0..radius.min(w) {
                sum += u32::from(source.get_pixel(x, y)[0]);
                count += 1;
            }
            for x in 0..w {
                if x + radius < w {
                    sum += u32::from(source.get_pixel(x + radius, y)[0]);
                    count += 1;
                }
                if x > radius {
                    sum -= u32::from(source.get_pixel(x - radius - 1, y)[0]);
                    count -= 1;
                }
                mask.put_pixel(x, y, image::Luma([(sum / count.max(1)) as u8]));
            }
        }
        let source = mask.clone();
        for x in 0..w {
            let mut sum = 0u32;
            let mut count = 0u32;
            for y in 0..radius.min(h) {
                sum += u32::from(source.get_pixel(x, y)[0]);
                count += 1;
            }
            for y in 0..h {
                if y + radius < h {
                    sum += u32::from(source.get_pixel(x, y + radius)[0]);
                    count += 1;
                }
                if y > radius {
                    sum -= u32::from(source.get_pixel(x, y - radius - 1)[0]);
                    count -= 1;
                }
                mask.put_pixel(x, y, image::Luma([(sum / count.max(1)) as u8]));
            }
        }
    }
}

/// Paste a result over the source layer's pixels, for `result.into = "replace"`.
pub fn replace_pixels(
    prepared: &Prepared,
    source: &RgbaImage,
    image: &RgbaImage,
    x: f32,
    y: f32,
) -> RgbaImage {
    let mut pixels = source.clone();
    let width = ((image.width() as f32 / prepared.scale).round() as u32).max(1);
    let height = ((image.height() as f32 / prepared.scale).round() as u32).max(1);
    let resized = if (width, height) == image.dimensions() {
        image.clone()
    } else {
        crate::render::resize_quality(image, width, height)
    };
    let ox = (prepared.crop.0 + x / prepared.scale).round() as i64;
    let oy = (prepared.crop.1 + y / prepared.scale).round() as i64;
    image::imageops::replace(&mut pixels, &resized, ox, oy);
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Document {
        let mut document = Document::new(400, 300).unwrap();
        let mut pixels = RgbaImage::from_pixel(200, 150, image::Rgba([0, 0, 255, 255]));
        pixels.put_pixel(100, 75, image::Rgba([255, 0, 0, 255]));
        document.layers[0].pixels = Some(Arc::new(pixels));
        // The layer is shown at twice its pixel size, offset into the canvas.
        document.layers[0].transform = Transform {
            x: 20.0,
            y: 10.0,
            width: 400.0,
            height: 300.0,
            ..Transform::new(200, 150)
        };
        document
    }

    #[test]
    fn layer_sources_crop_to_regions_and_map_coordinates_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let document = document();
        let source = Source {
            from: SourceKind::Layer,
            max_side: Some(50),
            crop_to_regions: true,
            padding: 0.0,
        };
        // A region over the red pixel in document space: layer pixel (100, 75)
        // sits at document (20 + 200, 10 + 150).
        let regions = [Region::rect(200.0, 140.0, 40.0, 40.0)];
        let prepared = prepare(&document, &source, &regions, dir.path()).unwrap();
        assert_eq!(prepared.layer, Some(document.layers[0].id));
        assert_eq!(prepared.source_size, (200, 150));
        assert_eq!(prepared.crop, (90.0, 65.0, 20.0, 20.0));
        let export = prepared.export.as_ref().unwrap();
        assert_eq!((export.width, export.height), (20, 20));
        assert_eq!(prepared.scale, 1.0);
        assert_eq!((export.x, export.y), (200.0, 140.0));
        assert_eq!(prepared.regions[0]["x"], 0.0);
        assert_eq!(prepared.regions[0]["width"], 20.0);
        let exported = super::super::edits::read_png(&export.path).unwrap();
        assert_eq!(exported.get_pixel(10, 10)[0], 255);
        let point = prepared.to_document(10.0, 10.0);
        assert!((point.x - 220.0).abs() < 1e-3 && (point.y - 160.0).abs() < 1e-3);
        let back = prepared.from_document(point);
        assert!((back.0 - 10.0).abs() < 1e-3 && (back.1 - 10.0).abs() < 1e-3);
        let placement = prepared.placement(0.0, 0.0, 20.0, 20.0);
        assert_eq!(
            (placement.x, placement.y, placement.width, placement.height),
            (200.0, 140.0, 40.0, 40.0)
        );

        let result = RgbaImage::from_pixel(20, 20, image::Rgba([0, 255, 0, 255]));
        let layer =
            place_layer(&prepared, "Edit", result.clone(), 0.0, 0.0, Some(&regions)).unwrap();
        assert_eq!(layer.transform.x, 200.0);
        let mask = &layer.mask.as_ref().unwrap().pixels;
        assert_eq!(mask.dimensions(), (20, 20));
        assert!(mask.get_pixel(10, 10)[0] > 200);
        let replaced = replace_pixels(
            &prepared,
            document.layers[0].pixels.as_ref().unwrap(),
            &result,
            0.0,
            0.0,
        );
        assert_eq!(replaced.get_pixel(100, 75)[1], 255);
        assert_eq!(replaced.get_pixel(0, 0)[2], 255);
        assert_eq!(replaced.dimensions(), (200, 150));
    }

    #[test]
    fn composites_downscale_and_selection_masks_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut document = document();
        let mut selection = GrayImage::new(400, 300);
        for y in 100..200 {
            for x in 100..300 {
                selection.put_pixel(x, y, image::Luma([255]));
            }
        }
        document.selection = Some(Arc::new(selection.clone()));
        let region = Region::from_mask(Arc::new(selection)).unwrap();
        assert_eq!(
            (region.x, region.y, region.width, region.height),
            (100.0, 100.0, 200.0, 100.0)
        );
        let source = Source {
            from: SourceKind::Composite,
            max_side: Some(200),
            crop_to_regions: false,
            padding: 0.25,
        };
        let prepared = prepare(
            &document,
            &source,
            std::slice::from_ref(&region),
            dir.path(),
        )
        .unwrap();
        assert_eq!(prepared.scale, 0.5);
        let export = prepared.export.as_ref().unwrap();
        assert_eq!((export.width, export.height), (200, 150));
        assert_eq!(prepared.regions[0]["x"], 50.0);
        assert_eq!(prepared.regions[0]["width"], 100.0);
        let mask_path = prepared.regions[0]["mask"].as_str().unwrap();
        let projected = super::super::edits::read_gray_png(Path::new(mask_path)).unwrap();
        assert_eq!(projected.dimensions(), (200, 150));
        assert_eq!(projected.get_pixel(100, 75)[0], 255);
        assert_eq!(projected.get_pixel(10, 10)[0], 0);
        let layer = place_layer(
            &prepared,
            "Out",
            RgbaImage::new(200, 150),
            0.0,
            0.0,
            Some(&[region]),
        )
        .unwrap();
        assert_eq!(
            (layer.transform.width, layer.transform.height),
            (400.0, 300.0)
        );
        let mask = &layer.mask.as_ref().unwrap().pixels;
        assert!(mask.get_pixel(100, 75)[0] > 200);
        assert_eq!(mask.get_pixel(5, 5)[0], 0);

        let selection_source = Source {
            from: SourceKind::Selection,
            ..source
        };
        let prepared = prepare(&document, &selection_source, &[], dir.path()).unwrap();
        assert_eq!(prepared.crop, (100.0, 100.0, 200.0, 100.0));
        assert_eq!(prepared.export.as_ref().unwrap().width, 200);
        let none = prepare(
            &document,
            &Source {
                from: SourceKind::None,
                ..source
            },
            &[],
            dir.path(),
        )
        .unwrap();
        assert!(none.export.is_none());
        assert!(none.describe().is_null());
        assert!(place_layer(&none, "x", RgbaImage::new(1, 1), 0.0, 0.0, None).is_err());
    }

    #[test]
    fn box_blur_softens_edges_without_changing_flat_areas() {
        let mut mask = GrayImage::new(20, 20);
        for y in 5..15 {
            for x in 5..15 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        box_blur(&mut mask, 2);
        assert_eq!(mask.get_pixel(10, 10)[0], 255);
        assert_eq!(mask.get_pixel(0, 0)[0], 0);
        let edge = mask.get_pixel(5, 10)[0];
        assert!(edge > 0 && edge < 255, "{edge}");
    }
}

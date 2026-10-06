//! Preparing what an action receives and fitting what it returns back into
//! the document. Plugins only see the exported source image; every scale,
//! crop and layer transform is undone here when the result comes back.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{
    edits::{Export, fit, sub_transform, write_gray_png, write_png},
    manifest::{MAX_MASK_RADIUS, Margins, MaskEmpty, Source, SourceKind, SourceMask},
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

/// How large an `image` output is placed, when not at its own pixel size
/// divided by the export scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct Placed {
    /// Width in document units.
    #[serde(default)]
    pub width: Option<f32>,
    /// Height in document units.
    #[serde(default)]
    pub height: Option<f32>,
    /// `source`: cover the bounds of the source that was sent.
    #[serde(default)]
    pub fit: Option<Fit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    Source,
}

impl Placed {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [("width", self.width), ("height", self.height)] {
            if let Some(value) = value {
                ensure!(
                    value.is_finite() && value > 0.0 && value <= crate::document::MAX_SIDE as f32,
                    "The placed {name} must be between 0 and {} document units",
                    crate::document::MAX_SIDE
                );
            }
        }
        ensure!(
            self.fit.is_none() || (self.width.is_none() && self.height.is_none()),
            "An image output gives either fit or a width and height, not both"
        );
        Ok(())
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
    /// The selection mask sent with the source, same size as the export.
    pub mask: Option<PathBuf>,
    /// New canvas the source was padded with (`source.extend`), in document
    /// pixels, and the mask of that new area, same size as the export.
    pub extend: Option<Margins>,
    pub extend_mask: Option<PathBuf>,
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
            mask: None,
            extend: None,
            extend_mask: None,
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
                "mask": self.mask,
                "extend": self.extend,
                "extend_mask": self.extend_mask,
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

    /// The size in source pixels an image of `pixels` size covers once placed.
    pub fn placed_size(&self, pixels: (u32, u32), placed: &Placed) -> Result<(f32, f32)> {
        placed.validate()?;
        let (w, h) = (pixels.0 as f32, pixels.1 as f32);
        if placed.fit.is_some() {
            return Ok((self.crop.2, self.crop.3));
        }
        if placed.width.is_none() && placed.height.is_none() {
            return Ok((w / self.scale, h / self.scale));
        }
        // Document units per source pixel; a missing side keeps the aspect.
        let (sw, sh) = (self.source_size.0 as f32, self.source_size.1 as f32);
        ensure!(sw > 0.0 && sh > 0.0, "The action had no source image");
        let (ux, uy) = (self.transform.width / sw, self.transform.height / sh);
        let width = placed.width.or(placed.height.map(|v| v * w / h));
        let height = placed.height.or(placed.width.map(|v| v * h / w));
        Ok((width.unwrap_or(w) / ux, height.unwrap_or(h) / uy))
    }

    /// The document transform of an output covering export pixels
    /// `x, y, width, height`.
    pub fn placement(&self, x: f32, y: f32, width: f32, height: f32) -> Transform {
        self.placement_sized(x, y, (width / self.scale, height / self.scale))
    }

    /// As `placement`, for a size already in source pixels.
    pub fn placement_sized(&self, x: f32, y: f32, size: (f32, f32)) -> Transform {
        sub_transform(
            self.transform,
            self.source_size,
            (
                self.crop.0 + x / self.scale,
                self.crop.1 + y / self.scale,
                size.0,
                size.1,
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
    let extend = (source.extend.as_ref())
        .map(|extend| extend.margins())
        .filter(|margins| !margins.is_empty() && source.from == SourceKind::Composite);
    let (pixels, transform) = match extend {
        Some(margins) => extended(&pixels, margins)?,
        None => (pixels, transform),
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
        mask: None,
        extend,
        extend_mask: None,
        hash: Some(super::edits::pixel_hash(&pixels)),
    };
    let selection_mask = source_mask(document, source)?;
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
    // A grown or feathered mask also widens a selection source's crop.
    let crop_mask = match &selection_mask {
        Some(SelectionMask::Shape(mask)) => Some(mask.as_ref()),
        _ => document.selection.as_deref(),
    };
    if source.from == SourceKind::Selection
        && let Some((x0, y0, x1, y1)) = crop_mask.and_then(crate::selection::bounds)
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
        mask_layer: None,
    });
    for (index, (region, bounds)) in regions.iter().zip(&boxes).enumerate() {
        let x = (bounds.0 - crop.0) * scale;
        let y = (bounds.1 - crop.1) * scale;
        let width = bounds.2 * scale;
        let height = bounds.3 * scale;
        let mut mask_path = None;
        if let Some(mask) = &region.mask {
            let (ew, eh) = (exported.width(), exported.height());
            let projected = project_mask(&prepared, mask, ew, eh);
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
    match selection_mask {
        Some(SelectionMask::Shape(mask)) => {
            let projected = project_mask(&prepared, &mask, exported.width(), exported.height());
            let path = dir.join("selection.png");
            write_gray_png(&projected, &path)?;
            prepared.mask = Some(path);
        }
        Some(SelectionMask::Constant(value)) => {
            let projected =
                GrayImage::from_pixel(exported.width(), exported.height(), image::Luma([value]));
            let path = dir.join("selection.png");
            write_gray_png(&projected, &path)?;
            prepared.mask = Some(path);
        }
        None => {}
    }
    if let Some(margins) = extend {
        let size = (
            pixels.width() - margins.left - margins.right,
            pixels.height() - margins.top - margins.bottom,
        );
        let new_area = new_area_mask(&prepared, margins, size, exported.dimensions());
        let path = dir.join("extend.png");
        write_gray_png(&new_area, &path)?;
        prepared.extend_mask = Some(path);
    }
    Ok(prepared)
}

/// The composite padded with transparent pixels, and the document placement
/// of the padded raster: its top-left is at `(-left, -top)`.
fn extended(composite: &RgbaImage, margins: Margins) -> Result<(Arc<RgbaImage>, Transform)> {
    let grow = |size: u32, a: u32, b: u32| {
        size.checked_add(a)
            .and_then(|size| size.checked_add(b))
            .unwrap_or(u32::MAX)
    };
    let width = grow(composite.width(), margins.left, margins.right);
    let height = grow(composite.height(), margins.top, margins.bottom);
    crate::document::validate_size(width, height)
        .context("The extended canvas would be too large")?;
    let mut padded = RgbaImage::new(width, height);
    image::imageops::replace(
        &mut padded,
        composite,
        i64::from(margins.left),
        i64::from(margins.top),
    );
    let transform = Transform {
        x: -(margins.left as f32),
        y: -(margins.top as f32),
        ..Transform::new(width, height)
    };
    Ok((Arc::new(padded), transform))
}

/// White over every export pixel that touches the new canvas, black over
/// those wholly on the old one. `size` is the old canvas, in pixels of the
/// padded source.
fn new_area_mask(
    prepared: &Prepared,
    margins: Margins,
    size: (u32, u32),
    export: (u32, u32),
) -> GrayImage {
    let (x0, y0) = (margins.left as f32, margins.top as f32);
    let (x1, y1) = (x0 + size.0 as f32, y0 + size.1 as f32);
    let (crop_x, crop_y, crop_w, crop_h) = prepared.crop;
    // Per axis, as the export was resampled.
    let sx = crop_w / export.0.max(1) as f32;
    let sy = crop_h / export.1.max(1) as f32;
    const EPSILON: f32 = 1e-3;
    GrayImage::from_fn(export.0, export.1, |px, py| {
        let left = crop_x + px as f32 * sx;
        let top = crop_y + py as f32 * sy;
        let old = left + EPSILON >= x0
            && left + sx <= x1 + EPSILON
            && top + EPSILON >= y0
            && top + sy <= y1 + EPSILON;
        image::Luma([if old { 0 } else { 255 }])
    })
}

/// The selection mask an action asked for, before it is cut to the export.
enum SelectionMask {
    /// The selection after `mask_grow` and `mask_feather`, at document size.
    Shape(Arc<GrayImage>),
    /// No selection: everything (255) or nothing (0).
    Constant(u8),
}

fn source_mask(document: &Document, source: &Source) -> Result<Option<SelectionMask>> {
    if source.mask != SourceMask::Selection {
        return Ok(None);
    }
    let Some(selection) = &document.selection else {
        return match source.mask_empty {
            MaskEmpty::Error => anyhow::bail!("Select an area first"),
            MaskEmpty::White => Ok(Some(SelectionMask::Constant(255))),
            MaskEmpty::Black => Ok(Some(SelectionMask::Constant(0))),
        };
    };
    let grow = source
        .mask_grow
        .clamp(-(MAX_MASK_RADIUS as i32), MAX_MASK_RADIUS as i32);
    let feather = source
        .mask_feather
        .clamp(0.0, MAX_MASK_RADIUS as f32)
        .round() as u32;
    if grow == 0 && feather == 0 {
        return Ok(Some(SelectionMask::Shape(selection.clone())));
    }
    let mut mask = (**selection).clone();
    grow_mask(&mut mask, grow);
    box_blur(&mut mask, feather);
    Ok(Some(SelectionMask::Shape(Arc::new(mask))))
}

/// `mask` (at document size) sampled at each pixel of an export.
fn project_mask(prepared: &Prepared, mask: &GrayImage, width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |px, py| {
        let point = prepared.to_document(px as f32 + 0.5, py as f32 + 0.5);
        let coverage = crate::selection::coverage(Some(mask), point);
        image::Luma([(coverage * 255.0).round() as u8])
    })
}

/// Grow (`radius > 0`) or shrink (`radius < 0`) a mask by that many pixels,
/// with a square reach.
pub fn grow_mask(mask: &mut GrayImage, radius: i32) {
    if radius == 0 {
        return;
    }
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let reach = radius.unsigned_abs() as usize;
    let widen = radius > 0;
    let mut line = Vec::new();
    let mut out = Vec::new();
    for row in mask.chunks_exact_mut(w) {
        window_extreme(row, reach, widen, &mut out);
        row.copy_from_slice(&out);
    }
    for x in 0..w {
        line.clear();
        line.extend((0..h).map(|y| mask.as_raw()[y * w + x]));
        window_extreme(&line, reach, widen, &mut out);
        for (y, value) in out.iter().enumerate() {
            mask.as_mut()[y * w + x] = *value;
        }
    }
}

/// The maximum (or minimum) of each `2 * reach + 1` window of `line`.
fn window_extreme(line: &[u8], reach: usize, maximum: bool, out: &mut Vec<u8>) {
    let n = line.len();
    out.clear();
    let better = |a: u8, b: u8| if maximum { a >= b } else { a <= b };
    let mut queue = std::collections::VecDeque::<usize>::new();
    let mut next = 0;
    for i in 0..n {
        let end = (i + reach + 1).min(n);
        while next < end {
            while queue.back().is_some_and(|&b| better(line[next], line[b])) {
                queue.pop_back();
            }
            queue.push_back(next);
            next += 1;
        }
        while queue.front().is_some_and(|&f| f + reach < i) {
            queue.pop_front();
        }
        out.push(line[*queue.front().unwrap()]);
    }
}

/// A new layer for an image the plugin returned at export position `x, y`.
pub fn place_layer(
    prepared: &Prepared,
    name: &str,
    image: RgbaImage,
    x: f32,
    y: f32,
    placed: &Placed,
    regions: Option<&[Region]>,
) -> Result<Layer> {
    ensure!(prepared.export.is_some(), "The action had no source image");
    let size = prepared.placed_size(image.dimensions(), placed)?;
    let (width, height) = (image.width() as f32, image.height() as f32);
    let mut layer = Layer::image(name, image);
    layer.transform = prepared.placement_sized(x, y, size);
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

/// The document transform of an output of `pixels` size for an action
/// without a source: `x, y` and the placed size are in document units, and
/// the size defaults to the pixel size.
pub fn unsourced_placement(
    pixels: (u32, u32),
    x: f32,
    y: f32,
    placed: &Placed,
) -> Result<Transform> {
    placed.validate()?;
    ensure!(
        placed.fit.is_none(),
        "fit = \"source\" needs an action with a source"
    );
    let (w, h) = (pixels.0 as f32, pixels.1 as f32);
    Ok(Transform {
        x,
        y,
        width: placed
            .width
            .unwrap_or_else(|| placed.height.map_or(w, |height| height * w / h)),
        height: placed
            .height
            .unwrap_or_else(|| placed.width.map_or(h, |width| width * h / w)),
        ..Transform::new(pixels.0, pixels.1)
    })
}

/// A `mask` output turned into selection coverage at document size: placed
/// like an `image` output at export position `x, y`, sampled bilinearly, and
/// empty wherever the mask does not reach. Grey values are partial coverage.
pub fn place_mask(
    prepared: &Prepared,
    document_size: (u32, u32),
    mask: &GrayImage,
    x: f32,
    y: f32,
    placed: &Placed,
) -> Result<GrayImage> {
    let transform = if prepared.export.is_some() {
        let size = prepared.placed_size(mask.dimensions(), placed)?;
        prepared.placement_sized(x, y, size)
    } else {
        unsourced_placement(mask.dimensions(), x, y, placed)?
    };
    ensure!(transform.valid(), "The mask does not fit the document");
    let (width, height) = document_size;
    let mut coverage = GrayImage::new(width, height);
    // Only the pixels under the placed mask's corners need sampling.
    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        .map(|(u, v)| transform.point(Point::new(u, v)));
    let min_x = corners.iter().map(|c| c.x).fold(f32::INFINITY, f32::min);
    let min_y = corners.iter().map(|c| c.y).fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|c| c.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = corners
        .iter()
        .map(|c| c.y)
        .fold(f32::NEG_INFINITY, f32::max);
    let x0 = min_x.floor().clamp(0.0, width as f32) as u32;
    let y0 = min_y.floor().clamp(0.0, height as f32) as u32;
    let x1 = max_x.ceil().clamp(0.0, width as f32) as u32;
    let y1 = max_y.ceil().clamp(0.0, height as f32) as u32;
    let (mw, mh) = mask.dimensions();
    let sample = |px: i64, py: i64| -> f32 {
        let px = px.clamp(0, i64::from(mw) - 1) as u32;
        let py = py.clamp(0, i64::from(mh) - 1) as u32;
        f32::from(mask.get_pixel(px, py)[0])
    };
    for y in y0..y1 {
        for x in x0..x1 {
            let unit = transform.inverse(Point::new(x as f32 + 0.5, y as f32 + 0.5));
            if !(0.0..1.0).contains(&unit.x) || !(0.0..1.0).contains(&unit.y) {
                continue;
            }
            let fx = unit.x * mw as f32 - 0.5;
            let fy = unit.y * mh as f32 - 0.5;
            let (ix, iy) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - ix, fy - iy);
            let (ix, iy) = (ix as i64, iy as i64);
            let top = sample(ix, iy) * (1.0 - tx) + sample(ix + 1, iy) * tx;
            let bottom = sample(ix, iy + 1) * (1.0 - tx) + sample(ix + 1, iy + 1) * tx;
            let value = top * (1.0 - ty) + bottom * ty;
            coverage.put_pixel(x, y, image::Luma([value.round().clamp(0.0, 255.0) as u8]));
        }
    }
    Ok(coverage)
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
///
/// The result is scaled to the size it is placed at, in the layer's pixels. That size is
/// checked before anything is allocated, so a large result for a source that
/// was sent much smaller fails instead of exhausting memory.
pub fn replace_pixels(
    prepared: &Prepared,
    source: &RgbaImage,
    image: &RgbaImage,
    x: f32,
    y: f32,
    placed: &Placed,
) -> Result<RgbaImage> {
    let scale = f64::from(prepared.scale);
    ensure!(
        scale.is_finite() && scale > 0.0,
        "The source was prepared with an invalid scale"
    );
    let size = prepared.placed_size(image.dimensions(), placed)?;
    ensure!(
        size.0.is_finite() && size.1.is_finite(),
        "The result does not fit the layer"
    );
    let (width, height) = (
        f64::from(size.0).round().max(1.0),
        f64::from(size.1).round().max(1.0),
    );
    ensure!(
        width <= f64::from(u32::MAX) && height <= f64::from(u32::MAX),
        "The result is too large to scale back to the layer"
    );
    let (width, height) = (width as u32, height as u32);
    crate::document::validate_size(width, height)
        .context("The result is too large to scale back to the layer")?;
    let mut pixels = source.clone();
    let resized = if (width, height) == image.dimensions() {
        image.clone()
    } else {
        crate::render::resize_quality(image, width, height)
    };
    let ox = (prepared.crop.0 + x / prepared.scale).round() as i64;
    let oy = (prepared.crop.1 + y / prepared.scale).round() as i64;
    image::imageops::replace(&mut pixels, &resized, ox, oy);
    Ok(pixels)
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
            ..Source::default()
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
        let layer = place_layer(
            &prepared,
            "Edit",
            result.clone(),
            0.0,
            0.0,
            &Placed::default(),
            Some(&regions),
        )
        .unwrap();
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
            &Placed::default(),
        )
        .unwrap();
        assert_eq!(replaced.get_pixel(100, 75)[1], 255);
        assert_eq!(replaced.get_pixel(0, 0)[2], 255);
        assert_eq!(replaced.dimensions(), (200, 150));
    }

    #[test]
    fn replace_results_that_would_scale_past_the_size_limit_fail_first() {
        // A 30k-pixel layer sent at a small max_side: every result pixel
        // stands for many layer pixels.
        let prepared = Prepared {
            scale: 256.0 / 30_000.0,
            ..Prepared::none()
        };
        let source = RgbaImage::new(4, 4);
        // 10k x 10k at that scale would be about 1.2M x 1.2M pixels.
        let huge = RgbaImage::new(10_000, 1);
        let error =
            replace_pixels(&prepared, &source, &huge, 0.0, 0.0, &Placed::default()).unwrap_err();
        assert!(format!("{error:#}").contains("too large"), "{error:#}");
        // Within the limit it still works.
        let small = RgbaImage::new(2, 2);
        let pixels =
            replace_pixels(&prepared, &source, &small, 0.0, 0.0, &Placed::default()).unwrap();
        assert_eq!(pixels.dimensions(), (4, 4));
        for scale in [0.0, f32::NAN, f32::INFINITY, -1.0] {
            let prepared = Prepared {
                scale,
                ..Prepared::none()
            };
            assert!(
                replace_pixels(&prepared, &source, &small, 0.0, 0.0, &Placed::default()).is_err()
            );
        }
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
            ..Source::default()
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
            &Placed::default(),
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
            ..source.clone()
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
        assert!(
            place_layer(
                &none,
                "x",
                RgbaImage::new(1, 1),
                0.0,
                0.0,
                &Placed::default(),
                None
            )
            .is_err()
        );
    }

    fn layer_source(dir: &Path) -> (Document, Prepared) {
        let document = document();
        let source = Source {
            from: SourceKind::Layer,
            max_side: None,
            crop_to_regions: false,
            padding: 0.0,
            ..Source::default()
        };
        let prepared = prepare(&document, &source, &[], dir).unwrap();
        (document, prepared)
    }

    #[test]
    fn a_double_size_result_can_cover_the_source_at_twice_the_density() {
        let dir = tempfile::tempdir().unwrap();
        let (document, prepared) = layer_source(dir.path());
        let upscaled = RgbaImage::from_pixel(400, 300, image::Rgba([0, 255, 0, 255]));
        let place = |placed: &Placed| {
            place_layer(&prepared, "Up", upscaled.clone(), 0.0, 0.0, placed, None).unwrap()
        };
        // By default it lands at its pixel size, twice as large as the source here.
        assert_eq!(place(&Placed::default()).transform.width, 800.0);
        let fit = Placed {
            fit: Some(Fit::Source),
            ..Placed::default()
        };
        let layer = place(&fit);
        let original = document.layers[0].transform;
        assert_eq!(
            (layer.transform.x, layer.transform.y),
            (original.x, original.y)
        );
        assert_eq!(
            (layer.transform.width, layer.transform.height),
            (original.width, original.height)
        );
        assert_eq!(layer.pixels.as_ref().unwrap().dimensions(), (400, 300));
        // Explicit sizes, and one side alone keeps the aspect ratio.
        let sized = Placed {
            width: Some(100.0),
            height: Some(50.0),
            fit: None,
        };
        let layer = place(&sized);
        assert_eq!(
            (layer.transform.width, layer.transform.height),
            (100.0, 50.0)
        );
        let wide = Placed {
            width: Some(100.0),
            ..Placed::default()
        };
        let layer = place(&wide);
        assert_eq!(
            (layer.transform.width, layer.transform.height),
            (100.0, 75.0)
        );
        // Replacing resamples to the source's own pixel grid.
        let pixels = document.layers[0].pixels.as_ref().unwrap();
        let replaced = replace_pixels(&prepared, pixels, &upscaled, 0.0, 0.0, &fit).unwrap();
        assert_eq!(replaced.dimensions(), (200, 150));
        assert_eq!(replaced.get_pixel(100, 75)[1], 255);
        assert_eq!(replaced.get_pixel(100, 75)[0], 0);
        // Placed at half the layer's size in document units, a quarter of its pixels.
        let half = Placed {
            width: Some(200.0),
            height: Some(150.0),
            fit: None,
        };
        let replaced = replace_pixels(&prepared, pixels, &upscaled, 0.0, 0.0, &half).unwrap();
        assert_eq!(replaced.get_pixel(99, 74)[1], 255);
        assert_eq!(replaced.get_pixel(100, 75)[1], 0);
    }

    #[test]
    fn invalid_placed_sizes_are_rejected_and_budgets_still_apply() {
        let dir = tempfile::tempdir().unwrap();
        let (document, prepared) = layer_source(dir.path());
        let image = RgbaImage::new(4, 4);
        let place = |placed: &Placed| {
            place_layer(&prepared, "x", image.clone(), 0.0, 0.0, placed, None).is_err()
        };
        for bad in [0.0, -5.0, f32::NAN, f32::INFINITY, 30_001.0] {
            assert!(place(&Placed {
                width: Some(bad),
                ..Placed::default()
            }));
            assert!(place(&Placed {
                height: Some(bad),
                ..Placed::default()
            }));
        }
        assert!(place(&Placed {
            width: Some(10.0),
            height: None,
            fit: Some(Fit::Source),
        }));
        // Replacing is still limited to the document's size and pixel budgets.
        let huge = Placed {
            width: Some(30_000.0),
            height: Some(30_000.0),
            fit: None,
        };
        let pixels = document.layers[0].pixels.as_ref().unwrap();
        let error = replace_pixels(&prepared, pixels, &image, 0.0, 0.0, &huge).unwrap_err();
        assert!(format!("{error:#}").contains("too large"), "{error:#}");
    }

    #[test]
    fn masks_are_placed_like_images_and_sampled_to_document_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let (_document, prepared) = layer_source(dir.path());
        // Left half selected, right half not.
        let mask = GrayImage::from_fn(4, 2, |x, _| image::Luma([if x < 2 { 255 } else { 0 }]));
        let fit = Placed {
            fit: Some(Fit::Source),
            ..Placed::default()
        };
        // Fitted over the layer, which sits at (20, 10) at 400 x 300 units.
        let coverage = place_mask(&prepared, (400, 300), &mask, 0.0, 0.0, &fit).unwrap();
        assert_eq!(coverage.dimensions(), (400, 300));
        assert_eq!(coverage.get_pixel(100, 100)[0], 255);
        assert_eq!(coverage.get_pixel(390, 100)[0], 0);
        assert_eq!(coverage.get_pixel(10, 5)[0], 0, "outside the mask");
        // Enlarged 100x, the edge between the halves is soft.
        let edge = coverage.get_pixel(219, 100)[0];
        assert!(edge > 0 && edge < 255, "{edge}");
        // At its pixel size: four source pixels, eight document units.
        let coverage =
            place_mask(&prepared, (400, 300), &mask, 0.0, 0.0, &Placed::default()).unwrap();
        assert_eq!(coverage.get_pixel(21, 11)[0], 255);
        assert_eq!(coverage.get_pixel(30, 11)[0], 0);
        // Without a source it is in document units, pixel for pixel.
        let none = Prepared::none();
        let coverage = place_mask(&none, (16, 16), &mask, 5.0, 6.0, &Placed::default()).unwrap();
        let row: Vec<u8> = (4..10).map(|x| coverage.get_pixel(x, 6)[0]).collect();
        assert_eq!(row, [0, 255, 255, 0, 0, 0]);
        assert_eq!(coverage.get_pixel(5, 8)[0], 0);
        assert!(place_mask(&none, (16, 16), &mask, 0.0, 0.0, &fit).is_err());
        // Placed far outside the document it selects nothing.
        let coverage = place_mask(&none, (16, 16), &mask, 500.0, 0.0, &Placed::default()).unwrap();
        assert!(coverage.as_raw().iter().all(|&v| v == 0));
        for bad in [f32::NAN, 0.0, 1e9] {
            let placed = Placed {
                width: Some(bad),
                ..Placed::default()
            };
            assert!(place_mask(&prepared, (400, 300), &mask, 0.0, 0.0, &placed).is_err());
        }
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

    /// The test document with 100..300 x 100..200 selected.
    fn selected() -> Document {
        let mut document = document();
        document.selection = Some(Arc::new(GrayImage::from_fn(400, 300, |x, y| {
            image::Luma([if (100..300).contains(&x) && (100..200).contains(&y) {
                255
            } else {
                0
            }])
        })));
        document
    }

    fn with_mask(from: SourceKind, max_side: Option<u32>) -> Source {
        Source {
            from,
            max_side,
            mask: SourceMask::Selection,
            ..Source::default()
        }
    }

    fn sent_mask(prepared: &Prepared) -> GrayImage {
        super::super::edits::read_gray_png(prepared.mask.as_ref().unwrap()).unwrap()
    }

    #[test]
    fn the_selection_mask_matches_the_source_export_for_every_source() {
        let dir = tempfile::tempdir().unwrap();
        let document = selected();
        // Layer: the layer's 200x150 pixels sit at (20, 10), twice as big.
        for (max_side, scale) in [(None, 1.0), (Some(100), 0.5)] {
            let prepared = prepare(
                &document,
                &with_mask(SourceKind::Layer, max_side),
                &[],
                dir.path(),
            )
            .unwrap();
            let export = prepared.export.as_ref().unwrap();
            let mask = sent_mask(&prepared);
            assert_eq!(mask.dimensions(), (export.width, export.height));
            assert_eq!(prepared.scale, scale);
            // Layer pixel (100, 75) is document (220, 160): selected. Layer
            // pixel (10, 10) is document (40, 30): not.
            let at = |lx: f32, ly: f32| mask.get_pixel((lx * scale) as u32, (ly * scale) as u32)[0];
            assert_eq!(at(100.0, 75.0), 255);
            assert_eq!(at(10.0, 10.0), 0);
            assert_eq!(at(190.0, 75.0), 0);
            assert_eq!(prepared.describe()["mask"], json!(prepared.mask));
        }
        // Composite: 400x300, selected 100..300 x 100..200.
        for (max_side, scale) in [(None, 1.0), (Some(200), 0.5)] {
            let prepared = prepare(
                &document,
                &with_mask(SourceKind::Composite, max_side),
                &[],
                dir.path(),
            )
            .unwrap();
            let mask = sent_mask(&prepared);
            let export = prepared.export.as_ref().unwrap();
            assert_eq!(mask.dimensions(), (export.width, export.height));
            assert_eq!(mask.width(), (400.0 * scale) as u32);
            let at = |x: f32, y: f32| mask.get_pixel((x * scale) as u32, (y * scale) as u32)[0];
            assert_eq!((at(150.0, 150.0), at(299.0, 199.0)), (255, 255));
            assert_eq!(
                (at(50.0, 150.0), at(150.0, 250.0), at(310.0, 150.0)),
                (0, 0, 0)
            );
        }
        // Selection: cropped to its bounds, so the mask is solid.
        for (max_side, size) in [(None, (200, 100)), (Some(100), (100, 50))] {
            let prepared = prepare(
                &document,
                &with_mask(SourceKind::Selection, max_side),
                &[],
                dir.path(),
            )
            .unwrap();
            assert_eq!(prepared.crop, (100.0, 100.0, 200.0, 100.0));
            let mask = sent_mask(&prepared);
            assert_eq!(mask.dimensions(), size);
            assert!(mask.as_raw().iter().all(|&v| v == 255));
        }
    }

    #[test]
    fn mask_feather_and_grow_change_the_mask_before_export() {
        let dir = tempfile::tempdir().unwrap();
        let document = selected();
        let mask_of = |feather: f32, grow: i32| {
            let source = Source {
                mask_feather: feather,
                mask_grow: grow,
                ..with_mask(SourceKind::Composite, None)
            };
            sent_mask(&prepare(&document, &source, &[], dir.path()).unwrap())
        };
        let plain = mask_of(0.0, 0);
        assert_eq!(
            (plain.get_pixel(95, 150)[0], plain.get_pixel(105, 150)[0]),
            (0, 255)
        );
        let grown = mask_of(0.0, 10);
        assert_eq!(
            (grown.get_pixel(95, 150)[0], grown.get_pixel(89, 150)[0]),
            (255, 0)
        );
        assert_eq!(grown.get_pixel(150, 205)[0], 255);
        let shrunk = mask_of(0.0, -10);
        assert_eq!(
            (shrunk.get_pixel(105, 150)[0], shrunk.get_pixel(115, 150)[0]),
            (0, 255)
        );
        let soft = mask_of(8.0, 0);
        let edge = soft.get_pixel(100, 150)[0];
        assert!(edge > 0 && edge < 255, "{edge}");
        assert_eq!(soft.get_pixel(200, 150)[0], 255);
        assert_eq!(soft.get_pixel(20, 20)[0], 0);
        // Grow happens first and widens a selection source's crop with it.
        let source = Source {
            mask_grow: 10,
            ..with_mask(SourceKind::Selection, None)
        };
        let prepared = prepare(&document, &source, &[], dir.path()).unwrap();
        assert_eq!(prepared.crop, (90.0, 90.0, 220.0, 120.0));
        assert!(sent_mask(&prepared).as_raw().iter().all(|&v| v == 255));
        // Out of range radii are clamped, not trusted.
        let source = Source {
            mask_grow: i32::MAX,
            mask_feather: f32::NAN,
            ..with_mask(SourceKind::Composite, None)
        };
        let prepared = prepare(&document, &source, &[], dir.path()).unwrap();
        assert!(sent_mask(&prepared).as_raw().iter().all(|&v| v == 255));
    }

    #[test]
    fn without_a_selection_the_mask_errors_or_is_a_constant() {
        let dir = tempfile::tempdir().unwrap();
        let document = document();
        let source = with_mask(SourceKind::Composite, Some(200));
        let error = prepare(&document, &source, &[], dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("Select an area"));
        for (empty, value) in [(MaskEmpty::White, 255), (MaskEmpty::Black, 0)] {
            let source = Source {
                mask_empty: empty,
                ..source.clone()
            };
            let prepared = prepare(&document, &source, &[], dir.path()).unwrap();
            let mask = sent_mask(&prepared);
            assert_eq!(mask.dimensions(), (200, 150));
            assert!(mask.as_raw().iter().all(|&v| v == value));
        }
        // An action that does not ask for a mask never gets one.
        let plain = Source::default();
        assert!(
            prepare(&document, &plain, &[], dir.path())
                .unwrap()
                .mask
                .is_none()
        );
    }

    fn extending(max_side: Option<u32>, mask: SourceMask) -> Source {
        use super::super::manifest::{Amount, Extend};
        Source {
            from: SourceKind::Composite,
            max_side,
            mask,
            extend: Some(Extend {
                left: Amount::Pixels(10),
                top: Amount::Pixels(20),
                right: Amount::Pixels(30),
                bottom: Amount::Pixels(0),
            }),
            ..Source::default()
        }
    }

    #[test]
    fn an_extended_source_is_padded_and_sends_a_mask_of_the_new_area() {
        let dir = tempfile::tempdir().unwrap();
        // 400x300, with 100..300 x 100..200 selected.
        let document = selected();
        let prepared = prepare(
            &document,
            &extending(None, SourceMask::Selection),
            &[],
            dir.path(),
        )
        .unwrap();
        let export = prepared.export.as_ref().unwrap();
        assert_eq!((export.width, export.height), (440, 320));
        assert_eq!((export.x, export.y, prepared.scale), (-10.0, -20.0, 1.0));
        assert_eq!(
            prepared.extend,
            Some(Margins {
                left: 10,
                top: 20,
                right: 30,
                bottom: 0
            })
        );
        let sent = super::super::edits::read_png(&export.path).unwrap();
        // New canvas is transparent; the layer (blue from document (20, 10))
        // sits 10 right and 20 down.
        assert_eq!(sent.get_pixel(5, 5)[3], 0);
        assert_eq!(sent.get_pixel(435, 100)[3], 0);
        assert_eq!(*sent.get_pixel(60, 60), image::Rgba([0, 0, 255, 255]));
        // The layer's red pixel is document (220, 160).
        assert!(sent.get_pixel(231, 181)[0] > 100);
        let new_area =
            super::super::edits::read_gray_png(prepared.extend_mask.as_ref().unwrap()).unwrap();
        assert_eq!(new_area.dimensions(), (440, 320));
        let at = |x, y| new_area.get_pixel(x, y)[0];
        assert_eq!(
            (at(9, 100), at(10, 100), at(409, 100), at(410, 100)),
            (255, 0, 0, 255)
        );
        assert_eq!((at(100, 19), at(100, 20), at(100, 319)), (255, 0, 0));
        // The selection mask uses the same grid: document (110, 110) is
        // selected, and nothing in the new area is.
        let selection = sent_mask(&prepared);
        assert_eq!(selection.dimensions(), (440, 320));
        assert_eq!(selection.get_pixel(120, 130)[0], 255);
        assert_eq!(selection.get_pixel(110, 120)[0], 255);
        assert_eq!(selection.get_pixel(109, 130)[0], 0);
        assert_eq!(selection.get_pixel(5, 5)[0], 0);
        let described = prepared.describe();
        assert_eq!(
            described["extend"],
            json!({"left": 10, "top": 20, "right": 30, "bottom": 0})
        );
        assert_eq!(described["extend_mask"], json!(prepared.extend_mask));
        assert_eq!(
            (&described["document_x"], &described["document_y"]),
            (&json!(-10.0), &json!(-20.0))
        );

        // fit = "source" covers the extended bounds: once the canvas grows by
        // the same margins, that is the whole new canvas.
        let fit = Placed {
            fit: Some(Fit::Source),
            ..Placed::default()
        };
        let image = RgbaImage::new(880, 640);
        let layer = place_layer(&prepared, "Out", image, 0.0, 0.0, &fit, None).unwrap();
        let t = layer.transform;
        assert_eq!((t.x, t.y, t.width, t.height), (-10.0, -20.0, 440.0, 320.0));
    }

    #[test]
    fn an_extended_source_scales_both_masks_with_max_side() {
        let dir = tempfile::tempdir().unwrap();
        let document = selected();
        let prepared = prepare(
            &document,
            &extending(Some(220), SourceMask::Selection),
            &[],
            dir.path(),
        )
        .unwrap();
        let export = prepared.export.as_ref().unwrap();
        assert_eq!(
            (export.width, export.height, prepared.scale),
            (220, 160, 0.5)
        );
        let new_area =
            super::super::edits::read_gray_png(prepared.extend_mask.as_ref().unwrap()).unwrap();
        let selection = sent_mask(&prepared);
        assert_eq!(new_area.dimensions(), (220, 160));
        assert_eq!(selection.dimensions(), (220, 160));
        let at = |x, y| new_area.get_pixel(x, y)[0];
        // Export pixel 5 covers padded pixels 10..12, the first old column.
        assert_eq!(
            (at(4, 50), at(5, 50), at(204, 50), at(205, 50)),
            (255, 0, 0, 255)
        );
        assert_eq!((at(50, 9), at(50, 10), at(50, 159)), (255, 0, 0));
        // Document (110, 110) is padded (120, 130), export (60, 65).
        assert_eq!(selection.get_pixel(60, 65)[0], 255);
        assert_eq!(selection.get_pixel(2, 2)[0], 0);
        // Without a selection mask the new-area mask still comes.
        let plain = prepare(
            &document,
            &extending(Some(220), SourceMask::None),
            &[],
            dir.path(),
        )
        .unwrap();
        assert!(plain.mask.is_none());
        assert!(plain.extend_mask.is_some());
        // Only composite sources extend; an unextended source sends no mask.
        let layer = Source {
            from: SourceKind::Layer,
            ..extending(None, SourceMask::None)
        };
        let prepared = prepare(&document, &layer, &[], dir.path()).unwrap();
        assert!(prepared.extend.is_none() && prepared.extend_mask.is_none());
        assert!(prepared.describe()["extend"].is_null());
    }

    #[test]
    fn an_extension_past_the_size_limits_is_refused_before_export() {
        use super::super::manifest::{Amount, Extend};
        let dir = tempfile::tempdir().unwrap();
        let document = document();
        for extend in [
            Extend {
                left: Amount::Pixels(29_700),
                ..Extend::default()
            },
            Extend {
                left: Amount::Pixels(19_600),
                top: Amount::Pixels(9_700),
                ..Extend::default()
            },
        ] {
            let source = Source {
                from: SourceKind::Composite,
                extend: Some(extend),
                ..Source::default()
            };
            let error = prepare(&document, &source, &[], dir.path()).unwrap_err();
            assert!(format!("{error:#}").contains("too large"), "{error:#}");
        }
        assert!(!dir.path().join("source.png").exists());
    }

    #[test]
    fn grow_mask_dilates_and_erodes_a_square_reach() {
        let mut mask = GrayImage::new(21, 21);
        mask.put_pixel(10, 10, image::Luma([255]));
        grow_mask(&mut mask, 3);
        let lit = mask.as_raw().iter().filter(|&&v| v == 255).count();
        assert_eq!(lit, 49);
        assert_eq!(mask.get_pixel(13, 7)[0], 255);
        assert_eq!(mask.get_pixel(14, 10)[0], 0);
        grow_mask(&mut mask, -3);
        assert_eq!(mask.as_raw().iter().filter(|&&v| v == 255).count(), 1);
        assert_eq!(mask.get_pixel(10, 10)[0], 255);
        // A shape touching the border does not erode from the border.
        let mut mask = GrayImage::from_pixel(8, 8, image::Luma([255]));
        grow_mask(&mut mask, -2);
        assert!(mask.as_raw().iter().all(|&v| v == 255));
    }
}

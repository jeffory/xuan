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

use super::manifest::FilesystemAccess;
use crate::{
    blend::BlendMode,
    document::{Document, Layer, MAX_PIXELS, MAX_SIDE, Mask, Transform},
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
        "provenance": layer.provenance,
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

/// Image outputs (`image`, `document`) one result may hold.
pub const MAX_OUTPUTS: usize = 64;
/// New layers and documents one result, edit batch or import may create.
pub const MAX_LAYERS: usize = 32;
/// Layers one imported file may have.
pub const MAX_IMPORT_LAYERS: usize = 1000;
/// Edits in one `document/edit` request or one result, all batches together.
pub const MAX_EDITS: usize = 1000;
/// Largest image file read for a plugin.
const MAX_FILE: u64 = 512 * 1024 * 1024;

/// Where the host may read and write files on a plugin's behalf: under its
/// own folders (the plugin folder, its data folder, and the scratch and work
/// folders the host made for it), or anywhere when the manifest's
/// `filesystem` permission allows it. Paths are resolved first, so a symlink
/// cannot lead out of those folders.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Access {
    roots: Vec<PathBuf>,
    read_anywhere: bool,
    write_anywhere: bool,
}

impl Access {
    /// No confinement, for files the host chose itself.
    pub fn anywhere() -> Self {
        Self {
            roots: Vec::new(),
            read_anywhere: true,
            write_anywhere: true,
        }
    }

    pub fn new(roots: impl IntoIterator<Item = PathBuf>, filesystem: FilesystemAccess) -> Self {
        let mut access = Self {
            roots: Vec::new(),
            read_anywhere: filesystem != FilesystemAccess::None,
            write_anywhere: filesystem == FilesystemAccess::Write,
        };
        for root in roots {
            access = access.with(&root);
        }
        access
    }

    /// Also allow `root`. Folders that do not exist are left out.
    pub fn with(mut self, root: &Path) -> Self {
        if let Ok(root) = std::fs::canonicalize(root)
            && !self.roots.contains(&root)
        {
            self.roots.push(root);
        }
        self
    }

    fn inside(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    /// The resolved path of a file or folder the host may read.
    pub fn readable(&self, path: &Path) -> Result<PathBuf> {
        let resolved = std::fs::canonicalize(path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        ensure!(
            self.read_anywhere || self.inside(&resolved),
            "{} is outside the plugin's folders; reading other files needs filesystem = \"read\" in its manifest",
            path.display()
        );
        Ok(resolved)
    }

    /// The resolved path of a folder the host may write into.
    pub fn writable_dir(&self, dir: &Path) -> Result<PathBuf> {
        let resolved = std::fs::canonicalize(dir)
            .with_context(|| format!("Cannot use the folder {}", dir.display()))?;
        ensure!(resolved.is_dir(), "{} is not a folder", dir.display());
        ensure!(
            self.write_anywhere || self.inside(&resolved),
            "{} is outside the plugin's folders; writing elsewhere needs filesystem = \"write\" in its manifest",
            dir.display()
        );
        Ok(resolved)
    }
}

/// Reads the images of one plugin result, edit batch or import, keeping a
/// running total so a plugin cannot make the host decode more than
/// [`MAX_PIXELS`] pixels, or create more layers than allowed, for one answer.
/// Each image is checked against the budget from its header, before it is
/// decoded.
#[derive(Debug)]
pub struct Reader {
    access: Access,
    pixels: u64,
    layers: usize,
    max_layers: usize,
    edits: usize,
}

impl Reader {
    pub fn new(access: Access, max_layers: usize) -> Self {
        Self {
            access,
            pixels: 0,
            layers: 0,
            max_layers,
            edits: 0,
        }
    }

    pub fn rgba(&mut self, path: &Path) -> Result<RgbaImage> {
        Ok(self.decode(path)?.to_rgba8())
    }

    pub fn gray(&mut self, path: &Path) -> Result<GrayImage> {
        Ok(self.decode(path)?.to_luma8())
    }

    /// Count a layer or document about to be created.
    pub fn add_layer(&mut self) -> Result<()> {
        self.layers += 1;
        ensure!(
            self.layers <= self.max_layers,
            "The plugin returned more than {} layers",
            self.max_layers
        );
        Ok(())
    }

    /// Count edits about to be applied.
    pub fn add_edits(&mut self, count: usize) -> Result<()> {
        self.edits = self.edits.saturating_add(count);
        ensure!(
            self.edits <= MAX_EDITS,
            "The plugin returned more than {MAX_EDITS} edits"
        );
        Ok(())
    }

    fn decode(&mut self, path: &Path) -> Result<image::DynamicImage> {
        use image::ImageDecoder;
        use std::io::Read;
        let resolved = self.access.readable(path)?;
        // A FIFO or device would block the UI thread on open or read.
        ensure!(
            std::fs::metadata(&resolved).is_ok_and(|m| m.is_file()),
            "{} is not a regular file",
            path.display()
        );
        let mut bytes = Vec::new();
        std::fs::File::open(&resolved)
            .with_context(|| format!("Cannot read {}", path.display()))?
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        ensure!(bytes.len() as u64 <= MAX_FILE, "Image exceeds 512 MiB");
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .with_context(|| format!("Cannot read {}", path.display()))?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_SIDE);
        limits.max_image_height = Some(MAX_SIDE);
        limits.max_alloc = Some(MAX_PIXELS * 8);
        reader.limits(limits);
        let decoder = reader
            .into_decoder()
            .with_context(|| format!("Cannot decode {}", path.display()))?;
        let (width, height) = decoder.dimensions();
        crate::document::validate_size(width, height)?;
        let pixels = self
            .pixels
            .saturating_add(u64::from(width) * u64::from(height));
        ensure!(
            pixels <= MAX_PIXELS,
            "The plugin's images exceed 100 megapixels in total"
        );
        self.pixels = pixels;
        image::DynamicImage::from_decoder(decoder)
            .with_context(|| format!("Cannot decode {}", path.display()))
    }
}

/// Read one image a plugin wrote.
pub fn read_png(path: &Path) -> Result<RgbaImage> {
    Reader::new(Access::anywhere(), 0).rgba(path)
}

/// Read one mask a plugin wrote.
pub fn read_gray_png(path: &Path) -> Result<GrayImage> {
    Reader::new(Access::anywhere(), 0).gray(path)
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
    /// Grow the canvas by whole document pixels on each side, moving the
    /// layers and guides like Canvas Size does. Needs `document = "edit"`,
    /// also in a result.
    ExtendCanvas {
        #[serde(default)]
        left: u32,
        #[serde(default)]
        top: u32,
        #[serde(default)]
        right: u32,
        #[serde(default)]
        bottom: u32,
    },
}

impl Edit {
    /// Whether the edit changes the document beyond what a read-only plugin
    /// may propose as a result.
    pub fn needs_edit_access(&self) -> bool {
        matches!(self, Self::ExtendCanvas { .. })
    }
}

/// How far a batch moves the document's content: the sum of the `left` and
/// `top` of its `extend_canvas` edits. Outputs placed in the coordinates of
/// the document as it was sent move by this much to stay on their content.
pub fn origin_shift<'a>(edits: impl IntoIterator<Item = &'a Edit>) -> (f32, f32) {
    edits
        .into_iter()
        .fold((0.0, 0.0), |(x, y), edit| match edit {
            Edit::ExtendCanvas { left, top, .. } => (x + *left as f32, y + *top as f32),
            _ => (x, y),
        })
}

/// Apply a batch, reading its images through `reader`. The caller clones the
/// document first and only keeps the result when every edit succeeded, so a
/// failing batch changes nothing.
pub fn apply(document: &mut Document, edits: &[Edit], reader: &mut Reader) -> Result<Vec<Uuid>> {
    reader.add_edits(edits.len())?;
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
                reader.add_layer()?;
                let pixels = reader.rgba(image)?;
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
                        pixels: Arc::new(reader.gray(mask)?),
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
                let pixels = reader.rgba(image)?;
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
                        let pixels = Arc::new(reader.gray(path)?);
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
                        let image = reader.gray(path)?;
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
            Edit::ExtendCanvas {
                left,
                top,
                right,
                bottom,
            } => {
                crate::operations::extend_canvas(document, *left, *top, *right, *bottom)?;
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

    fn run(document: &mut Document, edits: &[Edit]) -> Result<Vec<Uuid>> {
        apply(
            document,
            edits,
            &mut Reader::new(Access::anywhere(), MAX_LAYERS),
        )
    }

    fn png(dir: &Path, name: &str, color: [u8; 4], size: u32) -> PathBuf {
        let path = dir.join(name);
        write_png(
            &RgbaImage::from_pixel(size, size, image::Rgba(color)),
            &path,
        )
        .unwrap();
        path
    }

    /// A PNG that claims `width` × `height` but holds no real pixel data, so
    /// only its header can be read.
    fn huge_png(path: &Path, width: u32, height: u32) {
        fn crc(bytes: &[u8]) -> u32 {
            let mut crc = !0u32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xedb8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        let mut file = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            file.extend((data.len() as u32).to_be_bytes());
            let body = [kind, data].concat();
            file.extend(&body);
            file.extend(crc(&body).to_be_bytes());
        };
        let mut header = Vec::new();
        header.extend(width.to_be_bytes());
        header.extend(height.to_be_bytes());
        header.extend([8, 6, 0, 0, 0]);
        chunk(b"IHDR", &header);
        chunk(b"IDAT", &[0x78, 0x9c, 0x03, 0x00]);
        chunk(b"IEND", &[]);
        std::fs::write(path, file).unwrap();
    }

    #[test]
    fn results_are_held_to_a_pixel_layer_and_edit_budget() {
        let dir = tempfile::tempdir().unwrap();
        // 60 megapixels each: the first fits the budget (and then fails to
        // decode), the second is refused from its header alone.
        let big = dir.path().join("big.png");
        huge_png(&big, 10_000, 6_000);
        let mut reader = Reader::new(Access::anywhere(), MAX_LAYERS);
        let first = format!("{:#}", reader.rgba(&big).unwrap_err());
        assert!(!first.contains("megapixels"), "{first}");
        let second = format!("{:#}", reader.rgba(&big).unwrap_err());
        assert!(second.contains("100 megapixels"), "{second}");
        // A fresh reader starts a new budget; small images still fit.
        let small = png(dir.path(), "small.png", [1, 2, 3, 255], 4);
        assert_eq!(reader.pixels, 60_000_000);
        assert!(reader.rgba(&small).is_ok());
        assert!(Reader::new(Access::anywhere(), 0).gray(&small).is_ok());

        let mut document = Document::new(8, 8).unwrap();
        let add = Edit::AddLayer {
            image: small.clone(),
            name: None,
            x: None,
            y: None,
            width: None,
            height: None,
            mask: None,
            above: None,
            opacity: None,
            blend: None,
        };
        let too_many = vec![add.clone(); MAX_LAYERS + 1];
        let error = run(&mut document.clone(), &too_many).unwrap_err();
        assert!(error.to_string().contains("layers"), "{error}");
        assert_eq!(
            run(&mut document, &too_many[1..]).unwrap().len(),
            MAX_LAYERS
        );
        // Batches of one result share the layer and edit counts.
        let mut reader = Reader::new(Access::anywhere(), 2);
        let mut copy = Document::new(8, 8).unwrap();
        apply(&mut copy, std::slice::from_ref(&add), &mut reader).unwrap();
        apply(&mut copy, std::slice::from_ref(&add), &mut reader).unwrap();
        assert!(apply(&mut copy, &[add], &mut reader).is_err());
        let select = Edit::Select {
            layer: document.layers[0].id,
        };
        let mut reader = Reader::new(Access::anywhere(), 0);
        apply(&mut document, &vec![select.clone(); 600], &mut reader).unwrap();
        let error = apply(&mut document, &vec![select; 600], &mut reader).unwrap_err();
        assert!(error.to_string().contains("edits"), "{error}");
    }

    #[test]
    fn host_side_file_access_is_confined_to_the_plugins_folders() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let inside = png(root.path(), "in.png", [1, 2, 3, 255], 2);
        let elsewhere = png(outside.path(), "out.png", [1, 2, 3, 255], 2);
        let confined = Access::new([root.path().to_path_buf()], FilesystemAccess::None);
        assert!(confined.readable(&inside).is_ok());
        assert!(
            confined
                .readable(
                    &root
                        .path()
                        .join("../")
                        .join(root.path().file_name().unwrap())
                        .join("in.png")
                )
                .is_ok()
        );
        let error = confined.readable(&elsewhere).unwrap_err().to_string();
        assert!(error.contains("filesystem"), "{error}");
        assert!(confined.readable(&root.path().join("missing.png")).is_err());
        assert!(confined.writable_dir(root.path()).is_ok());
        assert!(confined.writable_dir(outside.path()).is_err());
        assert!(confined.writable_dir(&inside).is_err());
        let mut reader = Reader::new(confined.clone(), 1);
        assert!(reader.rgba(&inside).is_ok());
        assert!(reader.rgba(&elsewhere).is_err());
        assert!(reader.gray(&elsewhere).is_err());
        // A symlink inside the folder cannot point out of it.
        #[cfg(unix)]
        {
            let link = root.path().join("link.png");
            std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
            assert!(confined.readable(&link).is_err());
            let folder = root.path().join("folder");
            std::os::unix::fs::symlink(outside.path(), &folder).unwrap();
            assert!(confined.writable_dir(&folder).is_err());
        }
        // Only regular files are read, so a FIFO cannot block the UI.
        #[cfg(unix)]
        {
            let fifo = root.path().join("fifo.png");
            let made = std::process::Command::new("mkfifo").arg(&fifo).status();
            if made.is_ok_and(|status| status.success()) {
                let error = reader.rgba(&fifo).unwrap_err().to_string();
                assert!(error.contains("regular file"), "{error}");
                let error = read_png(&fifo).unwrap_err().to_string();
                assert!(error.contains("regular file"), "{error}");
                let error = crate::io::import_image(&fifo).unwrap_err().to_string();
                assert!(error.contains("regular file"), "{error}");
            }
        }
        assert!(reader.rgba(root.path()).is_err());
        // The manifest's filesystem permission widens it.
        let reads = Access::new([root.path().to_path_buf()], FilesystemAccess::Read);
        assert!(reads.readable(&elsewhere).is_ok());
        assert!(reads.writable_dir(outside.path()).is_err());
        let writes = Access::new([], FilesystemAccess::Write);
        assert!(writes.writable_dir(outside.path()).is_ok());
        assert!(Access::anywhere().readable(&elsewhere).is_ok());
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
        let added = run(
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

        run(
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
            run(
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
            run(
                &mut document,
                &[Edit::RemoveLayer {
                    layer: Uuid::new_v4()
                }]
            )
            .is_err()
        );
        assert!(run(&mut document, &[Edit::SetSelection { mask: Some(mask) }]).is_err());
        document.layers[0].locked = true;
        assert!(
            run(
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
        run(&mut document, &[Edit::RemoveLayer { layer: added[0] }]).unwrap();
        assert_eq!(document.layers.len(), 1);
        let selection = dir.path().join("sel.png");
        write_gray_png(
            &GrayImage::from_pixel(64, 64, image::Luma([255])),
            &selection,
        )
        .unwrap();
        run(
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
    fn extend_canvas_parses_with_defaults_and_moves_the_content() {
        let parsed: Edit =
            serde_json::from_value(json!({"op": "extend_canvas", "left": 4, "bottom": 2})).unwrap();
        assert_eq!(
            parsed,
            Edit::ExtendCanvas {
                left: 4,
                top: 0,
                right: 0,
                bottom: 2
            }
        );
        assert!(parsed.needs_edit_access());
        assert!(!Edit::Select { layer: Uuid::nil() }.needs_edit_access());
        // Shrinking is not part of the op.
        for bad in [json!(-1), json!(1.5), json!("4")] {
            let value = json!({"op": "extend_canvas", "left": bad});
            assert!(serde_json::from_value::<Edit>(value).is_err());
        }
        let mut document = Document::new(10, 8).unwrap();
        document.layers[0].transform.x = 1.0;
        let edits = [
            parsed.clone(),
            Edit::ExtendCanvas {
                left: 0,
                top: 3,
                right: 5,
                bottom: 0,
            },
        ];
        run(&mut document, &edits).unwrap();
        assert_eq!((document.width, document.height), (19, 13));
        assert_eq!(
            (
                document.layers[0].transform.x,
                document.layers[0].transform.y
            ),
            (5.0, 3.0)
        );
        assert_eq!(origin_shift(&edits), (4.0, 3.0));
        assert_eq!(origin_shift(&[]), (0.0, 0.0));
        // The size limits refuse the whole batch.
        let mut copy = document.clone();
        let error = run(
            &mut copy,
            &[Edit::ExtendCanvas {
                left: 30_000,
                top: 0,
                right: 0,
                bottom: 0,
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("30000"), "{error}");
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

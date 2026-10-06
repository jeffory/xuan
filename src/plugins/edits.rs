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
    document::{Adjustment, Document, Layer, MAX_PIXELS, MAX_SIDE, Mask, Point, Transform},
    effects::Filter,
    paint::ShapeKind,
    selection::SelectionMode,
    text::{TextRenderer, TextStyle},
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
/// cannot lead out of those folders. Folders the host manages, such as the
/// models folder, can be read but are never written into.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Access {
    roots: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    read_anywhere: bool,
    write_anywhere: bool,
}

impl Access {
    /// No confinement, for files the host chose itself.
    pub fn anywhere() -> Self {
        Self {
            roots: Vec::new(),
            read_only: Vec::new(),
            read_anywhere: true,
            write_anywhere: true,
        }
    }

    pub fn new(roots: impl IntoIterator<Item = PathBuf>, filesystem: FilesystemAccess) -> Self {
        let mut access = Self {
            roots: Vec::new(),
            read_only: Vec::new(),
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

    /// Never write into `dir`, whether or not it exists yet, even with
    /// `filesystem = "write"`.
    pub fn read_only(mut self, dir: &Path) -> Self {
        let resolved = std::fs::canonicalize(dir).ok().or_else(|| {
            let parent = std::fs::canonicalize(dir.parent()?).ok()?;
            Some(parent.join(dir.file_name()?))
        });
        if let Some(dir) = resolved {
            self.read_only.push(dir);
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
        ensure!(
            !self.read_only.iter().any(|root| resolved.starts_with(root)),
            "{} is managed by Xuan and cannot be written into",
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
pub struct Reader {
    access: Access,
    pixels: u64,
    layers: usize,
    max_layers: usize,
    edits: usize,
    /// Draws `add_text_layer` edits; made when first needed.
    text: Option<TextRenderer>,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("access", &self.access)
            .field("pixels", &self.pixels)
            .field("layers", &self.layers)
            .field("max_layers", &self.max_layers)
            .field("edits", &self.edits)
            .finish_non_exhaustive()
    }
}

impl Reader {
    pub fn new(access: Access, max_layers: usize) -> Self {
        Self {
            access,
            pixels: 0,
            layers: 0,
            max_layers,
            edits: 0,
            text: None,
        }
    }

    /// Draw text with this renderer (the editor's, whose fonts are loaded)
    /// instead of making one.
    pub fn with_text_renderer(mut self, renderer: Option<TextRenderer>) -> Self {
        self.text = renderer;
        self
    }

    /// The text renderer, to keep for the next time.
    pub fn take_text_renderer(&mut self) -> Option<TextRenderer> {
        self.text.take()
    }

    /// Count `count` more pixels against the budget, for images the host
    /// makes itself (text, shapes, strokes that grow a layer).
    fn add_pixels(&mut self, width: u32, height: u32) -> Result<()> {
        let pixels = self
            .pixels
            .saturating_add(u64::from(width) * u64::from(height));
        ensure!(
            pixels <= MAX_PIXELS,
            "The plugin's images exceed 100 megapixels in total"
        );
        self.pixels = pixels;
        Ok(())
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
    /// Make these layers the selected ones (the last is active), for the
    /// `host/run` commands that work on the selected layers, like `merge`.
    SelectLayers {
        layers: Vec<Uuid>,
    },
    /// Move, scale or rotate a layer (with its children, for a group) to
    /// this box, in document units; missing fields keep their value.
    Transform {
        layer: Uuid,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        height: Option<f32>,
        #[serde(default)]
        rotation: Option<f32>,
    },
    /// A rectangle or ellipse combined with the selection.
    SelectRect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default)]
        ellipse: bool,
        #[serde(default)]
        mode: SelectionMode,
    },
    /// A polygon (at least three points) combined with the selection.
    SelectPolygon {
        points: Vec<[f32; 2]>,
        #[serde(default)]
        mode: SelectionMode,
    },
    /// The Magic Wand at a point of the flattened image.
    SelectColor {
        x: f32,
        y: f32,
        #[serde(default = "default_tolerance")]
        tolerance: u8,
        #[serde(default = "yes")]
        contiguous: bool,
        #[serde(default)]
        mode: SelectionMode,
    },
    /// Select → Color Range over the flattened image.
    SelectColorRange {
        colors: Vec<Color>,
        #[serde(default)]
        exclude: Vec<Color>,
        #[serde(default = "default_fuzziness")]
        fuzziness: u32,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        mode: SelectionMode,
    },
    /// Grow (positive) or shrink (negative) the selection by whole pixels.
    GrowSelection {
        by: i32,
    },
    /// Soften the selection's edge with a blur of this radius.
    FeatherSelection {
        radius: f32,
    },
    /// Fill the selection (or the whole layer) with a colour.
    Fill {
        #[serde(default)]
        layer: Option<Uuid>,
        color: Color,
    },
    /// Paint a brush stroke through the points, in document coordinates.
    Stroke {
        #[serde(default)]
        layer: Option<Uuid>,
        points: Vec<[f32; 2]>,
        #[serde(default = "black")]
        color: Color,
        #[serde(default = "default_brush_size")]
        size: f32,
        #[serde(default = "default_hardness")]
        hardness: f32,
        #[serde(default = "one")]
        opacity: f32,
        #[serde(default)]
        erase: bool,
    },
    /// A filter applied to a layer's pixels inside the selection, written as
    /// in `.xuan` files, e.g. `{"GaussianBlur": {"radius": 4}}`.
    ApplyFilter {
        #[serde(default)]
        layer: Option<Uuid>,
        filter: Filter,
    },
    /// An adjustment applied to a layer's pixels inside the selection,
    /// written as in `.xuan` files, e.g. `"Invert"`.
    ApplyAdjustment {
        #[serde(default)]
        layer: Option<Uuid>,
        adjustment: Adjustment,
    },
    /// A non-destructive adjustment or filter layer, masked by the selection.
    AddAdjustmentLayer {
        #[serde(default)]
        adjustment: Option<Adjustment>,
        #[serde(default)]
        filter: Option<Filter>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        above: Option<Uuid>,
    },
    /// An empty pixel layer the size of the canvas.
    AddEmptyLayer {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        above: Option<Uuid>,
    },
    /// A mask layer made from the selection (all white without one).
    AddMaskLayer {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        above: Option<Uuid>,
    },
    /// An editable text layer with its top-left corner at `x`, `y`.
    AddTextLayer {
        text: String,
        #[serde(default)]
        x: f32,
        #[serde(default)]
        y: f32,
        #[serde(default)]
        family: Option<String>,
        #[serde(default)]
        size: Option<f32>,
        #[serde(default = "black")]
        color: Color,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default)]
        underline: bool,
        #[serde(default)]
        strikethrough: bool,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        above: Option<Uuid>,
    },
    /// An editable shape layer covering the box.
    AddShapeLayer {
        shape: ShapeKind,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        #[serde(default = "black")]
        color: Color,
        #[serde(default)]
        corner_radius: f32,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        above: Option<Uuid>,
    },
    /// Crop the canvas to a rectangle of the document, as the Crop tool does.
    Crop {
        x: f32,
        y: f32,
        width: u32,
        height: u32,
    },
    /// Image → Canvas Size: a new canvas size with the content placed by
    /// `anchor` (`[0, 0]` top-left, `[0.5, 0.5]` centred, the default).
    ResizeCanvas {
        width: u32,
        height: u32,
        #[serde(default = "centre")]
        anchor: [f32; 2],
    },
    /// Image → Image Size: scale the whole document to this size.
    ResizeImage {
        width: u32,
        height: u32,
    },
}

/// A colour written `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub fn parse(text: &str) -> Result<Self> {
        let hex = text
            .strip_prefix('#')
            .filter(|hex| matches!(hex.len(), 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .with_context(|| format!("`{text}` is not a colour like #rrggbb or #rrggbbaa"))?;
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
        let alpha = if hex.len() == 8 { byte(6) } else { 255 };
        Ok(Self([byte(0), byte(2), byte(4), alpha]))
    }
}

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let [r, g, b, a] = self.0;
        serializer.serialize_str(&format!("#{r:02x}{g:02x}{b:02x}{a:02x}"))
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

fn default_tolerance() -> u8 {
    32
}
fn default_fuzziness() -> u32 {
    crate::selection_ops::ColorRange::DEFAULT_FUZZINESS
}
fn default_brush_size() -> f32 {
    20.0
}
fn default_hardness() -> f32 {
    0.8
}
fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn black() -> Color {
    Color([0, 0, 0, 255])
}
fn centre() -> [f32; 2] {
    [0.5, 0.5]
}

/// Points one `select_polygon` or `stroke` edit may have.
pub const MAX_POINTS: usize = 10_000;
/// The largest `grow_selection` and `feather_selection` amount, in pixels.
pub const MAX_SELECTION_AMOUNT: f32 = 256.0;

impl Edit {
    /// Whether the edit changes the document beyond what a read-only plugin
    /// may propose as a result.
    pub fn needs_edit_access(&self) -> bool {
        matches!(self, Self::ExtendCanvas { .. })
    }
}

impl Edit {
    /// Whether the edit changes only the selection, like a `mask` output, and
    /// so may be proposed by a plugin that has `document = "read"`.
    pub fn is_selection_only(&self) -> bool {
        matches!(
            self,
            Self::SetSelection { .. }
                | Self::SelectRect { .. }
                | Self::SelectPolygon { .. }
                | Self::SelectColor { .. }
                | Self::SelectColorRange { .. }
                | Self::GrowSelection { .. }
                | Self::FeatherSelection { .. }
        )
    }

    /// Whether the edit may only be sent with `document/edit`, not returned
    /// in a result: it changes the canvas, and a result's images and masks
    /// are placed on the canvas as it was sent. (`extend_canvas` is the
    /// exception, as results account for it.)
    pub fn direct_only(&self) -> bool {
        matches!(
            self,
            Self::Crop { .. } | Self::ResizeCanvas { .. } | Self::ResizeImage { .. }
        )
    }

    /// The op's name, as plugins write it.
    pub fn op(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.get("op")?.as_str().map(str::to_owned))
            .unwrap_or_default()
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
            Edit::SelectLayers { layers } => {
                ensure!(!layers.is_empty(), "List at least one layer");
                ensure!(layers.len() <= 1000, "Too many layers");
                for id in layers {
                    find(document, *id)?;
                }
                document.selected = layers.iter().copied().collect();
                document.active = layers.last().copied();
            }
            Edit::Transform {
                layer,
                x,
                y,
                width,
                height,
                rotation,
            } => {
                let target = find(document, *layer)?;
                ensure!(!target.locked, "Layer {} is locked", target.name);
                document.select(*layer, false);
                let mut transform = crate::operations::transform_box(document, false)
                    .context("The layer cannot be transformed")?;
                for (value, field) in [
                    (x, &mut transform.x),
                    (y, &mut transform.y),
                    (width, &mut transform.width),
                    (height, &mut transform.height),
                    (rotation, &mut transform.rotation),
                ] {
                    if let Some(value) = value {
                        *field = *value;
                    }
                }
                ensure!(transform.valid(), "Invalid layer placement");
                crate::operations::apply_transform(document, transform, false)?;
                crate::paint::refresh_shapes(document)?;
            }
            Edit::SelectRect {
                x,
                y,
                width,
                height,
                ellipse,
                mode,
            } => {
                ensure!(
                    [x, y, width, height].iter().all(|v| v.is_finite())
                        && *width > 0.0
                        && *height > 0.0,
                    "The rectangle needs a finite position and a positive size"
                );
                let mask = crate::selection::rectangle(
                    document.width,
                    document.height,
                    Point::new(*x, *y),
                    Point::new(x + width, y + height),
                    *ellipse,
                );
                crate::selection::combine(document, mask, *mode);
            }
            Edit::SelectPolygon { points, mode } => {
                let points = valid_points(points)?;
                ensure!(points.len() >= 3, "A polygon needs at least three points");
                let mask = crate::selection::polygon(document.width, document.height, &points);
                crate::selection::combine(document, mask, *mode);
            }
            Edit::SelectColor {
                x,
                y,
                tolerance,
                contiguous,
                mode,
            } => {
                ensure!(
                    x.is_finite()
                        && y.is_finite()
                        && (0.0..document.width as f32).contains(x)
                        && (0.0..document.height as f32).contains(y),
                    "The point must be inside the canvas"
                );
                let pixels = crate::render::render(document);
                let mask =
                    crate::selection::wand(&pixels, Point::new(*x, *y), *tolerance, *contiguous);
                crate::selection::combine(document, mask, *mode);
            }
            Edit::SelectColorRange {
                colors,
                exclude,
                fuzziness,
                invert,
                mode,
            } => {
                ensure!(
                    !colors.is_empty() && colors.len() + exclude.len() <= 256,
                    "List between 1 and 256 colours"
                );
                let rgb = |c: &Color| [c.0[0], c.0[1], c.0[2]];
                let range = crate::selection_ops::ColorRange {
                    include: colors.iter().map(rgb).collect(),
                    exclude: exclude.iter().map(rgb).collect(),
                    fuzziness: (*fuzziness).min(crate::selection_ops::ColorRange::MAX_FUZZINESS),
                    invert: *invert,
                };
                let mask = range.mask(&crate::render::render(document));
                crate::selection::combine(document, mask, *mode);
            }
            Edit::GrowSelection { by } => {
                ensure!(
                    by.unsigned_abs() as f32 <= MAX_SELECTION_AMOUNT,
                    "Grow or shrink by at most {MAX_SELECTION_AMOUNT} pixels"
                );
                if let Some(selection) = document.selection.clone() {
                    let cancel = std::sync::atomic::AtomicBool::new(false);
                    let radius = by.unsigned_abs();
                    let grown = if *by >= 0 {
                        crate::selection_ops::expand(&selection, radius, &cancel)
                    } else {
                        crate::selection_ops::contract(&selection, radius, &cancel)
                    };
                    document.selection = grown.map(Arc::new);
                }
            }
            Edit::FeatherSelection { radius } => {
                ensure!(
                    radius.is_finite() && (0.0..=MAX_SELECTION_AMOUNT).contains(radius),
                    "The feather radius must be between 0 and {MAX_SELECTION_AMOUNT}"
                );
                if let Some(selection) = &document.selection
                    && *radius > 0.0
                {
                    document.selection = Some(Arc::new(crate::gpu::blur_gray(selection, *radius)));
                }
            }
            Edit::Fill { layer, color } => {
                activate(document, *layer)?;
                crate::paint::fill(document, color.0, false, false)?;
            }
            Edit::Stroke {
                layer,
                points,
                color,
                size,
                hardness,
                opacity,
                erase,
            } => {
                let points = valid_points(points)?;
                ensure!(!points.is_empty(), "A stroke needs at least one point");
                ensure!(
                    size.is_finite() && (1.0..=2000.0).contains(size),
                    "The brush size must be between 1 and 2000"
                );
                ensure!(
                    hardness.is_finite() && (0.0..=1.0).contains(hardness),
                    "The hardness must be between 0 and 1"
                );
                valid_opacity(*opacity)?;
                activate(document, *layer)?;
                let brush = crate::paint::Brush {
                    diameter: *size,
                    hardness: *hardness,
                    opacity: *opacity,
                    color: color.0,
                    ..crate::paint::Brush::default()
                };
                let mode = if *erase {
                    crate::paint::PaintMode::Erase
                } else {
                    crate::paint::PaintMode::Paint
                };
                let mut stroke = crate::paint::Stroke::default();
                let pairs = points
                    .windows(2)
                    .map(|pair| (pair[0], pair[1]))
                    .chain((points.len() == 1).then(|| (points[0], points[0])));
                for (from, to) in pairs {
                    stroke.segment(
                        document,
                        from,
                        to,
                        &brush,
                        &brush,
                        crate::paint::StrokeOptions {
                            mode,
                            mask_target: false,
                            source: None,
                            clone_offset: Point::default(),
                        },
                    )?;
                }
            }
            Edit::ApplyFilter { layer, filter } => {
                filter.validate()?;
                activate(document, *layer)?;
                crate::effects::apply_filter(document, filter, false)?;
            }
            Edit::ApplyAdjustment { layer, adjustment } => {
                crate::effects::validate_adjustment(adjustment)?;
                activate(document, *layer)?;
                crate::effects::apply_adjustment(document, adjustment, false)?;
            }
            Edit::AddAdjustmentLayer {
                adjustment,
                filter,
                name,
                above,
            } => {
                ensure!(
                    adjustment.is_some() != filter.is_some(),
                    "Give either an adjustment or a filter"
                );
                if let Some(adjustment) = adjustment {
                    crate::effects::validate_adjustment(adjustment)?;
                }
                if let Some(filter) = filter {
                    filter.validate()?;
                }
                reader.add_layer()?;
                let default = adjustment
                    .as_ref()
                    .map(Adjustment::name)
                    .or_else(|| filter.as_ref().map(Filter::name))
                    .unwrap_or_default();
                let mut layer =
                    Layer::blank(layer_name(name, default)?, document.width, document.height);
                layer.adjustment = adjustment.clone();
                layer.filter = filter.clone();
                if document.selection.is_some() {
                    layer.mask = Some(Mask {
                        pixels: Arc::new(crate::paint::mask_from_selection(document, &layer)),
                        ..Mask::white()
                    });
                }
                added.push(insert_above(document, layer, *above)?);
            }
            Edit::AddEmptyLayer { name, above } => {
                reader.add_layer()?;
                let layer =
                    Layer::blank(layer_name(name, "Layer")?, document.width, document.height);
                added.push(insert_above(document, layer, *above)?);
            }
            Edit::AddMaskLayer { name, above } => {
                reader.add_layer()?;
                let mut layer =
                    Layer::mask(layer_name(name, "Mask")?, document.width, document.height);
                layer.mask.as_mut().unwrap().pixels =
                    Arc::new(crate::paint::mask_from_selection(document, &layer));
                added.push(insert_above(document, layer, *above)?);
            }
            Edit::AddTextLayer {
                text,
                x,
                y,
                family,
                size,
                color,
                bold,
                italic,
                underline,
                strikethrough,
                name,
                above,
            } => {
                reader.add_layer()?;
                let defaults = TextStyle::default();
                let style = TextStyle {
                    content: text.clone(),
                    family: family.clone().unwrap_or(defaults.family),
                    size: size.unwrap_or(defaults.size),
                    color: color.0,
                    bold: *bold,
                    italic: *italic,
                    underline: *underline,
                    strikethrough: *strikethrough,
                };
                style.validate()?;
                ensure!(!text.trim().is_empty(), "The text is empty");
                let pixels = reader
                    .text
                    .get_or_insert_with(TextRenderer::default)
                    .render(&style)?;
                reader.add_pixels(pixels.width(), pixels.height())?;
                let mut layer = Layer::image(
                    match name {
                        Some(_) => layer_name(name, "")?,
                        None => style.layer_name(),
                    },
                    pixels,
                );
                layer.text = Some(style);
                layer.transform.x = *x;
                layer.transform.y = *y;
                ensure!(layer.transform.valid(), "Invalid layer placement");
                added.push(insert_above(document, layer, *above)?);
            }
            Edit::AddShapeLayer {
                shape,
                x,
                y,
                width,
                height,
                color,
                corner_radius,
                name,
                above,
            } => {
                reader.add_layer()?;
                ensure!(
                    [x, y, width, height, corner_radius]
                        .iter()
                        .all(|v| v.is_finite())
                        && *width >= 1.0
                        && *height >= 1.0
                        && *corner_radius >= 0.0,
                    "The shape needs a finite position and a size of at least 1"
                );
                reader.add_pixels(width.round() as u32, height.round() as u32)?;
                let mut layer = crate::paint::shape(
                    Point::new(*x, *y),
                    Point::new(x + width, y + height),
                    *shape,
                    color.0,
                    *corner_radius,
                )?;
                if name.is_some() {
                    layer.name = layer_name(name, "")?;
                }
                ensure!(layer.transform.valid(), "Invalid layer placement");
                added.push(insert_above(document, layer, *above)?);
            }
            Edit::Crop {
                x,
                y,
                width,
                height,
            } => {
                ensure!(
                    x.is_finite() && y.is_finite(),
                    "The crop needs a finite position"
                );
                crate::operations::crop(
                    document,
                    Point::new(*x, *y),
                    Point::new(x + *width as f32, y + *height as f32),
                )?;
            }
            Edit::ResizeCanvas {
                width,
                height,
                anchor,
            } => {
                ensure!(
                    anchor.iter().all(|a| (0.0..=1.0).contains(a)),
                    "The anchor's values must be between 0 and 1"
                );
                crate::operations::canvas_size(document, *width, *height, *anchor)?;
            }
            Edit::ResizeImage { width, height } => {
                crate::operations::image_size(document, *width, *height)?;
            }
        }
    }
    document.validate()?;
    Ok(added)
}

/// Make `layer` the active layer when given; there must be an active layer.
fn activate(document: &mut Document, layer: Option<Uuid>) -> Result<()> {
    if let Some(layer) = layer {
        find(document, layer)?;
        document.select(layer, false);
    }
    ensure!(document.active().is_some(), "Select a layer first");
    Ok(())
}

/// Insert a new layer above `above`, or above the active layer.
fn insert_above(document: &mut Document, layer: Layer, above: Option<Uuid>) -> Result<Uuid> {
    if let Some(above) = above {
        find(document, above)?;
        document.select(above, false);
    }
    let id = layer.id;
    document.insert(layer);
    Ok(id)
}

fn layer_name(name: &Option<String>, default: &str) -> Result<String> {
    match name {
        Some(name) => {
            ensure!(name.len() <= 256, "Layer name too long");
            Ok(name.clone())
        }
        None => Ok(default.to_owned()),
    }
}

fn valid_points(points: &[[f32; 2]]) -> Result<Vec<Point>> {
    ensure!(
        points.len() <= MAX_POINTS,
        "At most {MAX_POINTS} points are allowed"
    );
    ensure!(
        points
            .iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= 1_000_000.0),
        "Points must be finite"
    );
    Ok(points.iter().map(|[x, y]| Point::new(*x, *y)).collect())
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
        // A read-only folder inside can be read but not written into, even
        // when writing anywhere is allowed, and even before it exists.
        let models = root.path().join("models");
        for filesystem in [FilesystemAccess::None, FilesystemAccess::Write] {
            let access = Access::new([root.path().to_path_buf()], filesystem).read_only(&models);
            std::fs::create_dir_all(models.join("sub")).unwrap();
            let model = png(&models, "m.png", [1, 2, 3, 255], 2);
            assert!(access.readable(&model).is_ok());
            for dir in [models.clone(), models.join("sub")] {
                let error = access.writable_dir(&dir).unwrap_err().to_string();
                assert!(error.contains("managed by Xuan"), "{error}");
            }
            assert!(access.writable_dir(root.path()).is_ok());
            std::fs::remove_dir_all(&models).unwrap();
        }
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
        assert!(Edit::SetSelection { mask: None }.is_selection_only());
        assert!(!Edit::Select { layer: Uuid::nil() }.is_selection_only());
        assert!(!parsed.is_selection_only());
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

    fn edit(value: Value) -> Edit {
        serde_json::from_value(value).unwrap()
    }

    /// A 40 × 30 document with one opaque grey pixel layer.
    fn grey_document() -> (Document, Uuid) {
        let mut document = Document::new(40, 30).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            40,
            30,
            image::Rgba([100, 100, 100, 255]),
        )));
        let id = document.layers[0].id;
        (document, id)
    }

    #[test]
    fn colours_parse_and_print() {
        assert_eq!(Color::parse("#ff8000").unwrap(), Color([255, 128, 0, 255]));
        assert_eq!(
            Color::parse("#FF800080").unwrap(),
            Color([255, 128, 0, 128])
        );
        for bad in ["ff8000", "#ff80", "#gg8000", "#ff8000ff00", ""] {
            assert!(Color::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            serde_json::to_value(Color([1, 2, 3, 4])).unwrap(),
            json!("#01020304")
        );
        assert!(serde_json::from_value::<Edit>(json!({"op": "fill", "color": "red"})).is_err());
    }

    #[test]
    fn selection_ops_combine_with_the_selection() {
        let (mut document, _) = grey_document();
        let ops = [
            json!({"op": "select_rect", "x": 0, "y": 0, "width": 10, "height": 10}),
            json!({"op": "select_rect", "x": 20, "y": 0, "width": 10, "height": 10, "ellipse": true, "mode": "add"}),
            json!({"op": "select_polygon", "points": [[0, 20], [10, 20], [0, 30]], "mode": "add"}),
        ];
        let edits: Vec<Edit> = ops.into_iter().map(edit).collect();
        assert!(edits.iter().all(Edit::is_selection_only));
        assert!(!edits.iter().any(Edit::direct_only));
        run(&mut document, &edits).unwrap();
        let selection = document.selection.clone().unwrap();
        assert_eq!(selection.get_pixel(5, 5)[0], 255);
        assert_eq!(selection.get_pixel(25, 5)[0], 255);
        assert_eq!(selection.get_pixel(20, 0)[0], 0, "outside the ellipse");
        assert_eq!(selection.get_pixel(2, 25)[0], 255);
        assert_eq!(selection.get_pixel(35, 25)[0], 0);
        // Subtract, grow and feather.
        run(
            &mut document,
            &[edit(
                json!({"op": "select_rect", "x": 0, "y": 0, "width": 40, "height": 15, "mode": "subtract"}),
            )],
        )
        .unwrap();
        assert_eq!(document.selection.as_ref().unwrap().get_pixel(5, 5)[0], 0);
        run(
            &mut document,
            &[edit(json!({"op": "grow_selection", "by": 2}))],
        )
        .unwrap();
        assert_eq!(
            document.selection.as_ref().unwrap().get_pixel(2, 18)[0],
            255
        );
        run(
            &mut document,
            &[edit(json!({"op": "grow_selection", "by": -40}))],
        )
        .unwrap();
        assert!(crate::selection::bounds(document.selection.as_ref().unwrap()).is_none());
        run(
            &mut document,
            &[
                edit(json!({"op": "select_rect", "x": 10, "y": 10, "width": 20, "height": 10})),
                edit(json!({"op": "feather_selection", "radius": 3})),
            ],
        )
        .unwrap();
        let edge = document.selection.as_ref().unwrap().get_pixel(10, 15)[0];
        assert!(edge > 0 && edge < 255, "{edge}");
        for bad in [
            json!({"op": "select_polygon", "points": [[0, 0], [1, 1]]}),
            json!({"op": "select_rect", "x": 0, "y": 0, "width": 0, "height": 4}),
            json!({"op": "grow_selection", "by": 1000}),
            json!({"op": "feather_selection", "radius": -1}),
        ] {
            assert!(
                run(&mut document.clone(), &[edit(bad.clone())]).is_err(),
                "{bad}"
            );
        }
        let too_many = vec![[1.0f32, 1.0]; MAX_POINTS + 1];
        let error = run(
            &mut document,
            &[Edit::SelectPolygon {
                points: too_many,
                mode: SelectionMode::Replace,
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("points"), "{error}");
    }

    #[test]
    fn colour_selections_read_the_flattened_image() {
        let (mut document, base) = grey_document();
        let pixels = Arc::make_mut(document.layers[0].pixels.as_mut().unwrap());
        for y in 0..30 {
            for x in 20..40 {
                pixels.put_pixel(x, y, image::Rgba([220, 30, 30, 255]));
            }
        }
        run(
            &mut document,
            &[edit(
                json!({"op": "select_color", "x": 30, "y": 5, "tolerance": 10}),
            )],
        )
        .unwrap();
        let selection = document.selection.clone().unwrap();
        assert_eq!(selection.get_pixel(25, 10)[0], 255);
        assert_eq!(selection.get_pixel(5, 10)[0], 0);
        run(
            &mut document,
            &[edit(
                json!({"op": "select_color_range", "colors": ["#646464"], "fuzziness": 10}),
            )],
        )
        .unwrap();
        let selection = document.selection.clone().unwrap();
        assert_eq!(selection.get_pixel(5, 10)[0], 255);
        assert_eq!(selection.get_pixel(25, 10)[0], 0);
        assert!(
            run(
                &mut document,
                &[edit(json!({"op": "select_color", "x": 50, "y": 5}))]
            )
            .is_err()
        );
        assert!(
            run(
                &mut document,
                &[edit(json!({"op": "select_color_range", "colors": []}))]
            )
            .is_err()
        );
        assert_eq!(document.active, Some(base));
    }

    #[test]
    fn pixel_ops_respect_the_selection_and_locks() {
        let (mut document, base) = grey_document();
        run(
            &mut document,
            &[
                edit(json!({"op": "select_rect", "x": 0, "y": 0, "width": 20, "height": 30})),
                edit(json!({"op": "fill", "layer": base, "color": "#ff0000"})),
            ],
        )
        .unwrap();
        let pixels = document.layers[0].pixels.clone().unwrap();
        assert_eq!(pixels.get_pixel(5, 5).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(30, 5).0, [100, 100, 100, 255]);
        run(
            &mut document,
            &[edit(
                json!({"op": "apply_adjustment", "adjustment": "Invert"}),
            )],
        )
        .unwrap();
        let pixels = document.layers[0].pixels.clone().unwrap();
        assert_eq!(pixels.get_pixel(5, 5).0, [0, 255, 255, 255]);
        assert_eq!(pixels.get_pixel(30, 5).0, [100, 100, 100, 255]);
        run(
            &mut document,
            &[
                edit(json!({"op": "select_rect", "x": 0, "y": 0, "width": 40, "height": 30})),
                edit(json!({"op": "apply_filter", "filter": {"GaussianBlur": {"radius": 3}}})),
            ],
        )
        .unwrap();
        // The blur pads the layer, so read the flattened image.
        let pixels = crate::render::render(&document);
        let edge = pixels.get_pixel(20, 15).0;
        assert!(edge[0] > 0 && edge[0] < 100, "{edge:?}");
        // A stroke paints along its points and stays one change.
        run(
            &mut document,
            &[
                Edit::SetSelection { mask: None },
                edit(json!({"op": "stroke", "points": [[2, 25], [38, 25]], "color": "#0000ff", "size": 4, "hardness": 1})),
            ],
        )
        .unwrap();
        let pixels = crate::render::render(&document);
        assert_eq!(pixels.get_pixel(20, 25).0, [0, 0, 255, 255]);
        assert_ne!(pixels.get_pixel(20, 15).0, [0, 0, 255, 255]);
        run(
            &mut document,
            &[edit(json!({"op": "stroke", "points": [[20, 25]], "size": 6, "erase": true, "hardness": 1}))],
        )
        .unwrap();
        // Erased down to the white canvas the flattened image shows through.
        assert_ne!(
            crate::render::render(&document).get_pixel(20, 25).0,
            [0, 0, 255, 255]
        );
        // Bad values and locked layers are refused.
        for bad in [
            json!({"op": "apply_filter", "filter": {"GaussianBlur": {"radius": 1000}}}),
            json!({"op": "apply_filter", "filter": {"Sharpen": {}}}),
            json!({"op": "stroke", "points": [], "size": 4}),
            json!({"op": "stroke", "points": [[1, 1]], "size": 0}),
            json!({"op": "stroke", "points": [[1, 1]], "opacity": 2}),
            json!({"op": "fill", "layer": Uuid::new_v4(), "color": "#000000"}),
        ] {
            let parsed = serde_json::from_value::<Edit>(bad.clone());
            assert!(
                parsed.is_err() || run(&mut document.clone(), &[parsed.unwrap()]).is_err(),
                "{bad}"
            );
        }
        document.layers[0].locked = true;
        for locked in [
            json!({"op": "fill", "color": "#000000"}),
            json!({"op": "stroke", "points": [[1, 1]]}),
            json!({"op": "apply_adjustment", "adjustment": "Invert"}),
            json!({"op": "apply_filter", "filter": {"GaussianBlur": {"radius": 2}}}),
            json!({"op": "transform", "layer": base, "x": 3}),
        ] {
            let error = run(&mut document.clone(), &[edit(locked.clone())]).unwrap_err();
            assert!(error.to_string().contains("lock"), "{locked}: {error}");
        }
    }

    #[test]
    fn new_layers_are_editable_and_counted() {
        let (mut document, base) = grey_document();
        run(
            &mut document,
            &[edit(
                json!({"op": "select_rect", "x": 0, "y": 0, "width": 10, "height": 10}),
            )],
        )
        .unwrap();
        let added = run(
            &mut document,
            &[
                edit(json!({"op": "add_adjustment_layer", "adjustment": {"HueSaturation": {"hue": 30, "saturation": 0, "lightness": 0, "colorize": false}}})),
                edit(json!({"op": "add_adjustment_layer", "filter": {"GaussianBlur": {"radius": 2}}, "name": "Soft"})),
                edit(json!({"op": "add_empty_layer", "name": "Paint", "above": base})),
                edit(json!({"op": "add_mask_layer"})),
                edit(json!({"op": "add_shape_layer", "shape": "Ellipse", "x": 4, "y": 6, "width": 12, "height": 8, "color": "#00ff00"})),
            ],
        )
        .unwrap();
        assert_eq!(added.len(), 5);
        let layer = |id: Uuid| document.layers.iter().find(|l| l.id == id).unwrap();
        assert!(layer(added[0]).adjustment.is_some());
        assert!(layer(added[0]).mask.is_some(), "masked by the selection");
        assert_eq!(layer(added[1]).name, "Soft");
        assert!(layer(added[1]).filter.is_some());
        assert!(layer(added[2]).pixels.is_none());
        assert_eq!(layer(added[2]).name, "Paint");
        // Added right above the base layer.
        let index = |id: Uuid| document.layers.iter().position(|l| l.id == id).unwrap();
        assert_eq!(index(added[2]), index(base) + 1);
        assert!(layer(added[3]).standalone_mask);
        let shape = layer(added[4]);
        assert_eq!(shape.shape.as_ref().unwrap().color, [0, 255, 0, 255]);
        assert_eq!((shape.transform.x, shape.transform.y), (4.0, 6.0));
        assert_eq!(document.active, Some(added[4]));
        for bad in [
            json!({"op": "add_adjustment_layer"}),
            json!({"op": "add_adjustment_layer", "adjustment": "Invert", "filter": {"GaussianBlur": {"radius": 2}}}),
            json!({"op": "add_shape_layer", "shape": "Ellipse", "x": 0, "y": 0, "width": 0, "height": 5}),
            json!({"op": "add_empty_layer", "above": Uuid::new_v4()}),
            json!({"op": "add_empty_layer", "name": "x".repeat(300)}),
        ] {
            assert!(
                run(&mut document.clone(), &[edit(bad.clone())]).is_err(),
                "{bad}"
            );
        }
        // New layers count against the batch's layer limit.
        let empty = edit(json!({"op": "add_empty_layer"}));
        assert!(run(&mut document.clone(), &vec![empty; MAX_LAYERS + 1]).is_err());
    }

    #[test]
    fn text_layers_are_drawn_and_keep_their_style() {
        let (mut document, _) = grey_document();
        let added = run(
            &mut document,
            &[edit(json!({"op": "add_text_layer", "text": "Hi", "x": 3, "y": 4, "size": 12, "color": "#ff0000", "bold": true}))],
        )
        .unwrap();
        let layer = document.layers.iter().find(|l| l.id == added[0]).unwrap();
        let style = layer.text.as_ref().unwrap();
        assert_eq!(
            (style.content.as_str(), style.size, style.bold),
            ("Hi", 12.0, true)
        );
        assert_eq!(layer.name, "Hi");
        assert_eq!((layer.transform.x, layer.transform.y), (3.0, 4.0));
        assert!(layer.pixels.as_ref().unwrap().pixels().any(|p| p.0[3] > 0));
        for bad in [
            json!({"op": "add_text_layer", "text": "  "}),
            json!({"op": "add_text_layer", "text": "x", "size": 5000}),
            json!({"op": "add_text_layer", "text": "x".repeat(20_000)}),
        ] {
            assert!(
                run(&mut document.clone(), &[edit(bad.clone())]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn transforms_layer_selection_and_canvas_ops() {
        let (mut document, base) = grey_document();
        let second = run(&mut document, &[edit(json!({"op": "add_empty_layer"}))]).unwrap()[0];
        run(
            &mut document,
            &[edit(json!({"op": "transform", "layer": base, "x": 5, "y": 6, "width": 20, "rotation": 15}))],
        )
        .unwrap();
        let transform = document
            .layers
            .iter()
            .find(|l| l.id == base)
            .unwrap()
            .transform;
        assert_eq!(
            (
                transform.x,
                transform.y,
                transform.width,
                transform.height,
                transform.rotation
            ),
            (5.0, 6.0, 20.0, 30.0, 15.0)
        );
        assert!(
            run(
                &mut document.clone(),
                &[edit(json!({"op": "transform", "layer": base, "width": -3}))]
            )
            .is_err()
        );
        run(
            &mut document,
            &[edit(
                json!({"op": "select_layers", "layers": [base, second]}),
            )],
        )
        .unwrap();
        assert_eq!(document.selected.len(), 2);
        assert_eq!(document.active, Some(second));
        assert!(
            run(
                &mut document.clone(),
                &[edit(json!({"op": "select_layers", "layers": []}))]
            )
            .is_err()
        );
        assert!(
            run(
                &mut document.clone(),
                &[edit(
                    json!({"op": "select_layers", "layers": [Uuid::new_v4()]})
                )]
            )
            .is_err()
        );

        let canvas = [
            edit(json!({"op": "crop", "x": 2, "y": 3, "width": 20, "height": 10})),
            edit(json!({"op": "resize_canvas", "width": 30, "height": 20, "anchor": [0, 0]})),
            edit(json!({"op": "resize_image", "width": 60, "height": 40})),
        ];
        assert!(canvas.iter().all(Edit::direct_only));
        assert!(!canvas.iter().any(Edit::is_selection_only));
        assert_eq!(canvas[0].op(), "crop");
        run(&mut document, &canvas[..1]).unwrap();
        assert_eq!((document.width, document.height), (20, 10));
        let transform = document
            .layers
            .iter()
            .find(|l| l.id == base)
            .unwrap()
            .transform;
        assert_eq!((transform.x, transform.y), (3.0, 3.0));
        run(&mut document, &canvas[1..]).unwrap();
        assert_eq!((document.width, document.height), (60, 40));
        let transform = document
            .layers
            .iter()
            .find(|l| l.id == base)
            .unwrap()
            .transform;
        assert_eq!((transform.x, transform.y), (6.0, 6.0));
        for bad in [
            json!({"op": "crop", "x": 0, "y": 0, "width": 0, "height": 5}),
            json!({"op": "resize_canvas", "width": 40_000, "height": 5}),
            json!({"op": "resize_canvas", "width": 10, "height": 5, "anchor": [2, 0]}),
            json!({"op": "resize_image", "width": 0, "height": 5}),
        ] {
            assert!(
                run(&mut document.clone(), &[edit(bad.clone())]).is_err(),
                "{bad}"
            );
        }
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

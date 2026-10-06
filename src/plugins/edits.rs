//! What plugins see of a document and how they change it: JSON descriptions,
//! PNG exports and batched edits that the editor records as one undo step.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
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
        "layers": document
            .layers
            .iter()
            .map(|layer| describe_layer(document, layer))
            .collect::<Vec<_>>(),
    })
}

/// Describe one layer. Masks are reported both ways round: an image lists
/// the mask layers attached to it under `masks`, and an effect layer (mask,
/// adjustment or filter) attached to an image names it in `attached_to`.
pub fn describe_layer(document: &Document, layer: &Layer) -> Value {
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
    } else if layer.shape.is_some() {
        "shape"
    } else {
        "image"
    };
    let shape = layer.shape.as_ref().map(|shape| {
        json!({
            "shape": shape.kind,
            "color": Color(shape.color),
            "corner_radius": shape.corner_radius,
        })
    });
    let masks = masks(document, layer);
    // An effect layer inside an image's stack applies to that image only.
    let attached_to = document
        .attachment_owner(layer)
        .filter(|owner| *owner != layer.id);
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
        "flip_x": layer.transform.flip_x,
        "flip_y": layer.transform.flip_y,
        "pixel_width": width,
        "pixel_height": height,
        "has_mask": !masks.is_empty(),
        "masks": masks,
        "attached_to": attached_to,
        "shape": shape,
        "generated": layer.generated,
        "provenance": layer.provenance,
    })
}

/// The masks that apply to `layer`: its own (a mask, adjustment or filter
/// layer's) and, for an image, the mask layers attached to it, bottom to top.
/// Each names the layer that holds it, which is the one to pass to
/// `layer/export` with `what = "mask"` or to make active for `disable_mask`
/// and `link_mask`.
fn masks(document: &Document, layer: &Layer) -> Vec<Value> {
    let attached = document.layers.iter().filter(|child| {
        layer.can_attach_effects() && child.parent == Some(layer.id) && child.standalone_mask
    });
    std::iter::once(layer)
        .chain(attached)
        .filter_map(|holder| {
            let mask = holder.mask.as_ref()?;
            Some(json!({"layer": holder.id, "enabled": mask.enabled, "linked": mask.linked}))
        })
        .collect()
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
    /// For an image's mask: the attached mask layer it was read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_layer: Option<Uuid>,
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
    /// Work charged so far; see [`Cost`].
    cost: Cost,
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
            .field("cost", &self.cost)
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
            cost: Cost::default(),
            text: None,
        }
    }

    /// Charge the work a batch will do, before it does any, so a batch over
    /// budget is refused at once and as a whole.
    fn charge(&mut self, cost: Cost) -> Result<()> {
        let total = Cost {
            work: self.cost.work.saturating_add(cost.work),
            bytes: self.cost.bytes.saturating_add(cost.bytes),
            stroke: self.cost.stroke + cost.stroke,
        };
        ensure!(
            total.work <= MAX_WORK,
            "The edits would take too long: they touch more than {} million pixels in total \
             (filters, strokes, colour selections and masks on a large canvas count many times). \
             Send fewer or smaller edits per request",
            MAX_WORK / 1_000_000
        );
        ensure!(
            total.bytes <= MAX_NEW_BYTES,
            "The edits would need more than {} MiB of new masks and layers",
            MAX_NEW_BYTES / (1024 * 1024)
        );
        ensure!(
            total.stroke <= MAX_STROKE_LENGTH,
            "The strokes are longer than {MAX_STROKE_LENGTH} pixels in total"
        );
        self.cost = total;
        Ok(())
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
    let mut mask_layer = None;
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
            // An image's mask is a mask layer attached to it.
            let holder = match layer.mask {
                Some(_) => layer,
                None => attached_mask(document, layer)?,
            };
            if holder.id != layer.id {
                mask_layer = Some(holder.id);
            }
            let mask = holder.mask.as_ref().expect("checked above");
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
        mask_layer,
    })
}

/// The one mask layer attached to an image, or an error that says where its
/// masks are.
fn attached_mask<'a>(document: &'a Document, layer: &Layer) -> Result<&'a Layer> {
    let masks: Vec<&Layer> = document
        .layers
        .iter()
        .filter(|child| {
            layer.can_attach_effects()
                && child.parent == Some(layer.id)
                && child.standalone_mask
                && child.mask.is_some()
        })
        .collect();
    match masks[..] {
        [] => bail!(
            "Layer {} has no mask (add one from the selection with the `mask` command)",
            layer.name
        ),
        [mask] => Ok(mask),
        _ => bail!(
            "Layer {} has {} mask layers attached; ask for one of them: {}",
            layer.name,
            masks.len(),
            masks
                .iter()
                .map(|mask| mask.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
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
        mask_layer: None,
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
        mask_layer: None,
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
        /// Clip to this layer below in the same parent (a layer or a folder), or with
        /// `null` release the clipping; left out, it does not change.
        #[serde(
            default,
            deserialize_with = "nullable",
            skip_serializing_if = "Option::is_none"
        )]
        clip_to: Option<Option<Uuid>>,
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
    /// Layer → Merge on these layers: one layer merges down, several merge
    /// together. The merged layer counts as added.
    MergeLayers {
        layers: Vec<Uuid>,
    },
    /// Layer → Group on these layers (siblings of the first); the new group
    /// counts as added.
    GroupLayers {
        layers: Vec<Uuid>,
    },
    /// Layer → Ungroup: the group's layers move to its parent.
    UngroupLayers {
        layer: Uuid,
    },
    /// Move a layer in the stack, as dragging it in the Layers panel does:
    /// directly `above` or `below` another layer (joining that layer's group),
    /// or, with only `parent`, to the top of that group. When `parent` is given
    /// together with `above` or `below`, it must be the target's group.
    MoveLayer {
        layer: Uuid,
        #[serde(default)]
        above: Option<Uuid>,
        #[serde(default)]
        below: Option<Uuid>,
        #[serde(default)]
        parent: Option<Uuid>,
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
    /// Fill the selection (or the whole layer) with a gradient from `start`
    /// to `end` in document coordinates: linear, or radial around `start`
    /// with `end` on the rim.
    Gradient {
        #[serde(default)]
        layer: Option<Uuid>,
        start: [f32; 2],
        end: [f32; 2],
        stops: Vec<GradientStop>,
        #[serde(default)]
        radial: bool,
        #[serde(default = "one")]
        opacity: f32,
        /// Paint the layer's mask (as luminance) instead of its pixels.
        #[serde(default)]
        mask: bool,
    },
    /// Paint a brush stroke through the points, in document coordinates.
    /// A point may carry pen pressure, `[x, y, pressure]`, which scales the
    /// size (and the opacity with `pressure_opacity`). The brush dynamics
    /// (spacing, taper, scatter, jitter) are off by default.
    Stroke {
        #[serde(default)]
        layer: Option<Uuid>,
        points: Vec<StrokePoint>,
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
        /// Pressure (and taper) also scales the opacity.
        #[serde(default)]
        pressure_opacity: bool,
        /// Distance between dabs as a fraction of the size, 0 to 10; 0 is a
        /// continuous stroke.
        #[serde(default)]
        spacing: f32,
        /// Pixels over which the stroke grows from nothing at its start.
        #[serde(default)]
        taper_in: f32,
        /// Pixels over which the stroke shrinks to nothing at its end.
        #[serde(default)]
        taper_out: f32,
        /// How far dabs scatter from the path, as a fraction of the size, 0 to 10.
        #[serde(default)]
        scatter: f32,
        /// Dabs at each spacing step, 1 to 16.
        #[serde(default = "one_u32")]
        scatter_count: u32,
        #[serde(default)]
        size_jitter: f32,
        #[serde(default)]
        opacity_jitter: f32,
        #[serde(default)]
        hue_jitter: f32,
        /// The random sequence for scatter and jitter.
        #[serde(default)]
        seed: u64,
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

/// One colour of a gradient, at `position` 0 (the start) to 1 (the end).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub position: f32,
    pub color: Color,
}

/// Most colour stops one gradient may have.
const MAX_GRADIENT_STOPS: usize = 64;

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
fn one_u32() -> u32 {
    1
}

/// A stroke point in document coordinates, written `[x, y]` or
/// `[x, y, pressure]` with the pen pressure from 0 to 1 (1 when left out).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<f32>", into = "Vec<f32>")]
pub struct StrokePoint {
    pub x: f32,
    pub y: f32,
    pub pressure: Option<f32>,
}

impl StrokePoint {
    pub fn xy(&self) -> [f32; 2] {
        [self.x, self.y]
    }
    pub fn pressure(&self) -> f32 {
        self.pressure.unwrap_or(1.0)
    }
}

impl From<[f32; 2]> for StrokePoint {
    fn from([x, y]: [f32; 2]) -> Self {
        Self {
            x,
            y,
            pressure: None,
        }
    }
}

impl TryFrom<Vec<f32>> for StrokePoint {
    type Error = String;
    fn try_from(values: Vec<f32>) -> std::result::Result<Self, String> {
        match values[..] {
            [x, y] => Ok(Self {
                x,
                y,
                pressure: None,
            }),
            [x, y, pressure] => Ok(Self {
                x,
                y,
                pressure: Some(pressure),
            }),
            _ => Err("A stroke point is [x, y] or [x, y, pressure]".into()),
        }
    }
}

impl From<StrokePoint> for Vec<f32> {
    fn from(point: StrokePoint) -> Self {
        match point.pressure {
            Some(pressure) => vec![point.x, point.y, pressure],
            None => vec![point.x, point.y],
        }
    }
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
/// Pixel visits one `document/edit` request (or one result) may cost, as
/// [`cost`] estimates them. Edits run on the editor's thread, so this keeps a
/// request from freezing it: about a second or two of work.
pub const MAX_WORK: u64 = 1_000_000_000;
/// Bytes of new canvas-size masks and drawn layers one request may make.
pub const MAX_NEW_BYTES: u64 = 256 * 1024 * 1024;
/// Total length of the strokes of one request, in document pixels.
pub const MAX_STROKE_LENGTH: f64 = 200_000.0;

/// What a batch of edits costs, estimated before it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    /// Pixel visits: renders, filters, selection changes, masks, strokes.
    pub work: u64,
    /// New memory for masks and layers the host draws.
    pub bytes: u64,
    /// Stroke length in document pixels.
    pub stroke: f64,
}

/// Estimate a batch's cost from the document as it is, following the
/// canvas size through the batch. Upper bounds: an edit that fails early
/// costs less.
pub fn cost(document: &Document, edits: &[Edit]) -> Cost {
    let (mut width, mut height) = (u64::from(document.width), u64::from(document.height));
    let layers = document.layers.len() as u64;
    let layer_area = |layer: &Option<Uuid>, canvas: u64| -> u64 {
        let id = layer.or(document.active);
        let area = id
            .and_then(|id| document.layers.iter().find(|l| l.id == id))
            .map(|l| match &l.pixels {
                Some(pixels) => u64::from(pixels.width()) * u64::from(pixels.height()),
                None => (l.transform.width.max(1.0) * l.transform.height.max(1.0)) as u64,
            })
            .unwrap_or(canvas);
        area.max(1)
    };
    let mut total = Cost::default();
    for edit in edits {
        let canvas = width * height;
        let (work, bytes) = match edit {
            Edit::SelectRect { .. } | Edit::SelectPolygon { .. } | Edit::SetSelection { .. } => {
                (canvas, 0)
            }
            // A render of every layer, then the mask.
            Edit::SelectColor { .. } | Edit::SelectColorRange { .. } => {
                (canvas.saturating_mul(2 + layers), 0)
            }
            Edit::GrowSelection { by } => (
                canvas.saturating_mul(1 + u64::from(by.unsigned_abs()) / 4),
                0,
            ),
            Edit::FeatherSelection { .. } => (canvas.saturating_mul(4), 0),
            Edit::Fill { layer, .. }
            | Edit::Gradient { layer, .. }
            | Edit::ApplyAdjustment { layer, .. } => {
                (layer_area(layer, canvas).saturating_mul(2), 0)
            }
            Edit::ApplyFilter { layer, filter } => {
                let area = layer_area(layer, canvas);
                let side = (area as f64).sqrt();
                let (pad, factor) = match *filter {
                    Filter::GaussianBlur { radius } => ((radius * 6.0) as f64, 4.0),
                    Filter::MotionBlur { distance, .. } => {
                        (distance as f64, 1.0 + distance.max(0.0) as f64 / 4.0)
                    }
                    _ => (0.0, 2.0),
                };
                let padded = (side + pad).powi(2);
                ((padded * factor) as u64, 0)
            }
            Edit::Stroke {
                points,
                size,
                spacing,
                scatter,
                scatter_count,
                size_jitter,
                opacity_jitter,
                hue_jitter,
                taper_in,
                taper_out,
                ..
            } => {
                let reach = f64::from(size.max(1.0)) + 2.0;
                let mut work = 0u64;
                let mut length = 0.0;
                let points: Vec<[f32; 2]> = points.iter().map(StrokePoint::xy).collect();
                let pairs = points
                    .windows(2)
                    .map(|pair| (pair[0], pair[1]))
                    .chain((points.len() == 1).then(|| (points[0], points[0])));
                let dabs = *spacing > 0.0
                    || *scatter > 0.0
                    || *scatter_count > 1
                    || *size_jitter > 0.0
                    || *opacity_jitter > 0.0
                    || *hue_jitter > 0.0;
                if dabs {
                    // Each dab touches at most the brush's square, and dabs
                    // are at least `MIN_STEP` apart along the stroke.
                    let length: f64 = pairs
                        .clone()
                        .map(|(a, b)| f64::from((b[0] - a[0]).hypot(b[1] - a[1])))
                        .filter(|l| l.is_finite())
                        .sum();
                    let spacing = if *spacing > 0.0 {
                        f64::from(*spacing)
                    } else {
                        f64::from(crate::paint::dynamics::DEFAULT_SPACING)
                    };
                    let step_min = f64::from(crate::paint::dynamics::MIN_STEP);
                    let size = f64::from(size.max(1.0));
                    // Smaller dabs (pressure, taper, jitter) are closer together:
                    // the worst diameter is the size, or where the step stops shrinking.
                    let per_step = |d: f64| (d + 2.0).powi(2) / (spacing * d).max(step_min);
                    let small = (step_min / spacing).min(size);
                    let per_pixel = per_step(size).max(per_step(small)) + 256.0 / step_min;
                    let count = f64::from((*scatter_count).clamp(1, 16));
                    total.stroke += length;
                    (((length + 1.0) * per_pixel * count).min(1e15) as u64, 0)
                } else {
                    for (a, b) in pairs {
                        let dx = f64::from((b[0] - a[0]).abs());
                        let dy = f64::from((b[1] - a[1]).abs());
                        if !(dx.is_finite() && dy.is_finite()) {
                            // Refused when the stroke is checked.
                            continue;
                        }
                        length += dx.hypot(dy);
                        work = work.saturating_add(((dx + reach) * (dy + reach)).min(1e15) as u64);
                    }
                    // A taper paints its stretch in short pieces of their own.
                    if *taper_in > 0.0 || *taper_out > 0.0 {
                        let tapered = f64::from(taper_in + taper_out).min(length);
                        let pieces = tapered / f64::from(crate::paint::dynamics::TAPER_PIECE) + 2.0;
                        work = work.saturating_add((pieces * reach * reach).min(1e15) as u64);
                    }
                    total.stroke += length;
                    (work, 0)
                }
            }
            Edit::AddMaskLayer { .. } | Edit::AddAdjustmentLayer { .. } => (canvas, canvas),
            Edit::AddShapeLayer {
                width: w,
                height: h,
                ..
            } => {
                let area = (f64::from(w.max(1.0)) * f64::from(h.max(1.0))).min(1e15) as u64;
                (area, area.saturating_mul(4))
            }
            Edit::MergeLayers { .. } => (canvas.saturating_mul(1 + layers), canvas * 4),
            Edit::Transform { .. } => (canvas, 0),
            Edit::ResizeImage {
                width: w,
                height: h,
            } => {
                width = u64::from(*w);
                height = u64::from(*h);
                (canvas.max(width * height), 0)
            }
            Edit::ResizeCanvas {
                width: w,
                height: h,
                ..
            } => {
                width = u64::from(*w);
                height = u64::from(*h);
                (0, 0)
            }
            Edit::Crop {
                width: w,
                height: h,
                ..
            } => {
                width = u64::from(*w);
                height = u64::from(*h);
                (0, 0)
            }
            Edit::ExtendCanvas {
                left,
                top,
                right,
                bottom,
            } => {
                width = width.saturating_add(u64::from(*left) + u64::from(*right));
                height = height.saturating_add(u64::from(*top) + u64::from(*bottom));
                (0, 0)
            }
            // Images are charged to the pixel budget as they are read, and
            // text as it is drawn.
            _ => (0, 0),
        };
        total.work = total.work.saturating_add(work);
        total.bytes = total.bytes.saturating_add(bytes);
    }
    total
}

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

/// The fields of a `document/edit` edit that name a layer, where `"$n"` may
/// stand for the n-th layer the request added before it.
const LAYER_KEYS: [&str; 5] = ["layer", "above", "below", "parent", "clip_to"];

/// A field where `null` differs from leaving it out: `Some(None)` is `null`.
fn nullable<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
/// The ids `"$n"` references are parsed to until [`apply`] resolves them:
/// "xuan" then zeros and `n`, a version-0 UUID that no layer ever has.
const REFERENCE: u128 = 0x7875_616e_0000_0000_0000_0000_0000_0000;

/// The id that stands for `"$n"` until the edit runs.
fn reference(n: u32) -> Uuid {
    Uuid::from_u128(REFERENCE | u128::from(n))
}

/// Which `"$n"` an id stands for, if it is a reference.
fn referenced(id: &Uuid) -> Option<u32> {
    let value = id.as_u128();
    (value & !u128::from(u32::MAX) == REFERENCE).then_some(value as u32)
}

/// Parse the `edits` of a `document/edit` request. Besides layer ids, the
/// layer fields (`layer`, `above`, `below`, `parent` and the items of
/// `layers`) may say `"$n"`: the n-th layer (from 1) this request added
/// before that edit, counted as the answer's `layers` lists them.
pub fn parse_edits(edits: Value) -> Result<Vec<Edit>> {
    let mut edits = edits;
    for edit in edits.as_array_mut().into_iter().flatten() {
        let Some(edit) = edit.as_object_mut() else {
            continue;
        };
        let fields = edit.iter_mut().flat_map(|(key, value)| {
            if key == "layers" {
                match value {
                    Value::Array(items) => items.iter_mut().collect(),
                    _ => Vec::new(),
                }
            } else if LAYER_KEYS.contains(&key.as_str()) {
                vec![value]
            } else {
                Vec::new()
            }
        });
        for field in fields {
            let Some(text) = field.as_str().and_then(|t| t.strip_prefix('$')) else {
                continue;
            };
            let n = text.parse::<u32>().ok().filter(|n| *n >= 1).with_context(|| {
                format!(
                    "`${text}` is not a layer reference: `$1` is the first layer this request adds"
                )
            })?;
            *field = json!(reference(n));
        }
    }
    Ok(serde_json::from_value(edits)?)
}

/// The layer ids an edit names, by reference (`as_ref`, `iter`) or mutably
/// (`as_mut`, `iter_mut`).
macro_rules! layer_ids {
    ($edit:expr, $option:ident, $list:ident) => {
        match $edit {
            Edit::Set { layer, clip_to, .. } => std::iter::once(layer)
                .chain(clip_to.$option().and_then(|base| base.$option()))
                .collect(),
            Edit::ReplacePixels { layer, .. }
            | Edit::RemoveLayer { layer }
            | Edit::SetMask { layer, .. }
            | Edit::Select { layer }
            | Edit::UngroupLayers { layer }
            | Edit::Transform { layer, .. } => vec![layer],
            Edit::SelectLayers { layers }
            | Edit::MergeLayers { layers }
            | Edit::GroupLayers { layers } => layers.$list().collect(),
            Edit::MoveLayer {
                layer,
                above,
                below,
                parent,
            } => std::iter::once(layer)
                .chain(above.$option())
                .chain(below.$option())
                .chain(parent.$option())
                .collect(),
            Edit::Fill { layer, .. }
            | Edit::Gradient { layer, .. }
            | Edit::Stroke { layer, .. }
            | Edit::ApplyFilter { layer, .. }
            | Edit::ApplyAdjustment { layer, .. } => layer.$option().into_iter().collect(),
            Edit::AddLayer { above, .. }
            | Edit::AddAdjustmentLayer { above, .. }
            | Edit::AddEmptyLayer { above, .. }
            | Edit::AddMaskLayer { above, .. }
            | Edit::AddTextLayer { above, .. }
            | Edit::AddShapeLayer { above, .. } => above.$option().into_iter().collect(),
            Edit::SetSelection { .. }
            | Edit::ExtendCanvas { .. }
            | Edit::SelectRect { .. }
            | Edit::SelectPolygon { .. }
            | Edit::SelectColor { .. }
            | Edit::SelectColorRange { .. }
            | Edit::GrowSelection { .. }
            | Edit::FeatherSelection { .. }
            | Edit::Crop { .. }
            | Edit::ResizeCanvas { .. }
            | Edit::ResizeImage { .. } => Vec::new(),
        }
    };
}

impl Edit {
    /// Whether the edit names a layer by a `"$n"` reference.
    fn has_references(&self) -> bool {
        let ids: Vec<&Uuid> = layer_ids!(self, as_ref, iter);
        ids.into_iter().any(|id| referenced(id).is_some())
    }

    /// The edit with its `"$n"` references replaced by the layers they name.
    fn resolved(&self, added: &[Uuid]) -> Result<Self> {
        let mut edit = self.clone();
        let ids: Vec<&mut Uuid> = layer_ids!(&mut edit, as_mut, iter_mut);
        for id in ids {
            if let Some(n) = referenced(id) {
                *id = *added.get(n as usize - 1).with_context(|| {
                    format!(
                        "`${n}` names layer {n} of the ones this request added, but it has added {} so far",
                        added.len()
                    )
                })?;
            }
        }
        Ok(edit)
    }
}

/// Apply a batch, reading its images through `reader`. The caller clones the
/// document first and only keeps the result when every edit succeeded, so a
/// failing batch changes nothing. When a batch of several edits fails, the
/// error says which edit, as `Edit 3 (stroke): …`.
pub fn apply(document: &mut Document, edits: &[Edit], reader: &mut Reader) -> Result<Vec<Uuid>> {
    let mut at = None;
    apply_each(document, edits, reader, &mut at).map_err(|error| match at {
        Some(index) if edits.len() > 1 => {
            anyhow::anyhow!("Edit {} ({}): {error:#}", index + 1, edits[index].op())
        }
        _ => error,
    })
}

/// [`apply`], noting in `at` which edit runs.
fn apply_each(
    document: &mut Document,
    edits: &[Edit],
    reader: &mut Reader,
    at: &mut Option<usize>,
) -> Result<Vec<Uuid>> {
    reader.add_edits(edits.len())?;
    reader.charge(cost(document, edits))?;
    let mut added = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        *at = Some(index);
        let resolved;
        let edit = if edit.has_references() {
            resolved = edit.resolved(&added)?;
            &resolved
        } else {
            edit
        };
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
                clip_to,
            } => {
                if let Some(base) = clip_to {
                    let base = clipping_base(document, *layer, *base)?;
                    find_mut(document, *layer)?.clip_to = base;
                    ensure!(
                        !document.has_clipping_cycle(),
                        "The clipping base's shape depends on this layer"
                    );
                }
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
            Edit::SelectLayers { layers } => select_layers(document, layers)?,
            Edit::MergeLayers { layers } => {
                select_layers(document, layers)?;
                ensure!(
                    !document
                        .layers
                        .iter()
                        .any(|l| document.selected.contains(&l.id) && l.locked),
                    "Unlock the layers to merge them"
                );
                let before = document.layers.len();
                crate::operations::merge_selected(document, true)?;
                ensure!(
                    document.layers.len() < before,
                    "There is nothing below the layer to merge it with"
                );
                added.extend(document.active);
            }
            Edit::GroupLayers { layers } => {
                select_layers(document, layers)?;
                let before = document.layers.len();
                crate::operations::group(document);
                ensure!(
                    document.layers.len() > before,
                    "These layers cannot be grouped"
                );
                reader.add_layer()?;
                added.extend(document.active);
            }
            Edit::UngroupLayers { layer } => {
                ensure!(find(document, *layer)?.group, "The layer is not a group");
                document.select(*layer, false);
                crate::operations::ungroup(document);
            }
            Edit::MoveLayer {
                layer,
                above,
                below,
                parent,
            } => {
                ensure!(
                    above.is_none() || below.is_none(),
                    "Give either above or below, not both"
                );
                find(document, *layer)?;
                let family = document.descendants(*layer);
                let (target, at) = match (above, below, parent) {
                    (Some(target), _, _) => (*target, crate::operations::Placement::Above),
                    (_, Some(target), _) => (*target, crate::operations::Placement::Below),
                    (None, None, Some(group)) => {
                        ensure!(find(document, *group)?.group, "The parent is not a group");
                        (*group, crate::operations::Placement::Inside)
                    }
                    (None, None, None) => bail!("Give above, below or parent"),
                };
                let destination = find(document, target)?;
                ensure!(
                    !family.contains(&target),
                    "A layer cannot be moved into or next to itself"
                );
                if at != crate::operations::Placement::Inside
                    && let Some(parent) = parent
                {
                    ensure!(
                        destination.parent == Some(*parent),
                        "The parent must be the group of the layer it is moved next to"
                    );
                }
                crate::operations::move_layer(document, *layer, target, at);
                document.select(*layer, false);
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
            Edit::Gradient {
                layer,
                start,
                end,
                stops,
                radial,
                opacity,
                mask,
            } => {
                let ends = valid_points(&[*start, *end])?;
                ensure!(
                    start != end,
                    "A gradient's start and end must be different points"
                );
                ensure!(
                    (2..=MAX_GRADIENT_STOPS).contains(&stops.len()),
                    "A gradient needs between 2 and {MAX_GRADIENT_STOPS} colour stops"
                );
                ensure!(
                    stops
                        .iter()
                        .all(|stop| stop.position.is_finite()
                            && (0.0..=1.0).contains(&stop.position)),
                    "Colour stop positions must be between 0 and 1"
                );
                valid_opacity(*opacity)?;
                let mut stops: Vec<(f32, [u8; 4])> = stops
                    .iter()
                    .map(|stop| (stop.position, stop.color.0))
                    .collect();
                stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                activate(document, *layer)?;
                crate::paint::gradient_stops(
                    document,
                    ends[0],
                    ends[1],
                    &stops,
                    crate::paint::GradientShape {
                        radial: *radial,
                        opacity: *opacity,
                        mask_target: *mask,
                    },
                )?;
            }
            Edit::Stroke {
                layer,
                points,
                color,
                size,
                hardness,
                opacity,
                erase,
                pressure_opacity,
                spacing,
                taper_in,
                taper_out,
                scatter,
                scatter_count,
                size_jitter,
                opacity_jitter,
                hue_jitter,
                seed,
            } => {
                let xy: Vec<[f32; 2]> = points.iter().map(StrokePoint::xy).collect();
                let positions = valid_points(&xy)?;
                ensure!(!positions.is_empty(), "A stroke needs at least one point");
                ensure!(
                    points.iter().all(|p| p
                        .pressure
                        .is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v))),
                    "A point's pressure must be between 0 and 1"
                );
                ensure!(
                    size.is_finite() && (1.0..=2000.0).contains(size),
                    "The brush size must be between 1 and 2000"
                );
                ensure!(
                    hardness.is_finite() && (0.0..=1.0).contains(hardness),
                    "The hardness must be between 0 and 1"
                );
                valid_opacity(*opacity)?;
                let dynamics = crate::paint::Dynamics {
                    spacing: *spacing,
                    taper_in: *taper_in,
                    taper_out: *taper_out,
                    taper_size: true,
                    taper_opacity: *pressure_opacity,
                    scatter: *scatter,
                    count: *scatter_count,
                    size_jitter: *size_jitter,
                    opacity_jitter: *opacity_jitter,
                    hue_jitter: *hue_jitter,
                    seed: *seed,
                };
                dynamics.validate().map_err(anyhow::Error::msg)?;
                activate(document, *layer)?;
                let brush = crate::paint::Brush {
                    diameter: *size,
                    hardness: *hardness,
                    opacity: *opacity,
                    color: color.0,
                    dynamics,
                    ..crate::paint::Brush::default()
                };
                let mode = if *erase {
                    crate::paint::PaintMode::Erase
                } else {
                    crate::paint::PaintMode::Paint
                };
                // Pressure goes through the pen's path: size, and opacity when asked.
                let samples: Vec<(Point, crate::paint::Brush)> = positions
                    .into_iter()
                    .zip(points)
                    .map(|(position, point)| {
                        let mut brush = brush.clone();
                        if let Some(pressure) = point.pressure {
                            brush.diameter *= pressure.max(0.01);
                            if *pressure_opacity {
                                brush.opacity *= pressure;
                            }
                        }
                        (position, brush)
                    })
                    .collect();
                crate::paint::Stroke::default().path(
                    document,
                    &samples,
                    crate::paint::StrokeOptions {
                        mode,
                        mask_target: false,
                        source: None,
                        clone_offset: Point::default(),
                    },
                )?;
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
    // Grouping and moving release clipping that would depend on itself, as in the app.
    document.release_clipping_cycles();
    document.validate()?;
    Ok(added)
}

/// Select these layers, the last one active.
pub fn select_layers(document: &mut Document, layers: &[Uuid]) -> Result<()> {
    ensure!(!layers.is_empty(), "List at least one layer");
    ensure!(layers.len() <= 1000, "Too many layers");
    for id in layers {
        find(document, *id)?;
    }
    document.selected = layers.iter().copied().collect();
    document.active = layers.last().copied();
    Ok(())
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

/// The base `layer` clips to when asked to clip to `base`, following **Layer → Clipping
/// Mask**: a layer or folder below it in the same parent, not a mask, adjustment or filter
/// layer. A base that is itself clipped passes on its own base. Folders, mask layers and
/// filter layers cannot be clipped.
fn clipping_base(document: &Document, layer: Uuid, base: Option<Uuid>) -> Result<Option<Uuid>> {
    let clipped = find(document, layer)?;
    let Some(base) = base else {
        return Ok(None);
    };
    ensure!(
        !clipped.group && !clipped.standalone_mask && clipped.filter.is_none(),
        "Layer {} cannot be clipped: folders, mask layers and filter layers do not clip",
        clipped.name
    );
    let target = find(document, base)?;
    ensure!(base != layer, "A layer cannot clip to itself");
    ensure!(
        target.parent == clipped.parent,
        "The clipping base {} must be in the same folder as {}",
        target.name,
        clipped.name
    );
    let position = |id| document.layers.iter().position(|l| l.id == id);
    ensure!(
        position(base) < position(layer),
        "The clipping base {} must be below {}",
        target.name,
        clipped.name
    );
    ensure!(
        !target.is_effect(),
        "{} cannot be a clipping base: mask, adjustment and filter layers have no pixels to clip to",
        target.name
    );
    Ok(Some(target.clip_to.unwrap_or(base)))
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

    fn described(document: &Document, id: Uuid) -> Value {
        describe(document)["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|layer| layer["id"] == json!(id))
            .cloned()
            .unwrap()
    }

    #[test]
    fn describes_shapes_and_flips() {
        let mut document = Document::new(40, 30).unwrap();
        let added = run(
            &mut document,
            &[edit(json!({
                "op": "add_shape_layer", "shape": "RoundedRectangle", "x": 2, "y": 2,
                "width": 20, "height": 10, "color": "#ff000080", "corner_radius": 3,
            }))],
        )
        .unwrap();
        let shape = described(&document, added[0]);
        assert_eq!(shape["kind"], "shape");
        assert_eq!(
            shape["shape"],
            json!({"shape": "RoundedRectangle", "color": "#ff000080", "corner_radius": 3.0})
        );
        assert_eq!(
            (&shape["flip_x"], &shape["flip_y"]),
            (&json!(false), &json!(false))
        );
        let base = document.layers[0].id;
        assert_eq!(described(&document, base)["kind"], "image");
        assert!(described(&document, base)["shape"].is_null());
        document.layers[0].transform.flip_x = true;
        let flipped = described(&document, base);
        assert_eq!(
            (&flipped["flip_x"], &flipped["flip_y"]),
            (&json!(true), &json!(false))
        );
    }

    #[test]
    fn attached_masks_are_reported_and_exported_through_their_image() {
        let dir = tempfile::tempdir().unwrap();
        let mut document = Document::new(40, 30).unwrap();
        let image = document.layers[0].id;
        document.layers[0].pixels = Some(Arc::new(RgbaImage::new(40, 30)));
        let no_mask = export_layer(&document, image, What::Mask, None, dir.path(), "n.png");
        assert!(
            no_mask.unwrap_err().to_string().contains("`mask` command"),
            "the error says how to add one"
        );
        // As the `mask` command attaches one: a child mask layer.
        let mut mask = Layer::mask("Mask", 40, 30);
        mask.parent = Some(image);
        mask.mask.as_mut().unwrap().linked = false;
        let mask_id = mask.id;
        document.layers.push(mask);
        let value = described(&document, image);
        assert_eq!(value["has_mask"], true);
        assert_eq!(
            value["masks"],
            json!([{"layer": mask_id, "enabled": true, "linked": false}])
        );
        assert!(value["attached_to"].is_null());
        let value = described(&document, mask_id);
        assert_eq!(value["kind"], "mask");
        assert_eq!(value["attached_to"], json!(image));
        assert_eq!(value["masks"][0]["layer"], json!(mask_id));
        // The image's mask is read from the mask layer, which is named.
        let export = export_layer(&document, image, What::Mask, None, dir.path(), "m.png").unwrap();
        assert_eq!(export.mask_layer, Some(mask_id));
        assert_eq!(read_gray_png(&export.path).unwrap().dimensions(), (1, 1));
        let own = export_layer(&document, mask_id, What::Mask, None, dir.path(), "o.png").unwrap();
        assert_eq!(own.mask_layer, None);
        // With two, the client is told which to ask for.
        let mut second = Layer::mask("Mask 2", 40, 30);
        second.parent = Some(image);
        let second_id = second.id;
        document.layers.push(second);
        let error = export_layer(&document, image, What::Mask, None, dir.path(), "t.png")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&mask_id.to_string()) && error.contains(&second_id.to_string()),
            "{error}"
        );
        assert_eq!(
            described(&document, image)["masks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // A mask outside any image's stack is not attached.
        let loose = Layer::mask("Loose", 40, 30);
        let loose_id = loose.id;
        document.layers.push(loose);
        assert!(described(&document, loose_id)["attached_to"].is_null());
    }

    #[test]
    fn out_of_range_settings_name_the_field_and_its_range() {
        for (value, expected) in [
            (
                json!({"HueSaturation": {"hue": 0, "saturation": 150, "lightness": 0, "colorize": false}}),
                "`HueSaturation.saturation` must be between -100 and 100, not 150",
            ),
            (
                json!({"Grain": {"amount": 101, "monochrome": true, "seed": 1}}),
                "`Grain.amount` must be between 0 and 100",
            ),
            (
                json!({"Levels": {"black": 200, "gamma": 1, "white": 100, "output_black": 0, "output_white": 255}}),
                "must be above `black`",
            ),
            (
                json!({"Curves": {"points": [{"x": 0, "y": 0}, {"x": 1, "y": 2}]}}),
                "`Curves.points y` must be between 0 and 1",
            ),
            (
                json!({"FilmGrain": {"amount": 10, "size": 0, "roughness": 50, "seed": 1}}),
                "`FilmGrain.size` must be between 0.1 and 100",
            ),
        ] {
            let mut document = Document::new(8, 8).unwrap();
            document.layers[0].pixels = Some(Arc::new(RgbaImage::new(8, 8)));
            let edits = [edit(
                json!({"op": "apply_adjustment", "layer": document.layers[0].id, "adjustment": value}),
            )];
            let error = format!("{:#}", run(&mut document, &edits).unwrap_err());
            assert!(error.contains(expected), "{error}");
        }
        let filter = edit(
            json!({"op": "add_adjustment_layer", "filter": {"GaussianBlur": {"radius": 500}}}),
        );
        let error = run(&mut Document::new(8, 8).unwrap(), &[filter]).unwrap_err();
        assert!(
            format!("{error:#}").contains("`GaussianBlur.radius` must be between 0 and 100"),
            "{error:#}"
        );
    }

    /// The MCP server's tool descriptions list every adjustment and filter
    /// Xuan accepts. Serde's error for an unknown variant lists them all, so
    /// the list comes from the code and a new variant fails this test until
    /// it is described.
    #[test]
    fn the_mcp_tools_describe_every_adjustment_and_filter() {
        fn variants<T: serde::de::DeserializeOwned + std::fmt::Debug>() -> Vec<String> {
            let error = serde_json::from_value::<T>(json!({"NoSuchVariant": {}}))
                .unwrap_err()
                .to_string();
            let listed = error
                .split("expected one of")
                .nth(1)
                .expect("variants listed");
            listed
                .split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect()
        }
        let tools = include_str!("../../plugins/mcp-server/src/tools.rs");
        let constant = |name: &str| {
            let line = tools
                .lines()
                .find(|line| line.starts_with(&format!("const {name}: &str = ")))
                .unwrap_or_else(|| panic!("tools.rs has no {name}"));
            line.replace("\\\"", "\"")
        };
        for (described, names) in [
            (constant("ADJUSTMENTS"), variants::<Adjustment>()),
            (constant("FILTERS"), variants::<Filter>()),
        ] {
            assert!(names.len() >= 4, "{names:?}");
            for name in names {
                assert!(
                    described.contains(&format!("\"{name}\"")),
                    "plugins/mcp-server/src/tools.rs does not describe {name}"
                );
            }
        }
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
                    clip_to: None,
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
                    blend: None,
                    clip_to: None,
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
    fn a_one_point_stroke_paints_one_round_dab() {
        let (mut document, _) = grey_document();
        let edits = parse_edits(json!([
            {"op": "add_empty_layer", "name": "Stars"},
            {"op": "stroke", "layer": "$1", "points": [[10, 10]], "size": 8, "hardness": 1, "color": "#ff0000"},
            {"op": "stroke", "layer": "$1", "points": [[30, 20]], "size": 2, "hardness": 1, "color": "#00ff00"},
        ]))
        .unwrap();
        let added = run(&mut document, &edits).unwrap();
        let stars = document.layers.iter().find(|l| l.id == added[0]).unwrap();
        let pixels = stars.pixels.clone().unwrap();
        // A disc of diameter 8 around (10, 10): its centre and just inside
        // its edge are painted, outside it nothing is.
        assert_eq!(pixels.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(12, 10).0[3], 255);
        assert_eq!(pixels.get_pixel(15, 10).0[3], 0);
        assert_eq!(pixels.get_pixel(10, 15).0[3], 0);
        assert_eq!(pixels.get_pixel(13, 13).0[3], 0, "round, not square");
        // The second dab is separate: nothing is painted between them.
        assert_eq!(pixels.get_pixel(30, 20).0[1], 255);
        assert_eq!(pixels.get_pixel(20, 15).0[3], 0);
        let painted = pixels.pixels().filter(|p| p.0[3] > 0).count();
        assert!((40..=80).contains(&painted), "{painted} pixels painted");
    }

    #[test]
    fn set_clips_to_a_layer_or_folder_below_and_null_releases() {
        let mut document = Document::new(8, 8).unwrap();
        let mut group = Layer::blank("Figure", 8, 8);
        group.group = true;
        let mut inside = Layer::image("Cloak", RgbaImage::new(8, 8));
        inside.parent = Some(group.id);
        let base = Layer::image("Base", RgbaImage::new(8, 8));
        let mut clipped = Layer::image("Clipped", RgbaImage::new(8, 8));
        clipped.clip_to = Some(base.id);
        let shading = Layer::image("Shading", RgbaImage::new(8, 8));
        let mut invert = Layer::blank("Invert", 8, 8);
        invert.adjustment = Some(Adjustment::Invert);
        let [
            inside_id,
            group_id,
            base_id,
            clipped_id,
            shading_id,
            invert_id,
        ] = [&inside, &group, &base, &clipped, &shading, &invert].map(|l| l.id);
        // Bottom to top: the folder, a base with a clipped layer, the shading layer and an
        // adjustment.
        document.layers = vec![inside, group, base, clipped, shading, invert];
        document.select(shading_id, false);
        document.validate().unwrap();
        let set = |document: &mut Document, layer: Uuid, clip_to: Value| {
            let edits =
                parse_edits(json!([{"op": "set", "layer": layer, "clip_to": clip_to}])).unwrap();
            run(document, &edits).map_err(|e| format!("{e:#}"))
        };
        let clip_of = |document: &Document, id: Uuid| {
            document.layers.iter().find(|l| l.id == id).unwrap().clip_to
        };

        // A folder below in the same parent is a base.
        set(&mut document, shading_id, json!(group_id)).unwrap();
        assert_eq!(clip_of(&document, shading_id), Some(group_id));
        // Leaving `clip_to` out keeps it; `null` releases it.
        let edits =
            parse_edits(json!([{"op": "set", "layer": shading_id, "opacity": 0.5}])).unwrap();
        run(&mut document, &edits).unwrap();
        assert_eq!(clip_of(&document, shading_id), Some(group_id));
        set(&mut document, shading_id, Value::Null).unwrap();
        assert_eq!(clip_of(&document, shading_id), None);
        // A clipped layer passes on its base, as Layer → Clipping Mask does.
        set(&mut document, shading_id, json!(clipped_id)).unwrap();
        assert_eq!(clip_of(&document, shading_id), Some(base_id));
        // Adjustments clip too.
        set(&mut document, invert_id, json!(group_id)).unwrap();
        assert_eq!(clip_of(&document, invert_id), Some(group_id));

        let before = document.clone();
        for (layer, base, message) in [
            (base_id, json!(shading_id), "must be below"),
            (shading_id, json!(shading_id), "cannot clip to itself"),
            (shading_id, json!(inside_id), "same folder"),
            (inside_id, json!(group_id), "same folder"),
            (shading_id, json!(invert_id), "must be below"),
            (group_id, json!(base_id), "cannot be clipped"),
            (shading_id, json!(Uuid::new_v4()), "No layer"),
        ] {
            let mut copy = before.clone();
            let error = set(&mut copy, layer, base).unwrap_err();
            assert!(
                error.to_lowercase().contains(&message.to_lowercase()),
                "{message}: {error}"
            );
        }
        // An adjustment is no base.
        document.layers.swap(4, 5);
        let error = set(&mut document, shading_id, json!(invert_id)).unwrap_err();
        assert!(error.contains("cannot be a clipping base"), "{error}");

        // A layer the same request adds can be the base, as `$n`.
        let edits = parse_edits(json!([
            {"op": "add_empty_layer", "name": "New base", "above": invert_id},
            {"op": "add_empty_layer", "name": "On top"},
            {"op": "set", "layer": "$2", "clip_to": "$1"},
        ]))
        .unwrap();
        let added = run(&mut document, &edits).unwrap();
        assert_eq!(clip_of(&document, added[1]), Some(added[0]));
    }

    #[test]
    fn references_name_the_layers_added_before_and_say_which_edit_failed() {
        let (document, base) = grey_document();
        let edits = parse_edits(json!([
            {"op": "add_empty_layer", "name": "A"},
            {"op": "add_text_layer", "text": "B", "above": "$1"},
            {"op": "merge_layers", "layers": ["$2"]},
            {"op": "set", "layer": "$3", "name": "Merged"},
            {"op": "move_layer", "layer": "$3", "below": base},
        ]))
        .unwrap();
        let mut copy = document.clone();
        let added = run(&mut copy, &edits).unwrap();
        assert_eq!(added.len(), 3);
        assert_eq!(copy.layers.len(), 2, "B merged into A");
        assert_eq!(copy.layers[0].id, added[2]);
        assert_eq!(copy.layers[0].name, "Merged");

        // A reference past the layers added so far, before any, or not a
        // number at all is refused, naming the edit.
        let ahead = parse_edits(json!([
            {"op": "add_empty_layer"},
            {"op": "fill", "layer": "$2", "color": "#000000"},
        ]))
        .unwrap();
        let error = run(&mut document.clone(), &ahead).unwrap_err().to_string();
        assert!(error.starts_with("Edit 2 (fill): `$2`"), "{error}");
        let first = parse_edits(json!([{"op": "set", "layer": "$1", "visible": false}])).unwrap();
        let error = run(&mut document.clone(), &first).unwrap_err().to_string();
        assert!(error.contains("added 0 so far"), "{error}");
        for bad in ["$0", "$", "$one", "$-1"] {
            let error = parse_edits(json!([{"op": "select", "layer": bad}])).unwrap_err();
            assert!(
                format!("{error:#}").contains("not a layer reference"),
                "{bad}: {error:#}"
            );
        }
        // Text is never taken for a reference; a single failing edit is
        // reported as before, without an edit number.
        let text = parse_edits(json!([{"op": "add_text_layer", "text": "$1"}])).unwrap();
        assert!(matches!(&text[0], Edit::AddTextLayer { text, .. } if text == "$1"));
        let lone = [edit(
            json!({"op": "fill", "layer": Uuid::new_v4(), "color": "#000000"}),
        )];
        let error = run(&mut document.clone(), &lone).unwrap_err().to_string();
        assert!(!error.contains("Edit 1"), "{error}");
    }

    fn near(actual: [u8; 4], expected: [u8; 4], tolerance: u8) {
        for i in 0..4 {
            assert!(
                actual[i].abs_diff(expected[i]) <= tolerance,
                "{actual:?} is not near {expected:?}"
            );
        }
    }

    fn gradient(extra: Value) -> Edit {
        let mut op = json!({
            "op": "gradient", "start": [0, 0], "end": [40, 0],
            "stops": [
                {"position": 0, "color": "#000000"},
                {"position": 1, "color": "#ffffff"},
            ],
        });
        op.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        edit(op)
    }

    #[test]
    fn a_two_stop_linear_gradient_runs_from_start_to_end() {
        let (mut document, base) = grey_document();
        run(&mut document, &[gradient(json!({"layer": base}))]).unwrap();
        let pixels = document.layers[0].pixels.clone().unwrap();
        near(pixels.get_pixel(0, 10).0, [0, 0, 0, 255], 4);
        near(pixels.get_pixel(20, 10).0, [131, 131, 131, 255], 4);
        near(pixels.get_pixel(39, 10).0, [250, 250, 250, 255], 4);
        // Constant down each column.
        assert_eq!(pixels.get_pixel(20, 0), pixels.get_pixel(20, 29));

        // Opacity blends with what is there.
        let (mut document, _) = grey_document();
        run(&mut document, &[gradient(json!({"opacity": 0.5}))]).unwrap();
        let pixels = document.layers[0].pixels.clone().unwrap();
        near(pixels.get_pixel(0, 10).0, [50, 50, 50, 255], 4);
    }

    #[test]
    fn a_multi_stop_gradient_passes_through_each_stop_and_ignores_input_order() {
        let (mut document, _) = grey_document();
        let stops = json!([
            {"position": 1, "color": "#0000ff"},
            {"position": 0, "color": "#ff0000"},
            {"position": 0.5, "color": "#00ff00"},
        ]);
        run(&mut document, &[gradient(json!({"stops": stops}))]).unwrap();
        let pixels = document.layers[0].pixels.clone().unwrap();
        near(pixels.get_pixel(0, 5).0, [250, 5, 0, 255], 8);
        near(pixels.get_pixel(20, 5).0, [5, 250, 5, 255], 12);
        near(pixels.get_pixel(39, 5).0, [0, 5, 250, 255], 8);
        // Halfway between the first two stops.
        near(pixels.get_pixel(10, 5).0, [128, 128, 0, 255], 8);
    }

    #[test]
    fn a_radial_gradient_spreads_from_the_start_to_the_end_radius() {
        let (mut document, _) = grey_document();
        let radial = gradient(json!({"start": [20, 15], "end": [30, 15], "radial": true}));
        run(&mut document, &[radial]).unwrap();
        let pixels = document.layers[0].pixels.clone().unwrap();
        near(pixels.get_pixel(20, 15).0, [18, 18, 18, 255], 6);
        near(pixels.get_pixel(25, 15).0, [140, 140, 140, 255], 6);
        near(pixels.get_pixel(20, 25).0, [255, 255, 255, 255], 2);
        assert_eq!(pixels.get_pixel(35, 15).0, [255, 255, 255, 255]);
    }

    #[test]
    fn a_gradient_stays_inside_the_selection() {
        let (mut document, _) = grey_document();
        run(
            &mut document,
            &[
                edit(json!({"op": "select_rect", "x": 0, "y": 0, "width": 20, "height": 30})),
                gradient(json!({})),
            ],
        )
        .unwrap();
        let pixels = document.layers[0].pixels.clone().unwrap();
        near(pixels.get_pixel(10, 5).0, [66, 66, 66, 255], 4);
        assert_eq!(pixels.get_pixel(30, 5).0, [100, 100, 100, 255]);
    }

    #[test]
    fn a_gradient_can_paint_the_mask_and_leaves_the_pixels() {
        let (mut document, _) = grey_document();
        let before = document.layers[0].pixels.clone().unwrap();
        run(&mut document, &[gradient(json!({"mask": true}))]).unwrap();
        let layer = &document.layers[0];
        assert_eq!(layer.pixels.as_ref().unwrap(), &before);
        let mask = &layer.mask.as_ref().unwrap().pixels;
        assert_eq!(mask.dimensions(), (40, 30));
        assert!(mask.get_pixel(0, 5)[0] <= 4);
        assert!(mask.get_pixel(20, 5)[0].abs_diff(131) <= 4);
        assert!(mask.get_pixel(39, 5)[0] >= 250);
    }

    #[test]
    fn invalid_gradients_are_refused_and_locked_layers_are_left_alone() {
        let (mut document, _) = grey_document();
        let one = json!([{"position": 0, "color": "#000000"}]);
        let many: Vec<Value> = (0..65)
            .map(|i| json!({"position": i as f32 / 64.0, "color": "#000000"}))
            .collect();
        let bad = [
            json!({"stops": []}),
            json!({"stops": one}),
            json!({"stops": many}),
            json!({"stops": [
                {"position": -0.1, "color": "#000000"}, {"position": 1, "color": "#ffffff"}]}),
            json!({"stops": [
                {"position": 0, "color": "#000000"}, {"position": 1.5, "color": "#ffffff"}]}),
            json!({"opacity": 2}),
            json!({"opacity": -1}),
            json!({"start": [5, 5], "end": [5, 5]}),
            json!({"end": [1.0e12, 0]}),
            json!({"layer": Uuid::new_v4()}),
        ];
        for extra in bad {
            let mut op = json!({
                "op": "gradient", "start": [0, 0], "end": [40, 0],
                "stops": [
                    {"position": 0, "color": "#000000"},
                    {"position": 1, "color": "#ffffff"},
                ],
            });
            op.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let parsed = serde_json::from_value::<Edit>(op);
            let mut scratch = document.clone();
            assert!(
                parsed.is_err() || run(&mut scratch, &[parsed.unwrap()]).is_err(),
                "{extra}"
            );
        }
        // A colour that isn't #rrggbb is refused when parsing.
        assert!(
            serde_json::from_value::<Edit>(json!({
                "op": "gradient", "start": [0, 0], "end": [1, 0],
                "stops": [{"position": 0, "color": "red"}, {"position": 1, "color": "#fff"}],
            }))
            .is_err()
        );
        document.layers[0].locked = true;
        let error = run(&mut document, &[gradient(json!({}))]).unwrap_err();
        assert!(error.to_string().contains("lock"), "{error}");
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
    fn heavy_batches_are_refused_before_any_work() {
        let (mut document, base) = grey_document();
        let before = document.clone();
        // A zig-zag of huge dabs across the canvas.
        let points: Vec<[f32; 2]> = (0..MAX_POINTS)
            .map(|i| {
                if i % 2 == 0 {
                    [0.0, 0.0]
                } else {
                    [29_000.0, 29_000.0]
                }
            })
            .collect();
        let bomb = edit(json!({
            "op": "stroke", "layer": base, "points": points, "size": 2000, "hardness": 1,
        }));
        let started = std::time::Instant::now();
        let error = run(&mut document, &[bomb]).unwrap_err().to_string();
        assert!(
            error.contains("too long") || error.contains("longer"),
            "{error}"
        );
        // Canvas-size masks after growing the canvas to 100 megapixels.
        let mut masks = vec![edit(
            json!({"op": "resize_canvas", "width": 10_000, "height": 10_000}),
        )];
        masks.extend((0..MAX_LAYERS).map(|_| edit(json!({"op": "add_mask_layer"}))));
        let error = run(&mut document, &masks).unwrap_err().to_string();
        assert!(
            error.contains("MiB") || error.contains("too long"),
            "{error}"
        );
        // Colour selections and grown selections on a large canvas.
        let mut renders = vec![edit(
            json!({"op": "resize_canvas", "width": 10_000, "height": 10_000}),
        )];
        renders.extend((0..20).map(|_| edit(json!({"op": "select_color", "x": 1, "y": 1}))));
        assert!(run(&mut document, &renders).is_err());
        let grow = [
            edit(json!({"op": "resize_canvas", "width": 10_000, "height": 10_000})),
            edit(json!({"op": "grow_selection", "by": 256})),
        ];
        assert!(run(&mut document, &grow).is_err());
        let blur = [edit(
            json!({"op": "apply_filter", "filter": {"MotionBlur": {"distance": 200, "angle": 0}}}),
        )];
        let mut wide = document.clone();
        wide.layers[0].pixels = Some(Arc::new(RgbaImage::new(20_000, 5_000)));
        assert!(run(&mut wide, &blur).is_err());
        // Nothing was done, and quickly.
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(document.width, before.width);
        assert_eq!(document.layers.len(), before.layers.len());
        // One reader shares the budget across the batches of a result.
        let mut reader = Reader::new(Access::anywhere(), MAX_LAYERS);
        let big = [edit(
            json!({"op": "resize_canvas", "width": 10_000, "height": 10_000}),
        )];
        let mut huge = document.clone();
        apply(&mut huge, &big, &mut reader).unwrap();
        let masks: Vec<Edit> = (0..6)
            .map(|_| edit(json!({"op": "add_mask_layer"})))
            .collect();
        assert!(apply(&mut huge.clone(), &masks, &mut reader).is_err());
        // Ordinary edits fit easily.
        let stroke = edit(json!({"op": "stroke", "points": [[1, 1], [30, 20]], "size": 40}));
        let fine = vec![stroke; 50];
        run(&mut document, &fine).unwrap();
        assert!(cost(&document, &fine).work < MAX_WORK / 100);
    }

    #[test]
    fn move_layer_places_layers_and_rejects_bad_targets() {
        let (mut document, base) = grey_document();
        let add = |document: &mut Document| {
            run(document, &[edit(json!({"op": "add_empty_layer"}))]).unwrap()[0]
        };
        let (a, b, c) = (add(&mut document), add(&mut document), add(&mut document));
        let order =
            |document: &Document| -> Vec<Uuid> { document.layers.iter().map(|l| l.id).collect() };
        let mv = |document: &mut Document, value: serde_json::Value| run(document, &[edit(value)]);
        assert_eq!(order(&document), [base, a, b, c]);
        // Above and below.
        mv(
            &mut document,
            json!({"op": "move_layer", "layer": base, "above": c}),
        )
        .unwrap();
        assert_eq!(order(&document), [a, b, c, base]);
        mv(
            &mut document,
            json!({"op": "move_layer", "layer": base, "below": b}),
        )
        .unwrap();
        assert_eq!(order(&document), [a, base, b, c]);
        assert_eq!(document.active, Some(base));
        // Into a group, then back out next to a root layer.
        let group = run(
            &mut document,
            &[edit(json!({"op": "group_layers", "layers": [b, c]}))],
        )
        .unwrap()[0];
        mv(
            &mut document,
            json!({"op": "move_layer", "layer": a, "parent": group}),
        )
        .unwrap();
        let parent = |document: &Document, id: Uuid| {
            document.layers.iter().find(|l| l.id == id).unwrap().parent
        };
        assert_eq!(parent(&document, a), Some(group));
        let index = |document: &Document, id: Uuid| order(document).iter().position(|l| *l == id);
        assert!(
            index(&document, a) > index(&document, c),
            "above the group's top layer"
        );
        mv(
            &mut document,
            json!({"op": "move_layer", "layer": a, "above": b, "parent": group}),
        )
        .unwrap();
        assert_eq!(parent(&document, a), Some(group));
        mv(
            &mut document,
            json!({"op": "move_layer", "layer": a, "below": base}),
        )
        .unwrap();
        assert_eq!(parent(&document, a), None);
        for bad in [
            json!({"op": "move_layer", "layer": Uuid::new_v4(), "above": base}),
            json!({"op": "move_layer", "layer": base, "above": Uuid::new_v4()}),
            json!({"op": "move_layer", "layer": base, "parent": Uuid::new_v4()}),
            json!({"op": "move_layer", "layer": base}),
            json!({"op": "move_layer", "layer": base, "above": a, "below": a}),
            // Not a group.
            json!({"op": "move_layer", "layer": a, "parent": base}),
            // A group into itself or its own child, or next to itself.
            json!({"op": "move_layer", "layer": group, "parent": group}),
            json!({"op": "move_layer", "layer": group, "above": c}),
            json!({"op": "move_layer", "layer": base, "above": base}),
            // The parent is not the target's group.
            json!({"op": "move_layer", "layer": a, "above": base, "parent": group}),
        ] {
            assert!(mv(&mut document.clone(), bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn layers_merge_group_and_ungroup_in_one_edit() {
        let (mut document, base) = grey_document();
        let red = run(
            &mut document,
            &[edit(json!({"op": "add_shape_layer", "shape": "Rectangle", "x": 0, "y": 0, "width": 4, "height": 4, "color": "#ff0000"}))],
        )
        .unwrap()[0];
        let empty = run(&mut document, &[edit(json!({"op": "add_empty_layer"}))]).unwrap()[0];
        let group = run(
            &mut document,
            &[edit(json!({"op": "group_layers", "layers": [red, empty]}))],
        )
        .unwrap()[0];
        let layer =
            |document: &Document, id: Uuid| document.layers.iter().find(|l| l.id == id).cloned();
        assert!(layer(&document, group).unwrap().group);
        assert_eq!(layer(&document, red).unwrap().parent, Some(group));
        assert!(
            run(
                &mut document.clone(),
                &[edit(json!({"op": "ungroup_layers", "layer": base}))]
            )
            .is_err()
        );
        run(
            &mut document,
            &[edit(json!({"op": "ungroup_layers", "layer": group}))],
        )
        .unwrap();
        assert!(layer(&document, group).is_none());
        assert_eq!(layer(&document, red).unwrap().parent, None);
        // One layer merges down; the merged layer is reported.
        let merged = run(
            &mut document,
            &[edit(json!({"op": "merge_layers", "layers": [red]}))],
        )
        .unwrap();
        assert_eq!(merged.len(), 1);
        assert!(layer(&document, red).is_none() && layer(&document, base).is_none());
        let pixels = crate::render::render(&document);
        assert_eq!(pixels.get_pixel(1, 1).0, [255, 0, 0, 255]);
        // The bottom layer has nothing to merge into.
        let bottom = document.layers[0].id;
        let error = run(
            &mut document,
            &[edit(json!({"op": "merge_layers", "layers": [bottom]}))],
        )
        .unwrap_err();
        assert!(error.to_string().contains("layer"), "{error}");
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

    /// A clear 200 × 60 document with one pixel layer.
    fn clear_document() -> Document {
        let mut document = Document::new(200, 60).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::new(200, 60)));
        document
    }

    fn painted_rows(document: &Document, x: u32) -> usize {
        let image = crate::render::render(document);
        (0..image.height())
            .filter(|&y| image.get_pixel(x, y)[3] > 0)
            .count()
    }

    #[test]
    fn stroke_point_pressure_tapers_the_width_and_optionally_the_opacity() {
        let mut document = clear_document();
        run(
            &mut document,
            &[edit(json!({
                "op": "stroke", "size": 30, "hardness": 1,
                "points": [[10, 30, 0.1], [100, 30, 1], [190, 30, 0.1]],
            }))],
        )
        .unwrap();
        let (thin, wide) = (painted_rows(&document, 15), painted_rows(&document, 100));
        assert_eq!(wide, 30);
        assert!(thin > 0 && thin < 8, "{thin}");
        assert!(painted_rows(&document, 55) > thin && painted_rows(&document, 55) < wide);
        assert!(painted_rows(&document, 185) < 8);
        // Full strength along the middle: pressure changed only the size.
        let image = crate::render::render(&document);
        assert_eq!(image.get_pixel(15, 30)[3], 255);

        // With `pressure_opacity` the thin ends are also fainter.
        let mut faded = clear_document();
        run(
            &mut faded,
            &[edit(json!({
                "op": "stroke", "size": 30, "hardness": 1, "pressure_opacity": true,
                "points": [[10, 30, 0.1], [100, 30, 1], [190, 30, 0.1]],
            }))],
        )
        .unwrap();
        let image = crate::render::render(&faded);
        assert!(image.get_pixel(15, 30)[3] < 80);
        assert_eq!(image.get_pixel(100, 30)[3], 255);

        // Two-number points keep pressure 1, mixed with three-number ones.
        let mut mixed = clear_document();
        run(
            &mut mixed,
            &[edit(json!({
                "op": "stroke", "size": 30, "hardness": 1,
                "points": [[10, 30], [100, 30, 1], [190, 30]],
            }))],
        )
        .unwrap();
        assert_eq!(painted_rows(&mixed, 15), 30);
    }

    #[test]
    fn stroke_dynamics_are_checked_and_repeat_for_a_seed() {
        let mut document = clear_document();
        for (stroke, says) in [
            (json!({"op": "stroke", "points": [[1, 1, 1.5]]}), "pressure"),
            (
                json!({"op": "stroke", "points": [[1, 1, -0.1]]}),
                "pressure",
            ),
            (
                json!({"op": "stroke", "points": [[1, 1], [2, 2]], "spacing": 11}),
                "spacing",
            ),
            (
                json!({"op": "stroke", "points": [[1, 1]], "scatter_count": 0}),
                "count",
            ),
            (
                json!({"op": "stroke", "points": [[1, 1]], "scatter_count": 17}),
                "count",
            ),
            (
                json!({"op": "stroke", "points": [[1, 1]], "hue_jitter": 2}),
                "hue jitter",
            ),
            (
                json!({"op": "stroke", "points": [[1, 1]], "taper_in": -1}),
                "taper",
            ),
        ] {
            let error = run(&mut document, &[edit(stroke.clone())]).unwrap_err();
            assert!(error.to_string().contains(says), "{stroke}: {error}");
        }
        for points in [json!([[1, 2, 3, 4]]), json!([[1]])] {
            let error = serde_json::from_value::<Edit>(json!({"op": "stroke", "points": points}))
                .unwrap_err();
            assert!(error.to_string().contains("[x, y, pressure]"), "{error}");
        }
        assert_eq!(
            serde_json::to_value(edit(
                json!({"op": "stroke", "points": [[1, 2], [3, 4, 0.5]]})
            ))
            .unwrap()["points"],
            json!([[1.0, 2.0], [3.0, 4.0, 0.5]])
        );
        let stars = |seed: u64| {
            let mut document = clear_document();
            run(
                &mut document,
                &[edit(json!({
                    "op": "stroke", "size": 6, "hardness": 1, "color": "#ffe080",
                    "points": [[10, 30], [190, 30]],
                    "spacing": 2, "scatter": 3, "scatter_count": 2,
                    "size_jitter": 0.6, "opacity_jitter": 0.5, "hue_jitter": 0.3, "seed": seed,
                }))],
            )
            .unwrap();
            crate::render::render(&document)
        };
        assert_eq!(stars(5), stars(5));
        assert_ne!(stars(5), stars(6));
        // Spacing leaves gaps between the dabs on the path.
        let mut dotted = clear_document();
        run(
            &mut dotted,
            &[edit(json!({
                "op": "stroke", "size": 6, "hardness": 1, "points": [[10, 30], [190, 30]],
                "spacing": 3,
            }))],
        )
        .unwrap();
        let image = crate::render::render(&dotted);
        assert_eq!(image.get_pixel(10, 30)[3], 255);
        assert_eq!(image.get_pixel(28, 30)[3], 255);
        assert_eq!(image.get_pixel(19, 30)[3], 0);
    }

    #[test]
    fn stroke_dynamics_count_against_the_work_budget() {
        let document = clear_document();
        let plain = cost(
            &document,
            &[edit(
                json!({"op": "stroke", "points": [[0, 0], [1000, 0]], "size": 100}),
            )],
        );
        let scattered = cost(
            &document,
            &[edit(json!({
                "op": "stroke", "points": [[0, 0], [1000, 0]], "size": 100,
                "spacing": 0.01, "scatter": 1, "scatter_count": 16,
            }))],
        );
        assert_eq!(plain.stroke, scattered.stroke);
        assert!(scattered.work > plain.work * 100, "{scattered:?} {plain:?}");
        // Taper is cut into short pieces, which cost more than one sweep.
        let tapered = cost(
            &document,
            &[edit(json!({
                "op": "stroke", "points": [[0, 0], [1000, 0]], "size": 100,
                "taper_in": 500, "taper_out": 500,
            }))],
        );
        assert!(tapered.work > plain.work * 2, "{tapered:?} {plain:?}");
    }
}

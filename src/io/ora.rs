//! OpenRaster (`.ora`) import and export, the layered format Krita, GIMP and MyPaint share.
//!
//! Written from the OpenRaster 0.0.6 specification (file layout and layer stack), with Krita's
//! reader and writer (`plugins/impex/ora/`) for the names it gives blend modes OpenRaster lacks:
//!
//! - A file is a ZIP archive holding `mimetype` (first and stored, `image/openraster`),
//!   `stack.xml`, one PNG per layer (`data/…`), `mergedimage.png` and `Thumbnails/thumbnail.png`.
//! - Stacks become folders and layers become pixel layers, keeping names, offsets, opacity,
//!   visibility, locks, the selected layer and every blend mode in [`BLEND_MODES`]. Xuan's
//!   folders pass through, so an isolated stack's own blend mode, and the isolation of a stack
//!   whose layers blend, are reported; so are Krita's filter layers and other content left out.
//! - Export writes pixel layers and folders as they are. Text, shapes, layer effects, masks,
//!   fill and transforms are drawn into the layer's pixels; adjustment, filter and mask layers
//!   are merged with the layers below them in their folder. Everything drawn is counted in an
//!   [`ExportReport`].
//!
//! The archive is untrusted, under the same rules as `.xuan` projects: the entry count and every
//! entry's size are capped before reading, layer paths must stay inside the archive, each layer
//! image is checked against the canvas and pixel limits from its PNG header before it is
//! inflated, and stack nesting and XML size are capped.
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{Cursor, Read, Seek, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow, ensure};
use image::{ImageFormat, RgbaImage};
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::{
    MAX_ARCHIVE_ENTRIES, MAX_MANIFEST,
    compositor::{Dropped, ImportReport, ImportSource},
    decode_image, max_asset,
    psd::{MAX_FOLDER_DEPTH, PixelBudget},
    write_entry, zip_read,
};
use crate::{
    blend::BlendMode,
    document::{Document, Layer, MAX_LAYERS, validate_size},
    i18n::tr,
    render,
};

/// The content of the `mimetype` entry.
pub const MIMETYPE: &str = "image/openraster";
/// The specification version Xuan writes.
const VERSION: &str = "0.0.6";
/// The thumbnail's largest side.
pub const THUMBNAIL_SIDE: u32 = 256;
/// Elements, attributes and text in `stack.xml`: room for [`MAX_LAYERS`] with their whitespace.
const MAX_XML_NODES: u32 = 100_000;
/// Positions `Transform::valid` accepts.
const MAX_POSITION: f64 = 1_000_000.0;
/// Layer names Xuan's documents accept, in bytes.
const MAX_NAME_BYTES: usize = 16_384;
/// Layer paths, in bytes.
const MAX_SRC_BYTES: usize = 1_024;
/// Bytes a layer PNG may hold beyond its largest possible pixel data (16-bit RGBA with a
/// filter byte per row): room for metadata chunks, not for a deflate bomb.
const PNG_SLACK: u64 = 16 << 20;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// OpenRaster composite operations and the Xuan blend mode each is. The first name for a mode
/// is the one Xuan writes: OpenRaster's own (`svg:`) where it has one, otherwise Krita's
/// (`krita:`), which Krita reads back. Every Xuan mode is here.
pub const BLEND_MODES: [(&str, BlendMode); 27] = [
    ("svg:src-over", BlendMode::Normal),
    ("svg:multiply", BlendMode::Multiply),
    ("svg:screen", BlendMode::Screen),
    ("svg:overlay", BlendMode::Overlay),
    ("svg:darken", BlendMode::Darken),
    ("svg:lighten", BlendMode::Lighten),
    ("svg:color-dodge", BlendMode::ColorDodge),
    ("svg:color-burn", BlendMode::ColorBurn),
    ("svg:hard-light", BlendMode::HardLight),
    ("svg:soft-light", BlendMode::SoftLight),
    ("svg:difference", BlendMode::Difference),
    ("svg:color", BlendMode::Color),
    ("svg:luminosity", BlendMode::Luminosity),
    ("svg:hue", BlendMode::Hue),
    ("svg:saturation", BlendMode::Saturation),
    ("svg:plus", BlendMode::LinearDodge),
    ("krita:dissolve", BlendMode::Dissolve),
    ("krita:linear_burn", BlendMode::LinearBurn),
    ("krita:darker color", BlendMode::DarkerColor),
    ("krita:lighter color", BlendMode::LighterColor),
    ("krita:vivid_light", BlendMode::VividLight),
    ("krita:linear light", BlendMode::LinearLight),
    ("krita:pin_light", BlendMode::PinLight),
    ("krita:hard mix", BlendMode::HardMix),
    ("krita:exclusion", BlendMode::Exclusion),
    ("krita:subtract", BlendMode::Subtract),
    ("krita:divide", BlendMode::Divide),
];

/// Other names read as a Xuan mode: Krita's own names for modes it writes as `svg:`, its
/// Photoshop variants, and names older Krita versions wrote.
const BLEND_ALIASES: [(&str, BlendMode); 25] = [
    ("svg:exclusion", BlendMode::Exclusion),
    ("svg:add", BlendMode::LinearDodge),
    ("color-dodge", BlendMode::ColorDodge),
    ("difference", BlendMode::Difference),
    ("krita:normal", BlendMode::Normal),
    ("krita:multiply", BlendMode::Multiply),
    ("krita:screen", BlendMode::Screen),
    ("krita:overlay", BlendMode::Overlay),
    ("krita:darken", BlendMode::Darken),
    ("krita:lighten", BlendMode::Lighten),
    ("krita:dodge", BlendMode::ColorDodge),
    ("krita:burn", BlendMode::ColorBurn),
    ("krita:hard_light", BlendMode::HardLight),
    ("krita:soft_light", BlendMode::SoftLight),
    ("krita:soft_light_svg", BlendMode::SoftLight),
    ("krita:diff", BlendMode::Difference),
    ("krita:color", BlendMode::Color),
    ("krita:luminize", BlendMode::Luminosity),
    ("krita:hue", BlendMode::Hue),
    ("krita:saturation", BlendMode::Saturation),
    ("krita:add", BlendMode::LinearDodge),
    ("krita:linear_dodge", BlendMode::LinearDodge),
    ("krita:hard_mix_photoshop", BlendMode::HardMix),
    ("krita:vivid_light_hdr", BlendMode::VividLight),
    ("krita:linear_light", BlendMode::LinearLight),
];

/// OpenRaster's Porter-Duff operators, which Xuan draws as Normal and reports by name.
const UNSUPPORTED_OPS: [&str; 5] = [
    "svg:src-atop",
    "svg:dst-atop",
    "svg:dst-in",
    "svg:dst-out",
    "svg:clear",
];

/// Name reported for a composite operation Xuan does not know at all.
const UNKNOWN_OP: &str = "Unknown";

/// The Xuan blend mode for an OpenRaster composite operation, if Xuan has it.
pub fn blend_mode(op: &str) -> Option<BlendMode> {
    BLEND_MODES
        .iter()
        .chain(&BLEND_ALIASES)
        .find(|(name, _)| *name == op)
        .map(|(_, mode)| *mode)
}

/// The composite operation Xuan writes for a blend mode.
pub fn composite_op(mode: BlendMode) -> &'static str {
    BLEND_MODES
        .iter()
        .find(|(_, m)| *m == mode)
        .map_or("svg:src-over", |(name, _)| name)
}

/// The name a report shows for a composite operation: its own when it is a known one.
fn op_name(op: &str) -> &'static str {
    BLEND_MODES
        .iter()
        .chain(&BLEND_ALIASES)
        .map(|(name, _)| *name)
        .chain(UNSUPPORTED_OPS)
        .find(|name| *name == op)
        .unwrap_or(UNKNOWN_OP)
}

/// Whether `path` names an OpenRaster file (`.ora`, any case).
pub fn is_openraster(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ora"))
}

fn not_openraster() -> anyhow::Error {
    anyhow!(tr("This is not an OpenRaster file"))
}

fn damaged(detail: impl std::fmt::Display) -> anyhow::Error {
    anyhow!(
        "{} ({detail})",
        tr("The OpenRaster file is damaged or incomplete")
    )
}

fn too_large() -> anyhow::Error {
    let budget = crate::limits::get().project_pixels;
    anyhow!(
        tr("The OpenRaster file's layers don't fit within {} megapixels, the most this computer opens")
            .replace("{}", &crate::limits::grouped(budget / 1_000_000))
    )
}

/// Refuse a layer path that could name something outside the archive's own tree: absolute
/// paths, drive letters and URLs, backslashes, and empty, `.` or `..` components.
pub fn check_src(src: &str) -> Result<()> {
    let safe = !src.is_empty()
        && src.len() <= MAX_SRC_BYTES
        && !src.contains(['\\', ':', '\0'])
        && src
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    let shown: String = src.chars().take(80).collect();
    ensure!(
        safe,
        tr("The OpenRaster file names an unsafe layer path: {}")
            .replace("{}", &shown.escape_debug().to_string())
    );
    Ok(())
}

/// Read an OpenRaster file from disk; see [`read`].
pub fn load(path: &Path, budget: PixelBudget) -> Result<(Document, ImportReport)> {
    let metadata = fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    // Opening a FIFO or device could block forever.
    ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        path.display()
    );
    read(File::open(path)?, budget)
}

/// Read an OpenRaster archive, reporting what Xuan changed. `budget` is what the destination
/// may still hold; layers beyond it are refused.
pub fn read<R: Read + Seek>(reader: R, budget: PixelBudget) -> Result<(Document, ImportReport)> {
    let mut archive = ZipArchive::new(reader).map_err(|_| not_openraster())?;
    ensure!(
        archive.len() <= MAX_ARCHIVE_ENTRIES,
        tr("The OpenRaster file has too many entries")
    );
    ensure!(
        archive.index_for_name("mimetype").is_some(),
        tr("This is not an OpenRaster file: its mimetype entry is missing")
    );
    let mimetype = zip_read(&mut archive, "mimetype", 256)?;
    ensure!(
        mimetype.trim_ascii() == MIMETYPE.as_bytes(),
        not_openraster()
    );
    ensure!(
        archive.index_for_name("stack.xml").is_some(),
        damaged("stack.xml is missing")
    );
    let xml = zip_read(&mut archive, "stack.xml", MAX_MANIFEST)?;
    let xml = std::str::from_utf8(&xml).map_err(|_| damaged("stack.xml is not UTF-8"))?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XML_NODES,
    };
    let tree = roxmltree::Document::parse_with_options(xml, options).map_err(damaged)?;
    let image = tree.root_element();
    ensure!(
        image.has_tag_name("image"),
        damaged("stack.xml has no image")
    );
    let side = |name: &str| -> Result<u32> {
        image
            .attribute(name)
            .and_then(|v| v.trim().parse().ok())
            .ok_or_else(|| damaged(format!("image {name}")))
    };
    let (width, height) = (side("w")?, side("h")?);
    validate_size(width, height)?;
    let mut document = Document::new(width, height)?;
    document.layers.clear();
    if let Some(resolution) = image
        .attribute("xres")
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|r| r.is_finite() && (1.0..=9600.0).contains(r))
    {
        document.resolution = resolution;
    }
    let root = image
        .children()
        .find(|n| n.has_tag_name("stack"))
        .ok_or_else(|| damaged("stack.xml has no root stack"))?;
    let mut importer = Importer {
        archive,
        document,
        report: ImportReport::new(ImportSource::OpenRaster),
        pixels_left: budget.layers,
        decoded: 0,
        active: None,
    };
    importer.stack(root, None, 0)?;
    let Importer {
        mut document,
        report,
        active,
        ..
    } = importer;
    document.active = active.or_else(|| {
        document
            .layers
            .iter()
            .rev()
            .find(|l| l.parent.is_none())
            .map(|l| l.id)
    });
    document.selected = document.active.into_iter().collect();
    document.validate()?;
    Ok((document, report))
}

struct Importer<R> {
    archive: ZipArchive<R>,
    document: Document,
    report: ImportReport,
    /// Layer pixels the destination may still take.
    pixels_left: u64,
    /// Pixels decoded so far, for [`decode_image`]'s own budget.
    decoded: u64,
    /// The layer the file marks as selected.
    active: Option<Uuid>,
}

/// The attributes stacks and layers share.
struct Common {
    name: String,
    visible: bool,
    opacity: f32,
    op: String,
    locked: bool,
    selected: bool,
}

impl Common {
    fn read(node: roxmltree::Node<'_, '_>, fallback: &str) -> Result<Self> {
        let opacity = match node.attribute("opacity") {
            None => 1.0,
            Some(value) => value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| damaged(format!("opacity “{}”", short(value))))?
                .clamp(0.0, 1.0),
        };
        let mut name = node.attribute("name").unwrap_or_default().to_owned();
        if name.len() > MAX_NAME_BYTES {
            let mut end = MAX_NAME_BYTES;
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            name.truncate(end);
        }
        if name.trim().is_empty() {
            name = fallback.to_owned();
        }
        Ok(Self {
            name,
            visible: node.attribute("visibility") != Some("hidden"),
            opacity,
            op: node
                .attribute("composite-op")
                .unwrap_or("svg:src-over")
                .trim()
                .to_owned(),
            locked: node.attribute("edit-locked") == Some("true"),
            selected: node.attribute("selected") == Some("true"),
        })
    }

    fn apply(&self, layer: &mut Layer) {
        layer.name = self.name.clone();
        layer.visible = self.visible;
        layer.opacity = self.opacity;
        layer.locked = self.locked;
    }
}

/// The start of an untrusted value, for an error message.
fn short(value: &str) -> String {
    value
        .chars()
        .take(40)
        .collect::<String>()
        .escape_debug()
        .to_string()
}

fn position(node: roxmltree::Node<'_, '_>, name: &str) -> Result<f32> {
    let Some(value) = node.attribute(name) else {
        return Ok(0.0);
    };
    let number = value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && v.abs() <= MAX_POSITION)
        .ok_or_else(|| damaged(format!("{name} “{}”", short(value))))?;
    Ok(number.round() as f32)
}

impl<R: Read + Seek> Importer<R> {
    /// Add a stack's children under `parent`, bottom first as Xuan orders layers.
    fn stack(
        &mut self,
        node: roxmltree::Node<'_, '_>,
        parent: Option<Uuid>,
        depth: usize,
    ) -> Result<()> {
        let children: Vec<_> = node.children().filter(|n| n.is_element()).collect();
        for child in children.into_iter().rev() {
            ensure!(
                self.document.layers.len() < MAX_LAYERS,
                tr("The OpenRaster file has too many layers")
            );
            match child.tag_name().name() {
                "stack" => self.folder(child, parent, depth)?,
                "layer" => self.layer(child, parent)?,
                // Krita's adjustment layers (`filter`), text and anything newer.
                _ => self.report.add(Dropped::OpenRasterLeftOut),
            }
        }
        Ok(())
    }

    fn folder(
        &mut self,
        node: roxmltree::Node<'_, '_>,
        parent: Option<Uuid>,
        depth: usize,
    ) -> Result<()> {
        ensure!(
            depth < MAX_FOLDER_DEPTH,
            tr("The OpenRaster file's stacks are nested too deeply")
        );
        let common = Common::read(node, "Folder")?;
        let mut folder = Layer::blank("Folder", self.document.width, self.document.height);
        folder.group = true;
        folder.parent = parent;
        common.apply(&mut folder);
        if common.selected {
            self.active = Some(folder.id);
        }
        let first = self.document.layers.len();
        self.stack(node, Some(folder.id), depth + 1)?;
        // Xuan's folders pass through: a stack's own blend mode is lost, and so is the
        // isolation that keeps its layers' blend modes from reaching what is below.
        if blend_mode(&common.op) != Some(BlendMode::Normal) {
            self.report
                .add(Dropped::FolderBlendMode(op_name(&common.op)));
        } else if node.attribute("isolation").map(str::trim) != Some("auto")
            && self.document.layers[first..]
                .iter()
                .any(|l| !l.group && l.blend != BlendMode::Normal)
        {
            self.report.add(Dropped::IsolatedFolder);
        }
        // Folders follow their contents.
        self.document.layers.push(folder);
        Ok(())
    }

    fn layer(&mut self, node: roxmltree::Node<'_, '_>, parent: Option<Uuid>) -> Result<()> {
        let common = Common::read(node, "Layer")?;
        let (x, y) = (position(node, "x")?, position(node, "y")?);
        let Some(src) = node.attribute("src") else {
            self.report.add(Dropped::OpenRasterLeftOut);
            return Ok(());
        };
        check_src(src)?;
        let Some(pixels) = self.layer_image(src)? else {
            self.report.add(Dropped::OpenRasterLeftOut);
            return Ok(());
        };
        let mut layer = Layer::image(common.name.clone(), pixels);
        layer.parent = parent;
        layer.transform.x = x;
        layer.transform.y = y;
        common.apply(&mut layer);
        layer.blend = blend_mode(&common.op).unwrap_or_else(|| {
            self.report
                .add(Dropped::OpenRasterBlendMode(op_name(&common.op)));
            BlendMode::Normal
        });
        if node.attribute("alpha-preserve") == Some("true") {
            self.report
                .add(Dropped::OpenRasterBlendMode("alpha-preserve"));
        }
        if common.selected {
            self.active = Some(layer.id);
        }
        self.document.layers.push(layer);
        Ok(())
    }

    /// A layer's PNG, or `None` for a source in another format. Its size is checked from the
    /// PNG header before the rest is inflated, and the entry may not be much larger than the
    /// pixels it claims.
    fn layer_image(&mut self, src: &str) -> Result<Option<RgbaImage>> {
        let index = self.archive.index_for_name(src).ok_or_else(|| {
            anyhow!(
                tr("The OpenRaster file is missing the layer image {}").replace("{}", &short(src))
            )
        })?;
        let mut file = self.archive.by_index(index)?;
        ensure!(
            file.size() <= max_asset(),
            tr("An OpenRaster layer image is too large")
        );
        let mut header = [0; 24];
        if file.read_exact(&mut header).is_err()
            || header[..8] != PNG_SIGNATURE
            || &header[12..16] != b"IHDR"
        {
            return Ok(None);
        }
        let width = u32::from_be_bytes(header[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(header[20..24].try_into().unwrap());
        validate_size(width, height)?;
        let area = u64::from(width) * u64::from(height);
        ensure!(area <= self.pixels_left, too_large());
        let limit = (area * 8 + u64::from(height) + PNG_SLACK).min(max_asset());
        let mut bytes = header.to_vec();
        file.take(limit + 1 - header.len() as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= limit,
            tr("An OpenRaster layer image is too large")
        );
        let pixels = decode_image(bytes, &mut self.decoded)?.to_rgba8();
        self.pixels_left -= area;
        Ok(Some(pixels))
    }
}

// ---------------------------------------------------------------------------------------------
// Export

/// Something an OpenRaster export drew into pixels, merged or left out, as OpenRaster has no
/// place for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flattened {
    Text,
    Shape,
    LayerEffects,
    /// A layer mask, applied to the layer's pixels.
    Mask,
    /// A fill below 100%, applied to the layer's pixels.
    Fill,
    /// A moved by a fraction of a pixel, scaled, rotated, flipped or warped layer.
    Transform,
    /// Adjustments, filters and masks attached to a pixel layer.
    AttachedEffects,
    /// Layers clipped to another, drawn into it.
    Clipping,
    /// Adjustment layers, merged with the layers below them.
    Adjustment,
    /// Filter layers, merged with the layers below them.
    Filter,
    /// Mask layers in a folder, merged with the layers below them.
    MaskLayer,
    /// A folder with its own mask, drawn as one layer.
    FolderMask,
    /// Hidden adjustment, filter and mask layers, which change nothing and are left out.
    HiddenEffect,
    /// RAW layers, written as their developed pixels.
    Raw,
}

impl Flattened {
    fn label(self) -> &'static str {
        tr(match self {
            Self::Text => "Text layers (flattened to pixels)",
            Self::Shape => "Shape layers (flattened to pixels)",
            Self::LayerEffects => "Layer effects (drawn into their layers)",
            Self::Mask => "Layer masks (applied to the layer's pixels)",
            Self::Fill => "Fill below 100% (applied to the layer's pixels)",
            Self::Transform => "Scaled, rotated or warped layers (resampled to pixels)",
            Self::AttachedEffects => {
                "Adjustments, filters and masks on a layer (applied to its pixels)"
            }
            Self::Clipping => "Clipping masks (merged into their base layer)",
            Self::Adjustment => "Adjustment layers (merged with the layers below them)",
            Self::Filter => "Filter layers (merged with the layers below them)",
            Self::MaskLayer => "Mask layers (merged with the layers below them)",
            Self::FolderMask => "Folders with a mask (flattened to one layer)",
            Self::HiddenEffect => "Hidden adjustment, filter or mask layers (left out)",
            Self::Raw => "RAW layers (written as their developed pixels)",
        })
    }
}

/// What an OpenRaster export flattened. Empty when every layer was written as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportReport {
    flattened: BTreeMap<Flattened, usize>,
}

impl ExportReport {
    fn add(&mut self, item: Flattened) {
        *self.flattened.entry(item).or_default() += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.flattened.is_empty()
    }

    /// How many layers `item` applied to.
    pub fn count(&self, item: Flattened) -> usize {
        self.flattened.get(&item).copied().unwrap_or(0)
    }

    /// One translated line per kind of change, with how often it occurred.
    pub fn lines(&self) -> Vec<String> {
        self.flattened
            .iter()
            .map(|(item, count)| format!("{}: {count}", item.label()))
            .collect()
    }

    /// A short, translated notice for the user, or `None` when nothing was flattened.
    pub fn summary(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut text =
            tr("Exported with changes. OpenRaster can't hold these, so Xuan flattened them:")
                .to_owned();
        for line in self.lines() {
            text.push_str(&format!("\n• {line}"));
        }
        Some(text)
    }
}

/// One element of the written stack.
enum Entry<'a> {
    /// A folder written as a stack.
    Stack {
        folder: &'a Layer,
        children: Vec<Entry<'a>>,
    },
    /// A pixel layer written as it is.
    Pixels(&'a Layer),
    /// Layers drawn into one. With `merged`, the drawing keeps every layer's own opacity,
    /// blend mode and visibility, and the element is plain; otherwise those of `top` go on
    /// the element instead.
    Drawn {
        top: &'a Layer,
        roots: Vec<Uuid>,
        merged: bool,
    },
}

impl Entry<'_> {
    fn top(&self) -> &Layer {
        match self {
            Self::Stack { folder, .. } => folder,
            Self::Pixels(layer) | Self::Drawn { top: layer, .. } => layer,
        }
    }

    fn roots(&self) -> Vec<Uuid> {
        match self {
            Self::Drawn { roots, .. } => roots.clone(),
            _ => vec![self.top().id],
        }
    }

    /// Whether the element shows: a merged drawing always does.
    fn visible(&self) -> bool {
        matches!(self, Self::Drawn { merged: true, .. }) || self.top().visible
    }
}

/// Why a pixel, text or shape layer cannot be written as its own pixels.
fn reasons(owners: &HashSet<Uuid>, layer: &Layer) -> Vec<Flattened> {
    let mut reasons = Vec::new();
    if layer.text.is_some() {
        reasons.push(Flattened::Text);
    }
    if layer.shape.is_some() {
        reasons.push(Flattened::Shape);
    }
    if layer.effects.is_some() {
        reasons.push(Flattened::LayerEffects);
    }
    if layer.mask.is_some() {
        reasons.push(Flattened::Mask);
    }
    if layer.fill < 1.0 {
        reasons.push(Flattened::Fill);
    }
    let t = layer.transform;
    let plain = layer.pixels.as_ref().is_none_or(|pixels| {
        t.rotation == 0.0
            && !t.flip_x
            && !t.flip_y
            && t.warp.is_none()
            && t.x.fract() == 0.0
            && t.y.fract() == 0.0
            && t.width == pixels.width() as f32
            && t.height == pixels.height() as f32
    });
    if !plain {
        reasons.push(Flattened::Transform);
    }
    if owners.contains(&layer.id) {
        reasons.push(Flattened::AttachedEffects);
    }
    reasons
}

/// Plan the elements for the layers under `parent`, bottom first, counting what is flattened.
/// `owners` are the layers with others attached or inside.
fn plan<'a>(
    document: &'a Document,
    owners: &HashSet<Uuid>,
    parent: Option<Uuid>,
    depth: usize,
    report: &mut ExportReport,
) -> Vec<Entry<'a>> {
    let siblings: Vec<&Layer> = document
        .layers
        .iter()
        .filter(|l| l.parent == parent)
        .collect();
    let here: HashSet<Uuid> = siblings.iter().map(|l| l.id).collect();
    let mut entries: Vec<Entry<'a>> = Vec::new();
    for &layer in &siblings {
        // Clipped layers are drawn with their base.
        if layer.clip_to.is_some_and(|base| here.contains(&base)) {
            continue;
        }
        let clipped: Vec<Uuid> = siblings
            .iter()
            .filter(|l| l.clip_to == Some(layer.id))
            .map(|l| l.id)
            .collect();
        if layer.is_effect() {
            if !layer.visible {
                report.add(Flattened::HiddenEffect);
                continue;
            }
            report.add(if layer.adjustment.is_some() {
                Flattened::Adjustment
            } else if layer.filter.is_some() {
                Flattened::Filter
            } else {
                Flattened::MaskLayer
            });
            if !clipped.is_empty() {
                report.add(Flattened::Clipping);
            }
            // What shows below is merged with it; hidden layers stay as they are, beneath.
            let (shown, hidden): (Vec<_>, Vec<_>) = entries.drain(..).partition(Entry::visible);
            let mut roots: Vec<Uuid> = shown.iter().flat_map(Entry::roots).collect();
            roots.push(layer.id);
            roots.extend(clipped);
            entries = hidden;
            entries.push(Entry::Drawn {
                top: layer,
                roots,
                merged: true,
            });
            continue;
        }
        if !clipped.is_empty() {
            report.add(Flattened::Clipping);
        }
        let mut roots = vec![layer.id];
        roots.extend(&clipped);
        if layer.group {
            if layer.mask.is_some() || !clipped.is_empty() || depth >= MAX_FOLDER_DEPTH {
                if layer.mask.is_some() {
                    report.add(Flattened::FolderMask);
                }
                entries.push(Entry::Drawn {
                    top: layer,
                    roots,
                    merged: false,
                });
            } else {
                entries.push(Entry::Stack {
                    folder: layer,
                    children: plan(document, owners, Some(layer.id), depth + 1, report),
                });
            }
            continue;
        }
        if layer.raw.is_some() {
            report.add(Flattened::Raw);
        }
        let reasons = reasons(owners, layer);
        if reasons.is_empty() && clipped.is_empty() && layer.pixels.is_some() {
            entries.push(Entry::Pixels(layer));
        } else {
            for reason in reasons {
                report.add(reason);
            }
            entries.push(Entry::Drawn {
                top: layer,
                roots,
                merged: false,
            });
        }
    }
    entries
}

fn owners(document: &Document) -> HashSet<Uuid> {
    document.layers.iter().filter_map(|l| l.parent).collect()
}

/// What exporting `document` as OpenRaster would flatten, without writing anything.
pub fn export_report(document: &Document) -> ExportReport {
    let mut report = ExportReport::default();
    plan(document, &owners(document), None, 0, &mut report);
    report
}

/// Draw layers alone on a transparent canvas and crop to what they cover: the position and
/// pixels of the element. Nothing drawn gives one transparent pixel, as Krita writes.
fn draw(document: &Document, roots: &[Uuid], top: &Layer, merged: bool) -> (i64, i64, RgbaImage) {
    let mut keep = HashSet::new();
    for id in roots {
        keep.extend(document.descendants(*id));
    }
    let mut alone = document.clone();
    alone.layers.retain(|l| keep.contains(&l.id));
    for layer in &mut alone.layers {
        if layer.parent.is_some_and(|id| !keep.contains(&id)) {
            layer.parent = None;
        }
        if layer.clip_to.is_some_and(|id| !keep.contains(&id)) {
            layer.clip_to = None;
        }
        if !merged && layer.id == top.id {
            layer.opacity = 1.0;
            layer.visible = true;
            layer.blend = BlendMode::Normal;
        }
    }
    alone.active = None;
    alone.selected.clear();
    alone.selection = None;
    crop(&render::render(&alone))
}

fn crop(image: &RgbaImage) -> (i64, i64, RgbaImage) {
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] > 0 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    if left == u32::MAX {
        return (0, 0, RgbaImage::new(1, 1));
    }
    let cropped = image::imageops::crop_imm(image, left, top, right - left, bottom - top);
    (i64::from(left), i64::from(top), cropped.to_image())
}

/// Text for an XML attribute value. Control characters XML 1.0 cannot hold are left out.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

struct Writer<'a, W: Write + Seek> {
    document: &'a Document,
    archive: ZipWriter<W>,
    xml: String,
    files: usize,
}

impl<W: Write + Seek> Writer<'_, W> {
    fn attributes(&mut self, layer: &Layer, plain: bool) {
        let (opacity, visible, op) = if plain {
            (1.0, true, "svg:src-over")
        } else {
            (layer.opacity, layer.visible, composite_op(layer.blend))
        };
        self.xml.push_str(&format!(
            " name=\"{}\" opacity=\"{opacity}\" visibility=\"{}\" composite-op=\"{op}\"",
            escape(&layer.name),
            if visible { "visible" } else { "hidden" },
        ));
        if layer.locked {
            self.xml.push_str(" edit-locked=\"true\"");
        }
        if self.document.active == Some(layer.id) {
            self.xml.push_str(" selected=\"true\"");
        }
    }

    fn layer_file(&mut self, image: &RgbaImage) -> Result<String> {
        let name = format!("data/layer{}.png", self.files);
        self.files += 1;
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        write_entry(&mut self.archive, name.clone(), options, &png(image)?)?;
        Ok(name)
    }

    /// Write elements top first, as OpenRaster lists them.
    fn entries(&mut self, entries: &[Entry<'_>], indent: usize) -> Result<()> {
        let pad = "  ".repeat(indent);
        for entry in entries.iter().rev() {
            match entry {
                Entry::Stack { folder, children } => {
                    self.xml.push_str(&format!("{pad}<stack"));
                    self.attributes(folder, false);
                    // Xuan's folders pass through.
                    self.xml.push_str(" isolation=\"auto\">\n");
                    self.entries(children, indent + 1)?;
                    self.xml.push_str(&format!("{pad}</stack>\n"));
                }
                Entry::Pixels(layer) => {
                    let pixels = layer.pixels.as_ref().expect("planned with pixels");
                    let src = self.layer_file(pixels)?;
                    self.layer(
                        layer,
                        false,
                        &src,
                        layer.transform.x as i64,
                        layer.transform.y as i64,
                        &pad,
                    );
                }
                Entry::Drawn { top, roots, merged } => {
                    let (x, y, image) = draw(self.document, roots, top, *merged);
                    let src = self.layer_file(&image)?;
                    self.layer(top, *merged, &src, x, y, &pad);
                }
            }
        }
        Ok(())
    }

    fn layer(&mut self, layer: &Layer, plain: bool, src: &str, x: i64, y: i64, pad: &str) {
        self.xml.push_str(&format!("{pad}<layer"));
        self.attributes(layer, plain);
        self.xml
            .push_str(&format!(" src=\"{src}\" x=\"{x}\" y=\"{y}\"/>\n"));
    }
}

/// Write `document` as an OpenRaster archive.
pub fn write<W: Write + Seek>(document: &Document, writer: W) -> Result<ExportReport> {
    let mut report = ExportReport::default();
    let entries = plan(document, &owners(document), None, 0, &mut report);
    let mut archive = ZipWriter::new(writer);
    // The mimetype comes first, stored, so tools can recognise the file from its first bytes.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    archive.start_file("mimetype", stored)?;
    archive.write_all(MIMETYPE.as_bytes())?;
    let mut writer = Writer {
        document,
        archive,
        xml: String::new(),
        files: 0,
    };
    writer.xml.push_str(&format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<image version=\"{VERSION}\" w=\"{}\" h=\"{}\" xres=\"{res}\" yres=\"{res}\">\n  <stack>\n",
        document.width,
        document.height,
        res = document.resolution.round().max(1.0) as u32,
    ));
    writer.entries(&entries, 2)?;
    writer.xml.push_str("  </stack>\n</image>\n");
    let Writer {
        mut archive, xml, ..
    } = writer;
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file("stack.xml", deflated)?;
    archive.write_all(xml.as_bytes())?;
    let merged = render::render(document);
    let longest = merged.width().max(merged.height());
    let thumbnail = if longest > THUMBNAIL_SIDE {
        let scale = |side: u32| {
            ((u64::from(side) * u64::from(THUMBNAIL_SIDE)).div_ceil(u64::from(longest)) as u32)
                .clamp(1, THUMBNAIL_SIDE)
        };
        render::resize_quality(&merged, scale(merged.width()), scale(merged.height()))
    } else {
        merged.clone()
    };
    write_entry(
        &mut archive,
        "mergedimage.png".into(),
        stored,
        &png(&merged)?,
    )?;
    write_entry(
        &mut archive,
        "Thumbnails/thumbnail.png".into(),
        stored,
        &png(&thumbnail)?,
    )?;
    archive.finish()?;
    Ok(report)
}

/// Persist a complete sibling temporary file, then atomically replace `path`.
pub fn export(document: &Document, path: &Path) -> Result<ExportReport> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let report = write(document, temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(report)
}

#[cfg(test)]
#[path = "ora_tests.rs"]
mod tests;

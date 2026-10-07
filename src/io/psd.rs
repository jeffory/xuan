//! One-way import of Adobe Photoshop documents: PSD (version 1) and PSB (version 2), 8-bit RGB.
//!
//! Written from Adobe's *Photoshop File Formats Specification* (file header, color mode data,
//! image resources, layer and mask information, image data), with upstream Compositor's importer
//! (`Compositor/IO/PSD/PSDReader.swift`, `PSDDocumentBuilder.swift`, `PSDTypes.swift`,
//! `PSDVector.swift`, `PSDText.swift`) setting the scope and the mapping onto layers:
//!
//! - Folders, layer masks, opacity, fill opacity, visibility, clipping and every Photoshop blend
//!   mode ([`BLEND_MODES`]) stay editable; folders always pass through.
//! - Filled rectangles and ellipses become live shapes; simple horizontal type becomes an
//!   editable text layer that keeps Photoshop's pixels until it is edited.
//! - Levels, Curves, Exposure, Invert, Black & White and Color Balance adjustment layers stay
//!   editable; other adjustments are left out. Layer effects are left out, as upstream does.
//!   Smart objects, fill layers, other vector content and type Xuan cannot edit are imported as
//!   Photoshop's pixels.
//! - When the layers do not fit Xuan's pixel budget, every layer is cropped to the canvas.
//!
//! Everything changed is counted in the shared [`ImportReport`].
//!
//! The file is untrusted. Every read goes through the bounds-checked [`Reader`]; sizes are checked
//! against the canvas and memory limits before anything is allocated for them; layer counts,
//! folder nesting and descriptor recursion are capped; and PackBits never writes past its row.
use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use image::{GrayImage, Luma, RgbaImage};
use uuid::Uuid;

use super::compositor::{Dropped, ImportReport, ImportSource, font_from_postscript};
use crate::{
    blend::BlendMode,
    document::{
        Adjustment, Document, Layer, MAX_LAYERS, MAX_PIXELS, Mask, Point, ShapeStyle, Transform,
        validate_size,
    },
    i18n::tr,
    layer_effects::LayerEffects,
    paint::ShapeKind,
    text::{MAX_TEXT_BYTES, TextStyle},
};

mod descriptor;

use descriptor::{Descriptor, Engine};

/// The largest Photoshop file Xuan reads into memory.
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// Photoshop's own limit on channels per layer and per document.
const MAX_CHANNELS: u16 = 56;
/// Layer and mask bounds beyond PSB's 300,000 pixels per side mark a damaged file.
const MAX_RECT_SIDE: i64 = 300_000;
/// Positions `Transform::valid` accepts; layers further out are cropped to the canvas.
const MAX_POSITION: i64 = 1_000_000;
/// Additional layer information blocks per layer record.
const MAX_LAYER_BLOCKS: usize = 256;
/// Folder nesting Xuan's documents allow.
pub const MAX_FOLDER_DEPTH: usize = 64;
/// Bytes one ZIP-compressed channel may inflate to (it is decoded row by row).
const MAX_ZIP_SOURCE: u64 = 400_000_000;

/// Photoshop blend-mode keys, Photoshop's names for them, and the Xuan mode each becomes. Every
/// Photoshop mode has a Xuan equivalent; a key not listed here draws as Normal and is reported.
pub const BLEND_MODES: [([u8; 4], &str, BlendMode); 28] = [
    (*b"pass", "Pass Through", BlendMode::Normal),
    (*b"norm", "Normal", BlendMode::Normal),
    (*b"diss", "Dissolve", BlendMode::Dissolve),
    (*b"dark", "Darken", BlendMode::Darken),
    (*b"mul ", "Multiply", BlendMode::Multiply),
    (*b"idiv", "Color Burn", BlendMode::ColorBurn),
    (*b"lbrn", "Linear Burn", BlendMode::LinearBurn),
    (*b"dkCl", "Darker Color", BlendMode::DarkerColor),
    (*b"lite", "Lighten", BlendMode::Lighten),
    (*b"scrn", "Screen", BlendMode::Screen),
    (*b"div ", "Color Dodge", BlendMode::ColorDodge),
    (*b"lddg", "Linear Dodge (Add)", BlendMode::LinearDodge),
    (*b"lgCl", "Lighter Color", BlendMode::LighterColor),
    (*b"over", "Overlay", BlendMode::Overlay),
    (*b"sLit", "Soft Light", BlendMode::SoftLight),
    (*b"hLit", "Hard Light", BlendMode::HardLight),
    (*b"vLit", "Vivid Light", BlendMode::VividLight),
    (*b"lLit", "Linear Light", BlendMode::LinearLight),
    (*b"pLit", "Pin Light", BlendMode::PinLight),
    (*b"hMix", "Hard Mix", BlendMode::HardMix),
    (*b"diff", "Difference", BlendMode::Difference),
    (*b"smud", "Exclusion", BlendMode::Exclusion),
    (*b"fsub", "Subtract", BlendMode::Subtract),
    (*b"fdiv", "Divide", BlendMode::Divide),
    (*b"hue ", "Hue", BlendMode::Hue),
    (*b"sat ", "Saturation", BlendMode::Saturation),
    (*b"colr", "Color", BlendMode::Color),
    (*b"lum ", "Luminosity", BlendMode::Luminosity),
];

/// Name reported for a blend key that is not in [`BLEND_MODES`].
const UNKNOWN_BLEND_MODE: &str = "Unknown";

/// Additional layer information keys whose length is 8 bytes in PSB files.
const PSB_LONG_KEYS: [&[u8; 4]; 13] = [
    b"LMsk", b"Lr16", b"Lr32", b"Layr", b"Mt16", b"Mt32", b"Mtrn", b"Alph", b"FMsk", b"lnk2",
    b"FEid", b"FXid", b"PxSD",
];

/// Adjustment layer keys and their Photoshop names.
const ADJUSTMENTS: [(&[u8; 4], &str); 18] = [
    (b"levl", "Levels"),
    (b"curv", "Curves"),
    (b"expA", "Exposure"),
    (b"nvrt", "Invert"),
    (b"brit", "Brightness/Contrast"),
    (b"hue2", "Hue/Saturation"),
    (b"hue ", "Hue/Saturation"),
    (b"blnc", "Color Balance"),
    (b"blwh", "Black & White"),
    (b"vibA", "Vibrance"),
    (b"phfl", "Photo Filter"),
    (b"mixr", "Channel Mixer"),
    (b"clrL", "Color Lookup"),
    (b"selc", "Selective Color"),
    (b"grdm", "Gradient Map"),
    (b"thrs", "Threshold"),
    (b"post", "Posterize"),
    (b"CgEd", "Brightness/Contrast"),
];

/// Pixels a Photoshop import may still add: what the destination document has left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelBudget {
    pub layers: u64,
    pub masks: u64,
}

impl Default for PixelBudget {
    fn default() -> Self {
        Self {
            layers: MAX_PIXELS,
            masks: MAX_PIXELS,
        }
    }
}

impl PixelBudget {
    /// What is left after the pixels and masks `document` already holds.
    pub fn remaining(document: &Document) -> Self {
        let mut budget = Self::default();
        for layer in &document.layers {
            if let Some(pixels) = &layer.pixels {
                let area = u64::from(pixels.width()) * u64::from(pixels.height());
                budget.layers = budget.layers.saturating_sub(area);
            }
            if let Some(mask) = &layer.mask {
                let area = u64::from(mask.pixels.width()) * u64::from(mask.pixels.height());
                budget.masks = budget.masks.saturating_sub(area);
            }
        }
        budget
    }
}

/// Whether `path` names a Photoshop document (`.psd` or `.psb`, any case).
pub fn is_photoshop(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
}

/// Read a Photoshop file from disk; see [`read`].
pub fn load(path: &Path, budget: PixelBudget) -> Result<(Document, ImportReport)> {
    let metadata = fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        tr("Photoshop files are limited to 1 GiB")
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        tr("Photoshop files are limited to 1 GiB")
    );
    read(&bytes, budget)
}

pub(crate) fn damaged() -> anyhow::Error {
    anyhow!(tr("The Photoshop file is damaged or incomplete"))
}

fn too_large() -> anyhow::Error {
    anyhow!(tr(
        "The Photoshop file's layers don't fit in the 100-megapixel limit, even when cropped to the canvas"
    ))
}

/// Bounds-checked big-endian reads over a byte slice. Every read either succeeds completely or
/// fails with [`damaged`] without moving past the end.
#[derive(Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let rest = &self.data[self.pos..];
        self.pos = self.data.len();
        rest
    }

    pub fn bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(damaged)?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.bytes(count).map(drop)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.array()?))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.array()?))
    }

    /// A section length: 4 bytes, or 8 in PSB files where the specification says so.
    fn length(&mut self, long: bool) -> Result<usize> {
        let length = if long {
            self.u64()?
        } else {
            u64::from(self.u32()?)
        };
        usize::try_from(length).map_err(|_| damaged())
    }

    /// A reader over the next `count` bytes, which this reader skips.
    fn sub(&mut self, count: usize) -> Result<Reader<'a>> {
        Ok(Reader::new(self.bytes(count)?))
    }

    /// A length-prefixed section.
    fn section(&mut self, long: bool) -> Result<Reader<'a>> {
        let length = self.length(long)?;
        self.sub(length)
    }
}

struct Header {
    psb: bool,
    channels: u16,
    width: u32,
    height: u32,
}

fn color_mode_name(mode: u16) -> &'static str {
    match mode {
        0 => "Bitmap",
        1 => "Grayscale",
        2 => "Indexed Color",
        4 => "CMYK",
        7 => "Multichannel",
        8 => "Duotone",
        9 => "Lab",
        _ => "unknown",
    }
}

fn header(reader: &mut Reader<'_>) -> Result<Header> {
    ensure!(
        reader.bytes(4).ok() == Some(b"8BPS"),
        tr("This is not a Photoshop file")
    );
    let version = reader.u16()?;
    ensure!(
        version == 1 || version == 2,
        tr("This Photoshop file version isn't supported")
    );
    reader.skip(6)?;
    let channels = reader.u16()?;
    let height = reader.u32()?;
    let width = reader.u32()?;
    let depth = reader.u16()?;
    let mode = reader.u16()?;
    let only_rgb8 = tr(
        "Only 8-bit RGB Photoshop files can be imported. In Photoshop, choose Image → Mode → RGB Color and 8 Bits/Channel, then save again.",
    );
    if mode != 3 {
        bail!(
            "{} {only_rgb8}",
            tr("This Photoshop file uses the {} color mode.")
                .replace("{}", tr(color_mode_name(mode)))
        );
    }
    if depth != 8 {
        bail!(
            "{} {only_rgb8}",
            tr("This Photoshop file uses {} bits per channel.").replace("{}", &depth.to_string())
        );
    }
    ensure!((1..=MAX_CHANNELS).contains(&channels), damaged());
    validate_size(width, height)?;
    Ok(Header {
        psb: version == 2,
        channels,
        width,
        height,
    })
}

/// What the importer uses from the image resources.
struct Resources {
    /// From resource 1005 (ResolutionInfo), or 72 ppi.
    resolution: f32,
    /// Resource 1037: the global light angle effects can follow, or Photoshop's default 120°.
    global_angle: f64,
}

/// Read the image resources the importer uses. Malformed resources end the walk; they never
/// fail the import.
fn resources(mut resources: Reader<'_>) -> Resources {
    let mut resolution = 72.0;
    let mut global_angle = 120.0;
    let mut walk = || -> Result<()> {
        while resources.remaining() >= 12 {
            if resources.bytes(4)? != b"8BIM" {
                break;
            }
            let id = resources.u16()?;
            let name = resources.u8()? as usize;
            // The Pascal name, with its length byte, is padded to an even size.
            resources.skip(name + (name + 1) % 2)?;
            let length = resources.u32()? as usize;
            let mut data = resources.sub(length)?;
            if length % 2 == 1 && resources.remaining() > 0 {
                resources.skip(1)?;
            }
            if id == 1005 && length >= 4 {
                let ppi = f64::from(data.u32()?) / 65536.0;
                if ppi.is_finite() && ppi >= 1.0 {
                    resolution = ppi.min(9600.0) as f32;
                }
            }
            if id == 1037 && length >= 4 {
                global_angle = f64::from(data.i32()?).rem_euclid(360.0);
            }
        }
        Ok(())
    };
    let _ = walk();
    Resources {
        resolution,
        global_angle,
    }
}

/// Layer, mask and canvas bounds in document pixels; `right` and `bottom` are exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}

impl Rect {
    fn width(self) -> i64 {
        self.right - self.left
    }

    fn height(self) -> i64 {
        self.bottom - self.top
    }

    fn is_empty(self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    fn area(self) -> u64 {
        if self.is_empty() {
            0
        } else {
            self.width() as u64 * self.height() as u64
        }
    }

    fn intersect(self, other: Rect) -> Rect {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        Rect {
            left,
            top,
            right: self.right.min(other.right).max(left),
            bottom: self.bottom.min(other.bottom).max(top),
        }
    }

    /// Whether an image this size can sit at this position in a Xuan document.
    fn fits_document(self) -> bool {
        self.is_empty()
            || (validate_size(self.width() as u32, self.height() as u32).is_ok()
                && self.left.abs() <= MAX_POSITION
                && self.top.abs() <= MAX_POSITION)
    }

    fn transform(self) -> Transform {
        Transform {
            x: self.left as f32,
            y: self.top as f32,
            ..Transform::new(self.width() as u32, self.height() as u32)
        }
    }
}

fn read_rect(reader: &mut Reader<'_>) -> Result<Rect> {
    let top = i64::from(reader.i32()?);
    let left = i64::from(reader.i32()?);
    let bottom = i64::from(reader.i32()?);
    let right = i64::from(reader.i32()?);
    ensure!(
        (right - left).abs() <= MAX_RECT_SIDE && (bottom - top).abs() <= MAX_RECT_SIDE,
        tr("A Photoshop layer's bounds are out of range")
    );
    // Photoshop writes inverted bounds (a bottom of -1) for some empty layers.
    Ok(Rect {
        left,
        top,
        right: right.max(left),
        bottom: bottom.max(top),
    })
}

#[derive(Clone, Copy)]
struct MaskInfo {
    rect: Rect,
    default: u8,
    disabled: bool,
    linked: bool,
    /// Photoshop rendered this mask from other data (a vector mask).
    from_vector: bool,
}

struct Record<'a> {
    rect: Rect,
    channel_lengths: Vec<(i16, usize)>,
    channels: Vec<(i16, &'a [u8])>,
    blend: [u8; 4],
    opacity: u8,
    fill: u8,
    clipping: bool,
    hidden: bool,
    mask: Option<MaskInfo>,
    name: String,
    extra: Vec<([u8; 4], &'a [u8])>,
    /// Section divider type: 0 for layers, 1 and 2 for folders, 3 for a folder's end.
    section: u32,
    section_blend: Option<[u8; 4]>,
}

impl<'a> Record<'a> {
    fn get(&self, key: &[u8; 4]) -> Option<&'a [u8]> {
        self.extra.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    fn has(&self, keys: &[&[u8; 4]]) -> bool {
        self.extra.iter().any(|(k, _)| keys.contains(&k))
    }

    fn channel(&self, id: i16) -> Option<&'a [u8]> {
        self.channels
            .iter()
            .rev()
            .find(|(i, data)| *i == id && data.len() >= 2)
            .map(|(_, data)| *data)
    }

    fn has_pixels(&self) -> bool {
        !self.rect.is_empty() && (-1..=2).any(|id| self.channel(id).is_some())
    }
}

/// Mac OS Roman, bytes 0x80–0xFF, for legacy Pascal layer names.
const MAC_ROMAN: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø\
¿¡¬√ƒ≈∆«»…\u{a0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{f8ff}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";

fn mac_roman(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                b as char
            } else {
                MAC_ROMAN.chars().nth(usize::from(b - 0x80)).unwrap_or('?')
            }
        })
        .collect()
}

fn unicode_name(data: &[u8]) -> Option<String> {
    let mut reader = Reader::new(data);
    let count = reader.u32().ok()? as usize;
    let bytes = reader.bytes(count.checked_mul(2)?).ok()?;
    Some(utf16(bytes).trim_end_matches('\0').to_owned())
}

/// Big-endian UTF-16, with unpaired surrogates replaced (and a trailing odd byte ignored).
pub(crate) fn utf16(bytes: &[u8]) -> String {
    char::decode_utf16(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair)),
    )
    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
    .collect()
}

fn read_record<'a>(reader: &mut Reader<'a>, psb: bool) -> Result<Record<'a>> {
    let rect = read_rect(reader)?;
    let channel_count = reader.u16()?;
    ensure!(channel_count <= MAX_CHANNELS, damaged());
    let mut channel_lengths = Vec::with_capacity(channel_count.into());
    for _ in 0..channel_count {
        let id = reader.i16()?;
        channel_lengths.push((id, reader.length(psb)?));
    }
    ensure!(reader.bytes(4)? == b"8BIM", damaged());
    let blend: [u8; 4] = reader.array()?;
    let opacity = reader.u8()?;
    let clipping = reader.u8()? != 0;
    let flags = reader.u8()?;
    reader.skip(1)?;
    let mut extra = reader.section(false)?;

    let mut mask_data = extra.section(false)?;
    let mask = if mask_data.remaining() >= 18 {
        let rect = read_rect(&mut mask_data)?;
        let default = mask_data.u8()?;
        let flags = mask_data.u8()?;
        Some(MaskInfo {
            rect,
            default,
            disabled: flags & 2 != 0,
            linked: flags & 1 == 0,
            from_vector: flags & 8 != 0,
        })
    } else {
        None
    };
    extra.section(false)?; // blending ranges
    let name_length = extra.u8()? as usize;
    let mut name = mac_roman(extra.bytes(name_length)?);
    // The Pascal name, with its length byte, is padded to a multiple of 4.
    let padding = (4 - (name_length + 1) % 4) % 4;
    extra.skip(padding.min(extra.remaining()))?;

    let mut record = Record {
        rect,
        channel_lengths,
        channels: Vec::new(),
        blend,
        opacity,
        fill: 255,
        clipping,
        hidden: flags & 2 != 0,
        mask,
        name: String::new(),
        extra: Vec::new(),
        section: 0,
        section_blend: None,
    };
    while extra.remaining() >= 12 {
        let signature = extra.bytes(4)?;
        if signature != b"8BIM" && signature != b"8B64" {
            break;
        }
        // Photoshop writes a few dozen blocks; thousands of tiny ones would only cost memory.
        ensure!(record.extra.len() < MAX_LAYER_BLOCKS, damaged());
        let key: [u8; 4] = extra.array()?;
        let long = signature == b"8B64" || (psb && PSB_LONG_KEYS.contains(&&key));
        let data = extra.section(long)?.rest();
        if data.len() % 2 == 1 && extra.remaining() > 0 {
            extra.skip(1)?;
        }
        match &key {
            b"luni" => {
                if let Some(unicode) = unicode_name(data).filter(|n| !n.trim().is_empty()) {
                    name = unicode;
                }
            }
            b"iOpa" => record.fill = data.first().copied().unwrap_or(255),
            b"lsct" | b"lsdk" if data.len() >= 4 => {
                record.section = u32::from_be_bytes(data[..4].try_into().unwrap());
                if data.len() >= 12 && &data[4..8] == b"8BIM" {
                    record.section_blend = Some(data[8..12].try_into().unwrap());
                }
            }
            _ => {}
        }
        record.extra.push((key, data));
    }
    // Layer names have to be non-empty and bounded in Xuan.
    record.name = name.chars().take(255).collect();
    if record.name.trim().is_empty() {
        record.name = tr("Layer").into();
    }
    Ok(record)
}

fn read_records<'a>(info: &mut Reader<'a>, psb: bool) -> Result<Vec<Record<'a>>> {
    // A negative count says the first alpha channel is the merged result's transparency.
    let count = usize::from(info.i16()?.unsigned_abs());
    ensure!(
        count <= MAX_LAYERS,
        tr("The Photoshop file has too many layers")
    );
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(read_record(info, psb)?);
    }
    // Channel image data follows every record, in the same order.
    for record in &mut records {
        let mut channels = Vec::with_capacity(record.channel_lengths.len());
        for &(id, length) in &record.channel_lengths {
            channels.push((id, info.bytes(length)?));
        }
        record.channels = channels;
    }
    Ok(records)
}

/// The part of a channel to decode, in the channel's own pixel grid.
#[derive(Clone, Copy, Debug)]
struct Window {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl Window {
    /// `part` of the channel that covers `source`.
    fn within(source: Rect, part: Rect) -> Self {
        Self {
            x: (part.left - source.left) as usize,
            y: (part.top - source.top) as usize,
            width: part.width().max(0) as usize,
            height: part.height().max(0) as usize,
        }
    }
}

/// Minimum PackBits bytes for a row: each run yields at most 128 bytes from at least 2.
fn min_packbits_row(width: usize) -> usize {
    width.div_ceil(128) * 2
}

/// Row byte counts of a PackBits channel, checked against the data that follows them.
fn packbits_rows<'a>(
    reader: &mut Reader<'a>,
    rows: usize,
    width: usize,
    psb: bool,
) -> Result<Vec<&'a [u8]>> {
    let entry = if psb { 4 } else { 2 };
    let mut table = reader.sub(rows.checked_mul(entry).ok_or_else(damaged)?)?;
    let mut total = 0_usize;
    let minimum = min_packbits_row(width);
    let mut counts = Vec::with_capacity(rows);
    for _ in 0..rows {
        let count = if psb {
            table.u32()? as usize
        } else {
            usize::from(table.u16()?)
        };
        ensure!(count >= minimum, damaged());
        total = total.checked_add(count).ok_or_else(damaged)?;
        counts.push(count);
    }
    ensure!(total <= reader.remaining(), damaged());
    counts
        .into_iter()
        .map(|count| reader.bytes(count))
        .collect()
}

/// Unpack one PackBits row into `out`, which it fills exactly. Runs that would write past the
/// row, or read past the row's bytes, mark a damaged file.
fn unpack_row(source: &[u8], out: &mut [u8]) -> Result<()> {
    let mut read = 0;
    let mut written = 0;
    while written < out.len() {
        let header = *source.get(read).ok_or_else(damaged)? as i8;
        read += 1;
        if header >= 0 {
            let count = header as usize + 1;
            let literal = source.get(read..read + count).ok_or_else(damaged)?;
            out.get_mut(written..written + count)
                .ok_or_else(damaged)?
                .copy_from_slice(literal);
            read += count;
            written += count;
        } else if header != -128 {
            let count = (1 - i32::from(header)) as usize;
            let value = *source.get(read).ok_or_else(damaged)?;
            read += 1;
            out.get_mut(written..written + count)
                .ok_or_else(damaged)?
                .fill(value);
            written += count;
        }
    }
    Ok(())
}

/// Check a channel's compression and sizes before anything is allocated for it.
fn check_channel(data: &[u8], width: usize, height: usize, psb: bool) -> Result<()> {
    let mut reader = Reader::new(data);
    match reader.u16()? {
        0 => ensure!(
            width
                .checked_mul(height)
                .is_some_and(|size| size <= reader.remaining()),
            damaged()
        ),
        1 => {
            packbits_rows(&mut reader, height, width, psb)?;
        }
        2 | 3 => ensure!(
            (width as u64).saturating_mul(height as u64) <= MAX_ZIP_SOURCE,
            too_large()
        ),
        _ => bail!(tr(
            "This Photoshop file uses an unsupported compression method"
        )),
    }
    Ok(())
}

/// Decode `window` of a `width`×`height` channel (compression, then data) into a plane.
fn decode_channel(
    data: &[u8],
    width: usize,
    height: usize,
    window: Window,
    psb: bool,
) -> Result<Vec<u8>> {
    check_channel(data, width, height, psb)?;
    let mut reader = Reader::new(data);
    let compression = reader.u16()?;
    let mut plane = vec![0; window.width * window.height];
    if plane.is_empty() {
        return Ok(plane);
    }
    let mut copy_row = |row: usize, source: &[u8]| {
        if (window.y..window.y + window.height).contains(&row) {
            let start = (row - window.y) * window.width;
            plane[start..start + window.width]
                .copy_from_slice(&source[window.x..window.x + window.width]);
        }
    };
    let rows = window.y + window.height;
    match compression {
        0 => {
            let pixels = reader.rest();
            for row in window.y..rows {
                copy_row(row, &pixels[row * width..(row + 1) * width]);
            }
        }
        1 => {
            let encoded = packbits_rows(&mut reader, height, width, psb)?;
            let mut buffer = vec![0; width];
            for (row, source) in encoded.iter().enumerate().take(rows).skip(window.y) {
                unpack_row(source, &mut buffer)?;
                copy_row(row, &buffer);
            }
        }
        _ => {
            let mut inflater = flate2::read::ZlibDecoder::new(reader.rest());
            let mut buffer = vec![0; width];
            for row in 0..rows {
                inflater.read_exact(&mut buffer).map_err(|_| damaged())?;
                if compression == 3 {
                    // Prediction: each byte is stored as the difference from its left neighbor.
                    for i in 1..buffer.len() {
                        buffer[i] = buffer[i].wrapping_add(buffer[i - 1]);
                    }
                }
                copy_row(row, &buffer);
            }
        }
    }
    Ok(plane)
}

/// The document area a layer's pixels cover, cropped to the canvas when `crop` is set.
fn image_rect(record: &Record<'_>, crop: bool, canvas: Rect) -> Rect {
    if !record.has_pixels() || is_adjustment(record) || is_group(record) {
        Rect::default()
    } else if crop {
        record.rect.intersect(canvas)
    } else {
        record.rect
    }
}

/// The document area a layer's mask image covers, or `None` without a usable mask. Outside it
/// the mask hides everything, as Xuan's masks do, so a mask whose default is white covers the
/// layer's pixels (or the canvas), and a black one only its stored part.
fn mask_rect(record: &Record<'_>, image: Rect, crop: bool, canvas: Rect) -> Option<Rect> {
    let mask = record.mask.filter(|m| !m.from_vector)?;
    if mask.default == 0 {
        Some(if crop {
            mask.rect.intersect(canvas)
        } else {
            mask.rect
        })
    } else if image.is_empty() {
        Some(canvas)
    } else {
        Some(image)
    }
}

fn is_group(record: &Record<'_>) -> bool {
    matches!(record.section, 1 | 2)
}

fn is_adjustment(record: &Record<'_>) -> bool {
    !is_group(record) && ADJUSTMENTS.iter().any(|(key, _)| record.get(key).is_some())
}

/// Whether every layer and mask fits Xuan's limits, with or without cropping to the canvas.
fn fits(records: &[Record<'_>], crop: bool, canvas: Rect, budget: PixelBudget) -> bool {
    let mut pixels = 0_u64;
    let mut masks = 0_u64;
    for record in records.iter().filter(|r| r.section != 3) {
        let image = image_rect(record, crop, canvas);
        if !image.fits_document() {
            return false;
        }
        pixels += image.area();
        if let Some(mask) = mask_rect(record, image, crop, canvas) {
            if !mask.fits_document() {
                return false;
            }
            masks += mask.area();
        }
    }
    pixels <= budget.layers && masks <= budget.masks
}

fn decode_rgba(record: &Record<'_>, part: Rect, psb: bool) -> Result<RgbaImage> {
    let (width, height) = (record.rect.width() as usize, record.rect.height() as usize);
    let channels: Vec<_> = (-1..=2)
        .filter_map(|id| record.channel(id).map(|data| (id, data)))
        .collect();
    for (_, data) in &channels {
        check_channel(data, width, height, psb)?;
    }
    let window = Window::within(record.rect, part);
    let mut image = RgbaImage::from_pixel(
        window.width as u32,
        window.height as u32,
        image::Rgba([0, 0, 0, 255]),
    );
    for (id, data) in channels {
        let plane = decode_channel(data, width, height, window, psb)?;
        let index = if id < 0 { 3 } else { id as usize };
        for (pixel, value) in image.pixels_mut().zip(plane) {
            pixel[index] = value;
        }
    }
    Ok(image)
}

fn decode_mask(record: &Record<'_>, grid: Rect, psb: bool) -> Result<Mask> {
    let info = record.mask.expect("mask_rect checked the mask");
    let pixels = if grid.is_empty() {
        GrayImage::from_pixel(1, 1, Luma([info.default]))
    } else {
        let mut pixels = GrayImage::from_pixel(
            grid.width() as u32,
            grid.height() as u32,
            Luma([info.default]),
        );
        let part = info.rect.intersect(grid);
        if let Some(data) = record.channel(-2)
            && !part.is_empty()
        {
            let (width, height) = (info.rect.width() as usize, info.rect.height() as usize);
            let plane = decode_channel(data, width, height, Window::within(info.rect, part), psb)?;
            let offset = Window::within(grid, part);
            for (row, values) in plane.chunks_exact(offset.width).enumerate() {
                for (column, value) in values.iter().enumerate() {
                    pixels.put_pixel(
                        (offset.x + column) as u32,
                        (offset.y + row) as u32,
                        Luma([*value]),
                    );
                }
            }
        }
        pixels
    };
    Ok(Mask {
        pixels: Arc::new(pixels),
        enabled: !info.disabled,
        linked: info.linked,
        // An empty black mask hides the whole layer; place it anywhere.
        placement: Some(if grid.is_empty() {
            Rect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            }
            .transform()
        } else {
            grid.transform()
        }),
    })
}

/// Map a Photoshop blend key, reporting unknown keys, which draw as Normal.
fn blend_mode(key: [u8; 4], report: &mut ImportReport) -> BlendMode {
    match BLEND_MODES.iter().find(|(k, _, _)| *k == key) {
        Some((_, _, mode)) => *mode,
        None => {
            report.add(Dropped::PhotoshopBlendMode(UNKNOWN_BLEND_MODE));
            BlendMode::Normal
        }
    }
}

fn blend_name(key: [u8; 4]) -> &'static str {
    BLEND_MODES
        .iter()
        .find(|(k, _, _)| *k == key)
        .map_or(UNKNOWN_BLEND_MODE, |(_, name, _)| name)
}

/// Supported adjustment layers, or the Photoshop name of one Xuan leaves out.
fn adjustment(record: &Record<'_>) -> Result<Adjustment, &'static str> {
    let (key, name) = ADJUSTMENTS
        .iter()
        .find(|(key, _)| record.get(key).is_some())
        .expect("is_adjustment checked the key");
    let data = record.get(key).unwrap_or_default();
    let parsed = match *key {
        b"nvrt" => Some(Adjustment::Invert),
        b"levl" => levels(data),
        b"curv" => curves(data),
        b"expA" => exposure(data),
        b"blnc" => color_balance(data),
        b"blwh" => black_white(data),
        _ => None,
    };
    parsed
        .filter(|a| crate::effects::validate_adjustment(a).is_ok())
        .ok_or(*name)
}

/// `blnc`: for shadows, midtones and highlights, the cyan–red, magenta–green and yellow–blue
/// shifts (−100…100), then whether luminosity is preserved.
fn color_balance(data: &[u8]) -> Option<Adjustment> {
    let mut reader = Reader::new(data);
    let mut tone = || -> Option<[f32; 3]> {
        Some([
            f32::from(reader.i16().ok()?),
            f32::from(reader.i16().ok()?),
            f32::from(reader.i16().ok()?),
        ])
    };
    let (shadows, midtones, highlights) = (tone()?, tone()?, tone()?);
    Some(Adjustment::ColorBalance {
        shadows,
        midtones,
        highlights,
        preserve_luminosity: reader.u8().ok()? != 0,
    })
}

/// `blwh`: a versioned descriptor with the six color weights (percent) and an optional RGB tint,
/// which becomes Xuan's tint hue and saturation.
fn black_white(data: &[u8]) -> Option<Adjustment> {
    let settings = block_descriptor(data)?;
    let weights = ["Rd  ", "Yllw", "Grn ", "Cyn ", "Bl  ", "Mgnt"]
        .map(|key| settings.double(key).map(|v| v as f32));
    let Adjustment::BlackWhite {
        weights: defaults,
        tint_hue,
        tint_saturation,
        ..
    } = Adjustment::BLACK_WHITE
    else {
        unreachable!()
    };
    let (mut hue, mut saturation) = (tint_hue, tint_saturation);
    if let Some(color) = settings.object("tintColor").and_then(rgb) {
        let [r, g, b, _] = color.map(f32::from);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if max > 0.0 && max > min {
            let delta = max - min;
            let sector = if max == r {
                (g - b) / delta
            } else if max == g {
                (b - r) / delta + 2.0
            } else {
                (r - g) / delta + 4.0
            };
            hue = (sector * 60.0).rem_euclid(360.0);
            saturation = delta / max * 100.0;
        }
    }
    Some(Adjustment::BlackWhite {
        weights: std::array::from_fn(|i| weights[i].unwrap_or(defaults[i])),
        tint: settings.bool("useTint").unwrap_or(false),
        tint_hue: hue,
        tint_saturation: saturation,
    })
}

/// `levl`: a version, then records of input black, input white, output black, output white and
/// gamma ×100, for the composite and then red, green and blue.
fn levels(data: &[u8]) -> Option<Adjustment> {
    let mut reader = Reader::new(data);
    reader.u16().ok()?;
    let mut ranges = [[0.0; 5]; 4];
    for range in &mut ranges {
        let values: Vec<f32> = (0..5)
            .map(|_| reader.u16().map(f32::from))
            .collect::<Result<_>>()
            .ok()?;
        *range = [
            values[0],
            values[4] / 100.0,
            values[1],
            values[2],
            values[3],
        ];
    }
    Some(Adjustment::LevelsChannels { ranges })
}

/// `curv`: a map flag (0 for points), version 1, a bitmap of the curves present (composite,
/// red, green, blue, …), then each curve's points as output, input pairs.
fn curves(data: &[u8]) -> Option<Adjustment> {
    let mut reader = Reader::new(data);
    if reader.u8().ok()? != 0 || reader.u16().ok()? != 1 {
        return None;
    }
    let present = reader.u32().ok()?;
    let identity = || vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)];
    let mut channels: [Vec<Point>; 4] = std::array::from_fn(|_| identity());
    for (channel, curve) in channels.iter_mut().enumerate() {
        if present & (1 << channel) == 0 {
            continue;
        }
        let count = reader.u16().ok()?;
        if !(2..=19).contains(&count) {
            return None;
        }
        let mut points = Vec::new();
        for _ in 0..count {
            let output = f32::from(reader.u16().ok()?);
            let input = f32::from(reader.u16().ok()?);
            points.push(Point::new(
                input.min(255.0) / 255.0,
                output.min(255.0) / 255.0,
            ));
        }
        points.sort_by(|a, b| a.x.total_cmp(&b.x));
        if let Some(first) = points.first().copied()
            && first.x > 0.0
        {
            points.insert(0, Point::new(0.0, first.y));
        }
        if let Some(last) = points.last().copied()
            && last.x < 1.0
        {
            points.push(Point::new(1.0, last.y));
        }
        *curve = points;
    }
    Some(Adjustment::CurvesChannels { channels })
}

/// `expA`: a version, then exposure, offset and gamma as 32-bit floats.
fn exposure(data: &[u8]) -> Option<Adjustment> {
    let mut reader = Reader::new(data);
    reader.u16().ok()?;
    Some(Adjustment::Exposure {
        exposure: reader.f32().ok()?,
        offset: reader.f32().ok()?,
        gamma: reader.f32().ok()?,
    })
}

/// `SoCo`, `vstk` and similar blocks: a version (16) and a descriptor.
fn block_descriptor(data: &[u8]) -> Option<Descriptor> {
    descriptor::versioned(&mut Reader::new(data)).ok()
}

/// An RGB color descriptor (`RGBC`, components 0–255).
fn rgb(color: &Descriptor) -> Option<[u8; 4]> {
    if color.class != b"RGBC" {
        return None;
    }
    let channel = |key| color.double(key).map(|v| v.clamp(0.0, 255.0).round() as u8);
    Some([channel("Rd  ")?, channel("Grn ")?, channel("Bl  ")?, 255])
}

/// The solid color of a fill or shape layer: `SoCo`, or (Photoshop CC) `vscg` holding a `SoCo`.
fn fill_color(record: &Record<'_>) -> Option<[u8; 4]> {
    let data = match record.get(b"SoCo") {
        Some(data) => data,
        None => record.get(b"vscg")?.strip_prefix(b"SoCo")?,
    };
    rgb(block_descriptor(data)?.object("Clr ")?)
}

/// Fill and shape layers: a solid color, gradient or pattern fill.
const FILL_KEYS: [&[u8; 4]; 4] = [b"SoCo", b"GdFl", b"PtFl", b"vscg"];
/// Vector masks, which shape layers draw through.
const VECTOR_KEYS: [&[u8; 4]; 2] = [b"vmsk", b"vsms"];
/// Bézier segments are flattened into this many lines.
const CURVE_STEPS: usize = 16;
/// Vector path knots read from one layer.
const MAX_PATH_KNOTS: usize = 10_000;
/// Edge-row visits allowed when drawing one path; larger paths are left out.
const MAX_FILL_WORK: u64 = 200_000_000;

/// A vector mask (`vmsk`/`vsms`: version, flags, then 26-byte path records) as closed polygons in
/// document pixels, or `None` for a path that cannot be drawn. Knot coordinates are 8.24 fixed
/// point fractions of the canvas. An empty path whose initial fill rule is set covers the canvas.
fn vector_path(data: &[u8], canvas: Rect) -> Option<Vec<Vec<(f64, f64)>>> {
    let mut reader = Reader::new(data);
    reader.skip(8).ok()?;
    let point = |reader: &mut Reader<'_>| -> Option<(f64, f64)> {
        let y = f64::from(reader.i32().ok()?) / f64::from(1 << 24);
        let x = f64::from(reader.i32().ok()?) / f64::from(1 << 24);
        Some((x * canvas.width() as f64, y * canvas.height() as f64))
    };
    let mut subpaths: Vec<Vec<[(f64, f64); 3]>> = Vec::new();
    let mut knots = 0;
    let mut fill_all = false;
    while reader.remaining() >= 26 {
        let mut record = reader.sub(26).ok()?;
        match record.u16().ok()? {
            0 | 3 => subpaths.push(Vec::new()),
            1 | 2 | 4 | 5 => {
                knots += 1;
                if knots > MAX_PATH_KNOTS {
                    return None;
                }
                let knot = [
                    point(&mut record)?,
                    point(&mut record)?,
                    point(&mut record)?,
                ];
                subpaths.last_mut()?.push(knot);
            }
            8 => fill_all = record.u16().ok()? == 1,
            _ => {}
        }
    }
    let polygons: Vec<Vec<(f64, f64)>> = subpaths
        .iter()
        .filter(|knots| knots.len() >= 2)
        .map(|knots| {
            let mut polygon = Vec::with_capacity(knots.len() * CURVE_STEPS);
            for (i, &[_, anchor, leaving]) in knots.iter().enumerate() {
                let [entering, next, _] = knots[(i + 1) % knots.len()];
                for step in 0..CURVE_STEPS {
                    let t = step as f64 / CURVE_STEPS as f64;
                    let u = 1.0 - t;
                    let mix = |a: f64, b: f64, c: f64, d: f64| {
                        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
                    };
                    polygon.push((
                        mix(anchor.0, leaving.0, entering.0, next.0),
                        mix(anchor.1, leaving.1, entering.1, next.1),
                    ));
                }
            }
            polygon
        })
        .collect();
    if polygons.is_empty() && fill_all {
        let (w, h) = (canvas.width() as f64, canvas.height() as f64);
        return Some(vec![vec![(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]]);
    }
    (!polygons.is_empty()).then_some(polygons)
}

/// Fill polygons (nonzero winding, 4 sample rows per pixel, exact horizontal coverage) with
/// `color` over `area`, which must be non-empty.
fn fill_polygons(polygons: &[Vec<(f64, f64)>], area: Rect, color: [u8; 4]) -> RgbaImage {
    const SAMPLES: usize = 4;
    let (width, height) = (area.width() as usize, area.height() as usize);
    // Edges as (top y, bottom y, x at top, dx per y, winding).
    let mut edges: Vec<(f64, f64, f64, f64, i32)> = Vec::new();
    for polygon in polygons {
        for (i, &(x0, y0)) in polygon.iter().enumerate() {
            let (x1, y1) = polygon[(i + 1) % polygon.len()];
            if (y1 - y0).abs() < 1e-9 || !(x0.is_finite() && y0.is_finite()) {
                continue;
            }
            let (top, bottom, x, winding) = if y0 < y1 {
                (y0, y1, x0, 1)
            } else {
                (y1, y0, x1, -1)
            };
            edges.push((top, bottom, x, (x1 - x0) / (y1 - y0), winding));
        }
    }
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut coverage = vec![0.0_f32; width * height];
    let mut next = 0;
    let mut active: Vec<usize> = Vec::new();
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for row in 0..height * SAMPLES {
        let y = area.top as f64 + (row as f64 + 0.5) / SAMPLES as f64;
        while next < edges.len() && edges[next].0 <= y {
            active.push(next);
            next += 1;
        }
        active.retain(|&e| edges[e].1 > y);
        crossings.clear();
        for &e in &active {
            let (top, _, x, slope, winding) = edges[e];
            if top <= y {
                crossings.push((x + (y - top) * slope, winding));
            }
        }
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        let line = &mut coverage[(row / SAMPLES) * width..(row / SAMPLES + 1) * width];
        let mut winding = 0;
        for pair in crossings.windows(2) {
            winding += pair[0].1;
            if winding == 0 {
                continue;
            }
            let start = (pair[0].0 - area.left as f64).clamp(0.0, width as f64);
            let end = (pair[1].0 - area.left as f64).clamp(0.0, width as f64);
            let mut x = start;
            while x < end {
                let pixel = x.floor() as usize;
                let stop = end.min(pixel as f64 + 1.0);
                if let Some(value) = line.get_mut(pixel) {
                    *value += ((stop - x) / SAMPLES as f64) as f32;
                }
                x = stop;
            }
        }
    }
    RgbaImage::from_fn(width as u32, height as u32, |x, y| {
        let alpha = coverage[y as usize * width + x as usize].clamp(0.0, 1.0);
        image::Rgba([
            color[0],
            color[1],
            color[2],
            (alpha * f32::from(color[3])).round() as u8,
        ])
    })
}

/// Draw a solid-filled vector shape Photoshop stored without pixels, within the canvas.
fn draw_vector(record: &Record<'_>, canvas: Rect, pixels_left: u64) -> Option<(RgbaImage, Rect)> {
    let color = fill_color(record)?;
    let data = VECTOR_KEYS.iter().find_map(|key| record.get(key))?;
    let polygons = vector_path(data, canvas)?;
    let (mut left, mut top, mut right, mut bottom) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in polygons.iter().flatten() {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    let area = Rect {
        left: left.floor().max(-1e9) as i64,
        top: top.floor().max(-1e9) as i64,
        right: right.ceil().min(1e9) as i64,
        bottom: bottom.ceil().min(1e9) as i64,
    }
    .intersect(canvas);
    // Filling visits every edge on every sample row in the worst case.
    let edges: usize = polygons.iter().map(Vec::len).sum();
    let work = (edges as u64).saturating_mul(area.height().max(0) as u64 * 4);
    if area.is_empty() || area.area() > pixels_left || work > MAX_FILL_WORK {
        return None;
    }
    Some((fill_polygons(&polygons, area, color), area))
}

/// A live shape for a solid-filled rectangle, rounded rectangle or ellipse with no stroke: its
/// style and document bounds. Anything else stays as Photoshop's pixels.
fn live_shape(record: &Record<'_>) -> Option<(ShapeStyle, Rect)> {
    let fill = fill_color(record)?;
    if let Some(stroke) = record.get(b"vstk").map(block_descriptor) {
        let stroke = stroke?;
        if stroke.bool("strokeEnabled") == Some(true) || stroke.bool("fillEnabled") == Some(false) {
            return None;
        }
    }
    // `vogk`: a version (1), then a versioned descriptor listing each shape's origin.
    let mut reader = Reader::new(record.get(b"vogk")?);
    if reader.u32().ok()? != 1 {
        return None;
    }
    let origins = descriptor::versioned(&mut reader).ok()?;
    let [descriptor::Value::Descriptor(origin)] = origins.list("keyDescriptorList")? else {
        return None;
    };
    let mut kind = match origin.long("keyOriginType")? {
        1 | 2 => ShapeKind::Rectangle,
        5 => ShapeKind::Ellipse,
        _ => return None,
    };
    let bounds = origin.object("keyOriginShapeBBox")?;
    let side = |key| {
        bounds
            .double(key)
            .filter(|v| v.abs() <= MAX_POSITION as f64)
    };
    let rect = Rect {
        left: side("Left")?.round() as i64,
        top: side("Top ")?.round() as i64,
        right: side("Rght")?.round() as i64,
        bottom: side("Btom")?.round() as i64,
    };
    if rect.is_empty() || !rect.fits_document() {
        return None;
    }
    let mut corner_radius = 0.0;
    if kind == ShapeKind::Rectangle
        && let Some(radii) = origin.object("keyOriginRRectRadii")
    {
        let values: Vec<f64> = ["topLeft", "topRight", "bottomRight", "bottomLeft"]
            .iter()
            .map(|key| radii.double(key))
            .collect::<Option<_>>()?;
        let (low, high) = values
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        if high - low > 0.5 || low < 0.0 {
            return None;
        }
        if high > 0.0 {
            kind = ShapeKind::RoundedRectangle;
            corner_radius = high as f32;
        }
    }
    Some((
        ShapeStyle {
            kind,
            color: fill,
            corner_radius,
            path: None,
        },
        rect,
    ))
}

/// Editable text from a `TySh` block, reporting styles and layout Xuan keeps only until the text
/// is edited. `None` keeps the layer as pixels.
fn text_style(data: &[u8], report: &mut ImportReport) -> Option<TextStyle> {
    let mut reader = Reader::new(data);
    if reader.u16().ok()? != 1 {
        return None;
    }
    let matrix: Vec<f64> = (0..6).map(|_| reader.f64()).collect::<Result<_>>().ok()?;
    if reader.u16().ok()? != 50 {
        return None;
    }
    let text = descriptor::versioned(&mut reader).ok()?;
    if text.enumeration("Ornt").is_some_and(|o| o == b"Vrtc") {
        return None;
    }
    // Only unrotated, unskewed text at one scale: Xuan's text has no transform of its own.
    let [xx, xy, yx, yy, _, _] = matrix[..] else {
        return None;
    };
    if !(xx.is_finite() && xx > 0.0)
        || (yy - xx).abs() > 0.02 * xx
        || xy.abs() > 0.02 * xx
        || yx.abs() > 0.02 * xx
    {
        return None;
    }
    if reader.u16().ok()? == 1
        && let Ok(warp) = descriptor::versioned(&mut reader)
        && warp
            .enumeration("warpStyle")
            .is_some_and(|style| style != b"warpNone")
    {
        return None;
    }
    let content = text.text("Txt ")?.replace("\r\n", "\n").replace('\r', "\n");
    let content = content.trim_end_matches('\n');
    if content.is_empty() || content.len() > MAX_TEXT_BYTES {
        return None;
    }
    let engine = descriptor::engine(text.raw("EngineData")?).ok()?;
    let runs = engine
        .walk(&["EngineDict", "StyleRun", "RunArray"])
        .map_or(&[][..], Engine::array);
    fn style_of(run: &Engine) -> Option<&Engine> {
        run.walk(&["StyleSheet", "StyleSheetData"])
    }
    let first = style_of(runs.first()?)?;
    let size = first.get("FontSize")?.number()? * xx;
    let fonts = engine
        .walk(&["ResourceDict", "FontSet"])
        .map_or(&[][..], Engine::array);
    let font = first
        .get("Font")
        .and_then(Engine::number)
        .and_then(|i| fonts.get(i as usize))
        .and_then(|f| f.get("Name"))
        .and_then(Engine::string)
        .unwrap_or("Arial")
        .trim_end_matches('\0');
    let (family, bold, italic) = font_from_postscript(font);
    let mut color = [0, 0, 0, 255];
    if let Some(values) = first.walk(&["FillColor", "Values"]).map(Engine::array) {
        let values: Vec<f64> = values.iter().filter_map(Engine::number).collect();
        if let [_, r, g, b] = values[..] {
            color = [r, g, b, 1.0].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    let flag = |key| first.get(key).and_then(Engine::boolean).unwrap_or(false);
    let style = TextStyle {
        content: content.into(),
        family: if family.trim().is_empty() {
            TextStyle::default().family
        } else {
            family
        },
        size: size as f32,
        color,
        bold: bold || flag("FauxBold"),
        italic: italic || flag("FauxItalic"),
        underline: flag("Underline"),
        strikethrough: flag("Strikethrough"),
        path: None,
    };
    style.validate().ok()?;
    if runs.iter().skip(1).any(|run| style_of(run) != Some(first)) {
        report.add(Dropped::PhotoshopTextStyles);
    }
    let justification = engine
        .walk(&["EngineDict", "ParagraphRun", "RunArray"])
        .and_then(|runs| runs.array().first())
        .and_then(|run| run.walk(&["ParagraphSheet", "Properties", "Justification"]))
        .and_then(Engine::number)
        .unwrap_or(0.0);
    let boxed = engine
        .walk(&["EngineDict", "Rendered", "Shapes", "Children"])
        .and_then(|c| c.array().first())
        .and_then(|shape| shape.get("ShapeType"))
        .and_then(Engine::number)
        == Some(1.0);
    let tracking = first
        .get("Tracking")
        .and_then(Engine::number)
        .unwrap_or(0.0);
    let auto_leading = first
        .get("AutoLeading")
        .and_then(Engine::boolean)
        .unwrap_or(true);
    if justification != 0.0 || boxed || tracking != 0.0 || !auto_leading {
        report.add(Dropped::TextLayout);
    }
    Some(style)
}

/// A layer's effects as Xuan draws them, and what of Photoshop's could not come along.
#[derive(Default)]
struct ImportedEffects {
    effects: LayerEffects,
    /// Switched-on effects Xuan has no equivalent for: bevel and emboss, satin, gradient and
    /// pattern overlays, gradient or pattern strokes, gradient glows, several effects of one
    /// kind, colors outside RGB, or effects only in the legacy `lrFX`/`lmfx` blocks.
    left_out: bool,
    /// Settings Xuan approximates: effect blend modes other than Photoshop's defaults, spread
    /// or choke, noise, centered strokes, glows from the center, sizes beyond Xuan's ranges.
    approximated: bool,
}

/// Effects that live in `lfx2` and that Xuan does not draw.
const UNSUPPORTED_EFFECTS: [&[u8]; 4] = [b"ebbl", b"ChFX", b"GrFl", b"patternFill"];

/// Read a layer's effects (`lfx2`: a version, then a versioned descriptor of one object per
/// effect), or `None` without effects or with all of them switched off.
fn read_effects(record: &Record<'_>, global_angle: f64) -> Option<ImportedEffects> {
    use crate::layer_effects::{GlowEffect, OverlayEffect, ShadowEffect, StrokeEffect};
    use descriptor::Value;
    let left_out = ImportedEffects {
        left_out: true,
        ..ImportedEffects::default()
    };
    let Some(data) = record.get(b"lfx2") else {
        return record.has(&[b"lrFX", b"lmfx"]).then_some(left_out);
    };
    let mut reader = Reader::new(data);
    let Some(effects) = reader
        .u32()
        .ok()
        .and_then(|_| descriptor::versioned(&mut reader).ok())
    else {
        return Some(left_out);
    };
    if effects.bool("masterFXSwitch") == Some(false) {
        return None;
    }
    let scale = effects
        .double("Scl ")
        .map_or(1.0, |percent| percent / 100.0)
        .clamp(0.01, 10.0);
    let mut out = ImportedEffects::default();
    for (key, value) in &effects.items {
        let Value::Descriptor(effect) = value else {
            // Photoshop CC's `…Multi` lists hold several effects of one kind.
            if key.ends_with(b"Multi")
                && let Value::List(list) = value
                && list
                    .iter()
                    .filter(|v| matches!(v, Value::Descriptor(d) if d.bool("enab") != Some(false)))
                    .count()
                    > 1
            {
                out.left_out = true;
            }
            continue;
        };
        let enabled = effect.bool("enab").unwrap_or(true);
        let opacity = (effect.double("Opct").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32;
        let color = effect
            .object("Clr ")
            .and_then(rgb)
            .map(|[r, g, b, _]| [r, g, b]);
        let mode = effect.enumeration("Md  ");
        let mut approximated = false;
        let mut length = |key: &str, max: f64| {
            let value = effect.double(key).unwrap_or(0.0).max(0.0) * scale;
            approximated |= value > max;
            value.min(max) as f32
        };
        let size = length("Sz  ", 500.0);
        let blur = length("blur", 500.0);
        let distance = length("Dstn", 5000.0);
        let unmodeled = ["Ckmt", "Nose"]
            .iter()
            .any(|key| effect.double(key).is_some_and(|v| v != 0.0));
        let default_mode = match key.as_slice() {
            b"DrSh" | b"IrSh" => b"Mltp".as_slice(),
            b"OrGl" | b"IrGl" => b"Scrn",
            _ => b"Nrml",
        };
        approximated |= mode.is_some_and(|m| m != default_mode);
        let Some(color) = color.filter(|_| {
            !(key == b"FrFX" && effect.enumeration("PntT").is_some_and(|p| p != b"SClr"))
        }) else {
            let known = [
                b"FrFX".as_slice(),
                b"DrSh",
                b"IrSh",
                b"OrGl",
                b"IrGl",
                b"SoFi",
            ];
            out.left_out |= enabled
                && (known.contains(&key.as_slice())
                    || UNSUPPORTED_EFFECTS.contains(&key.as_slice()));
            continue;
        };
        match key.as_slice() {
            b"FrFX" => {
                let style = effect.enumeration("Styl");
                approximated |= style == Some(b"CtrF");
                out.effects.stroke = Some(StrokeEffect {
                    enabled,
                    size,
                    color,
                    opacity,
                    inside: style == Some(b"InsF"),
                });
            }
            b"DrSh" | b"IrSh" => {
                approximated |= unmodeled;
                let angle = if effect.bool("uglg") == Some(true) {
                    global_angle
                } else {
                    effect.double("lagl").unwrap_or(global_angle)
                };
                let shadow = Some(ShadowEffect {
                    enabled,
                    angle: angle.rem_euclid(360.0) as f32,
                    distance,
                    blur,
                    color,
                    opacity,
                });
                if key == b"DrSh" {
                    out.effects.drop_shadow = shadow;
                } else {
                    out.effects.inner_shadow = shadow;
                }
            }
            b"OrGl" | b"IrGl" => {
                approximated |= unmodeled;
                let glow = Some(GlowEffect {
                    enabled,
                    size: blur,
                    color,
                    opacity,
                });
                if key == b"OrGl" {
                    out.effects.outer_glow = glow;
                } else {
                    approximated |= effect.enumeration("glwS") == Some(b"SrcC");
                    out.effects.inner_glow = glow;
                }
            }
            b"SoFi" => {
                out.effects.color_overlay = Some(OverlayEffect {
                    enabled,
                    color,
                    opacity,
                })
            }
            _ => {
                out.left_out |= enabled && UNSUPPORTED_EFFECTS.contains(&key.as_slice());
                continue;
            }
        }
        out.approximated |= enabled && approximated;
    }
    if out.effects.validate().is_err() {
        out.effects = LayerEffects::default();
        out.left_out = true;
    }
    Some(out)
}

/// What one layer record became, for resolving clipping afterwards.
struct Built {
    id: Uuid,
    parent: Option<Uuid>,
    clipping: bool,
    group: bool,
    /// Kept, and a layer Xuan can clip to (a pixel layer or a folder, not an adjustment).
    base: bool,
    kept: bool,
}

/// Read a Photoshop document, reporting what Xuan changed. `budget` is what the destination may
/// still hold; when the layers exceed it, they are cropped to the canvas.
pub fn read(bytes: &[u8], budget: PixelBudget) -> Result<(Document, ImportReport)> {
    let mut reader = Reader::new(bytes);
    let header = header(&mut reader)?;
    let psb = header.psb;
    reader.section(false)?; // color mode data: empty for RGB
    let resources = resources(reader.section(false)?);
    let mut layers_and_masks = reader.section(psb)?;
    let records = if layers_and_masks.remaining() >= if psb { 8 } else { 4 } {
        let mut info = layers_and_masks.section(psb)?;
        if info.remaining() >= 2 {
            read_records(&mut info, psb)?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let mut report = ImportReport::new(ImportSource::Photoshop);
    let mut document = Document::new(header.width, header.height)?;
    document.resolution = resources.resolution;
    document.layers.clear();
    if records.is_empty() {
        ensure!(
            u64::from(header.width) * u64::from(header.height) <= budget.layers,
            too_large()
        );
        let image = merged_image(&mut reader, &header)?;
        document.layers.push(Layer::image(tr("Background"), image));
    } else {
        build(
            &mut document,
            &records,
            psb,
            budget,
            resources.global_angle,
            &mut report,
        )?;
    }
    document.active = document
        .layers
        .iter()
        .rev()
        .find(|l| l.parent.is_none())
        .map(|l| l.id);
    document.selected = document.active.into_iter().collect();
    document.validate()?;
    Ok((document, report))
}

fn build(
    document: &mut Document,
    records: &[Record<'_>],
    psb: bool,
    budget: PixelBudget,
    global_angle: f64,
    report: &mut ImportReport,
) -> Result<()> {
    let canvas = Rect {
        left: 0,
        top: 0,
        right: i64::from(document.width),
        bottom: i64::from(document.height),
    };
    let crop = !fits(records, false, canvas, budget);
    ensure!(!crop || fits(records, true, canvas, budget), too_large());
    let mut pixels_left = budget.layers;
    let mut open: Vec<Uuid> = Vec::new();
    let mut built = Vec::with_capacity(records.len());
    for record in records {
        if record.section == 3 {
            ensure!(
                open.len() < MAX_FOLDER_DEPTH,
                tr("The Photoshop file's folders are nested too deeply")
            );
            open.push(Uuid::new_v4());
            continue;
        }
        let group = is_group(record);
        let id = if group {
            open.pop().unwrap_or_else(Uuid::new_v4)
        } else {
            Uuid::new_v4()
        };
        let parent = open.last().copied();
        let mut layer = Layer::blank(record.name.clone(), document.width, document.height);
        layer.id = id;
        layer.parent = parent;
        layer.visible = !record.hidden;
        layer.group = group;
        let opacity = f32::from(record.opacity) / 255.0;
        let effects = if group {
            // Xuan's folders take no effects.
            if record.has(&[b"lfx2", b"lrFX", b"lmfx"]) {
                report.add(Dropped::LayerEffect);
            }
            None
        } else {
            read_effects(record, global_angle)
        };
        // Fill opacity dims the layer's own pixels but not its effects, which Xuan cannot
        // separate: layers with effects that draw keep their layer opacity, as upstream does.
        let drawn = effects
            .as_ref()
            .is_some_and(|e| e.left_out || !e.effects.visible().is_empty());
        layer.opacity = if group || (drawn && record.fill != 255) {
            if !group {
                report.add(Dropped::FillOpacity);
            }
            opacity
        } else {
            opacity * f32::from(record.fill) / 255.0
        };
        if group {
            let key = record.section_blend.unwrap_or(record.blend);
            if !matches!(&key, b"pass" | b"norm") {
                report.add(Dropped::FolderBlendMode(blend_name(key)));
            }
        } else {
            layer.blend = blend_mode(record.blend, report);
        }
        if is_adjustment(record) {
            match adjustment(record) {
                Ok(adjustment) => {
                    layer.adjustment = Some(adjustment);
                    // Xuan's adjustments replace the backdrop; they have no blend mode.
                    if layer.blend != BlendMode::Normal {
                        report.add(Dropped::PhotoshopBlendMode(blend_name(record.blend)));
                        layer.blend = BlendMode::Normal;
                    }
                }
                Err(name) => {
                    report.add(Dropped::PhotoshopAdjustment(name));
                    built.push(Built {
                        id,
                        parent,
                        clipping: record.clipping,
                        group: false,
                        base: false,
                        kept: false,
                    });
                    continue;
                }
            }
        }
        let image = image_rect(record, crop, canvas);
        let mut cropped = crop && image != Rect::default() && image != record.rect;
        if !image.is_empty() {
            let pixels = decode_rgba(record, image, psb)?;
            pixels_left = pixels_left.saturating_sub(image.area());
            layer.pixels = Some(Arc::new(pixels));
            layer.transform = image.transform();
        }
        if !group && layer.adjustment.is_none() {
            content(record, &mut layer, image, canvas, &mut pixels_left, report);
        }
        if let Some(mask) = record.mask {
            if mask.from_vector {
                // Shape layers are reported with their vector content.
                if !record.has(&VECTOR_KEYS) {
                    report.add(Dropped::VectorMask);
                }
            } else if let Some(grid) = mask_rect(record, image, crop, canvas) {
                cropped |= crop && mask.default == 0 && grid != mask.rect;
                layer.mask = Some(decode_mask(record, grid, psb)?);
            }
        }
        if cropped {
            report.add(Dropped::CroppedToCanvas);
        }
        if let Some(imported) = effects {
            if imported.left_out {
                report.add(Dropped::PhotoshopEffects);
            }
            if imported.approximated {
                report.add(Dropped::PhotoshopEffectSettings);
            }
            if !imported.effects.is_empty() {
                // As upstream, only layers with pixels of their own take effects.
                if layer.pixels.is_some() && !layer.is_effect() {
                    layer.effects = Some(imported.effects);
                } else {
                    report.add(Dropped::LayerEffect);
                }
            }
        }
        built.push(Built {
            id,
            parent,
            clipping: record.clipping,
            group,
            base: layer.adjustment.is_none(),
            kept: true,
        });
        document.layers.push(layer);
    }
    ensure!(open.is_empty(), damaged());
    // A clipped layer clips to the nearest unclipped layer below it in the same folder.
    let mut bases: HashMap<Option<Uuid>, Option<Uuid>> = HashMap::new();
    let mut clips = HashMap::new();
    for entry in &built {
        if !entry.clipping {
            bases.insert(entry.parent, entry.base.then_some(entry.id));
        } else if entry.kept {
            match bases.get(&entry.parent).copied().flatten() {
                Some(base) if !entry.group => {
                    clips.insert(entry.id, base);
                }
                _ => report.add(Dropped::ClippingBase),
            }
        }
    }
    for layer in &mut document.layers {
        layer.clip_to = clips.get(&layer.id).copied();
    }
    Ok(())
}

/// Type, shapes, smart objects, fill layers and vector content: what stays editable, and what is
/// reported as pixels or left out.
fn content(
    record: &Record<'_>,
    layer: &mut Layer,
    image: Rect,
    canvas: Rect,
    pixels_left: &mut u64,
    report: &mut ImportReport,
) {
    if let Some(data) = record.get(b"TySh") {
        // Xuan's text layers keep pixels; until edited they show Photoshop's rendering.
        let style = layer
            .pixels
            .is_some()
            .then(|| text_style(data, report))
            .flatten();
        match style {
            Some(style) => layer.text = Some(style),
            None => report.add(Dropped::PhotoshopTextAsPixels),
        }
        return;
    }
    if record.has(&[b"SoLd", b"SoLE", b"PlLd"]) {
        report.add(Dropped::SmartObject);
        return;
    }
    let vector = record.has(&VECTOR_KEYS);
    let fill = record.has(&FILL_KEYS);
    if vector && fill {
        // Upstream turns plain filled rectangles and ellipses into live shapes.
        if let Some((style, rect)) = live_shape(record) {
            let available = pixels_left.saturating_add(image.area());
            if rect.area() <= available
                && let Ok(drawn) = crate::paint::shape(
                    Point::default(),
                    Point::new(rect.width() as f32, rect.height() as f32),
                    style.kind,
                    style.color,
                    style.corner_radius,
                )
                && let Some(pixels) = drawn.pixels
            {
                *pixels_left = available - rect.area();
                layer.pixels = Some(pixels);
                layer.transform = rect.transform();
                layer.shape = Some(style);
                return;
            }
        }
        // Older files store shapes without pixels; upstream draws the path itself.
        if layer.pixels.is_none() {
            match draw_vector(record, canvas, *pixels_left) {
                Some((pixels, area)) => {
                    *pixels_left -= area.area();
                    layer.pixels = Some(Arc::new(pixels));
                    layer.transform = area.transform();
                }
                None => {
                    report.add(Dropped::VectorLeftOut);
                    return;
                }
            }
        }
        report.add(Dropped::VectorAsPixels);
    } else if fill {
        report.add(Dropped::FillLayer);
        // A solid fill stored without pixels covers the canvas.
        if layer.pixels.is_none()
            && let Some(color) = fill_color(record)
            && canvas.area() <= *pixels_left
        {
            *pixels_left -= canvas.area();
            layer.pixels = Some(Arc::new(RgbaImage::from_pixel(
                canvas.width() as u32,
                canvas.height() as u32,
                image::Rgba(color),
            )));
            layer.transform = canvas.transform();
        }
    } else if vector {
        report.add(Dropped::VectorMask);
    } else if record.has(&[b"vogk", b"vstk"]) {
        report.add(Dropped::VectorAsPixels);
    }
}

/// The merged image of a file without layers: compression, then planar channels.
fn merged_image(reader: &mut Reader<'_>, header: &Header) -> Result<RgbaImage> {
    ensure!(header.channels >= 3, damaged());
    let (width, height) = (header.width as usize, header.height as usize);
    let compression = reader.u16()?;
    let plane = width * height;
    let planes: Vec<Vec<u8>> = match compression {
        0 => {
            let data = reader.bytes(plane.checked_mul(3).ok_or_else(damaged)?)?;
            data.chunks_exact(plane).map(<[u8]>::to_vec).collect()
        }
        1 => {
            let rows = height
                .checked_mul(usize::from(header.channels))
                .ok_or_else(damaged)?;
            let encoded = packbits_rows(reader, rows, width, header.psb)?;
            let mut planes = vec![vec![0; plane]; 3];
            for (index, source) in encoded.iter().take(height * 3).enumerate() {
                let (channel, row) = (index / height, index % height);
                unpack_row(source, &mut planes[channel][row * width..(row + 1) * width])?;
            }
            planes
        }
        _ => bail!(tr(
            "This Photoshop file uses an unsupported compression method"
        )),
    };
    Ok(RgbaImage::from_fn(header.width, header.height, |x, y| {
        let i = y as usize * width + x as usize;
        image::Rgba([planes[0][i], planes[1][i], planes[2][i], 255])
    }))
}

#[cfg(test)]
#[path = "psd_tests.rs"]
mod tests;

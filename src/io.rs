use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use image::{DynamicImage, ImageFormat, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{
    document::{Document, MAX_SIDE, validate_size},
    render,
    watch::ContentHash,
};

const MAX_MANIFEST: u64 = 4 * 1024 * 1024;

/// Entries a project or OpenRaster archive may hold: a manifest and an image and mask for
/// every layer, with room to spare.
const MAX_ARCHIVE_ENTRIES: usize = 30_001;

/// Largest image file or project asset read whole; see [`crate::limits::Limits::file_bytes`].
pub(crate) fn max_asset() -> u64 {
    crate::limits::get().file_bytes()
}

mod compositor;
mod heif;
pub mod ora;
pub mod psd;
pub mod resolution;
pub mod svg;

pub use compositor::{Dropped, ImportReport, ImportSource};
pub use ora::is_openraster;
pub use psd::is_photoshop;
pub use svg::is_svg;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    document: Document,
    pixel_layers: HashSet<Uuid>,
}

/// Add one file to a project archive. A file of 4 GiB or more needs a ZIP64 entry, which the
/// archive only marks where it must, so smaller projects stay readable by any ZIP tool.
fn write_entry<W: Write + std::io::Seek>(
    archive: &mut ZipWriter<W>,
    name: String,
    options: SimpleFileOptions,
    bytes: &[u8],
) -> Result<()> {
    let large = bytes.len() as u64 >= u64::from(u32::MAX);
    archive.start_file(name, options.large_file(large))?;
    archive.write_all(bytes)?;
    Ok(())
}

fn decode_image(bytes: Vec<u8>, used: &mut u64) -> Result<DynamicImage> {
    ensure!(bytes.len() as u64 <= max_asset(), "Image file is too large");
    if heic_rs::ftyp::parse(&bytes).is_ok() {
        return heif::decode(&bytes, used).map(DynamicImage::ImageRgba8);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    // Room for a 16-bit RGBA decode of the largest image allowed.
    limits.max_alloc = Some(crate::limits::get().image_pixels * 8);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    use image::ImageDecoder;
    let (width, height) = decoder.dimensions();
    reserve_pixels(width, height, used)?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn reserve_pixels(width: u32, height: u32, used: &mut u64) -> Result<()> {
    validate_size(width, height)?;
    let total = used.saturating_add(u64::from(width) * u64::from(height));
    let budget = crate::limits::get().project_pixels;
    ensure!(
        total <= budget,
        "Project exceeds {} of source images, the most this computer opens",
        crate::limits::megapixels(budget)
    );
    *used = total;
    Ok(())
}

pub fn import_image(path: &Path) -> Result<RgbaImage> {
    import_image_with_resolution(path).map(|(image, _)| image)
}

/// [`import_image`], with the print resolution the file states, in pixels per inch, when it
/// states a usable one (see [`resolution::read`]). SVG and HEIC give none.
pub fn import_image_with_resolution(path: &Path) -> Result<(RgbaImage, Option<f32>)> {
    if is_svg(path) {
        return Ok((svg::load(path, svg::SvgSize::Natural)?.image, None));
    }
    let metadata = fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    // Opening a FIFO or device could block forever.
    ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let too_large = || {
        format!(
            "Image files are limited to {} on this computer",
            crate::limits::size(max_asset())
        )
    };
    ensure!(metadata.len() <= max_asset(), too_large());
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max_asset() + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= max_asset(), too_large());
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "heic" | "heif" | "hif") {
        return Ok((heif::decode(&bytes, &mut 0)?, None));
    }
    let resolution = resolution::read(&bytes);
    Ok((decode_image(bytes, &mut 0)?.to_rgba8(), resolution))
}

/// Persist a complete sibling temporary file, then atomically replace the destination.
pub fn save(document: &Document, path: &Path) -> Result<()> {
    save_hashed(document, path, |_| {})
}

/// [`save`], telling `before_replace` the hash of the bytes about to replace the destination
/// just before they do, so a watcher of the file can tell Xuan's own save from another
/// program's write (see [`crate::watch`]).
pub fn save_hashed(
    document: &Document,
    path: &Path,
    before_replace: impl FnOnce(ContentHash),
) -> Result<()> {
    document.validate()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut archive = ZipWriter::new(temporary.as_file_mut());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let manifest = Manifest {
            format: "me.silverl.xuan".into(),
            version: format_version(document),
            document: document.clone(),
            pixel_layers: document
                .layers
                .iter()
                .filter(|l| l.pixels.is_some())
                .map(|l| l.id)
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&manifest)?;
        ensure!(
            json.len() as u64 <= MAX_MANIFEST,
            "Project metadata exceeds 4 MiB"
        );
        archive.start_file("manifest.json", options)?;
        archive.write_all(&json)?;
        for layer in &document.layers {
            if let Some(raw) = &layer.raw {
                // The legacy archive suffix is shared by all RAW formats.
                write_entry(
                    &mut archive,
                    format!("raw/{}.nef", layer.id),
                    options,
                    &raw.bytes,
                )?;
            }
            // Encoded straight from the layer, without copying its pixels first.
            if let Some(pixels) = &layer.pixels {
                let mut png = Cursor::new(Vec::new());
                pixels.write_to(&mut png, ImageFormat::Png)?;
                let name = format!("images/{}.png", layer.id);
                write_entry(&mut archive, name, options, png.get_ref())?;
            }
            if let Some(mask) = &layer.mask {
                let mut png = Cursor::new(Vec::new());
                mask.pixels.write_to(&mut png, ImageFormat::Png)?;
                let name = format!("images/{}.mask.png", layer.id);
                write_entry(&mut archive, name, options, png.get_ref())?;
            }
            if let Some(crate::document::Adjustment::ColorLookup { table, .. }) = &layer.adjustment
            {
                let name = format!("luts/{}.cube", layer.id);
                write_entry(&mut archive, name, options, table.to_cube().as_bytes())?;
            }
        }
        archive.finish()?;
    }
    temporary.as_file().sync_all()?;
    let file = temporary.as_file_mut();
    file.seek(SeekFrom::Start(0))?;
    before_replace(ContentHash::read(file)?);
    temporary.persist(path).map_err(|error| error.error)?;

    // Sync the rename on Unix; Windows cannot open directories with File::open.
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

/// The newest version supported by `load`.
const LATEST_VERSION: u32 = 16;

/// The first version that records a collage's layout and cells (`document.collage`).
const COLLAGE: u32 = 16;

/// The first version with Color Lookup adjustment layers, whose tables are stored as
/// `luts/<layer UUID>.cube`.
const COLOR_LOOKUP: u32 = 15;

/// Bytes of Color Lookup tables one project may bring in when it is opened: room for 64 of
/// the largest (65³) tables.
const MAX_PROJECT_LUT_BYTES: usize = 64 * 65 * 65 * 65 * 12;

/// The first version with upstream Compositor's Vignette, Bloom, Tonal Contrast and Dither
/// filters, which older readers cannot draw.
const COMPOSITOR_FILTERS: u32 = 14;

/// The first version that stores a layer's Fill apart from its opacity.
const FILL_OPACITY: u32 = 13;

/// The first version that stores Lens Correction's vignette with Photoshop's sign (negative
/// darkens the corners); older files stored the opposite and are negated when loaded.
const PHOTOSHOP_VIGNETTE: u32 = 12;

/// The lowest format version that can hold everything `document` uses, so
/// older readers keep opening projects that do not need the newer features.
fn format_version(document: &Document) -> u32 {
    // Older readers would drop the record, and with it the collage's layout.
    if document.collage.is_some() {
        COLLAGE
    // Older readers do not know the adjustment and would refuse the whole project.
    } else if document.layers.iter().any(|l| {
        matches!(
            l.adjustment,
            Some(crate::document::Adjustment::ColorLookup { .. })
        )
    }) {
        COLOR_LOOKUP
    } else if document
        .layers
        .iter()
        .any(|l| l.filter.as_ref().is_some_and(|f| !f.is_legacy()))
    {
        COMPOSITOR_FILTERS
    // Older readers would drop the fill and draw the layer's pixels at full fill.
    } else if document.layers.iter().any(|l| l.fill < 1.0) {
        FILL_OPACITY
    // Older readers would draw the vignette with the opposite sign.
    } else if document.layers.iter().any(|l| {
        matches!(l.filter, Some(crate::effects::Filter::LensCorrection { vignette, .. }) if vignette != 0.0)
    }) {
        PHOTOSHOP_VIGNETTE
    // Older readers would drop a text layer's path and set its text in a box when edited.
    } else if document
        .layers
        .iter()
        .any(|l| l.text.as_ref().is_some_and(|t| t.path.is_some()))
    {
        11
    // Older readers would drop the paths, and cannot draw a path shape.
    } else if !document.paths.is_empty()
        || document
            .layers
            .iter()
            .any(|l| l.shape.as_ref().is_some_and(|s| s.path.is_some()))
    {
        10
    // Older readers reject a folder as a clipping base.
    } else if document.layers.iter().any(|l| {
        l.clip_to
            .is_some_and(|id| document.layers.iter().any(|b| b.id == id && b.group))
    }) {
        9
    } else if document.layers.iter().any(|l| l.provenance.is_some()) {
        8
    } else if document.layers.iter().any(|l| {
        !l.blend.is_legacy()
            || l.adjustment.as_ref().is_some_and(|a| !a.is_legacy())
            || l.effects.is_some()
    }) {
        7
    } else if document.layers.iter().any(|l| l.generated.is_some()) {
        6
    } else if !document.guides.is_empty() || document.grid.is_some() {
        5
    } else if document.layers.iter().any(|l| {
        l.filter.is_some()
            || l.parent.is_some_and(|id| {
                document
                    .layers
                    .iter()
                    .any(|p| p.id == id && p.can_attach_effects())
            })
    }) {
        4
    } else if document.layers.iter().any(|l| l.standalone_mask) {
        3
    } else if document.layers.iter().any(|l| l.raw.is_some()) {
        2
    } else {
        1
    }
}

fn zip_read<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    let file = archive
        .by_name(name)
        .with_context(|| format!("Missing project asset: {name}"))?;
    ensure!(file.size() <= limit, "Project asset exceeds size limit");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Project asset exceeds size limit"
    );
    Ok(bytes)
}

pub fn load(path: &Path) -> Result<Document> {
    if path.is_dir() {
        return load_compositor(path);
    }
    load_archive(File::open(path)?)
}

/// A `.xuan` project already read into memory, as [`load`] reads a file.
pub fn load_bytes(bytes: &[u8]) -> Result<Document> {
    load_archive(Cursor::new(bytes))
}

fn load_archive<R: Read + Seek>(reader: R) -> Result<Document> {
    let mut archive = ZipArchive::new(reader)?;
    ensure!(
        archive.len() <= MAX_ARCHIVE_ENTRIES,
        "Too many project assets"
    );
    let mut manifest: Manifest =
        serde_json::from_slice(&zip_read(&mut archive, "manifest.json", MAX_MANIFEST)?)?;
    ensure!(
        manifest.format == "me.silverl.xuan" && (1..=LATEST_VERSION).contains(&manifest.version),
        "Unsupported xuan project version"
    );
    let mut used_pixels = 0;
    let mut used_masks = 0;
    let mut used_raw = 0;
    let mut used_luts = 0;
    ensure!(manifest.document.layers.len() <= 10_000, "Too many layers");
    validate_size(manifest.document.width, manifest.document.height)?;
    for layer in &mut manifest.document.layers {
        if let Some(raw) = &mut layer.raw {
            let bytes = zip_read(&mut archive, &format!("raw/{}.nef", layer.id), max_asset())?;
            used_raw += bytes.len() as u64;
            let budget = crate::limits::get().raw_bytes;
            ensure!(
                used_raw <= budget,
                "Project exceeds {} of RAW files, the most this computer opens",
                crate::limits::size(budget)
            );
            raw.bytes = Arc::new(bytes);
            raw.validate()?;
        }
        if manifest.pixel_layers.contains(&layer.id) {
            let bytes = zip_read(
                &mut archive,
                &format!("images/{}.png", layer.id),
                max_asset(),
            )?;
            layer.pixels = Some(Arc::new(decode_image(bytes, &mut used_pixels)?.to_rgba8()));
        }
        if let Some(mask) = &mut layer.mask {
            let bytes = zip_read(
                &mut archive,
                &format!("images/{}.mask.png", layer.id),
                max_asset(),
            )?;
            mask.pixels = Arc::new(decode_image(bytes, &mut used_masks)?.to_luma8());
        }
        if let Some(crate::document::Adjustment::ColorLookup { table, .. }) = &mut layer.adjustment
        {
            let bytes = zip_read(
                &mut archive,
                &format!("luts/{}.cube", layer.id),
                crate::lut::MAX_FILE_BYTES,
            )?;
            let lut = crate::lut::Lut::parse_bytes(&bytes)
                .with_context(|| format!("Invalid colour lookup table for layer {}", layer.id))?;
            used_luts += lut.bytes();
            ensure!(
                used_luts <= MAX_PROJECT_LUT_BYTES,
                "Project holds more colour lookup tables than Xuan opens"
            );
            *table = Arc::new(lut);
        }
    }
    ensure!(
        manifest
            .pixel_layers
            .iter()
            .all(|id| manifest.document.layers.iter().any(|l| l.id == *id)),
        "Unreferenced pixel layer metadata"
    );
    if manifest.version < PHOTOSHOP_VIGNETTE {
        for layer in &mut manifest.document.layers {
            if let Some(crate::effects::Filter::LensCorrection { vignette, .. }) = &mut layer.filter
            {
                *vignette = -*vignette;
            }
        }
    }
    manifest.document.selected = manifest.document.active.into_iter().collect();
    manifest.document.validate()?;
    Ok(manifest.document)
}

fn package_read(root: &Path, relative: &Path, limit: u64) -> Result<Vec<u8>> {
    let root = root.canonicalize()?;
    let file = root.join(relative);
    let metadata = fs::symlink_metadata(&file)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= limit,
        "Unsafe or oversized project asset"
    );
    ensure!(
        file.canonicalize()?.starts_with(&root),
        "Project asset escapes its package"
    );
    let mut bytes = Vec::new();
    File::open(file)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Project asset exceeds size limit"
    );
    Ok(bytes)
}

/// Open a Compositor `.comp` package (format versions 1–11); see [`compositor::load`].
pub fn load_compositor(path: &Path) -> Result<Document> {
    compositor::load(path).map(|(document, _)| document)
}

/// Open a project like [`load`], also returning what a Compositor, Photoshop or OpenRaster
/// import left out or changed. `.xuan` projects always load completely, so their report is empty.
pub fn load_with_report(path: &Path) -> Result<(Document, ImportReport)> {
    if path.is_dir() {
        compositor::load(path)
    } else if is_photoshop(path) {
        psd::load(path, psd::PixelBudget::default())
    } else if is_openraster(path) {
        ora::load(path, psd::PixelBudget::default())
    } else {
        Ok((load(path)?, ImportReport::default()))
    }
}

/// Refuse sizes a format cannot store, before rendering anything. PNG and JPEG hold any
/// document size Xuan allows (JPEG up to [`MAX_SIDE`], which is its own limit).
fn check_export_size(extension: &str, width: u32, height: u32) -> Result<()> {
    match extension {
        "webp" => ensure!(
            width.max(height) <= 16_383,
            "WebP images are limited to 16,383 pixels a side; export PNG or TIFF instead"
        ),
        // Classic TIFF addresses its data with 32-bit offsets.
        "tif" | "tiff" => ensure!(
            u64::from(width) * u64::from(height) * 4 < u64::from(u32::MAX) - (1 << 20),
            "TIFF files are limited to 4 GiB, and this image needs {}; export PNG instead",
            crate::limits::size(u64::from(width) * u64::from(height) * 4)
        ),
        _ => {}
    }
    Ok(())
}

/// How [`export`] encodes the formats that offer a choice: the Export dialog's
/// settings, which plugins may override for one export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    /// JPEG quality, 1–100.
    pub jpeg_quality: u8,
    /// Lossy WebP quality, 1–100, used when `webp_lossless` is off.
    pub webp_quality: u8,
    /// WebP without loss, as Xuan always wrote it before lossy WebP.
    pub webp_lossless: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            jpeg_quality: 90,
            webp_quality: 85,
            webp_lossless: true,
        }
    }
}

/// The quality range of lossy JPEG and WebP exports; values outside it are clamped.
pub const EXPORT_QUALITY: std::ops::RangeInclusive<u8> = 1..=100;

/// Encode an image as WebP, keeping its alpha. Lossless uses `image`'s
/// encoder; lossy uses libwebp at `quality` (clamped to [`EXPORT_QUALITY`]).
pub fn encode_webp(image: &RgbaImage, lossless: bool, quality: u8) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if lossless {
        image::codecs::webp::WebPEncoder::new_lossless(&mut bytes).encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )?;
    } else {
        let quality = quality.clamp(*EXPORT_QUALITY.start(), *EXPORT_QUALITY.end());
        let encoded = webp::Encoder::from_rgba(image.as_raw(), image.width(), image.height())
            .encode_simple(false, f32::from(quality))
            .map_err(|error| anyhow::anyhow!("Cannot encode WebP: {error:?}"))?;
        bytes.extend_from_slice(&encoded);
    }
    Ok(bytes)
}

pub fn export(document: &Document, path: &Path, options: &ExportOptions) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    // Layered: what it flattens is reported by `ora::export` itself.
    if extension == "ora" {
        return ora::export(document, path).map(drop);
    }
    check_export_size(&extension, document.width, document.height)?;
    let image = render::unaltered(document)
        .map_or_else(|| render::render(document), |pixels| (*pixels).clone());
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    match extension.as_str() {
        "jpg" | "jpeg" => {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                temporary.as_file_mut(),
                options
                    .jpeg_quality
                    .clamp(*EXPORT_QUALITY.start(), *EXPORT_QUALITY.end()),
            );
            encoder.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(
                document.resolution.round() as u16,
            ));
            encoder.encode_image(&render::flatten_white(&image))?;
        }
        "png" => {
            let mut encoder =
                png::Encoder::new(temporary.as_file_mut(), image.width(), image.height());
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let pixels_per_meter = (document.resolution / 0.0254).round() as u32;
            encoder.set_pixel_dims(Some(png::PixelDimensions {
                xppu: pixels_per_meter,
                yppu: pixels_per_meter,
                unit: png::Unit::Meter,
            }));
            encoder.write_header()?.write_image_data(image.as_raw())?;
        }
        "tif" | "tiff" => write_tiff(temporary.as_file_mut(), &image, document.resolution)?,
        "webp" => temporary.write_all(&encode_webp(
            &image,
            options.webp_lossless,
            options.webp_quality,
        )?)?,
        _ => bail!("Export as PNG, JPEG, TIFF, WebP or OpenRaster"),
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// An uncompressed RGBA TIFF, as `image` writes one, stating `ppi` as its resolution in pixels
/// per inch (to a hundredth).
fn write_tiff(file: &mut File, image: &RgbaImage, ppi: f32) -> Result<()> {
    use tiff::{
        encoder::{Rational, TiffEncoder, colortype::RGBA8},
        tags::{ResolutionUnit, Tag},
    };
    let mut encoder = TiffEncoder::new(std::io::BufWriter::new(file))?;
    let mut tiff = encoder.new_image::<RGBA8>(image.width(), image.height())?;
    let ppi = f64::from(ppi).clamp(
        f64::from(crate::units::MIN_RESOLUTION),
        f64::from(crate::units::MAX_RESOLUTION),
    );
    let resolution = Rational {
        n: (ppi * 100.0).round() as u32,
        d: 100,
    };
    let directory = tiff.encoder();
    directory.write_tag(Tag::ResolutionUnit, ResolutionUnit::Inch)?;
    directory.write_tag(Tag::XResolution, resolution.clone())?;
    directory.write_tag(Tag::YResolution, resolution)?;
    tiff.write_data(image.as_raw())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Layer, Mask, Transform};
    use image::{GrayImage, Luma, Rgba};
    use serde_json::Value;

    #[test]
    fn exports_refuse_sizes_their_format_cannot_store() {
        assert!(check_export_size("webp", 16_383, 100).is_ok());
        let error = check_export_size("webp", 16_384, 100)
            .unwrap_err()
            .to_string();
        assert!(error.contains("16,383"), "{error}");
        assert!(check_export_size("tiff", 30_000, 30_000).is_ok());
        let error = check_export_size("tif", 40_000, 30_000)
            .unwrap_err()
            .to_string();
        assert!(error.contains("4 GiB"), "{error}");
        for extension in ["png", "jpg", "jpeg"] {
            assert!(check_export_size(extension, MAX_SIDE, MAX_SIDE).is_ok());
        }
    }

    #[test]
    fn exports_all_formats_and_png_print_resolution() {
        let temporary = tempfile::tempdir().unwrap();
        let mut doc = Document::new(12, 8).unwrap();
        doc.resolution = 300.0;
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            12,
            8,
            Rgba([180, 90, 30, 128]),
        )));
        for extension in ["png", "jpg", "tiff", "webp"] {
            let path = temporary.path().join(format!("image.{extension}"));
            export(
                &doc,
                &path,
                &ExportOptions {
                    jpeg_quality: 95,
                    ..ExportOptions::default()
                },
            )
            .unwrap();
            let image = import_image(&path).unwrap();
            assert_eq!(image.dimensions(), (12, 8));
            if extension == "jpg" {
                assert_eq!(image.get_pixel(0, 0)[3], 255);
            } else {
                assert_eq!(image.get_pixel(0, 0)[3], 128);
            }
        }
        let decoder = png::Decoder::new(std::io::BufReader::new(
            File::open(temporary.path().join("image.png")).unwrap(),
        ));
        let reader = decoder.read_info().unwrap();
        let density = reader.info().pixel_dims.unwrap();
        assert_eq!(density.xppu, 11811);
        assert_eq!(density.unit, png::Unit::Meter);
        // Opening each export again gives the document's resolution back; WebP states none.
        for (extension, expected) in [
            ("png", Some(300.0)),
            ("jpg", Some(300.0)),
            ("tiff", Some(300.0)),
            ("webp", None),
        ] {
            let path = temporary.path().join(format!("image.{extension}"));
            let (_, resolution) = import_image_with_resolution(&path).unwrap();
            assert_eq!(resolution, expected, "{extension}");
        }
    }

    #[test]
    fn tiff_exports_keep_fractional_and_extreme_resolutions() {
        let temporary = tempfile::tempdir().unwrap();
        let mut doc = Document::new(3, 2).unwrap();
        for ppi in [1.0, 72.0, 150.0, 299.5, 9600.0] {
            doc.resolution = ppi;
            let path = temporary.path().join("image.tif");
            export(&doc, &path, &ExportOptions::default()).unwrap();
            let (image, resolution) = import_image_with_resolution(&path).unwrap();
            assert_eq!(image.dimensions(), (3, 2));
            assert_eq!(resolution, Some(ppi));
        }
    }

    #[test]
    fn packed_rgba_pngs_round_trip_every_channel_unchanged() {
        // A mask map: independent data in every channel, colour under zero alpha included.
        let packed = RgbaImage::from_fn(16, 16, |x, y| {
            Rgba([
                (x * 17) as u8,
                (y * 13 + 3) as u8,
                ((x * y) % 256) as u8,
                if x < 4 { 0 } else { (x * 16 + y) as u8 },
            ])
        });
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("mask.png");
        packed.save(&source).unwrap();
        // Opened as the editor opens an image, saved as a project, reopened, exported.
        let image = import_image(&source).unwrap();
        assert_eq!(image, packed);
        let mut document = Document::new(16, 16).unwrap();
        let layer = Layer::image("mask", image);
        document.select(layer.id, false);
        document.layers = vec![layer];
        let project = temporary.path().join("mask.xuan");
        save(&document, &project).unwrap();
        let document = load(&project).unwrap();
        let exported = temporary.path().join("out.png");
        export(&document, &exported, &ExportOptions::default()).unwrap();
        assert_eq!(import_image(&exported).unwrap(), packed);
        for extension in ["tiff", "webp"] {
            let path = temporary.path().join(format!("out.{extension}"));
            export(&document, &path, &ExportOptions::default()).unwrap();
            assert_eq!(import_image(&path).unwrap(), packed, "{extension}");
        }
        // A layer that is moved, faded or masked is composited as before.
        let mut faded = document.clone();
        faded.layers[0].opacity = 0.5;
        assert!(render::unaltered(&faded).is_none());
        let mut moved = document.clone();
        moved.layers[0].transform.x = 1.0;
        assert!(render::unaltered(&moved).is_none());
        let mut masked = document;
        masked.layers[0].mask = Some(Mask::white());
        assert!(render::unaltered(&masked).is_none());
    }

    /// A photo-like test image: smooth gradients, a soft disc and grain,
    /// transparent on the left, fading in, then opaque.
    fn photo_like(width: u32, height: u32) -> RgbaImage {
        let mut seed = 0x2545_f491_u32;
        RgbaImage::from_fn(width, height, |x, y| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let grain = (seed % 25) as f32 - 12.0;
            let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
            let disc = (1.0 - ((u - 0.6).powi(2) + (v - 0.45).powi(2)).sqrt() * 3.0).max(0.0);
            let channel = |base: f32| (base + grain).clamp(0.0, 255.0) as u8;
            let alpha = (u * 4.0 - 1.0).clamp(0.0, 1.0);
            Rgba([
                channel(40.0 + 160.0 * u + 50.0 * disc),
                channel(70.0 + 120.0 * v + 30.0 * disc),
                channel(150.0 - 90.0 * u * v + 80.0 * disc),
                (alpha * 255.0).round() as u8,
            ])
        })
    }

    #[test]
    fn lossy_webp_is_much_smaller_and_keeps_transparency() {
        let image = photo_like(256, 256);
        let lossless = encode_webp(&image, true, 80).unwrap();
        let lossy = encode_webp(&image, false, 80).unwrap();
        assert!(
            lossy.len() * 3 < lossless.len(),
            "lossy {} bytes, lossless {} bytes",
            lossy.len(),
            lossless.len()
        );
        let decoded = image::load_from_memory(&lossy).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), image.dimensions());
        let mut colour_error = 0u64;
        let mut opaque = 0u64;
        for (original, decoded) in image.pixels().zip(decoded.pixels()) {
            assert!(
                original[3].abs_diff(decoded[3]) <= 2,
                "alpha {} became {}",
                original[3],
                decoded[3]
            );
            if original[3] == 255 {
                opaque += 1;
                colour_error += (0..3)
                    .map(|c| u64::from(original[c].abs_diff(decoded[c])))
                    .sum::<u64>();
            }
        }
        assert_eq!(decoded.get_pixel(0, 0)[3], 0);
        let mean = colour_error as f64 / (opaque * 3) as f64;
        assert!(mean < 10.0, "mean colour error {mean}");
        // Lower quality, smaller file.
        assert!(encode_webp(&image, false, 20).unwrap().len() < lossy.len());
    }

    #[test]
    fn lossless_webp_is_the_image_encoder_byte_for_byte() {
        let image = photo_like(64, 48);
        let mut before = Vec::new();
        DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut Cursor::new(&mut before), ImageFormat::WebP)
            .unwrap();
        // Quality does not matter without loss.
        assert_eq!(encode_webp(&image, true, 1).unwrap(), before);
        assert_eq!(encode_webp(&image, true, 100).unwrap(), before);
        assert_eq!(image::load_from_memory(&before).unwrap().to_rgba8(), image);

        let temporary = tempfile::tempdir().unwrap();
        let mut doc = Document::new(64, 48).unwrap();
        doc.layers[0].pixels = Some(Arc::new(image.clone()));
        let path = temporary.path().join("image.webp");
        export(&doc, &path, &ExportOptions::default()).unwrap();
        // One plain layer exports as its own pixels (see `render::unaltered`).
        let mut expected = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut expected), ImageFormat::WebP)
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
    }

    #[test]
    fn webp_and_jpeg_quality_is_clamped_to_its_range() {
        let image = photo_like(32, 32);
        assert_eq!(EXPORT_QUALITY, 1..=100);
        let webp = |quality| encode_webp(&image, false, quality).unwrap();
        assert_eq!(webp(0), webp(1));
        assert_eq!(webp(255), webp(100));
        assert_ne!(webp(1), webp(100));

        let temporary = tempfile::tempdir().unwrap();
        let mut doc = Document::new(32, 32).unwrap();
        doc.layers[0].pixels = Some(Arc::new(image));
        let size = |extension: &str, options: ExportOptions| {
            let path = temporary.path().join(format!("image.{extension}"));
            export(&doc, &path, &options).unwrap();
            let image = import_image(&path).unwrap();
            assert_eq!(image.dimensions(), (32, 32));
            fs::metadata(&path).unwrap().len()
        };
        let lossy = |webp_quality| ExportOptions {
            webp_quality,
            webp_lossless: false,
            ..ExportOptions::default()
        };
        assert_eq!(size("webp", lossy(0)), size("webp", lossy(1)));
        assert!(size("webp", lossy(1)) < size("webp", lossy(100)));
        let jpeg = |jpeg_quality| ExportOptions {
            jpeg_quality,
            ..ExportOptions::default()
        };
        assert_eq!(size("jpg", jpeg(0)), size("jpg", jpeg(1)));
        assert_eq!(size("jpg", jpeg(255)), size("jpg", jpeg(100)));
    }

    #[test]
    fn standalone_masks_round_trip_with_scope_and_transforms() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mask.xuan");
        let mut document = Document::new(4, 2).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            4,
            2,
            Rgba([10, 90, 180, 255]),
        )));
        crate::operations::group(&mut document);
        let mut mask = Layer::mask("Standalone", 4, 2);
        mask.opacity = 0.75;
        mask.mask = Some(Mask {
            pixels: Arc::new(GrayImage::from_fn(4, 2, |x, _| Luma([(x * 70) as u8]))),
            placement: Some(Transform {
                x: 1.0,
                ..Transform::new(4, 2)
            }),
            linked: false,
            ..Mask::white()
        });
        document.insert(mask);
        save(&document, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(render::render(&document), render::render(&loaded));
        let expected = document.active().unwrap();
        let actual = loaded.active().unwrap();
        assert!(actual.standalone_mask);
        assert_eq!(actual.parent, expected.parent);
        assert_eq!(actual.opacity, expected.opacity);
        assert_eq!(
            actual.mask.as_ref().unwrap().pixels,
            expected.mask.as_ref().unwrap().pixels
        );
        assert_eq!(
            actual.mask.as_ref().unwrap().placement,
            expected.mask.as_ref().unwrap().placement
        );
        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        let manifest: Manifest =
            serde_json::from_slice(&zip_read(&mut archive, "manifest.json", MAX_MANIFEST).unwrap())
                .unwrap();
        assert_eq!(manifest.version, 3);
    }

    #[test]
    fn portable_project_round_trip_and_atomic_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.xuan");
        let mut doc = Document::new(3, 2).unwrap();
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            3,
            2,
            Rgba([20, 40, 80, 128]),
        )));
        doc.layers[0].mask = Some(Mask {
            pixels: Arc::new(GrayImage::from_pixel(3, 2, Luma([128]))),
            ..Mask::white()
        });
        save(&doc, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(render::render(&doc), render::render(&loaded));
        doc.layers[0].name = "Renamed".into();
        save(&doc, &path).unwrap();
        assert_eq!(load(&path).unwrap().layers[0].name, "Renamed");
        doc.width = 0;
        assert!(save(&doc, &path).is_err());
        assert_eq!(load(&path).unwrap().width, 3);
    }

    fn manifest_json(path: &Path) -> Value {
        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        serde_json::from_slice(&zip_read(&mut archive, "manifest.json", MAX_MANIFEST).unwrap())
            .unwrap()
    }

    #[test]
    fn guides_and_grid_round_trip_as_version_5() {
        use crate::layout::{GridColor, GridSettings, GridStyle, Guide, GuideAxis};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("guides.xuan");
        let mut doc = Document::new(40, 30).unwrap();
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::new(40, 30)));
        save(&doc, &path).unwrap();
        // Without guides or a grid of its own, a project keeps the older version and keys.
        let plain = manifest_json(&path);
        assert_eq!(plain["version"], 1);
        assert!(plain["document"].get("guides").is_none());
        assert!(plain["document"].get("grid").is_none());

        doc.guides = vec![
            Guide::new(GuideAxis::Vertical, 12.5),
            Guide::new(GuideAxis::Horizontal, -4.0),
        ];
        let grid = GridSettings {
            spacing: 100,
            subdivisions: 4,
            color: GridColor::Custom,
            custom_color: [10, 20, 30],
            style: GridStyle::DashedLines,
            opacity: 70,
        };
        doc.grid = Some(grid);
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 5);
        assert_eq!(manifest["document"]["guides"][0]["axis"], "vertical");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.guides, doc.guides);
        assert_eq!(loaded.grid, Some(grid));

        // Guides alone also need version 5.
        doc.grid = None;
        save(&doc, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 5);
        assert_eq!(load(&path).unwrap().grid, None);
    }

    #[test]
    fn path_shapes_and_document_paths_round_trip_as_version_10() {
        use crate::vector::{FillRule, NamedPath, VectorPath};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("paths.xuan");
        let mut doc = Document::new(64, 48).unwrap();
        let outline = VectorPath::parse(
            "M 8 40 C 8 4 56 4 56 40 Z M 24 30 a 8 8 0 1 0 16 0 a 8 8 0 1 0 -16 0 Z",
        )
        .unwrap();
        let mut shape =
            crate::paint::path_shape(&outline, FillRule::Evenodd, [30, 120, 200, 230]).unwrap();
        shape.transform.rotation = 12.0;
        doc.insert(shape);
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 10);
        let stored = &manifest["document"]["layers"][1]["shape"];
        assert_eq!(stored["kind"], "Path");
        assert_eq!(stored["path"]["fill_rule"], "evenodd");
        assert!(stored["path"]["d"].as_str().unwrap().starts_with("M0,"));
        let loaded = load(&path).unwrap();
        let style = loaded.layers[1].shape.as_ref().unwrap();
        assert_eq!(style.path, doc.layers[1].shape.as_ref().unwrap().path);
        assert_eq!(render::render(&loaded), render::render(&doc));
        // Still live after loading: a new size redraws the outline.
        let mut resized = loaded.clone();
        resized.layers[1].transform.width = 96.0;
        crate::paint::refresh_shapes(&mut resized).unwrap();
        assert_eq!(resized.layers[1].pixels.as_ref().unwrap().width(), 96);

        // Document paths alone also need version 10, and keep their names and data.
        let mut plain = Document::new(8, 8).unwrap();
        save(&plain, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 1);
        assert!(manifest_json(&path)["document"].get("paths").is_none());
        plain.paths.push(NamedPath::new(
            "Hill",
            VectorPath::parse("M 0 7 Q 4 0 8 7").unwrap(),
        ));
        save(&plain, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 10);
        assert_eq!(manifest["document"]["paths"][0]["name"], "Hill");
        assert_eq!(manifest["document"]["paths"][0]["d"], "M0,7 Q4,0 8,7");
        assert_eq!(load(&path).unwrap().paths, plain.paths);

        // Broken path data, or a path shape without its outline, is refused on load.
        for (pointer, value) in [
            ("/document/paths/0/d", serde_json::json!("M 0 0 L")),
            ("/document/paths/0/name", serde_json::json!(" ")),
        ] {
            let mut bad = manifest.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            let hostile = directory.path().join("bad.xuan");
            write_manifest(&hostile, &bad);
            assert!(load(&hostile).is_err(), "{pointer}");
        }
        let mut shapeless = doc.clone();
        shapeless.layers[1].shape.as_mut().unwrap().path = None;
        assert!(shapeless.validate().is_err());
        let mut boxless = doc.clone();
        boxless.layers[1]
            .shape
            .as_mut()
            .unwrap()
            .path
            .as_mut()
            .unwrap()
            .width = 0.0;
        assert!(boxless.validate().is_err());
    }

    #[test]
    fn text_on_a_path_round_trips_as_version_11() {
        use crate::text::{PathTextOptions, TextRenderer, TextStyle, path_layer};
        use crate::vector::VectorPath;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("text.xuan");
        let mut doc = Document::new(120, 80).unwrap();
        let style = TextStyle {
            content: "Curve".into(),
            size: 20.0,
            ..Default::default()
        };
        let mut renderer = TextRenderer::default();
        let mut boxed = Layer::image("Curve", renderer.render(&style).unwrap());
        boxed.text = Some(style.clone());
        doc.insert(boxed);
        // Text in a box needs no newer version.
        save(&doc, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 1);
        let curve = VectorPath::parse("M 10 60 Q 60 0 110 60").unwrap();
        let options = PathTextOptions {
            start_offset: 50.0,
            align: crate::text::PathAlign::Center,
            size_end: Some(10.0),
            ..Default::default()
        };
        doc.insert(path_layer(&mut renderer, style, &curve, options).unwrap());
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 11);
        let stored = &manifest["document"]["layers"][2]["text"]["path"];
        assert_eq!(stored["align"], "center");
        assert_eq!(stored["size_end"], 10.0);
        assert!(stored["d"].as_str().unwrap().starts_with('M'));
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[2].text, doc.layers[2].text);
        assert_eq!(render::render(&loaded), render::render(&doc));

        // A broken text path is refused when read or checked.
        let mut layer = manifest["document"]["layers"][2].clone();
        layer["text"]["path"]["d"] = serde_json::json!("M 0 0 L");
        assert!(serde_json::from_value::<Layer>(layer).is_err());
        for (pointer, value) in [
            ("/text/path/width", serde_json::json!(0)),
            ("/text/path/opacity_end", serde_json::json!(3)),
            ("/text/path/start_offset", serde_json::json!(-500)),
        ] {
            let mut json = manifest["document"]["layers"][2].clone();
            *json.pointer_mut(pointer).unwrap() = value;
            let mut bad = doc.clone();
            let pixels = bad.layers[2].pixels.clone();
            bad.layers[2] = serde_json::from_value(json).unwrap();
            bad.layers[2].pixels = pixels;
            assert!(bad.validate().is_err(), "{pointer}");
        }
    }

    #[test]
    fn clipping_to_a_folder_round_trips_as_version_9() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("clip.xuan");
        let mut doc = Document::new(4, 2).unwrap();
        let mut group = Layer::blank("Figure", 4, 2);
        group.group = true;
        group.opacity = 0.75;
        group.mask = Some(Mask {
            pixels: Arc::new(GrayImage::from_fn(4, 2, |x, _| Luma([(x * 80) as u8]))),
            ..Mask::white()
        });
        let mut shape = Layer::image(
            "Shape",
            RgbaImage::from_fn(4, 2, |x, y| {
                Rgba([200, 30, 30, if x > y { 255 } else { 0 }])
            }),
        );
        shape.parent = Some(group.id);
        let mut clipped =
            Layer::image("Shading", RgbaImage::from_pixel(4, 2, Rgba([0, 0, 0, 255])));
        clipped.blend = crate::blend::BlendMode::Multiply;
        clipped.clip_to = Some(group.id);
        doc.layers = vec![shape, group, clipped];
        doc.active = Some(doc.layers[2].id);
        doc.validate().unwrap();
        save(&doc, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 9);
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[2].clip_to, Some(loaded.layers[1].id));
        assert!(loaded.layers[1].group);
        assert_eq!(render::render(&loaded), render::render(&doc));
        // Released, the same project needs no newer version than before.
        doc.layers[2].clip_to = None;
        save(&doc, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 1);
        assert_eq!(load(&path).unwrap().layers[2].clip_to, None);
    }

    #[test]
    fn photoshop_blend_modes_round_trip_as_version_7() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("blend.xuan");
        let mut doc = Document::new(4, 4).unwrap();
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::new(4, 4)));
        // The thirteen original modes still save as version 1.
        for mode in &crate::blend::BlendMode::ALL[..13] {
            doc.layers[0].blend = *mode;
            save(&doc, &path).unwrap();
            assert_eq!(manifest_json(&path)["version"], 1, "{}", mode.name());
        }
        for mode in &crate::blend::BlendMode::ALL[13..] {
            doc.layers[0].blend = *mode;
            save(&doc, &path).unwrap();
            assert_eq!(manifest_json(&path)["version"], 7, "{}", mode.name());
            assert_eq!(load(&path).unwrap().layers[0].blend, *mode);
        }
        assert_eq!(
            manifest_json(&path)["document"]["layers"][0]["blend"],
            "Divide"
        );
        // So do Black & White and Color Balance adjustment layers.
        doc.layers[0].blend = crate::blend::BlendMode::Normal;
        use crate::document::Adjustment;
        for adjustment in [Adjustment::BLACK_WHITE, Adjustment::COLOR_BALANCE] {
            let mut layer = Layer::blank(adjustment.name(), 4, 4);
            layer.pixels = None;
            layer.adjustment = Some(adjustment.clone());
            let mut with_layer = doc.clone();
            with_layer.layers.push(layer);
            save(&with_layer, &path).unwrap();
            assert_eq!(manifest_json(&path)["version"], 7);
            assert_eq!(load(&path).unwrap().layers[1].adjustment, Some(adjustment));
        }
        // And layer effects.
        let mut effects = crate::layer_effects::LayerEffects::default();
        for kind in crate::layer_effects::EffectKind::ALL {
            effects.add(kind, [10, 20, 30]);
        }
        effects.set_enabled(crate::layer_effects::EffectKind::InnerGlow, false);
        let mut with_effects = doc.clone();
        with_effects.layers[0].effects = Some(effects.clone());
        let effects_path = directory.path().join("effects.xuan");
        save(&with_effects, &effects_path).unwrap();
        assert_eq!(manifest_json(&effects_path)["version"], 7);
        assert_eq!(
            load(&effects_path).unwrap().layers[0].effects,
            Some(effects.clone())
        );
        // Plugin provenance alone stays version 6; with these features it is version 7,
        // and both survive the round trip.
        let generated = crate::document::Generated {
            plugin: "example.plugin".into(),
            version: "1.0.0".into(),
            action: "outline".into(),
            inputs: serde_json::json!({"radius": 3}),
            source: Some(doc.layers[0].id),
            source_hash: Some("fnv1a:0123456789abcdef".into()),
            created: "2026-10-05T12:00:00Z".into(),
        };
        let mut provenance = doc.clone();
        provenance.layers[0].generated = Some(generated.clone());
        save(&provenance, &effects_path).unwrap();
        assert_eq!(manifest_json(&effects_path)["version"], 6);
        provenance.layers[0].effects = Some(effects.clone());
        provenance.layers[0].blend = crate::blend::BlendMode::LinearDodge;
        save(&provenance, &effects_path).unwrap();
        assert_eq!(manifest_json(&effects_path)["version"], 7);
        let loaded = load(&effects_path).unwrap();
        assert_eq!(loaded.layers[0].generated, Some(generated));
        assert_eq!(loaded.layers[0].effects, Some(effects));
        assert_eq!(loaded.layers[0].blend, crate::blend::BlendMode::LinearDodge);
        // Model provenance makes it version 8, whatever else the document uses; without it
        // nothing changes, and version 7 files still open.
        let record = crate::provenance::Provenance {
            model: Some("sdxl.safetensors".into()),
            sampler: Some("euler".into()),
            steps: Some(30),
            seed: Some(42),
            cfg: Some(7.5),
            request_id: Some("r-1".into()),
            extra: [("lora".to_string(), serde_json::json!(["a"]))].into(),
            ..Default::default()
        };
        let mut recorded = provenance.clone();
        recorded.layers[0].provenance = Some(record.clone());
        let recorded_path = directory.path().join("provenance.xuan");
        save(&recorded, &recorded_path).unwrap();
        assert_eq!(manifest_json(&recorded_path)["version"], 8);
        let loaded = load(&recorded_path).unwrap();
        assert_eq!(loaded.layers[0].provenance, Some(record.clone()));
        assert_eq!(loaded.layers[0].generated, recorded.layers[0].generated);
        assert_eq!(loaded.layers[0].effects, recorded.layers[0].effects);
        assert_eq!(loaded.layers[0].blend, crate::blend::BlendMode::LinearDodge);
        // Alone it is still version 8, and a layer without it adds no key.
        let mut alone = doc.clone();
        alone.layers[0].provenance = Some(record.clone());
        save(&alone, &recorded_path).unwrap();
        assert_eq!(manifest_json(&recorded_path)["version"], 8);
        save(&provenance, &recorded_path).unwrap();
        let unused = manifest_json(&recorded_path);
        assert_eq!(unused["version"], 7);
        assert!(unused["document"]["layers"][0].get("provenance").is_none());
        assert!(load(&recorded_path).unwrap().layers[0].provenance.is_none());
        // A hostile file with a secret, an unknown key or an oversized record is refused.
        for bad in [
            serde_json::json!({"extra": {"api_key": "x"}}),
            serde_json::json!({"nope": 1}),
            serde_json::json!({"model": "x".repeat(300)}),
        ] {
            let mut manifest = manifest_json(&recorded_path);
            manifest["version"] = 8.into();
            manifest["document"]["layers"][0]["provenance"] = bad;
            let hostile = directory.path().join("hostile.xuan");
            let mut archive = ZipArchive::new(File::open(&recorded_path).unwrap()).unwrap();
            let image = zip_read(
                &mut archive,
                &format!("images/{}.png", doc.layers[0].id),
                max_asset(),
            )
            .unwrap();
            let mut writer = ZipWriter::new(File::create(&hostile).unwrap());
            writer
                .start_file("manifest.json", SimpleFileOptions::default())
                .unwrap();
            writer
                .write_all(&serde_json::to_vec(&manifest).unwrap())
                .unwrap();
            writer
                .start_file(
                    format!("images/{}.png", doc.layers[0].id),
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(&image).unwrap();
            writer.finish().unwrap();
            assert!(load(&hostile).is_err(), "{manifest}");
        }
        // Out of range settings are refused on load.
        let mut manifest = manifest_json(&path);
        manifest["document"]["layers"][1]["adjustment"]["ColorBalance"]["shadows"][0] =
            serde_json::json!(500.0);
        manifest["pixel_layers"] = serde_json::json!([doc.layers[0].id]);
        let broken = directory.path().join("broken.xuan");
        let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let image = zip_read(
            &mut archive,
            &format!("images/{}.png", doc.layers[0].id),
            max_asset(),
        )
        .unwrap();
        let mut writer = ZipWriter::new(File::create(&broken).unwrap());
        writer
            .start_file("manifest.json", SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        writer
            .start_file(
                format!("images/{}.png", doc.layers[0].id),
                SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(&image).unwrap();
        writer.finish().unwrap();
        assert!(load(&broken).is_err());
    }

    /// Before version 12 a positive Lens Correction vignette darkened the corners. Such files
    /// load with Photoshop's sign and look the same; newer files keep their sign.
    #[test]
    fn lens_correction_vignette_migrates_to_photoshop_sign_as_version_12() {
        use crate::effects::Filter;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lens.xuan");
        let mut doc = Document::new(32, 32).unwrap();
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            32,
            32,
            Rgba([128, 128, 128, 255]),
        )));
        let mut lens = Layer::blank("Lens", 32, 32);
        let darker = Filter::LensCorrection {
            distortion: 0.0,
            vignette: -40.0,
        };
        lens.filter = Some(darker.clone());
        doc.layers.push(lens);
        let expected = render::render(&doc);

        // Saving and loading the new format does not flip the sign, however often it is done.
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 12);
        let mut loaded = load(&path).unwrap();
        for _ in 0..2 {
            assert_eq!(loaded.layers[1].filter, Some(darker.clone()));
            assert_eq!(render::render(&loaded), expected);
            save(&loaded, &path).unwrap();
            loaded = load(&path).unwrap();
        }

        // Version 11 and earlier stored +40 for corners that darken.
        let old_path = directory.path().join("old.xuan");
        for version in [4, 11] {
            let mut old = manifest.clone();
            old["version"] = version.into();
            old["document"]["layers"][1]["filter"]["LensCorrection"]["vignette"] = 40.0.into();
            let mut source = ZipArchive::new(File::open(&path).unwrap()).unwrap();
            let mut writer = ZipWriter::new(File::create(&old_path).unwrap());
            writer
                .start_file("manifest.json", SimpleFileOptions::default())
                .unwrap();
            writer
                .write_all(&serde_json::to_vec(&old).unwrap())
                .unwrap();
            for index in 0..source.len() {
                let file = source.by_index_raw(index).unwrap();
                if file.name() != "manifest.json" {
                    writer.raw_copy_file(file).unwrap();
                }
            }
            writer.finish().unwrap();
            let migrated = load(&old_path).unwrap();
            assert_eq!(migrated.layers[1].filter, Some(darker.clone()), "{version}");
            let image = render::render(&migrated);
            assert_eq!(image, expected);
            // As the old formula, 1 - vignette × r² × 0.005 with r² = 2 × (31/32)², drew it.
            let old_corner = 128.0 * (1.0 - 40.0 * 2.0 * (31.0f32 / 32.0).powi(2) * 0.005);
            assert!((image.get_pixel(0, 0)[0] as f32 - old_corner).abs() <= 1.5);
            // Saved again, it is written in the new format and keeps its look.
            save(&migrated, &old_path).unwrap();
            assert_eq!(manifest_json(&old_path)["version"], 12);
            assert_eq!(
                load(&old_path).unwrap().layers[1].filter,
                Some(darker.clone())
            );
        }

        // Without a vignette nothing changes for older readers.
        doc.layers[1].filter = Some(Filter::LensCorrection {
            distortion: 10.0,
            vignette: 0.0,
        });
        save(&doc, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 4);
    }

    /// Upstream Compositor's filters save as version 14, as filter layers and as filters
    /// attached to a layer, and come back the same; documents without them keep their version.
    #[test]
    fn compositor_filters_round_trip_as_version_14() {
        use crate::effects::{DitherColors, DitherSettings, DitherStyle, Filter};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("filters.xuan");
        let dither = Filter::Dither(Box::new(DitherSettings {
            style: DitherStyle::HalftoneDots,
            colors: DitherColors::TwoColors,
            dark: [10, 20, 30],
            light: [240, 230, 220],
            characters: "#@".into(),
            ..Default::default()
        }));
        let vignette = Filter::Vignette {
            amount: 60.0,
            color: [40, 0, 80],
            midpoint: 30.0,
            roundness: -20.0,
            feather: 70.0,
            highlights: 10.0,
        };
        for filter in [vignette, Filter::BLOOM, Filter::TONAL_CONTRAST, dither] {
            let mut doc = Document::new(32, 32).unwrap();
            doc.layers[0].pixels = Some(Arc::new(RgbaImage::from_fn(32, 32, |x, y| {
                Rgba([(x * 8) as u8, (y * 8) as u8, 128, 255])
            })));
            let mut layer = Layer::blank(filter.name(), 32, 32);
            layer.filter = Some(filter.clone());
            doc.layers.push(layer);
            // The same filter attached to the image.
            let mut attached = Layer::blank(filter.name(), 32, 32);
            attached.filter = Some(filter.clone());
            attached.parent = Some(doc.layers[0].id);
            doc.layers.insert(1, attached);
            doc.validate().unwrap();
            save(&doc, &path).unwrap();
            assert_eq!(manifest_json(&path)["version"], 14, "{filter:?}");
            let loaded = load(&path).unwrap();
            assert_eq!(loaded.layers[1].filter, Some(filter.clone()));
            assert_eq!(loaded.layers[2].filter, Some(filter.clone()));
            assert_eq!(render::render(&loaded), render::render(&doc), "{filter:?}");
            // Without the new filters the document goes back to the version it needs.
            let mut plain = loaded.clone();
            for layer in &mut plain.layers[1..] {
                layer.filter = Some(Filter::GaussianBlur { radius: 1.0 });
            }
            save(&plain, &path).unwrap();
            assert_eq!(manifest_json(&path)["version"], 4);
        }
    }

    /// Rewrite the project at `path`, replacing (or with `None`, leaving out) one entry.
    fn replace_entry(path: &Path, name: &str, bytes: Option<&[u8]>) {
        let copy = path.with_extension("copy");
        fs::copy(path, &copy).unwrap();
        let mut source = ZipArchive::new(File::open(&copy).unwrap()).unwrap();
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        for index in 0..source.len() {
            let file = source.by_index_raw(index).unwrap();
            if file.name() != name {
                writer.raw_copy_file(file).unwrap();
            }
        }
        if let Some(bytes) = bytes {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    /// A Color Lookup layer needs version 15 and keeps its table in the project, so the
    /// project looks the same without the original `.cube` file. A missing or broken table
    /// is refused rather than opened as some other look.
    #[test]
    fn color_lookup_tables_round_trip_inside_the_project_as_version_15() {
        use crate::{
            document::Adjustment,
            lut::{Dimension, Interpolation, Lut},
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lookup.xuan");
        let mut doc = Document::new(16, 16).unwrap();
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::from_fn(16, 16, |x, y| {
            Rgba([(x * 16) as u8, (y * 16) as u8, 90, 255])
        })));
        let mut table = Lut::identity(Dimension::Three, 9);
        table.title = "Bleach".into();
        for entry in &mut table.table {
            *entry = [entry[2], entry[0] * 0.5 + 0.25, entry[1].powf(0.7)];
        }
        let mut layer = Layer::blank("Color Lookup", 16, 16);
        layer.opacity = 0.5;
        layer.adjustment = Some(Adjustment::ColorLookup {
            name: "bleach.cube".into(),
            interpolation: Interpolation::Trilinear,
            table: Arc::new(table.clone()),
        });
        let id = layer.id;
        doc.layers.push(layer);
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 15);
        let stored = &manifest["document"]["layers"][1]["adjustment"]["ColorLookup"];
        assert_eq!(stored["name"], "bleach.cube");
        assert_eq!(stored["interpolation"], "Trilinear");
        assert!(stored.get("table").is_none(), "{stored}");
        // The archive is closed before the file is saved over again, which Windows refuses
        // while it is open.
        let cube = {
            let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
            let mut cube = String::new();
            archive
                .by_name(&format!("luts/{id}.cube"))
                .unwrap()
                .read_to_string(&mut cube)
                .unwrap();
            cube
        };
        assert_eq!(Lut::parse(&cube).unwrap(), table);

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[1].adjustment, doc.layers[1].adjustment);
        assert_eq!(render::render(&loaded), render::render(&doc));

        // Without it the project keeps the version it needed before.
        let mut plain = loaded.clone();
        plain.layers[1].adjustment = Some(Adjustment::Invert);
        save(&plain, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 1);

        save(&doc, &path).unwrap();
        let entry = format!("luts/{id}.cube");
        replace_entry(&path, &entry, Some(b"LUT_3D_SIZE 9\n0 0 0\n"));
        let error = format!("{:#}", load(&path).unwrap_err());
        assert!(error.contains("incomplete"), "{error}");
        replace_entry(&path, &entry, None);
        let error = load(&path).unwrap_err().to_string();
        assert!(error.contains("Missing project asset"), "{error}");
    }

    /// A layer's Fill below 100% needs version 13; at 100% the key is left out and the
    /// project keeps the version it needed before.
    #[test]
    fn fill_round_trips_as_version_13() {
        use crate::layer_effects::{EffectKind, LayerEffects};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fill.xuan");
        let mut doc = Document::new(32, 32).unwrap();
        doc.layers[0].pixels = Some(Arc::new(RgbaImage::from_fn(32, 32, |x, y| {
            Rgba([
                (x * 8) as u8,
                (y * 8) as u8,
                90,
                if x > 4 { 255 } else { 0 },
            ])
        })));
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 1);
        assert!(manifest["document"]["layers"][0].get("fill").is_none());

        doc.layers[0].fill = 0.25;
        let mut effects = LayerEffects::default();
        effects.add(EffectKind::Stroke, [255, 0, 0]);
        doc.layers[0].effects = Some(effects);
        let expected = render::render(&doc);
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 13);
        assert_eq!(manifest["document"]["layers"][0]["fill"], 0.25);
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[0].fill, 0.25);
        assert_eq!(render::render(&loaded), expected);

        // With another newer feature it is still version 13.
        let mut lens = Layer::blank("Lens", 32, 32);
        lens.filter = Some(crate::effects::Filter::LensCorrection {
            distortion: 0.0,
            vignette: -10.0,
        });
        doc.layers.push(lens);
        save(&doc, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 13);
        assert_eq!(load(&path).unwrap().layers[0].fill, 0.25);

        // Back at 100%, older readers open it again.
        doc.layers.pop();
        doc.layers[0].fill = 1.0;
        save(&doc, &path).unwrap();
        assert_eq!(manifest_json(&path)["version"], 7);

        // Out of range, or on a folder, it is refused.
        doc.layers[0].fill = 1.5;
        assert!(save(&doc, &path).is_err());
        doc.layers[0].fill = f32::NAN;
        assert!(doc.validate().is_err());
        let mut folder = Layer::blank("Folder", 32, 32);
        folder.group = true;
        folder.fill = 0.5;
        doc.layers = vec![folder];
        assert!(doc.validate().is_err());
        let mut adjustment = Layer::blank("Invert", 32, 32);
        adjustment.adjustment = Some(crate::document::Adjustment::Invert);
        adjustment.fill = 0.5;
        doc.layers = vec![adjustment];
        assert!(doc.validate().is_err());
    }

    fn write_manifest(path: &Path, manifest: &Value) {
        let mut archive = ZipWriter::new(File::create(path).unwrap());
        archive
            .start_file("manifest.json", SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(&serde_json::to_vec(manifest).unwrap())
            .unwrap();
        archive.finish().unwrap();
    }

    #[test]
    fn projects_from_before_guides_still_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old.xuan");
        for version in 1..=4 {
            // Written as a release before format version 5 wrote it: no guide or grid keys.
            write_manifest(
                &path,
                &serde_json::json!({
                    "format": "me.silverl.xuan",
                    "version": version,
                    "document": {"id": Uuid::new_v4(), "width": 8, "height": 6,
                        "resolution": 72.0, "layers": [], "active": null},
                    "pixel_layers": [],
                }),
            );
            let loaded = load(&path).unwrap();
            assert_eq!((loaded.width, loaded.height), (8, 6));
            assert!(loaded.guides.is_empty() && loaded.grid.is_none());
        }
    }

    #[test]
    fn invalid_guides_or_grid_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad.xuan");
        let document = |guides: Value, grid: Value| {
            serde_json::json!({
                "format": "me.silverl.xuan",
                "version": 5,
                "document": {"id": Uuid::new_v4(), "width": 8, "height": 6, "resolution": 72.0,
                    "layers": [], "active": null, "guides": guides, "grid": grid},
                "pixel_layers": [],
            })
        };
        let id = Uuid::new_v4();
        let guide = serde_json::json!({"id": id, "axis": "vertical", "position": 3.0});
        write_manifest(&path, &document(serde_json::json!([guide]), Value::Null));
        assert_eq!(load(&path).unwrap().guides.len(), 1);
        for (guides, grid) in [
            (serde_json::json!([guide, guide]), Value::Null),
            (
                serde_json::json!([{"id": id, "axis": "vertical", "position": 5.0e7}]),
                Value::Null,
            ),
            (
                serde_json::json!([{"id": id, "axis": "diagonal", "position": 1.0}]),
                Value::Null,
            ),
            (
                serde_json::json!([]),
                serde_json::json!({"spacing": 4, "subdivisions": 8}),
            ),
            (serde_json::json!([]), serde_json::json!({"opacity": 0})),
        ] {
            write_manifest(&path, &document(guides.clone(), grid.clone()));
            assert!(load(&path).is_err(), "{guides} {grid}");
        }
        // A future version is refused rather than half read.
        let mut future = document(serde_json::json!([]), Value::Null);
        future["version"] = serde_json::json!(LATEST_VERSION + 1);
        write_manifest(&path, &future);
        assert!(load(&path).is_err());
    }

    /// A collage keeps its layout and cells as version 16; its layers need nothing newer, so
    /// the same layers without the record keep their lower version.
    #[test]
    fn collages_round_trip_as_version_16() {
        use crate::collage::{self, Layout, Template};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collage.xuan");
        let layout = Layout {
            template: Template::LargeTop,
            spacing: 6,
            border: 4,
            color: [10, 20, 30, 255],
            corner_radius: 5.0,
            ..Layout::default()
        };
        let mut doc = collage::new_document(120, 90, layout).unwrap();
        let cell = collage::cells(&doc)[2];
        let photo = Layer::image("Photo", RgbaImage::from_pixel(8, 6, Rgba([200, 0, 0, 255])));
        collage::place(&mut doc, cell, photo).unwrap();
        save(&doc, &path).unwrap();
        let manifest = manifest_json(&path);
        assert_eq!(manifest["version"], 16);
        assert_eq!(
            manifest["document"]["collage"]["layout"]["template"],
            "LargeTop"
        );
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.collage, doc.collage);
        assert_eq!(collage::cells(&loaded).len(), 4);
        assert!(!collage::is_empty(&loaded, cell));
        assert_eq!(
            crate::render::render(&loaded).as_raw(),
            crate::render::render(&doc).as_raw()
        );

        doc.collage = None;
        save(&doc, &path).unwrap();
        assert!(manifest_json(&path)["version"].as_u64().unwrap() < 16);

        // A hostile record is refused; one naming layers that are gone is kept, and laying
        // the collage out again makes them anew.
        let mut manifest = manifest_json(&path);
        let id = Uuid::new_v4();
        for (record, loads) in [
            (
                serde_json::json!({"layout": {"template": "Grid", "columns": 0, "rows": 2,
                "spacing": 0, "border": 0, "color": [0, 0, 0, 255], "corner_radius": 0.0}}),
                false,
            ),
            (
                serde_json::json!({"layout": {"template": "Mosaic", "columns": 2, "rows": 2,
                "spacing": 0, "border": 0, "color": [0, 0, 0, 255], "corner_radius": 0.0}}),
                false,
            ),
            (
                serde_json::json!({"layout": layout, "cells": [id, id]}),
                false,
            ),
            (
                serde_json::json!({"layout": layout, "background": id, "cells": [Uuid::new_v4()]}),
                true,
            ),
        ] {
            manifest["version"] = serde_json::json!(16);
            manifest["document"]["collage"] = record.clone();
            manifest["pixel_layers"] = serde_json::json!([]);
            for layer in manifest["document"]["layers"].as_array_mut().unwrap() {
                layer["shape"] = Value::Null;
            }
            write_manifest(&path, &manifest);
            let result = load(&path);
            assert_eq!(result.is_ok(), loads, "{record}");
            if let Ok(mut loaded) = result {
                assert!(collage::cells(&loaded).is_empty());
                collage::relayout(&mut loaded, layout).unwrap();
                assert_eq!(collage::cells(&loaded).len(), 4);
                loaded.validate().unwrap();
            }
        }
    }
}

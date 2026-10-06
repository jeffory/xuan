use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use image::{DynamicImage, ImageFormat, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{
    document::{Document, MAX_PIXELS, validate_size},
    render,
};

const MAX_MANIFEST: u64 = 4 * 1024 * 1024;
const MAX_ASSET: u64 = 512 * 1024 * 1024;

mod compositor;
mod heif;
pub mod psd;

pub use compositor::{Dropped, ImportReport, ImportSource};
pub use psd::is_photoshop;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    document: Document,
    pixel_layers: HashSet<Uuid>,
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, ImageFormat::Png)?;
    Ok(encoded.into_inner())
}

fn decode_image(bytes: Vec<u8>, used: &mut u64) -> Result<DynamicImage> {
    ensure!(bytes.len() as u64 <= MAX_ASSET, "Image file is too large");
    if heic_rs::ftyp::parse(&bytes).is_ok() {
        return heif::decode(&bytes, used).map(DynamicImage::ImageRgba8);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(30_000);
    limits.max_image_height = Some(30_000);
    limits.max_alloc = Some(MAX_PIXELS * 8);
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
    ensure!(
        total <= MAX_PIXELS,
        "Project exceeds 100 megapixels of source images"
    );
    *used = total;
    Ok(())
}

pub fn import_image(path: &Path) -> Result<RgbaImage> {
    let metadata = fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    // Opening a FIFO or device could block forever.
    ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        path.display()
    );
    ensure!(metadata.len() <= MAX_ASSET, "Image exceeds 512 MiB");
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_ASSET + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_ASSET, "Image exceeds 512 MiB");
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "heic" | "heif" | "hif") {
        return heif::decode(&bytes, &mut 0);
    }
    Ok(decode_image(bytes, &mut 0)?.to_rgba8())
}

/// Persist a complete sibling temporary file, then atomically replace the destination.
pub fn save(document: &Document, path: &Path) -> Result<()> {
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
                archive.start_file(format!("raw/{}.nef", layer.id), options)?;
                archive.write_all(&raw.bytes)?;
            }
            if let Some(pixels) = &layer.pixels {
                archive.start_file(format!("images/{}.png", layer.id), options)?;
                archive.write_all(&encode_png(&DynamicImage::ImageRgba8((**pixels).clone()))?)?;
            }
            if let Some(mask) = &layer.mask {
                archive.start_file(format!("images/{}.mask.png", layer.id), options)?;
                archive.write_all(&encode_png(&DynamicImage::ImageLuma8(
                    (*mask.pixels).clone(),
                ))?)?;
            }
        }
        archive.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;

    // Sync the rename on Unix; Windows cannot open directories with File::open.
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

/// The newest version supported by `load`.
const LATEST_VERSION: u32 = 11;

/// The lowest format version that can hold everything `document` uses, so
/// older readers keep opening projects that do not need the newer features.
fn format_version(document: &Document) -> u32 {
    // Older readers would drop a text layer's path and set its text in a box when edited.
    if document
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

fn zip_read(archive: &mut ZipArchive<File>, name: &str, limit: u64) -> Result<Vec<u8>> {
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
    let mut archive = ZipArchive::new(File::open(path)?)?;
    ensure!(archive.len() <= 30_001, "Too many project assets");
    let mut manifest: Manifest =
        serde_json::from_slice(&zip_read(&mut archive, "manifest.json", MAX_MANIFEST)?)?;
    ensure!(
        manifest.format == "me.silverl.xuan" && (1..=LATEST_VERSION).contains(&manifest.version),
        "Unsupported xuan project version"
    );
    let mut used_pixels = 0;
    let mut used_masks = 0;
    let mut used_raw = 0;
    ensure!(manifest.document.layers.len() <= 10_000, "Too many layers");
    validate_size(manifest.document.width, manifest.document.height)?;
    for layer in &mut manifest.document.layers {
        if let Some(raw) = &mut layer.raw {
            let bytes = zip_read(&mut archive, &format!("raw/{}.nef", layer.id), MAX_ASSET)?;
            used_raw += bytes.len() as u64;
            ensure!(
                used_raw <= crate::raw::MAX_RAW_BYTES,
                "Project exceeds 512 MiB of RAW assets"
            );
            raw.bytes = Arc::new(bytes);
            raw.validate()?;
        }
        if manifest.pixel_layers.contains(&layer.id) {
            let bytes = zip_read(&mut archive, &format!("images/{}.png", layer.id), MAX_ASSET)?;
            layer.pixels = Some(Arc::new(decode_image(bytes, &mut used_pixels)?.to_rgba8()));
        }
        if let Some(mask) = &mut layer.mask {
            let bytes = zip_read(
                &mut archive,
                &format!("images/{}.mask.png", layer.id),
                MAX_ASSET,
            )?;
            mask.pixels = Arc::new(decode_image(bytes, &mut used_masks)?.to_luma8());
        }
    }
    ensure!(
        manifest
            .pixel_layers
            .iter()
            .all(|id| manifest.document.layers.iter().any(|l| l.id == *id)),
        "Unreferenced pixel layer metadata"
    );
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

/// Open a project like [`load`], also returning what a Compositor or Photoshop import left out
/// or changed. `.xuan` projects always load completely, so their report is empty.
pub fn load_with_report(path: &Path) -> Result<(Document, ImportReport)> {
    if path.is_dir() {
        compositor::load(path)
    } else if is_photoshop(path) {
        psd::load(path, psd::PixelBudget::default())
    } else {
        Ok((load(path)?, ImportReport::default()))
    }
}

pub fn export(document: &Document, path: &Path, quality: u8) -> Result<()> {
    let image = render::render(document);
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    match extension.as_str() {
        "jpg" | "jpeg" => {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                temporary.as_file_mut(),
                quality.clamp(1, 100),
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
        "tif" | "tiff" => {
            DynamicImage::ImageRgba8(image).write_to(temporary.as_file_mut(), ImageFormat::Tiff)?
        }
        "webp" => {
            DynamicImage::ImageRgba8(image).write_to(temporary.as_file_mut(), ImageFormat::WebP)?
        }
        _ => bail!("Export as PNG, JPEG, TIFF or WebP"),
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Layer, Mask, Transform};
    use image::{GrayImage, Luma, Rgba};
    use serde_json::Value;

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
            export(&doc, &path, 95).unwrap();
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
                MAX_ASSET,
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
            MAX_ASSET,
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
}

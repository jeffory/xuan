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

pub use compositor::{Dropped, ImportReport};

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
            version: if !document.guides.is_empty() || document.grid.is_some() {
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
            },
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
        manifest.format == "me.silverl.xuan" && (1..=5).contains(&manifest.version),
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

/// Open a project like [`load`], also returning what a Compositor import left out. `.xuan`
/// projects always load completely, so their report is empty.
pub fn load_with_report(path: &Path) -> Result<(Document, ImportReport)> {
    if path.is_dir() {
        compositor::load(path)
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
        future["version"] = serde_json::json!(6);
        write_manifest(&path, &future);
        assert!(load(&path).is_err());
    }
}

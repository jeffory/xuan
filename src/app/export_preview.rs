//! The Export dialog's preview and estimated file size (issue 91). Both are made on a worker
//! thread, a short while after the settings stop changing, so dragging Quality or typing a
//! Scale never waits on rendering or encoding.
//!
//! The estimate encodes the image with the export's own encoder ([`io::encode`]). An image of
//! up to [`SAMPLE_PIXELS`] is encoded whole, which gives its size to within the metadata. A
//! larger one is estimated from tiles taken across it at full size, encoded together and scaled
//! up by area, so detail compresses as it will in the file; the dialog labels it approximate.
//! The rendered tiles and preview are kept while only the format or quality changes.

use std::{
    io::Cursor,
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

use image::RgbaImage;
use uuid::Uuid;
use xuan::{document::Document, io, render};

/// The preview's longest side, in pixels.
const PREVIEW_SIDE: f64 = 700.0;
/// Images of up to this many pixels are encoded whole; larger ones are sampled in tiles of
/// [`TILE`] pixels a side, about this many pixels of them.
pub(super) const SAMPLE_PIXELS: u64 = 1 << 20;
const TILE: u32 = 256;
/// The most pixels rendered to take the tiles from. Past it the tiles come from a render at a
/// smaller scale, which makes the estimate a little high.
const RENDER_PIXELS: u64 = 1 << 25;
/// How long the settings must rest before a new preview and estimate start.
pub(super) const DEBOUNCE: Duration = Duration::from_millis(250);

/// What a preview and an estimate are made for: the document as it is, and the export's
/// format and options.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Settings {
    pub document: Uuid,
    pub revision: u64,
    pub format: String,
    pub options: io::ExportOptions,
}

/// The rendered pixels a preview and an estimate come from, for one document revision and
/// scale.
pub(super) struct Sample {
    document: Uuid,
    revision: u64,
    scale: u16,
    /// The image, at most [`PREVIEW_SIDE`] a side.
    preview: RgbaImage,
    /// The whole image, or tiles taken across it.
    tiles: RgbaImage,
    /// The image's pixels per pixel of `tiles`: 1 when they are the whole image.
    ratio: f64,
    /// The resolution the export states.
    ppi: f32,
}

impl Sample {
    /// Renders `document` at `scale` percent.
    pub(super) fn new(document: &Document, revision: u64, scale: u16) -> Self {
        let (width, height) = io::export_size(document.width, document.height, scale);
        let pixels = u64::from(width) * u64::from(height);
        // Render at most RENDER_PIXELS, and never more than a canvas may hold.
        let limit = RENDER_PIXELS.min(xuan::limits::get().image_pixels);
        let factor = (limit as f64 / pixels as f64).sqrt().min(1.0);
        let side = |side: u32| ((f64::from(side) * factor).round() as u32).max(1);
        let image = render::render_scaled(document, side(width), side(height));
        let preview = PREVIEW_SIDE / f64::from(image.width().max(image.height()));
        let preview = if preview < 1.0 {
            let side = |side: u32| ((f64::from(side) * preview).round() as u32).max(1);
            image::imageops::thumbnail(&image, side(image.width()), side(image.height()))
        } else {
            image.clone()
        };
        let tiles = tiles(&image);
        let ratio = pixels as f64 / (f64::from(tiles.width()) * f64::from(tiles.height()));
        Self {
            document: document.id,
            revision,
            scale,
            preview,
            tiles,
            ratio,
            ppi: io::export_resolution(document.resolution, scale),
        }
    }

    /// Whether it shows the image `settings` exports.
    fn fits(&self, settings: &Settings) -> bool {
        (self.document, self.revision, self.scale)
            == (settings.document, settings.revision, settings.options.scale)
    }

    /// The file's size in bytes as `format` (png, jpg, tiff, webp); `None` for other formats.
    pub(super) fn bytes(&self, format: &str, options: &io::ExportOptions) -> Option<u64> {
        let mut encoded = Cursor::new(Vec::new());
        io::encode(&mut encoded, &self.tiles, format, options, self.ppi).ok()?;
        Some((encoded.get_ref().len() as f64 * self.ratio).round() as u64)
    }

    /// The preview as `format` will look: lossy JPEG and WebP are encoded and decoded again.
    fn preview(&self, format: &str, options: &io::ExportOptions) -> RgbaImage {
        let lossy = format == "jpg" || (format == "webp" && !options.webp_lossless);
        let mut encoded = Cursor::new(Vec::new());
        if lossy
            && io::encode(&mut encoded, &self.preview, format, options, self.ppi).is_ok()
            && let Ok(decoded) = image::load_from_memory(encoded.get_ref())
        {
            return decoded.to_rgba8();
        }
        self.preview.clone()
    }
}

/// `image` itself when it has at most [`SAMPLE_PIXELS`]; otherwise tiles of it, spread evenly
/// across and down it, side by side.
fn tiles(image: &RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    if u64::from(width) * u64::from(height) <= SAMPLE_PIXELS {
        return image.clone();
    }
    let (tile_width, tile_height) = (TILE.min(width), TILE.min(height));
    let count = SAMPLE_PIXELS / (u64::from(tile_width) * u64::from(tile_height));
    let columns = ((count as f64).sqrt().ceil() as u32).clamp(1, width / tile_width);
    let rows = ((count / u64::from(columns)) as u32).clamp(1, height / tile_height);
    let mut tiles = RgbaImage::new(columns * tile_width, rows * tile_height);
    let at = |index: u32, count: u32, room: u32| {
        u64::from(room) * u64::from(index) / u64::from((count - 1).max(1))
    };
    for row in 0..rows {
        for column in 0..columns {
            let x = at(column, columns, width - tile_width) as u32;
            let y = at(row, rows, height - tile_height) as u32;
            let tile = image::imageops::crop_imm(image, x, y, tile_width, tile_height);
            image::imageops::replace(
                &mut tiles,
                &*tile,
                i64::from(column * tile_width),
                i64::from(row * tile_height),
            );
        }
    }
    tiles
}

/// A finished preview and estimate.
pub(super) struct Estimate {
    pub settings: Settings,
    pub preview: RgbaImage,
    /// The file's estimated size; `None` for formats it is not estimated for (OpenRaster,
    /// plugin formats) or when the image cannot be made.
    pub bytes: Option<u64>,
}

/// The Export dialog's preview and estimate: the one shown, the one being made, and the
/// settings asked for.
#[derive(Default)]
pub(super) struct ExportPreview {
    /// The settings last asked for, and when they changed.
    wanted: Option<(Settings, Instant)>,
    running: Option<Receiver<(Estimate, Arc<Sample>)>>,
    sample: Option<Arc<Sample>>,
    /// The settings of the estimate shown, and its size.
    pub shown: Option<(Settings, Option<u64>)>,
}

impl ExportPreview {
    /// Forgets the preview and estimate, when the Export dialog opens: they are made again at
    /// once, without waiting.
    pub(super) fn reset(&mut self) {
        // One still being made is for the dialog as it was: let it finish unseen.
        self.running = None;
        self.wanted = None;
        self.shown = None;
        self.sample = None;
    }

    /// The size estimated for `settings`, once it is ready.
    pub(super) fn bytes(&self, settings: &Settings) -> Option<Option<u64>> {
        self.shown
            .as_ref()
            .filter(|(shown, _)| shown == settings)
            .map(|(_, bytes)| *bytes)
    }

    /// Whether nothing is being made or waiting to be: the preview shows the settings.
    pub(super) fn settled(&self) -> bool {
        self.running.is_none()
            && self.shown.as_ref().map(|(settings, _)| settings)
                == self.wanted.as_ref().map(|(settings, _)| settings)
    }

    /// Called each frame the dialog shows, with the settings and the document: takes in a
    /// finished estimate, returning its preview, and starts the next one when the settings
    /// have rested for [`DEBOUNCE`] (at once when none is shown yet).
    pub(super) fn poll(
        &mut self,
        ctx: &egui::Context,
        settings: Settings,
        document: &Document,
    ) -> Option<RgbaImage> {
        let mut preview = None;
        if let Some(receive) = &self.running {
            match receive.try_recv() {
                Ok((estimate, sample)) => {
                    self.sample = Some(sample);
                    self.shown = Some((estimate.settings, estimate.bytes));
                    preview = Some(estimate.preview);
                    self.running = None;
                }
                Err(TryRecvError::Disconnected) => self.running = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self
            .wanted
            .as_ref()
            .is_none_or(|(wanted, _)| *wanted != settings)
        {
            self.wanted = Some((settings, Instant::now()));
        }
        let (wanted, since) = self.wanted.clone()?;
        let shown = self.shown.as_ref().map(|(settings, _)| settings);
        if self.running.is_some() || shown == Some(&wanted) {
            return preview;
        }
        let waited = since.elapsed();
        if shown.is_some() && waited < DEBOUNCE {
            ctx.request_repaint_after(DEBOUNCE - waited);
            return preview;
        }
        let sample = self.sample.clone().filter(|sample| sample.fits(&wanted));
        // Rendering needs the document only when the sample is for another revision or scale.
        let document = sample.is_none().then(|| document.clone());
        let (send, receive) = mpsc::channel();
        let repaint = ctx.clone();
        xuan::gpu::spawn(move || {
            let sample = sample.unwrap_or_else(|| {
                let document = document.expect("a document to render");
                Arc::new(Sample::new(
                    &document,
                    wanted.revision,
                    wanted.options.scale,
                ))
            });
            let estimate = estimate(&sample, wanted);
            let _ = send.send((estimate, sample));
            repaint.request_repaint();
        });
        self.running = Some(receive);
        preview
    }
}

/// The preview and size of an export with `settings`, from `sample`.
pub(super) fn estimate(sample: &Sample, settings: Settings) -> Estimate {
    let format = settings.format.as_str();
    let bytes = ["png", "jpg", "tiff", "webp"]
        .contains(&format)
        .then(|| sample.bytes(format, &settings.options))
        .flatten();
    Estimate {
        preview: sample.preview(format, &settings.options),
        bytes,
        settings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grainy, like a photo, so that estimates from tiles have detail to get right.
    fn photo(width: u32, height: u32) -> RgbaImage {
        let mut seed = 0x2545_f491_u32;
        RgbaImage::from_fn(width, height, |x, y| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let grain = (seed % 21) as i32 - 10;
            let channel = |base: u32| (base as i32 + grain).clamp(0, 255) as u8;
            image::Rgba([
                channel(40 + 160 * x / width),
                channel(60 + 120 * y / height),
                channel(200 - 100 * (x + y) / (width + height)),
                255,
            ])
        })
    }

    fn document(width: u32, height: u32) -> Document {
        let mut document = Document::new(width, height).unwrap();
        document.resolution = 300.0;
        document.layers[0].pixels = Some(Arc::new(photo(width, height)));
        document
    }

    fn settings(document: &Document, format: &str, options: io::ExportOptions) -> Settings {
        Settings {
            document: document.id,
            revision: 0,
            format: format.into(),
            options,
        }
    }

    /// The size of the file `io::export` writes.
    fn exported(document: &Document, format: &str, options: &io::ExportOptions) -> u64 {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("image.{format}"));
        io::export(document, &path, options).unwrap();
        std::fs::metadata(&path).unwrap().len()
    }

    #[test]
    fn small_images_are_estimated_exactly_with_the_export_encoder() {
        let document = document(120, 80);
        let sample = Sample::new(&document, 0, 100);
        let lossy = io::ExportOptions {
            webp_lossless: false,
            ..io::ExportOptions::default()
        };
        for (format, options) in [
            ("png", io::ExportOptions::default()),
            ("jpg", io::ExportOptions::default()),
            ("tiff", io::ExportOptions::default()),
            ("webp", io::ExportOptions::default()),
            ("webp", lossy),
        ] {
            let estimate = estimate(&sample, settings(&document, format, options));
            assert_eq!(
                estimate.bytes,
                Some(exported(&document, format, &options)),
                "{format} {options:?}"
            );
            assert_eq!(estimate.preview.dimensions(), (120, 80));
        }
        // Layered OpenRaster and plugin formats have no estimate.
        let options = io::ExportOptions::default();
        assert_eq!(
            estimate(&sample, settings(&document, "ora", options)).bytes,
            None
        );
        assert_eq!(
            estimate(&sample, settings(&document, "foo", options)).bytes,
            None
        );
    }

    #[test]
    fn scaled_exports_are_estimated_at_their_size() {
        let document = document(120, 80);
        let options = io::ExportOptions {
            scale: 50,
            ..io::ExportOptions::default()
        };
        let sample = Sample::new(&document, 0, 50);
        let estimate = estimate(&sample, settings(&document, "png", options));
        assert_eq!(estimate.preview.dimensions(), (60, 40));
        assert_eq!(estimate.bytes, Some(exported(&document, "png", &options)));
    }

    #[test]
    fn large_images_are_estimated_from_tiles_across_them() {
        let image = photo(2000, 1500);
        let sampled = tiles(&image);
        let pixels = u64::from(sampled.width()) * u64::from(sampled.height());
        assert!(pixels <= SAMPLE_PIXELS, "{:?}", sampled.dimensions());
        assert!(pixels >= SAMPLE_PIXELS / 2, "{:?}", sampled.dimensions());
        // The first tile is the top left corner, the last the bottom right.
        assert_eq!(sampled.get_pixel(0, 0), image.get_pixel(0, 0));
        let (w, h) = sampled.dimensions();
        assert_eq!(sampled.get_pixel(w - 1, h - 1), image.get_pixel(1999, 1499));
        // A strip narrower than a tile still gives tiles of its own height.
        let strip = tiles(&photo(20_000, 100));
        assert_eq!(strip.height(), 100);

        let document = document(2000, 1500);
        let sample = Sample::new(&document, 0, 100);
        assert_eq!(sample.preview.dimensions(), (700, 525));
        for format in ["png", "jpg", "tiff"] {
            let options = io::ExportOptions::default();
            let estimated = estimate(&sample, settings(&document, format, options))
                .bytes
                .unwrap() as f64;
            let actual = exported(&document, format, &options) as f64;
            assert!(
                (estimated / actual - 1.0).abs() < 0.1,
                "{format}: estimated {estimated}, wrote {actual}"
            );
        }
    }

    #[test]
    fn settings_are_estimated_once_they_rest() {
        let ctx = egui::Context::default();
        let document = document(40, 30);
        let mut preview = ExportPreview::default();
        let png = settings(&document, "png", io::ExportOptions::default());
        let wait = |preview: &mut ExportPreview, settings: &Settings| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(image) = preview.poll(&ctx, settings.clone(), &document) {
                    return image;
                }
                assert!(Instant::now() < deadline, "no preview");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // The first estimate starts at once.
        assert_eq!(wait(&mut preview, &png).dimensions(), (40, 30));
        assert!(preview.settled());
        assert!(preview.bytes(&png).unwrap().is_some());

        // A change waits for the settings to rest; until then the old size is not shown as
        // the new one's.
        let jpeg = settings(&document, "jpg", io::ExportOptions::default());
        let changed = Instant::now();
        assert!(preview.poll(&ctx, jpeg.clone(), &document).is_none());
        assert!(!preview.settled());
        assert_eq!(preview.bytes(&jpeg), None);
        wait(&mut preview, &jpeg);
        assert!(changed.elapsed() >= DEBOUNCE);
        assert!(preview.bytes(&jpeg).unwrap().is_some());
        assert!(preview.settled());
    }
}

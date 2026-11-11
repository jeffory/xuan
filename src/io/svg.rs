//! SVG and SVGZ import. A file is drawn with resvg at the size it is placed at, so it stays sharp
//! at any size: its own size when opened, the canvas it is imported into otherwise.
//!
//! Nothing outside the file is read: `<image>` links to files or URLs are refused (resvg never
//! fetches URLs, and relative paths have no base directory), and stylesheets and fonts are not
//! loaded. Resvg is built without text shaping or bitmap decoding, so text and embedded bitmaps
//! are not drawn; [`SvgImage::notice`] tells the user when a file had them.
use std::{
    fs::File,
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;
use resvg::{tiny_skia, usvg};

use crate::{document::validate_size, i18n::tr};

/// Largest SVG file read, and largest SVGZ file once decompressed.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Most XML nodes a file may have; usvg itself stops at a million elements.
const MAX_NODES: u32 = 4_000_000;
/// Longest a file may take to parse and draw before the import gives up.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Long side of a drawing that gives neither a size nor a `viewBox`.
pub const FALLBACK_SIDE: u32 = 1024;

/// Whether `path` names an SVG or SVGZ file, by its extension in any case.
pub fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"))
}

/// The size to draw a file at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgSize {
    /// The file's own size: `width` and `height`, else its `viewBox`, else its drawing scaled to
    /// [`FALLBACK_SIDE`] on the long side.
    Natural,
    /// As large as fits in `width` × `height`, keeping the file's aspect ratio.
    Fit { width: u32, height: u32 },
}

/// A drawn SVG file, with what it held that was not drawn.
#[derive(Clone, Debug)]
pub struct SvgImage {
    pub image: RgbaImage,
    /// `<text>` elements, which are not drawn.
    pub text: usize,
    /// `<image>` elements whose picture is a bitmap or lies outside the file.
    pub images: usize,
}

impl SvgImage {
    /// A short, translated notice for the user, or `None` when everything was drawn.
    pub fn notice(&self) -> Option<String> {
        if self.text == 0 && self.images == 0 {
            return None;
        }
        let mut text =
            tr("Imported with changes. Xuan does not draw these parts of the SVG file:").to_owned();
        for (label, count) in [
            (tr("Text (convert it to outlines first)"), self.text),
            (tr("Bitmaps and linked images"), self.images),
        ] {
            if count > 0 {
                text.push_str(&format!("\n• {label}: {count}"));
            }
        }
        Some(text)
    }
}

/// Read and draw the SVG or SVGZ file at `path`.
pub fn load(path: &Path, size: SvgSize) -> Result<SvgImage> {
    let bytes = read(path, MAX_BYTES)?;
    rasterize(bytes, size, TIMEOUT)
}

fn too_large(cap: u64) -> String {
    format!("SVG files are limited to {}", crate::limits::size(cap))
}

fn read(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    // Opening a FIFO or device could block forever.
    ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        path.display()
    );
    ensure!(metadata.len() <= cap, too_large(cap));
    let mut bytes = Vec::new();
    File::open(path)?.take(cap + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= cap, too_large(cap));
    Ok(bytes)
}

/// Decompress SVGZ (gzip) data, refusing more than `cap` bytes of SVG.
fn decompress(bytes: &[u8], cap: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take(cap + 1)
        .read_to_end(&mut data)
        .context("The SVGZ file is damaged")?;
    ensure!(data.len() as u64 <= cap, too_large(cap));
    Ok(data)
}

/// Parse and draw on a worker thread, so a file that takes too long is refused instead of
/// holding the window. The worker cannot be stopped; it ends on its own and its result is
/// dropped.
fn rasterize(bytes: Vec<u8>, size: SvgSize, timeout: Duration) -> Result<SvgImage> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("svg import".into())
        .spawn(move || {
            let _ = sender.send(draw(&bytes, size));
        })?;
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => bail!(
            "The SVG file took longer than {} seconds to draw",
            timeout.as_secs()
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => bail!("The SVG file could not be drawn"),
    }
}

/// Whether a `width` or `height` attribute gives a length; a percentage is relative to a
/// viewport the file does not have.
fn absolute(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty() && !v.trim().ends_with('%'))
}

fn draw(bytes: &[u8], size: SvgSize) -> Result<SvgImage> {
    let data = if bytes.starts_with(&[0x1f, 0x8b]) {
        decompress(bytes, MAX_BYTES)?
    } else {
        bytes.to_vec()
    };
    let text = std::str::from_utf8(&data).context("The SVG file is not UTF-8 text")?;
    let xml = usvg::roxmltree::Document::parse_with_options(
        text,
        usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: MAX_NODES,
        },
    )
    .context("The SVG file is damaged")?;
    let root = xml.root_element();
    ensure!(
        root.tag_name().name() == "svg",
        "The file is not an SVG drawing"
    );
    // usvg sizes such a file by its drawing, from the origin to the drawing's far corner.
    let no_size = root.attribute("viewBox").is_none()
        && !(absolute(root.attribute("width")) && absolute(root.attribute("height")));
    let texts = xml
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "text")
        .count();

    let images = AtomicUsize::new(0);
    let options = usvg::Options {
        // Relative links resolve against nothing, and the string resolver refuses them anyway.
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|mime, data, options| {
                if mime == "image/svg+xml" {
                    // An embedded SVG is parsed with resolvers that load nothing at all.
                    return (usvg::ImageHrefResolver::default_data_resolver())(mime, data, options);
                }
                // Bitmaps cannot be drawn without resvg's bitmap decoders.
                images.fetch_add(1, Ordering::Relaxed);
                None
            }),
            resolve_string: Box::new(|_, _| {
                // A file path or URL: never read or fetched.
                images.fetch_add(1, Ordering::Relaxed);
                None
            }),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_xmltree(&xml, &options).map_err(|error| match error {
        usvg::Error::InvalidSize => anyhow::anyhow!("The SVG file has no valid size"),
        error => anyhow::anyhow!("The SVG file is damaged: {error}"),
    })?;
    let own = tree.size();
    let (mut width, mut height) = (own.width(), own.height());
    if no_size && size == SvgSize::Natural {
        let scale = FALLBACK_SIDE as f32 / width.max(height);
        (width, height) = (width * scale, height * scale);
    }
    let (width, height) = match size {
        SvgSize::Natural => (width, height),
        SvgSize::Fit {
            width: bound_w,
            height: bound_h,
        } => {
            let scale = (bound_w as f32 / width).min(bound_h as f32 / height);
            (
                (width * scale).min(bound_w as f32),
                (height * scale).min(bound_h as f32),
            )
        }
    };
    ensure!(
        width.is_finite() && height.is_finite(),
        "The SVG file has no valid size"
    );
    // `as` saturates, so a huge size fails the check below.
    let (width, height) = (
        (width.round() as u32).max(1),
        (height.round() as u32).max(1),
    );
    validate_size(width, height)?;
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).context("Not enough memory to draw the SVG file")?;
    let transform =
        tiny_skia::Transform::from_scale(width as f32 / own.width(), height as f32 / own.height());
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.take();
    // tiny-skia stores premultiplied alpha; layers do not.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        if alpha != 0 && alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    let image = RgbaImage::from_raw(width, height, pixels).context("Invalid SVG pixel buffer")?;
    Ok(SvgImage {
        image,
        text: texts,
        images: images.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
#[path = "svg_tests.rs"]
mod tests;

//! The red, green, blue and alpha channels of the active layer, as in Photoshop's Channels
//! panel. Xuan is layer-based, so channels belong to the active pixel layer rather than to the
//! whole document.
//!
//! Two session states use a [`Channels`] set (neither is saved in the project):
//! - the *targets*, which edits may change. [`protect`] puts the other channels back after an
//!   edit, so painting, Fill, destructive adjustments, filters and plugins all write only the
//!   targeted channels without each knowing about them;
//! - the *view*, which channels the canvas shows. [`view_document`] stands in for the document
//!   while anything other than the colour composite is shown.
use std::sync::Arc;

use anyhow::{Result, ensure};
use image::{GrayImage, Luma, RgbaImage};
use rayon::prelude::*;

use crate::document::{Document, Layer, Point, Transform};

/// One of a layer's four channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Red,
    Green,
    Blue,
    Alpha,
}

impl Channel {
    pub const ALL: [Self; 4] = [Self::Red, Self::Green, Self::Blue, Self::Alpha];

    /// The channel's byte in an RGBA pixel.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Untranslated name; pass it through `tr` to show it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
            Self::Alpha => "Alpha",
        }
    }

    /// One letter, as in the pane header: R, G, B or A.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Red => "R",
            Self::Green => "G",
            Self::Blue => "B",
            Self::Alpha => "A",
        }
    }
}

/// A set of channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Channels(u8);

impl Default for Channels {
    fn default() -> Self {
        Self::ALL
    }
}

impl Channels {
    pub const NONE: Self = Self(0);
    /// Red, green and blue: the colour composite.
    pub const COLOR: Self = Self(0b0111);
    pub const ALL: Self = Self(0b1111);

    pub const fn only(channel: Channel) -> Self {
        Self(1 << channel as u8)
    }

    pub fn contains(self, channel: Channel) -> bool {
        self.0 & (1 << channel as u8) != 0
    }

    pub fn with(self, channel: Channel, on: bool) -> Self {
        if on {
            Self(self.0 | 1 << channel as u8)
        } else {
            Self(self.0 & !(1 << channel as u8))
        }
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The channel when the set holds exactly one.
    pub fn single(self) -> Option<Channel> {
        let mut channels = self.iter();
        let first = channels.next()?;
        channels.next().is_none().then_some(first)
    }

    pub fn iter(self) -> impl Iterator<Item = Channel> {
        Channel::ALL.into_iter().filter(move |c| self.contains(*c))
    }

    /// The letters of the channels in the set, such as "G" or "RB".
    pub fn letters(self) -> String {
        self.iter().map(Channel::letter).collect()
    }
}

/// Whether edits to `layer`'s pixels can be limited to some channels: an unlocked pixel layer
/// whose pixels are its own, not drawn from text, a shape or a RAW photo.
pub fn editable(layer: &Layer) -> bool {
    layer.can_attach_effects()
        && layer.text.is_none()
        && layer.shape.is_none()
        && layer.raw.is_none()
}

/// Where pixel (0, 0) of a `new` grid lies in an `old` grid, when every pixel of the new grid
/// lands exactly on one of the old grid (the same scale, rotation and flips; a stroke that grew
/// the layer only adds pixels around it).
fn grid_offset(
    old: Transform,
    old_size: (u32, u32),
    new: Transform,
    new_size: (u32, u32),
) -> Option<(i64, i64)> {
    let old_pixel = |x: f32, y: f32| {
        let point = new.point(Point::new(
            (x + 0.5) / new_size.0 as f32,
            (y + 0.5) / new_size.1 as f32,
        ));
        let unit = old.inverse(point);
        (
            unit.x * old_size.0 as f32 - 0.5,
            unit.y * old_size.1 as f32 - 0.5,
        )
    };
    let (ox, oy) = old_pixel(0.0, 0.0);
    let (dx, dy) = (ox.round(), oy.round());
    let (right, bottom) = ((new_size.0 - 1) as f32, (new_size.1 - 1) as f32);
    let close = |(x, y): (f32, f32), (ex, ey): (f32, f32)| {
        x.is_finite() && y.is_finite() && (x - ex).abs() < 0.05 && (y - ey).abs() < 0.05
    };
    (close((ox, oy), (dx, dy))
        && close(old_pixel(right, 0.0), (dx + right, dy))
        && close(old_pixel(0.0, bottom), (dx, dy + bottom))
        && close(old_pixel(right, bottom), (dx + right, dy + bottom)))
    .then_some((dx as i64, dy as i64))
}

/// Put back the channels outside `targets` in the active layer, as they were in `original`
/// (the document before the edit), so the edit changed only the targeted channels.
///
/// Only an edit of the active pixel layer's pixels is limited: one that leaves the canvas, the
/// layers and the active layer as they were, and keeps the pixel grid (a stroke may grow the
/// layer, whose new pixels start transparent). Anything else, such as resizing the image or
/// merging layers, is left whole. Returns whether a channel was put back.
pub fn protect(document: &mut Document, original: &Document, targets: Channels) -> bool {
    if targets == Channels::ALL
        || targets.is_empty()
        || (document.width, document.height) != (original.width, original.height)
        || document.layers.len() != original.layers.len()
        || document
            .layers
            .iter()
            .zip(&original.layers)
            .any(|(a, b)| a.id != b.id)
        || document.active.is_none()
        || document.active != original.active
    {
        return false;
    }
    let Some(before) = original.active() else {
        return false;
    };
    let before_pixels = before.pixels.clone();
    let before_transform = before.transform;
    let Some(after) = document.active_mut() else {
        return false;
    };
    if !editable(after) {
        return false;
    }
    let after_transform = after.transform;
    let Some(pixels) = &mut after.pixels else {
        return false;
    };
    if before_pixels
        .as_ref()
        .is_some_and(|old| Arc::ptr_eq(old, pixels))
    {
        return false;
    }
    let old = match &before_pixels {
        Some(old) => {
            match grid_offset(
                before_transform,
                old.dimensions(),
                after_transform,
                pixels.dimensions(),
            ) {
                Some(offset) => Some((old, offset)),
                None => return false,
            }
        }
        None => None,
    };
    let keep: Vec<usize> = Channel::ALL
        .into_iter()
        .filter(|c| !targets.contains(*c))
        .map(Channel::index)
        .collect();
    let image = Arc::make_mut(pixels);
    let width = image.width() as usize;
    image
        .as_mut()
        .par_chunks_exact_mut(width * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let previous = old
                    .and_then(|(old, (dx, dy))| {
                        let ox = u32::try_from(x as i64 + dx).ok()?;
                        let oy = u32::try_from(y as i64 + dy).ok()?;
                        (ox < old.width() && oy < old.height()).then(|| old.get_pixel(ox, oy).0)
                    })
                    .unwrap_or([0; 4]);
                for &c in &keep {
                    pixel[c] = previous[c];
                }
            }
        });
    true
}

/// One channel of an image, as grey.
pub fn extract(image: &RgbaImage, channel: Channel) -> GrayImage {
    let index = channel.index();
    GrayImage::from_raw(
        image.width(),
        image.height(),
        image.pixels().map(|p| p[index]).collect(),
    )
    .expect("one byte per pixel")
}

/// What the canvas shows of `image` when the channels in `view` are visible: one channel as
/// grey; several colour channels in their colours (the others black); alpha with colour
/// channels as a translucent red overlay where the layer is transparent, as Photoshop shows
/// a quick mask. The result is opaque.
pub fn view_pixels(image: &RgbaImage, view: Channels) -> RgbaImage {
    let colors: Vec<usize> = [Channel::Red, Channel::Green, Channel::Blue]
        .into_iter()
        .filter(|c| view.contains(*c))
        .map(Channel::index)
        .collect();
    let alpha = view.contains(Channel::Alpha);
    let mut result = RgbaImage::new(image.width(), image.height());
    for (source, target) in image.pixels().zip(result.pixels_mut()) {
        let mut pixel = if colors.len() == 1 {
            [source[colors[0]]; 3]
        } else if colors.is_empty() {
            [if alpha { source[3] } else { 0 }; 3]
        } else {
            let mut pixel = [0; 3];
            for &c in &colors {
                pixel[c] = source[c];
            }
            pixel
        };
        if alpha && !colors.is_empty() {
            // Half-strength red over what the alpha leaves transparent.
            let cover = f32::from(255 - source[3]) / 255.0 * 0.5;
            let red = [255.0, 0.0, 0.0];
            for (value, red) in pixel.iter_mut().zip(red) {
                *value = (f32::from(*value) * (1.0 - cover) + red * cover).round() as u8;
            }
        }
        target.0 = [pixel[0], pixel[1], pixel[2], 255];
    }
    result
}

/// A document showing only the active layer's channels in `view`, for the canvas to draw
/// instead of the composite, or `None` when the composite is shown: the view is the colour
/// composite, or the active layer has no pixels of its own.
pub fn view_document(document: &Document, view: Channels) -> Option<Document> {
    if view == Channels::COLOR || view.is_empty() {
        return None;
    }
    let layer = document.active().filter(|l| editable(l))?;
    let pixels = layer.pixels.as_ref()?;
    let mut shown = Layer::image(layer.name.clone(), view_pixels(pixels, view));
    shown.id = layer.id;
    shown.transform = layer.transform;
    let mut result = document.clone();
    result.layers = vec![shown];
    result.selected = std::collections::HashSet::from([layer.id]);
    Some(result)
}

/// The coverage of one of the active layer's channels over the canvas, for Load Channel as
/// Selection: a pixel is selected as much as the channel is bright there (value ÷ 255), and
/// outside the layer nothing is.
pub fn selection(document: &Document, channel: Channel) -> Option<GrayImage> {
    let layer = document.active().filter(|l| editable(l))?;
    let channel_pixels = match &layer.pixels {
        Some(pixels) => extract(pixels, channel),
        None => return Some(GrayImage::new(document.width, document.height)),
    };
    let transform = layer.transform;
    Some(GrayImage::from_fn(
        document.width,
        document.height,
        |x, y| {
            let unit = transform.inverse(Point::new(x as f32 + 0.5, y as f32 + 0.5));
            Luma([(crate::render::mask_sample(&channel_pixels, unit) * 255.0).round() as u8])
        },
    ))
}

/// Paste an image into one channel of the active layer, its top-left corner at `origin` in
/// the canvas: each covered pixel takes the image's brightness there, blended by the image's
/// own alpha. The other channels and pixels the image does not cover are left as they were.
pub fn paste(
    document: &mut Document,
    channel: Channel,
    image: &RgbaImage,
    origin: Point,
) -> Result<()> {
    let layer = document
        .active_mut()
        .ok_or_else(|| anyhow::anyhow!("Select a layer first"))?;
    ensure!(
        editable(layer),
        "Select a pixel layer to paste into its channel"
    );
    crate::paint::ensure_pixels(layer)?;
    let transform = layer.transform;
    let pixels = Arc::make_mut(layer.pixels.as_mut().expect("ensured"));
    let (width, height) = pixels.dimensions();
    let index = channel.index();
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let point = transform.point(Point::new(
            (x as f32 + 0.5) / width as f32,
            (y as f32 + 0.5) / height as f32,
        ));
        let (px, py) = ((point.x - origin.x).floor(), (point.y - origin.y).floor());
        if px < 0.0 || py < 0.0 || px >= image.width() as f32 || py >= image.height() as f32 {
            continue;
        }
        let source = image.get_pixel(px as u32, py as u32);
        let grey = luma(source.0);
        let amount = f32::from(source[3]) / 255.0;
        pixel[index] =
            (f32::from(pixel[index]) * (1.0 - amount) + f32::from(grey) * amount).round() as u8;
    }
    Ok(())
}

/// Rec. 709 brightness of a colour, ignoring its alpha.
fn luma([r, g, b, _]: [u8; 4]) -> u8 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b))
        .round()
        .clamp(0.0, 255.0) as u8
}

/// A small picture of one channel (grey) or of `view` for the pane's rows, fitted into
/// `side` × `side` with nearest sampling so it stays cheap for large layers.
pub fn thumbnail(image: &RgbaImage, view: Channels, side: u32) -> RgbaImage {
    let side = side.max(1);
    let (width, height) = image.dimensions();
    let scale = (side as f32 / width.max(height).max(1) as f32).min(1.0);
    let w = ((width as f32 * scale).round() as u32).max(1);
    let h = ((height as f32 * scale).round() as u32).max(1);
    let sampled = RgbaImage::from_fn(w, h, |x, y| {
        *image.get_pixel(
            ((x as u64 * 2 + 1) * u64::from(width) / (u64::from(w) * 2)) as u32,
            ((y as u64 * 2 + 1) * u64::from(height) / (u64::from(h) * 2)) as u32,
        )
    });
    if view == Channels::COLOR {
        sampled
    } else {
        view_pixels(&sampled, view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{document::Document, paint};
    use image::Rgba;

    /// A 6 × 4 document whose one layer has a different value in each channel of each pixel.
    fn packed() -> Document {
        let mut document = Document::new(6, 4).unwrap();
        let pixels = RgbaImage::from_fn(6, 4, |x, y| {
            Rgba([
                (x * 40 + y) as u8,
                (y * 60 + 7) as u8,
                (x * 10 + y * 30) as u8,
                (255 - x * 30 - y * 20) as u8,
            ])
        });
        document.layers[0].pixels = Some(Arc::new(pixels));
        document
    }

    fn pixels(document: &Document) -> RgbaImage {
        (**document.layers[0].pixels.as_ref().unwrap()).clone()
    }

    #[test]
    fn channel_sets() {
        assert_eq!(Channels::default(), Channels::ALL);
        let green = Channels::only(Channel::Green);
        assert_eq!(green.single(), Some(Channel::Green));
        assert_eq!(Channels::COLOR.single(), None);
        assert_eq!(Channels::NONE.single(), None);
        assert_eq!(green.with(Channel::Alpha, true).letters(), "GA");
        assert_eq!(Channels::ALL.with(Channel::Alpha, false), Channels::COLOR);
        assert!(Channels::NONE.is_empty());
        assert_eq!(Channels::ALL.iter().count(), 4);
    }

    #[test]
    fn painting_with_only_green_targeted_leaves_the_other_channels_byte_identical() {
        let original = packed();
        let mut document = original.clone();
        let brush = paint::Brush {
            color: [250, 3, 250, 255],
            diameter: 2.0,
            ..paint::Brush::default()
        };
        paint::stroke(
            &mut document,
            Point::new(2.0, 2.0),
            Point::new(4.0, 2.0),
            &brush,
            paint::StrokeOptions {
                mode: paint::PaintMode::Paint,
                mask_target: false,
                source: None,
                clone_offset: Point::default(),
            },
        )
        .unwrap();
        assert!(protect(
            &mut document,
            &original,
            Channels::only(Channel::Green)
        ));
        let (before, after) = (pixels(&original), pixels(&document));
        assert_eq!(before.dimensions(), after.dimensions());
        let mut changed = false;
        for (b, a) in before.pixels().zip(after.pixels()) {
            assert_eq!([b[0], b[2], b[3]], [a[0], a[2], a[3]]);
            changed |= b[1] != a[1];
        }
        assert!(changed, "the stroke should have changed green");
    }

    #[test]
    fn fill_with_alpha_targeted_changes_only_alpha() {
        let original = packed();
        let mut document = original.clone();
        paint::fill(&mut document, [0, 0, 0, 255], false, false).unwrap();
        protect(&mut document, &original, Channels::only(Channel::Alpha));
        for (b, a) in pixels(&original).pixels().zip(pixels(&document).pixels()) {
            assert_eq!([b[0], b[1], b[2], 255], a.0);
        }
    }

    #[test]
    fn a_stroke_that_grows_the_layer_keeps_old_pixels_and_new_ones_start_transparent() {
        let mut original = Document::new(20, 20).unwrap();
        let mut layer = Layer::image(
            "Small",
            RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 200])),
        );
        layer.transform.x = 8.0;
        layer.transform.y = 8.0;
        original.layers = vec![layer];
        original.active = Some(original.layers[0].id);
        let mut document = original.clone();
        let brush = paint::Brush {
            color: [255, 255, 255, 255],
            diameter: 6.0,
            ..paint::Brush::default()
        };
        paint::stroke(
            &mut document,
            Point::new(4.0, 10.0),
            Point::new(16.0, 10.0),
            &brush,
            paint::StrokeOptions {
                mode: paint::PaintMode::Paint,
                mask_target: false,
                source: None,
                clone_offset: Point::default(),
            },
        )
        .unwrap();
        let grown = pixels(&document);
        assert!(grown.width() > 4, "the stroke should have grown the layer");
        assert!(protect(
            &mut document,
            &original,
            Channels::only(Channel::Red)
        ));
        let after = pixels(&document);
        let layer = &document.layers[0];
        for (x, y, pixel) in after.enumerate_pixels() {
            let point = layer.transform.point(Point::new(
                (x as f32 + 0.5) / after.width() as f32,
                (y as f32 + 0.5) / after.height() as f32,
            ));
            let inside = (8.0..12.0).contains(&point.x) && (8.0..12.0).contains(&point.y);
            let expected = if inside { [20, 30, 200] } else { [0, 0, 0] };
            assert_eq!([pixel[1], pixel[2], pixel[3]], expected, "{x},{y}");
        }
    }

    #[test]
    fn edits_that_change_the_canvas_or_the_layers_are_left_whole() {
        let original = packed();
        let green = Channels::only(Channel::Green);
        // Image size.
        let mut resized = original.clone();
        crate::operations::image_size(&mut resized, 3, 2).unwrap();
        assert!(!protect(&mut resized, &original, green));
        // A new layer.
        let mut added = original.clone();
        added.insert(Layer::blank("New", 6, 4));
        assert!(!protect(&mut added, &original, green));
        // Every channel targeted, or pixels that did not change.
        let mut same = original.clone();
        assert!(!protect(&mut same, &original, Channels::ALL));
        assert!(!protect(&mut same, &original, green));
        // A flipped layer no longer lines up with its old pixels.
        let mut flipped = original.clone();
        let layer = &mut flipped.layers[0];
        layer.transform.flip_x = true;
        layer.pixels = Some(Arc::new(RgbaImage::new(6, 4)));
        assert!(!protect(&mut flipped, &original, green));
        // Text layers are drawn from their text.
        let mut text = original.clone();
        text.layers[0].text = Some(crate::text::TextStyle::default());
        text.layers[0].pixels = Some(Arc::new(RgbaImage::new(6, 4)));
        assert!(!protect(&mut text, &original, green));
    }

    #[test]
    fn painting_a_blank_layer_keeps_untargeted_channels_at_zero() {
        let original = Document::new(4, 4).unwrap();
        let mut document = original.clone();
        paint::fill(&mut document, [100, 150, 200, 255], false, false).unwrap();
        assert!(protect(
            &mut document,
            &original,
            Channels::only(Channel::Blue)
        ));
        assert!(pixels(&document).pixels().all(|p| p.0 == [0, 0, 200, 0]));
    }

    #[test]
    fn viewing_alpha_alone_shows_it_as_grey() {
        let document = packed();
        let view = view_document(&document, Channels::only(Channel::Alpha)).unwrap();
        let source = pixels(&document);
        assert_eq!(view.layers.len(), 1);
        let shown = crate::render::render(&view);
        for (s, v) in source.pixels().zip(shown.pixels()) {
            assert_eq!(v.0, [s[3], s[3], s[3], 255]);
        }
        // One colour channel is grey too; the composite needs no stand-in.
        let red = view_pixels(&source, Channels::only(Channel::Red));
        assert!(
            red.pixels()
                .zip(source.pixels())
                .all(|(v, s)| v.0 == [s[0], s[0], s[0], 255])
        );
        assert!(view_document(&document, Channels::COLOR).is_none());
    }

    #[test]
    fn several_channels_show_in_colour_and_alpha_as_a_red_overlay() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 255]));
        let shown = view_pixels(
            &image,
            Channels::only(Channel::Red).with(Channel::Blue, true),
        );
        assert_eq!(shown.get_pixel(0, 0).0, [10, 0, 30, 255]);
        // Opaque: no overlay. Transparent: half red.
        assert_eq!(
            view_pixels(&image, Channels::ALL).get_pixel(0, 0).0,
            [10, 20, 30, 255]
        );
        let clear = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0]));
        assert_eq!(
            view_pixels(&clear, Channels::ALL).get_pixel(0, 0).0,
            [128, 0, 0, 255]
        );
    }

    #[test]
    fn loading_red_as_a_selection_gives_coverage_equal_to_red() {
        let document = packed();
        let selection = selection(&document, Channel::Red).unwrap();
        let source = pixels(&document);
        assert_eq!(selection.dimensions(), (6, 4));
        for (x, y, value) in selection.enumerate_pixels() {
            assert_eq!(value[0], source.get_pixel(x, y)[0]);
            let coverage = crate::selection::coverage(
                Some(&selection),
                Point::new(x as f32 + 0.5, y as f32 + 0.5),
            );
            assert!((coverage - f32::from(source.get_pixel(x, y)[0]) / 255.0).abs() < 1e-6);
        }
        // Outside a smaller layer nothing is selected.
        let mut small = packed();
        small.layers[0].transform.width = 3.0;
        let selection = super::selection(&small, Channel::Green).unwrap();
        assert_eq!(selection.get_pixel(5, 0)[0], 0);
        assert!(selection.get_pixel(0, 0)[0] > 0);
    }

    #[test]
    fn pasting_writes_brightness_into_one_channel() {
        let original = packed();
        let mut document = original.clone();
        let mut image = RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]));
        image.put_pixel(1, 1, Rgba([0, 0, 0, 255]));
        image.put_pixel(0, 1, Rgba([255, 255, 255, 0]));
        paste(&mut document, Channel::Green, &image, Point::new(1.0, 1.0)).unwrap();
        let (before, after) = (pixels(&original), pixels(&document));
        for (x, y, a) in after.enumerate_pixels() {
            let b = before.get_pixel(x, y);
            assert_eq!([a[0], a[2], a[3]], [b[0], b[2], b[3]]);
            let expected = match (x, y) {
                (1, 1) | (2, 1) => 255,
                (2, 2) => 0,
                _ => b[1],
            };
            assert_eq!(a[1], expected, "{x},{y}");
        }
        // Not into a group or a text layer.
        let mut group = original.clone();
        group.layers[0].group = true;
        assert!(paste(&mut group, Channel::Red, &image, Point::default()).is_err());
    }

    #[test]
    fn thumbnails_fit_and_show_one_channel_as_grey() {
        let image = RgbaImage::from_pixel(400, 100, Rgba([10, 20, 30, 40]));
        let thumbnail = thumbnail(&image, Channels::only(Channel::Green), 36);
        assert_eq!(thumbnail.dimensions(), (36, 9));
        assert_eq!(thumbnail.get_pixel(0, 0).0, [20, 20, 20, 255]);
        let composite = super::thumbnail(&RgbaImage::new(1, 1), Channels::COLOR, 0);
        assert_eq!(composite.dimensions(), (1, 1));
    }
}

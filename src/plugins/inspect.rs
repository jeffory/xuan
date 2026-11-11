//! Reading a document's pixels as numbers: colours at points, histograms, and
//! the part of the image inside a rectangle. A plugin that cannot look at a
//! picture, such as an agent, measures with these instead.

use anyhow::{Result, ensure};
use image::RgbaImage;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::document::Document;

/// The most points one `document/sample` request reads.
pub const MAX_POINTS: usize = 1000;
/// The widest average around a point: `radius` pixels each way.
pub const MAX_RADIUS: u32 = 50;

/// A rectangle of whole document pixels inside the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// Read `[x, y, width, height]` in document pixels, cut to the canvas. A
    /// rectangle that misses the canvas is an error, not an empty image.
    pub fn parse(document: &Document, value: Option<&Value>) -> Result<Option<Self>> {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let numbers: Vec<f64> = value
            .as_array()
            .map(|list| list.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        let &[x, y, width, height] = numbers.as_slice() else {
            anyhow::bail!("`region` must be [x, y, width, height] in document pixels");
        };
        ensure!(
            numbers.iter().all(|number| number.is_finite()) && width > 0.0 && height > 0.0,
            "`region` must be [x, y, width, height] with a width and height above 0"
        );
        let left = x.floor().max(0.0);
        let top = y.floor().max(0.0);
        let right = (x + width).ceil().min(f64::from(document.width));
        let bottom = (y + height).ceil().min(f64::from(document.height));
        ensure!(
            right > left && bottom > top,
            "`region` lies outside the {} × {} canvas",
            document.width,
            document.height
        );
        Ok(Some(Self {
            x: left as u32,
            y: top as u32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        }))
    }
}

/// The image to measure, the size of the canvas: everything visible, or one
/// layer alone as it sits in the document (moved, scaled, rotated and masked,
/// with a folder's layers inside it).
pub fn canvas_image(document: &Document, layer: Option<Uuid>) -> Result<RgbaImage> {
    match layer {
        None => Ok(crate::render::render(document)),
        Some(id) => {
            ensure!(
                document.layers.iter().any(|layer| layer.id == id),
                "No layer {id}"
            );
            Ok(crate::operations::render_layers(document, &[id]))
        }
    }
}

/// The part of the image inside `region`, no longer than `max_side` on its
/// longest side: the pixels, and how many of them there are to a document pixel.
pub fn part(
    document: &Document,
    layer: Option<Uuid>,
    region: Region,
    max_side: Option<u32>,
) -> Result<(RgbaImage, f32)> {
    let image = canvas_image(document, layer)?;
    let cropped =
        image::imageops::crop_imm(&image, region.x, region.y, region.width, region.height)
            .to_image();
    Ok(super::edits::fit(&cropped, max_side))
}

/// Rec. 709 luminance of 8-bit colour, 0–255, with the weights the Levels histogram uses.
fn luminance(rgb: [u32; 3]) -> u32 {
    ((rgb[0] * 2126 + rgb[1] * 7152 + rgb[2] * 722 + 5000) / 10000).min(255)
}

/// The colours at `points` (document pixels), each averaged over the pixels
/// within `radius` of it, weighted by their alpha as the eyedropper's
/// averages are. A point off the canvas has no colour.
pub fn sample(
    document: &Document,
    layer: Option<Uuid>,
    points: &[[f64; 2]],
    radius: u32,
) -> Result<Value> {
    ensure!(!points.is_empty(), "`points` must list at least one [x, y]");
    ensure!(
        points.len() <= MAX_POINTS,
        "`points` takes at most {MAX_POINTS} points"
    );
    ensure!(
        radius <= MAX_RADIUS,
        "`radius` must be at most {MAX_RADIUS}"
    );
    let image = canvas_image(document, layer)?;
    let (width, height) = (i64::from(image.width()), i64::from(image.height()));
    let samples: Vec<Value> = points
        .iter()
        .map(|&[x, y]| {
            let inside = x.is_finite()
                && y.is_finite()
                && x >= 0.0
                && y >= 0.0
                && x < width as f64
                && y < height as f64;
            if !inside {
                return json!({"x": x, "y": y, "color": null});
            }
            let (cx, cy, r) = (x.floor() as i64, y.floor() as i64, i64::from(radius));
            let (mut sum, mut alpha, mut count) = ([0u64; 3], 0u64, 0u64);
            for py in (cy - r).max(0)..=(cy + r).min(height - 1) {
                for px in (cx - r).max(0)..=(cx + r).min(width - 1) {
                    let pixel = image.get_pixel(px as u32, py as u32).0;
                    let a = u64::from(pixel[3]);
                    for channel in 0..3 {
                        sum[channel] += u64::from(pixel[channel]) * a;
                    }
                    alpha += a;
                    count += 1;
                }
            }
            let rgb = sum.map(|channel| {
                if alpha == 0 {
                    0
                } else {
                    ((channel as f64 / alpha as f64).round() as u32).min(255)
                }
            });
            let a = ((alpha as f64 / count.max(1) as f64).round() as u32).min(255);
            json!({
                "x": x,
                "y": y,
                "color": format!("#{:02x}{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2], a),
                "rgba": [rgb[0], rgb[1], rgb[2], a],
                "luminance": luminance(rgb),
            })
        })
        .collect();
    Ok(json!({"width": width, "height": height, "radius": radius, "samples": samples}))
}

/// The level below which `fraction` of the counted pixels lie.
fn percentile(bins: &[u64; 256], total: u64, fraction: f64) -> u32 {
    let target = (total as f64 * fraction).ceil().max(1.0) as u64;
    let mut seen = 0;
    for (level, count) in bins.iter().enumerate() {
        seen += count;
        if seen >= target {
            return level as u32;
        }
    }
    255
}

fn describe_channel(bins: &[u64; 256], total: u64) -> Value {
    if total == 0 {
        return json!({"bins": bins.as_slice(), "mean": null});
    }
    let sum: u64 = bins
        .iter()
        .enumerate()
        .map(|(level, count)| level as u64 * count)
        .sum();
    let round = |value: f64| (value * 10000.0).round() / 10000.0;
    json!({
        "bins": bins.as_slice(),
        "mean": round(sum as f64 / total as f64),
        "min": bins.iter().position(|count| *count > 0),
        "max": bins.iter().rposition(|count| *count > 0),
        "median": percentile(bins, total, 0.5),
        "p1": percentile(bins, total, 0.01),
        "p99": percentile(bins, total, 0.99),
        // The share of pixels at the very ends: crushed shadows and blown highlights.
        "clipped_low": round(bins[0] as f64 / total as f64),
        "clipped_high": round(bins[255] as f64 / total as f64),
    })
}

/// Histograms of the image (or one layer, or a region of either): 256 counts
/// each for luminance, red, green and blue over the pixels that are not fully
/// transparent, with the mean, median, range and clipped share of each.
pub fn histogram(
    document: &Document,
    layer: Option<Uuid>,
    region: Option<Region>,
) -> Result<Value> {
    let image = canvas_image(document, layer)?;
    let region = region.unwrap_or(Region {
        x: 0,
        y: 0,
        width: image.width(),
        height: image.height(),
    });
    let mut bins = [[0u64; 256]; 4];
    let (mut counted, mut transparent, mut alpha) = (0u64, 0u64, 0u64);
    for y in region.y..region.y + region.height {
        for x in region.x..region.x + region.width {
            let pixel = image.get_pixel(x, y).0;
            if pixel[3] == 0 {
                transparent += 1;
                continue;
            }
            counted += 1;
            alpha += u64::from(pixel[3]);
            let rgb = [pixel[0], pixel[1], pixel[2]].map(u32::from);
            bins[0][luminance(rgb) as usize] += 1;
            for channel in 0..3 {
                bins[channel + 1][rgb[channel] as usize] += 1;
            }
        }
    }
    let mean_alpha =
        (counted > 0).then(|| ((alpha as f64 / counted as f64) * 100.0).round() / 100.0);
    Ok(json!({
        "region": [region.x, region.y, region.width, region.height],
        "pixels": counted,
        "transparent": transparent,
        "mean_alpha": mean_alpha,
        "luminance": describe_channel(&bins[0], counted),
        "red": describe_channel(&bins[1], counted),
        "green": describe_channel(&bins[2], counted),
        "blue": describe_channel(&bins[3], counted),
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// 8 × 4: the left half opaque red, the right half transparent, on one layer.
    fn document() -> (Document, Uuid) {
        let mut document = Document::new(8, 4).unwrap();
        let mut pixels = RgbaImage::new(8, 4);
        for y in 0..4 {
            for x in 0..4 {
                pixels.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            }
        }
        document.layers[0].pixels = Some(Arc::new(pixels));
        let id = document.layers[0].id;
        (document, id)
    }

    #[test]
    fn regions_are_cut_to_the_canvas_and_refused_outside_it() {
        let (document, _) = document();
        let region = |value: Value| Region::parse(&document, Some(&value));
        assert_eq!(Region::parse(&document, None).unwrap(), None);
        assert_eq!(
            region(json!([1.2, 0, 3, 2])).unwrap(),
            Some(Region {
                x: 1,
                y: 0,
                width: 4,
                height: 2
            })
        );
        assert_eq!(
            region(json!([-5, -5, 100, 100])).unwrap(),
            Some(Region {
                x: 0,
                y: 0,
                width: 8,
                height: 4
            })
        );
        for bad in [
            json!([8, 0, 2, 2]),
            json!([0, 0, 0, 2]),
            json!([0, 0, 2]),
            json!("all"),
        ] {
            assert!(region(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn samples_average_by_alpha_and_points_off_the_canvas_have_no_colour() {
        let (document, layer) = document();
        let sampled = sample(&document, None, &[[1.5, 1.5], [6.0, 1.0], [9.0, 1.0]], 0).unwrap();
        let samples = sampled["samples"].as_array().unwrap();
        assert_eq!(samples[0]["color"], "#ff0000ff");
        assert_eq!(samples[0]["rgba"], json!([255, 0, 0, 255]));
        assert_eq!(samples[0]["luminance"], 54);
        assert_eq!(samples[1]["rgba"], json!([0, 0, 0, 0]));
        assert_eq!(samples[2]["color"], Value::Null);
        // Across the edge, the colour is the opaque side's and the alpha is the average.
        let edge = sample(&document, Some(layer), &[[4.0, 1.0]], 1).unwrap();
        let rgba = &edge["samples"][0]["rgba"];
        assert_eq!((rgba[0].as_u64(), rgba[1].as_u64()), (Some(255), Some(0)));
        assert_eq!(rgba[3], 85, "one of three columns is opaque");
        assert!(sample(&document, None, &[], 0).is_err());
        assert!(sample(&document, None, &[[0.0, 0.0]], MAX_RADIUS + 1).is_err());
        assert!(sample(&document, Some(Uuid::new_v4()), &[[0.0, 0.0]], 0).is_err());
    }

    #[test]
    fn histograms_count_visible_pixels_and_report_clipping() {
        let (document, _) = document();
        let whole = histogram(&document, None, None).unwrap();
        assert_eq!(whole["pixels"], 16);
        assert_eq!(whole["transparent"], 16);
        assert_eq!(whole["red"]["bins"][255], 16);
        assert_eq!(whole["red"]["clipped_high"], 1.0);
        assert_eq!(whole["green"]["clipped_low"], 1.0);
        assert_eq!(whole["luminance"]["median"], 54);
        assert_eq!(whole["luminance"]["bins"].as_array().unwrap().len(), 256);
        let empty = Region {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        };
        let empty = histogram(&document, None, Some(empty)).unwrap();
        assert_eq!(empty["pixels"], 0);
        assert_eq!(empty["red"]["mean"], Value::Null);
    }

    #[test]
    fn a_part_is_the_region_scaled_to_fit() {
        let (document, _) = document();
        let region = Region {
            x: 2,
            y: 0,
            width: 4,
            height: 4,
        };
        let (image, scale) = part(&document, None, region, None).unwrap();
        assert_eq!((image.dimensions(), scale), ((4, 4), 1.0));
        assert_eq!(image.get_pixel(1, 0).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(2, 0).0[3], 0);
        let (small, scale) = part(&document, None, region, Some(2)).unwrap();
        assert_eq!((small.dimensions(), scale), ((2, 2), 0.5));
    }
}

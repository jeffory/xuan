use image::{GrayImage, Rgba, RgbaImage};
use rayon::prelude::*;

use crate::{
    blend::{BlendMode, composite_filled, dissolve_alpha},
    document::{Document, Layer, Point},
};

mod attachments;
pub(crate) use attachments::prepare as prepare_attachments;

pub fn sample(image: &RgbaImage, unit: Point) -> [f32; 4] {
    if !(0.0..1.0).contains(&unit.x) || !(0.0..1.0).contains(&unit.y) {
        return [0.0; 4];
    }
    let x = unit.x * image.width() as f32 - 0.5;
    let y = unit.y * image.height() as f32 - 0.5;
    let fx = x.fract().rem_euclid(1.0);
    let fy = y.fract().rem_euclid(1.0);
    let mut result = [0.0; 4];
    for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
        for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
            let px = (x.floor() as i32 + dx).clamp(0, image.width() as i32 - 1) as u32;
            let py = (y.floor() as i32 + dy).clamp(0, image.height() as i32 - 1) as u32;
            let pixel = image.get_pixel(px, py).0.map(|v| v as f32 / 255.0);
            let weight = wx * wy;
            for i in 0..3 {
                result[i] += pixel[i] * pixel[3] * weight;
            }
            result[3] += pixel[3] * weight;
        }
    }
    if result[3] > 0.0 {
        for i in 0..3 {
            result[i] /= result[3];
        }
    }
    result
}

pub fn mask_sample(mask: &GrayImage, unit: Point) -> f32 {
    if !(0.0..1.0).contains(&unit.x) || !(0.0..1.0).contains(&unit.y) {
        return 0.0;
    }
    let x = (unit.x * mask.width() as f32) as u32;
    let y = (unit.y * mask.height() as f32) as u32;
    mask.get_pixel(x.min(mask.width() - 1), y.min(mask.height() - 1))[0] as f32 / 255.0
}

pub fn own_mask(layer: &Layer, point: Point) -> f32 {
    layer
        .mask
        .as_ref()
        .filter(|m| m.enabled)
        .map_or(1.0, |mask| {
            mask_sample(
                &mask.pixels,
                mask.placement.unwrap_or(layer.transform).inverse(point),
            )
        })
}

/// The alpha `layer` contributes at `point` as a clipping base: its pixels' alpha times its
/// opacity and mask (and its own clipping base's alpha), or for a folder, the folder's
/// composited alpha ([`group_alpha`]).
pub fn layer_alpha(document: &Document, layer: &Layer, point: Point, depth: usize) -> f32 {
    if depth > 256 {
        return 0.0;
    }
    if layer.group {
        return group_alpha(document, layer, point, depth);
    }
    let mut alpha = layer.pixels.as_ref().map_or(0.0, |image| {
        sample(image, layer.transform.inverse(point))[3]
    });
    alpha *= layer.opacity * own_mask(layer, point);
    if let Some(source) = layer
        .clip_to
        .and_then(|id| document.layers.iter().find(|l| l.id == id))
    {
        alpha *= layer_alpha(document, source, point, depth + 1);
    }
    alpha
}

/// Whether a folder child adds to the folder's alpha: pixel layers and folders do;
/// adjustments and filters only recolour what is below and add none.
pub(crate) fn adds_group_alpha(layer: &Layer) -> bool {
    layer.group
        || (layer.pixels.is_some()
            && !layer.standalone_mask
            && layer.adjustment.is_none()
            && layer.filter.is_none())
}

/// Whether a folder child is a mask layer that fades the folder's content below it.
pub(crate) fn fades_group(layer: &Layer) -> bool {
    layer.standalone_mask && layer.opacity > 0.0 && layer.mask.as_ref().is_some_and(|m| m.enabled)
}

/// The alpha a folder composites to on its own, over transparency: the visible children
/// are combined bottom to top as `a + below * (1 - a)` (every blend mode composites alpha
/// that way), mask layers inside fade what is below them, and the folder's own opacity and
/// mask apply last. Folders pass through, so their children blend straight into the
/// backdrop, but the alpha is the same either way; this is the shape a layer clipped to the
/// folder takes. The folder's own visibility is not checked, as for a layer base.
pub fn group_alpha(document: &Document, group: &Layer, point: Point, depth: usize) -> f32 {
    if depth > 256 {
        return 0.0;
    }
    let mut alpha = 0.0;
    for child in document
        .layers
        .iter()
        .filter(|l| l.parent == Some(group.id) && l.visible)
    {
        if fades_group(child) {
            alpha *= 1.0 - child.opacity * (1.0 - own_mask(child, point));
        } else if adds_group_alpha(child) {
            let a = layer_alpha(document, child, point, depth + 1);
            alpha = a + alpha * (1.0 - a);
        }
    }
    alpha * group.opacity * own_mask(group, point)
}

pub fn inherited_coverage(document: &Document, layer: &Layer, point: Point) -> f32 {
    if !layer.visible {
        return 0.0;
    }
    let mut coverage = 1.0;
    let mut parent = layer.parent;
    for _ in 0..64 {
        let Some(group) = parent.and_then(|id| document.layers.iter().find(|l| l.id == id)) else {
            break;
        };
        if !group.visible {
            return 0.0;
        }
        coverage *= own_mask(group, point) * group.opacity;
        parent = group.parent;
    }
    coverage
}

pub(crate) enum CompositeStep<'a> {
    BeginGroup,
    Layer(&'a Layer),
    Filtered(&'a Layer, RgbaImage),
    EndGroup,
}

/// Save the backdrop of each group containing an active standalone mask. Masks
/// fade the group's contribution back toward that backdrop, preserving outside
/// layers and existing pass-through blending and adjustments.
pub(crate) fn composite_steps(document: &Document) -> Vec<CompositeStep<'_>> {
    fn visit<'a>(
        document: &'a Document,
        parent: Option<uuid::Uuid>,
        out: &mut Vec<CompositeStep<'a>>,
        depth: usize,
    ) {
        if depth > 64 {
            return;
        }
        for layer in document
            .layers
            .iter()
            .filter(|l| l.parent == parent && l.visible)
        {
            if layer.group {
                let masked = document.layers.iter().any(|child| {
                    child.parent == Some(layer.id)
                        && child.standalone_mask
                        && child.visible
                        && child.opacity > 0.0
                        && child.mask.as_ref().is_some_and(|m| m.enabled)
                });
                if masked {
                    out.push(CompositeStep::BeginGroup);
                }
                visit(document, Some(layer.id), out, depth + 1);
                if masked {
                    out.push(CompositeStep::EndGroup);
                }
            } else if !layer.standalone_mask
                || (layer.opacity > 0.0 && layer.mask.as_ref().is_some_and(|m| m.enabled))
            {
                out.push(CompositeStep::Layer(layer));
            }
        }
    }
    let mut steps = Vec::new();
    visit(document, None, &mut steps, 0);
    steps
}

/// Depth-first bottom-to-top traversal keeps each folder's subtree together.
pub fn paint_order(document: &Document) -> Vec<&Layer> {
    fn visit<'a>(
        document: &'a Document,
        parent: Option<uuid::Uuid>,
        out: &mut Vec<&'a Layer>,
        depth: usize,
    ) {
        if depth > 64 {
            return;
        }
        for layer in document
            .layers
            .iter()
            .filter(|layer| layer.parent == parent)
        {
            if layer.group {
                visit(document, Some(layer.id), out, depth + 1);
            } else {
                out.push(layer);
            }
        }
    }
    let mut result = Vec::new();
    visit(document, None, &mut result, 0);
    result
}

/// Filter premultiplied colors so transparent edges never acquire dark halos.
pub fn resize_quality(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    if let Some(result) = crate::gpu::resize_rgba(image, width, height) {
        return result;
    }
    let premultiplied = image::ImageBuffer::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0.map(|v| v as f32 / 255.0);
        Rgba([p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]])
    });
    let filtered = image::imageops::resize(
        &premultiplied,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    RgbaImage::from_fn(width, height, |x, y| {
        let p = filtered.get_pixel(x, y).0;
        let alpha = p[3].clamp(0.0, 1.0);
        Rgba([
            (p[0] / alpha.max(0.00001) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8,
            (p[1] / alpha.max(0.00001) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8,
            (p[2] / alpha.max(0.00001) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8,
            (alpha * 255.0).round() as u8,
        ])
    })
}

pub fn source_size(document: &Document, layer: &Layer, size: [u32; 2]) -> [u32; 2] {
    let Some(image) = &layer.pixels else {
        return [1, 1];
    };
    let corners = layer.transform.corners().map(|p| {
        Point::new(
            p.x * size[0] as f32 / document.width as f32,
            p.y * size[1] as f32 / document.height as f32,
        )
    });
    let width = corners[0]
        .distance(corners[1])
        .max(corners[3].distance(corners[2]));
    let height = corners[0]
        .distance(corners[3])
        .max(corners[1].distance(corners[2]));
    let target = |length: f32, original: u32| {
        // Quantization keeps cached textures stable during small transform changes.
        let factor = (original as f32 / length.max(1.0)).log2().floor().max(0.0);
        ((original as f32 / 2.0_f32.powf(factor)) as u32).max(1)
    };
    [target(width, image.width()), target(height, image.height())]
}

/// The canvas as exactly the one layer's pixels, when the document is a single plain layer that
/// covers the canvas pixel for pixel. Compositing would round colours and drop the colour under
/// fully transparent pixels, where packed channel maps keep data, so exports use these instead.
pub fn unaltered(document: &Document) -> Option<std::sync::Arc<RgbaImage>> {
    let [layer] = document.layers.as_slice() else {
        return None;
    };
    let pixels = layer.pixels.as_ref()?;
    (layer.visible
        && !layer.group
        && !layer.is_effect()
        && layer.opacity == 1.0
        && layer.fill == 1.0
        && layer.blend == crate::blend::BlendMode::Normal
        && layer.mask.is_none()
        && layer.clip_to.is_none()
        && layer.effects.as_ref().is_none_or(|e| e.is_empty())
        && pixels.dimensions() == (document.width, document.height)
        && layer.transform == crate::document::Transform::new(document.width, document.height))
    .then(|| pixels.clone())
}

pub fn render(document: &Document) -> RgbaImage {
    render_scaled(document, document.width, document.height)
}

pub fn render_scaled(document: &Document, width: u32, height: u32) -> RgbaImage {
    let prepared = prepare_attachments(document);
    let document = prepared.as_ref();
    if let Some(result) = crate::gpu::compose(document, width, height) {
        return result;
    }
    let mut filtered = document.clone();
    for layer in &mut filtered.layers {
        if let Some(pixels) = &layer.pixels {
            let [w, h] = source_size(document, layer, [width, height]);
            if (w, h) != pixels.dimensions() {
                layer.pixels = Some(std::sync::Arc::new(resize_quality(pixels, w, h)));
            }
        }
    }
    render_pixels(&filtered, width, height)
}

/// Small UI thumbnails must not resize every full-resolution source on each edit.
/// Supersample the thumbnail itself; exports still use the filtered renderer above.
pub fn render_thumbnail(document: &Document, width: u32, height: u32) -> RgbaImage {
    resize_quality(
        &render_pixels(document, width * 2, height * 2),
        width,
        height,
    )
}

pub(crate) fn render_pixels(document: &Document, width: u32, height: u32) -> RgbaImage {
    let prepared = prepare_attachments(document);
    let document = prepared.as_ref();
    let mut steps = Vec::new();
    for step in composite_steps(document) {
        if let CompositeStep::Layer(layer) = step
            && let Some(filter) = &layer.filter
        {
            let backdrop = render_steps(document, &steps, width, height);
            let filter = filter.scaled(width as f32 / document.width as f32);
            steps.push(CompositeStep::Filtered(
                layer,
                crate::effects::filtered_backdrop(&backdrop, &filter),
            ));
        } else {
            steps.push(step);
        }
    }
    render_steps(document, &steps, width, height)
}

fn render_steps(
    document: &Document,
    steps: &[CompositeStep<'_>],
    width: u32,
    height: u32,
) -> RgbaImage {
    let mut output = RgbaImage::new(width, height);
    output
        .as_mut()
        .par_chunks_exact_mut(4)
        .enumerate()
        .for_each_init(Vec::new, |groups, (index, target)| {
            let x = index as u32 % width;
            let y = index as u32 / width;
            let point = Point::new(
                (x as f32 + 0.5) * document.width as f32 / width as f32,
                (y as f32 + 0.5) * document.height as f32 / height as f32,
            );
            let pixel = composite_at(document, steps, point, groups);
            target.copy_from_slice(&pixel.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
        });
    output
}

pub fn pixel_at(document: &Document, point: Point) -> [f32; 4] {
    let prepared = prepare_attachments(document);
    let document = prepared.as_ref();
    if document.layers.iter().any(|l| l.filter.is_some()) {
        return sample(
            &render_pixels(document, document.width, document.height),
            Point::new(
                point.x / document.width as f32,
                point.y / document.height as f32,
            ),
        );
    }
    composite_at(document, &composite_steps(document), point, &mut Vec::new())
}

fn composite_at(
    document: &Document,
    steps: &[CompositeStep<'_>],
    point: Point,
    groups: &mut Vec<[f32; 4]>,
) -> [f32; 4] {
    let mut pixel = [0.0; 4];
    groups.clear();
    for step in steps {
        let layer = match step {
            CompositeStep::BeginGroup => {
                groups.push(pixel);
                continue;
            }
            CompositeStep::EndGroup => {
                groups.pop();
                continue;
            }
            CompositeStep::Layer(layer) => *layer,
            CompositeStep::Filtered(layer, image) => {
                let filtered = sample(
                    image,
                    Point::new(
                        point.x / document.width as f32,
                        point.y / document.height as f32,
                    ),
                );
                let amount = layer.opacity
                    * own_mask(layer, point)
                    * inherited_coverage(document, layer, point);
                pixel = attachments::mix(pixel, filtered, amount);
                continue;
            }
        };
        if layer.standalone_mask {
            let amount = 1.0 - layer.opacity * (1.0 - own_mask(layer, point));
            let background = groups.last().copied().unwrap_or([0.0; 4]);
            let alpha = background[3] * (1.0 - amount) + pixel[3] * amount;
            for i in 0..3 {
                pixel[i] = (background[i] * background[3] * (1.0 - amount)
                    + pixel[i] * pixel[3] * amount)
                    / alpha.max(0.000001);
            }
            pixel[3] = alpha;
            continue;
        }
        let coverage = inherited_coverage(document, layer, point);
        if coverage == 0.0 {
            continue;
        }
        if let Some(adjustment) = &layer.adjustment {
            let adjusted = crate::effects::adjust(pixel, adjustment, point);
            let mut amount = coverage * layer.opacity * own_mask(layer, point);
            if let Some(source) = layer
                .clip_to
                .and_then(|id| document.layers.iter().find(|l| l.id == id))
            {
                amount *= layer_alpha(document, source, point, 0);
            }
            for i in 0..3 {
                pixel[i] += (adjusted[i] - pixel[i]) * amount;
            }
            continue;
        }
        if let Some(image) = &layer.pixels {
            let mut source = sample(image, layer.transform.inverse(point));
            source[3] = layer_alpha(document, layer, point, 0) * coverage;
            // Fill fades the layer's own pixels (the effects are already drawn into a layer
            // with any, at full fill); as a clipping base the layer keeps its full alpha.
            let mut fill = layer.fill;
            if layer.blend == BlendMode::Dissolve {
                source[3] = dissolve_alpha(source[3] * fill, point.x, point.y);
                fill = 1.0;
            }
            pixel = composite_filled(pixel, source, layer.blend, fill);
        }
    }
    pixel
}

pub fn hit_test(document: &Document, point: Point) -> Option<uuid::Uuid> {
    let prepared = prepare_attachments(document);
    let document = prepared.as_ref();
    paint_order(document)
        .into_iter()
        .rev()
        .find(|layer| {
            !layer.locked
                && inherited_coverage(document, layer, point)
                    * layer_alpha(document, layer, point, 0)
                    * standalone_coverage(document, layer, point)
                    > 0.05
        })
        .map(|l| l.id)
}

fn standalone_coverage(document: &Document, layer: &Layer, point: Point) -> f32 {
    let mut coverage = 1.0;
    let mut target = layer;
    for _ in 0..65 {
        let Some(index) = document.layers.iter().position(|l| l.id == target.id) else {
            break;
        };
        for mask in document.layers[index + 1..]
            .iter()
            .filter(|l| l.standalone_mask && l.visible && l.parent == target.parent)
        {
            coverage *= 1.0 - mask.opacity * (1.0 - own_mask(mask, point));
        }
        let Some(parent) = target
            .parent
            .and_then(|id| document.layers.iter().find(|l| l.id == id))
        else {
            break;
        };
        target = parent;
    }
    coverage
}

/// Select by transformed layer bounds, ignoring pixel, mask, and opacity coverage.
pub fn hit_test_bounds(document: &Document, point: Point) -> Option<uuid::Uuid> {
    paint_order(document)
        .into_iter()
        .rev()
        .find(|layer| {
            if layer.locked || !layer.visible || layer.is_effect() {
                return false;
            }
            let unit = layer.transform.inverse(point);
            if !(0.0..1.0).contains(&unit.x) || !(0.0..1.0).contains(&unit.y) {
                return false;
            }
            let mut parent = layer.parent;
            for _ in 0..64 {
                let Some(group) = parent.and_then(|id| document.layers.iter().find(|l| l.id == id))
                else {
                    break;
                };
                if !group.visible {
                    return false;
                }
                parent = group.parent;
            }
            true
        })
        .map(|l| l.id)
}

pub fn flatten_white(image: &RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let Rgba([r, g, b, a]) = *image.get_pixel(x, y);
        image::Rgb(
            [r, g, b]
                .map(|v| ((u32::from(v) * u32::from(a) + 255 * (255 - u32::from(a))) / 255) as u8),
        )
    })
}

#[cfg(test)]
mod mask_tests;

#[cfg(test)]
mod attachment_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Mask;
    use std::sync::Arc;

    #[test]
    fn bounds_hit_testing_follows_rotation_flips_and_perspective() {
        let mut document = Document::new(100, 100).unwrap();
        let bottom = document.layers[0].id;
        let mut top = Layer::image("Transparent", RgbaImage::new(40, 20));
        top.transform.x = 30.0;
        top.transform.y = 40.0;
        top.transform.rotation = 35.0;
        top.transform.flip_x = true;
        top.transform.warp = Some([
            Point::new(0.1, 0.1),
            Point::new(0.9, 0.0),
            Point::new(1.0, 1.0),
            Point::new(0.0, 0.9),
        ]);
        let id = top.id;
        let inside = top.transform.point(Point::new(0.25, 0.5));
        let outside = top.transform.point(Point::new(0.5, -0.2));
        document.insert(top);

        assert_eq!(hit_test(&document, inside), None);
        assert_eq!(hit_test_bounds(&document, inside), Some(id));
        assert_eq!(hit_test_bounds(&document, outside), Some(bottom));
        assert_eq!(hit_test_bounds(&document, Point::new(101.0, 50.0)), None);
    }

    #[test]
    fn bounds_hit_testing_respects_stacking_visibility_and_locks() {
        let mut document = Document::new(100, 100).unwrap();
        let bottom = document.layers[0].id;
        let mut folder = Layer::blank("Folder", 100, 100);
        folder.group = true;
        folder.opacity = 0.0;
        let mut top = Layer::blank("Masked", 100, 100);
        top.parent = Some(folder.id);
        top.opacity = 0.0;
        top.mask = Some(Mask {
            pixels: Arc::new(GrayImage::new(1, 1)),
            ..Mask::white()
        });
        top.clip_to = Some(bottom);
        let id = top.id;
        document.layers.extend([folder, top]);
        let point = Point::new(50.0, 50.0);

        assert_eq!(hit_test_bounds(&document, point), Some(id));
        document.layers[2].locked = true;
        assert_eq!(hit_test_bounds(&document, point), Some(bottom));
        document.layers[2].locked = false;
        document.layers[2].visible = false;
        assert_eq!(hit_test_bounds(&document, point), Some(bottom));
        document.layers[2].visible = true;
        document.layers[1].visible = false;
        assert_eq!(hit_test_bounds(&document, point), Some(bottom));
        document.layers[1].visible = true;
        document.layers[2].adjustment = Some(crate::document::Adjustment::Invert);
        assert_eq!(hit_test_bounds(&document, point), Some(bottom));
    }

    #[test]
    fn thumbnails_preserve_layer_placement_and_transparent_color() {
        let mut document = Document::new(64, 48).unwrap();
        let mut layer = Layer::image(
            "Thumbnail",
            RgbaImage::from_pixel(256, 128, Rgba([255, 0, 0, 128])),
        );
        layer.transform = crate::document::Transform::new(32, 16);
        layer.transform.x = 16.0;
        layer.transform.y = 16.0;
        document.layers = vec![layer];
        let thumbnail = render_thumbnail(&document, 32, 24);
        assert_eq!(thumbnail.dimensions(), (32, 24));
        assert_eq!(thumbnail.get_pixel(16, 12).0, [255, 0, 0, 128]);
        assert_eq!(thumbnail.get_pixel(0, 0)[3], 0);
        assert_eq!(thumbnail.get_pixel(31, 23)[3], 0);
        for pixel in thumbnail.pixels().filter(|p| p[3] > 0) {
            assert_eq!(&pixel.0[..3], &[255, 0, 0]);
        }
    }

    #[test]
    fn downsampling_filters_fine_detail_and_preserves_transparent_edge_color() {
        let checker = RgbaImage::from_fn(64, 64, |x, y| {
            let value = if (x + y) % 2 == 0 { 0 } else { 255 };
            Rgba([value, value, value, 255])
        });
        let mut doc = Document::new(4, 4).unwrap();
        let mut layer = Layer::image("checker", checker);
        layer.transform = crate::document::Transform::new(4, 4);
        doc.insert(layer);
        assert!(render(&doc).pixels().all(|p| (125..=130).contains(&p[0])));
        let edge = RgbaImage::from_fn(16, 16, |x, _| {
            if x < 8 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0; 4])
            }
        });
        let resized = resize_quality(&edge, 4, 4);
        for pixel in resized.pixels().filter(|p| p[3] > 0) {
            assert_eq!(pixel[0], 255);
        }
    }

    /// Upstream's LayerOpacity (Document/LayerGroups.swift): a folder's opacity multiplies into
    /// each layer inside it, nested folders included, and the layers still blend one by one
    /// with what is below (folders pass through rather than being flattened first).
    #[test]
    fn folder_opacity_dims_every_layer_inside() {
        let mut doc = Document::new(1, 1).unwrap();
        let base = Layer::image("Base", RgbaImage::from_pixel(1, 1, Rgba([0, 0, 255, 255])));
        let mut outer = Layer::blank("Outer", 1, 1);
        outer.group = true;
        outer.opacity = 0.5;
        let mut inner = Layer::blank("Inner", 1, 1);
        inner.group = true;
        inner.parent = Some(outer.id);
        inner.opacity = 0.5;
        let mut red = Layer::image("Red", RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])));
        red.parent = Some(inner.id);
        let mut invert = Layer::blank("Invert", 1, 1);
        invert.pixels = None;
        invert.adjustment = Some(crate::document::Adjustment::Invert);
        invert.parent = Some(outer.id);
        doc.layers = vec![base, outer, inner, red];
        // Red at a quarter over blue.
        assert_eq!(
            render_pixels(&doc, 1, 1).get_pixel(0, 0).0,
            [64, 0, 191, 255]
        );
        doc.layers[2].opacity = 1.0;
        assert_eq!(
            render_pixels(&doc, 1, 1).get_pixel(0, 0).0,
            [128, 0, 128, 255]
        );
        // An adjustment inside the folder is dimmed too: half an inversion of (128, 0, 128).
        doc.layers.push(invert);
        let pixel = render_pixels(&doc, 1, 1).get_pixel(0, 0).0;
        assert_eq!(pixel, [128, 128, 128, 255]);
        // At zero opacity nothing inside the folder draws.
        doc.layers[1].opacity = 0.0;
        assert_eq!(
            render_pixels(&doc, 1, 1).get_pixel(0, 0).0,
            [0, 0, 255, 255]
        );
    }

    /// A folder holding two one-pixel shapes at x = 0 and x = 1 of a 4×1 canvas, and a blue
    /// layer clipped to the folder: [shape A, shape B, folder, clipped].
    fn clipped_to_folder() -> Document {
        let mut doc = Document::new(4, 1).unwrap();
        let mut group = Layer::blank("Figure", 4, 1);
        group.group = true;
        let shape = |name, at: u32, color| {
            let mut layer = Layer::image(
                name,
                RgbaImage::from_fn(4, 1, |x, _| Rgba(if x == at { color } else { [0; 4] })),
            );
            layer.parent = Some(group.id);
            layer
        };
        let a = shape("Cloak", 0, [255, 0, 0, 255]);
        let b = shape("Hood", 1, [0, 255, 0, 255]);
        let mut clipped = Layer::image(
            "Shading",
            RgbaImage::from_pixel(4, 1, Rgba([0, 0, 255, 255])),
        );
        clipped.clip_to = Some(group.id);
        doc.layers = vec![a, b, group, clipped];
        doc.active = doc.layers.last().map(|l| l.id);
        doc.validate().unwrap();
        doc
    }

    #[test]
    fn a_layer_clipped_to_a_folder_shows_only_inside_its_layers() {
        let doc = clipped_to_folder();
        let image = render_pixels(&doc, 4, 1);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(2, 0).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(3, 0).0, [0, 0, 0, 0]);
        // The shape follows later edits: moving a layer in the folder moves the clipping.
        let mut moved = doc.clone();
        moved.layers[1].transform.x = 2.0;
        let image = render_pixels(&moved, 4, 1);
        assert_eq!(image.get_pixel(1, 0).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(3, 0).0, [0, 0, 255, 255]);
        // Hidden layers, adjustments and empty layers in the folder add nothing.
        let mut hidden = doc.clone();
        hidden.layers[1].visible = false;
        let mut invert = Layer::blank("Invert", 4, 1);
        invert.adjustment = Some(crate::document::Adjustment::Invert);
        invert.parent = doc.layers[0].parent;
        hidden.layers.insert(2, invert);
        let image = render_pixels(&hidden, 4, 1);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(1, 0)[3], 0);
        assert_eq!(image.get_pixel(2, 0)[3], 0);
    }

    #[test]
    fn a_folder_base_clips_through_its_mask_and_mask_layers() {
        let mut doc = clipped_to_folder();
        // The folder's own mask hides the left pixel: neither the folder's layers nor the
        // clipped layer show there.
        doc.layers[2].mask = Some(Mask {
            pixels: Arc::new(GrayImage::from_fn(4, 1, |x, _| {
                image::Luma([if x == 0 { 0 } else { 255 }])
            })),
            ..Mask::white()
        });
        let image = render_pixels(&doc, 4, 1);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 0, 255, 255]);
        let point = Point::new(0.5, 0.5);
        assert_eq!(group_alpha(&doc, &doc.layers[2], point, 0), 0.0);
        // A mask layer inside the folder fades the layers below it, and so the shape.
        doc.layers[2].mask = None;
        let mut mask = Layer::mask("Fade", 4, 1);
        mask.parent = Some(doc.layers[2].id);
        mask.mask.as_mut().unwrap().pixels = Arc::new(GrayImage::from_fn(4, 1, |x, _| {
            image::Luma([if x == 1 { 0 } else { 255 }])
        }));
        doc.layers.insert(2, mask);
        doc.validate().unwrap();
        let image = render_pixels(&doc, 4, 1);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 0, 0, 0]);
    }

    /// As for a layer base (and in Photoshop), the base's opacity applies to the clipped
    /// layers: half-opaque folder, half-opaque clipped layer on top.
    #[test]
    fn a_folder_base_passes_its_opacity_to_the_clipped_layer() {
        let mut doc = clipped_to_folder();
        doc.layers[2].opacity = 0.5;
        let point = Point::new(0.5, 0.5);
        assert_eq!(layer_alpha(&doc, &doc.layers[2], point, 0), 0.5);
        let image = render_pixels(&doc, 4, 1);
        // Red at half, then blue at half over it: alpha 0.75.
        let pixel = image.get_pixel(0, 0).0;
        assert_eq!(pixel, [85, 0, 170, 191]);
        assert_eq!(image.get_pixel(2, 0).0, [0, 0, 0, 0]);
        // A non-folder base works the same way.
        let mut layer_base = clipped_to_folder();
        layer_base.layers[0].opacity = 0.5;
        layer_base.layers[0].parent = None;
        layer_base.layers[3].clip_to = Some(layer_base.layers[0].id);
        layer_base.layers.swap(0, 2);
        layer_base.validate().unwrap();
        assert_eq!(render_pixels(&layer_base, 4, 1).get_pixel(0, 0).0, pixel);
        // Overlapping layers in the folder combine like the composite: 0.5 over 0.5 is 0.75,
        // and the clipped layer at 0.75 over that makes 0.9375.
        let mut overlap = clipped_to_folder();
        overlap.layers[1].transform.x = -1.0;
        overlap.layers[0].opacity = 0.5;
        overlap.layers[1].opacity = 0.5;
        assert_eq!(layer_alpha(&overlap, &overlap.layers[2], point, 0), 0.75);
        let image = render_pixels(&overlap, 4, 1);
        assert_eq!(image.get_pixel(0, 0)[3], 239);
    }

    #[test]
    fn folder_visibility_mask_and_clipping_compose() {
        let mut doc = Document::new(2, 1).unwrap();
        doc.layers.clear();
        let base = Layer::image(
            "Base",
            RgbaImage::from_fn(2, 1, |x, _| Rgba([255, 0, 0, if x == 0 { 255 } else { 0 }])),
        );
        let mut group = Layer::blank("Folder", 2, 1);
        group.group = true;
        group.mask = Some(Mask {
            pixels: Arc::new(GrayImage::from_pixel(1, 1, image::Luma([128]))),
            ..Mask::white()
        });
        let mut top = Layer::image(
            "Clipped",
            RgbaImage::from_pixel(2, 1, Rgba([0, 0, 255, 255])),
        );
        top.clip_to = Some(base.id);
        top.parent = Some(group.id);
        doc.layers = vec![base, group, top];
        let image = render(&doc);
        assert_eq!(image.get_pixel(0, 0).0, [127, 0, 128, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 0, 0, 0]);
        doc.layers[1].visible = false;
        assert_eq!(render(&doc).get_pixel(0, 0).0, [255, 0, 0, 255]);
    }
}

#[cfg(test)]
mod fill_tests;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context as _, Result, ensure};
use image::{GrayImage, Luma, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    document::{Document, Layer, Point, Transform, validate_size},
    i18n::tr,
    render, selection,
};

pub fn transform_box(document: &Document, mask_target: bool) -> Option<Transform> {
    let active = document.active()?;
    if mask_target {
        return Some(
            active
                .mask
                .as_ref()
                .and_then(|m| m.placement)
                .unwrap_or(active.transform),
        );
    }
    if document.selected.len() <= 1 && !active.group {
        return Some(active.transform);
    }
    let targets = document.transform_targets();
    let mut min = Point::new(f32::MAX, f32::MAX);
    let mut max = Point::new(f32::MIN, f32::MIN);
    for layer in document
        .layers
        .iter()
        .filter(|l| targets.contains(&l.id) && !l.group)
    {
        for point in layer.transform.corners() {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
        }
    }
    if min.x > max.x {
        return Some(active.transform);
    }
    Some(Transform {
        x: min.x,
        y: min.y,
        width: (max.x - min.x).max(1.0),
        height: (max.y - min.y).max(1.0),
        ..Transform::new(1, 1)
    })
}

pub fn apply_transform(document: &mut Document, new: Transform, mask_target: bool) -> Result<()> {
    ensure!(new.valid(), "Invalid transform");
    let Some(old) = transform_box(document, mask_target) else {
        return Ok(());
    };
    let targets = if mask_target {
        document.active.into_iter().collect()
    } else {
        document.movement_targets()
    };
    for layer in &mut document.layers {
        if !targets.contains(&layer.id) || layer.locked {
            continue;
        }
        if mask_target {
            if let Some(mask) = &mut layer.mask {
                mask.placement = Some(new);
                mask.linked = false;
            }
        } else {
            let transform = if targets.len() == 1 {
                new
            } else {
                layer.transform.following(old, new)
            };
            layer.set_transform(transform);
        }
    }
    Ok(())
}

pub fn duplicate(document: &mut Document) {
    let targets = document.transform_targets();
    let ids: HashMap<_, _> = targets.iter().map(|id| (*id, Uuid::new_v4())).collect();
    let mut copies = Vec::new();
    for layer in &document.layers {
        if !targets.contains(&layer.id) {
            continue;
        }
        let mut copy = layer.clone();
        copy.id = ids[&layer.id];
        copy.name = format!("{} copy", layer.name);
        copy.parent = copy.parent.map(|id| ids.get(&id).copied().unwrap_or(id));
        copy.clip_to = copy.clip_to.map(|id| ids.get(&id).copied().unwrap_or(id));
        copies.push(copy);
    }
    if copies.is_empty() {
        return;
    }
    let index = document
        .layers
        .iter()
        .rposition(|l| targets.contains(&l.id))
        .unwrap()
        + 1;
    document.active = document.active.and_then(|id| ids.get(&id).copied());
    document.selected = document
        .selected
        .iter()
        .filter_map(|id| ids.get(id).copied())
        .collect();
    document.layers.splice(index..index, copies);
}

pub fn copy_layers(source: &Document, destination: &mut Document, root: Uuid) -> Result<()> {
    let targets = source.descendants(root);
    let ids: HashMap<_, _> = targets.iter().map(|id| (*id, Uuid::new_v4())).collect();
    let anchor = source
        .layers
        .iter()
        .find(|l| l.id == root)
        .ok_or_else(|| anyhow::anyhow!("The dragged layer no longer exists"))?
        .transform
        .center();
    let offset = Point::new(
        destination.width as f32 * 0.5 - anchor.x,
        destination.height as f32 * 0.5 - anchor.y,
    );
    let mut copies = Vec::new();
    for layer in source.layers.iter().filter(|l| targets.contains(&l.id)) {
        let mut copy = layer.clone();
        if let Some(clip) = layer.clip_to.filter(|id| !targets.contains(id))
            && let Some(pixels) = &layer.pixels
        {
            let prepared_source = render::prepare_attachments(source);
            let base = prepared_source
                .layers
                .iter()
                .find(|l| l.id == clip)
                .unwrap();
            let baked = crate::gpu::bake_alpha(&prepared_source, base, pixels, layer.transform)
                .unwrap_or_else(|| {
                    let mut baked = (**pixels).clone();
                    let (width, height) = baked.dimensions();
                    for (x, y, pixel) in baked.enumerate_pixels_mut() {
                        let point = layer.transform.point(Point::new(
                            (x as f32 + 0.5) / width as f32,
                            (y as f32 + 0.5) / height as f32,
                        ));
                        pixel[3] = (pixel[3] as f32
                            * render::layer_alpha(&prepared_source, base, point, 0))
                        .round() as u8;
                    }
                    baked
                });
            copy.pixels = Some(Arc::new(baked));
            copy.shape = None;
            copy.text = None;
            copy.raw = None;
        }
        copy.id = ids[&layer.id];
        copy.parent = layer.parent.and_then(|id| ids.get(&id).copied());
        copy.clip_to = layer.clip_to.and_then(|id| ids.get(&id).copied());
        copy.transform.x += offset.x;
        copy.transform.y += offset.y;
        if let Some(placement) = copy.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            placement.x += offset.x;
            placement.y += offset.y;
        }
        copies.push(copy);
    }
    destination.layers.extend(copies);
    destination.select(ids[&root], false);
    destination.validate()
}

pub fn group(document: &mut Document) {
    let parent = document.active().and_then(|l| l.parent);
    if parent.is_some_and(|id| document.layers.iter().any(|l| l.id == id && !l.group)) {
        return;
    }
    let mut group = Layer::blank("Group", document.width, document.height);
    group.group = true;
    group.parent = parent;
    let id = group.id;
    for layer in &mut document.layers {
        if document.selected.contains(&layer.id) && layer.parent == parent {
            layer.parent = Some(id);
        }
    }
    let index = document
        .layers
        .iter()
        .rposition(|l| l.parent == Some(id))
        .map_or(document.layers.len(), |i| i + 1);
    document.layers.insert(index, group);
    document.select(id, false);
    document.release_clipping_cycles();
}

/// Where a moved layer lands relative to its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Directly above the target in the stack, in the target's group.
    Above,
    /// Directly below the target in the stack, in the target's group.
    Below,
    /// At the top of the target group.
    Inside,
}

/// Move `source` next to or into `target`, as dragging in the Layers panel does.
/// Does nothing when either layer is missing or `target` is inside `source`.
pub fn move_layer(document: &mut Document, source: Uuid, target: Uuid, at: Placement) {
    if document.descendants(source).contains(&target) {
        return;
    }
    let Some(destination) = document.layers.iter().find(|l| l.id == target).cloned() else {
        return;
    };
    let Some(index) = document.layers.iter().position(|l| l.id == source) else {
        return;
    };
    let mut layer = document.layers.remove(index);
    layer.parent = if at == Placement::Inside {
        Some(target)
    } else {
        destination.parent
    };
    layer.clip_to = None;
    // The panel lists siblings from top to bottom, opposite their paint order.
    let target_index = document.layers.iter().position(|l| l.id == target).unwrap();
    let index = match at {
        Placement::Above => target_index + 1,
        Placement::Below => target_index,
        Placement::Inside => document
            .layers
            .iter()
            .rposition(|l| l.parent == Some(target))
            .map_or(target_index + 1, |i| i + 1),
    };
    document.layers.insert(index, layer);
    // A folder moved inside another can bring layers clipped to that folder.
    document.release_clipping_cycles();
}

pub fn ungroup(document: &mut Document) {
    let Some(group) = document.active().filter(|l| l.group).cloned() else {
        return;
    };
    for layer in &mut document.layers {
        if layer.parent == Some(group.id) {
            layer.parent = group.parent;
        }
        // Layers clipped to the folder are released, as Photoshop does.
        if layer.clip_to == Some(group.id) {
            layer.clip_to = None;
        }
    }
    document.layers.retain(|l| l.id != group.id);
    document.active = document.layers.last().map(|l| l.id);
    document.selected = document.active.into_iter().collect();
}

pub fn merge_selected(document: &mut Document, down: bool) -> Result<()> {
    let mut targets = document.transform_targets();
    // An attached effect can only be baked together with its image and stack.
    for layer in &document.layers {
        if targets.contains(&layer.id)
            && layer.is_effect()
            && let Some(owner) = document.attachment_owner(layer)
        {
            targets.extend(document.descendants(owner));
        }
    }
    if down && targets.len() == 1 {
        let index = document
            .layers
            .iter()
            .position(|l| Some(l.id) == document.active)
            .unwrap_or(0);
        if index > 0 {
            let parent = document.layers[index].parent;
            if let Some(layer) = document.layers[..index]
                .iter()
                .rev()
                .find(|l| l.parent == parent)
            {
                targets.extend(document.descendants(layer.id));
            }
        }
    }
    // Baking a standalone mask or filter must include its entire lower stack.
    for (index, mask) in document.layers.iter().enumerate().rev() {
        if (mask.standalone_mask || mask.filter.is_some()) && targets.contains(&mask.id) {
            for layer in document.layers[..index]
                .iter()
                .filter(|l| l.parent == mask.parent)
            {
                targets.extend(document.descendants(layer.id));
            }
        }
    }
    ensure!(
        targets.len() >= 2 || document.active().is_some_and(|l| l.group),
        "Select at least two layers, or a layer above another"
    );
    let parents: std::collections::HashSet<_> = document
        .layers
        .iter()
        .filter(|layer| {
            targets.contains(&layer.id) && !layer.parent.is_some_and(|id| targets.contains(&id))
        })
        .map(|layer| layer.parent)
        .collect();
    let parent = if parents.len() == 1 {
        *parents.iter().next().unwrap()
    } else {
        None
    };
    let mut isolated = document.clone();
    for layer in &mut isolated.layers {
        if !targets.contains(&layer.id) {
            if layer.group && parent.is_some() {
                // The merged layer inherits these ancestors after rasterization.
                layer.visible = true;
                layer.opacity = 1.0;
                layer.mask = None;
            } else if !layer.group {
                layer.visible = false;
            }
        }
    }
    keep_folder_clipping_shapes(document, &targets, &mut isolated);
    let pixels = render::render(&isolated);
    let mut merged = Layer::image(
        document
            .active()
            .map_or("Merged".into(), |l| l.name.clone()),
        pixels,
    );
    merged.parent = parent;
    let insert = document
        .layers
        .iter()
        .rposition(|l| targets.contains(&l.id))
        .unwrap_or(0);
    let remaining_below = document.layers[..insert]
        .iter()
        .filter(|l| !targets.contains(&l.id))
        .count();
    document.layers.retain(|l| !targets.contains(&l.id));
    for layer in &mut document.layers {
        if layer.clip_to.is_some_and(|id| targets.contains(&id)) {
            layer.clip_to = Some(merged.id);
        }
    }
    document.select(merged.id, false);
    document
        .layers
        .insert(remaining_below.min(document.layers.len()), merged);
    Ok(())
}

/// A merge renders its layers with the others hidden, which would empty a folder that merged
/// layers are clipped to. Each such folder's shape is rendered from the real document into a
/// hidden stand-in layer (a hidden base still clips) that the merged layers clip to instead.
fn keep_folder_clipping_shapes(
    document: &Document,
    targets: &HashSet<Uuid>,
    isolated: &mut Document,
) {
    let bases: HashSet<Uuid> = isolated
        .layers
        .iter()
        .filter_map(|l| l.clip_to)
        .filter(|id| {
            !targets.contains(id) && document.layers.iter().any(|l| l.id == *id && l.group)
        })
        .collect();
    if bases.is_empty() {
        return;
    }
    let prepared = render::prepare_attachments(document);
    for base in bases {
        let Some(group) = prepared.layers.iter().find(|l| l.id == base) else {
            continue;
        };
        let alpha = alpha_image(&prepared, group);
        let mut shape = Layer::image(
            "Clipping shape",
            RgbaImage::from_fn(alpha.width(), alpha.height(), |x, y| {
                image::Rgba([255, 255, 255, alpha.get_pixel(x, y)[0]])
            }),
        );
        shape.visible = false;
        for layer in &mut isolated.layers {
            if layer.clip_to == Some(base) {
                layer.clip_to = Some(shape.id);
            }
        }
        isolated.layers.push(shape);
    }
}

/// A layer's clipping-base alpha ([`render::layer_alpha`]) at document resolution.
fn alpha_image(document: &Document, layer: &Layer) -> GrayImage {
    crate::gpu::coverage_image(
        document,
        layer,
        [document.width, document.height],
        crate::gpu::CoverageMode::Alpha,
        None,
    )
    .unwrap_or_else(|| {
        GrayImage::from_fn(document.width, document.height, |x, y| {
            let point = Point::new(x as f32 + 0.5, y as f32 + 0.5);
            Luma([(render::layer_alpha(document, layer, point, 0) * 255.0).round() as u8])
        })
    })
}

pub fn canvas_size(
    document: &mut Document,
    width: u32,
    height: u32,
    anchor: [f32; 2],
) -> Result<()> {
    let dx = (width as f32 - document.width as f32) * anchor[0];
    let dy = (height as f32 - document.height as f32) * anchor[1];
    resize_canvas(document, width, height, dx, dy)
}

/// Grow the canvas by whole pixels on each side: Canvas Size with the
/// content moved `left`, `top` pixels into the larger canvas.
pub fn extend_canvas(
    document: &mut Document,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
) -> Result<()> {
    let grow = |size: u32, a: u32, b: u32| {
        size.checked_add(a)
            .and_then(|size| size.checked_add(b))
            .unwrap_or(u32::MAX)
    };
    let width = grow(document.width, left, right);
    let height = grow(document.height, top, bottom);
    resize_canvas(document, width, height, left as f32, top as f32)
}

/// Change the canvas size, moving every layer, mask placement and guide by
/// `dx`, `dy` so the content keeps its place relative to the new origin.
fn resize_canvas(document: &mut Document, width: u32, height: u32, dx: f32, dy: f32) -> Result<()> {
    validate_size(width, height)?;
    for layer in &mut document.layers {
        layer.transform.x += dx;
        layer.transform.y += dy;
        if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            placement.x += dx;
            placement.y += dy;
        }
    }
    for guide in &mut document.guides {
        guide.offset(dx, dy);
    }
    crate::vector::transform_paths(
        &mut document.paths,
        kurbo::Affine::translate((f64::from(dx), f64::from(dy))),
    );
    document.width = width;
    document.height = height;
    document.selection = None;
    Ok(())
}

pub fn crop(document: &mut Document, start: Point, end: Point) -> Result<()> {
    let left = start.x.min(end.x).round();
    let top = start.y.min(end.y).round();
    let width = (end.x - start.x).abs().round() as u32;
    let height = (end.y - start.y).abs().round() as u32;
    validate_size(width, height)?;
    for layer in &mut document.layers {
        layer.transform.x -= left;
        layer.transform.y -= top;
        if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            placement.x -= left;
            placement.y -= top;
        }
    }
    for guide in &mut document.guides {
        guide.offset(-left, -top);
    }
    crate::vector::transform_paths(
        &mut document.paths,
        kurbo::Affine::translate((-f64::from(left), -f64::from(top))),
    );
    document.width = width;
    document.height = height;
    document.selection = None;
    Ok(())
}

pub fn image_size(document: &mut Document, width: u32, height: u32) -> Result<()> {
    validate_size(width, height)?;
    let old = Transform::new(document.width, document.height);
    let new = Transform::new(width, height);
    let scale = |t: &mut Transform| *t = t.following(old, new);
    for layer in &mut document.layers {
        scale(&mut layer.transform);
        if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            scale(placement);
        }
    }
    let (sx, sy) = (
        width as f32 / document.width as f32,
        height as f32 / document.height as f32,
    );
    for guide in &mut document.guides {
        guide.scale(sx, sy);
    }
    crate::vector::transform_paths(
        &mut document.paths,
        kurbo::Affine::scale_non_uniform(f64::from(sx), f64::from(sy)),
    );
    document.width = width;
    document.height = height;
    if let Some(selection) = &document.selection {
        document.selection = Some(Arc::new(crate::gpu::resize_gray(selection, width, height)));
    }
    Ok(())
}

pub fn flip_canvas(document: &mut Document, horizontal: bool) {
    let flip = |t: &mut Transform| {
        if horizontal {
            t.x = document.width as f32 - t.x - t.width;
            t.flip_x = !t.flip_x;
        } else {
            t.y = document.height as f32 - t.y - t.height;
            t.flip_y = !t.flip_y;
        }
        t.rotation = -t.rotation;
    };
    for layer in &mut document.layers {
        flip(&mut layer.transform);
        if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            flip(placement);
        }
    }
    let extent = if horizontal {
        document.width
    } else {
        document.height
    } as f32;
    for guide in &mut document.guides {
        guide.mirror(horizontal, extent);
    }
    let extent = f64::from(extent);
    crate::vector::transform_paths(
        &mut document.paths,
        if horizontal {
            kurbo::Affine::new([-1.0, 0.0, 0.0, 1.0, extent, 0.0])
        } else {
            kurbo::Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, extent])
        },
    );
    if let Some(selection) = &document.selection {
        document.selection = Some(Arc::new(if horizontal {
            image::imageops::flip_horizontal(&**selection)
        } else {
            image::imageops::flip_vertical(&**selection)
        }));
    }
}

/// A turn of the whole canvas, as Image → Rotate Canvas offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasRotation {
    Clockwise,
    CounterClockwise,
    Half,
}

impl CanvasRotation {
    /// The turn for an angle in degrees clockwise: 90, 180, 270 or -90.
    pub fn from_degrees(degrees: i32) -> Option<Self> {
        match degrees {
            90 | -270 => Some(Self::Clockwise),
            -90 | 270 => Some(Self::CounterClockwise),
            180 | -180 => Some(Self::Half),
            _ => None,
        }
    }

    fn degrees(self) -> f32 {
        match self {
            Self::Clockwise => 90.0,
            Self::CounterClockwise => -90.0,
            Self::Half => 180.0,
        }
    }

    /// Where a document point goes on the old `width` × `height` canvas.
    fn map(self, point: Point, width: f32, height: f32) -> Point {
        match self {
            Self::Clockwise => Point::new(height - point.y, point.x),
            Self::CounterClockwise => Point::new(point.y, width - point.x),
            Self::Half => Point::new(width - point.x, height - point.y),
        }
    }
}

/// Image → Rotate Canvas: turn the whole document a quarter or half turn. Every layer's
/// placement, mask placement, guide, document path and the selection turn with it, and a quarter
/// turn swaps the canvas width and height. Pixels are not resampled, so text, shapes and
/// effects stay live (an effect's light angle stays put, as with Photoshop's global light).
pub fn rotate_canvas(document: &mut Document, rotation: CanvasRotation) {
    let (width, height) = (document.width as f32, document.height as f32);
    let turn = |t: &mut Transform| {
        let center = rotation.map(t.center(), width, height);
        t.x = center.x - t.width * 0.5;
        t.y = center.y - t.height * 0.5;
        let angle = t.rotation + rotation.degrees();
        t.rotation = if angle > 180.0 {
            angle - 360.0
        } else if angle <= -180.0 {
            angle + 360.0
        } else {
            angle
        };
    };
    for layer in &mut document.layers {
        turn(&mut layer.transform);
        if let Some(placement) = layer.mask.as_mut().and_then(|m| m.placement.as_mut()) {
            turn(placement);
        }
    }
    for guide in &mut document.guides {
        use crate::layout::GuideAxis::{Horizontal, Vertical};
        let p = guide.position;
        (guide.axis, guide.position) = match (rotation, guide.axis) {
            (CanvasRotation::Clockwise, Vertical) => (Horizontal, p),
            (CanvasRotation::Clockwise, Horizontal) => (Vertical, height - p),
            (CanvasRotation::CounterClockwise, Vertical) => (Horizontal, width - p),
            (CanvasRotation::CounterClockwise, Horizontal) => (Vertical, p),
            (CanvasRotation::Half, Vertical) => (Vertical, width - p),
            (CanvasRotation::Half, Horizontal) => (Horizontal, height - p),
        };
    }
    let (w, h) = (f64::from(width), f64::from(height));
    crate::vector::transform_paths(
        &mut document.paths,
        kurbo::Affine::new(match rotation {
            CanvasRotation::Clockwise => [0.0, 1.0, -1.0, 0.0, h, 0.0],
            CanvasRotation::CounterClockwise => [0.0, -1.0, 1.0, 0.0, 0.0, w],
            CanvasRotation::Half => [-1.0, 0.0, 0.0, -1.0, w, h],
        }),
    );
    if let Some(selection) = &document.selection {
        document.selection = Some(Arc::new(match rotation {
            CanvasRotation::Clockwise => image::imageops::rotate90(&**selection),
            CanvasRotation::CounterClockwise => image::imageops::rotate270(&**selection),
            CanvasRotation::Half => image::imageops::rotate180(&**selection),
        }));
    }
    if rotation != CanvasRotation::Half {
        std::mem::swap(&mut document.width, &mut document.height);
    }
}

/// Image → Crop to Selection: crop the canvas to the selection's bounds.
pub fn crop_to_selection(document: &mut Document) -> Result<()> {
    let (left, top, right, bottom) = document
        .selection
        .as_deref()
        .and_then(selection::bounds)
        .context(tr("The selection is empty."))?;
    crop(
        document,
        Point::new(left as f32, top as f32),
        Point::new(right as f32, bottom as f32),
    )
}

/// What Image → Trim looks for at the edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrimBasis {
    /// Fully transparent pixels.
    #[default]
    Transparent,
    /// Pixels of the same colour as the top-left pixel.
    TopLeft,
    /// Pixels of the same colour as the bottom-right pixel.
    BottomRight,
}

/// Which edges Image → Trim may cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrimSides {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl Default for TrimSides {
    fn default() -> Self {
        Self {
            top: true,
            bottom: true,
            left: true,
            right: true,
        }
    }
}

/// The `(left, top, right, bottom)` an image trims to, or `None` when the trim would change
/// nothing (or nothing would be left, as for a flat image).
pub fn trim_bounds(
    image: &RgbaImage,
    basis: TrimBasis,
    sides: TrimSides,
) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let background = match basis {
        TrimBasis::Transparent => None,
        TrimBasis::TopLeft => Some(*image.get_pixel(0, 0)),
        TrimBasis::BottomRight => Some(*image.get_pixel(width - 1, height - 1)),
    };
    let is_content = |pixel: &Rgba<u8>| match background {
        None => pixel[3] != 0,
        Some(background) => *pixel != background,
    };
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, 0, 0);
    for (x, y, pixel) in image.enumerate_pixels() {
        if is_content(pixel) {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if min_x > max_x || min_y > max_y {
        return None;
    }
    let bounds = (
        if sides.left { min_x } else { 0 },
        if sides.top { min_y } else { 0 },
        if sides.right { max_x + 1 } else { width },
        if sides.bottom { max_y + 1 } else { height },
    );
    (bounds != (0, 0, width, height)).then_some(bounds)
}

/// Image → Trim: crop the canvas to the visible composite's content. Returns whether anything
/// was trimmed; when not, the document is unchanged.
pub fn trim(document: &mut Document, basis: TrimBasis, sides: TrimSides) -> Result<bool> {
    let image = render::render(document);
    let Some((left, top, right, bottom)) = trim_bounds(&image, basis, sides) else {
        return Ok(false);
    };
    crop(
        document,
        Point::new(left as f32, top as f32),
        Point::new(right as f32, bottom as f32),
    )?;
    Ok(true)
}

pub fn copy_pixels(document: &Document, merged: bool) -> Option<(RgbaImage, Point)> {
    let image = if merged {
        render::render(document)
    } else {
        let layer = document.active()?;
        let mut isolated = document.clone();
        let targets = document.descendants(layer.id);
        isolated.layers.retain(|l| targets.contains(&l.id));
        let root = isolated
            .layers
            .iter_mut()
            .find(|l| l.id == layer.id)
            .unwrap();
        root.parent = None;
        root.visible = true;
        root.clip_to = None;
        render::render(&isolated)
    };
    let (left, top, right, bottom) = if let Some(mask) = &document.selection {
        selection::bounds(mask)?
    } else {
        (0, 0, document.width, document.height)
    };
    let mut image =
        image::imageops::crop_imm(&image, left, top, right - left, bottom - top).to_image();
    if let Some(mask) = &document.selection {
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            pixel[3] =
                (u16::from(pixel[3]) * u16::from(mask.get_pixel(x + left, y + top)[0]) / 255) as u8;
        }
    }
    Some((image, Point::new(left as f32, top as f32)))
}

pub fn selection_from_layer(document: &mut Document, mask_target: bool) {
    let prepared = if mask_target {
        std::borrow::Cow::Borrowed(&*document)
    } else {
        render::prepare_attachments(document)
    };
    let source = prepared.as_ref();
    let Some(layer) = source.active() else {
        return;
    };
    if let Some(mask) = crate::gpu::coverage_image(
        source,
        layer,
        [document.width, document.height],
        if mask_target {
            crate::gpu::CoverageMode::Mask
        } else {
            crate::gpu::CoverageMode::Alpha
        },
        None,
    ) {
        document.selection = Some(Arc::new(mask));
        return;
    }
    let mask = GrayImage::from_fn(document.width, document.height, |x, y| {
        let point = Point::new(x as f32 + 0.5, y as f32 + 0.5);
        let value = if mask_target {
            render::own_mask(layer, point)
        } else {
            render::layer_alpha(source, layer, point, 0)
        };
        Luma([(value * 255.0).round() as u8])
    });
    document.selection = Some(Arc::new(mask));
}

/// Select → Layer's Pixels: the active layer's opacity, ignoring its mask, becomes the
/// selection (soft where the layer is partly transparent), as Compositor's does.
pub fn selection_from_layer_pixels(document: &mut Document) -> bool {
    let Some(layer) = document.active().filter(|l| l.pixels.is_some() && !l.group) else {
        return false;
    };
    let mut bare = layer.clone();
    bare.mask = None;
    bare.clip_to = None;
    bare.opacity = 1.0;
    let mask = GrayImage::from_fn(document.width, document.height, |x, y| {
        let point = Point::new(x as f32 + 0.5, y as f32 + 0.5);
        Luma([(render::layer_alpha(document, &bare, point, 0) * 255.0).round() as u8])
    });
    document.selection = Some(Arc::new(mask));
    true
}

/// Select → Mask's Black Areas: what the active layer's mask hides becomes the selection,
/// as Compositor's does. Grey is partly selected; outside the mask nothing is.
pub fn selection_from_mask_black(document: &mut Document) -> bool {
    let Some(layer) = document.active() else {
        return false;
    };
    let Some(mask) = &layer.mask else {
        return false;
    };
    let placement = mask.placement.unwrap_or(layer.transform);
    let pixels = mask.pixels.clone();
    let selection = GrayImage::from_fn(document.width, document.height, |x, y| {
        let unit = placement.inverse(Point::new(x as f32 + 0.5, y as f32 + 0.5));
        if !(0.0..1.0).contains(&unit.x) || !(0.0..1.0).contains(&unit.y) {
            return Luma([0]);
        }
        Luma([255 - (render::mask_sample(&pixels, unit) * 255.0).round() as u8])
    });
    document.selection = Some(Arc::new(selection));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_a_standalone_mask_bakes_all_lower_siblings() {
        let mut document = Document::new(4, 2).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_pixel(
            4,
            2,
            image::Rgba([255, 0, 0, 255]),
        )));
        document.insert(Layer::image(
            "Top",
            RgbaImage::from_pixel(4, 2, image::Rgba([0, 255, 0, 255])),
        ));
        group(&mut document);
        document.insert(Layer::image(
            "Middle",
            RgbaImage::from_pixel(4, 2, image::Rgba([0, 0, 255, 255])),
        ));
        let mut mask = Layer::mask("Mask", 4, 2);
        mask.mask.as_mut().unwrap().pixels = Arc::new(GrayImage::from_pixel(4, 2, Luma([128])));
        document.insert(mask);
        let before = render::render(&document);
        merge_selected(&mut document, true).unwrap();
        document.validate().unwrap();
        assert_eq!(render::render(&document), before);
        assert_eq!(document.layers.len(), 3);
        assert!(!document.layers.iter().any(|l| l.standalone_mask));
    }

    #[test]
    fn linked_placed_masks_follow_transforms_and_unlinked_masks_stay_put() {
        let mut doc = Document::new(20, 20).unwrap();
        doc.layers[0].mask = Some(crate::document::Mask {
            placement: Some(Transform {
                x: 4.0,
                ..Transform::new(20, 20)
            }),
            ..crate::document::Mask::white()
        });
        let mut moved = doc.layers[0].transform;
        moved.x = 10.0;
        apply_transform(&mut doc, moved, false).unwrap();
        assert_eq!(
            doc.layers[0].mask.as_ref().unwrap().placement.unwrap().x,
            14.0
        );
        doc.layers[0].mask.as_mut().unwrap().linked = false;
        moved.x = 15.0;
        apply_transform(&mut doc, moved, false).unwrap();
        assert_eq!(
            doc.layers[0].mask.as_ref().unwrap().placement.unwrap().x,
            14.0
        );
    }

    #[test]
    fn resizing_rotated_layers_scales_all_corners() {
        let mut doc = Document::new(100, 100).unwrap();
        doc.layers[0].transform.rotation = 35.0;
        let corners = doc.layers[0].transform.corners();
        image_size(&mut doc, 200, 50).unwrap();
        for (old, new) in corners.into_iter().zip(doc.layers[0].transform.corners()) {
            assert!(new.distance(Point::new(old.x * 2.0, old.y * 0.5)) < 0.001);
        }
    }

    #[test]
    fn merging_inside_a_masked_folder_applies_ancestor_coverage_once() {
        let mut doc = Document::new(4, 4).unwrap();
        doc.layers.clear();
        let mut folder = Layer::blank("folder", 4, 4);
        folder.group = true;
        folder.opacity = 0.5;
        let mut a = Layer::image(
            "a",
            RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255])),
        );
        a.parent = Some(folder.id);
        let mut b = Layer::image(
            "b",
            RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 0])),
        );
        b.parent = Some(folder.id);
        doc.layers = vec![a.clone(), b.clone(), folder];
        doc.active = Some(b.id);
        doc.selected = [a.id, b.id].into_iter().collect();
        let before = render::render(&doc);
        merge_selected(&mut doc, false).unwrap();
        assert_eq!(render::render(&doc), before);
        doc.validate().unwrap();
    }

    /// Merging layers clipped to a folder that is not merged keeps the folder's shape, though
    /// the merge renders with the folder's layers hidden; merging into the folder's shape,
    /// layers clipped to merged layers clip to the result.
    #[test]
    fn merging_layers_clipped_to_a_folder_keeps_their_shape() {
        let mut doc = Document::new(4, 2).unwrap();
        doc.layers.clear();
        let backdrop = Layer::image(
            "Backdrop",
            RgbaImage::from_pixel(4, 2, image::Rgba([10, 200, 10, 255])),
        );
        let mut folder = Layer::blank("Figure", 4, 2);
        folder.group = true;
        folder.opacity = 0.6;
        let mut shape = Layer::image(
            "Shape",
            RgbaImage::from_fn(4, 2, |x, _| {
                image::Rgba([200, 0, 0, if x < 2 { 255 } else { 0 }])
            }),
        );
        shape.parent = Some(folder.id);
        let mut shading = Layer::image(
            "Shading",
            RgbaImage::from_pixel(4, 2, image::Rgba([0, 0, 255, 255])),
        );
        shading.clip_to = Some(folder.id);
        let mut invert = Layer::blank("Invert", 4, 2);
        invert.adjustment = Some(crate::document::Adjustment::Invert);
        invert.clip_to = Some(folder.id);
        let ids = [backdrop.id, folder.id, shape.id, shading.id, invert.id];
        doc.layers = vec![backdrop, shape, folder, shading, invert];
        doc.select(ids[0], false);
        doc.validate().unwrap();
        // The clipped layers merge into one layer with the folder's shape and opacity.
        let mut merged = doc.clone();
        merged.selected = [ids[3], ids[4]].into_iter().collect();
        merged.active = Some(ids[4]);
        merge_selected(&mut merged, false).unwrap();
        merged.validate().unwrap();
        let layer = merged.active().unwrap();
        assert_eq!(layer.clip_to, None);
        let pixels = layer.pixels.as_ref().unwrap();
        for (x, _, pixel) in pixels.enumerate_pixels() {
            assert_eq!(pixel[3], if x < 2 { 153 } else { 0 }, "{x}: {pixel:?}");
        }
        // Grouping the shading layer and moving that folder into the figure would make the
        // figure's shape depend on itself: that clipping is released, the rest is kept.
        let mut moved = doc.clone();
        moved.select(ids[3], false);
        group(&mut moved);
        let wrapper = moved.active.unwrap();
        moved.validate().unwrap();
        move_layer(&mut moved, wrapper, ids[1], Placement::Inside);
        moved.validate().unwrap();
        let clip_of = |doc: &Document, id| doc.layers.iter().find(|l| l.id == id).unwrap().clip_to;
        assert_eq!(clip_of(&moved, ids[3]), None);
        assert_eq!(clip_of(&moved, ids[4]), Some(ids[1]));
        // Ungrouping releases the layers clipped to the folder.
        doc.select(ids[1], false);
        ungroup(&mut doc);
        doc.validate().unwrap();
        assert!(doc.layers.iter().all(|l| l.clip_to.is_none()));
    }

    #[test]
    fn group_rotation_moves_children_about_a_shared_center() {
        let mut doc = Document::new(100, 100).unwrap();
        doc.layers.clear();
        let mut left = Layer::blank("Left", 10, 10);
        left.transform.x = 10.0;
        left.transform.y = 20.0;
        let mut right = Layer::blank("Right", 10, 10);
        right.transform.x = 70.0;
        right.transform.y = 20.0;
        doc.selected = [left.id, right.id].into_iter().collect();
        doc.active = Some(left.id);
        doc.layers = vec![left, right];
        let old = transform_box(&doc, false).unwrap();
        let before = doc
            .layers
            .iter()
            .map(|l| l.transform.corners())
            .collect::<Vec<_>>();
        let mut new = old;
        new.rotation = 90.0;
        apply_transform(&mut doc, new, false).unwrap();
        for (layer, corners) in doc.layers.iter().zip(before) {
            for (before, after) in corners.into_iter().zip(layer.transform.corners()) {
                assert!(new.point(old.inverse(before)).distance(after) < 0.001);
            }
        }
        doc.validate().unwrap();
    }

    #[test]
    fn resize_and_crop_preserve_source_resolution_and_undo_assets() {
        let mut doc = Document::new(100, 80).unwrap();
        doc.layers[0] = Layer::image("Image", RgbaImage::new(100, 80));
        let original = doc.layers[0].pixels.clone().unwrap();
        image_size(&mut doc, 50, 40).unwrap();
        assert!(Arc::ptr_eq(
            &original,
            doc.layers[0].pixels.as_ref().unwrap()
        ));
        assert_eq!(doc.layers[0].transform.width, 50.0);
        crop(&mut doc, Point::new(10.0, 5.0), Point::new(40.0, 35.0)).unwrap();
        assert_eq!((doc.width, doc.height), (30, 30));
        assert_eq!(doc.layers[0].transform.x, -10.0);
    }

    #[test]
    fn duplicate_folder_remaps_child_and_clipping_references() {
        let mut doc = Document::new(2, 2).unwrap();
        doc.layers[0].group = true;
        let parent = doc.layers[0].id;
        doc.insert(Layer::blank("Child", 2, 2));
        doc.select(parent, false);
        duplicate(&mut doc);
        doc.validate().unwrap();
        assert_eq!(doc.layers.len(), 4);
        assert_ne!(doc.active, Some(parent));
        assert_eq!(doc.descendants(doc.active.unwrap()).len(), 2);
    }

    #[test]
    fn guides_stay_on_their_content_through_canvas_operations() {
        use crate::layout::{Guide, GuideAxis};
        let positions = |document: &Document| -> Vec<f32> {
            document.guides.iter().map(|g| g.position).collect()
        };
        let mut document = Document::new(100, 50).unwrap();
        document.guides = vec![
            Guide::new(GuideAxis::Vertical, 30.0),
            Guide::new(GuideAxis::Horizontal, 10.0),
        ];
        crop(&mut document, Point::new(10.0, 5.0), Point::new(90.0, 45.0)).unwrap();
        assert_eq!(positions(&document), [20.0, 5.0]);
        canvas_size(&mut document, 100, 60, [0.5, 0.5]).unwrap();
        assert_eq!(positions(&document), [30.0, 15.0]);
        image_size(&mut document, 200, 30).unwrap();
        assert_eq!(positions(&document), [60.0, 7.5]);
        flip_canvas(&mut document, true);
        assert_eq!(positions(&document), [140.0, 7.5]);
        flip_canvas(&mut document, false);
        assert_eq!(positions(&document), [140.0, 22.5]);
    }

    #[test]
    fn extending_the_canvas_moves_content_like_canvas_size() {
        use crate::layout::{Guide, GuideAxis};
        let setup = || {
            let mut document = Document::new(100, 50).unwrap();
            document.layers[0].transform.x = 10.0;
            document.layers[0].transform.y = 5.0;
            document.layers[0].mask = Some(crate::document::Mask {
                placement: Some(Transform::new(100, 50)),
                ..crate::document::Mask::white()
            });
            document.guides = vec![
                Guide::new(GuideAxis::Vertical, 30.0),
                Guide::new(GuideAxis::Horizontal, 10.0),
            ];
            document.selection = Some(Arc::new(GrayImage::new(100, 50)));
            document
        };
        let state = |document: &Document| {
            let layer = &document.layers[0];
            let placement = layer.mask.as_ref().unwrap().placement.unwrap();
            (
                (document.width, document.height),
                (layer.transform.x, layer.transform.y),
                (placement.x, placement.y),
                document
                    .guides
                    .iter()
                    .map(|g| g.position)
                    .collect::<Vec<_>>(),
                document.selection.is_some(),
            )
        };
        let mut extended = setup();
        extend_canvas(&mut extended, 20, 10, 0, 30).unwrap();
        assert_eq!(
            state(&extended),
            (
                (120, 90),
                (30.0, 15.0),
                (20.0, 10.0),
                vec![50.0, 20.0],
                false
            )
        );
        // Canvas Size to the same size, anchored right and a quarter down.
        let mut resized = setup();
        canvas_size(&mut resized, 120, 90, [1.0, 0.25]).unwrap();
        assert_eq!(state(&resized), state(&extended));
        // Nothing to add changes nothing but the selection, as Canvas Size does.
        let mut same = setup();
        extend_canvas(&mut same, 0, 0, 0, 0).unwrap();
        assert_eq!(same.layers[0].transform.x, 10.0);
        // The document size limits apply, overflow included.
        let mut document = setup();
        let error = extend_canvas(&mut document, 29_901, 0, 0, 0).unwrap_err();
        assert!(error.to_string().contains("30000"), "{error}");
        let error = extend_canvas(&mut document, 19_900, 9_950, 0, 0).unwrap_err();
        assert!(error.to_string().contains("100 megapixels"), "{error}");
        assert!(extend_canvas(&mut document, u32::MAX, 0, u32::MAX, 0).is_err());
        assert_eq!(state(&document), state(&setup()));
    }

    #[test]
    fn layer_pixels_and_mask_black_areas_become_the_selection() {
        let mut document = Document::new(4, 2).unwrap();
        let mut layer = Layer::image(
            "Top",
            RgbaImage::from_fn(4, 2, |x, _| {
                image::Rgba([9, 9, 9, [0, 128, 255, 255][x as usize]])
            }),
        );
        let mut mask = crate::document::Mask::white();
        mask.pixels = Arc::new(GrayImage::from_fn(4, 2, |x, _| {
            Luma([[255, 0, 64, 255][x as usize]])
        }));
        layer.mask = Some(mask);
        layer.opacity = 0.5;
        document.insert(layer);
        assert!(selection_from_layer_pixels(&mut document));
        // The layer's own opacity, ignoring its mask and layer opacity.
        assert_eq!(
            document.selection.as_ref().unwrap().as_raw()[..4],
            [0, 128, 255, 255]
        );
        assert!(selection_from_mask_black(&mut document));
        assert_eq!(
            document.selection.as_ref().unwrap().as_raw()[..4],
            [0, 255, 191, 0]
        );
        document.active_mut().unwrap().mask = None;
        assert!(!selection_from_mask_black(&mut document));
    }

    fn px(n: u8) -> Rgba<u8> {
        Rgba([n * 10, 0, 0, 255])
    }

    /// A 3 × 2 document with distinct pixels: row 0 is 1 2 3, row 1 is 4 5 6.
    fn asymmetric() -> Document {
        let mut document = Document::new(3, 2).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_fn(3, 2, |x, y| {
            px((y * 3 + x + 1) as u8)
        })));
        document
    }

    fn rows(image: &RgbaImage) -> Vec<Vec<u8>> {
        (0..image.height())
            .map(|y| {
                (0..image.width())
                    .map(|x| image.get_pixel(x, y)[0] / 10)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn rotating_the_canvas_turns_the_pixels_and_swaps_the_size() {
        let cases = [
            (
                CanvasRotation::Clockwise,
                (2, 3),
                vec![vec![4, 1], vec![5, 2], vec![6, 3]],
            ),
            (
                CanvasRotation::CounterClockwise,
                (2, 3),
                vec![vec![3, 6], vec![2, 5], vec![1, 4]],
            ),
            (
                CanvasRotation::Half,
                (3, 2),
                vec![vec![6, 5, 4], vec![3, 2, 1]],
            ),
        ];
        for (rotation, size, expected) in cases {
            let mut document = asymmetric();
            rotate_canvas(&mut document, rotation);
            assert_eq!((document.width, document.height), size, "{rotation:?}");
            assert_eq!(rows(&render::render(&document)), expected, "{rotation:?}");
        }
    }

    #[test]
    fn four_quarter_turns_and_opposite_turns_restore_the_document() {
        use crate::layout::{Guide, GuideAxis};
        let mut document = asymmetric();
        document.layers[0].transform.rotation = 90.0;
        document.layers[0].transform.x = 0.5;
        document.guides = vec![Guide::new(GuideAxis::Vertical, 1.0)];
        let original = (document.layers[0].transform, document.guides[0]);
        for _ in 0..4 {
            rotate_canvas(&mut document, CanvasRotation::Clockwise);
        }
        assert_eq!((document.width, document.height), (3, 2));
        assert_eq!((document.layers[0].transform, document.guides[0]), original);
        rotate_canvas(&mut document, CanvasRotation::Clockwise);
        rotate_canvas(&mut document, CanvasRotation::CounterClockwise);
        assert_eq!((document.layers[0].transform, document.guides[0]), original);
        rotate_canvas(&mut document, CanvasRotation::Half);
        rotate_canvas(&mut document, CanvasRotation::Half);
        assert_eq!(document.layers[0].transform, original.0);
    }

    #[test]
    fn offset_and_turned_layers_export_like_the_rotated_export() {
        let mut document = Document::new(5, 3).unwrap();
        let mut offset = Layer::image("Offset", RgbaImage::from_fn(2, 1, |x, _| px(x as u8 + 1)));
        offset.transform.x = 2.0;
        offset.transform.y = 1.0;
        let mut turned = Layer::image("Turned", RgbaImage::from_fn(3, 1, |x, _| px(x as u8 + 7)));
        turned.transform.y = 1.0;
        turned.transform.rotation = 90.0;
        turned.transform.flip_x = true;
        document.insert(offset);
        document.insert(turned);
        let before = render::render(&document);
        for (rotation, expected) in [
            (
                CanvasRotation::Clockwise,
                image::imageops::rotate90(&before),
            ),
            (
                CanvasRotation::CounterClockwise,
                image::imageops::rotate270(&before),
            ),
            (CanvasRotation::Half, image::imageops::rotate180(&before)),
        ] {
            let mut rotated = document.clone();
            rotate_canvas(&mut rotated, rotation);
            assert_eq!(render::render(&rotated), expected, "{rotation:?}");
        }
    }

    #[test]
    fn masks_paths_guides_and_the_selection_turn_with_the_canvas() {
        use crate::layout::{Guide, GuideAxis};
        let mut document = Document::new(6, 4).unwrap();
        document.layers[0].mask = Some(crate::document::Mask {
            placement: Some(Transform {
                x: 1.0,
                y: 0.0,
                ..Transform::new(2, 2)
            }),
            ..crate::document::Mask::white()
        });
        document.paths = vec![crate::vector::NamedPath::new(
            "Path",
            crate::vector::VectorPath::parse("M 0 0 L 4 0 L 4 1 Z").unwrap(),
        )];
        document.guides = vec![
            Guide::new(GuideAxis::Vertical, 1.0),
            Guide::new(GuideAxis::Horizontal, 1.0),
        ];
        let mut mask = GrayImage::new(6, 4);
        mask.put_pixel(5, 0, Luma([255]));
        document.selection = Some(Arc::new(mask));
        rotate_canvas(&mut document, CanvasRotation::Clockwise);
        // (x, y) -> (4 - y, x): the mask's centre (2, 1) lands on (3, 2).
        let placement = document.layers[0].mask.as_ref().unwrap().placement.unwrap();
        assert_eq!((placement.center().x, placement.center().y), (3.0, 2.0));
        assert_eq!(placement.rotation, 90.0);
        let bounds = document.paths[0].d.bounds().unwrap();
        assert_eq!(
            (bounds.x0, bounds.y0, bounds.x1, bounds.y1),
            (3.0, 0.0, 4.0, 4.0)
        );
        assert_eq!(
            document
                .guides
                .iter()
                .map(|g| (g.axis, g.position))
                .collect::<Vec<_>>(),
            [(GuideAxis::Horizontal, 1.0), (GuideAxis::Vertical, 3.0)]
        );
        let selection = document.selection.as_ref().unwrap();
        assert_eq!(selection.dimensions(), (4, 6));
        assert_eq!(selection::bounds(selection), Some((4 - 1, 5, 4, 6)));
    }

    #[test]
    fn crop_to_selection_makes_the_canvas_the_selection_bounds() {
        let mut document = Document::new(8, 6).unwrap();
        assert!(crop_to_selection(&mut document).is_err());
        let mut mask = GrayImage::new(8, 6);
        for y in 1..3 {
            for x in 2..5 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        document.selection = Some(Arc::new(mask));
        crop_to_selection(&mut document).unwrap();
        assert_eq!((document.width, document.height), (3, 2));
        assert_eq!(document.layers[0].transform.x, -2.0);
        assert!(document.selection.is_none());
    }

    fn margins() -> Document {
        let mut document = Document::new(7, 5).unwrap();
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_fn(7, 5, |x, y| {
            if (2..5).contains(&x) && (1..3).contains(&y) {
                px(5)
            } else {
                Rgba([0, 0, 0, 0])
            }
        })));
        document
    }

    #[test]
    fn trim_removes_transparent_margins() {
        let mut document = margins();
        assert!(trim(&mut document, TrimBasis::Transparent, TrimSides::default()).unwrap());
        assert_eq!((document.width, document.height), (3, 2));
        assert!(
            rows(&render::render(&document))
                .iter()
                .flatten()
                .all(|v| *v == 5)
        );
        // Nothing left to trim.
        assert!(!trim(&mut document, TrimBasis::Transparent, TrimSides::default()).unwrap());
        assert_eq!((document.width, document.height), (3, 2));
    }

    #[test]
    fn trim_honours_the_chosen_sides() {
        let sides = |top, bottom, left, right| TrimSides {
            top,
            bottom,
            left,
            right,
        };
        for (sides, size) in [
            (sides(true, false, false, false), (7, 4)),
            (sides(false, true, false, false), (7, 3)),
            (sides(false, false, true, false), (5, 5)),
            (sides(false, false, false, true), (5, 5)),
            (sides(true, true, false, false), (7, 2)),
        ] {
            let mut document = margins();
            assert!(trim(&mut document, TrimBasis::Transparent, sides).unwrap());
            assert_eq!((document.width, document.height), size, "{sides:?}");
        }
        let mut document = margins();
        let none = sides(false, false, false, false);
        assert!(!trim(&mut document, TrimBasis::Transparent, none).unwrap());
    }

    #[test]
    fn trim_by_corner_colour_removes_a_flat_border() {
        let mut document = Document::new(6, 5).unwrap();
        let border = Rgba([9, 9, 9, 255]);
        document.layers[0].pixels = Some(Arc::new(RgbaImage::from_fn(6, 5, |x, y| {
            if (1..4).contains(&x) && (2..4).contains(&y) {
                px(3)
            } else {
                border
            }
        })));
        let mut by_top_left = document.clone();
        assert!(trim(&mut by_top_left, TrimBasis::TopLeft, TrimSides::default()).unwrap());
        assert_eq!((by_top_left.width, by_top_left.height), (3, 2));
        let mut by_bottom_right = document.clone();
        assert!(
            trim(
                &mut by_bottom_right,
                TrimBasis::BottomRight,
                TrimSides::default()
            )
            .unwrap()
        );
        assert_eq!((by_bottom_right.width, by_bottom_right.height), (3, 2));
        // A fully opaque image has no transparent margin.
        assert!(!trim(&mut document, TrimBasis::Transparent, TrimSides::default()).unwrap());
    }
}

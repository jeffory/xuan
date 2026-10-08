use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, ensure};
use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::blend::BlendMode;

pub use crate::limits::MAX_SIDE;
pub const MAX_LAYERS: usize = 10_000;

/// Check a canvas, layer or mask size against [`MAX_SIDE`] and this computer's
/// [`image_pixels`](crate::limits::Limits::image_pixels).
pub fn validate_size(width: u32, height: u32) -> Result<()> {
    ensure!(
        (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height),
        "Dimensions must be between 1 and {} pixels",
        crate::limits::grouped(MAX_SIDE.into())
    );
    let limit = crate::limits::get().image_pixels;
    ensure!(
        u64::from(width) * u64::from(height) <= limit,
        "Images are limited to {} on this computer",
        crate::limits::megapixels(limit)
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    #[serde(default)]
    pub warp: Option<[Point; 4]>,
}

impl Transform {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
            rotation: 0.0,
            flip_x: false,
            flip_y: false,
            warp: None,
        }
    }

    pub fn center(self) -> Point {
        Point::new(self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    /// Map normalized source coordinates to document coordinates.
    pub fn point(self, unit: Point) -> Point {
        let unit = self
            .warp
            .and_then(crate::geometry::Homography::from_quad)
            .map_or(unit, |h| h.map(unit));
        let x = (if self.flip_x { 1.0 - unit.x } else { unit.x } - 0.5) * self.width;
        let y = (if self.flip_y { 1.0 - unit.y } else { unit.y } - 0.5) * self.height;
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let center = self.center();
        Point::new(center.x + cos * x - sin * y, center.y + sin * x + cos * y)
    }

    pub fn inverse(self, point: Point) -> Point {
        let center = self.center();
        let x = point.x - center.x;
        let y = point.y - center.y;
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let mut u = (cos * x + sin * y) / self.width + 0.5;
        let mut v = (-sin * x + cos * y) / self.height + 0.5;
        if self.flip_x {
            u = 1.0 - u;
        }
        if self.flip_y {
            v = 1.0 - v;
        }
        let unit = Point::new(u, v);
        self.warp
            .and_then(crate::geometry::Homography::from_quad)
            .and_then(|h| h.inverse())
            .map_or(unit, |h| h.map(unit))
    }

    pub fn corners(self) -> [Point; 4] {
        [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(0.0, 1.0),
        ]
        .map(|point| self.point(point))
    }

    /// The affine map from a `width` × `height` box stretched over the layer to document
    /// coordinates, unless the layer is warped (then no affine map fits).
    pub fn box_affine(self, width: f32, height: f32) -> Option<kurbo::Affine> {
        if self.warp.is_some() {
            return None;
        }
        let o = self.point(Point::new(0.0, 0.0));
        let x = self.point(Point::new(1.0, 0.0));
        let y = self.point(Point::new(0.0, 1.0));
        let (w, h) = (f64::from(width), f64::from(height));
        Some(kurbo::Affine::new([
            f64::from(x.x - o.x) / w,
            f64::from(x.y - o.y) / w,
            f64::from(y.x - o.x) / h,
            f64::from(y.y - o.y) / h,
            f64::from(o.x),
            f64::from(o.y),
        ]))
    }

    pub fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height, self.rotation]
            .iter()
            .all(|x| x.is_finite())
            && (1.0..=300_000.0).contains(&self.width)
            && (1.0..=300_000.0).contains(&self.height)
            && self.x.abs() <= 1_000_000.0
            && self.y.abs() <= 1_000_000.0
            && self
                .warp
                .is_none_or(|quad| crate::geometry::Homography::from_quad(quad).is_some())
    }

    /// Enlarge the source grid without moving any of its existing pixels.
    pub fn expanded(self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        let corners = [
            Point::new(left, top),
            Point::new(right, top),
            Point::new(right, bottom),
            Point::new(left, bottom),
        ]
        .map(|p| self.point(p));
        let center = self.point(Point::new((left + right) * 0.5, (top + bottom) * 0.5));
        let mut result = self;
        result.width *= right - left;
        result.height *= bottom - top;
        result.x = center.x - result.width * 0.5;
        result.y = center.y - result.height * 0.5;
        result.warp = None;
        result.warp = Some(corners.map(|p| result.inverse(p)));
        result
    }

    pub fn following(self, old: Self, new: Self) -> Self {
        if old.width == new.width
            && old.height == new.height
            && old.rotation == new.rotation
            && old.flip_x == new.flip_x
            && old.flip_y == new.flip_y
            && old.warp == new.warp
        {
            return Self {
                x: self.x + new.x - old.x,
                y: self.y + new.y - old.y,
                ..self
            };
        }
        let map = |point| new.point(old.inverse(point));
        let center = map(self.center());
        let mut result = self;
        result.width = (self.width * new.width / old.width).max(1.0);
        result.height = (self.height * new.height / old.height).max(1.0);
        result.x = center.x - result.width * 0.5;
        result.y = center.y - result.height * 0.5;
        result.rotation += new.rotation - old.rotation;
        result.warp = None;
        let quad = self.corners().map(|p| result.inverse(map(p)));
        if crate::geometry::Homography::from_quad(quad).is_some() {
            result.warp = Some(quad);
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mask {
    #[serde(skip)]
    pub pixels: Arc<GrayImage>,
    pub enabled: bool,
    pub linked: bool,
    pub placement: Option<Transform>,
}

impl Mask {
    pub fn white() -> Self {
        Self {
            pixels: Arc::new(GrayImage::from_pixel(1, 1, image::Luma([255]))),
            enabled: true,
            linked: true,
            placement: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Adjustment {
    HueRanges {
        settings: Box<crate::color::HueSettings>,
    },
    LevelsChannels {
        ranges: [[f32; 5]; 4],
    },
    CurvesChannels {
        channels: [Vec<Point>; 4],
    },
    HueSaturation {
        hue: f32,
        saturation: f32,
        lightness: f32,
        colorize: bool,
    },
    Levels {
        black: f32,
        gamma: f32,
        white: f32,
        output_black: f32,
        output_white: f32,
    },
    Curves {
        points: Vec<Point>,
    },
    Exposure {
        exposure: f32,
        offset: f32,
        gamma: f32,
    },
    GradientMap {
        shadows: [u8; 4],
        highlights: [u8; 4],
    },
    FilmGrain {
        amount: f32,
        size: f32,
        roughness: f32,
        seed: u32,
    },
    Grain {
        amount: f32,
        monochrome: bool,
        seed: u32,
    },
    Invert,
    /// Photoshop's Black & White (upstream's `BlackWhiteSettings`): how bright each family of
    /// colors becomes in gray, in percent (−200…300), ordered red, yellow, green, cyan, blue,
    /// magenta; optionally tinted with a hue (0…360°) at a saturation (0…100%). Format 7.
    BlackWhite {
        weights: [f32; 6],
        tint: bool,
        tint_hue: f32,
        tint_saturation: f32,
    },
    /// Photoshop's Color Balance (upstream's `ColorBalanceSettings`): for shadows, midtones and
    /// highlights, shifts toward red (from cyan), green (from magenta) and blue (from yellow), in
    /// percent (−100…100). Format 7.
    ColorBalance {
        shadows: [f32; 3],
        midtones: [f32; 3],
        highlights: [f32; 3],
        preserve_luminosity: bool,
    },
}

impl Adjustment {
    /// Photoshop's defaults for Black & White (reds 40, yellows 60, greens 40, cyans 60, blues 20,
    /// magentas 80; a 20% tint at 40° when tinting).
    pub const BLACK_WHITE: Self = Self::BlackWhite {
        weights: [40.0, 60.0, 40.0, 60.0, 20.0, 80.0],
        tint: false,
        tint_hue: 40.0,
        tint_saturation: 20.0,
    };
    pub const COLOR_BALANCE: Self = Self::ColorBalance {
        shadows: [0.0; 3],
        midtones: [0.0; 3],
        highlights: [0.0; 3],
        preserve_luminosity: true,
    };

    /// Whether .xuan format 6 and earlier can store this adjustment.
    pub fn is_legacy(&self) -> bool {
        !matches!(self, Self::BlackWhite { .. } | Self::ColorBalance { .. })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::HueSaturation { .. } => "Hue/Saturation",
            Self::HueRanges { .. } => "Hue/Saturation",
            Self::LevelsChannels { .. } => "Levels",
            Self::CurvesChannels { .. } => "Curves",
            Self::Levels { .. } => "Levels",
            Self::Curves { .. } => "Curves",
            Self::Exposure { .. } => "Exposure",
            Self::GradientMap { .. } => "Gradient Map",
            Self::Grain { .. } | Self::FilmGrain { .. } => "Grain",
            Self::Invert => "Invert",
            Self::BlackWhite { .. } => "Black & White",
            Self::ColorBalance { .. } => "Color Balance",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShapeStyle {
    pub kind: crate::paint::ShapeKind,
    pub color: [u8; 4],
    pub corner_radius: f32,
    /// The outline of a `Path` shape (format 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<ShapePath>,
}

/// A path shape's outline: SVG path data in the coordinates of a `width` × `height` box,
/// which the layer's pixels cover, so the path stretches with the layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapePath {
    pub d: crate::vector::VectorPath,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub fill_rule: crate::vector::FillRule,
}

impl ShapePath {
    pub fn validate(&self) -> Result<()> {
        self.d.validate()?;
        ensure!(
            [self.width, self.height]
                .iter()
                .all(|v| v.is_finite() && (1.0..=300_000.0).contains(v)),
            "Invalid path shape box"
        );
        Ok(())
    }

    /// The outline in document coordinates for a layer placed by `transform`, unless the
    /// layer is warped (then it is no longer an affine image of the box).
    pub fn in_document(&self, transform: Transform) -> Option<crate::vector::VectorPath> {
        if transform.warp.is_some() {
            return None;
        }
        let o = transform.point(Point::new(0.0, 0.0));
        let x = transform.point(Point::new(1.0, 0.0));
        let y = transform.point(Point::new(0.0, 1.0));
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        let affine = kurbo::Affine::new([
            f64::from(x.x - o.x) / w,
            f64::from(x.y - o.y) / w,
            f64::from(y.x - o.x) / h,
            f64::from(y.y - o.y) / h,
            f64::from(o.x),
            f64::from(o.y),
        ]);
        Some(self.d.transformed(affine))
    }
}

/// Where a layer produced by a plugin came from, so the action can be repeated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Generated {
    pub plugin: String,
    pub version: String,
    pub action: String,
    #[serde(default)]
    pub inputs: serde_json::Value,
    /// The layer the source pixels were taken from, if it still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Uuid>,
    /// Hash of the source pixels that were sent, to tell whether they changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    /// RFC 3339 timestamp.
    #[serde(default)]
    pub created: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: Uuid,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub transform: Transform,
    pub parent: Option<Uuid>,
    pub group: bool,
    pub clip_to: Option<Uuid>,
    pub mask: Option<Mask>,
    #[serde(default)]
    pub standalone_mask: bool,
    pub adjustment: Option<Adjustment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<crate::effects::Filter>,
    #[serde(default)]
    pub shape: Option<ShapeStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<crate::text::TextStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<crate::raw::RawAsset>,
    /// Stroke, shadows, overlay and glows drawn around the layer's pixels (format 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<crate::layer_effects::LayerEffects>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<Generated>,
    /// The model and sampler a plugin reported for the pixels (format 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::provenance::Provenance>,
    #[serde(skip)]
    pub pixels: Option<Arc<RgbaImage>>,
}

impl Layer {
    pub fn is_effect(&self) -> bool {
        self.standalone_mask || self.adjustment.is_some() || self.filter.is_some()
    }

    pub fn can_attach_effects(&self) -> bool {
        !self.group && !self.is_effect()
    }

    pub fn blank(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            blend: BlendMode::Normal,
            transform: Transform::new(width, height),
            parent: None,
            group: false,
            clip_to: None,
            mask: None,
            standalone_mask: false,
            adjustment: None,
            filter: None,
            shape: None,
            text: None,
            raw: None,
            effects: None,
            generated: None,
            provenance: None,
            pixels: None,
        }
    }

    pub fn set_transform(&mut self, transform: Transform) {
        if let Some(mask) = &mut self.mask {
            if mask.linked || self.standalone_mask {
                mask.placement = mask
                    .placement
                    .map(|placement| placement.following(self.transform, transform));
            } else if mask.placement.is_none() {
                mask.placement = Some(self.transform);
            }
        }
        self.transform = transform;
    }

    pub fn image(name: impl Into<String>, pixels: RgbaImage) -> Self {
        let mut layer = Self::blank(name, pixels.width(), pixels.height());
        layer.pixels = Some(Arc::new(pixels));
        layer
    }

    pub fn mask(name: impl Into<String>, width: u32, height: u32) -> Self {
        let mut layer = Self::blank(name, width, height);
        layer.standalone_mask = true;
        layer.mask = Some(Mask::white());
        layer
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub id: Uuid,
    pub width: u32,
    pub height: u32,
    pub resolution: f32,
    pub layers: Vec<Layer>,
    pub active: Option<Uuid>,
    /// Alignment guides (format version 5). Missing in older projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<crate::layout::Guide>,
    /// This project's layout grid; `None` uses the app's default grid (format version 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<crate::layout::GridSettings>,
    /// Paths kept with the document, as in Photoshop's Paths panel (format version 10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<crate::vector::NamedPath>,
    #[serde(skip)]
    pub selected: HashSet<Uuid>,
    #[serde(skip)]
    pub selection: Option<Arc<GrayImage>>,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        validate_size(width, height)?;
        let layer = Layer::blank("Layer 1", width, height);
        Ok(Self {
            id: Uuid::new_v4(),
            width,
            height,
            resolution: 72.0,
            active: Some(layer.id),
            selected: HashSet::from([layer.id]),
            layers: vec![layer],
            guides: Vec::new(),
            grid: None,
            paths: Vec::new(),
            selection: None,
        })
    }

    pub fn active(&self) -> Option<&Layer> {
        self.layers
            .iter()
            .find(|layer| Some(layer.id) == self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut Layer> {
        self.layers
            .iter_mut()
            .find(|layer| Some(layer.id) == self.active)
    }

    pub fn select(&mut self, id: Uuid, extend: bool) {
        if !extend {
            self.selected.clear();
        }
        if extend && self.selected.contains(&id) {
            self.selected.remove(&id);
        } else {
            self.selected.insert(id);
        }
        self.active = if self.selected.contains(&id) {
            Some(id)
        } else {
            self.layers
                .iter()
                .rev()
                .find(|layer| self.selected.contains(&layer.id))
                .map(|layer| layer.id)
        };
    }

    pub fn insert(&mut self, mut layer: Layer) {
        let index = self
            .layers
            .iter()
            .position(|layer| Some(layer.id) == self.active);
        if let Some(active) = self.active() {
            layer.parent = if active.group {
                Some(active.id)
            } else {
                self.sibling_parent(active)
            };
        }
        self.select(layer.id, false);
        self.layers
            .insert(index.map_or(self.layers.len(), |i| i + 1), layer);
    }

    /// New standalone layers stay outside an image's attached effect stack.
    pub fn sibling_parent(&self, layer: &Layer) -> Option<Uuid> {
        layer.parent.and_then(|id| {
            let parent = self.layers.iter().find(|l| l.id == id)?;
            if parent.can_attach_effects() {
                parent.parent
            } else {
                Some(id)
            }
        })
    }

    pub fn attachment_owner(&self, layer: &Layer) -> Option<Uuid> {
        if layer.can_attach_effects() {
            Some(layer.id)
        } else {
            layer.parent.filter(|id| {
                self.layers
                    .iter()
                    .any(|l| l.id == *id && l.can_attach_effects())
            })
        }
    }

    /// Upgrade the original single-mask slot to a reorderable child without
    /// changing its placement, coverage, or the selected image.
    pub fn promote_image_masks(&mut self) {
        let mut masks = Vec::new();
        for layer in &mut self.layers {
            if layer.can_attach_effects()
                && let Some(mask) = layer.mask.take()
            {
                let mut child = Layer::mask("Mask", self.width, self.height);
                child.parent = Some(layer.id);
                child.transform = layer.transform;
                child.mask = Some(mask);
                masks.push(child);
            }
        }
        // Legacy masks ran after the image's existing effects.
        self.layers.extend(masks);
    }

    pub fn descendants(&self, id: Uuid) -> HashSet<Uuid> {
        let mut result = HashSet::from([id]);
        loop {
            let old = result.len();
            for layer in &self.layers {
                if layer.parent.is_some_and(|parent| result.contains(&parent)) {
                    result.insert(layer.id);
                }
            }
            if result.len() == old {
                break;
            }
        }
        result
    }

    pub fn transform_targets(&self) -> HashSet<Uuid> {
        let mut result = self.selected.clone();
        for id in &self.selected {
            result.extend(self.descendants(*id));
        }
        result
    }

    pub fn movement_targets(&self) -> HashSet<Uuid> {
        self.transform_targets()
            .into_iter()
            .filter(|id| {
                self.selected.contains(id)
                    || self.layers.iter().find(|l| l.id == *id).is_none_or(|l| {
                        !l.standalone_mask
                            || self.attachment_owner(l).is_none()
                            || l.mask.as_ref().is_none_or(|m| m.linked)
                    })
            })
            .collect()
    }

    pub fn delete_selected(&mut self) {
        let deleted = self.transform_targets();
        self.layers.retain(|layer| !deleted.contains(&layer.id));
        for layer in &mut self.layers {
            if layer.clip_to.is_some_and(|id| deleted.contains(&id)) {
                layer.clip_to = None;
            }
        }
        self.active = self.layers.last().map(|layer| layer.id);
        self.selected = self.active.into_iter().collect();
    }

    /// A layer whose clipping is part of a cycle, where some clipping shape depends on itself.
    /// A layer's clipping shape depends on its base, and a folder's shape on its children,
    /// so a layer inside a folder (or clipped through a chain to a layer inside it) cannot
    /// clip to that folder.
    fn clipping_cycle(&self) -> Option<usize> {
        let index: HashMap<Uuid, usize> = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (l.id, i))
            .collect();
        // A layer's clipping edge comes first among its edges.
        let mut edges: Vec<Vec<usize>> = self
            .layers
            .iter()
            .map(|layer| {
                layer
                    .clip_to
                    .as_ref()
                    .and_then(|id| index.get(id))
                    .copied()
                    .into_iter()
                    .collect()
            })
            .collect();
        let clips: Vec<bool> = edges.iter().map(|e| !e.is_empty()).collect();
        for (i, layer) in self.layers.iter().enumerate() {
            if let Some(&parent) = layer.parent.as_ref().and_then(|id| index.get(id))
                && self.layers[parent].group
            {
                edges[parent].push(i);
            }
        }
        // Iterative depth-first search: 0 unvisited, 1 on the stack, 2 done. Each stack entry
        // holds the index of its next edge, so the edge it last took is one before.
        let mut state = vec![0_u8; self.layers.len()];
        for start in 0..self.layers.len() {
            if state[start] != 0 {
                continue;
            }
            let mut stack = vec![(start, 0)];
            state[start] = 1;
            while let Some(&(node, next)) = stack.last() {
                if let Some(&to) = edges[node].get(next) {
                    stack.last_mut().unwrap().1 += 1;
                    match state[to] {
                        0 => {
                            state[to] = 1;
                            stack.push((to, 0));
                        }
                        1 => {
                            let from = stack.iter().position(|(n, _)| *n == to).unwrap_or(0);
                            let cycle = &stack[from..];
                            return cycle
                                .iter()
                                .find(|(n, next)| clips[*n] && *next == 1)
                                .or_else(|| cycle.iter().find(|(n, _)| clips[*n]))
                                .map(|(n, _)| *n);
                        }
                        _ => {}
                    }
                } else {
                    state[node] = 2;
                    stack.pop();
                }
            }
        }
        None
    }

    /// Whether some clipping shape depends on itself (see [`Self::release_clipping_cycles`]).
    pub fn has_clipping_cycle(&self) -> bool {
        self.clipping_cycle().is_some()
    }

    /// Release clipping that makes a clipping shape depend on itself, as moving a layer that
    /// clips to a folder into that folder would. Other clipping is kept.
    pub fn release_clipping_cycles(&mut self) {
        let groups: HashSet<Uuid> = self
            .layers
            .iter()
            .filter(|l| l.group)
            .map(|l| l.id)
            .collect();
        if !self
            .layers
            .iter()
            .any(|l| l.clip_to.is_some_and(|id| groups.contains(&id)))
        {
            return;
        }
        while let Some(index) = self.clipping_cycle() {
            self.layers[index].clip_to = None;
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_size(self.width, self.height)?;
        ensure!(
            self.resolution.is_finite() && (1.0..=9600.0).contains(&self.resolution),
            "Invalid resolution"
        );
        ensure!(self.layers.len() <= MAX_LAYERS, "Too many layers");
        crate::layout::validate_guides(&self.guides)?;
        if let Some(grid) = &self.grid {
            grid.validate()?;
        }
        crate::vector::validate_paths(&self.paths)?;
        // Indexed so hostile files with many deeply nested layers validate in linear time.
        let ids: std::collections::HashMap<_, _> =
            self.layers.iter().map(|layer| (layer.id, layer)).collect();
        ensure!(
            ids.len() == self.layers.len(),
            "Duplicate layer identifiers"
        );
        ensure!(
            self.active.is_none_or(|id| ids.contains_key(&id)),
            "Missing active layer"
        );
        // Only each image's own size is checked: how many pixels a project holds is limited
        // when a file is opened or imported (see `crate::limits`), so a document edited in
        // the app can always be saved.
        for layer in &self.layers {
            ensure!(
                !layer.standalone_mask
                    || (layer.mask.is_some()
                        && !layer.group
                        && layer.adjustment.is_none()
                        && layer.filter.is_none()
                        && layer.pixels.is_none()
                        && layer.clip_to.is_none()),
                "Invalid standalone mask layer"
            );
            if let Some(raw) = &layer.raw {
                raw.validate()?;
                ensure!(
                    layer.pixels.is_some() && layer.text.is_none() && layer.shape.is_none(),
                    "Invalid RAW layer"
                );
            }
            if let Some(text) = &layer.text {
                text.validate()?;
                ensure!(
                    layer.pixels.is_some() && layer.shape.is_none(),
                    "Invalid text layer"
                );
            }
            if let Some(shape) = &layer.shape {
                ensure!(
                    shape.corner_radius.is_finite()
                        && shape.corner_radius >= 0.0
                        && layer.pixels.is_some()
                        && (shape.kind == crate::paint::ShapeKind::Path) == shape.path.is_some(),
                    "Invalid live shape"
                );
                if let Some(path) = &shape.path {
                    path.validate()?;
                }
            }
            if let Some(adjustment) = &layer.adjustment {
                crate::effects::validate_adjustment(adjustment)?;
            }
            if let Some(provenance) = &layer.provenance {
                provenance.validate()?;
            }
            if let Some(effects) = &layer.effects {
                effects.validate()?;
                // As upstream, only layers with pixels of their own take effects.
                ensure!(
                    !layer.group && !layer.is_effect(),
                    "Layer effects need a pixel layer"
                );
            }
            if let Some(filter) = &layer.filter {
                filter.validate()?;
                ensure!(
                    !layer.group && layer.adjustment.is_none() && layer.clip_to.is_none(),
                    "Invalid filter layer"
                );
            }
            ensure!(
                !layer.name.trim().is_empty() && layer.name.len() <= 16_384,
                "Invalid layer name"
            );
            ensure!(layer.transform.valid(), "Invalid layer transform");
            ensure!(
                layer.opacity.is_finite() && (0.0..=1.0).contains(&layer.opacity),
                "Invalid opacity"
            );
            ensure!(
                !(layer.group || layer.adjustment.is_some() || layer.filter.is_some())
                    || layer.pixels.is_none(),
                "Group/effect cannot contain pixels"
            );
            if let Some(image) = &layer.pixels {
                validate_size(image.width(), image.height())?;
            }
            if let Some(mask) = &layer.mask {
                validate_size(mask.pixels.width(), mask.pixels.height())?;
                ensure!(
                    mask.placement.is_none_or(Transform::valid),
                    "Invalid mask transform"
                );
            }
            let mut parent = layer.parent;
            let mut child = layer;
            let mut visited = HashSet::from([layer.id]);
            while let Some(id) = parent {
                ensure!(
                    visited.insert(id) && visited.len() <= 65,
                    "Cyclic or excessively nested groups"
                );
                let container = ids
                    .get(&id)
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("Missing parent layer"))?;
                ensure!(
                    container.group || (container.can_attach_effects() && child.is_effect()),
                    "Only masks, filters, and adjustments can attach to an image"
                );
                child = container;
                parent = container.parent;
            }
            let mut source = layer.clip_to;
            let mut visited = HashSet::from([layer.id]);
            while let Some(id) = source {
                ensure!(
                    !layer.group && visited.insert(id) && visited.len() <= 257,
                    "Invalid clipping mask graph"
                );
                let target = ids.get(&id).copied();
                ensure!(
                    target.is_some_and(|l| !l.standalone_mask && l.filter.is_none()),
                    "Missing clipping source"
                );
                source = target.and_then(|l| l.clip_to);
            }
        }
        if self
            .layers
            .iter()
            .any(|l| l.clip_to.is_some_and(|id| ids[&id].group))
        {
            ensure!(
                self.clipping_cycle().is_none(),
                "A layer cannot clip to a folder whose shape depends on it"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_each_image_is_size_checked_so_edited_documents_always_save() {
        // 30 layers sharing a 2,000 × 2,000 image: 120 megapixels in all, more than opening a
        // file may bring in (100 megapixels in tests), yet each image is small.
        let side = 2_000;
        let pixels = Arc::new(RgbaImage::new(side, side));
        let mut document = Document::new(side, side).unwrap();
        for i in 0..30 {
            let mut layer = Layer::blank(format!("Copy {i}"), side, side);
            layer.pixels = Some(pixels.clone());
            document.insert(layer);
        }
        let total: u64 = document
            .layers
            .iter()
            .filter_map(|l| l.pixels.as_ref())
            .map(|p| u64::from(p.width()) * u64::from(p.height()))
            .sum();
        assert!(total > crate::limits::get().project_pixels);
        document.validate().unwrap();

        // A single image past the limits is refused, naming the limit.
        assert!(validate_size(MAX_SIDE, 1).is_ok());
        let error = validate_size(MAX_SIDE + 1, 1).unwrap_err().to_string();
        assert!(error.contains("65,535"), "{error}");
        let error = validate_size(20_000, 20_000).unwrap_err().to_string();
        assert!(error.contains("100 megapixels on this computer"), "{error}");
    }

    #[test]
    fn old_layers_default_to_attached_masks_and_standalone_masks_validate() {
        let mut layer = Layer::blank("Existing", 2, 2);
        layer.mask = Some(Mask::white());
        let mut legacy = serde_json::to_value(&layer).unwrap();
        legacy.as_object_mut().unwrap().remove("standalone_mask");
        assert!(
            !serde_json::from_value::<Layer>(legacy)
                .unwrap()
                .standalone_mask
        );

        let mut document = Document::new(2, 2).unwrap();
        document.insert(Layer::mask("Mask", 2, 2));
        document.validate().unwrap();
        document.active_mut().unwrap().mask = None;
        assert!(document.validate().is_err());
        document.active_mut().unwrap().mask = Some(Mask::white());
        document.active_mut().unwrap().pixels = Some(Arc::new(RgbaImage::new(2, 2)));
        assert!(document.validate().is_err());
        document.active_mut().unwrap().pixels = None;
        let mut clipped = Layer::blank("Clipped", 2, 2);
        clipped.clip_to = document.active;
        document.insert(clipped);
        assert!(document.validate().is_err());
    }

    #[test]
    fn image_parents_only_accept_valid_effect_children() {
        let mut doc = Document::new(4, 4).unwrap();
        let owner = doc.active.unwrap();
        let mut child = Layer::blank("Child", 4, 4);
        child.parent = Some(owner);
        let child_id = child.id;
        doc.layers.push(child);
        assert!(doc.validate().is_err());
        doc.layers[1].filter = Some(crate::effects::Filter::GaussianBlur { radius: 2.0 });
        doc.validate().unwrap();
        doc.layers[1].filter = Some(crate::effects::Filter::GaussianBlur { radius: f32::NAN });
        assert!(doc.validate().is_err());
        doc.layers[1].filter = None;
        doc.layers[1].adjustment = Some(Adjustment::Invert);
        doc.validate().unwrap();
        let mut mask = Layer::mask("Nested effect", 4, 4);
        mask.parent = Some(child_id);
        doc.layers.push(mask);
        assert!(doc.validate().is_err());
    }

    #[test]
    fn transform_round_trip_with_rotation_and_flips() {
        for flip_x in [false, true] {
            let t = Transform {
                x: -13.0,
                y: 25.0,
                width: 90.0,
                height: 45.0,
                rotation: 37.0,
                flip_x,
                flip_y: true,
                warp: None,
            };
            let p = Point::new(0.17, 0.89);
            assert!(t.inverse(t.point(p)).distance(p) < 0.00001);
        }
    }

    #[test]
    fn rejects_invalid_hierarchy_and_clipping_cycles() {
        let mut doc = Document::new(8, 8).unwrap();
        doc.layers[0].clip_to = Some(doc.layers[0].id);
        assert!(doc.validate().is_err());
        doc.layers[0].clip_to = None;
        doc.layers[0].parent = Some(Uuid::new_v4());
        assert!(doc.validate().is_err());
    }

    #[test]
    fn folders_are_clipping_bases_unless_the_shape_depends_on_the_clipped_layer() {
        let mut doc = Document::new(8, 8).unwrap();
        doc.layers[0].group = true;
        let group = doc.layers[0].id;
        let mut child = Layer::image("Child", RgbaImage::new(8, 8));
        child.parent = Some(group);
        let child = {
            let id = child.id;
            doc.layers.push(child);
            id
        };
        let mut clipped = Layer::blank("Clipped", 8, 8);
        clipped.clip_to = Some(group);
        let clipped = {
            let id = clipped.id;
            doc.layers.push(clipped);
            id
        };
        doc.validate().unwrap();
        // A chain through a clipped layer reaches the folder too.
        let mut chained = Layer::blank("Chained", 8, 8);
        chained.clip_to = Some(clipped);
        doc.layers.push(chained);
        doc.validate().unwrap();
        // A layer inside the folder cannot clip to it, directly or through a chain.
        doc.layers[1].clip_to = Some(group);
        assert!(doc.validate().is_err());
        doc.layers[1].clip_to = Some(clipped);
        assert!(doc.validate().is_err());
        doc.layers[1].clip_to = None;
        doc.validate().unwrap();
        // Two folders whose layers clip to each other.
        let mut other = Layer::blank("Other", 8, 8);
        other.group = true;
        let mut inside = Layer::image("Inside", RgbaImage::new(8, 8));
        inside.parent = Some(other.id);
        inside.clip_to = Some(group);
        doc.layers[1].clip_to = Some(other.id);
        doc.layers.extend([other, inside]);
        assert!(doc.validate().is_err());
        doc.layers[1].clip_to = None;
        doc.validate().unwrap();
        // A folder still cannot be clipped itself.
        doc.layers[0].clip_to = Some(child);
        assert!(doc.validate().is_err());
    }

    #[test]
    fn deleting_group_removes_descendants_and_stale_links() {
        let mut doc = Document::new(8, 8).unwrap();
        doc.layers[0].group = true;
        let group = doc.layers[0].id;
        doc.insert(Layer::blank("Child", 8, 8));
        let child = doc.active.unwrap();
        let mut outside = Layer::blank("Outside", 8, 8);
        outside.clip_to = Some(child);
        doc.layers.push(outside);
        doc.select(group, false);
        doc.delete_selected();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].clip_to, None);
        doc.validate().unwrap();
    }
}

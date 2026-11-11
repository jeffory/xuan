//! Collages: photos laid out in a grid of cells with borders, like PhotoScape X's Collage.
//!
//! A collage is made of ordinary layers, so everything stays editable. A rectangle shape the
//! size of the canvas, filled with the border colour, sits at the bottom. Each cell is a folder
//! holding a (rounded) rectangle shape, the cell's frame, with the cell's photo clipped to it,
//! so a photo can be moved and scaled within its cell. The document keeps the [`Layout`] and
//! which folders are its cells ([`Collage`], format version 16), so the layout, spacing and
//! borders can be changed later ([`relayout`]); the photos follow their cells.
use std::collections::HashSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    document::{Document, Layer, MAX_SIDE, Transform},
    i18n::tr,
    paint::{self, ShapeKind},
};

/// Most columns or rows a grid may have.
pub const MAX_TRACKS: u32 = 10;
/// Most cells a collage may have: a full grid of [`MAX_TRACKS`] × [`MAX_TRACKS`].
pub const MAX_CELLS: usize = (MAX_TRACKS * MAX_TRACKS) as usize;
/// Fill of a cell's frame, which shows while the cell has no photo.
pub const EMPTY_CELL: [u8; 4] = [204, 204, 204, 255];
/// Largest layer side a transform may have (see [`Transform::valid`]).
const MAX_EXTENT: f32 = 300_000.0;

/// How the cells are arranged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Template {
    /// [`Layout::columns`] × [`Layout::rows`] equal cells.
    #[default]
    Grid,
    /// One large cell on the left and two small ones stacked on the right.
    LargeLeft,
    /// One large cell across the top and three small ones below.
    LargeTop,
}

impl Template {
    pub const ALL: [Self; 3] = [Self::Grid, Self::LargeLeft, Self::LargeTop];

    pub fn name(self) -> &'static str {
        match self {
            Self::Grid => "Grid",
            Self::LargeLeft => "One large, two small",
            Self::LargeTop => "One large above three",
        }
    }
}

/// A collage's arrangement and borders. Sizes are in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub template: Template,
    /// Columns and rows of a [`Template::Grid`], 1–[`MAX_TRACKS`]; other templates keep them
    /// for when the grid is chosen again.
    pub columns: u32,
    pub rows: u32,
    /// Between neighbouring cells.
    pub spacing: u32,
    /// Around the outside of the cells.
    pub border: u32,
    /// The border and spacing colour, as RGBA.
    pub color: [u8; 4],
    /// Of each cell's corners; at most half the cell's shorter side is used.
    pub corner_radius: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            template: Template::Grid,
            columns: 2,
            rows: 2,
            spacing: 20,
            border: 20,
            color: [255, 255, 255, 255],
            corner_radius: 0.0,
        }
    }
}

/// A cell's whole-pixel box on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CellRect {
    fn transform(self) -> Transform {
        Transform {
            x: self.x as f32,
            y: self.y as f32,
            ..Transform::new(self.width, self.height)
        }
    }
}

/// The start and end of each track along one side of the canvas, or `None` when the borders
/// and spacing leave less than a pixel per track.
fn tracks(length: u32, count: u32, border: u32, spacing: u32) -> Option<Vec<(f64, f64)>> {
    let room =
        i64::from(length) - 2 * i64::from(border) - i64::from(count - 1) * i64::from(spacing);
    if room < i64::from(count) {
        return None;
    }
    let track = room as f64 / f64::from(count);
    Some(
        (0..count)
            .map(|i| {
                let start = f64::from(border) + f64::from(i) * (track + f64::from(spacing));
                (start, start + track)
            })
            .collect(),
    )
}

impl Layout {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_TRACKS).contains(&self.columns) && (1..=MAX_TRACKS).contains(&self.rows),
            "A collage grid has 1 to {MAX_TRACKS} columns and rows"
        );
        ensure!(
            self.spacing <= MAX_SIDE && self.border <= MAX_SIDE,
            "Invalid collage spacing"
        );
        ensure!(
            self.corner_radius.is_finite() && (0.0..=MAX_SIDE as f32).contains(&self.corner_radius),
            "Invalid collage corner radius"
        );
        Ok(())
    }

    /// Columns and rows of the grid the cells are placed on.
    fn grid(&self) -> (u32, u32) {
        match self.template {
            Template::Grid => (self.columns, self.rows),
            Template::LargeLeft => (2, 2),
            Template::LargeTop => (3, 2),
        }
    }

    /// Each cell as `[column, row, columns spanned, rows spanned]` on [`Self::grid`], in
    /// reading order.
    fn spans(&self) -> Vec<[u32; 4]> {
        match self.template {
            Template::Grid => (0..self.rows)
                .flat_map(|row| (0..self.columns).map(move |column| [column, row, 1, 1]))
                .collect(),
            Template::LargeLeft => vec![[0, 0, 1, 2], [1, 0, 1, 1], [1, 1, 1, 1]],
            Template::LargeTop => vec![[0, 0, 3, 1], [0, 1, 1, 1], [1, 1, 1, 1], [2, 1, 1, 1]],
        }
    }

    /// How many cells the layout has.
    pub fn cell_count(&self) -> usize {
        self.spans().len()
    }

    /// The cells on a `width` × `height` canvas, in reading order. Equal tracks share the room
    /// the border and spacing leave; cell edges are rounded to whole pixels, so neighbouring
    /// gaps may differ by a pixel.
    pub fn cells(&self, width: u32, height: u32) -> Result<Vec<CellRect>> {
        self.validate()?;
        let (columns, rows) = self.grid();
        let room = || anyhow::anyhow!(tr("The border and spacing leave no room for the photos"));
        let xs = tracks(width, columns, self.border, self.spacing).ok_or_else(room)?;
        let ys = tracks(height, rows, self.border, self.spacing).ok_or_else(room)?;
        Ok(self
            .spans()
            .into_iter()
            .map(|[column, row, across, down]| {
                let x0 = xs[column as usize].0.round();
                let x1 = xs[(column + across - 1) as usize].1.round();
                let y0 = ys[row as usize].0.round();
                let y1 = ys[(row + down - 1) as usize].1.round();
                CellRect {
                    x: x0 as u32,
                    y: y0 as u32,
                    width: (x1 - x0).max(1.0) as u32,
                    height: (y1 - y0).max(1.0) as u32,
                }
            })
            .collect())
    }
}

/// The collage a document was made as: its layout, its border layer and its cells' folders,
/// first cell first. Layers it names may since have been deleted; [`relayout`] recreates them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collage {
    pub layout: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Uuid>,
    #[serde(default)]
    pub cells: Vec<Uuid>,
}

impl Collage {
    pub fn validate(&self) -> Result<()> {
        self.layout.validate()?;
        ensure!(self.cells.len() <= MAX_CELLS, "Too many collage cells");
        let unique: HashSet<_> = self.cells.iter().chain(&self.background).collect();
        ensure!(
            unique.len() == self.cells.len() + usize::from(self.background.is_some()),
            "Duplicate collage cells"
        );
        Ok(())
    }
}

/// A new `width` × `height` document laid out as a collage with empty cells. The first cell's
/// folder is selected.
pub fn new_document(width: u32, height: u32, layout: Layout) -> Result<Document> {
    let mut document = Document::new(width, height)?;
    document.layers.clear();
    document.active = None;
    document.selected.clear();
    document.collage = Some(Box::new(Collage {
        layout,
        background: None,
        cells: Vec::new(),
    }));
    relayout(&mut document, layout)?;
    if let Some(first) = cells(&document).first() {
        document.select(*first, false);
    }
    Ok(document)
}

fn frame_kind(layout: &Layout) -> ShapeKind {
    if layout.corner_radius > 0.0 {
        ShapeKind::RoundedRectangle
    } else {
        ShapeKind::Rectangle
    }
}

/// A rectangle shape over `rect`, drawn later by [`paint::refresh_shapes`].
fn rectangle(name: String, rect: CellRect, kind: ShapeKind, color: [u8; 4], radius: f32) -> Layer {
    let mut layer = Layer::blank(name, rect.width, rect.height);
    layer.transform = rect.transform();
    layer.shape = Some(crate::document::ShapeStyle {
        kind,
        color,
        corner_radius: radius,
        path: None,
    });
    layer
}

/// Restyles a shape layer and lets [`paint::refresh_shapes`] redraw it.
fn restyle(layer: &mut Layer, rect: CellRect, kind: ShapeKind, color: [u8; 4], radius: f32) {
    layer.set_transform(rect.transform());
    if let Some(style) = &mut layer.shape {
        style.kind = kind;
        style.color = color;
        style.corner_radius = radius;
    }
    layer.pixels = None;
}

/// `transform` moved and scaled as its cell goes from `old` to `new`: scaled evenly about the
/// cell's centre by as much as the cell grows on its more-growing side, so a photo that covered
/// the old cell covers the new one. `None` when the result would be too large.
fn follow(transform: Transform, old: Transform, new: Transform) -> Option<Transform> {
    let scale = (new.width / old.width).max(new.height / old.height);
    let (from, to, center) = (old.center(), new.center(), transform.center());
    let width = transform.width * scale;
    let height = transform.height * scale;
    let result = Transform {
        x: to.x + (center.x - from.x) * scale - width * 0.5,
        y: to.y + (center.y - from.y) * scale - height * 0.5,
        width,
        height,
        ..transform
    };
    result.valid().then_some(result)
}

/// Lays `document`'s collage out again with `layout`: the border layer covers the canvas in the
/// border colour, each cell's frame moves to its new box, and the layers in each cell follow
/// their frame. Cells are added or, with their photos, removed to match the layout; border or
/// cell layers that were deleted are made again. A document that is not a collage becomes one.
pub fn relayout(document: &mut Document, layout: Layout) -> Result<()> {
    let rects = layout.cells(document.width, document.height)?;
    let record = document.collage.as_deref().cloned().unwrap_or(Collage {
        layout,
        background: None,
        cells: Vec::new(),
    });
    let kind = frame_kind(&layout);
    let canvas = CellRect {
        x: 0,
        y: 0,
        width: document.width,
        height: document.height,
    };

    // The border layer, at the bottom when it has to be made again.
    let background = record.background.filter(|id| {
        document
            .layers
            .iter()
            .any(|l| l.id == *id && l.shape.as_ref().is_some_and(|s| s.path.is_none()))
    });
    let background = if let Some(id) = background {
        let layer = document.layers.iter_mut().find(|l| l.id == id).unwrap();
        restyle(layer, canvas, ShapeKind::Rectangle, layout.color, 0.0);
        id
    } else {
        let layer = rectangle(
            tr("Border").into(),
            canvas,
            ShapeKind::Rectangle,
            layout.color,
            0.0,
        );
        let id = layer.id;
        document.layers.insert(0, layer);
        id
    };

    // Keep the cells that are still whole, in order; make new ones on top.
    document.collage = Some(Box::new(record.clone()));
    let mut cells: Vec<(Uuid, Uuid)> = record
        .cells
        .iter()
        .filter_map(|folder| Some((*folder, frame(document, *folder)?)))
        .collect();
    let removed: Vec<Uuid> = cells.iter().skip(rects.len()).map(|c| c.0).collect();
    cells.truncate(rects.len());
    for (index, rect) in rects.iter().enumerate() {
        if let Some(&(folder, shape)) = cells.get(index) {
            let old = document
                .layers
                .iter()
                .find(|l| l.id == shape)
                .map(|l| l.transform)
                .unwrap();
            let new = rect.transform();
            let inside = document.descendants(folder);
            for layer in &mut document.layers {
                if layer.id == shape {
                    restyle(layer, *rect, kind, EMPTY_CELL, layout.corner_radius);
                } else if layer.id != folder
                    && inside.contains(&layer.id)
                    && let Some(moved) = follow(layer.transform, old, new)
                {
                    layer.set_transform(moved);
                }
            }
        } else {
            let mut folder = Layer::blank(
                format!("{} {}", tr("Cell"), index + 1),
                document.width,
                document.height,
            );
            folder.group = true;
            let mut shape = rectangle(
                tr("Frame").into(),
                *rect,
                kind,
                EMPTY_CELL,
                layout.corner_radius,
            );
            shape.parent = Some(folder.id);
            cells.push((folder.id, shape.id));
            document.layers.push(shape);
            document.layers.push(folder);
        }
    }

    // Cells the layout no longer has go, with their photos.
    let mut gone = HashSet::new();
    for folder in removed {
        gone.extend(document.descendants(folder));
    }
    if !gone.is_empty() {
        document.layers.retain(|l| !gone.contains(&l.id));
        for layer in &mut document.layers {
            if layer.clip_to.is_some_and(|id| gone.contains(&id)) {
                layer.clip_to = None;
            }
        }
        document.selected.retain(|id| !gone.contains(id));
        if document.active.is_some_and(|id| gone.contains(&id)) {
            document.active = None;
        }
        if document.active.is_none()
            && let Some(&(first, _)) = cells.first()
        {
            document.select(first, false);
        }
    }
    document.collage = Some(Box::new(Collage {
        layout,
        background: Some(background),
        cells: cells.iter().map(|c| c.0).collect(),
    }));
    paint::refresh_shapes(document)
}

/// The cells' folders that are still whole, first cell first.
pub fn cells(document: &Document) -> Vec<Uuid> {
    document.collage.as_ref().map_or_else(Vec::new, |c| {
        c.cells
            .iter()
            .copied()
            .filter(|folder| frame(document, *folder).is_some())
            .collect()
    })
}

/// The frame of the cell whose folder is `folder`: the folder's lowest layer, a rectangle
/// shape. `None` when `folder` is not a whole cell of the document's collage.
pub fn frame(document: &Document, folder: Uuid) -> Option<Uuid> {
    let collage = document.collage.as_ref()?;
    if !collage.cells.contains(&folder)
        || !document.layers.iter().any(|l| l.id == folder && l.group)
    {
        return None;
    }
    let lowest = document.layers.iter().find(|l| l.parent == Some(folder))?;
    lowest
        .shape
        .as_ref()
        .is_some_and(|s| s.path.is_none())
        .then_some(lowest.id)
}

/// The cell holding the active layer (or whose folder is active).
pub fn active_cell(document: &Document) -> Option<Uuid> {
    let mut id = document.active;
    // Bounded like the document's nesting.
    for _ in 0..66 {
        let current = id?;
        if frame(document, current).is_some() {
            return Some(current);
        }
        id = document
            .layers
            .iter()
            .find(|l| l.id == current)
            .and_then(|l| l.parent);
    }
    None
}

/// Whether a layer is a photo in a cell with frame `frame`: a pixel layer clipped to it.
fn is_photo(layer: &Layer, frame: Uuid) -> bool {
    layer.clip_to == Some(frame)
        && !layer.group
        && !layer.is_effect()
        && layer.shape.is_none()
        && layer.text.is_none()
}

/// Whether the cell `folder` has no photo yet.
pub fn is_empty(document: &Document, folder: Uuid) -> bool {
    frame(document, folder).is_none_or(|frame| {
        !document
            .layers
            .iter()
            .any(|l| l.parent == Some(folder) && is_photo(l, frame))
    })
}

/// The first empty cell after the cell `after`, in cell order.
pub fn next_empty_cell(document: &Document, after: Uuid) -> Option<Uuid> {
    cells(document)
        .into_iter()
        .skip_while(|id| *id != after)
        .skip(1)
        .find(|id| is_empty(document, *id))
}

/// Puts `photo` in the cell `folder`, in place of the photos it had: scaled to cover the cell's
/// frame, centred on it and clipped to it. The photo is selected.
pub fn place(document: &mut Document, folder: Uuid, mut photo: Layer) -> Result<Uuid> {
    let shape = frame(document, folder).ok_or_else(|| anyhow::anyhow!("Not a collage cell"))?;
    let cell = document
        .layers
        .iter()
        .find(|l| l.id == shape)
        .map(|l| l.transform)
        .unwrap();
    let (width, height) = photo
        .pixels
        .as_ref()
        .map(|p| p.dimensions())
        .ok_or_else(|| anyhow::anyhow!("A photo needs pixels"))?;
    ensure!(width > 0 && height > 0, "A photo needs pixels");
    let (width, height) = (width as f32, height as f32);
    let mut scale = (cell.width / width).max(cell.height / height);
    scale = scale.min(MAX_EXTENT / width).min(MAX_EXTENT / height);
    let center = cell.center();
    let transform = Transform {
        x: center.x - width * scale * 0.5,
        y: center.y - height * scale * 0.5,
        width: (width * scale).max(1.0),
        height: (height * scale).max(1.0),
        rotation: cell.rotation,
        ..Transform::new(1, 1)
    };
    ensure!(transform.valid(), "The photo cannot be fitted to its cell");

    let mut old = HashSet::new();
    for layer in &document.layers {
        if layer.parent == Some(folder) && is_photo(layer, shape) {
            old.extend(document.descendants(layer.id));
        }
    }
    document.layers.retain(|l| !old.contains(&l.id));
    for layer in &mut document.layers {
        if layer.clip_to.is_some_and(|id| old.contains(&id)) {
            layer.clip_to = None;
        }
    }

    photo.set_transform(transform);
    photo.parent = Some(folder);
    photo.clip_to = Some(shape);
    let id = photo.id;
    // At the top of the cell, just below its folder.
    let index = document.layers.iter().position(|l| l.id == folder).unwrap();
    document.layers.insert(index, photo);
    document.select(id, false);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Point;
    use image::{Rgba, RgbaImage};

    fn rect(x: u32, y: u32, width: u32, height: u32) -> CellRect {
        CellRect {
            x,
            y,
            width,
            height,
        }
    }

    fn photo(name: &str, width: u32, height: u32, color: [u8; 4]) -> Layer {
        Layer::image(name, RgbaImage::from_pixel(width, height, Rgba(color)))
    }

    fn pixel(document: &Document, x: u32, y: u32) -> [u8; 4] {
        crate::render::render(document).get_pixel(x, y).0
    }

    #[test]
    fn a_two_by_two_grid_shares_the_room_the_borders_leave() {
        let cells = Layout::default().cells(1000, 800).unwrap();
        // (1000 - 2 × 20 - 20) / 2 = 470 wide, (800 - 2 × 20 - 20) / 2 = 370 high.
        assert_eq!(
            cells,
            [
                rect(20, 20, 470, 370),
                rect(510, 20, 470, 370),
                rect(20, 410, 470, 370),
                rect(510, 410, 470, 370),
            ]
        );
    }

    #[test]
    fn templates_span_their_large_cell_over_the_grid() {
        let layout = |template| Layout {
            template,
            spacing: 10,
            border: 0,
            ..Layout::default()
        };
        assert_eq!(
            layout(Template::LargeLeft).cells(210, 110).unwrap(),
            [
                rect(0, 0, 100, 110),
                rect(110, 0, 100, 50),
                rect(110, 60, 100, 50)
            ]
        );
        assert_eq!(
            layout(Template::LargeTop).cells(320, 210).unwrap(),
            [
                rect(0, 0, 320, 100),
                rect(0, 110, 100, 100),
                rect(110, 110, 100, 100),
                rect(220, 110, 100, 100),
            ]
        );
        // Other templates ignore the grid's own columns and rows.
        let mut big = layout(Template::LargeTop);
        big.columns = 7;
        assert_eq!(big.cell_count(), 4);
    }

    #[test]
    fn uneven_room_rounds_cell_edges_without_gaps_or_overlaps() {
        let layout = Layout {
            columns: 3,
            rows: 1,
            spacing: 0,
            border: 0,
            ..Layout::default()
        };
        // 100 / 3 = 33.3: edges at 0, 33, 67 and 100.
        let cells = layout.cells(100, 5).unwrap();
        assert_eq!(
            cells,
            [rect(0, 0, 33, 5), rect(33, 0, 34, 5), rect(67, 0, 33, 5)]
        );
        // Every grid size fits the canvas with gaps of the spacing, give or take a pixel.
        for columns in 1..=MAX_TRACKS {
            for rows in 1..=MAX_TRACKS {
                let layout = Layout {
                    columns,
                    rows,
                    spacing: 7,
                    border: 3,
                    ..Layout::default()
                };
                let cells = layout.cells(997, 613).unwrap();
                assert_eq!(cells.len(), (columns * rows) as usize);
                for pair in cells.windows(2).filter(|p| p[0].y == p[1].y) {
                    let gap = pair[1].x - (pair[0].x + pair[0].width);
                    assert!((6..=8).contains(&gap), "{columns}×{rows}: {gap}");
                }
                let last = cells.last().unwrap();
                assert_eq!(last.x + last.width, 994);
                assert_eq!(last.y + last.height, 610);
            }
        }
    }

    #[test]
    fn layouts_without_room_or_out_of_range_are_refused() {
        let layout = Layout {
            border: 50,
            ..Layout::default()
        };
        assert!(layout.cells(120, 1000).is_err());
        assert!(layout.cells(122, 1000).is_ok(), "one pixel per track");
        for bad in [
            Layout {
                columns: 0,
                ..Layout::default()
            },
            Layout {
                rows: MAX_TRACKS + 1,
                ..Layout::default()
            },
            Layout {
                spacing: MAX_SIDE + 1,
                ..Layout::default()
            },
            Layout {
                corner_radius: f32::NAN,
                ..Layout::default()
            },
            Layout {
                corner_radius: -1.0,
                ..Layout::default()
            },
        ] {
            assert!(bad.cells(1000, 1000).is_err(), "{bad:?}");
        }
        // The largest borders never overflow.
        let huge = Layout {
            spacing: MAX_SIDE,
            border: MAX_SIDE,
            columns: MAX_TRACKS,
            ..Layout::default()
        };
        assert!(huge.cells(MAX_SIDE, 10).is_err());
    }

    /// The issue's acceptance check: a 2 × 2 collage with 20 px white borders.
    #[test]
    fn a_two_by_two_collage_with_white_borders_renders_as_expected() {
        let mut document = new_document(200, 160, Layout::default()).unwrap();
        document.validate().unwrap();
        let cells = cells(&document);
        assert_eq!(cells.len(), 4);
        assert_eq!(document.active, Some(cells[0]));
        // Cells of 70 × 50 at x 20 and 110, y 20 and 90.
        let colors = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
        ];
        for (cell, color) in cells.iter().zip(colors) {
            place(&mut document, *cell, photo("Photo", 30, 20, color)).unwrap();
        }
        document.validate().unwrap();
        let image = crate::render::render(&document);
        let white = [255, 255, 255, 255];
        // Outer border, the spacing between cells and the centre where they meet.
        for (x, y) in [(5, 5), (19, 50), (100, 50), (55, 80), (100, 80), (195, 155)] {
            assert_eq!(image.get_pixel(x, y).0, white, "({x}, {y})");
        }
        for ((x, y), color) in [(20, 20), (179, 20), (20, 139), (179, 139)]
            .into_iter()
            .zip(colors)
        {
            assert_eq!(image.get_pixel(x, y).0, color, "({x}, {y})");
        }
    }

    #[test]
    fn empty_cells_show_their_frame_and_round_corners_show_the_border() {
        let layout = Layout {
            corner_radius: 20.0,
            color: [10, 20, 30, 255],
            ..Layout::default()
        };
        let document = new_document(200, 160, layout).unwrap();
        assert_eq!(pixel(&document, 55, 45), EMPTY_CELL);
        // The cell's very corner is outside its rounded frame.
        assert_eq!(pixel(&document, 20, 20), [10, 20, 30, 255]);
        assert_eq!(pixel(&document, 30, 30), EMPTY_CELL);
    }

    #[test]
    fn a_photo_covers_its_cell_and_can_be_moved_and_scaled_within_it() {
        let mut document = new_document(200, 160, Layout::default()).unwrap();
        let cell = cells(&document)[0];
        // A 4:1 photo in a 70 × 50 cell is scaled to its height: 200 × 50, centred on x = 55.
        let id = place(&mut document, cell, photo("Wide", 40, 10, [255, 0, 0, 255])).unwrap();
        assert_eq!(document.active, Some(id));
        let index = |document: &Document, id| document.layers.iter().position(|l| l.id == id);
        let layer = &document.layers[index(&document, id).unwrap()];
        assert_eq!((layer.transform.x, layer.transform.y), (-45.0, 20.0));
        assert_eq!(
            (layer.transform.width, layer.transform.height),
            (200.0, 50.0)
        );
        assert_eq!(layer.parent, Some(cell));
        assert_eq!(layer.clip_to, frame(&document, cell));
        assert!(
            index(&document, id) < index(&document, cell),
            "inside its folder"
        );
        // It does not spill over the border or the neighbouring cell.
        assert_eq!(pixel(&document, 10, 40), [255, 255, 255, 255]);
        assert_eq!(pixel(&document, 120, 40), EMPTY_CELL);

        // Moved right by 40 and scaled down, it stays clipped to the cell and uncovers it.
        let i = index(&document, id).unwrap();
        let mut moved = document.layers[i].transform;
        moved.x = 60.0;
        moved.width = 40.0;
        moved.height = 10.0;
        document.layers[i].set_transform(moved);
        assert_eq!(pixel(&document, 30, 40), EMPTY_CELL);
        assert_eq!(pixel(&document, 65, 25), [255, 0, 0, 255]);
        assert_eq!(pixel(&document, 100, 25), [255, 255, 255, 255]);
        assert!(!is_empty(&document, cell));
    }

    #[test]
    fn placing_again_replaces_the_cells_photo_and_its_attachments() {
        let mut document = new_document(200, 160, Layout::default()).unwrap();
        let cell = cells(&document)[1];
        let first = place(&mut document, cell, photo("A", 4, 4, [255, 0, 0, 255])).unwrap();
        let mut invert = Layer::blank("Invert", 4, 4);
        invert.adjustment = Some(crate::document::Adjustment::Invert);
        invert.parent = Some(first);
        let index = document.layers.iter().position(|l| l.id == first).unwrap();
        document.layers.insert(index + 1, invert);
        document.validate().unwrap();
        let count = document.layers.len();
        let second = place(&mut document, cell, photo("B", 4, 4, [0, 0, 255, 255])).unwrap();
        assert_eq!(document.layers.len(), count - 1);
        assert!(!document.layers.iter().any(|l| l.id == first));
        document.validate().unwrap();
        assert_eq!(document.active, Some(second));
        assert_eq!(pixel(&document, 150, 50), [0, 0, 255, 255]);
    }

    #[test]
    fn cells_are_found_from_their_layers_and_filled_in_order() {
        let mut document = new_document(200, 160, Layout::default()).unwrap();
        let all = cells(&document);
        assert_eq!(active_cell(&document), Some(all[0]));
        let id = place(&mut document, all[0], photo("A", 4, 4, [9; 4])).unwrap();
        assert_eq!(active_cell(&document), Some(all[0]), "from the photo");
        assert_eq!(next_empty_cell(&document, all[0]), Some(all[1]));
        place(&mut document, all[2], photo("C", 4, 4, [9; 4])).unwrap();
        assert_eq!(next_empty_cell(&document, all[1]), Some(all[3]));
        assert_eq!(next_empty_cell(&document, all[3]), None);
        // The border layer is no cell, and neither is an ordinary folder.
        let background = document.collage.as_ref().unwrap().background.unwrap();
        document.select(background, false);
        assert_eq!(active_cell(&document), None);
        let mut plain = Document::new(10, 10).unwrap();
        let mut folder = Layer::blank("Folder", 10, 10);
        folder.group = true;
        let folder_id = folder.id;
        plain.insert(folder);
        assert_eq!(active_cell(&plain), None);
        assert!(place(&mut plain, folder_id, photo("A", 4, 4, [9; 4])).is_err());
        assert!(document.layers.iter().any(|l| l.id == id));
    }

    #[test]
    fn relayout_moves_cells_and_their_photos_and_restyles_the_border() {
        let mut document = new_document(200, 160, Layout::default()).unwrap();
        let all = cells(&document);
        let id = place(&mut document, all[3], photo("D", 70, 50, [255, 0, 0, 255])).unwrap();
        let layout = Layout {
            spacing: 0,
            border: 0,
            color: [0, 0, 0, 255],
            ..Layout::default()
        };
        relayout(&mut document, layout).unwrap();
        document.validate().unwrap();
        assert_eq!(cells(&document), all);
        // Cell 4 is now 100 × 80 at (100, 80): the photo grows by 1.6 and still covers it.
        let photo = document.layers.iter().find(|l| l.id == id).unwrap();
        assert_eq!(photo.transform.center(), Point::new(150.0, 120.0));
        assert_eq!(
            (photo.transform.width, photo.transform.height),
            (112.0, 80.0)
        );
        assert_eq!(pixel(&document, 100, 80), [255, 0, 0, 255]);
        assert_eq!(pixel(&document, 0, 0), EMPTY_CELL, "no border left");
        assert_eq!(document.collage.as_ref().unwrap().layout, layout);

        // A different border colour shows in the gaps.
        relayout(
            &mut document,
            Layout {
                color: [0, 0, 255, 255],
                ..Layout::default()
            },
        )
        .unwrap();
        assert_eq!(pixel(&document, 100, 50), [0, 0, 255, 255]);
    }

    #[test]
    fn relayout_adds_and_removes_cells_and_remakes_deleted_layers() {
        let mut document = new_document(300, 200, Layout::default()).unwrap();
        let all = cells(&document);
        let photo_id = place(&mut document, all[3], photo("D", 8, 8, [1; 4])).unwrap();
        let three = Layout {
            template: Template::LargeLeft,
            ..Layout::default()
        };
        relayout(&mut document, three).unwrap();
        document.validate().unwrap();
        assert_eq!(cells(&document), all[..3]);
        assert!(
            !document
                .layers
                .iter()
                .any(|l| l.id == photo_id || l.id == all[3])
        );

        // Grow to six, after deleting the border and a cell.
        let background = document.collage.as_ref().unwrap().background.unwrap();
        document.layers.retain(|l| l.id != background);
        document.select(all[1], false);
        document.delete_selected();
        let six = Layout {
            columns: 3,
            ..Layout::default()
        };
        relayout(&mut document, six).unwrap();
        document.validate().unwrap();
        let now = cells(&document);
        assert_eq!(now.len(), 6);
        assert_eq!(now[..2], [all[0], all[2]]);
        let border = document.collage.as_ref().unwrap().background.unwrap();
        assert_ne!(border, background);
        assert_eq!(document.layers[0].id, border, "at the bottom");
        assert_eq!(pixel(&document, 2, 2), [255, 255, 255, 255]);
    }

    #[test]
    fn records_from_files_are_validated() {
        let layout = Layout::default();
        let id = Uuid::new_v4();
        let ok = Collage {
            layout,
            background: Some(Uuid::new_v4()),
            cells: vec![id],
        };
        ok.validate().unwrap();
        let mut duplicate = ok.clone();
        duplicate.cells.push(id);
        assert!(duplicate.validate().is_err());
        let mut border = ok.clone();
        border.background = Some(id);
        assert!(border.validate().is_err());
        let mut many = ok.clone();
        many.cells = (0..=MAX_CELLS).map(|_| Uuid::new_v4()).collect();
        assert!(many.validate().is_err());
        let mut bad = ok;
        bad.layout.columns = 0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn huge_photos_are_fitted_within_transform_limits() {
        let mut document = new_document(200, 160, Layout::default()).unwrap();
        let cell = cells(&document)[0];
        // A 1 × 10,000 sliver would need a 700,000-pixel-high layer to cover the cell's width;
        // it is kept within the limits instead.
        let id = place(&mut document, cell, photo("Sliver", 1, 10_000, [1; 4])).unwrap();
        let layer = document.layers.iter().find(|l| l.id == id).unwrap();
        assert!(layer.transform.valid());
        document.validate().unwrap();
    }
}

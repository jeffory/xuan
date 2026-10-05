//! View → Snap To: one snapping engine for moves, resize handles, marquees, shapes, crops,
//! selection moves and guides.
//!
//! Follows Compositor's `TransformSnap` (`Document/LayerTransform.swift`) and
//! `alignmentSnapTargets` (`Document/Guides.swift`): the pull is a fixed distance on screen, so it
//! feels the same at any zoom, and each axis snaps on its own to the nearest target in reach.
//! Where two targets are equally near, guides win over the document bounds, then layers, then the
//! grid, so the line drawn names the most deliberate target.
use std::collections::HashSet;

use uuid::Uuid;
use xuan::{
    document::{Document, Point, Transform},
    layout::{GridSettings, Guide, GuideAxis, SnapSettings},
};

/// How close, in screen points, a target comes before it snaps (upstream `TransformSnap.distance`).
pub(super) const SNAP_DISTANCE: f32 = 10.0;

/// The snapping reach in document pixels at `zoom`.
pub(super) fn tolerance(zoom: f32) -> f32 {
    SNAP_DISTANCE / zoom.max(0.0001)
}

/// What a target is; also the order in which equally near targets win.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SnapKind {
    Guide,
    Bounds,
    Layer,
    Grid,
}

/// A line that was snapped to, for the feedback drawn across the canvas. A `Vertical` line sits
/// at an X.
pub(super) type SnapLine = (GuideAxis, f32);

/// The View settings that decide what snaps.
#[derive(Clone, Copy, Debug)]
pub(super) struct SnapOptions {
    pub settings: SnapSettings,
    pub show_guides: bool,
    pub show_grid: bool,
    pub grid: GridSettings,
}

/// Everything a drag may snap to, along each axis.
#[derive(Debug, Default)]
pub(super) struct SnapTargets {
    xs: Vec<(f32, SnapKind)>,
    ys: Vec<(f32, SnapKind)>,
    /// The grid is searched rather than listed: (settings, document size).
    grid: Option<(GridSettings, [f32; 2])>,
}

impl SnapTargets {
    /// Targets in `document` for a drag of the layers in `moving`. `centers` adds the middles of
    /// the canvas and layers, as moves and resizes use; marquees, crops and selections don't.
    /// `guides` are the guides as shown, without one being dragged.
    pub(super) fn new(
        document: &Document,
        guides: &[Guide],
        options: &SnapOptions,
        moving: &HashSet<Uuid>,
        centers: bool,
    ) -> Self {
        let mut targets = Self::default();
        let settings = options.settings;
        if !settings.enabled {
            return targets;
        }
        let (width, height) = (document.width as f32, document.height as f32);
        if settings.bounds {
            for (values, length) in [(&mut targets.xs, width), (&mut targets.ys, height)] {
                values.extend([(0.0, SnapKind::Bounds), (length, SnapKind::Bounds)]);
                if centers {
                    values.push((length * 0.5, SnapKind::Bounds));
                }
            }
        }
        if settings.layers {
            for layer in &document.layers {
                if !layer.visible
                    || layer.pixels.is_none()
                    || layer.standalone_mask
                    || moving.contains(&layer.id)
                {
                    continue;
                }
                let [min, max] = bounds(layer.transform);
                for (values, low, high) in [
                    (&mut targets.xs, min.x, max.x),
                    (&mut targets.ys, min.y, max.y),
                ] {
                    values.extend([
                        (low.round(), SnapKind::Layer),
                        (high.round(), SnapKind::Layer),
                    ]);
                    if centers {
                        values.push((((low + high) * 0.5).round(), SnapKind::Layer));
                    }
                }
            }
        }
        // Hidden extras do not snap, matching Photoshop.
        if settings.grid && options.show_grid {
            targets.grid = Some((options.grid, [width, height]));
        }
        if settings.guides && options.show_guides {
            for guide in guides {
                let values = match guide.axis {
                    GuideAxis::Vertical => &mut targets.xs,
                    GuideAxis::Horizontal => &mut targets.ys,
                };
                values.push((guide.position, SnapKind::Guide));
            }
        }
        targets
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.xs.is_empty() && self.ys.is_empty() && self.grid.is_none()
    }

    /// The nearest target to `value` on the line across `axis` (a `Vertical` axis searches X),
    /// within `tolerance` document pixels.
    pub(super) fn nearest(
        &self,
        axis: GuideAxis,
        value: f32,
        tolerance: f32,
    ) -> Option<(f32, SnapKind)> {
        let (list, grid_length) = match axis {
            GuideAxis::Vertical => (&self.xs, self.grid.map(|(_, size)| size[0])),
            GuideAxis::Horizontal => (&self.ys, self.grid.map(|(_, size)| size[1])),
        };
        let grid = self
            .grid
            .zip(grid_length)
            .and_then(|((grid, _), length)| grid.nearest_line(value, length))
            .map(|line| (line, SnapKind::Grid));
        list.iter()
            .copied()
            .chain(grid)
            .filter(|(target, _)| (target - value).abs() <= tolerance)
            .min_by(|a, b| {
                (a.0 - value)
                    .abs()
                    .total_cmp(&(b.0 - value).abs())
                    .then(a.1.cmp(&b.1))
            })
    }

    /// The smallest move along `axis` that puts one of `edges` on a target, with the target met.
    pub(super) fn shift(
        &self,
        axis: GuideAxis,
        edges: &[f32],
        tolerance: f32,
    ) -> Option<(f32, f32)> {
        edges
            .iter()
            .filter_map(|&edge| {
                self.nearest(axis, edge, tolerance)
                    .map(|(target, kind)| (target - edge, target, kind))
            })
            .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()).then(a.2.cmp(&b.2)))
            .map(|(delta, target, _)| (delta, target))
    }

    /// `point` moved onto the nearest target on each axis: where a marquee, shape or crop starts,
    /// and where its corner is dragged to.
    pub(super) fn snap_point(&self, point: Point, tolerance: f32) -> (Point, Vec<SnapLine>) {
        let mut lines = Vec::new();
        let mut result = point;
        if let Some((x, _)) = self.nearest(GuideAxis::Vertical, point.x, tolerance) {
            result.x = x;
            lines.push((GuideAxis::Vertical, x));
        }
        if let Some((y, _)) = self.nearest(GuideAxis::Horizontal, point.y, tolerance) {
            result.y = y;
            lines.push((GuideAxis::Horizontal, y));
        }
        (result, lines)
    }

    /// The offset that lands a box (`min`..`max`) moved by `offset` on targets with its left,
    /// middle or right, and top, middle or bottom. An axis the drag has locked doesn't snap.
    pub(super) fn snap_box(
        &self,
        [min, max]: [Point; 2],
        offset: Point,
        lock: [bool; 2],
        tolerance: f32,
    ) -> (Point, Vec<SnapLine>) {
        let mut lines = Vec::new();
        let mut result = offset;
        let edges = |low: f32, high: f32, by: f32| [low + by, (low + high) * 0.5 + by, high + by];
        if !lock[0]
            && let Some((delta, x)) = self.shift(
                GuideAxis::Vertical,
                &edges(min.x, max.x, offset.x),
                tolerance,
            )
        {
            result.x += delta;
            lines.push((GuideAxis::Vertical, x));
        }
        if !lock[1]
            && let Some((delta, y)) = self.shift(
                GuideAxis::Horizontal,
                &edges(min.y, max.y, offset.y),
                tolerance,
            )
        {
            result.y += delta;
            lines.push((GuideAxis::Horizontal, y));
        }
        (result, lines)
    }

    /// A resize handle dragged to `point`: the pointer nudged so the edges the handle moves land on
    /// a nearby target, as upstream's `snappedResizePoint`. `handle` is the handle's unit position,
    /// `grab` where it sat when the drag began, `start` the press. Each edge snaps on its own;
    /// `proportional`, only the nearer one does and the other follows the ratio. `update` is the
    /// drag's own result for a pointer position.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn snap_resize(
        &self,
        point: Point,
        start: Point,
        handle: Point,
        grab: Point,
        proportional: bool,
        tolerance: f32,
        update: impl Fn(Point) -> Transform,
    ) -> (Point, Vec<SnapLine>) {
        // Where the dragged handle is, to tell its edge from the one across from it.
        let at = Point::new(grab.x + point.x - start.x, grab.y + point.y - start.y);
        let edge = |t: Transform, axis: GuideAxis| {
            let [min, max] = bounds(t);
            let (low, high, at) = match axis {
                GuideAxis::Vertical => (min.x, max.x, at.x),
                GuideAxis::Horizontal => (min.y, max.y, at.y),
            };
            if (low - at).abs() <= (high - at).abs() {
                low
            } else {
                high
            }
        };
        let draft = update(point);
        let mut snaps: Vec<(GuideAxis, f32, f32)> = Vec::new();
        for (axis, moves) in [
            (GuideAxis::Vertical, handle.x != 0.5),
            (GuideAxis::Horizontal, handle.y != 0.5),
        ] {
            if moves {
                let value = edge(draft, axis);
                if let Some((target, _)) = self.nearest(axis, value, tolerance) {
                    snaps.push((axis, target, (target - value).abs()));
                }
            }
        }
        if proportional && snaps.len() == 2 {
            let nearer = if snaps[0].2 <= snaps[1].2 { 0 } else { 1 };
            snaps = vec![snaps[nearer]];
        }
        // An edge follows the pointer in a straight line along each axis, so one step measured
        // across a pixel lands it.
        let mut result = point;
        let mut lines = Vec::new();
        for (axis, target, _) in snaps {
            let before = edge(update(result), axis);
            let mut nudged = result;
            match axis {
                GuideAxis::Vertical => nudged.x += 1.0,
                GuideAxis::Horizontal => nudged.y += 1.0,
            }
            let per_pixel = edge(update(nudged), axis) - before;
            if per_pixel.abs() <= 0.01 {
                continue;
            }
            let shift = (target - before) / per_pixel;
            match axis {
                GuideAxis::Vertical => result.x += shift,
                GuideAxis::Horizontal => result.y += shift,
            }
            lines.push((axis, target));
        }
        (result, lines)
    }
}

/// The axis-aligned box around a transform's corners: [min, max].
pub(super) fn bounds(transform: Transform) -> [Point; 2] {
    let corners = transform.corners();
    let mut min = corners[0];
    let mut max = corners[0];
    for corner in &corners[1..] {
        min = Point::new(min.x.min(corner.x), min.y.min(corner.y));
        max = Point::new(max.x.max(corner.x), max.y.max(corner.y));
    }
    [min, max]
}

#[cfg(test)]
mod tests {
    use super::*;
    use xuan::document::Layer;

    fn options() -> SnapOptions {
        SnapOptions {
            settings: SnapSettings {
                enabled: true,
                guides: true,
                grid: true,
                layers: true,
                bounds: true,
            },
            show_guides: true,
            show_grid: true,
            grid: GridSettings {
                spacing: 50,
                subdivisions: 1,
                ..GridSettings::default()
            },
        }
    }

    fn document() -> Document {
        let mut document = Document::new(200, 100).unwrap();
        document.layers[0].pixels = Some(std::sync::Arc::new(image::RgbaImage::new(200, 100)));
        let mut layer = Layer::image("Box", image::RgbaImage::new(30, 20));
        layer.transform.x = 120.0;
        layer.transform.y = 33.0;
        document.insert(layer);
        document
    }

    fn targets(options: &SnapOptions, guides: &[Guide]) -> SnapTargets {
        SnapTargets::new(&document(), guides, options, &HashSet::new(), true)
    }

    #[test]
    fn the_nearest_target_within_reach_wins() {
        let guides = [Guide::new(GuideAxis::Vertical, 73.0)];
        let targets = targets(&options(), &guides);
        // 73 (guide) is nearer 70 than the 50 and 100 grid lines.
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 70.0, 5.0),
            Some((73.0, SnapKind::Guide))
        );
        // The layer's left edge at 120 against the grid at 100 and 150.
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 118.0, 5.0),
            Some((120.0, SnapKind::Layer))
        );
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 147.0, 5.0),
            Some((150.0, SnapKind::Layer))
        );
        // Nothing in reach.
        assert_eq!(targets.nearest(GuideAxis::Horizontal, 20.0, 2.0), None);
        // The horizontal axis has the layer's top and the canvas middle.
        assert_eq!(
            targets.nearest(GuideAxis::Horizontal, 31.0, 5.0),
            Some((33.0, SnapKind::Layer))
        );
    }

    #[test]
    fn equally_near_targets_prefer_guides_then_bounds_then_layers_then_grid() {
        // A guide, the canvas edge and a grid line all at x = 0, and at 200.
        let guides = [
            Guide::new(GuideAxis::Vertical, 0.0),
            Guide::new(GuideAxis::Vertical, 100.0),
        ];
        let targets = targets(&options(), &guides);
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 2.0, 5.0),
            Some((0.0, SnapKind::Guide))
        );
        // The canvas centre (bounds) and a grid line meet at 100; the guide there wins.
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 98.0, 5.0),
            Some((100.0, SnapKind::Guide))
        );
        let targets = SnapTargets::new(&document(), &[], &options(), &HashSet::new(), true);
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 198.0, 5.0),
            Some((200.0, SnapKind::Bounds))
        );
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 48.0, 5.0),
            Some((50.0, SnapKind::Grid))
        );
    }

    #[test]
    fn toggles_and_hidden_extras_remove_targets() {
        let guides = [Guide::new(GuideAxis::Vertical, 73.0)];
        let mut off = options();
        off.settings.enabled = false;
        assert!(targets(&off, &guides).is_empty());

        let mut hidden = options();
        hidden.show_guides = false;
        hidden.show_grid = false;
        let targets = targets(&hidden, &guides);
        assert_eq!(targets.nearest(GuideAxis::Vertical, 72.0, 5.0), None);
        assert_eq!(targets.nearest(GuideAxis::Vertical, 52.0, 5.0), None);

        let mut no_bounds = options();
        no_bounds.settings.bounds = false;
        no_bounds.settings.grid = false;
        no_bounds.settings.layers = false;
        let targets = super::tests::targets(&no_bounds, &guides);
        assert_eq!(targets.nearest(GuideAxis::Vertical, 1.0, 5.0), None);
        assert_eq!(
            targets.nearest(GuideAxis::Vertical, 75.0, 5.0),
            Some((73.0, SnapKind::Guide))
        );
    }

    #[test]
    fn the_reach_is_fixed_on_screen_whatever_the_zoom() {
        let mut settings = options();
        settings.settings.grid = false;
        settings.settings.layers = false;
        settings.settings.bounds = false;
        let guides = [Guide::new(GuideAxis::Vertical, 60.0)];
        let targets = targets(&settings, &guides);
        for zoom in [0.25_f32, 1.0, 4.0, 16.0] {
            let reach = tolerance(zoom);
            assert!((reach * zoom - SNAP_DISTANCE).abs() < 1e-4);
            // 9 screen points away snaps, 11 doesn't, at every zoom.
            let near = 60.0 + 9.0 / zoom;
            let far = 60.0 + 11.0 / zoom;
            assert_eq!(
                targets.nearest(GuideAxis::Vertical, near, reach),
                Some((60.0, SnapKind::Guide)),
                "zoom {zoom}"
            );
            assert_eq!(
                targets.nearest(GuideAxis::Vertical, far, reach),
                None,
                "zoom {zoom}"
            );
        }
    }

    #[test]
    fn moving_layers_are_not_their_own_targets() {
        let document = document();
        let moving: HashSet<_> = document.selected.clone();
        let mut settings = options();
        settings.settings.grid = false;
        settings.settings.bounds = false;
        let targets = SnapTargets::new(&document, &[], &settings, &moving, true);
        assert_eq!(targets.nearest(GuideAxis::Vertical, 121.0, 5.0), None);
        // Without centres, a layer's middle is not a target.
        let targets = SnapTargets::new(&document, &[], &settings, &HashSet::new(), false);
        assert_eq!(targets.nearest(GuideAxis::Vertical, 135.0, 2.0), None);
    }

    #[test]
    fn boxes_snap_by_edge_or_middle_and_points_per_axis() {
        let mut settings = options();
        settings.settings.grid = false;
        settings.settings.layers = false;
        let guides = [
            Guide::new(GuideAxis::Vertical, 40.0),
            Guide::new(GuideAxis::Horizontal, 70.0),
        ];
        let targets = targets(&settings, &guides);
        // A 20x10 box at (10, 10) moved by (13, 3): its middle x 33 is 7 from the guide at 40 but
        // its right edge 43 is 3 from it, so it moves left by 3.
        let box_ = [Point::new(10.0, 10.0), Point::new(30.0, 20.0)];
        let (offset, lines) = targets.snap_box(box_, Point::new(13.0, 3.0), [false; 2], 4.0);
        assert_eq!(offset, Point::new(10.0, 3.0));
        assert_eq!(lines, [(GuideAxis::Vertical, 40.0)]);
        // A locked axis stays put.
        let (offset, lines) = targets.snap_box(box_, Point::new(13.0, 3.0), [true, false], 4.0);
        assert_eq!(offset, Point::new(13.0, 3.0));
        assert!(lines.is_empty());

        let (point, lines) = targets.snap_point(Point::new(38.0, 68.5), 4.0);
        assert_eq!(point, Point::new(40.0, 70.0));
        assert_eq!(lines.len(), 2);
        let (point, lines) = targets.snap_point(Point::new(55.0, 60.0), 4.0);
        assert_eq!(point, Point::new(55.0, 60.0));
        assert!(lines.is_empty());
    }

    #[test]
    fn resize_handles_land_their_edge_on_a_target() {
        let mut settings = options();
        settings.settings.grid = false;
        settings.settings.layers = false;
        let guides = [Guide::new(GuideAxis::Vertical, 100.0)];
        let targets = targets(&settings, &guides);
        // A 50 px wide box at x = 20 stretched from its right edge, which starts at 70.
        let original = Transform {
            x: 20.0,
            width: 50.0,
            ..Transform::new(50, 40)
        };
        let update = |p: Point| {
            let mut t = original;
            t.width = (p.x - original.x).max(1.0);
            t
        };
        let start = Point::new(70.0, 20.0);
        let (point, lines) = targets.snap_resize(
            Point::new(97.0, 20.0),
            start,
            Point::new(1.0, 0.5),
            start,
            false,
            5.0,
            update,
        );
        assert!((point.x - 100.0).abs() < 1e-3, "{point:?}");
        assert_eq!(lines, [(GuideAxis::Vertical, 100.0)]);
        // Out of reach, the pointer is left alone.
        let (point, lines) = targets.snap_resize(
            Point::new(90.0, 20.0),
            start,
            Point::new(1.0, 0.5),
            start,
            false,
            5.0,
            update,
        );
        assert_eq!(point, Point::new(90.0, 20.0));
        assert!(lines.is_empty());
    }
}

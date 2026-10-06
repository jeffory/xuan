//! The Pen tool: draws Bézier paths into the document's paths (Select → Paths…) and edits
//! them, and the outlines of path shape layers, on the canvas.
//!
//! Drawing: a click adds a corner anchor and a drag a smooth one with symmetric handles.
//! Clicking the first anchor closes the path; Enter, Escape or another tool finishes an open
//! one. Editing works on the path shown (the one just drawn, picked with the Pen or in the
//! Paths dialog, or the active path shape layer's outline), as Photoshop's Pen does with
//! Auto Add/Delete: drag anchors and handles (Alt-drag a handle breaks the symmetry), click a
//! segment to add an anchor, click an anchor to delete it, Alt-click an anchor to make it a
//! corner or smooth. Every finished change is one undo step.
use anyhow::{Context as _, Result, ensure};
use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, Vec2};
use kurbo::{Affine, BezPath, PathEl, Shape as _};
use uuid::Uuid;
use xuan::{
    document::{Layer, Point},
    i18n::tr,
    paint::ShapeKind,
    path_edit::{Anchor, EditPath, Hit, Side, Subpath},
    vector::NamedPath,
};

use super::EditorApp;

/// How near, in screen pixels, the pointer must be to an anchor, handle or segment.
pub(super) const HIT_RADIUS: f32 = 6.0;
/// How far, in screen pixels, the pointer must move before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 3.0;
const PATH_COLOR: Color32 = Color32::from_rgb(40, 170, 255);

/// What the Pen edits: a document path or a path shape layer's outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PenTarget {
    Path(Uuid),
    Layer(Uuid),
}

/// A path being drawn, in the document `document`.
#[derive(Clone, Debug)]
pub(super) struct Draft {
    pub document: Uuid,
    pub subpath: Subpath,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragKind {
    /// Placing the draft's newest anchor; a drag pulls out its handles.
    Draft,
    /// Pressed on the draft's first anchor: closes the path on release.
    Close,
    /// Pressed on an anchor: a drag moves it (Alt pulls new handles out), a click deletes it
    /// (Alt-click converts it).
    Anchor { subpath: usize, anchor: usize },
    Handle {
        subpath: usize,
        anchor: usize,
        side: Side,
    },
    /// Added on a segment; a drag moves it.
    Added { subpath: usize, anchor: usize },
}

#[derive(Clone, Debug)]
struct PenDrag {
    kind: DragKind,
    target: Option<PenTarget>,
    /// Where the press was, in document and screen pixels.
    press: kurbo::Point,
    screen: Pos2,
    /// Where the pressed anchor was.
    origin: kurbo::Point,
    moved: bool,
    /// The edited path while the drag lasts, in document coordinates.
    working: Option<EditPath>,
}

/// The Pen tool's state.
#[derive(Clone, Debug, Default)]
pub(super) struct PenState {
    pub draft: Option<Draft>,
    /// The path shown for editing, chosen with the Pen or in the Paths dialog, in the
    /// document with the first id.
    pub target: Option<(Uuid, PenTarget)>,
    /// The anchor Delete removes: the last one added, moved or converted.
    pub selected: Option<(usize, usize)>,
    drag: Option<PenDrag>,
}

fn kp(p: Point) -> kurbo::Point {
    kurbo::Point::new(f64::from(p.x), f64::from(p.y))
}

/// The outline of a path shape layer in document coordinates, if it has one and is not warped.
pub(super) fn layer_outline(layer: &Layer) -> Option<EditPath> {
    let shape = layer.shape.as_ref()?;
    if shape.kind != ShapeKind::Path {
        return None;
    }
    let path = shape.path.as_ref()?.in_document(layer.transform)?;
    Some(EditPath::from_vector(&path))
}

/// The affine map from a layer's unit square to the document, for an unwarped layer.
fn unit_to_document(layer: &Layer) -> Affine {
    let t = layer.transform;
    let o = kp(t.point(Point::new(0.0, 0.0)));
    let x = kp(t.point(Point::new(1.0, 0.0)));
    let y = kp(t.point(Point::new(0.0, 1.0)));
    Affine::new([x.x - o.x, x.y - o.y, y.x - o.x, y.y - o.y, o.x, o.y])
}

/// Give a path shape layer the outline `path` (document coordinates), keeping the outline in
/// box-local coordinates: the box grows or shrinks, in whole box units, to the curves' bounds,
/// and the layer's placement follows so that nothing else moves.
pub(super) fn set_layer_outline(layer: &mut Layer, path: &EditPath) -> Result<()> {
    ensure!(
        layer.transform.warp.is_none(),
        "A warped shape's outline cannot be edited"
    );
    let unit = unit_to_document(layer);
    ensure!(unit.determinant().abs() > 1e-12, "The layer has no area");
    let shape = layer
        .shape
        .as_ref()
        .and_then(|s| s.path.as_ref())
        .context("The layer is not a path shape")?;
    let (w, h) = (f64::from(shape.width), f64::from(shape.height));
    let to_box = Affine::scale_non_uniform(w, h) * unit.inverse();
    let local = to_box * path.to_vector()?.bez();
    ensure!(
        local.elements().iter().any(|e| !matches!(e, PathEl::MoveTo(_))),
        "A path shape needs at least one segment"
    );
    let bounds = local.bounding_box();
    let x0 = bounds.x0.floor();
    let y0 = bounds.y0.floor();
    let x1 = bounds.x1.ceil().max(x0 + 1.0);
    let y1 = bounds.y1.ceil().max(y0 + 1.0);
    let t = layer.transform;
    let center = t.point(Point::new(
        ((x0 + x1) * 0.5 / w) as f32,
        ((y0 + y1) * 0.5 / h) as f32,
    ));
    let mut transform = t;
    transform.width = (f64::from(t.width) * (x1 - x0) / w) as f32;
    transform.height = (f64::from(t.height) * (y1 - y0) / h) as f32;
    transform.x = center.x - transform.width * 0.5;
    transform.y = center.y - transform.height * 0.5;
    ensure!(transform.valid(), "The outline is too large");
    let d = xuan::vector::VectorPath::from_bez(Affine::translate((-x0, -y0)) * &local)?;
    let shape = layer
        .shape
        .as_mut()
        .and_then(|s| s.path.as_mut())
        .context("The layer is not a path shape")?;
    shape.d = d;
    shape.width = (x1 - x0) as f32;
    shape.height = (y1 - y0) as f32;
    shape.validate()?;
    layer.transform = transform;
    // `refresh_shapes` redraws it at the new size.
    layer.pixels = None;
    Ok(())
}

impl EditorApp {
    fn current_document_id(&self) -> Option<Uuid> {
        self.session().map(|s| s.document.id)
    }

    /// The path the Pen edits: the one chosen, if it still exists, or else the active layer's
    /// outline when it is a path shape layer.
    pub(super) fn pen_target(&self) -> Option<PenTarget> {
        let document = &self.session()?.document;
        if let Some((id, target)) = self.pen.target
            && id == document.id
            && self.pen_outline(target).is_some()
        {
            return Some(target);
        }
        let layer = document.active()?;
        layer_outline(layer).map(|_| PenTarget::Layer(layer.id))
    }

    /// The target's anchors in document coordinates.
    pub(super) fn pen_outline(&self, target: PenTarget) -> Option<EditPath> {
        let document = &self.session()?.document;
        match target {
            PenTarget::Path(id) => document
                .paths
                .iter()
                .find(|p| p.id == id)
                .map(|p| EditPath::from_vector(&p.d)),
            PenTarget::Layer(id) => document
                .layers
                .iter()
                .find(|l| l.id == id)
                .and_then(layer_outline),
        }
    }

    /// Show `target` for editing.
    pub(super) fn pen_select(&mut self, target: Option<PenTarget>) {
        let document = self.current_document_id();
        self.pen.target = document.zip(target);
        self.pen.selected = None;
    }

    /// The path the overlay draws: the working copy while a drag edits it.
    fn pen_shown(&self) -> Option<EditPath> {
        if let Some(working) = self.pen.drag.as_ref().and_then(|d| d.working.clone()) {
            return Some(working);
        }
        self.pen_target().and_then(|t| self.pen_outline(t))
    }

    /// Pointer input on the canvas with the Pen tool.
    pub(super) fn pen_pointer(
        &mut self,
        ctx: &egui::Context,
        hovered: bool,
        point: Option<Point>,
        screen: Option<Pos2>,
        zoom: f32,
    ) {
        let (pressed, down, modifiers) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.modifiers,
            )
        });
        let (Some(point), Some(screen)) = (point, screen) else {
            return;
        };
        let mut just_pressed = false;
        if pressed && hovered {
            self.pen_press(kp(point), screen, zoom);
            just_pressed = true;
        }
        if self.pen.drag.is_some() {
            if down {
                if !just_pressed {
                    self.pen_drag(kp(point), screen, modifiers.alt);
                }
            } else {
                self.pen_release(modifiers.alt);
            }
        }
    }

    fn pen_press(&mut self, point: kurbo::Point, screen: Pos2, zoom: f32) {
        let radius = f64::from(HIT_RADIUS / zoom.max(1e-6));
        let Some(document) = self.current_document_id() else {
            return;
        };
        if self
            .pen
            .draft
            .as_ref()
            .is_some_and(|d| d.document != document)
        {
            self.pen.draft = None;
        }
        let drag = |kind, target, working| PenDrag {
            kind,
            target,
            press: point,
            screen,
            origin: point,
            moved: false,
            working,
        };
        if let Some(draft) = &mut self.pen.draft {
            let anchors = &mut draft.subpath.anchors;
            if anchors.len() >= 2 && anchors[0].point.distance(point) <= radius {
                self.pen.drag = Some(drag(DragKind::Close, None, None));
            } else {
                anchors.push(Anchor::corner(point));
                self.pen.drag = Some(drag(DragKind::Draft, None, None));
            }
            return;
        }
        // The path shown: its handles, anchors and segments.
        if let Some(target) = self.pen_target()
            && let Some(mut path) = self.pen_outline(target)
            && let Some(hit) = path.hit(point, radius, |_, _| true)
        {
            let kind = match hit {
                Hit::Anchor { subpath, anchor } => DragKind::Anchor { subpath, anchor },
                Hit::Handle {
                    subpath,
                    anchor,
                    side,
                } => DragKind::Handle {
                    subpath,
                    anchor,
                    side,
                },
                Hit::Segment {
                    subpath,
                    segment,
                    t,
                } => {
                    let anchor = path.split(subpath, segment, t);
                    DragKind::Added { subpath, anchor }
                }
            };
            if let DragKind::Anchor { subpath, anchor }
            | DragKind::Handle {
                subpath, anchor, ..
            }
            | DragKind::Added { subpath, anchor } = kind
            {
                self.pen.selected = Some((subpath, anchor));
            }
            let origin = match kind {
                DragKind::Anchor { subpath, anchor } | DragKind::Added { subpath, anchor } => {
                    path.anchor(subpath, anchor).map_or(point, |a| a.point)
                }
                _ => point,
            };
            self.pen.target = Some((document, target));
            self.pen.drag = Some(PenDrag {
                origin,
                ..drag(kind, Some(target), Some(path))
            });
            return;
        }
        // Another path: show it for editing.
        if let Some(target) = self.pen_hit_other(point, radius) {
            self.pen_select(Some(target));
            return;
        }
        self.pen.target = None;
        self.pen.selected = None;
        self.pen.draft = Some(Draft {
            document,
            subpath: Subpath {
                anchors: vec![Anchor::corner(point)],
                closed: false,
            },
        });
        self.pen.drag = Some(drag(DragKind::Draft, None, None));
    }

    /// A document path, or the active path shape layer's outline, passing within `radius` of
    /// `point`, the newest first.
    fn pen_hit_other(&self, point: kurbo::Point, radius: f64) -> Option<PenTarget> {
        let document = &self.session()?.document;
        let shown = self.pen_target();
        let layer = document
            .active()
            .and_then(|l| layer_outline(l).map(|p| (PenTarget::Layer(l.id), p)));
        let paths = document
            .paths
            .iter()
            .rev()
            .map(|p| (PenTarget::Path(p.id), EditPath::from_vector(&p.d)));
        layer
            .into_iter()
            .chain(paths)
            .filter(|(target, _)| Some(*target) != shown)
            .find(|(_, path)| path.hit(point, radius, |_, _| false).is_some())
            .map(|(target, _)| target)
    }

    fn pen_drag(&mut self, point: kurbo::Point, screen: Pos2, alt: bool) {
        let Some(drag) = &mut self.pen.drag else {
            return;
        };
        if !drag.moved && drag.screen.distance(screen) < DRAG_THRESHOLD {
            return;
        }
        drag.moved = true;
        let delta = point - drag.press;
        match drag.kind {
            DragKind::Draft => {
                if let Some(anchor) = self
                    .pen
                    .draft
                    .as_mut()
                    .and_then(|d| d.subpath.anchors.last_mut())
                {
                    *anchor = Anchor::smooth(anchor.point, point);
                }
            }
            DragKind::Close => {}
            DragKind::Anchor { subpath, anchor } => {
                let Some(path) = &mut drag.working else {
                    return;
                };
                if alt {
                    path.pull_handles(subpath, anchor, point);
                } else {
                    path.move_anchor(subpath, anchor, drag.origin + delta);
                }
            }
            DragKind::Added { subpath, anchor } => {
                if let Some(path) = &mut drag.working {
                    path.move_anchor(subpath, anchor, drag.origin + delta);
                }
            }
            DragKind::Handle {
                subpath,
                anchor,
                side,
            } => {
                if let Some(path) = &mut drag.working {
                    path.move_handle(subpath, anchor, side, point, alt);
                }
            }
        }
    }

    fn pen_release(&mut self, alt: bool) {
        let Some(mut drag) = self.pen.drag.take() else {
            return;
        };
        let (Some(target), Some(mut path)) = (drag.target, drag.working.take()) else {
            if drag.kind == DragKind::Close {
                if let Some(draft) = &mut self.pen.draft {
                    draft.subpath.closed = true;
                }
                self.pen_finish();
            }
            return;
        };
        let name = match drag.kind {
            DragKind::Draft | DragKind::Close => return,
            DragKind::Anchor { .. } if drag.moved && alt => tr("Convert Anchor"),
            DragKind::Anchor { .. } if drag.moved => tr("Move Anchor"),
            DragKind::Anchor { subpath, anchor } if alt => {
                path.convert(subpath, anchor);
                tr("Convert Anchor")
            }
            DragKind::Anchor { subpath, anchor } => {
                path.delete_anchor(subpath, anchor);
                self.pen.selected = None;
                tr("Delete Anchor")
            }
            DragKind::Handle { .. } if !drag.moved => return,
            DragKind::Handle { .. } => tr("Move Handle"),
            DragKind::Added { .. } => tr("Add Anchor"),
        };
        self.pen_commit(name, target, &path);
    }

    /// Store `path` as `target`'s new outline, as one undo step. A document path left without
    /// anchors is deleted.
    pub(super) fn pen_commit(&mut self, name: &str, target: PenTarget, path: &EditPath) {
        match target {
            PenTarget::Path(id) => {
                if path.is_empty() {
                    self.edit(tr("Delete Path"), |document| {
                        document.paths.retain(|p| p.id != id);
                        Ok(())
                    });
                    self.pen.target = None;
                    self.pen.selected = None;
                    return;
                }
                let d = match path.to_vector() {
                    Ok(d) => d,
                    Err(error) => {
                        self.error = Some(error.to_string());
                        return;
                    }
                };
                self.edit(name, |document| {
                    let path = (document.paths.iter_mut())
                        .find(|p| p.id == id)
                        .context("The path is gone")?;
                    path.d = d;
                    Ok(())
                });
            }
            PenTarget::Layer(id) => self.edit(name, |document| {
                let layer = (document.layers.iter_mut())
                    .find(|l| l.id == id)
                    .context("The layer is gone")?;
                set_layer_outline(layer, path)
            }),
        }
    }

    /// Finish the path being drawn: it becomes the next "Path N" of the document, as one undo
    /// step, and is shown for editing. A path of one anchor is dropped.
    pub(super) fn pen_finish(&mut self) {
        if self
            .pen
            .drag
            .as_ref()
            .is_some_and(|d| matches!(d.kind, DragKind::Draft | DragKind::Close))
        {
            self.pen.drag = None;
        }
        let Some(draft) = self.pen.draft.take() else {
            return;
        };
        if Some(draft.document) != self.current_document_id() || draft.subpath.anchors.len() < 2
        {
            return;
        }
        let path = EditPath {
            subpaths: vec![draft.subpath],
        };
        let d = match path.to_vector() {
            Ok(d) => d,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let mut id = None;
        self.edit(tr("New Path"), |document| {
            let path = NamedPath::new(xuan::vector::next_path_name(&document.paths), d);
            id = Some(path.id);
            document.paths.push(path);
            xuan::vector::validate_paths(&document.paths)
        });
        if self.error.is_none() {
            self.pen_select(id.map(PenTarget::Path));
        }
    }

    /// Escape: finish the path being drawn, or else stop showing the path for editing.
    pub(super) fn pen_escape(&mut self) {
        self.pen.drag = None;
        if self.pen.draft.is_some() {
            self.pen_finish();
        } else {
            self.pen.target = None;
            self.pen.selected = None;
        }
    }

    /// Delete or Backspace: remove the draft's last anchor, or the selected anchor of the path
    /// shown. Whether there was one.
    pub(super) fn pen_delete(&mut self) -> bool {
        if let Some(draft) = &mut self.pen.draft {
            draft.subpath.anchors.pop();
            if draft.subpath.anchors.is_empty() {
                self.pen.draft = None;
            }
            return true;
        }
        let Some((subpath, anchor)) = self.pen.selected else {
            return false;
        };
        let Some(target) = self.pen_target() else {
            return false;
        };
        let Some(mut path) = self.pen_outline(target) else {
            return false;
        };
        if path.anchor(subpath, anchor).is_none() {
            self.pen.selected = None;
            return false;
        }
        path.delete_anchor(subpath, anchor);
        self.pen.selected = None;
        self.pen_commit(tr("Delete Anchor"), target, &path);
        true
    }

    /// What the tool options bar says about the Pen.
    pub(super) fn pen_status(&self) -> String {
        if self.pen.draft.is_some() {
            return tr("Click the first anchor to close the path · Enter finishes it").into();
        }
        let Some(session) = self.session() else {
            return String::new();
        };
        match self.pen_target() {
            Some(PenTarget::Path(id)) => {
                let name = (session.document.paths.iter())
                    .find(|p| p.id == id)
                    .map_or("", |p| p.name.as_str());
                tr("Editing “{}”").replace("{}", name)
            }
            Some(PenTarget::Layer(id)) => {
                let name = (session.document.layers.iter())
                    .find(|l| l.id == id)
                    .map_or("", |l| l.name.as_str());
                tr("Editing the outline of “{}”").replace("{}", name)
            }
            None => tr("Click to start a path · Click a path to edit it").into(),
        }
    }

    /// Draw the path being drawn, its rubber band to the pointer, and the path shown for
    /// editing with its anchors and handles, all in screen space.
    pub(super) fn pen_overlay(
        &self,
        painter: &egui::Painter,
        origin: Pos2,
        zoom: f32,
        pointer: Option<Pos2>,
    ) {
        let to_screen = Affine::new([
            f64::from(zoom),
            0.0,
            0.0,
            f64::from(zoom),
            f64::from(origin.x),
            f64::from(origin.y),
        ]);
        let screen = |p: kurbo::Point| {
            let p = to_screen * p;
            Pos2::new(p.x as f32, p.y as f32)
        };
        let line = Stroke::new(1.0_f32, PATH_COLOR);
        if let Some(path) = self.pen_shown() {
            draw_bez(painter, &(to_screen * &path.to_bez()), line);
            for (s, sub) in path.subpaths.iter().enumerate() {
                for (a, anchor) in sub.anchors.iter().enumerate() {
                    draw_anchor(
                        painter,
                        screen(anchor.point),
                        anchor,
                        &screen,
                        self.pen.selected == Some((s, a)),
                    );
                }
            }
        }
        let Some(draft) = &self.pen.draft else {
            return;
        };
        if self.current_document_id() != Some(draft.document) {
            return;
        }
        let draft_path = EditPath {
            subpaths: vec![draft.subpath.clone()],
        };
        draw_bez(painter, &(to_screen * &draft_path.to_bez()), line);
        let dragging = self.pen.drag.is_some();
        if let (Some(last), Some(pointer), false) =
            (draft.subpath.anchors.last(), pointer, dragging)
        {
            let start = screen(last.point);
            if last.has_out() {
                let mut band = BezPath::new();
                band.move_to(kurbo::Point::new(f64::from(start.x), f64::from(start.y)));
                let p = kurbo::Point::new(f64::from(pointer.x), f64::from(pointer.y));
                let c = screen(last.handle_out);
                band.curve_to(kurbo::Point::new(f64::from(c.x), f64::from(c.y)), p, p);
                draw_bez(painter, &band, Stroke::new(1.0_f32, PATH_COLOR.gamma_multiply(0.7)));
            } else {
                painter.line_segment(
                    [start, pointer],
                    Stroke::new(1.0_f32, PATH_COLOR.gamma_multiply(0.7)),
                );
            }
        }
        let count = draft.subpath.anchors.len();
        for (index, anchor) in draft.subpath.anchors.iter().enumerate() {
            draw_anchor(
                painter,
                screen(anchor.point),
                anchor,
                &screen,
                index + 1 == count,
            );
        }
    }
}

/// `path` (screen coordinates) as 1 px lines.
fn draw_bez(painter: &egui::Painter, path: &BezPath, stroke: Stroke) {
    let mut points: Vec<Pos2> = Vec::new();
    let mut start = None;
    let mut flush = |points: &mut Vec<Pos2>| {
        if points.len() > 1 {
            painter.add(egui::Shape::line(std::mem::take(points), stroke));
        }
        points.clear();
    };
    kurbo::flatten(path, 0.25, |element| match element {
        PathEl::MoveTo(p) => {
            flush(&mut points);
            let p = Pos2::new(p.x as f32, p.y as f32);
            start = Some(p);
            points.push(p);
        }
        PathEl::LineTo(p) => points.push(Pos2::new(p.x as f32, p.y as f32)),
        PathEl::ClosePath => {
            if let Some(p) = start {
                points.push(p);
            }
        }
        _ => {}
    });
    flush(&mut points);
}

fn draw_anchor(
    painter: &egui::Painter,
    at: Pos2,
    anchor: &Anchor,
    screen: &impl Fn(kurbo::Point) -> Pos2,
    selected: bool,
) {
    for side in [Side::In, Side::Out] {
        let handle = anchor.handle(side);
        if handle != anchor.point {
            let h = screen(handle);
            painter.line_segment([at, h], Stroke::new(1.0_f32, PATH_COLOR));
            painter.circle(
                h,
                3.0,
                Color32::WHITE,
                Stroke::new(1.0_f32, PATH_COLOR),
            );
        }
    }
    painter.rect(
        Rect::from_center_size(at, Vec2::splat(7.0)),
        0.0,
        if selected { PATH_COLOR } else { Color32::WHITE },
        Stroke::new(1.0_f32, PATH_COLOR),
        StrokeKind::Inside,
    );
}

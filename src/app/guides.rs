//! Guides: dragged out of the rulers, moved with the Move tool, removed by dropping them back on a
//! ruler or with View → Clear Guides.
//!
//! Follows Compositor's `Document/Guides.swift`: cyan lines across the whole view, a 5 point
//! grab distance, edits recorded as undoable "New Guide", "Move Guide", "Delete Guide" and
//! "Clear Guides" steps, and guides that snap to the other View → Snap To targets while dragged.
use std::collections::HashSet;

use egui::{Color32, Painter, Pos2, Rect, pos2};
use uuid::Uuid;
use xuan::{
    i18n::tr,
    layout::{Guide, GuideAxis},
};

use xuan::{document::Point, operations};

use super::{
    EditorApp,
    canvas::near_transform_handle,
    commands::ctrl_or_cmd,
    pixel_grid::align_to_pixel,
    rulers::RulerLayout,
    snap::{self, SnapOptions, SnapTargets},
};

/// Photoshop's default guide color, cyan at 90%.
pub(super) const GUIDE_COLOR: Color32 = Color32::from_rgba_unmultiplied_const(0, 255, 255, 230);
/// How close, in points, the pointer must come to grab a guide.
pub(super) const GUIDE_HIT_DISTANCE: f32 = 5.0;

/// A guide being created or moved; the document changes only when the drag ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GuideDrag {
    pub id: Uuid,
    pub axis: GuideAxis,
    pub position: f32,
    /// Where an existing guide was; `None` for one being dragged out of a ruler.
    pub original: Option<f32>,
}

/// Screen position of a guide for an image at `origin` and `zoom`.
pub(super) fn screen_position(guide: &Guide, origin: Pos2, zoom: f32) -> f32 {
    match guide.axis {
        GuideAxis::Vertical => origin.x + guide.position * zoom,
        GuideAxis::Horizontal => origin.y + guide.position * zoom,
    }
}

/// The guide nearest `point` within `tolerance` points, if any.
pub(super) fn hit_test(
    guides: &[Guide],
    point: Pos2,
    origin: Pos2,
    zoom: f32,
    tolerance: f32,
) -> Option<Guide> {
    guides
        .iter()
        .map(|guide| {
            let at = screen_position(guide, origin, zoom);
            let distance = match guide.axis {
                GuideAxis::Vertical => (point.x - at).abs(),
                GuideAxis::Horizontal => (point.y - at).abs(),
            };
            (guide, distance)
        })
        .filter(|(_, distance)| *distance <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(guide, _)| *guide)
}

/// The document position along `axis` under a screen `point`.
pub(super) fn document_position(axis: GuideAxis, point: Pos2, origin: Pos2, zoom: f32) -> f32 {
    match axis {
        GuideAxis::Vertical => (point.x - origin.x) / zoom,
        GuideAxis::Horizontal => (point.y - origin.y) / zoom,
    }
}

/// Paints guides across all of `viewport`, one physical pixel wide.
pub(super) fn paint(painter: &Painter, guides: &[Guide], origin: Pos2, zoom: f32, viewport: Rect) {
    let ppp = painter.pixels_per_point();
    let thickness = 1.0 / ppp.max(f32::MIN_POSITIVE);
    let mut mesh = egui::Mesh::default();
    for guide in guides {
        let at = align_to_pixel(screen_position(guide, origin, zoom), ppp);
        let rect = match guide.axis {
            GuideAxis::Vertical => Rect::from_min_max(
                pos2(at, viewport.top()),
                pos2(at + thickness, viewport.bottom()),
            ),
            GuideAxis::Horizontal => Rect::from_min_max(
                pos2(viewport.left(), at),
                pos2(viewport.right(), at + thickness),
            ),
        };
        if rect.intersects(viewport) {
            mesh.add_colored_rect(rect, GUIDE_COLOR);
        }
    }
    painter
        .with_clip_rect(viewport)
        .add(egui::Shape::mesh(mesh));
}

impl EditorApp {
    /// Rulers and guides for one canvas frame: starts a guide drag from a ruler, or from a guide
    /// under a Move tool press, follows the pointer, and finishes on release, deleting the guide
    /// when it is dropped on a ruler. Returns the cursor to show over the canvas, if any.
    pub(super) fn guide_interaction(
        &mut self,
        ctx: &egui::Context,
        canvas: &egui::Response,
        rulers: Option<(&RulerLayout, &(egui::Response, egui::Response))>,
        origin: Pos2,
        zoom: f32,
    ) -> Option<egui::CursorIcon> {
        let (pointer, press, down, free, space) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.press_origin(),
                i.pointer.primary_down(),
                ctrl_or_cmd(i.modifiers),
                i.key_down(egui::Key::Space),
            )
        });
        let cursor = |axis: GuideAxis| match axis {
            GuideAxis::Vertical => egui::CursorIcon::ResizeHorizontal,
            GuideAxis::Horizontal => egui::CursorIcon::ResizeVertical,
        };
        let interrupted = self.error.is_some()
            || self.close_app
            || self.close_tab.is_some()
            || self.job.is_some()
            || self.dialog.is_some()
            || self.develop.is_some();
        if interrupted {
            self.cancel_guide_drag();
            return None;
        }
        if self.guide_drag.is_none() {
            if let Some((_, (top, left))) = rulers {
                for (response, axis) in [(top, GuideAxis::Horizontal), (left, GuideAxis::Vertical)]
                {
                    if response.hovered() || response.dragged() {
                        ctx.set_cursor_icon(cursor(axis));
                    }
                    if response.drag_started_by(egui::PointerButton::Primary)
                        && let Some(at) = press.or(pointer)
                    {
                        self.begin_guide_creation(
                            axis,
                            document_position(axis, at, origin, zoom),
                            free,
                        );
                    }
                }
            }
            if self.guide_drag.is_none() && self.tool == super::Tool::Move && !space {
                let handles = |app: &Self, at: Pos2| {
                    app.show_controls
                        && app.session().is_some_and(|session| {
                            operations::transform_box(&session.document, app.transforming_mask())
                                .is_some_and(|t| {
                                    near_transform_handle(
                                        t,
                                        Point::new(
                                            (at.x - origin.x) / zoom,
                                            (at.y - origin.y) / zoom,
                                        ),
                                        zoom,
                                    )
                                })
                        })
                };
                if canvas.drag_started_by(egui::PointerButton::Primary)
                    && let Some(at) = press
                    && !handles(self, at)
                    && let Some(guide) = self.guide_at(at, origin, zoom)
                {
                    self.begin_guide_move(guide);
                } else if canvas.hovered()
                    && let Some(at) = pointer
                    && !handles(self, at)
                    && let Some(guide) = self.guide_at(at, origin, zoom)
                {
                    return Some(cursor(guide.axis));
                }
            }
        }
        let drag = self.guide_drag?;
        ctx.set_cursor_icon(cursor(drag.axis));
        if down {
            if let Some(at) = pointer {
                self.move_guide_drag(document_position(drag.axis, at, origin, zoom), free);
            }
        } else {
            let over_ruler = rulers
                .zip(pointer)
                .is_some_and(|((layout, _), at)| layout.contains(at));
            self.finish_guide_drag(over_ruler);
        }
        ctx.request_repaint();
        Some(cursor(drag.axis))
    }

    /// What snaps, from the View menu; `None` when snapping is off.
    pub(super) fn snap_options(&self) -> Option<SnapOptions> {
        self.config.snap.enabled.then(|| SnapOptions {
            settings: self.config.snap,
            show_guides: self.config.show_guides,
            show_grid: self.showing_grid(),
            grid: self.grid_settings(),
        })
    }

    /// The current document's guides as shown, including a drag in progress.
    pub(super) fn displayed_guides(&self) -> Vec<Guide> {
        let mut guides = self
            .session()
            .map(|s| s.document.guides.clone())
            .unwrap_or_default();
        if let Some(drag) = self.guide_drag {
            let current = Guide {
                id: drag.id,
                axis: drag.axis,
                position: drag.position,
            };
            match guides.iter_mut().find(|g| g.id == drag.id) {
                Some(guide) => *guide = current,
                None => guides.push(current),
            }
        }
        guides
    }

    /// Guides can be added and moved: a document is open, guides are unlocked, and nothing modal
    /// or mid-gesture is in the way.
    pub(super) fn can_edit_guides(&self) -> bool {
        self.session().is_some()
            && !self.config.lock_guides
            && self.job.is_none()
            && self.develop.is_none()
            && self.dialog.is_none()
            && self.gesture.is_none()
            && self.rename.is_none()
    }

    /// The guide under a screen `point`, if guides are shown, unlocked and editable.
    pub(super) fn guide_at(&self, point: Pos2, origin: Pos2, zoom: f32) -> Option<Guide> {
        if !self.config.show_guides || !self.can_edit_guides() {
            return None;
        }
        hit_test(
            &self.displayed_guides(),
            point,
            origin,
            zoom,
            GUIDE_HIT_DISTANCE,
        )
    }

    /// Starts dragging a new guide out of a ruler. New guides show guides again, as upstream.
    pub(super) fn begin_guide_creation(&mut self, axis: GuideAxis, position: f32, free: bool) {
        if !self.can_edit_guides() {
            return;
        }
        if !self.config.show_guides {
            self.set_view_option(|config| config.show_guides = true);
        }
        let id = Uuid::new_v4();
        self.guide_drag = Some(GuideDrag {
            id,
            axis,
            position: self.snapped_guide_position(axis, position, id, free),
            original: None,
        });
    }

    pub(super) fn begin_guide_move(&mut self, guide: Guide) {
        if !self.can_edit_guides() {
            return;
        }
        self.guide_drag = Some(GuideDrag {
            id: guide.id,
            axis: guide.axis,
            position: guide.position,
            original: Some(guide.position),
        });
    }

    /// Follows the pointer; holding Ctrl drags without snapping, as for other drags.
    pub(super) fn move_guide_drag(&mut self, position: f32, free: bool) {
        let Some(drag) = self.guide_drag else {
            return;
        };
        let position = self.snapped_guide_position(drag.axis, position, drag.id, free);
        if let Some(drag) = &mut self.guide_drag {
            drag.position = position;
        }
    }

    /// Ends the drag. `delete` is true when it was released over a ruler: a new guide is
    /// abandoned and an existing one removed.
    pub(super) fn finish_guide_drag(&mut self, delete: bool) {
        let Some(drag) = self.guide_drag.take() else {
            return;
        };
        self.snap_lines.clear();
        let Some(session) = self.session_mut() else {
            return;
        };
        let name = match (delete, drag.original) {
            (true, None) => return,
            (true, Some(_)) => tr("Delete Guide"),
            (false, None) => tr("New Guide"),
            (false, Some(original)) if original == drag.position => return,
            (false, Some(_)) => tr("Move Guide"),
        };
        session.history.commit();
        session.history.begin(name, &session.document);
        let guides = &mut session.document.guides;
        if delete {
            guides.retain(|g| g.id != drag.id);
        } else if let Some(guide) = guides.iter_mut().find(|g| g.id == drag.id) {
            guide.position = drag.position;
        } else if guides.len() < xuan::layout::MAX_GUIDES
            && drag.position.abs() <= xuan::layout::MAX_GUIDE_POSITION
        {
            guides.push(Guide {
                id: drag.id,
                axis: drag.axis,
                position: drag.position,
            });
        }
        session.history.commit();
        self.status = name.into();
    }

    pub(super) fn cancel_guide_drag(&mut self) {
        if self.guide_drag.take().is_some() {
            self.snap_lines.clear();
        }
    }

    /// View → Clear Guides, as one undoable step.
    pub(super) fn clear_guides(&mut self) {
        self.cancel_guide_drag();
        let Some(session) = self.session_mut() else {
            return;
        };
        if session.document.guides.is_empty() {
            return;
        }
        session.history.commit();
        session.history.begin(tr("Clear Guides"), &session.document);
        session.document.guides.clear();
        session.history.commit();
        self.status = tr("Clear Guides").into();
    }

    /// A dragged guide's position pulled onto a nearby target along its own axis: the grid, the
    /// other guides, the canvas edges and middle, and layer edges and middles.
    fn snapped_guide_position(
        &mut self,
        axis: GuideAxis,
        position: f32,
        id: Uuid,
        free: bool,
    ) -> f32 {
        self.snap_lines.clear();
        let (Some(options), Some(session)) = (self.snap_options(), self.session()) else {
            return position;
        };
        if free {
            return position;
        }
        let others: Vec<Guide> = self
            .displayed_guides()
            .into_iter()
            .filter(|g| g.id != id && g.axis == axis)
            .collect();
        let targets = SnapTargets::new(&session.document, &others, &options, &HashSet::new(), true);
        match targets.nearest(axis, position, snap::tolerance(session.zoom)) {
            Some((target, _)) => target,
            None => position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_picks_the_nearest_guide_within_reach() {
        let a = Guide::new(GuideAxis::Vertical, 10.0);
        let b = Guide::new(GuideAxis::Vertical, 14.0);
        let c = Guide::new(GuideAxis::Horizontal, 50.0);
        let guides = [a, b, c];
        // At 100% with the image at (100, 200): `a` at x 110, `b` at 114, `c` at y 250.
        let origin = pos2(100.0, 200.0);
        assert_eq!(
            hit_test(&guides, pos2(111.0, 0.0), origin, 1.0, 5.0),
            Some(a)
        );
        assert_eq!(
            hit_test(&guides, pos2(113.0, 0.0), origin, 1.0, 5.0),
            Some(b)
        );
        assert_eq!(
            hit_test(&guides, pos2(500.0, 253.0), origin, 1.0, 5.0),
            Some(c)
        );
        assert_eq!(
            hit_test(&guides, pos2(130.0, 300.0), origin, 1.0, 5.0),
            None
        );
        // The reach is in points: at 400% `a` is at 140 and `b` at 156.
        assert_eq!(
            hit_test(&guides, pos2(144.0, 0.0), origin, 4.0, 5.0),
            Some(a)
        );
        assert_eq!(hit_test(&guides, pos2(148.0, 0.0), origin, 4.0, 5.0), None);
        assert!(hit_test(&[], pos2(0.0, 0.0), origin, 1.0, 5.0).is_none());
    }

    #[test]
    fn positions_map_both_ways() {
        let origin = pos2(-20.0, 40.0);
        let guide = Guide::new(GuideAxis::Horizontal, 12.5);
        let y = screen_position(&guide, origin, 2.0);
        assert_eq!(y, 65.0);
        assert_eq!(
            document_position(GuideAxis::Horizontal, pos2(0.0, y), origin, 2.0),
            12.5
        );
        assert_eq!(
            document_position(GuideAxis::Vertical, pos2(0.0, y), origin, 2.0),
            10.0
        );
    }
}

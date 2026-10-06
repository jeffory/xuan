use super::theme::PaletteExt as _;
use super::widgets;
use std::{ops::RangeInclusive, sync::Arc};
use xuan::i18n::tr;

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use xuan::{
    document::Point,
    layout::GuideAxis,
    operations,
    paint::{self, PaintMode},
    render,
    selection::{self, SelectionMode},
};

use super::{EditorApp, Gesture, Tool, TransformDrag};

/// Colour of the Clone Stamp source marker: dimmer than the pointer's outline.
pub(super) const CLONE_SOURCE_COLOR: Color32 = Color32::from_rgba_premultiplied(190, 190, 190, 190);

/// Where the Clone Stamp samples from, in document coordinates. Before an
/// offset exists (and in unaligned mode between strokes) that is the stored
/// source; otherwise it is the pointer shifted by the offset (`source - start`).
pub(super) fn clone_sample_position(
    source: Option<Point>,
    offset: Option<Point>,
    pointer: Option<Point>,
    aligned: bool,
    stroking: bool,
) -> Option<Point> {
    source?;
    match (offset, pointer) {
        (Some(offset), Some(pointer)) if aligned || stroking => {
            Some(Point::new(pointer.x + offset.x, pointer.y + offset.y))
        }
        _ => source,
    }
}

/// Screen-space outline of the brush tip centred at `centre`; `shape` is the
/// tilt axis, tilt aspect and pressure scale from `EditorApp::brush_shape`.
fn brush_outline(
    brush: &paint::Brush,
    pencil: bool,
    centre: Pos2,
    origin: Pos2,
    zoom: f32,
    (axis, aspect, pressure): (Point, f32, f32),
) -> Vec<Pos2> {
    if pencil {
        // Pixel-exact tip: whole-pixel size, snapped to the pixel grid.
        let size = (brush.diameter * pressure).round().max(1.0);
        let half = size * zoom * 0.5;
        let doc = Point::new((centre.x - origin.x) / zoom, (centre.y - origin.y) / zoom);
        let snapped = if size % 2.0 == 1.0 {
            Point::new(doc.x.floor() + 0.5, doc.y.floor() + 0.5)
        } else {
            Point::new(doc.x.round(), doc.y.round())
        };
        let centre = origin + vec2(snapped.x, snapped.y) * zoom;
        if brush.square {
            vec![
                centre + vec2(-half, -half),
                centre + vec2(half, -half),
                centre + vec2(half, half),
                centre + vec2(-half, half),
            ]
        } else {
            (0..48)
                .map(|i| {
                    let angle = i as f32 * std::f32::consts::TAU / 48.0;
                    centre + vec2(angle.cos(), angle.sin()) * half
                })
                .collect()
        }
    } else {
        let radius = brush.diameter * zoom * pressure * 0.5;
        (0..48)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / 48.0;
                let (sin, cos) = angle.sin_cos();
                centre
                    + vec2(
                        axis.x * cos - axis.y * sin * aspect,
                        axis.y * cos + axis.x * sin * aspect,
                    ) * radius
            })
            .collect()
    }
}

/// A contrasting double stroke: a dark halo under a light line.
fn paint_brush_outline(painter: &egui::Painter, outline: Vec<Pos2>, light: Color32, halo: u8) {
    painter.add(egui::Shape::closed_line(
        outline.clone(),
        Stroke::new(2.5_f32, Color32::from_black_alpha(halo)),
    ));
    painter.add(egui::Shape::closed_line(
        outline,
        Stroke::new(1.0_f32, light),
    ));
}

impl EditorApp {
    /// Tilt axis, tilt aspect and pressure scale of the brush tip right now.
    fn brush_shape(&self) -> (Point, f32, f32) {
        let pen = self.tablet.as_ref().and_then(|tablet| tablet.sample());
        let tilt = if self.tilt_shape {
            pen.and_then(|sample| sample.tilt).unwrap_or([0.0; 2])
        } else {
            [0.0; 2]
        };
        let (axis, aspect) = paint::tilt_shape(tilt);
        let pressure = if self.pressure_size {
            pen.filter(|sample| {
                matches!(
                    sample.phase,
                    super::tablet::Phase::Down | super::tablet::Phase::Move
                )
            })
            .and_then(|sample| sample.pressure)
            .unwrap_or(1.0)
            .max(0.01)
        } else {
            1.0
        };
        (axis, aspect, pressure)
    }
}

const HANDLES: [Point; 8] = [
    Point::new(0.0, 0.0),
    Point::new(0.5, 0.0),
    Point::new(1.0, 0.0),
    Point::new(1.0, 0.5),
    Point::new(1.0, 1.0),
    Point::new(0.5, 1.0),
    Point::new(0.0, 1.0),
    Point::new(0.0, 0.5),
];

pub(super) enum ZoomAnchor {
    Pointer,
    Center,
}

/// Smallest and largest zoom factor the editor allows.
pub(super) const ZOOM_LIMITS: RangeInclusive<f32> = 0.01..=64.0;

/// Screen position of the image's top-left corner: the image is centred in
/// `viewport` and then offset by `pan`. Every image/screen mapping derives from this.
pub(super) fn image_origin(viewport: Rect, image_size: Vec2, zoom: f32, pan: Vec2) -> Pos2 {
    viewport.center() - image_size * zoom * 0.5 + pan
}

/// Change `zoom` to `new_zoom` keeping the point `focus` (an offset from the
/// viewport centre) stationary. A zero `focus` zooms about the centre.
pub(super) fn zoom_about(zoom: &mut f32, pan: &mut Vec2, new_zoom: f32, focus: Vec2) {
    *pan += (focus - *pan) * (1.0 - new_zoom / *zoom);
    *zoom = new_zoom;
}

/// Pan horizontally and zoom using the unpanned image center.
pub(super) fn scroll_canvas(
    ui: &egui::Ui,
    response: &egui::Response,
    center: Pos2,
    anchor: ZoomAnchor,
    zoom: &mut f32,
    pan: &mut Vec2,
    limits: RangeInclusive<f32>,
) -> bool {
    if !response.hovered() {
        return false;
    }
    let scroll = ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta));
    if scroll == Vec2::ZERO {
        return false;
    }
    if scroll.y != 0.0 {
        let new_zoom = (*zoom * (scroll.y * 0.003).exp()).clamp(*limits.start(), *limits.end());
        let focus = match anchor {
            ZoomAnchor::Pointer => ui.input(|i| i.pointer.hover_pos()).unwrap_or(center),
            ZoomAnchor::Center => center,
        };
        zoom_about(zoom, pan, new_zoom, focus - center);
    }
    pan.x += scroll.x;
    true
}

/// Whether a press at `point` (document pixels) grabs one of the Move tool's handles for `t`,
/// which take precedence over guides beneath them.
pub(super) fn near_transform_handle(t: xuan::document::Transform, point: Point, zoom: f32) -> bool {
    if HANDLES
        .iter()
        .any(|unit| t.point(*unit).distance(point) * zoom < 9.0)
    {
        return true;
    }
    let top = t.point(Point::new(0.5, 0.0));
    let center = t.center();
    let distance = top.distance(center).max(0.01);
    let rotate = Point::new(
        top.x + (top.x - center.x) / distance * 23.0 / zoom,
        top.y + (top.y - center.y) / distance * 23.0 / zoom,
    );
    rotate.distance(point) * zoom < 9.0
}

fn drag_transform(
    old: xuan::document::Transform,
    start: Point,
    point: Point,
    kind: TransformDrag,
    lock_ratio: bool,
    shift: bool,
) -> xuan::document::Transform {
    let mut t = old;
    match kind {
        TransformDrag::Move | TransformDrag::Pixels => {
            t.x += point.x - start.x;
            t.y += point.y - start.y;
        }
        TransformDrag::Rotate => {
            let center = old.center();
            let a = (start.y - center.y).atan2(start.x - center.x);
            let b = (point.y - center.y).atan2(point.x - center.x);
            let angle = (b - a).to_degrees();
            t.rotation += if shift {
                (angle / 15.0).round() * 15.0
            } else {
                angle
            };
        }
        TransformDrag::Scale(index) => {
            let handle = HANDLES[index];
            let anchor = Point::new(1.0 - handle.x, 1.0 - handle.y);
            let unit = old.inverse(point);
            let anchor_point = old.point(anchor);
            let mut sx = if handle.x == 0.5 {
                1.0
            } else {
                ((unit.x - anchor.x) / (handle.x - anchor.x)).max(1.0 / old.width)
            };
            let mut sy = if handle.y == 0.5 {
                1.0
            } else {
                ((unit.y - anchor.y) / (handle.y - anchor.y)).max(1.0 / old.height)
            };
            if lock_ratio != shift {
                let factor = if handle.x == 0.5 {
                    sy
                } else if handle.y == 0.5 {
                    sx
                } else {
                    sx.max(sy)
                };
                sx = factor;
                sy = factor;
            }
            t.width = (old.width * sx).clamp(1.0, 300_000.0);
            t.height = (old.height * sy).clamp(1.0, 300_000.0);
            let moved_anchor = t.point(anchor);
            t.x += anchor_point.x - moved_anchor.x;
            t.y += anchor_point.y - moved_anchor.y;
        }
        TransformDrag::Distort(index) => {
            let mut affine = old;
            affine.warp = None;
            let mut quad = old
                .warp
                .unwrap_or([HANDLES[0], HANDLES[2], HANDLES[4], HANDLES[6]]);
            let mut moved = point;
            if shift {
                if (point.x - start.x).abs() > (point.y - start.y).abs() {
                    moved.y = start.y;
                } else {
                    moved.x = start.x;
                }
            }
            quad[index] = affine.inverse(moved);
            if xuan::geometry::Homography::from_quad(quad).is_some() {
                t.warp = Some(quad);
            }
        }
        TransformDrag::Selection => {}
    }
    t
}

impl EditorApp {
    pub(super) fn canvas(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ctx.palette().canvas))
            .show(ctx, |ui| {
                let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                if self.sessions.is_empty() {
                    self.welcome(ui, area);
                    return;
                }
                // View → Rulers takes a strip along the top and left of the canvas area.
                let rulers = self
                    .config
                    .rulers
                    .then(|| super::rulers::RulerLayout::new(area));
                let viewport = rulers.map_or(area, |layout| layout.canvas);
                let response =
                    ui.interact(viewport, ui.id().with("canvas"), Sense::click_and_drag());
                let ruler_responses = rulers.map(|layout| {
                    (
                        ui.interact(layout.top, ui.id().with("ruler_top"), Sense::drag()),
                        ui.interact(layout.left, ui.id().with("ruler_left"), Sense::drag()),
                    )
                });
                self.release_sample_caches();
                let mask_target = self.transforming_mask();
                let session = &mut self.sessions[self.current];
                if session.fit {
                    session.zoom = ((viewport.width() - 100.0) / session.document.width as f32)
                        .min((viewport.height() - 90.0) / session.document.height as f32)
                        .clamp(0.01, 8.0);
                    session.pan = Vec2::ZERO;
                    session.fit = false;
                }
                session.refresh(ctx, self.gpu_state.as_ref());
                let zoom = session.zoom;
                let size = vec2(
                    session.document.width as f32,
                    session.document.height as f32,
                ) * zoom;
                let origin = image_origin(
                    viewport,
                    vec2(
                        session.document.width as f32,
                        session.document.height as f32,
                    ),
                    zoom,
                    session.pan,
                );
                self.canvas_viewport = Some(viewport);
                let canvas = Rect::from_min_size(origin, size);
                self.canvas_rect = Some(canvas);
                let guide_cursor = self.guide_interaction(
                    ctx,
                    &response,
                    rulers.as_ref().zip(ruler_responses.as_ref()),
                    origin,
                    zoom,
                );
                let layout_grid = self.showing_grid().then(|| self.grid_settings());
                let shown_guides = if self.config.show_guides {
                    self.displayed_guides()
                } else {
                    Vec::new()
                };
                let session = &self.sessions[self.current];
                let visible = canvas.intersect(viewport);
                let painter = ui.painter().with_clip_rect(viewport);
                let palette = ui.palette();
                painter.rect_filled(canvas.expand(3.0), 0.0, palette.canvas_shadow);
                if visible.is_positive() {
                    let checker = 12.0;
                    let min_x = ((visible.left() - origin.x) / checker).floor() as i32;
                    let max_x = ((visible.right() - origin.x) / checker).ceil() as i32;
                    let min_y = ((visible.top() - origin.y) / checker).floor() as i32;
                    let max_y = ((visible.bottom() - origin.y) / checker).ceil() as i32;
                    let checker_painter = painter.with_clip_rect(visible);
                    for y in min_y..max_y {
                        for x in min_x..max_x {
                            checker_painter.rect_filled(
                                Rect::from_min_size(
                                    origin + vec2(x as f32 * checker, y as f32 * checker),
                                    Vec2::splat(checker),
                                ),
                                0.0,
                                palette.checker[usize::from((x + y) % 2 != 0)],
                            );
                        }
                    }
                    if let Some(texture) = session
                        .gpu
                        .as_ref()
                        .and_then(|g| g.texture)
                        .or_else(|| session.texture.as_ref().map(|t| t.id()))
                    {
                        painter.image(
                            texture,
                            canvas,
                            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                }
                painter.rect_stroke(
                    canvas,
                    0.0,
                    Stroke::new(1.0_f32, palette.canvas_edge),
                    StrokeKind::Outside,
                );
                let map = |p: Point| origin + vec2(p.x, p.y) * zoom;
                // A hardware-limited preview cannot represent individual document pixels.
                if self.config.pixel_grid
                    && session.preview_size == [session.document.width, session.document.height]
                {
                    super::pixel_grid::draw(
                        &painter,
                        origin,
                        Vec2::splat(zoom),
                        [session.document.width, session.document.height],
                        visible,
                        zoom * 100.0,
                        self.config.pixel_grid_percent(),
                    );
                }
                // The layout grid goes over the pixel grid, on the same physical pixels.
                if let Some(grid) = &layout_grid {
                    super::layout_grid::paint(
                        &painter,
                        grid,
                        origin,
                        zoom,
                        [session.document.width, session.document.height],
                        visible,
                    );
                }
                if let Some(mask) = &session.document.selection {
                    let step = (1.0 / zoom).ceil().max(1.0) as usize;
                    let start_x = ((visible.left() - origin.x) / zoom).floor().max(0.0) as u32;
                    let start_y = ((visible.top() - origin.y) / zoom).floor().max(0.0) as u32;
                    let end_x = ((visible.right() - origin.x) / zoom)
                        .ceil()
                        .min(mask.width() as f32) as u32;
                    let end_y = ((visible.bottom() - origin.y) / zoom)
                        .ceil()
                        .min(mask.height() as f32) as u32;
                    for y in (start_y..end_y).step_by(step) {
                        for x in (start_x..end_x).step_by(step) {
                            if mask.get_pixel(x, y)[0] < 128 {
                                continue;
                            }
                            let s = step as u32;
                            let color = if (x + y) / s % 8 < 4 {
                                Color32::WHITE
                            } else {
                                Color32::BLACK
                            };
                            if y < s || mask.get_pixel(x, y - s)[0] < 128 {
                                painter.line_segment(
                                    [
                                        map(Point::new(x as f32, y as f32)),
                                        map(Point::new((x + s) as f32, y as f32)),
                                    ],
                                    Stroke::new(1.0_f32, color),
                                );
                            }
                            if x < s || mask.get_pixel(x - s, y)[0] < 128 {
                                painter.line_segment(
                                    [
                                        map(Point::new(x as f32, y as f32)),
                                        map(Point::new(x as f32, (y + s) as f32)),
                                    ],
                                    Stroke::new(1.0_f32, color),
                                );
                            }
                            if y + s >= mask.height() || mask.get_pixel(x, y + s)[0] < 128 {
                                painter.line_segment(
                                    [
                                        map(Point::new(x as f32, (y + s) as f32)),
                                        map(Point::new((x + s) as f32, (y + s) as f32)),
                                    ],
                                    Stroke::new(1.0_f32, color),
                                );
                            }
                            if x + s >= mask.width() || mask.get_pixel(x + s, y)[0] < 128 {
                                painter.line_segment(
                                    [
                                        map(Point::new((x + s) as f32, y as f32)),
                                        map(Point::new((x + s) as f32, (y + s) as f32)),
                                    ],
                                    Stroke::new(1.0_f32, color),
                                );
                            }
                        }
                    }
                }
                let mut hover_handle = None;
                if self.tool == Tool::Move
                    && self.show_controls
                    && let Some(t) = operations::transform_box(&session.document, mask_target)
                {
                    let corners = t.corners().map(map);
                    painter.add(egui::Shape::closed_line(
                        corners.to_vec(),
                        Stroke::new(1.0_f32, Color32::from_gray(225)),
                    ));
                    for (index, unit) in HANDLES.iter().enumerate() {
                        let point = map(t.point(*unit));
                        painter.rect(
                            Rect::from_center_size(point, Vec2::splat(6.0)),
                            0.0,
                            Color32::from_gray(245),
                            Stroke::new(1.0_f32, Color32::from_gray(55)),
                            StrokeKind::Outside,
                        );
                        if response
                            .hover_pos()
                            .is_some_and(|p| p.distance(point) < 8.0)
                        {
                            hover_handle = Some(TransformDrag::Scale(index));
                        }
                    }
                    let top = map(t.point(Point::new(0.5, 0.0)));
                    let center = map(t.center());
                    let rotate = top + (top - center).normalized() * 23.0;
                    painter
                        .line_segment([top, rotate], Stroke::new(1.0_f32, super::theme::ON_CANVAS));
                    painter.circle_filled(rotate, 3.5, super::theme::ON_CANVAS);
                    if response
                        .hover_pos()
                        .is_some_and(|p| p.distance(rotate) < 8.0)
                    {
                        hover_handle = Some(TransformDrag::Rotate);
                    }
                }
                super::guides::paint(&painter, &shown_guides, origin, zoom, viewport);
                // What the current drag snapped to.
                for (axis, coordinate) in &self.snap_lines {
                    let line = match axis {
                        GuideAxis::Vertical => [
                            pos2(origin.x + coordinate * zoom, viewport.top()),
                            pos2(origin.x + coordinate * zoom, viewport.bottom()),
                        ],
                        GuideAxis::Horizontal => [
                            pos2(viewport.left(), origin.y + coordinate * zoom),
                            pos2(viewport.right(), origin.y + coordinate * zoom),
                        ],
                    };
                    painter
                        .line_segment(line, Stroke::new(1.0_f32, Color32::from_rgb(219, 115, 213)));
                }
                if let Some(layout) = &rulers {
                    super::rulers::paint(ui.painter(), layout, origin, zoom);
                }
                if let Some((start, end)) = self.crop_rect {
                    let rect = Rect::from_two_pos(map(start), map(end));
                    painter.rect_stroke(
                        rect,
                        0.0,
                        Stroke::new(1.5_f32, Color32::WHITE),
                        StrokeKind::Inside,
                    );
                    for f in [1.0 / 3.0, 2.0 / 3.0] {
                        painter.line_segment(
                            [
                                pos2(rect.left() + rect.width() * f, rect.top()),
                                pos2(rect.left() + rect.width() * f, rect.bottom()),
                            ],
                            Stroke::new(0.7_f32, Color32::from_white_alpha(140)),
                        );
                        painter.line_segment(
                            [
                                pos2(rect.left(), rect.top() + rect.height() * f),
                                pos2(rect.right(), rect.top() + rect.height() * f),
                            ],
                            Stroke::new(0.7_f32, Color32::from_white_alpha(140)),
                        );
                    }
                }
                if let Some(gesture) = &self.gesture {
                    let rect = Rect::from_two_pos(map(gesture.start), map(gesture.last));
                    if (matches!(self.tool, Tool::Marquee | Tool::Shape)
                        || (self.tool == Tool::Wand && self.wand_object))
                        && !matches!(gesture.kind, TransformDrag::Selection)
                    {
                        if (self.tool == Tool::Marquee && self.ellipse)
                            || (self.tool == Tool::Shape
                                && self.shape_kind == xuan::paint::ShapeKind::Ellipse)
                        {
                            painter.add(egui::epaint::EllipseShape::stroke(
                                rect.center(),
                                rect.size() * 0.5,
                                Stroke::new(1.0_f32, Color32::WHITE),
                            ));
                        } else {
                            painter.rect_stroke(
                                rect,
                                0.0,
                                Stroke::new(1.0_f32, Color32::WHITE),
                                StrokeKind::Inside,
                            );
                        }
                    }
                    if matches!(self.tool, Tool::Lasso | Tool::Heal) && gesture.points.len() > 1 {
                        painter.add(egui::Shape::line(
                            gesture.points.iter().copied().map(map).collect(),
                            Stroke::new(1.0_f32, Color32::WHITE),
                        ));
                    }
                    if self.tool == Tool::Gradient {
                        painter.line_segment(
                            [map(gesture.start), map(gesture.last)],
                            Stroke::new(1.5_f32, Color32::WHITE),
                        );
                        painter.circle_filled(map(gesture.start), 3.0, Color32::WHITE);
                        painter.circle_filled(map(gesture.last), 3.0, Color32::WHITE);
                    }
                    if self.tool == Tool::Region {
                        painter.rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(1.5_f32, ui.palette().accent),
                            StrokeKind::Inside,
                        );
                    }
                }
                if let Some(edit) = &self.plugins.action {
                    for (index, region) in edit.regions.iter().enumerate() {
                        let rect = Rect::from_two_pos(
                            map(Point::new(region.x, region.y)),
                            map(Point::new(
                                region.x + region.width,
                                region.y + region.height,
                            )),
                        );
                        let selected = edit.selected == Some(index);
                        painter.rect_filled(
                            rect,
                            0.0,
                            ui.palette()
                                .accent
                                .gamma_multiply(if selected { 0.2 } else { 0.08 }),
                        );
                        painter.rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(
                                if selected { 2.0_f32 } else { 1.0_f32 },
                                ui.palette().accent,
                            ),
                            StrokeKind::Inside,
                        );
                        let badge = Rect::from_min_size(rect.min, vec2(22.0, 16.0));
                        painter.rect_filled(badge, 0.0, ui.palette().accent);
                        painter.text(
                            badge.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{:02}", index + 1),
                            egui::FontId::monospace(10.0),
                            Color32::WHITE,
                        );
                        if let Some(text) = region
                            .fields
                            .values()
                            .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
                        {
                            painter.text(
                                rect.min + vec2(4.0, 20.0),
                                egui::Align2::LEFT_TOP,
                                text.chars().take(28).collect::<String>(),
                                egui::FontId::proportional(11.0),
                                Color32::WHITE,
                            );
                        }
                    }
                }
                if !self.polygon.is_empty() {
                    let mut points: Vec<_> = self.polygon.iter().copied().map(map).collect();
                    if let Some(p) = response.hover_pos() {
                        points.push(p);
                    }
                    painter.add(egui::Shape::line(
                        points,
                        Stroke::new(1.0_f32, Color32::WHITE),
                    ));
                }
                let blocked = self.job.is_some()
                    || self.develop.is_some()
                    || self.dialog.is_some()
                    || self.error.is_some()
                    || self.close_app
                    || self.close_tab.is_some()
                    || self.rename.is_some();
                if blocked {
                    if self.pen_stroke {
                        self.cancel_gesture();
                    }
                    self.pen_samples.clear();
                    return;
                }
                let pointer = response
                    .interact_pointer_pos()
                    .or_else(|| ctx.input(|i| i.pointer.hover_pos()));
                let doc_point =
                    pointer.map(|p| Point::new((p.x - origin.x) / zoom, (p.y - origin.y) / zoom));
                if self.tool == Tool::Clone {
                    let doc_pointer = pointer
                        .map(|p| Point::new((p.x - origin.x) / zoom, (p.y - origin.y) / zoom));
                    let stroking = self.gesture.is_some();
                    if let Some(sample) = clone_sample_position(
                        self.clone_source,
                        self.clone_offset,
                        doc_pointer,
                        self.clone_aligned,
                        stroking,
                    ) {
                        let centre = map(sample);
                        let shape = self.brush_shape();
                        let outline =
                            brush_outline(&self.brush, false, centre, origin, zoom, shape);
                        paint_brush_outline(&painter, outline, CLONE_SOURCE_COLOR, 90);
                        for arm in [vec2(5.0, 0.0), vec2(0.0, 5.0)] {
                            painter.line_segment(
                                [centre - arm, centre + arm],
                                Stroke::new(2.5_f32, Color32::from_black_alpha(90)),
                            );
                            painter.line_segment(
                                [centre - arm, centre + arm],
                                Stroke::new(1.0_f32, CLONE_SOURCE_COLOR),
                            );
                        }
                    }
                }
                let modifiers = ctx.input(|i| i.modifiers);
                let panning = self.tool == Tool::Hand
                    || ctx.input(|i| i.key_down(egui::Key::Space))
                    || ctx.input(|i| {
                        i.pointer.button_down(egui::PointerButton::Middle)
                            || i.pointer.button_pressed(egui::PointerButton::Middle)
                    });
                if response.hovered() {
                    let session = &mut self.sessions[self.current];
                    if scroll_canvas(
                        ui,
                        &response,
                        viewport.center(),
                        ZoomAnchor::Pointer,
                        &mut session.zoom,
                        &mut session.pan,
                        ZOOM_LIMITS,
                    ) {
                        session.fit = false;
                    }
                    let cursor = if panning {
                        egui::CursorIcon::Grab
                    } else if let Some(cursor) = guide_cursor {
                        cursor
                    } else if hover_handle.is_some() {
                        egui::CursorIcon::ResizeNwSe
                    } else if self.tool == Tool::Move {
                        egui::CursorIcon::Move
                    } else if self.tool == Tool::Text {
                        egui::CursorIcon::Text
                    } else {
                        egui::CursorIcon::Crosshair
                    };
                    ctx.set_cursor_icon(cursor);
                    let pen = self.tablet.as_ref().and_then(|tablet| tablet.sample());
                    if (self.tool.is_brush() || pen.is_some_and(|sample| sample.eraser))
                        && !panning
                        && let Some(p) = pointer
                    {
                        let p = self
                            .gesture
                            .as_ref()
                            .filter(|gesture| gesture.smoothing.is_some())
                            .map_or(p, |gesture| {
                                origin + vec2(gesture.last.x, gesture.last.y) * zoom
                            });
                        let (axis, aspect, pressure) = self.brush_shape();
                        let outline = brush_outline(
                            &self.brush,
                            self.tool == Tool::Pencil,
                            p,
                            origin,
                            zoom,
                            (axis, aspect, pressure),
                        );
                        paint_brush_outline(&painter, outline, Color32::WHITE, 130);
                    }
                }
                let pen_frames = std::mem::take(&mut self.pen_samples);
                let continuing_pan = self.gesture.as_ref().is_some_and(|gesture| gesture.panning);
                let pen_brush = !continuing_pan
                    && (self.pen_stroke
                        || (!panning
                            && (self.tool.is_brush()
                                || pen_frames.iter().any(|sample| sample.eraser))
                            && (self
                                .tablet
                                .as_ref()
                                .and_then(|tablet| tablet.sample())
                                .is_some()
                                || pen_frames
                                    .iter()
                                    .any(|sample| sample.phase != super::tablet::Phase::Leave))));
                if pen_brush {
                    self.paint_pen_samples(&pen_frames, &response, canvas, ctx, modifiers);
                } else if !continuing_pan
                    && (self
                        .gesture
                        .as_ref()
                        .is_some_and(|gesture| gesture.smoothing.is_some())
                        || (!panning && self.tool.is_brush() && self.brush_smoothing > 0.0))
                {
                    self.paint_smoothed_mouse_samples(ctx, &response, canvas, modifiers);
                } else if self.tool == Tool::Dropper && !panning && self.gesture.is_none() {
                    let (pressed, down) =
                        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down()));
                    if self.color_range.is_some() {
                        // Select → Color Range picks its colours instead of the brush's.
                        if pressed
                            && response.hovered()
                            && let Some(point) = doc_point
                        {
                            self.sample_color_range(point, modifiers);
                        }
                    } else if pressed && response.hovered() {
                        if let Some(point) = doc_point {
                            self.dropper_press(point);
                        }
                    } else if self.dropper.is_some() {
                        if down {
                            if let Some(point) = doc_point {
                                self.dropper_update(point);
                            }
                        } else {
                            self.dropper_release();
                        }
                    }
                    let inside = doc_point.is_some_and(|p| {
                        p.x >= 0.0
                            && p.y >= 0.0
                            && p.x < self.sessions[self.current].document.width as f32
                            && p.y < self.sessions[self.current].document.height as f32
                    });
                    if let (Some(screen), Some(point)) = (pointer, doc_point)
                        && (response.hovered() || self.dropper.is_some())
                    {
                        let new = if self.dropper.is_some() {
                            Some(self.brush.color)
                        } else if inside {
                            self.sample_color(point)
                        } else {
                            None
                        };
                        if let Some(new) = new {
                            let current = self.dropper.map_or(self.brush.color, |g| g.previous);
                            let rect = super::eyedropper::bubble_rect(
                                screen,
                                super::eyedropper::BUBBLE_SIZE,
                                viewport,
                            );
                            super::eyedropper::paint_bubble(&painter, rect, new, current);
                        }
                    }
                } else if self.guide_drag.is_none() {
                    let started = response.drag_started()
                        || response.drag_started_by(egui::PointerButton::Middle);
                    if started {
                        if let (Some(screen), Some(point)) = (pointer, doc_point) {
                            let press = ctx.input(|i| i.pointer.press_origin()).unwrap_or(screen);
                            let start = Point::new(
                                (press.x - origin.x) / zoom,
                                (press.y - origin.y) / zoom,
                            );
                            self.begin_gesture(start, press, panning, hover_handle, modifiers);
                            self.update_gesture(point, screen, modifiers);
                        }
                    } else if self.gesture.is_some()
                        && ctx.input(|i| i.pointer.any_down())
                        && let (Some(screen), Some(point)) = (pointer, doc_point)
                    {
                        self.update_gesture(point, screen, modifiers);
                    }
                    if self.gesture.is_some() && !ctx.input(|i| i.pointer.any_down()) {
                        self.end_gesture(modifiers);
                    }
                    if response.clicked()
                        && !panning
                        && hover_handle.is_none()
                        && let Some(point) = doc_point
                    {
                        self.canvas_click(point, modifiers);
                    }
                }
                if response.double_clicked() && self.tool == Tool::Lasso && self.polygonal {
                    self.finish_polygon();
                }
                if response.double_clicked()
                    && self.tool == Tool::Move
                    && !panning
                    && let Some(point) = doc_point
                    && let Some(id) =
                        render::hit_test_bounds(&self.sessions[self.current].document, point)
                    && self.sessions[self.current]
                        .document
                        .layers
                        .iter()
                        .any(|l| l.id == id && l.raw.is_some())
                {
                    self.start_develop_layer(id);
                }
                if !ctx.input(|i| i.raw.hovered_files.is_empty()) {
                    painter.rect_stroke(
                        viewport.shrink(5.0),
                        8.0,
                        Stroke::new(2.0_f32, ui.palette().accent),
                        StrokeKind::Inside,
                    );
                }
                // The Navigator is drawn before the canvas, so it saw last frame's view.
                // A collapsed or hidden Navigator draws nothing and needs no repaint.
                if let Some(session) = self.session()
                    && self.pane_open(xuan::panes::NAVIGATOR)
                    && self.navigator_view != Some((viewport, session.zoom, session.pan))
                {
                    ctx.request_repaint();
                }
            });
    }

    fn welcome(&mut self, ui: &mut egui::Ui, viewport: Rect) {
        let width = 500.0_f32.min(viewport.width() - 40.0);
        let rect = Rect::from_center_size(viewport.center(), vec2(width, 260.0));
        let mut create = false;
        let mut open = false;
        let mut import = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.heading(tr("New canvas"));
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(tr("A blank space for your next composition."))
                    .size(14.0)
                    .color(ui.palette().muted),
            );
            ui.add_space(24.0);
            egui::Grid::new("welcome_dimensions")
                .num_columns(3)
                .min_col_width(0.0)
                .min_row_height(0.0)
                .show(ui, |ui| {
                    ui.label(tr("Width"));
                    ui.label("");
                    ui.label(tr("Height"));
                    ui.end_row();

                    ui.add(
                        widgets::Number::new(&mut self.dimensions[0])
                            .size(vec2(180.0, 36.0))
                            .range(1..=30_000)
                            .suffix(" px"),
                    );
                    ui.label("×");
                    ui.add(
                        widgets::Number::new(&mut self.dimensions[1])
                            .size(vec2(180.0, 36.0))
                            .range(1..=30_000)
                            .suffix(" px"),
                    );
                    ui.end_row();
                });
            ui.add_space(15.0);
            ui.label(
                egui::RichText::new(tr("Transparent canvas · sRGB")).color(ui.palette().muted),
            );
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                open = widgets::button(ui, tr("Open project")).clicked();
                import = widgets::button(ui, tr("Import image")).clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    create = ui
                        .add(widgets::Button::new(tr("Create canvas")).primary())
                        .clicked();
                });
            });
        });
        if create {
            self.new_document();
        }
        if open {
            self.open_dialog(false);
        }
        if import {
            self.open_dialog(false);
        }
    }

    fn paint_smoothed_mouse_samples(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        canvas: Rect,
        modifiers: egui::Modifiers,
    ) {
        let zoom = self.session().unwrap().zoom;
        let events = ctx.input(|input| input.events.clone());
        // Process contact as well as motion: a complete stroke can arrive between
        // GUI frames, before egui has had a chance to recognize a drag.
        for event in events {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                } if response.rect.contains(pos)
                    && ctx.layer_id_at(pos) == Some(response.layer_id) =>
                {
                    let point =
                        Point::new((pos.x - canvas.min.x) / zoom, (pos.y - canvas.min.y) / zoom);
                    let from = if modifiers.shift {
                        self.last_brush.unwrap_or(point)
                    } else {
                        point
                    };
                    self.begin_gesture(from, pos, false, None, modifiers);
                    self.update_gesture(point, pos, modifiers);
                }
                egui::Event::PointerMoved(pos)
                | egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } => {
                    let point =
                        Point::new((pos.x - canvas.min.x) / zoom, (pos.y - canvas.min.y) / zoom);
                    if self.gesture.as_ref().is_some_and(|gesture| {
                        gesture
                            .smoothing
                            .as_ref()
                            .map_or(gesture.last, |smoothing| smoothing.input)
                            != point
                    }) {
                        self.update_gesture(point, pos, modifiers);
                    }
                    if matches!(event, egui::Event::PointerButton { pressed: false, .. }) {
                        self.end_gesture(modifiers);
                    }
                }
                egui::Event::PointerGone | egui::Event::WindowFocused(false) => {
                    self.end_gesture(modifiers);
                }
                _ => {}
            }
        }
        if self.gesture.is_some() && !ctx.input(|input| input.pointer.primary_down()) {
            self.end_gesture(modifiers);
        }
    }

    fn paint_pen_samples(
        &mut self,
        samples: &[super::tablet::Sample],
        response: &egui::Response,
        canvas: Rect,
        ctx: &egui::Context,
        modifiers: egui::Modifiers,
    ) {
        use super::tablet::Phase;
        let origin = canvas.min;
        let zoom = self.session().unwrap().zoom;
        for sample in samples {
            let point = Point::new(
                (sample.position.x - origin.x) / zoom,
                (sample.position.y - origin.y) / zoom,
            );
            self.pen_sample = Some(*sample);
            match sample.phase {
                Phase::Down
                    if response.rect.contains(sample.position)
                        && ctx.layer_id_at(sample.position) == Some(response.layer_id) =>
                {
                    let from = if modifiers.shift {
                        self.last_brush.unwrap_or(point)
                    } else {
                        point
                    };
                    self.begin_gesture(from, sample.position, false, None, modifiers);
                    self.pen_stroke = self.gesture.is_some();
                    if self.pen_stroke {
                        self.update_gesture(point, sample.position, modifiers);
                    }
                }
                Phase::Move if self.pen_stroke => {
                    self.update_gesture(point, sample.position, modifiers)
                }
                Phase::Up | Phase::Leave if self.pen_stroke => {
                    // A zero pressure Up is a release, not an extra transparent dab.
                    // Include its final position using the preceding brush state.
                    if sample.phase == Phase::Up
                        && self.gesture.as_ref().is_some_and(|gesture| {
                            gesture
                                .smoothing
                                .as_ref()
                                .map_or(gesture.last, |smoothing| smoothing.input)
                                .distance(point)
                                > 0.001
                        })
                    {
                        self.pen_sample = Some(super::tablet::Sample {
                            pressure: None,
                            tilt: self.gesture.as_ref().map(|gesture| gesture.brush.tilt),
                            ..*sample
                        });
                        let brush = self.brush.clone();
                        if let Some(gesture) = &self.gesture {
                            self.brush = gesture.brush.clone();
                        }
                        self.update_gesture(point, sample.position, modifiers);
                        self.brush = brush;
                    }
                    self.end_gesture(modifiers);
                    self.pen_stroke = false;
                }
                _ => {}
            }
        }
        self.pen_sample = None;
    }

    fn canvas_click(&mut self, point: Point, modifiers: egui::Modifiers) {
        match self.tool {
            Tool::Text => self.text_click(point),
            Tool::Region => self.select_region_at(point),
            Tool::Move => {
                if self.auto_select || modifiers.ctrl {
                    self.select_canvas_layer(point, modifiers.shift, false);
                }
            }
            Tool::Wand if self.wand_object => {
                let mode = self.selection_mode(modifiers);
                self.select_object(
                    xuan::segment::Seeds {
                        points: vec![(point.x, point.y)],
                        rect: None,
                    },
                    mode,
                );
            }
            Tool::Wand => {
                let tolerance = self.tolerance;
                let contiguous = self.contiguous;
                let mode = self.selection_mode(modifiers);
                self.edit_selection(tr("Magic Wand"), |doc| {
                    let pixels = render::render(doc);
                    selection::combine(
                        doc,
                        selection::wand(&pixels, point, tolerance, contiguous),
                        mode,
                    );
                });
            }
            Tool::Zoom => {
                if let Some(s) = self.session_mut() {
                    s.zoom = (s.zoom * if modifiers.alt { 0.8 } else { 1.25 })
                        .clamp(*ZOOM_LIMITS.start(), *ZOOM_LIMITS.end());
                }
            }
            Tool::Lasso if self.polygonal => {
                if self.polygon.len() > 2
                    && self.polygon[0].distance(point) * self.session().unwrap().zoom < 8.0
                {
                    self.finish_polygon();
                } else {
                    self.polygon.push(point);
                }
            }
            Tool::Clone if modifiers.alt => {
                self.clone_source = Some(point);
                self.clone_offset = None;
            }
            tool if tool.is_brush() => {
                let from = if modifiers.shift {
                    self.last_brush.unwrap_or(point)
                } else {
                    point
                };
                self.begin_gesture(from, Pos2::ZERO, false, None, modifiers);
                self.update_gesture(point, Pos2::ZERO, modifiers);
                self.end_gesture(modifiers);
            }
            _ => {}
        }
    }

    fn select_canvas_layer(&mut self, point: Point, extend: bool, dragging: bool) -> bool {
        let ignore_transparent_pixels = self.ignore_transparent_pixels;
        let Some(session) = self.session_mut() else {
            return false;
        };
        let document = &mut session.document;
        let inside = point.x >= 0.0
            && point.y >= 0.0
            && point.x < document.width as f32
            && point.y < document.height as f32;
        let hit = inside
            .then(|| {
                if ignore_transparent_pixels {
                    render::hit_test(document, point)
                } else {
                    render::hit_test_bounds(document, point)
                }
            })
            .flatten();
        if let Some(id) = hit {
            // Moving an already selected layer keeps the other selected layers and mask target.
            if !dragging || !document.transform_targets().contains(&id) {
                document.select(id, extend);
                self.mask_target = false;
            }
        } else if !extend {
            document.selected.clear();
            document.active = None;
            self.mask_target = false;
        }
        hit.is_some()
    }

    fn selection_mode(&self, modifiers: egui::Modifiers) -> SelectionMode {
        if modifiers.shift {
            SelectionMode::Add
        } else if modifiers.alt {
            SelectionMode::Subtract
        } else {
            self.selection_mode
        }
    }

    pub(super) fn finish_polygon(&mut self) {
        if self.polygon.len() < 3 {
            return;
        }
        let points = std::mem::take(&mut self.polygon);
        let mode = self.selection_mode;
        self.edit_selection(tr("Polygonal Lasso"), |doc| {
            selection::combine(
                doc,
                selection::polygon(doc.width, doc.height, &points),
                mode,
            );
        });
    }

    fn begin_gesture(
        &mut self,
        point: Point,
        screen: Pos2,
        panning: bool,
        mut handle: Option<TransformDrag>,
        modifiers: egui::Modifiers,
    ) {
        let tool = if self.pen_sample.is_some_and(|sample| sample.eraser) {
            Tool::Erase
        } else {
            self.tool
        };
        let brush = self.input_brush();
        if self.gesture.is_some() || self.sessions.is_empty() {
            return;
        }
        if panning {
            // View navigation must not start an edit or run tool-specific setup.
            let session = &self.sessions[self.current];
            self.gesture = Some(Gesture {
                tool,
                brush: brush.clone(),
                brushes: vec![brush],
                stroke: paint::Stroke::default(),
                smoothing: None,
                start: point,
                last: point,
                screen_start: screen,
                pan_start: session.pan,
                points: Vec::new(),
                original: session.document.clone(),
                kind: TransformDrag::Move,
                panning: true,
                clone_offset: Point::default(),
                source: None,
                reference: None,
                selection_bounds: None,
            });
            return;
        }
        if tool == Tool::Text {
            return;
        }
        if tool == Tool::Region {
            // Regions belong to the open plugin action, not to the document.
            if self.plugins.action.is_none() {
                return;
            }
            let session = &self.sessions[self.current];
            self.gesture = Some(Gesture {
                tool,
                brush: brush.clone(),
                brushes: vec![brush],
                stroke: paint::Stroke::default(),
                smoothing: None,
                start: point,
                last: point,
                screen_start: screen,
                pan_start: session.pan,
                points: Vec::new(),
                original: session.document.clone(),
                kind: TransformDrag::Move,
                panning: false,
                clone_offset: Point::default(),
                source: None,
                reference: None,
                selection_bounds: None,
            });
            return;
        }
        if tool == Tool::Clone && modifiers.alt {
            self.clone_source = Some(point);
            self.clone_offset = None;
            return;
        }
        if tool == Tool::Heal && self.editing_mask() {
            // As upstream: Spot Healing reworks image pixels and has nothing to do on a mask.
            self.status = tr("Spot Healing works on layer pixels, not masks").into();
            return;
        }
        if tool == Tool::Clone && self.clone_source.is_none() {
            self.status = tr("Alt-click on the canvas to set a clone source").into();
            return;
        }
        if tool == Tool::Move && self.show_controls {
            // The press location determines the handle, even if the pointer has moved since.
            handle = None;
            let session = &self.sessions[self.current];
            if let Some(t) = operations::transform_box(&session.document, self.transforming_mask())
            {
                if let Some(index) = HANDLES
                    .iter()
                    .position(|unit| t.point(*unit).distance(point) * session.zoom < 9.0)
                {
                    handle = Some(if modifiers.ctrl && index % 2 == 0 {
                        TransformDrag::Distort(index / 2)
                    } else {
                        TransformDrag::Scale(index)
                    });
                } else {
                    let top = t.point(Point::new(0.5, 0.0));
                    let center = t.center();
                    let distance = top.distance(center).max(0.01);
                    let rotate = Point::new(
                        top.x + (top.x - center.x) / distance * 23.0 / session.zoom,
                        top.y + (top.y - center.y) / distance * 23.0 / session.zoom,
                    );
                    if rotate.distance(point) * session.zoom < 9.0 {
                        handle = Some(TransformDrag::Rotate);
                    }
                }
            }
        }
        let mut kind = handle.unwrap_or(TransformDrag::Move);
        if tool == Tool::Move
            && (self.auto_select || modifiers.ctrl)
            && handle.is_none()
            && !self.select_canvas_layer(point, modifiers.shift, true)
        {
            return;
        }
        let mask_target = self.transforming_mask();
        // A marquee, shape or crop starts on a nearby Snap To target; Ctrl starts it freely.
        let snapping = self
            .snap_options()
            .filter(|_| !modifiers.ctrl && matches!(tool, Tool::Marquee | Tool::Shape | Tool::Crop))
            .map(|options| (options, self.displayed_guides()));
        let session = &mut self.sessions[self.current];
        if tool == Tool::Move && session.document.active.is_none() {
            return;
        }
        session.history.begin(tool.label(), &session.document);
        if tool == Tool::Move && modifiers.alt {
            operations::duplicate(&mut session.document);
        }
        if tool.is_selection()
            && !modifiers.shift
            && (!modifiers.alt || modifiers.ctrl)
            && session
                .document
                .selection
                .as_ref()
                .is_some_and(|m| selection::coverage(Some(m), point) > 0.0)
        {
            if modifiers.ctrl {
                if let Some((pixels, origin)) = operations::copy_pixels(&session.document, false) {
                    if !modifiers.alt
                        && let Err(error) = paint::fill(&mut session.document, [0; 4], true, false)
                    {
                        session.history.cancel(&mut session.document);
                        self.error = Some(error.to_string());
                        return;
                    }
                    let mut layer = xuan::document::Layer::image(tr("Selection"), pixels);
                    layer.transform.x = origin.x;
                    layer.transform.y = origin.y;
                    session.document.insert(layer);
                    session.document.selection = None;
                    kind = TransformDrag::Pixels;
                }
            } else {
                kind = TransformDrag::Selection;
            }
        }
        let source = if matches!(tool, Tool::Clone | Tool::Blur) {
            if tool == Tool::Clone && self.clone_all {
                Some(Arc::new(render::render(&session.document)))
            } else {
                let mut isolated = session.document.clone();
                let id = isolated.active;
                for l in &mut isolated.layers {
                    l.visible = Some(l.id) == id || l.group;
                }
                Some(Arc::new(render::render(&isolated)))
            }
        } else {
            None
        };
        let offset = if self.clone_aligned {
            self.clone_offset
        } else {
            None
        }
        .unwrap_or_else(|| {
            self.clone_source.map_or(Point::default(), |p| {
                Point::new(p.x - point.x, p.y - point.y)
            })
        });
        if tool == Tool::Clone {
            self.clone_offset = Some(offset);
        }
        self.snap_lines.clear();
        let mut point = point;
        if matches!(kind, TransformDrag::Move)
            && let Some((options, guides)) = &snapping
        {
            let targets = super::snap::SnapTargets::new(
                &session.document,
                guides,
                options,
                &Default::default(),
                false,
            );
            (point, self.snap_lines) =
                targets.snap_point(point, super::snap::tolerance(session.zoom));
        }
        let selection_bounds = matches!(kind, TransformDrag::Selection)
            .then(|| {
                session
                    .document
                    .selection
                    .as_deref()
                    .and_then(selection::bounds)
            })
            .flatten()
            .map(|(x0, y0, x1, y1)| {
                [
                    Point::new(x0 as f32, y0 as f32),
                    Point::new(x1 as f32, y1 as f32),
                ]
            });
        self.gesture = Some(Gesture {
            tool,
            brush: brush.clone(),
            brushes: vec![brush],
            stroke: paint::Stroke::default(),
            smoothing: (tool.is_brush() && self.brush_smoothing > 0.0 && !modifiers.shift).then(
                || {
                    super::stroke_smoothing::StrokeSmoother::new(
                        point,
                        self.brush_smoothing,
                        session.zoom,
                    )
                },
            ),
            start: point,
            last: point,
            screen_start: screen,
            pan_start: session.pan,
            points: vec![point],
            original: session.document.clone(),
            kind,
            panning: false,
            clone_offset: offset,
            source,
            reference: operations::transform_box(&session.document, mask_target),
            selection_bounds,
        });
    }

    fn update_gesture(&mut self, point: Point, screen: Pos2, modifiers: egui::Modifiers) {
        let brush = self.input_brush();
        self.update_gesture_with_brush(point, screen, modifiers, brush);
    }

    fn update_gesture_with_brush(
        &mut self,
        mut point: Point,
        screen: Pos2,
        modifiers: egui::Modifiers,
        brush: paint::Brush,
    ) {
        let Some(mut gesture) = self.gesture.take() else {
            return;
        };
        let tool = gesture.tool;
        if gesture.panning {
            self.sessions[self.current].pan = gesture.pan_start + (screen - gesture.screen_start);
            self.gesture = Some(gesture);
            return;
        }
        if let Some(smoothing) = &mut gesture.smoothing {
            point = smoothing.update(point);
        }
        let shaped = matches!(tool, Tool::Shape | Tool::Marquee | Tool::Crop)
            && matches!(gesture.kind, TransformDrag::Move);
        let moving = tool == Tool::Move
            || matches!(
                gesture.kind,
                TransformDrag::Selection | TransformDrag::Pixels
            );
        // View → Snap To, unless Ctrl is held.
        let snapping = if shaped || moving {
            self.snap_lines.clear();
            self.snap_options()
                .filter(|_| !modifiers.ctrl)
                .map(|options| (options, self.displayed_guides()))
        } else {
            None
        };
        let tolerance = super::snap::tolerance(self.sessions[self.current].zoom);
        if shaped && let Some((options, guides)) = &snapping {
            let targets = super::snap::SnapTargets::new(
                &gesture.original,
                guides,
                options,
                &Default::default(),
                false,
            );
            (point, self.snap_lines) = targets.snap_point(point, tolerance);
        }
        if modifiers.shift && matches!(tool, Tool::Shape | Tool::Marquee | Tool::Crop) {
            let dx = point.x - gesture.start.x;
            let dy = point.y - gesture.start.y;
            let size = dx.abs().max(dy.abs());
            point = Point::new(
                gesture.start.x + size * dx.signum(),
                gesture.start.y + size * dy.signum(),
            );
        }
        let mask_target = self.editing_mask();
        let transform_mask = self.transforming_mask();
        let session = &mut self.sessions[self.current];
        let result = if matches!(gesture.kind, TransformDrag::Selection) && tool.is_selection() {
            if let Some(mask) = &gesture.original.selection {
                let mut offset = Point::new(
                    (point.x - gesture.start.x).round(),
                    (point.y - gesture.start.y).round(),
                );
                // The outline's edges or middle meet targets, as a drawn marquee's corner does.
                if let (Some((options, guides)), Some(bounds)) =
                    (&snapping, gesture.selection_bounds)
                {
                    let targets = super::snap::SnapTargets::new(
                        &gesture.original,
                        guides,
                        options,
                        &Default::default(),
                        false,
                    );
                    let snapped;
                    (snapped, self.snap_lines) =
                        targets.snap_box(bounds, offset, [false; 2], tolerance);
                    offset = Point::new(snapped.x.round(), snapped.y.round());
                }
                session.document.selection = Some(Arc::new(selection::translate(
                    mask,
                    offset.x as i32,
                    offset.y as i32,
                )));
            }
            Ok(())
        } else {
            match tool {
                Tool::Heal => {
                    gesture.points.push(point);
                    gesture.brushes.push(brush.clone());
                    Ok(())
                }
                tool if tool.is_brush() => {
                    let mode = match tool {
                        Tool::Erase => PaintMode::Erase,
                        Tool::Pencil => PaintMode::Pencil,
                        Tool::Clone => PaintMode::Clone,
                        Tool::Heal => PaintMode::Heal,
                        Tool::Blur => self.blur_mode,
                        _ => PaintMode::Paint,
                    };
                    let offset = if mode == PaintMode::Smudge {
                        Point::new(gesture.last.x - point.x, gesture.last.y - point.y)
                    } else {
                        gesture.clone_offset
                    };
                    gesture.stroke.segment(
                        &mut session.document,
                        gesture.last,
                        point,
                        &gesture.brush,
                        &brush,
                        paint::StrokeOptions {
                            mode,
                            mask_target,
                            source: gesture.source.as_deref(),
                            clone_offset: offset,
                        },
                    )
                }
                _ if tool == Tool::Move || matches!(gesture.kind, TransformDrag::Pixels) => {
                    let mut dx = point.x - gesture.start.x;
                    let mut dy = point.y - gesture.start.y;
                    let mut lock = [false; 2];
                    if modifiers.shift && matches!(gesture.kind, TransformDrag::Move) {
                        if dx.abs() > dy.abs() {
                            dy = 0.0;
                            lock[1] = true;
                        } else {
                            dx = 0.0;
                            lock[0] = true;
                        }
                    }
                    let targets = if transform_mask {
                        gesture.original.active.into_iter().collect()
                    } else {
                        gesture.original.movement_targets()
                    };
                    if let Some((options, guides)) = &snapping
                        && let Some(t) = gesture.reference
                    {
                        let snap_targets = super::snap::SnapTargets::new(
                            &gesture.original,
                            guides,
                            options,
                            &targets,
                            true,
                        );
                        match gesture.kind {
                            TransformDrag::Move | TransformDrag::Pixels => {
                                let offset;
                                (offset, self.snap_lines) = snap_targets.snap_box(
                                    super::snap::bounds(t),
                                    Point::new(dx, dy),
                                    lock,
                                    tolerance,
                                );
                                (dx, dy) = (offset.x, offset.y);
                            }
                            // A turned or distorted box's edges don't run along the targets.
                            TransformDrag::Scale(index)
                                if t.rotation == 0.0 && t.warp.is_none() =>
                            {
                                let (start, kind, lock_ratio) =
                                    (gesture.start, gesture.kind, self.lock_ratio);
                                let snapped;
                                (snapped, self.snap_lines) = snap_targets.snap_resize(
                                    Point::new(start.x + dx, start.y + dy),
                                    start,
                                    HANDLES[index],
                                    t.point(HANDLES[index]),
                                    lock_ratio != modifiers.shift,
                                    tolerance,
                                    |p| {
                                        drag_transform(
                                            t,
                                            start,
                                            p,
                                            kind,
                                            lock_ratio,
                                            modifiers.shift,
                                        )
                                    },
                                );
                                (dx, dy) = (snapped.x - start.x, snapped.y - start.y);
                            }
                            _ => {}
                        }
                    }
                    if let Some(reference) = gesture.reference {
                        let moved = Point::new(gesture.start.x + dx, gesture.start.y + dy);
                        let transformed = drag_transform(
                            reference,
                            gesture.start,
                            moved,
                            gesture.kind,
                            self.lock_ratio,
                            modifiers.shift,
                        );
                        if transformed.valid() {
                            for layer in &mut session.document.layers {
                                if !targets.contains(&layer.id) || layer.locked {
                                    continue;
                                }
                                let Some(original) =
                                    gesture.original.layers.iter().find(|l| l.id == layer.id)
                                else {
                                    continue;
                                };
                                let old = if transform_mask {
                                    original
                                        .mask
                                        .as_ref()
                                        .and_then(|m| m.placement)
                                        .unwrap_or(original.transform)
                                } else {
                                    original.transform
                                };
                                let transform = if targets.len() == 1 {
                                    transformed
                                } else {
                                    old.following(reference, transformed)
                                };
                                if transform_mask {
                                    if let Some(mask) = &mut layer.mask {
                                        mask.placement = Some(transform);
                                        mask.linked = false;
                                    }
                                } else {
                                    layer.transform = original.transform;
                                    layer.mask = original.mask.clone();
                                    layer.set_transform(transform);
                                }
                            }
                        }
                    }
                    Ok(())
                }
                Tool::Lasso => {
                    if !self.polygonal {
                        gesture.points.push(point);
                    }
                    Ok(())
                }
                Tool::Crop => {
                    self.crop_rect = Some((gesture.start, point));
                    Ok(())
                }
                _ => Ok(()),
            }
        };
        if let Err(error) = result {
            session.history.cancel(&mut session.document);
            self.error = Some(error.to_string());
            session.invalidate();
            return;
        }
        gesture.last = point;
        gesture.brush = brush;
        if gesture.changes_composition(tool) && !matches!(tool, Tool::Gradient | Tool::Shape) {
            session.invalidate();
        }
        self.gesture = Some(gesture);
    }

    fn end_gesture(&mut self, modifiers: egui::Modifiers) {
        let tail = self.gesture.as_mut().and_then(|gesture| {
            let smoothing = gesture.smoothing.take()?;
            (gesture.last.distance(smoothing.input) > 0.001)
                .then(|| (smoothing.input, gesture.brush.clone()))
        });
        if let Some((point, brush)) = tail {
            // Finish at the last input position using the last contact pressure
            // and tilt. The tail belongs to the same history transaction.
            self.update_gesture_with_brush(point, Pos2::ZERO, modifiers, brush);
        }
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        self.snap_lines.clear();
        let tool = gesture.tool;
        if gesture.panning {
            return;
        }
        if tool == Tool::Region {
            self.add_region(gesture.start, gesture.last);
            return;
        }
        if tool == Tool::Wand
            && self.wand_object
            && !matches!(
                gesture.kind,
                TransformDrag::Selection | TransformDrag::Pixels
            )
        {
            // Object mode: the dragged rectangle bounds the object.
            let (a, b) = (gesture.start, gesture.last);
            let rect = [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)];
            if rect[2] - rect[0] >= 2.0 && rect[3] - rect[1] >= 2.0 {
                let mode = self.selection_mode(modifiers);
                self.select_object(
                    xuan::segment::Seeds {
                        points: Vec::new(),
                        rect: Some(rect),
                    },
                    mode,
                );
            }
            return;
        }
        if tool == Tool::Heal {
            let points = gesture.points;
            let brushes = gesture.brushes;
            let mode = self.heal_mode;
            self.start_job(tr("Spot Healing"), move |document, cancel| {
                xuan::retouch::heal_path_varying(document, &points, &brushes, mode, cancel)
            });
            return;
        }
        let mode = self.selection_mode(modifiers);
        let mask_target = self.editing_mask();
        let session = &mut self.sessions[self.current];
        let start = gesture.start;
        let end = gesture.last;
        let changes_composition = gesture.changes_composition(tool);
        let result = if matches!(
            gesture.kind,
            TransformDrag::Selection | TransformDrag::Pixels
        ) && tool.is_selection()
        {
            Ok(())
        } else {
            match tool {
                Tool::Marquee => {
                    selection::combine(
                        &mut session.document,
                        selection::rectangle(
                            gesture.original.width,
                            gesture.original.height,
                            start,
                            end,
                            self.ellipse,
                        ),
                        mode,
                    );
                    Ok(())
                }
                Tool::Lasso if !self.polygonal => {
                    selection::combine(
                        &mut session.document,
                        selection::polygon(
                            gesture.original.width,
                            gesture.original.height,
                            &gesture.points,
                        ),
                        mode,
                    );
                    Ok(())
                }
                Tool::Gradient => paint::gradient(
                    &mut session.document,
                    start,
                    end,
                    paint::GradientOptions {
                        foreground: self.brush.color,
                        background: self.background,
                        radial: self.radial,
                        opacity: self.brush.opacity,
                        mask_target,
                    },
                ),
                Tool::Shape => {
                    let start = if modifiers.alt {
                        Point::new(start.x - (end.x - start.x), start.y - (end.y - start.y))
                    } else {
                        start
                    };
                    paint::shape(
                        start,
                        end,
                        self.shape_kind,
                        self.brush.color,
                        self.corner_radius,
                    )
                    .map(|layer| session.document.insert(layer))
                }
                Tool::Crop => {
                    session.history.cancel(&mut session.document);
                    return;
                }
                _ => Ok(()),
            }
        };
        if !changes_composition && result.is_ok() {
            session.history.commit();
            return;
        }
        match result {
            Ok(()) => match paint::refresh_shapes(&mut session.document) {
                Ok(()) => session.history.commit(),
                Err(error) => {
                    session.history.cancel(&mut session.document);
                    self.error = Some(error.to_string());
                }
            },
            Err(error) => {
                session.history.cancel(&mut session.document);
                self.error = Some(error.to_string());
            }
        }
        session.invalidate();
        self.snap_lines.clear();
        if tool.is_brush() {
            self.last_brush = Some(end);
        }
    }
}

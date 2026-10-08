//! The Crop tool: a box drawn on the canvas, adjusted by its handles or moved before it is applied,
//! the Ratio it keeps, and the options bar that applies or cancels it. The box's arithmetic is
//! in `xuan::crop`; this draws it and wires it to the pointer and the bar.
//!
//! Its Perspective mode (Shift+C) places four corners instead, on something photographed at an
//! angle, and straightens them into an upright canvas (`xuan::crop::perspective`).
use super::theme::PaletteExt as _;
use super::widgets;
use egui::{Color32, CursorIcon, Painter, Pos2, Rect, RichText, Stroke, StrokeKind, pos2, vec2};
use xuan::{
    crop::{
        self, CropBox, Handle, Hit, Ratio,
        perspective::{self, Quad},
    },
    document::Point,
    i18n::tr,
};

use super::EditorApp;

/// How close, in points, a press must come to a handle to grab it, as with the Move tool.
const HANDLE_GRAB: f32 = 9.0;

/// How many parts each way the grid over the perspective corners divides the straightened canvas.
const GRID_DIVISIONS: u32 = 4;

/// What a drag on the canvas does to the box, or to the perspective corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CropDrag {
    /// Draw a new box from the press.
    Draw,
    /// Move the box that was there.
    Move(CropBox),
    /// Drag one of its handles.
    Resize(Handle, CropBox),
    /// Drag one perspective corner, from the corners that were there.
    Corner(usize, Quad),
    /// Move all four corners.
    MoveQuad(Quad),
}

/// The Crop tool's state. The ratio and custom terms stay chosen while Xuan runs.
#[derive(Debug)]
pub(super) struct CropTool {
    pub ratio: Ratio,
    /// The W : H of the Custom ratio.
    pub custom: [u32; 2],
    /// The box waiting to be applied, in canvas pixels.
    pub rect: Option<CropBox>,
    pub drag: Option<CropDrag>,
    /// The document the box belongs to; another tab never sees it.
    pub document: Option<uuid::Uuid>,
    /// Perspective mode: four corners instead of a box. Stays chosen while Xuan runs.
    pub perspective: bool,
    /// The perspective corners waiting to be applied, in canvas pixels.
    pub quad: Option<Quad>,
    /// Corners clicked so far while placing a new quad by clicks.
    pub placing: Vec<Point>,
    /// The straightened canvas's size as typed; `None` works it out from the corners.
    pub size: Option<[u32; 2]>,
}

impl Default for CropTool {
    fn default() -> Self {
        Self {
            ratio: Ratio::Free,
            custom: [1, 1],
            rect: None,
            drag: None,
            document: None,
            perspective: false,
            quad: None,
            placing: Vec::new(),
            size: None,
        }
    }
}

impl CropTool {
    /// The terms the box keeps on a canvas of this size; Shift keeps a square while drawing
    /// freely, as with the marquee.
    fn terms(&self, canvas: [u32; 2], shift: bool) -> Option<[u32; 2]> {
        self.ratio
            .terms(canvas, self.custom)
            .or_else(|| shift.then_some([1, 1]))
    }

    /// What a press at `point` grabs: a handle or the inside of the box, else a new box.
    pub(super) fn press(&self, point: Point, zoom: f32) -> CropDrag {
        let Some(rect) = self.rect.filter(|r| !r.is_empty()) else {
            return CropDrag::Draw;
        };
        match rect.hit(point, HANDLE_GRAB / zoom) {
            Some(Hit::Handle(handle)) => CropDrag::Resize(handle, rect),
            Some(Hit::Inside) => CropDrag::Move(rect),
            None => CropDrag::Draw,
        }
    }

    /// What a press at `point` grabs in Perspective mode: a corner or the inside, else nothing.
    pub(super) fn press_quad(&self, point: Point, zoom: f32) -> Option<CropDrag> {
        let quad = self.quad?;
        perspective::corner_at(quad, point, HANDLE_GRAB / zoom)
            .map(|corner| CropDrag::Corner(corner, quad))
            .or_else(|| perspective::contains(quad, point).then_some(CropDrag::MoveQuad(quad)))
    }

    /// Places the next of four corners by a click; the fourth makes the quad.
    pub(super) fn place_corner(&mut self, point: Point) {
        self.placing.push(point);
        if let Ok(points) = <[Point; 4]>::try_from(self.placing.as_slice()) {
            self.quad = Some(perspective::order(points));
            self.placing.clear();
        }
    }

    /// The straightened canvas's size: as typed, else from the corners.
    pub(super) fn output_size(&self) -> Option<[u32; 2]> {
        self.size
            .or_else(|| self.quad.map(perspective::output_size))
    }

    /// Follows the pointer from `start` to `point`.
    pub(super) fn drag_to(&mut self, start: Point, point: Point, canvas: [u32; 2], shift: bool) {
        let delta = Point::new(point.x - start.x, point.y - start.y);
        if self.perspective {
            match self.drag {
                Some(CropDrag::Corner(corner, mut quad)) => {
                    quad[corner] = Point::new(quad[corner].x + delta.x, quad[corner].y + delta.y);
                    self.quad = Some(quad);
                }
                Some(CropDrag::MoveQuad(quad)) => self.quad = Some(perspective::moved(quad, delta)),
                _ => {}
            }
            return;
        }
        let terms = self.terms(canvas, shift);
        self.rect = Some(match self.drag.unwrap_or(CropDrag::Draw) {
            CropDrag::Draw => crop::draw(start, point, terms, canvas),
            CropDrag::Move(original) => crop::moved(original, delta, canvas),
            CropDrag::Resize(handle, original) => {
                crop::resize(original, handle, delta, terms, canvas)
            }
            CropDrag::Corner(..) | CropDrag::MoveQuad(_) => return,
        });
    }

    /// The drag is over; a box with no area is dropped.
    pub(super) fn release(&mut self) {
        self.drag = None;
        self.rect = self.rect.filter(|r| !r.is_empty());
    }
}

/// The cursor over a handle or the inside of the box.
fn cursor(hit: Hit) -> CursorIcon {
    match hit {
        Hit::Inside => CursorIcon::Move,
        Hit::Handle(Handle::TopLeft | Handle::BottomRight) => CursorIcon::ResizeNwSe,
        Hit::Handle(Handle::TopRight | Handle::BottomLeft) => CursorIcon::ResizeNeSw,
        Hit::Handle(Handle::Left | Handle::Right) => CursorIcon::ResizeHorizontal,
        Hit::Handle(Handle::Top | Handle::Bottom) => CursorIcon::ResizeVertical,
    }
}

/// The ratio as the menu shows it.
fn ratio_label(ratio: Ratio, canvas: [u32; 2]) -> String {
    match ratio {
        Ratio::Free => tr("Free").into(),
        Ratio::Original => match ratio.terms(canvas, [1, 1]) {
            Some([a, b]) if a.max(b) <= crop::EXACT_TERMS => {
                format!("{} ({a}:{b})", tr("Original"))
            }
            _ => tr("Original").into(),
        },
        Ratio::Preset([a, b]) => format!("{a}:{b}"),
        Ratio::Custom => tr("Custom").into(),
    }
}

impl EditorApp {
    /// Drops the box (and any drag of it) once another document is active, or none is: switching
    /// tabs, closing one or opening another. The ratio stays chosen.
    pub(super) fn forget_crop_of_other_documents(&mut self) {
        let current = self.session().map(|s| s.document.id);
        if self.crop.document != current {
            self.crop.document = current;
            self.crop.rect = None;
            self.crop.drag = None;
            self.crop.quad = None;
            self.crop.placing.clear();
            self.crop.size = None;
        }
    }

    fn canvas_size(&self) -> Option<[u32; 2]> {
        self.session()
            .map(|s| [s.document.width, s.document.height])
    }

    /// Options bar: Ratio, Custom W : H, Swap, the box's size, then Cancel and Apply.
    pub(super) fn crop_options(&mut self, ui: &mut egui::Ui) {
        self.forget_crop_of_other_documents();
        let Some(canvas) = self.canvas_size() else {
            return;
        };
        let mut mode = self.crop.perspective;
        widgets::segmented(
            ui,
            &mut mode,
            &[(false, tr("Box")), (true, tr("Perspective"))],
        )
        .on_hover_text(tr(
            "Switch between a crop box and perspective corners (Shift+C)",
        ));
        if mode != self.crop.perspective {
            self.set_crop_mode(mode);
        }
        ui.separator();
        if self.crop.perspective {
            self.perspective_options(ui);
            return;
        }
        let (mut ratio, mut custom) = (self.crop.ratio, self.crop.custom);
        ui.label(tr("Ratio"));
        widgets::PopUp::from_id_salt("crop_ratio")
            .selected_text(ratio_label(ratio, canvas))
            .width(110.0)
            .show_tall_ui(ui, |ui| {
                for choice in Ratio::menu() {
                    widgets::menu_choice(ui, &mut ratio, choice, ratio_label(choice, canvas));
                }
            });
        if ratio == Ratio::Custom {
            let field = |ui: &mut egui::Ui, value: &mut u32| {
                ui.add(
                    widgets::Number::new(value)
                        .size(vec2(48.0, 22.0))
                        .speed(0.1)
                        .range(1..=1000),
                )
            };
            field(ui, &mut custom[0]).on_hover_text(tr("Ratio width"));
            ui.label(":");
            field(ui, &mut custom[1]).on_hover_text(tr("Ratio height"));
        }
        let mut swap = false;
        ui.add_enabled_ui(ratio != Ratio::Free, |ui| {
            swap = widgets::button(ui, tr("Swap"))
                .on_hover_text(tr(
                    "Swap the ratio's width and height (portrait / landscape)",
                ))
                .clicked();
        });
        if swap {
            (ratio, custom) = ratio.swapped(canvas, custom);
        }
        if (ratio, custom) != (self.crop.ratio, self.crop.custom) {
            self.set_crop_ratio(ratio, custom, swap);
        }
        ui.separator();
        let rect = self.crop.rect.filter(|r| !r.is_empty());
        if let Some(rect) = rect {
            ui.label(format!("{} × {} px", rect.width, rect.height));
        } else {
            ui.label(
                RichText::new(tr("Drag a crop area, then press Enter to apply"))
                    .color(ui.palette().muted),
            );
        }
        ui.separator();
        ui.add_enabled_ui(rect.is_some(), |ui| {
            if widgets::button(ui, tr("Cancel")).clicked() {
                self.cancel_crop();
            }
            if widgets::primary_button(ui, tr("Apply")).clicked() {
                self.apply_crop();
            }
        });
    }

    /// Perspective mode's part of the bar: the straightened size (worked out from the corners
    /// until W or H is typed), what is wrong with the corners if anything, then Reset, Cancel and
    /// Apply.
    fn perspective_options(&mut self, ui: &mut egui::Ui) {
        let quad = self.crop.quad;
        let automatic = self.crop.output_size().unwrap_or([0, 0]);
        let mut size = automatic;
        ui.add_enabled_ui(quad.is_some(), |ui| {
            let field = |ui: &mut egui::Ui, value: &mut u32| {
                ui.add(
                    widgets::Number::new(value)
                        .size(vec2(56.0, 22.0))
                        .speed(1.0)
                        .range(1..=xuan::document::MAX_SIDE),
                )
            };
            ui.label(tr("Size"));
            field(ui, &mut size[0]).on_hover_text(tr("Width of the straightened canvas"));
            ui.label("×");
            field(ui, &mut size[1]).on_hover_text(tr("Height of the straightened canvas"));
            ui.label("px");
            if quad.is_some() && size != automatic {
                self.crop.size = Some(size);
            }
            ui.add_enabled_ui(self.crop.size.is_some(), |ui| {
                if widgets::button(ui, tr("Auto"))
                    .on_hover_text(tr(
                        "Work the size out from the corners: the average of opposite sides",
                    ))
                    .clicked()
                {
                    self.crop.size = None;
                }
            });
        });
        ui.separator();
        match quad.map(perspective::check) {
            None if self.crop.placing.is_empty() => {
                ui.label(
                    RichText::new(tr(
                        "Click four corners, or press Reset to start from the canvas",
                    ))
                    .color(ui.palette().muted),
                );
            }
            None => {
                ui.label(
                    RichText::new(
                        tr("{} of 4 corners placed")
                            .replace("{}", &self.crop.placing.len().to_string()),
                    )
                    .color(ui.palette().muted),
                );
            }
            Some(Err(problem)) => {
                ui.label(RichText::new(tr(problem.message())).color(ui.visuals().error_fg_color));
            }
            Some(Ok(())) => {
                ui.label(
                    RichText::new(tr(
                        "Drag the corners onto the edges, then press Enter to apply",
                    ))
                    .color(ui.palette().muted),
                );
            }
        }
        ui.separator();
        if widgets::button(ui, tr("Reset"))
            .on_hover_text(tr(
                "Put the corners back at the selection's bounds, or inset from the canvas edges",
            ))
            .clicked()
        {
            self.reset_quad();
        }
        ui.add_enabled_ui(quad.is_some() || !self.crop.placing.is_empty(), |ui| {
            if widgets::button(ui, tr("Cancel")).clicked() {
                self.cancel_crop();
            }
        });
        ui.add_enabled_ui(quad.is_some(), |ui| {
            if widgets::primary_button(ui, tr("Apply")).clicked() {
                self.apply_perspective_crop();
            }
        });
    }

    /// Box or Perspective: drops what the other mode had and starts this one as picking the tool
    /// does.
    pub(super) fn set_crop_mode(&mut self, perspective: bool) {
        self.cancel_crop();
        self.crop.perspective = perspective;
        self.start_crop();
    }

    /// Picking the Crop tool: a box at the selection's bounds, or perspective corners at them (at
    /// the inset canvas without a selection).
    pub(super) fn start_crop(&mut self) {
        if self.crop.perspective {
            self.reset_quad();
        } else {
            self.crop_from_selection();
        }
    }

    /// Puts the perspective corners where they start, and works the size out from them again.
    pub(super) fn reset_quad(&mut self) {
        self.forget_crop_of_other_documents();
        if self.crop.drag.is_some() {
            self.cancel_gesture();
        }
        let Some(document) = self.session().map(|s| &s.document) else {
            return;
        };
        let canvas = [document.width, document.height];
        let selection = document
            .selection
            .as_deref()
            .and_then(xuan::selection::bounds)
            .map(CropBox::from_bounds);
        self.crop.quad = Some(perspective::initial(canvas, selection));
        self.crop.placing.clear();
        self.crop.size = None;
    }

    /// Straightens the corners into an upright canvas as one undo step, then fits the view to
    /// it. Corners that cannot be straightened stay for fixing, with the reason in the status
    /// bar. Apply and Enter.
    pub(super) fn apply_perspective_crop(&mut self) {
        self.forget_crop_of_other_documents();
        if self.gesture.is_some() {
            self.cancel_gesture();
        }
        self.crop.drag = None;
        let (Some(quad), Some(size)) = (self.crop.quad, self.crop.output_size()) else {
            return;
        };
        if let Err(problem) = perspective::check(quad) {
            self.status = tr(problem.message()).into();
            return;
        }
        let mut removed = None;
        self.edit(tr("Perspective Crop"), |doc| {
            removed = Some(xuan::operations::perspective_crop(doc, quad, size)?);
            Ok(())
        });
        let Some(removed) = removed else {
            return;
        };
        self.crop.quad = None;
        self.crop.size = None;
        if removed > 0 {
            self.status = tr("Perspective Crop removed {} paths that reached the vanishing line")
                .replace("{}", &removed.to_string());
        }
        if let Some(s) = self.session_mut() {
            s.fit = true;
        }
    }

    /// Chooses a ratio and refits the box to it; a swap turns the box first.
    pub(super) fn set_crop_ratio(&mut self, ratio: Ratio, custom: [u32; 2], swap: bool) {
        self.crop.ratio = ratio;
        self.crop.custom = custom.map(|term| term.max(1));
        let Some(canvas) = self.canvas_size() else {
            return;
        };
        let terms = self.crop.terms(canvas, false);
        if let Some(rect) = self.crop.rect {
            let refitted = if swap {
                crop::turned(rect, terms, canvas)
            } else {
                crop::conform(rect, terms, canvas)
            };
            self.crop.rect = Some(refitted).filter(|r| !r.is_empty());
        }
    }

    /// Picking the Crop tool with a selection starts the box at the selection's bounds, in the
    /// chosen ratio.
    pub(super) fn crop_from_selection(&mut self) {
        self.forget_crop_of_other_documents();
        let Some(session) = self.session() else {
            return;
        };
        let document = &session.document;
        let canvas = [document.width, document.height];
        let Some(bounds) = document
            .selection
            .as_deref()
            .and_then(xuan::selection::bounds)
        else {
            return;
        };
        let terms = self.crop.terms(canvas, false);
        let rect = crop::fit_inside(CropBox::from_bounds(bounds).clamped(canvas), terms);
        self.crop.rect = Some(rect).filter(|r| !r.is_empty());
    }

    /// Crops the canvas to the box as one undo step, then fits the view to it. Apply and Enter.
    pub(super) fn apply_crop(&mut self) {
        self.forget_crop_of_other_documents();
        let Some(canvas) = self.canvas_size() else {
            return;
        };
        if self.gesture.is_some() {
            self.cancel_gesture();
        }
        self.crop.drag = None;
        let Some(rect) = self.crop.rect.take().map(|r| r.clamped(canvas)) else {
            return;
        };
        if rect.is_empty() {
            return;
        }
        let (start, end) = rect.corners();
        self.edit(tr("Crop"), |doc| xuan::operations::crop(doc, start, end));
        if let Some(s) = self.session_mut() {
            s.fit = true;
        }
    }

    /// Drops the box without cropping. Cancel and Escape.
    pub(super) fn cancel_crop(&mut self) {
        if self.crop.drag.is_some() {
            self.cancel_gesture();
        }
        self.crop.rect = None;
        self.crop.drag = None;
        self.crop.quad = None;
        self.crop.placing.clear();
        self.crop.size = None;
    }

    /// Arrow keys move the box rather than the layers beneath it.
    pub(super) fn nudge_crop(&mut self, dx: f32, dy: f32) -> bool {
        self.forget_crop_of_other_documents();
        if self.crop.perspective {
            let Some(quad) = self.crop.quad else {
                return false;
            };
            self.crop.quad = Some(perspective::moved(quad, Point::new(dx, dy)));
            return true;
        }
        let (Some(rect), Some(canvas)) = (self.crop.rect, self.canvas_size()) else {
            return false;
        };
        self.crop.rect = Some(crop::moved(rect, Point::new(dx, dy), canvas));
        true
    }

    /// Draws the box: the canvas outside it dimmed, its outline and handles, and, while it is
    /// being adjusted, the rule of thirds and its size. Returns the cursor for the pointer at
    /// `hover` (document pixels).
    pub(super) fn paint_crop(
        &self,
        painter: &Painter,
        canvas: Rect,
        origin: Pos2,
        zoom: f32,
        hover: Option<Point>,
    ) -> Option<CursorIcon> {
        let size = self.canvas_size()?;
        // A box from another tab is dropped at the next chance; it is never drawn here.
        if self.crop.document != self.session().map(|s| s.document.id) {
            return None;
        }
        if self.crop.perspective {
            return self.paint_quad(painter, canvas, origin, zoom, hover);
        }
        let rect = self.crop.rect?.clamped(size);
        let map = |p: Point| origin + vec2(p.x, p.y) * zoom;
        let (min, max) = rect.corners();
        let screen = Rect::from_min_max(map(min), map(max));
        let shade = Color32::from_black_alpha(110);
        for side in [
            Rect::from_min_max(canvas.min, pos2(canvas.max.x, screen.min.y)),
            Rect::from_min_max(pos2(canvas.min.x, screen.max.y), canvas.max),
            Rect::from_min_max(
                pos2(canvas.min.x, screen.min.y),
                pos2(screen.min.x, screen.max.y),
            ),
            Rect::from_min_max(
                pos2(screen.max.x, screen.min.y),
                pos2(canvas.max.x, screen.max.y),
            ),
        ] {
            if side.is_positive() {
                painter.rect_filled(side, 0.0, shade);
            }
        }
        painter.rect_stroke(
            screen,
            0.0,
            Stroke::new(1.5_f32, Color32::WHITE),
            StrokeKind::Inside,
        );
        let adjusting = self.crop.drag.is_some();
        if adjusting {
            for f in [1.0 / 3.0, 2.0 / 3.0] {
                let thirds = Stroke::new(0.7_f32, Color32::from_white_alpha(140));
                painter.line_segment(
                    [
                        pos2(screen.left() + screen.width() * f, screen.top()),
                        pos2(screen.left() + screen.width() * f, screen.bottom()),
                    ],
                    thirds,
                );
                painter.line_segment(
                    [
                        pos2(screen.left(), screen.top() + screen.height() * f),
                        pos2(screen.right(), screen.top() + screen.height() * f),
                    ],
                    thirds,
                );
            }
            // The size, below the box (above it at the bottom of the view).
            let text = format!("{} × {} px", rect.width, rect.height);
            let font = egui::FontId::proportional(11.0);
            let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
            let mut at = pos2(screen.center().x, screen.bottom() + 8.0);
            if at.y + galley.size().y + 4.0 > painter.clip_rect().bottom() {
                at.y = screen.top() - 8.0 - galley.size().y;
            }
            let label = Rect::from_center_size(
                pos2(at.x, at.y + galley.size().y / 2.0),
                galley.size() + vec2(10.0, 4.0),
            );
            painter.rect_filled(label, 3.0, Color32::from_black_alpha(170));
            painter.galley(label.min + vec2(5.0, 2.0), galley, Color32::WHITE);
        }
        // The Move tool's handles.
        if !rect.is_empty() {
            for handle in Handle::ALL {
                painter.rect(
                    Rect::from_center_size(map(rect.point(handle.unit())), egui::Vec2::splat(6.0)),
                    0.0,
                    Color32::from_gray(245),
                    Stroke::new(1.0_f32, Color32::from_gray(55)),
                    StrokeKind::Outside,
                );
            }
        }
        match self.crop.drag {
            Some(CropDrag::Move(_)) => Some(CursorIcon::Move),
            Some(CropDrag::Resize(handle, _)) => Some(cursor(Hit::Handle(handle))),
            Some(CropDrag::Draw | CropDrag::Corner(..) | CropDrag::MoveQuad(_)) => None,
            None => hover
                .and_then(|point| rect.hit(point, HANDLE_GRAB / zoom))
                .map(cursor),
        }
    }

    /// Draws the perspective corners: the canvas outside them dimmed, their outline (red when they
    /// cannot be straightened), the grid the straightened canvas will have, and the corner handles;
    /// or the corners clicked so far. Returns the cursor for the pointer at `hover`.
    fn paint_quad(
        &self,
        painter: &Painter,
        canvas: Rect,
        origin: Pos2,
        zoom: f32,
        hover: Option<Point>,
    ) -> Option<CursorIcon> {
        let map = |p: Point| origin + vec2(p.x, p.y) * zoom;
        let handle = |p: Point| {
            painter.rect(
                Rect::from_center_size(map(p), egui::Vec2::splat(7.0)),
                0.0,
                Color32::from_gray(245),
                Stroke::new(1.0_f32, Color32::from_gray(55)),
                StrokeKind::Outside,
            );
        };
        let Some(quad) = self.crop.quad else {
            let line = Stroke::new(1.5_f32, Color32::WHITE);
            for pair in self.crop.placing.windows(2) {
                painter.line_segment([map(pair[0]), map(pair[1])], line);
            }
            self.crop.placing.iter().copied().for_each(handle);
            return Some(CursorIcon::Crosshair);
        };
        let screen = quad.map(map);
        let valid = perspective::check(quad).is_ok();
        // The canvas outside the quad, in four pieces between the canvas's corners and its own
        // (clockwise corners only; others are refused or mirror, and are only outlined).
        let shade = Color32::from_black_alpha(110);
        let outer = [
            canvas.left_top(),
            canvas.right_top(),
            canvas.right_bottom(),
            canvas.left_bottom(),
        ];
        let mut mesh = egui::Mesh::default();
        for i in 0..4 {
            let j = (i + 1) % 4;
            let base = mesh.vertices.len() as u32;
            for p in [outer[i], outer[j], screen[j], screen[i]] {
                mesh.colored_vertex(p, shade);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        if valid && perspective::area(quad) > 0.0 {
            painter.add(mesh);
        }
        let grid = Stroke::new(0.7_f32, Color32::from_white_alpha(140));
        for [a, b] in perspective::grid(quad, GRID_DIVISIONS) {
            painter.line_segment([map(a), map(b)], grid);
        }
        let outline = if valid {
            Color32::WHITE
        } else {
            Color32::from_rgb(235, 64, 52)
        };
        let mut closed = screen.to_vec();
        closed.push(screen[0]);
        painter.add(egui::Shape::line(closed, Stroke::new(1.5_f32, outline)));
        quad.into_iter().for_each(handle);
        match self.crop.drag {
            Some(CropDrag::MoveQuad(_)) => Some(CursorIcon::Move),
            Some(CropDrag::Corner(..)) => Some(CursorIcon::Crosshair),
            _ => hover.and_then(|point| {
                if perspective::corner_at(quad, point, HANDLE_GRAB / zoom).is_some() {
                    Some(CursorIcon::Crosshair)
                } else {
                    perspective::contains(quad, point).then_some(CursorIcon::Move)
                }
            }),
        }
    }
}

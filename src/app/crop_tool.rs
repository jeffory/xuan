//! The Crop tool: a box drawn on the canvas, adjusted by its handles or moved before it is applied,
//! the Ratio it keeps, and the options bar that applies or cancels it. The box's arithmetic is
//! in `xuan::crop`; this draws it and wires it to the pointer and the bar.
use super::theme::PaletteExt as _;
use super::widgets;
use egui::{Color32, CursorIcon, Painter, Pos2, Rect, RichText, Stroke, StrokeKind, pos2, vec2};
use xuan::{
    crop::{self, CropBox, Handle, Hit, Ratio},
    document::Point,
    i18n::tr,
};

use super::EditorApp;

/// How close, in points, a press must come to a handle to grab it, as with the Move tool.
const HANDLE_GRAB: f32 = 9.0;

/// What a drag on the canvas does to the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CropDrag {
    /// Draw a new box from the press.
    Draw,
    /// Move the box that was there.
    Move(CropBox),
    /// Drag one of its handles.
    Resize(Handle, CropBox),
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
}

impl Default for CropTool {
    fn default() -> Self {
        Self {
            ratio: Ratio::Free,
            custom: [1, 1],
            rect: None,
            drag: None,
            document: None,
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

    /// Follows the pointer from `start` to `point`.
    pub(super) fn drag_to(&mut self, start: Point, point: Point, canvas: [u32; 2], shift: bool) {
        let delta = Point::new(point.x - start.x, point.y - start.y);
        let terms = self.terms(canvas, shift);
        self.rect = Some(match self.drag.unwrap_or(CropDrag::Draw) {
            CropDrag::Draw => crop::draw(start, point, terms, canvas),
            CropDrag::Move(original) => crop::moved(original, delta, canvas),
            CropDrag::Resize(handle, original) => {
                crop::resize(original, handle, delta, terms, canvas)
            }
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
    }

    /// Arrow keys move the box rather than the layers beneath it.
    pub(super) fn nudge_crop(&mut self, dx: f32, dy: f32) -> bool {
        self.forget_crop_of_other_documents();
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
            Some(CropDrag::Draw) => None,
            None => hover
                .and_then(|point| rect.hit(point, HANDLE_GRAB / zoom))
                .map(cursor),
        }
    }
}

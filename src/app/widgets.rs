//! AppKit-like controls. Keep egui's input, focus and accessibility behavior where
//! possible, and paint the small details its stock theme cannot express.
use std::ops::RangeInclusive;
use xuan::i18n::tr;

use egui::{
    Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Widget, pos2,
    vec2,
};

use super::theme::{self, PaletteExt};

pub const NUMBER_WIDTH: f32 = 80.0;
pub const SLIDER_LABEL_WIDTH: f32 = 82.0;
pub const SLIDER_SPACING: f32 = 6.0;
pub const SLIDER_FIELD_WIDTH: f32 = SLIDER_LABEL_WIDTH + 2.0 * SLIDER_SPACING + NUMBER_WIDTH;

const COLOR_PICKER_WIDTH: f32 = 260.0;

pub fn gradient(ui: &Ui, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    if !rect.is_positive() {
        return;
    }

    // Reuse egui's rounded outline and pixel-scaled feathering. Raw triangle
    // meshes are passed through unchanged by the UI renderer.
    let mut mesh = egui::Mesh::default();
    let mut tessellator = egui::epaint::Tessellator::new(
        ui.pixels_per_point(),
        ui.ctx().tessellation_options(|options| *options),
        [1, 1],
        Vec::new(),
    );
    tessellator.tessellate_rect(
        &egui::epaint::RectShape::filled(rect, radius, Color32::WHITE),
        &mut mesh,
    );

    for vertex in &mut mesh.vertices {
        let t = ((vertex.pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let color = Color32::from_rgba_premultiplied(
            egui::lerp(top.r() as f32..=bottom.r() as f32, t) as u8,
            egui::lerp(top.g() as f32..=bottom.g() as f32, t) as u8,
            egui::lerp(top.b() as f32..=bottom.b() as f32, t) as u8,
            egui::lerp(top.a() as f32..=bottom.a() as f32, t) as u8,
        );
        // Preserve edge coverage when tinting the white fill with the gradient.
        vertex.color = color.gamma_multiply_u8(vertex.color.a());
    }
    ui.painter().add(mesh);
}

pub fn bezel(ui: &Ui, response: &Response, radius: f32, primary: bool) {
    let rect = response.rect;
    let pressed = response.is_pointer_button_down_on();
    let p = ui.palette();
    let [top, bottom] = match (primary, pressed) {
        (true, true) => p.accent_pressed,
        (true, false) => p.accent_gradient,
        (false, true) => p.control_pressed,
        (false, false) if response.hovered() => p.control_hover,
        (false, false) => p.control,
    };
    ui.painter()
        .rect_filled(rect.translate(vec2(0.0, 1.0)), radius, p.control_shadow);
    gradient(ui, rect, radius, top, bottom);
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        radius,
        Stroke::new(1.0_f32, p.control_edge),
        StrokeKind::Inside,
    );
    focus_ring(ui, response, radius);
}

fn focus_ring(ui: &Ui, response: &Response, radius: f32) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            radius + 2.0,
            Stroke::new(2.0_f32, ui.palette().accent.gamma_multiply(0.8)),
            StrokeKind::Outside,
        );
    }
}

pub struct Button {
    label: String,
    primary: bool,
    destructive: bool,
    size: egui::Vec2,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            primary: false,
            destructive: false,
            size: vec2(0.0, 22.0),
        }
    }
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
    /// An action that throws work away, such as Discard changes: a plain bezel with the label in
    /// the error colour, so it never looks like the default button.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
    pub fn min_size(mut self, size: egui::Vec2) -> Self {
        self.size = size;
        self
    }
}

impl Widget for Button {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = ui.palette();
        let text = if self.primary {
            p.on_accent_text
        } else if self.destructive && ui.is_enabled() {
            p.error
        } else {
            p.text
        };
        let galley =
            ui.painter()
                .layout_no_wrap(self.label.clone(), FontId::proportional(12.0), text);
        let size = vec2(galley.size().x + 24.0, 22.0).max(self.size);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &self.label)
        });
        if ui.is_rect_visible(rect) {
            bezel(ui, &response, theme::BUTTON_RADIUS as f32, self.primary);
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, text);
        }
        response
    }
}

pub fn button(ui: &mut Ui, label: impl Into<String>) -> Response {
    ui.add(Button::new(label))
}
pub fn primary_button(ui: &mut Ui, label: impl Into<String>) -> Response {
    ui.add(Button::new(label).primary())
}

/// Consume vertical wheel motion over a control, leaving horizontal scrolling to its parent.
fn wheel_steps(ui: &Ui, response: &Response) -> f64 {
    let id = response.id.with("wheel_remainder");
    if !response.enabled() || !response.hovered() {
        ui.data_mut(|data| data.remove::<f64>(id));
        return 0.0;
    }
    let delta = ui.input_mut(|input| {
        if input.smooth_scroll_delta.y == 0.0 {
            return 0.0;
        }
        // Consume the smoothing tail too, so the containing panel stays still.
        input.smooth_scroll_delta.y = 0.0;
        std::mem::take(&mut input.raw_scroll_delta.y)
    });
    let line_height = ui
        .ctx()
        .options(|options| options.input_options.line_scroll_speed);
    ui.data_mut(|data| {
        let remainder = data.get_temp_mut_or_default::<f64>(id);
        *remainder += f64::from(delta / line_height);
        let steps = remainder.trunc();
        *remainder -= steps;
        steps
    })
}

pub(super) fn wheel_value<N: egui::emath::Numeric>(
    ui: &Ui,
    response: &mut Response,
    value: &mut N,
    range: RangeInclusive<f64>,
    step: f64,
    decimals: Option<usize>,
) {
    let steps = wheel_steps(ui, response);
    if steps == 0.0 {
        return;
    }
    let old = value.to_f64();
    let step = if N::INTEGRAL { step.max(1.0) } else { step };
    let mut new = old + steps * step;
    if let Some(decimals) = decimals {
        new = egui::emath::round_to_decimals(new, decimals);
    }
    *value = N::from_f64(new.clamp(
        range.start().min(*range.end()),
        range.start().max(*range.end()),
    ));
    if value.to_f64() != old {
        response.mark_changed();
        // DragValue caches its text while focused. Refresh it after a wheel edit.
        ui.data_mut(|data| data.remove::<String>(response.id));
        ui.ctx().request_repaint();
    }
}

/// Reserve the control's footprint even when its text grows during editing.
pub fn fixed_size(ui: &mut Ui, size: egui::Vec2, widget: impl Widget) -> Response {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.set_clip_rect(ui.clip_rect().intersect(rect));
    let mut response = child.add(widget);
    response.rect = rect;
    response.interact_rect = response.interact_rect.intersect(rect);
    response
}

/// A recessed field in place of DragValue's raised button.
pub struct Number<'a, N> {
    value: &'a mut N,
    speed: f64,
    range: RangeInclusive<f64>,
    suffix: String,
    max_decimals: Option<usize>,
    clamp_existing_to_range: bool,
    size: egui::Vec2,
}
impl<'a, N: egui::emath::Numeric> Number<'a, N> {
    pub fn new(value: &'a mut N) -> Self {
        Self {
            value,
            speed: if N::INTEGRAL { 0.25 } else { 1.0 },
            range: N::MIN.to_f64()..=N::MAX.to_f64(),
            suffix: String::new(),
            max_decimals: N::INTEGRAL.then_some(0),
            clamp_existing_to_range: true,
            size: vec2(NUMBER_WIDTH, 22.0),
        }
    }
    pub fn speed(mut self, speed: impl Into<f64>) -> Self {
        self.speed = speed.into();
        self
    }
    pub fn range<T: egui::emath::Numeric>(mut self, range: RangeInclusive<T>) -> Self {
        self.range = range.start().to_f64()..=range.end().to_f64();
        self
    }
    /// Show a unit inside the field while idle; egui edits the numeric text alone.
    pub fn suffix(mut self, suffix: impl ToString) -> Self {
        self.suffix = suffix.to_string();
        self
    }
    pub fn max_decimals(mut self, decimals: usize) -> Self {
        self.max_decimals = Some(decimals);
        self
    }
    pub fn size(mut self, size: egui::Vec2) -> Self {
        self.size = size;
        self
    }
}
impl<N: egui::emath::Numeric> Widget for Number<'_, N> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.scope(|ui| {
            ui.spacing_mut().button_padding = vec2(6.0, 3.0);
            ui.spacing_mut().interact_size = self.size;
            let p = ui.palette();
            let visuals = ui.visuals_mut();
            visuals.selection.bg_fill = p.accent.gamma_multiply(0.5);
            for widget in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
            ] {
                widget.corner_radius = CornerRadius::same(4);
                widget.bg_fill = p.field;
                widget.weak_bg_fill = p.field;
                widget.bg_stroke = Stroke::new(1.0_f32, p.border);
                widget.expansion = 0.0;
            }
            let mut number = egui::DragValue::new(&mut *self.value)
                .speed(self.speed)
                .range(self.range.clone())
                .clamp_existing_to_range(self.clamp_existing_to_range)
                .suffix(self.suffix);
            if let Some(decimals) = self.max_decimals {
                number = number.max_decimals(decimals);
            }
            let mut response = fixed_size(ui, self.size, number);
            wheel_value(
                ui,
                &mut response,
                self.value,
                self.range,
                self.speed,
                self.max_decimals,
            );
            response
        })
        .inner
    }
}

pub fn checkbox(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    described_checkbox(ui, value, label, label)
}

/// A checkbox without visible text, named `description` for assistive technology.
pub fn bare_checkbox(ui: &mut Ui, value: &mut bool, description: &str) -> Response {
    described_checkbox(ui, value, "", description)
}

fn described_checkbox(ui: &mut Ui, value: &mut bool, label: &str, description: &str) -> Response {
    let galley =
        ui.painter()
            .layout_no_wrap(label.into(), FontId::proportional(12.0), ui.palette().text);
    let width = 14.0
        + if label.is_empty() {
            0.0
        } else {
            6.0 + galley.size().x
        };
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            *value,
            description,
        )
    });
    let box_rect =
        Rect::from_center_size(pos2(rect.left() + 7.0, rect.center().y), vec2(14.0, 14.0));
    let p = ui.palette();
    let [top, bottom] = if *value {
        p.check
    } else if response.hovered() {
        p.checkbox_hover
    } else {
        p.checkbox
    };
    gradient(ui, box_rect, 3.5, top, bottom);
    ui.painter().rect_stroke(
        box_rect,
        3.5,
        Stroke::new(0.7_f32, p.checkbox_edge),
        StrokeKind::Inside,
    );
    if *value {
        ui.painter().add(egui::Shape::line(
            vec![
                box_rect.min + vec2(3.0, 7.0),
                box_rect.min + vec2(6.0, 10.0),
                box_rect.min + vec2(11.0, 4.0),
            ],
            Stroke::new(1.6_f32, p.on_accent),
        ));
    }
    ui.painter().galley(
        pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ui.palette().text,
    );
    focus_ring(ui, &response, 4.0);
    response
}

pub struct Slider<'a, N> {
    value: &'a mut N,
    range: RangeInclusive<N>,
    label: String,
    suffix: String,
    logarithmic: bool,
    percentage: bool,
    max_decimals: Option<usize>,
    clamp_existing_to_range: bool,
    value_size: egui::Vec2,
}
impl<'a, N: egui::emath::Numeric> Slider<'a, N> {
    pub fn new(value: &'a mut N, range: RangeInclusive<N>) -> Self {
        Self {
            value,
            range,
            label: String::new(),
            suffix: String::new(),
            logarithmic: false,
            percentage: false,
            max_decimals: None,
            clamp_existing_to_range: true,
            value_size: vec2(NUMBER_WIDTH, 22.0),
        }
    }
    pub fn text(mut self, label: impl ToString) -> Self {
        self.label = label.to_string();
        self
    }
    pub fn suffix(mut self, suffix: impl ToString) -> Self {
        self.suffix = suffix.to_string();
        self
    }
    pub fn logarithmic(mut self, logarithmic: bool) -> Self {
        self.logarithmic = logarithmic;
        self
    }
    pub fn percentage(mut self) -> Self {
        self.percentage = true;
        self.suffix = "%".into();
        self
    }
    /// Set the numeric field width for this slider; defaults to `NUMBER_WIDTH`.
    pub fn value_width(mut self, width: f32) -> Self {
        self.value_size.x = width;
        self
    }
    pub fn max_decimals(mut self, decimals: usize) -> Self {
        self.max_decimals = Some(decimals);
        self
    }
    /// Keep loaded values intact while still clamping edits to the slider range.
    pub fn clamp_existing_to_range(mut self, clamp: bool) -> Self {
        self.clamp_existing_to_range = clamp;
        self
    }
}
impl<N: egui::emath::Numeric> Widget for Slider<'_, N> {
    fn ui(self, ui: &mut Ui) -> Response {
        let old = self.value.to_f64();
        let mut value = old;
        let range = self.range.start().to_f64()..=self.range.end().to_f64();
        let scale = if self.percentage { 100.0 } else { 1.0 };
        let decimals = self.max_decimals.unwrap_or(
            if N::INTEGRAL || self.percentage || range.end() - range.start() > 20.0 {
                0
            } else {
                2
            },
        );
        let speed = if decimals == 0 { 1.0 } else { 0.01 };
        let result = ui.horizontal(|ui| {
            ui.set_min_height(self.value_size.y.max(ui.spacing().interact_size.y));
            ui.spacing_mut().item_spacing.x = SLIDER_SPACING;
            if !self.label.is_empty() {
                ui.add_sized(
                    [SLIDER_LABEL_WIDTH, 22.0],
                    egui::Label::new(&self.label).halign(egui::Align::Min),
                );
            }
            // An invisible native slider retains keyboard navigation and range semantics.
            // Only its painting is replaced; input remains enabled.
            let mut response = ui
                .scope(|ui| {
                    ui.set_opacity(0.0);
                    ui.spacing_mut().interact_size.y = 18.0;
                    let mut slider = egui::Slider::new(&mut value, range.clone())
                        .show_value(false)
                        .logarithmic(self.logarithmic)
                        .max_decimals_opt(self.max_decimals)
                        .clamping(if self.clamp_existing_to_range {
                            egui::SliderClamping::Always
                        } else {
                            egui::SliderClamping::Edits
                        })
                        .handle_shape(egui::style::HandleShape::Circle);
                    if N::INTEGRAL {
                        slider = slider.integer();
                    }
                    ui.add(slider)
                })
                .inner;
            wheel_value(
                ui,
                &mut response,
                &mut value,
                range.clone(),
                speed / scale,
                Some(decimals + if self.percentage { 2 } else { 0 }),
            );
            let r = response.rect;
            let radius = r.height() / 2.5;
            let x_range = (r.left() + radius)..=(r.right() - radius);
            let t = if self.logarithmic && *range.start() > 0.0 {
                (value.ln() - range.start().ln()) / (range.end().ln() - range.start().ln())
            } else {
                (value - range.start()) / (range.end() - range.start())
            };
            let x = egui::lerp(x_range.clone(), t.clamp(0.0, 1.0) as f32);
            let rail = Rect::from_min_max(
                pos2(*x_range.start(), r.center().y - 1.5),
                pos2(*x_range.end(), r.center().y + 1.5),
            );
            let p = ui.palette();
            ui.painter()
                .rect_filled(rail.translate(vec2(0.0, 1.0)), 2.0, p.slider_rail_edge);
            ui.painter().rect_filled(rail, 2.0, p.slider_rail);
            ui.painter().rect_filled(
                Rect::from_min_max(rail.min, pos2(x, rail.bottom())),
                2.0,
                p.accent,
            );
            let thumb = Rect::from_center_size(pos2(x, r.center().y), vec2(14.0, 14.0));
            ui.painter()
                .circle_filled(thumb.center() + vec2(0.0, 1.0), 7.5, p.thumb_shadow);
            gradient(
                ui,
                thumb,
                7.0,
                p.thumb[0],
                if response.is_pointer_button_down_on() {
                    p.thumb_pressed
                } else {
                    p.thumb[1]
                },
            );
            ui.painter()
                .circle_stroke(thumb.center(), 7.0, Stroke::new(0.6_f32, p.thumb_edge));
            focus_ring(ui, &response, 4.0);
            let mut display = value * scale;
            let mut number = Number::new(&mut display)
                .size(self.value_size)
                .range(range.start() * scale..=range.end() * scale)
                .speed(speed)
                .suffix(self.suffix)
                .max_decimals(decimals);
            number.clamp_existing_to_range = self.clamp_existing_to_range;
            let number = ui.add(number);
            if number.changed() {
                value = display / scale;
            }
            response.union(number)
        });
        *self.value = N::from_f64(value);
        let mut response = result.inner;
        if self.value.to_f64() != old {
            response.mark_changed();
        }
        response
    }
}

pub fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
) -> Response {
    let result = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut responses = Vec::new();
        for &(option, label) in options {
            let galley = ui.painter().layout_no_wrap(
                label.into(),
                FontId::proportional(12.0),
                ui.palette().text,
            );
            let (rect, mut response) =
                ui.allocate_exact_size(vec2(galley.size().x + 20.0, 22.0), Sense::click());
            if response.clicked() && *value != option {
                *value = option;
                response.mark_changed();
            }
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    ui.is_enabled(),
                    *value == option,
                    label,
                )
            });
            responses.push((rect, response, galley, *value == option));
        }
        let rect = responses
            .iter()
            .fold(Rect::NOTHING, |rect, (r, ..)| rect.union(*r));
        let p = ui.palette();
        gradient(ui, rect, 5.0, p.segment_track[0], p.segment_track[1]);
        ui.painter().rect_stroke(
            rect,
            5.0,
            Stroke::new(1.0_f32, p.segment_edge),
            StrokeKind::Inside,
        );
        let mut combined = ui.interact(rect, ui.next_auto_id(), Sense::hover());
        for (index, (rect, response, galley, selected)) in responses.into_iter().enumerate() {
            if selected {
                bezel(
                    ui,
                    &Response {
                        rect: rect.shrink(1.0),
                        ..response.clone()
                    },
                    4.0,
                    false,
                );
            } else if index > 0 {
                ui.painter().line_segment(
                    [
                        rect.left_top() + vec2(0.0, 5.0),
                        rect.left_bottom() - vec2(0.0, 5.0),
                    ],
                    Stroke::new(1.0_f32, p.segment_separator),
                );
            }
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, p.text);
            combined = combined.union(response);
        }
        combined
    });
    result.inner
}

/// Rounded pop-up with the paired AppKit chevrons; the menu itself stays native egui.
pub struct PopUp {
    id: egui::Id,
    text: String,
    width: f32,
}
impl PopUp {
    pub fn from_id_salt(id: impl std::hash::Hash) -> Self {
        Self {
            id: egui::Id::new(id),
            text: String::new(),
            width: 160.0,
        }
    }
    pub fn selected_text(mut self, text: impl ToString) -> Self {
        self.text = text.to_string();
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    pub fn show_ui<R>(
        self,
        ui: &mut Ui,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> Option<egui::InnerResponse<R>> {
        let response = self.button(ui);
        egui::Popup::menu(&response).width(self.width).show(content)
    }

    /// Like `show_ui`, for long lists: the menu opens on the side of the button with more room
    /// (and is kept on screen), rather than always below it.
    pub fn show_tall_ui<R>(
        self,
        ui: &mut Ui,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> Option<egui::InnerResponse<R>> {
        let response = self.button(ui);
        let screen = ui.ctx().content_rect();
        let above = response.rect.top() - screen.top();
        let below = screen.bottom() - response.rect.bottom();
        let align = if above > below {
            egui::RectAlign::TOP_START
        } else {
            egui::RectAlign::BOTTOM_START
        };
        egui::Popup::menu(&response)
            .width(self.width)
            .align(align)
            .align_alternatives(&[])
            .show(content)
    }

    pub fn button(&self, ui: &mut Ui) -> Response {
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, 22.0), Sense::hover());
        let response = ui.interact(rect, ui.make_persistent_id(self.id), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), &self.text)
        });
        bezel(ui, &response, theme::BUTTON_RADIUS as f32, false);
        let painter = ui.painter().with_clip_rect(Rect::from_min_max(
            rect.min + vec2(10.0, 0.0),
            rect.max - vec2(26.0, 0.0),
        ));
        painter.text(
            pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &self.text,
            FontId::proportional(12.0),
            ui.palette().text,
        );
        let center = pos2(rect.right() - 12.0, rect.center().y);
        for direction in [-1.0, 1.0] {
            ui.painter().add(egui::Shape::line(
                vec![
                    center + vec2(-3.0, direction * 1.5),
                    center + vec2(0.0, direction * 4.0),
                    center + vec2(3.0, direction * 1.5),
                ],
                Stroke::new(1.2_f32, ui.palette().text),
            ));
        }
        response
    }
}

pub fn color_well(ui: &mut Ui, color: &mut [u8; 4]) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ColorButton, ui.is_enabled(), tr("Color"))
    });
    ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
    checkerboard(ui, rect.shrink(2.0), 4.0);
    ui.painter().rect_filled(
        rect.shrink(2.0),
        2.0,
        Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]),
    );
    ui.painter().rect_stroke(
        rect.shrink(1.5),
        3.0,
        Stroke::new(1.0_f32, Color32::from_gray(225)),
        StrokeKind::Inside,
    );
    focus_ring(ui, &response, 4.0);
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            if color_picker(ui, color) {
                response.mark_changed();
            }
        });
    response
}

fn color_picker(ui: &mut Ui, color: &mut [u8; 4]) -> bool {
    ui.set_width(COLOR_PICKER_WIDTH);
    ui.spacing_mut().slider_width = ui.available_width();

    let before = *color;
    let state_id = ui.id().with("color_picker_state");
    // RGB cannot retain hue for gray or black, or saturation for black. Keep the
    // full picker state until the color is changed outside this popup.
    let mut hsva = ui
        .data(|data| data.get_temp::<([u8; 4], egui::ecolor::Hsva)>(state_id))
        .filter(|(rgba, _)| *rgba == before)
        .map(|(_, hsva)| hsva)
        .unwrap_or_else(|| {
            let [r, g, b, a] = before;
            egui::ecolor::Hsva {
                a: a as f32 / 255.0,
                ..egui::ecolor::Hsva::from_srgb([r, g, b])
            }
        });

    let first_shape = ui
        .ctx()
        .graphics_mut(|graphics| graphics.entry(ui.layer_id()).next_idx());
    if egui::color_picker::color_picker_hsva_2d(
        ui,
        &mut hsva,
        egui::color_picker::Alpha::BlendOrAdditive,
    ) {
        *color = hsva.to_srgba_unmultiplied();
    }
    ui.data_mut(|data| data.insert_temp(state_id, (*color, hsva)));

    // egui 0.33 sizes the map marker to 1/12 of the map width and exposes no
    // radius setting. Restyle only that circle in this picker's paint range.
    let marker_radius = ui.spacing().slider_width / 12.0;
    ui.ctx().graphics_mut(|graphics| {
        let shapes = graphics.entry(ui.layer_id());
        for index in first_shape.0..shapes.next_idx().0 {
            shapes.mutate_shape(egui::layers::ShapeIdx(index), |shape| {
                if let egui::Shape::Circle(circle) = &mut shape.shape
                    && (circle.radius - marker_radius).abs() < f32::EPSILON
                {
                    circle.radius = 5.0;
                }
            });
        }
    });

    *color != before
}

#[cfg(test)]
#[path = "tests/color_picker.rs"]
mod color_picker_tests;

pub fn checkerboard(ui: &Ui, rect: Rect, cell: f32) {
    ui.painter().rect_filled(rect, 2.0, Color32::from_gray(115));
    for row in 0..(rect.height() / cell).ceil() as usize {
        for col in 0..(rect.width() / cell).ceil() as usize {
            if (row + col) % 2 == 0 {
                ui.painter().rect_filled(
                    Rect::from_min_size(
                        rect.min + vec2(col as f32 * cell, row as f32 * cell),
                        vec2(cell, cell),
                    )
                    .intersect(rect),
                    0.0,
                    Color32::from_gray(160),
                );
            }
        }
    }
}

/// The dialog's close control, drawn in the main window's title bar style: a traffic-light
/// dot on the left for macOS, else the compact window button on the side the button layout
/// puts it. Returns whether it was clicked.
fn close_control(ui: &mut Ui, bar: Rect, id: egui::Id) -> bool {
    use super::chrome::{BUTTON_SIZE, DialogChrome, WindowButton, paint_window_button};
    let chrome: Option<DialogChrome> = ui.data(|data| data.get_temp(DialogChrome::id()));
    let label = tr("Close panel");
    let compact = chrome
        .as_ref()
        .filter(|chrome| chrome.title_bar != xuan::config::TitleBar::MacOs);
    let Some(chrome) = compact else {
        let center = pos2(bar.left() + 15.0, bar.center().y);
        let response = ui.interact(
            Rect::from_center_size(center, vec2(22.0, 22.0)),
            id,
            Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        ui.painter().circle_filled(
            center,
            5.0,
            ui.palette().close_dot[usize::from(response.hovered())],
        );
        if response.hovered() {
            let stroke = Stroke::new(1.0_f32, ui.palette().panel);
            ui.painter()
                .line_segment([center - vec2(2.0, 2.0), center + vec2(2.0, 2.0)], stroke);
            ui.painter()
                .line_segment([center + vec2(-2.0, 2.0), center + vec2(2.0, -2.0)], stroke);
        }
        return response.on_hover_text(label).clicked();
    };
    let on_left = !chrome.layout.right.contains(&WindowButton::Close)
        && chrome.layout.left.contains(&WindowButton::Close);
    let margin = 6.0;
    let rect = Rect::from_center_size(
        pos2(
            if on_left {
                bar.left() + margin + BUTTON_SIZE.x / 2.0
            } else {
                bar.right() - margin - BUTTON_SIZE.x / 2.0
            },
            bar.center().y,
        ),
        BUTTON_SIZE,
    );
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
    paint_window_button(
        ui,
        &chrome.style,
        rect,
        WindowButton::Close,
        false,
        &response,
        focused,
    );
    response.on_hover_text(label).clicked()
}

/// Floating utility panel: compact centered title, a close control placed as in the
/// main title bar, and 24-point content insets, as in FloatingPanelController / the SwiftUI sheets.
pub struct Window<'a> {
    title: String,
    open: Option<&'a mut bool>,
    width: f32,
    id: Option<egui::Id>,
    /// Where the window first shows: centred unless set.
    place: Option<(egui::Align2, egui::Pos2)>,
}
impl<'a> Window<'a> {
    pub fn new(title: impl ToString) -> Self {
        Self {
            title: title.to_string(),
            open: None,
            width: 410.0,
            id: None,
            place: None,
        }
    }
    /// Shows the window first with its `pivot` corner at `pos`, for panels that leave
    /// the canvas free to click on.
    pub fn default_place(mut self, pivot: egui::Align2, pos: egui::Pos2) -> Self {
        self.place = Some((pivot, pos));
        self
    }
    pub fn id(mut self, id: impl std::hash::Hash) -> Self {
        self.id = Some(egui::Id::new(id));
        self
    }
    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }
    pub fn default_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    /// Shows the window with `content` as its whole body, scrolled when it is taller than the
    /// space the window has.
    pub fn show<R>(self, ctx: &egui::Context, content: impl FnOnce(&mut Ui) -> R) {
        self.show_parts(ctx, content, None::<fn(&mut Ui, R)>);
    }

    /// Shows the window with a scrolled `body` and a `footer` below it that never scrolls, so the
    /// dialog's buttons stay in sight however small the main window is. The footer gets what the
    /// body returned, such as whether the form is valid; draw it with [`dialog_footer`].
    pub fn show_with_footer<R>(
        self,
        ctx: &egui::Context,
        body: impl FnOnce(&mut Ui) -> R,
        footer: impl FnOnce(&mut Ui, R),
    ) {
        self.show_parts(ctx, body, Some(footer));
    }

    fn show_parts<R>(
        mut self,
        ctx: &egui::Context,
        body: impl FnOnce(&mut Ui) -> R,
        footer: Option<impl FnOnce(&mut Ui, R)>,
    ) {
        if self.open.as_deref() == Some(&false) {
            return;
        }
        let id = self.id.unwrap_or_else(|| egui::Id::new(&self.title));
        // Keep clear of the title and menu bar at the top and of the window's bottom edge.
        let bounds = dialog_bounds(ctx);
        let footer_height_id = id.with("footer_height");
        let mut close = false;
        egui::Window::new(&self.title)
            .id(id)
            .title_bar(false)
            .auto_sized()
            .pivot(self.place.map_or(egui::Align2::CENTER_CENTER, |p| p.0))
            .default_pos(self.place.map_or(bounds.center(), |p| p.1))
            .default_width(self.width)
            .constrain_to(bounds)
            .frame(egui::Frame::window(&ctx.style()).inner_margin(0))
            .show(ctx, |ui| {
                ui.set_width(self.width);
                ui.spacing_mut().item_spacing.y = 0.0;
                // Reserve the title bar now and paint it last, at the width the content gave
                // the window.
                let (bar, response) =
                    ui.allocate_exact_size(vec2(self.width, TITLE_HEIGHT), Sense::hover());
                // The footer's height from the last frame, or a guess for the first (sizing) one.
                let footer_height = match footer {
                    Some(_) => ui
                        .data(|data| data.get_temp::<f32>(footer_height_id))
                        .unwrap_or(FOOTER_GUESS),
                    None => 0.0,
                };
                let inset = DIALOG_INSET as f32;
                let bottom = if footer.is_some() { 0.0 } else { inset };
                let max_body =
                    (bounds.height() - TITLE_HEIGHT - inset - bottom - footer_height - 2.0)
                        .max(MIN_BODY_HEIGHT);
                let result = egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: DIALOG_INSET,
                        // The scroll bar sits in the right inset, clear of the content.
                        right: DIALOG_INSET - SCROLL_GUTTER,
                        top: DIALOG_INSET,
                        bottom: bottom as i8,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 12.0;
                        ui.spacing_mut().slider_width =
                            (self.width - 2.0 * inset - SLIDER_FIELD_WIDTH).max(90.0);
                        ui.set_width(self.width - 2.0 * inset + SCROLL_GUTTER as f32);
                        // Show the bar whenever the body overflows, not only on hover.
                        ui.spacing_mut().scroll.dormant_handle_opacity = 0.5;
                        ui.spacing_mut().scroll.dormant_background_opacity = 0.0;
                        let output = egui::ScrollArea::vertical()
                            .max_height(max_body)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.set_width(self.width - 2.0 * inset);
                                body(ui)
                            });
                        overflow_fades(ui, &output);
                        output.inner
                    })
                    .inner;
                if let Some(footer) = footer {
                    let top = ui.cursor().top();
                    let width = ui.min_rect().width().max(self.width);
                    // One rule and gap above every dialog's buttons.
                    ui.add_space(FOOTER_GAP);
                    let rule = ui.cursor().top();
                    ui.add_space(FOOTER_GAP);
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: DIALOG_INSET,
                            right: DIALOG_INSET,
                            top: 0,
                            bottom: DIALOG_INSET,
                        })
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 8.0;
                            ui.set_width(width - 2.0 * inset);
                            footer(ui, result);
                        });
                    let height = ui.min_rect().bottom() - top;
                    ui.data_mut(|data| data.insert_temp(footer_height_id, height));
                    ui.painter().hline(
                        ui.min_rect().x_range(),
                        rule,
                        Stroke::new(1.0_f32, ui.palette().divider),
                    );
                }
                let bar = Rect::from_min_size(
                    bar.min,
                    vec2(ui.min_rect().width().max(bar.width()), TITLE_HEIGHT),
                );
                ui.painter().rect_filled(
                    bar,
                    CornerRadius {
                        nw: 10,
                        ne: 10,
                        sw: 0,
                        se: 0,
                    },
                    ui.palette().titlebar,
                );
                ui.painter().line_segment(
                    [bar.left_bottom(), bar.right_bottom()],
                    Stroke::new(1.0_f32, ui.palette().header_rule),
                );
                // The same title text as the main title bar's.
                ui.painter().text(
                    bar.center(),
                    egui::Align2::CENTER_CENTER,
                    &self.title,
                    FontId::proportional(12.0),
                    ui.palette().muted,
                );
                if self.open.is_some() {
                    close = close_control(ui, bar, response.id.with("close"));
                }
            });
        if close && let Some(open) = self.open.as_mut() {
            **open = false;
        }
    }
}

/// The buttons in a dialog footer; see [`dialog_footer`].
pub struct FooterButtons<'a> {
    commit: Option<&'a str>,
    commit_enabled: bool,
    cancel: Option<&'a str>,
    destructive: Option<&'a str>,
}

impl<'a> FooterButtons<'a> {
    /// [Cancel][`commit`], the usual pair.
    pub fn commit(label: &'a str) -> Self {
        Self {
            commit: Some(label),
            commit_enabled: true,
            cancel: Some(tr("Cancel")),
            destructive: None,
        }
    }
    /// A single button with no Cancel: OK on an alert, Done on a settings window.
    pub fn single(label: &'a str) -> Self {
        Self {
            cancel: None,
            ..Self::commit(label)
        }
    }
    /// Only Cancel, for work in progress.
    pub fn cancel_only() -> Self {
        Self {
            commit: None,
            ..Self::commit("")
        }
    }
    /// Names the cancelling button for what it does, such as Deny.
    pub fn cancel_label(mut self, label: &'a str) -> Self {
        self.cancel = Some(label);
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.commit_enabled = enabled;
        self
    }
    /// Adds a button that throws work away, such as Discard changes.
    pub fn destructive(mut self, label: &'a str) -> Self {
        self.destructive = Some(label);
        self
    }
}

/// What a dialog footer's buttons did this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FooterResponse {
    pub commit: bool,
    pub cancel: bool,
    pub destructive: bool,
}

/// The width the commit and cancel buttons have at least, so short labels such as OK still
/// make a comfortable target and the pair lines up from dialog to dialog.
const FOOTER_BUTTON_WIDTH: f32 = 76.0;

/// Draws a dialog's button row in the one layout every dialog uses:
///
/// `[Destructive] [left …]                    [Cancel] [Commit]`
///
/// - The commit button is the default (accent) button, rightmost, with Cancel on its left. This is
///   the order KDE and GNOME use, and macOS too, so it is where Linux and Mac users look, and it
///   is the order most of Xuan's dialogs already had.
/// - A destructive button (Discard changes) goes at the far left, away from the commit button so
///   neither is clicked for the other, and is drawn with its label in the error colour.
/// - `left` adds anything else after it: a Preview checkbox, an estimate, a secondary choice such
///   as Always Allow or Restore Defaults.
///
/// Draw it in [`Window::show_with_footer`]'s footer, which puts the same rule and gap above every
/// dialog's buttons and keeps them out of the scrolled body.
///
/// Commit labels:
/// - **Apply** for edits with a live preview, and for a form whose values take effect only when
///   it is committed (adjustments, filters, Layer Effects, Text, Color Range, Grid).
/// - **Done** for settings windows, where each change takes effect as it is made; they have no
///   Cancel.
/// - **A specific verb** when one says what happens: Create canvas, Resize, Export…, Save,
///   Reassign, Run, Allow, Install, Download, Import, Insert as layer.
/// - **OK** only on alerts that just report something.
pub fn dialog_footer(
    ui: &mut Ui,
    buttons: FooterButtons,
    left: impl FnOnce(&mut Ui),
) -> FooterResponse {
    let mut response = FooterResponse::default();
    let size = vec2(FOOTER_BUTTON_WIDTH, 22.0);
    ui.horizontal(|ui| {
        if let Some(label) = buttons.destructive {
            response.destructive = ui.add(Button::new(label).destructive()).clicked();
        }
        left(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(label) = buttons.commit {
                response.commit = ui
                    .add_enabled(
                        buttons.commit_enabled,
                        Button::new(label).primary().min_size(size),
                    )
                    .clicked();
            }
            if let Some(label) = buttons.cancel {
                response.cancel = ui.add(Button::new(label).min_size(size)).clicked();
            }
        });
    });
    response
}

const TITLE_HEIGHT: f32 = 32.0;
/// The space round a dialog's body and footer.
const DIALOG_INSET: i8 = 24;
/// How much of the right inset the body's scroll bar may use.
const SCROLL_GUTTER: i8 = 12;
/// The gap on either side of the rule above a dialog's buttons.
const FOOTER_GAP: f32 = 12.0;
/// A footer's height before it has been measured: the rule, its gaps, a button row, the inset.
const FOOTER_GUESS: f32 = 2.0 * FOOTER_GAP + 22.0 + DIALOG_INSET as f32;
const MIN_BODY_HEIGHT: f32 = 60.0;
/// The space a dialog keeps from the window's edges.
const DIALOG_MARGIN: f32 = 8.0;

/// Where dialogs may sit: the window below the title and menu bar, less a margin.
pub fn dialog_bounds(ctx: &egui::Context) -> Rect {
    let content = ctx.content_rect();
    let top = (content.top() + TITLE_HEIGHT + DIALOG_MARGIN).min(content.bottom());
    Rect::from_min_max(
        pos2(content.left() + DIALOG_MARGIN, top),
        pos2(
            content.right() - DIALOG_MARGIN,
            (content.bottom() - DIALOG_MARGIN).max(top),
        ),
    )
}

/// Fades the body's edges into the window where more of it is scrolled out of sight.
fn overflow_fades<R>(ui: &Ui, output: &egui::scroll_area::ScrollAreaOutput<R>) {
    let view = output.inner_rect;
    let hidden = output.content_size.y - view.height();
    if hidden <= 0.5 {
        return;
    }
    let fill = ui.visuals().window_fill;
    let clear = fill.gamma_multiply(0.0);
    let offset = output.state.offset.y;
    let depth = 24.0_f32.min(view.height() / 3.0);
    // The bar's gutter stays clear so the bar shows over the fade.
    let right = view.right() - ui.spacing().scroll.bar_width - 2.0;
    if offset < hidden - 0.5 {
        gradient(
            ui,
            Rect::from_min_max(
                pos2(view.left(), view.bottom() - depth),
                pos2(right, view.bottom()),
            ),
            0.0,
            clear,
            fill,
        );
    }
    if offset > 0.5 {
        gradient(
            ui,
            Rect::from_min_max(view.left_top(), pos2(right, view.top() + depth)),
            0.0,
            fill,
            clear,
        );
    }
}

pub fn palette(ui: &mut Ui, foreground: &mut [u8; 4], background: &mut [u8; 4]) {
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 40.0), Sense::hover());
    for (offset, color, label) in [
        (
            vec2(12.0, 12.0),
            background as &mut [u8; 4],
            tr("Background color"),
        ),
        (
            vec2(0.0, 0.0),
            foreground as &mut [u8; 4],
            tr("Foreground color"),
        ),
    ] {
        let swatch = Rect::from_min_size(rect.min + offset, vec2(24.0, 24.0));
        let response = ui
            .interact(swatch, ui.id().with(label), Sense::click())
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::ColorButton, ui.is_enabled(), label)
        });
        ui.painter().rect_filled(swatch, 6.0, Color32::BLACK);
        ui.painter()
            .rect_filled(swatch.shrink(1.0), 5.0, Color32::WHITE);
        ui.painter().rect_filled(
            swatch.shrink(2.5),
            3.5,
            Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]),
        );
        egui::Popup::menu(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.label(label);
                color_picker(ui, color);
            });
    }
    let swap_rect = Rect::from_min_size(rect.min + vec2(26.0, -4.0), vec2(13.0, 13.0));
    let swap = ui
        .interact(swap_rect, ui.id().with("swap_colors"), Sense::click())
        .on_hover_text(tr("Swap colors (X)"));
    let c = swap_rect.center();
    let stroke = Stroke::new(1.0_f32, ui.palette().muted);
    ui.painter().add(egui::Shape::line(
        vec![
            c + vec2(-4.0, -2.0),
            c + vec2(3.0, -2.0),
            c + vec2(1.0, -4.0),
        ],
        stroke,
    ));
    ui.painter().add(egui::Shape::line(
        vec![c + vec2(4.0, 2.0), c + vec2(-3.0, 2.0), c + vec2(-1.0, 4.0)],
        stroke,
    ));
    if swap.clicked() {
        std::mem::swap(foreground, background);
    }
    let reset_rect = Rect::from_min_size(rect.min + vec2(-1.0, 27.0), vec2(12.0, 12.0));
    let reset = ui
        .interact(reset_rect, ui.id().with("reset_colors"), Sense::click())
        .on_hover_text(tr("Default colors (D)"));
    ui.painter().rect_filled(
        reset_rect.shrink(3.0).translate(vec2(1.5, 1.5)),
        1.0,
        Color32::WHITE,
    );
    ui.painter().rect(
        reset_rect.shrink(3.0).translate(vec2(-1.5, -1.5)),
        1.0,
        Color32::BLACK,
        stroke,
        StrokeKind::Inside,
    );
    if reset.clicked() {
        *foreground = [0, 0, 0, 255];
        *background = [255; 4];
    }
}

pub fn menu_choice<T: PartialEq>(
    ui: &mut Ui,
    value: &mut T,
    option: T,
    label: impl ToString,
) -> Response {
    let response = selectable_value(ui, value, option, label);
    if response.clicked() {
        ui.close();
    }
    response
}

pub fn selectable_value<T: PartialEq>(
    ui: &mut Ui,
    value: &mut T,
    option: T,
    label: impl ToString,
) -> Response {
    let label = label.to_string();
    let selected = *value == option;
    let galley =
        ui.painter()
            .layout_no_wrap(label.clone(), FontId::proportional(12.0), ui.palette().text);
    let (rect, mut response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(galley.size().x + 32.0), 22.0),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            &label,
        )
    });
    let p = ui.palette();
    let highlighted = response.hovered() || response.has_focus();
    let text = if highlighted {
        ui.painter().rect_filled(rect, 4.0, p.accent);
        p.on_accent_text
    } else {
        p.text
    };
    if selected {
        let center = pos2(rect.left() + 10.0, rect.center().y);
        ui.painter().add(egui::Shape::line(
            vec![
                center + vec2(-3.0, 0.0),
                center + vec2(-1.0, 2.5),
                center + vec2(4.0, -3.0),
            ],
            Stroke::new(1.3_f32, text),
        ));
    }
    ui.painter().galley_with_override_text_color(
        pos2(rect.left() + 23.0, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    if response.clicked() && !selected {
        *value = option;
        response.mark_changed();
    }
    response
}

/// A menu toggle: a check mark when `checked`, the label, and a right-aligned shortcut hint. Its
/// accessible label is "<label> <shortcut>", like other menu items.
pub fn menu_check(ui: &mut Ui, checked: bool, label: &str, shortcut: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::proportional(12.0),
        ui.palette().text,
    );
    let hint = ui.painter().layout_no_wrap(
        shortcut.to_owned(),
        FontId::proportional(12.0),
        ui.palette().muted,
    );
    let gap = if shortcut.is_empty() { 0.0 } else { 24.0 };
    let width = galley.size().x + hint.size().x + 32.0 + gap;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width().max(width), 22.0), Sense::click());
    let accessible = if shortcut.is_empty() {
        label.to_owned()
    } else {
        format!("{label} {shortcut}")
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            checked,
            &accessible,
        )
    });
    let enabled = ui.is_enabled();
    let p = ui.palette();
    let highlighted = enabled && (response.hovered() || response.has_focus());
    let (text, hint_color) = if highlighted {
        ui.painter().rect_filled(rect, 4.0, p.accent);
        (p.on_accent_text, p.on_accent_muted)
    } else if enabled {
        (p.text, p.muted)
    } else {
        (p.muted, p.muted)
    };
    if checked {
        let center = pos2(rect.left() + 10.0, rect.center().y);
        ui.painter().add(egui::Shape::line(
            vec![
                center + vec2(-3.0, 0.0),
                center + vec2(-1.0, 2.5),
                center + vec2(4.0, -3.0),
            ],
            Stroke::new(1.3_f32, text),
        ));
    }
    ui.painter().galley_with_override_text_color(
        pos2(rect.left() + 23.0, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    ui.painter().galley_with_override_text_color(
        pos2(
            rect.right() - 8.0 - hint.size().x,
            rect.center().y - hint.size().y / 2.0,
        ),
        hint,
        hint_color,
    );

    response
}

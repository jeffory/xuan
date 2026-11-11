//! The Curves editor: a square plot, so the identity line runs at 45°, over a faint histogram
//! of the original pixels, with input and output gradient strips along its bottom and left
//! edges and an In / Out readout of the selected point.
use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use xuan::{document::Point, effects, i18n::tr};

use super::{levels_controls, theme::PaletteExt as _};

/// The gradient strips' thickness and their gap to the plot.
const STRIP: f32 = 12.0;
const STRIP_GAP: f32 = 4.0;
/// The plot's largest side, so the dialog stays short enough for small windows.
const MAX_SIDE: f32 = 300.0;

/// Where the parts go in `width`, from `origin`: the square plot, the output strip on its left
/// and the input strip under it.
pub(super) struct Layout {
    pub plot: Rect,
    pub output: Rect,
    pub input: Rect,
}

impl Layout {
    pub(super) fn new(origin: Pos2, width: f32) -> Self {
        let side = (width - STRIP - STRIP_GAP).clamp(64.0, MAX_SIDE);
        let left = origin.x + STRIP + STRIP_GAP;
        let plot = Rect::from_min_size(pos2(left, origin.y), vec2(side, side));
        Self {
            plot,
            output: Rect::from_min_size(origin, vec2(STRIP, side)),
            input: Rect::from_min_size(pos2(left, plot.bottom() + STRIP_GAP), vec2(side, STRIP)),
        }
    }

    /// The space taken, from the output strip's left to the input strip's bottom.
    pub(super) fn size(&self) -> egui::Vec2 {
        vec2(
            self.plot.right() - self.output.left(),
            self.input.bottom() - self.output.top(),
        )
    }

    /// `p` (0 to 1 on each axis) on the plot.
    pub(super) fn to_screen(&self, p: Point) -> Pos2 {
        pos2(
            self.plot.left() + p.x * self.plot.width(),
            self.plot.bottom() - p.y * self.plot.height(),
        )
    }

    /// The curve point under `pos`, clamped to the plot.
    pub(super) fn to_curve(&self, pos: Pos2) -> Point {
        Point::new(
            ((pos.x - self.plot.left()) / self.plot.width()).clamp(0.0, 1.0),
            ((self.plot.bottom() - pos.y) / self.plot.height()).clamp(0.0, 1.0),
        )
    }
}

/// A point's input and output levels, 0 to 255, as the readout shows them.
pub(super) fn levels(p: Point) -> [u8; 2] {
    [p.x, p.y].map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Black to white for the composite, black to the primary for a single channel.
fn ramp_color(channel: usize, t: f32) -> Color32 {
    let v = (t * 255.0).round() as u8;
    match channel {
        1 => Color32::from_rgb(v, 0, 0),
        2 => Color32::from_rgb(0, v, 0),
        3 => Color32::from_rgb(0, 0, v),
        _ => Color32::from_gray(v),
    }
}

/// A gradient strip from black at the start (left, or the bottom when `vertical`) to the
/// channel's full value at the end.
fn strip(ui: &Ui, rect: Rect, channel: usize, vertical: bool) {
    const STEPS: usize = 64;
    let mut mesh = egui::Mesh::default();
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let color = ramp_color(channel, t);
        let (a, b) = if vertical {
            let y = egui::lerp(rect.bottom()..=rect.top(), t);
            (pos2(rect.left(), y), pos2(rect.right(), y))
        } else {
            let x = egui::lerp(rect.x_range(), t);
            (pos2(x, rect.top()), pos2(x, rect.bottom()))
        };
        mesh.colored_vertex(a, color);
        mesh.colored_vertex(b, color);
        if i > 0 {
            let n = mesh.vertices.len() as u32;
            mesh.add_triangle(n - 4, n - 3, n - 2);
            mesh.add_triangle(n - 3, n - 1, n - 2);
        }
    }
    ui.painter().add(mesh);
    ui.painter().rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0_f32, ui.palette().plot_grid),
        egui::StrokeKind::Inside,
    );
}

/// The curve editor for `points` on `channel` (0 composite, 1 to 3 red, green, blue), over the
/// histogram of `image`. Whether the curve changed.
pub(super) fn editor(
    ui: &mut Ui,
    points: &mut Vec<Point>,
    image: &image::RgbaImage,
    channel: usize,
) -> bool {
    let mut changed = false;
    let palette = ui.palette();
    ui.label(
        egui::RichText::new(tr(
            "Click to add a point · Drag points to reshape the curve",
        ))
        .color(palette.muted),
    );
    let layout = Layout::new(ui.cursor().min, ui.available_width());
    let (_, response) = ui.allocate_exact_size(layout.size(), Sense::hover());
    let response = ui
        .interact(
            layout.plot,
            response.id.with("plot"),
            Sense::click_and_drag(),
        )
        .on_hover_cursor(egui::CursorIcon::Crosshair);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, tr("Curve")));
    // The selected point, by index, kept per channel's editor.
    let selected_id = response.id.with(("selected", channel));
    let mut selected: Option<usize> = ui.data(|d| d.get_temp(selected_id));

    let painter = ui.painter();
    let plot = layout.plot;
    painter.rect_filled(plot, 0.0, palette.plot);
    // A faint histogram of the original pixels, as Photoshop draws behind its curve.
    let bins = levels_controls::bins(image, channel);
    let peak = bins.iter().copied().fold(1.0_f32, f32::max);
    let shade = palette.levels_histogram[channel.min(3)].gamma_multiply(0.35);
    for (index, count) in bins.into_iter().enumerate() {
        let x = plot.left() + (index as f32 + 0.5) / 256.0 * plot.width();
        let height = (count / peak).sqrt() * plot.height() * 0.9;
        if height > 0.0 {
            painter.line_segment(
                [pos2(x, plot.bottom()), pos2(x, plot.bottom() - height)],
                Stroke::new(plot.width() / 256.0, shade),
            );
        }
    }
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let x = plot.left() + plot.width() * t;
        let y = plot.top() + plot.height() * t;
        let grid = Stroke::new(1.0_f32, palette.plot_grid);
        painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], grid);
        painter.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], grid);
    }
    painter.line_segment(
        [plot.left_bottom(), plot.right_top()],
        Stroke::new(1.0_f32, palette.plot_diagonal),
    );
    strip(ui, layout.output, channel, true);
    strip(ui, layout.input, channel, false);

    if let Some(pos) = response.interact_pointer_pos() {
        let point = layout.to_curve(pos);
        let near = points
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (a.x - point.x).abs().total_cmp(&(b.x - point.x).abs()))
            .map(|(i, p)| (i, (p.x - point.x).abs()));
        if response.clicked() {
            match near {
                // A click on an existing point's column selects it.
                Some((index, distance)) if distance < 0.025 => selected = Some(index),
                _ if points.len() < 32 => {
                    points.push(point);
                    points.sort_by(|a, b| a.x.total_cmp(&b.x));
                    selected = points.iter().position(|p| *p == point);
                    changed = true;
                }
                _ => {}
            }
        }
        if response.dragged()
            && let Some((index, _)) = near
        {
            points[index].y = point.y;
            selected = Some(index);
            changed = true;
        }
    }
    let selected = selected.filter(|&i| i < points.len());
    ui.data_mut(|d| match selected {
        Some(index) => d.insert_temp(selected_id, index),
        None => d.remove::<usize>(selected_id),
    });

    let painter = ui.painter();
    painter.add(egui::Shape::line(
        (0..=255)
            .map(|i| {
                let x = i as f32 / 255.0;
                layout.to_screen(Point::new(x, effects::curve_value(points, x)))
            })
            .collect(),
        Stroke::new(1.5_f32, palette.text),
    ));
    for (index, p) in points.iter().enumerate() {
        let center = layout.to_screen(*p);
        if Some(index) == selected {
            painter.circle_filled(center, 4.5, palette.accent);
            painter.circle_stroke(center, 4.5, Stroke::new(1.0_f32, palette.text));
        } else {
            painter.circle_filled(center, 3.0, palette.text);
        }
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let [input, output] = selected.map_or([None, None], |i| levels(points[i]).map(Some));
        let value = |v: Option<u8>| v.map_or("–".to_owned(), |v| v.to_string());
        ui.label(format!("{} {}", tr("In"), value(input)));
        ui.add_space(12.0);
        ui.label(format!("{} {}", tr("Out"), value(output)));
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plot_is_square_with_the_strips_along_its_edges() {
        let layout = Layout::new(pos2(10.0, 20.0), 408.0);
        assert_eq!(layout.plot.width(), layout.plot.height());
        assert_eq!(layout.plot.width(), MAX_SIDE);
        assert_eq!(layout.output.left(), 10.0);
        assert_eq!(layout.output.height(), layout.plot.height());
        assert_eq!(layout.output.right() + STRIP_GAP, layout.plot.left());
        assert_eq!(layout.input.width(), layout.plot.width());
        assert_eq!(layout.input.left(), layout.plot.left());
        assert_eq!(layout.input.top(), layout.plot.bottom() + STRIP_GAP);
        assert_eq!(
            layout.size(),
            vec2(STRIP + STRIP_GAP + MAX_SIDE, MAX_SIDE + STRIP_GAP + STRIP)
        );
        // A narrow dialog shrinks the plot and keeps it square.
        let narrow = Layout::new(Pos2::ZERO, 216.0);
        assert_eq!(narrow.plot.width(), 200.0);
        assert_eq!(narrow.plot.height(), 200.0);
    }

    #[test]
    fn the_identity_runs_corner_to_corner_at_45_degrees() {
        let layout = Layout::new(Pos2::ZERO, 300.0);
        let a = layout.to_screen(Point::new(0.0, 0.0));
        let b = layout.to_screen(Point::new(1.0, 1.0));
        assert_eq!(a, layout.plot.left_bottom());
        assert_eq!(b, layout.plot.right_top());
        assert!(((b.x - a.x) - (a.y - b.y)).abs() < 1e-4);
        let p = Point::new(0.25, 0.75);
        let back = layout.to_curve(layout.to_screen(p));
        assert!((back.x - p.x).abs() < 1e-5 && (back.y - p.y).abs() < 1e-5);
    }

    #[test]
    fn readout_levels_are_rounded_to_0_255() {
        assert_eq!(levels(Point::new(0.0, 1.0)), [0, 255]);
        assert_eq!(levels(Point::new(0.5, 0.25)), [128, 64]);
    }
}

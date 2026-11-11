//! Canvas Size's anchor: a 3 × 3 grid of cells, one per edge, corner and the centre. The
//! anchor's cell holds a square and the cells around it hold arrows pointing away from it, the
//! way the canvas grows, as in Photoshop and GIMP.
use egui::{Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};
use xuan::i18n::tr;

use super::{theme::PaletteExt as _, widgets};

/// One cell, also the click target.
pub(super) const CELL: f32 = 26.0;
const GAP: f32 = 2.0;

/// The cell's name, for assistive technology and the tooltip.
fn name(x: usize, y: usize) -> &'static str {
    [
        ["Top left", "Top", "Top right"],
        ["Left", "Centre", "Right"],
        ["Bottom left", "Bottom", "Bottom right"],
    ][y][x]
}

/// The anchor's cell (0 to 2 on each axis) for an anchor of 0, 0.5 or 1 on each axis.
pub(super) fn cell_of(anchor: [f32; 2]) -> [usize; 2] {
    anchor.map(|a| (a * 2.0).round().clamp(0.0, 2.0) as usize)
}

/// The direction of the arrow in `cell`: away from the anchor's cell when it is a neighbour,
/// none for the anchor itself and the cells beyond its neighbours.
pub(super) fn arrow(cell: [usize; 2], anchor: [usize; 2]) -> Option<Vec2> {
    let dx = cell[0] as f32 - anchor[0] as f32;
    let dy = cell[1] as f32 - anchor[1] as f32;
    (dx.abs() <= 1.0 && dy.abs() <= 1.0 && (dx, dy) != (0.0, 0.0))
        .then(|| vec2(dx, dy).normalized())
}

/// The strokes of an arrow `length` long centred on `center`, pointing along the unit vector
/// `direction`: the shaft and the two sides of its head.
pub(super) fn arrow_lines(center: Pos2, direction: Vec2, length: f32) -> [[Pos2; 2]; 3] {
    let tip = center + direction * (length / 2.0);
    let tail = center - direction * (length / 2.0);
    let head = length * 0.4;
    let back = -direction * head;
    let side = direction.rot90() * head;
    [
        [tail, tip],
        [tip, tip + (back + side) * std::f32::consts::FRAC_1_SQRT_2],
        [tip, tip + (back - side) * std::f32::consts::FRAC_1_SQRT_2],
    ]
}

/// The anchor picker; `anchor` is 0, 0.5 or 1 on each axis, from the top left.
pub(super) fn anchor_picker(ui: &mut Ui, anchor: &mut [f32; 2]) -> Response {
    let size = Vec2::splat(3.0 * CELL + 2.0 * GAP);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.palette();
    ui.painter().rect_filled(rect, 5.0, p.field);
    ui.painter().rect_stroke(
        rect,
        5.0,
        Stroke::new(1.0_f32, p.widget_stroke),
        StrokeKind::Inside,
    );
    let current = cell_of(*anchor);
    let mut cells = Vec::with_capacity(9);
    for y in 0..3 {
        for x in 0..3 {
            let min = rect.min + vec2(x as f32, y as f32) * (CELL + GAP);
            let cell = Rect::from_min_size(min, Vec2::splat(CELL));
            let cell_response = ui
                .interact(cell, response.id.with((x, y)), Sense::click())
                .on_hover_text(tr(name(x, y)));
            if cell_response.clicked() && current != [x, y] {
                *anchor = [x as f32 * 0.5, y as f32 * 0.5];
                response.mark_changed();
            }
            cells.push(([x, y], cell, cell_response));
        }
    }
    // Paint after the clicks, so the new anchor shows at once.
    let current = cell_of(*anchor);
    let enabled = ui.is_enabled();
    let ink = widgets::label_color(ui, enabled, p.text);
    for (index, cell, cell_response) in cells {
        let selected = index == current;
        cell_response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                enabled,
                selected,
                tr(name(index[0], index[1])),
            )
        });
        if cell_response.hovered() && !selected {
            ui.painter().rect_filled(cell.shrink(1.0), 4.0, p.hover);
        }
        if selected {
            ui.painter().rect_filled(
                Rect::from_center_size(cell.center(), Vec2::splat(10.0)),
                2.0,
                p.accent,
            );
        } else if let Some(direction) = arrow(index, current) {
            for line in arrow_lines(cell.center(), direction, 12.0) {
                ui.painter().line_segment(line, Stroke::new(1.5_f32, ink));
            }
        }
        widgets::focus_ring(ui, &cell_response, 4.0);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_follow_the_anchor_fractions() {
        assert_eq!(cell_of([0.0, 0.0]), [0, 0]);
        assert_eq!(cell_of([0.5, 1.0]), [1, 2]);
        assert_eq!(cell_of([1.0, 0.5]), [2, 1]);
    }

    #[test]
    fn arrows_point_away_from_the_anchor_from_its_neighbours_only() {
        // Centre anchor: all eight neighbours point outwards.
        assert_eq!(arrow([1, 1], [1, 1]), None);
        assert_eq!(arrow([2, 1], [1, 1]), Some(vec2(1.0, 0.0)));
        assert_eq!(arrow([1, 0], [1, 1]), Some(vec2(0.0, -1.0)));
        let diagonal = arrow([0, 2], [1, 1]).unwrap();
        assert!((diagonal - vec2(-1.0, 1.0).normalized()).length() < 1e-6);
        // Top-left anchor: three arrows, and the far cells stay empty.
        let corner: Vec<_> = (0..9)
            .filter_map(|i| arrow([i % 3, i / 3], [0, 0]))
            .collect();
        assert_eq!(corner.len(), 3);
        assert_eq!(arrow([2, 2], [0, 0]), None);
        assert_eq!(arrow([2, 0], [0, 0]), None);
    }

    #[test]
    fn arrow_lines_are_centred_with_the_head_at_the_tip() {
        let [shaft, left, right] = arrow_lines(Pos2::new(10.0, 10.0), vec2(1.0, 0.0), 12.0);
        assert_eq!(shaft, [Pos2::new(4.0, 10.0), Pos2::new(16.0, 10.0)]);
        // Both sides start at the tip, run back and spread symmetrically.
        assert_eq!(left[0], shaft[1]);
        assert_eq!(right[0], shaft[1]);
        assert!(left[1].x < 16.0 && right[1].x < 16.0);
        assert!((left[1].x - right[1].x).abs() < 1e-5);
        assert!(((left[1].y - 10.0) + (right[1].y - 10.0)).abs() < 1e-5);
        assert!((left[1].y - right[1].y).abs() > 1.0);
    }
}

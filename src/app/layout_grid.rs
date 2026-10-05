//! View → Show → Grid: the non-printing layout grid over the document.
//!
//! Drawn as Compositor's `TransformOverlay.drawLayoutGrid`: majors in the chosen color, style and
//! opacity, subdivisions dotted and fainter, and subdivisions left out once they would be closer
//! than 4 points. It is separate from the pixel grid (`pixel_grid.rs`) and is drawn after it,
//! with lines aligned to the same physical pixels, so where both mark a pixel boundary the layout
//! line covers the pixel line instead of doubling it.
use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, pos2};
use xuan::layout::GridSettings;

use super::pixel_grid::align_to_pixel;

/// Subdivisions closer than this, in points, are not drawn.
const MIN_SUBDIVISION_GAP: f32 = 4.0;
/// Most dashes or dots drawn in one frame; past it, patterned lines are drawn solid at a matching
/// strength so a dense grid stays cheap.
const MAX_DASHES: usize = 40_000;

/// One grid line on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScreenLine {
    /// Vertical (at an X) or horizontal (at a Y).
    pub vertical: bool,
    /// Leading edge, aligned to a physical pixel.
    pub at: f32,
    pub major: bool,
}

/// The grid lines that show inside `visible` for an image at `origin` and `zoom`.
pub(super) fn screen_lines(
    grid: &GridSettings,
    origin: Pos2,
    zoom: f32,
    image: [u32; 2],
    visible: Rect,
    pixels_per_point: f32,
) -> Vec<ScreenLine> {
    if !(zoom.is_finite() && zoom > 0.0 && visible.is_positive()) {
        return Vec::new();
    }
    let subdivisions = grid.step() * zoom >= MIN_SUBDIVISION_GAP;
    let mut lines = Vec::new();
    for (vertical, start, end, origin, length) in [
        (true, visible.left(), visible.right(), origin.x, image[0]),
        (false, visible.top(), visible.bottom(), origin.y, image[1]),
    ] {
        let from = (start - origin) / zoom;
        let to = (end - origin) / zoom;
        let values = if subdivisions {
            grid.lines(from, to, length as f32)
        } else {
            // Majors only: a grid whose subdivisions are one.
            let majors = GridSettings {
                subdivisions: 1,
                ..*grid
            };
            if grid.spacing as f32 * zoom < 1.0 {
                // Lines closer than a point would fill the canvas.
                Vec::new()
            } else {
                majors.lines(from, to, length as f32)
            }
        };
        lines.extend(values.into_iter().map(|value| ScreenLine {
            vertical,
            at: align_to_pixel(origin + value * zoom, pixels_per_point),
            major: grid.is_major(value),
        }));
    }
    lines
}

/// Paints the grid over the document, clipped to `visible`.
pub(super) fn paint(
    painter: &Painter,
    grid: &GridSettings,
    origin: Pos2,
    zoom: f32,
    image: [u32; 2],
    visible: Rect,
) {
    let ppp = painter.pixels_per_point();
    let lines = screen_lines(grid, origin, zoom, image, visible, ppp);
    if lines.is_empty() {
        return;
    }
    let [r, g, b] = grid.rgb();
    let color =
        |alpha: f32| Color32::from_rgba_unmultiplied(r, g, b, (alpha * 255.0).round() as u8);
    let thickness = 1.0 / ppp.max(f32::MIN_POSITIVE);
    let (major_pattern, minor_pattern) = (grid.style.dashes(), Some([1.0, 2.0]));
    let extent = |line: &ScreenLine| {
        if line.vertical {
            (visible.top(), visible.bottom())
        } else {
            (visible.left(), visible.right())
        }
    };
    // How many dashes the patterns would take; too many and lines are drawn solid.
    let dashes: usize = lines
        .iter()
        .map(|line| {
            let pattern = if line.major {
                major_pattern
            } else {
                minor_pattern
            };
            pattern.map_or(0, |[on, off]| {
                let (a, b) = extent(line);
                ((b - a) / (on + off)) as usize + 1
            })
        })
        .sum();
    let patterned = dashes <= MAX_DASHES;
    let mut mesh = Mesh::default();
    for line in &lines {
        let (pattern, alpha) = if line.major {
            (major_pattern, grid.major_alpha())
        } else {
            (minor_pattern, grid.subdivision_alpha())
        };
        let (a, b) = extent(line);
        let rect = |from: f32, to: f32| {
            if line.vertical {
                Rect::from_min_max(pos2(line.at, from), pos2(line.at + thickness, to))
            } else {
                Rect::from_min_max(pos2(from, line.at), pos2(to, line.at + thickness))
            }
        };
        match pattern {
            Some([on, off]) if patterned => {
                let mut position = a;
                while position < b {
                    mesh.add_colored_rect(rect(position, (position + on).min(b)), color(alpha));
                    position += on + off;
                }
            }
            // Solid at the pattern's average ink.
            Some([on, off]) => mesh.add_colored_rect(rect(a, b), color(alpha * on / (on + off))),
            None => mesh.add_colored_rect(rect(a, b), color(alpha)),
        }
    }
    painter.with_clip_rect(visible).add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn grid() -> GridSettings {
        GridSettings::default()
    }

    #[test]
    fn lines_follow_the_canvas_mapping_and_mark_majors() {
        // A 200x100 image at 100%, at (10, 20), all visible: a line every 8 px.
        let visible = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));
        let lines = screen_lines(&grid(), pos2(10.0, 20.0), 1.0, [200, 100], visible, 1.0);
        let xs: Vec<_> = lines.iter().filter(|l| l.vertical).collect();
        assert_eq!(xs.len(), 200 / 8 + 1);
        assert_eq!(xs[0].at, 10.0);
        assert_eq!(xs[8].at, 10.0 + 64.0);
        assert!(xs[8].major && !xs[7].major);
        assert_eq!(lines.iter().filter(|l| !l.vertical).count(), 100 / 8 + 1);
    }

    #[test]
    fn subdivisions_drop_out_when_zoomed_out() {
        let visible = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        // At 25% an 8 px step is 2 points apart: only the 64 px majors remain.
        let lines = screen_lines(&grid(), Pos2::ZERO, 0.25, [2048, 64], visible, 1.0);
        assert!(lines.iter().all(|l| l.major));
        assert_eq!(lines.iter().filter(|l| l.vertical).count(), 2048 / 64 + 1);
        // At 1% even the majors would be under a point apart.
        assert!(screen_lines(&grid(), Pos2::ZERO, 0.01, [2048, 64], visible, 1.0).is_empty());
    }

    #[test]
    fn lines_share_physical_pixels_with_the_pixel_grid() {
        // At 1000% and 1.5 pixels per point, the layout line on pixel boundary 8 must cover the
        // very physical pixel the pixel grid draws there.
        let origin = pos2(3.3, 7.7);
        let visible = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0));
        let layout = screen_lines(&grid(), origin, 10.0, [32, 32], visible, 1.5);
        let pixel = super::super::pixel_grid::grid_lines(
            origin,
            egui::Vec2::splat(10.0),
            [32, 32],
            visible,
            1.5,
        );
        let line = layout.iter().find(|l| l.vertical && !l.major).unwrap();
        assert!(
            pixel.xs.contains(&line.at),
            "{} not in {:?}",
            line.at,
            pixel.xs
        );
    }

    #[test]
    fn only_the_visible_part_is_listed() {
        let visible = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        // A large image scrolled so pixels 1000..1100 are on screen.
        let lines = screen_lines(
            &grid(),
            pos2(-1000.0, -1000.0),
            1.0,
            [30_000, 30_000],
            visible,
            1.0,
        );
        assert!(lines.len() <= 2 * (100 / 8 + 2));
        assert!(lines.iter().all(|l| (0.0..=100.0).contains(&l.at)));
    }
}

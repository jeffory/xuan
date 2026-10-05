//! Pixel grid overlay: lines along image pixel boundaries once zoomed in far enough.
//!
//! The overlay is a screen-space decoration painted over the canvas. It is never
//! part of the document, so exports and copies cannot contain it. Placement is a
//! pure function of the image→screen mapping (`origin` + `scale`), so it can be
//! tested without a UI.
use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Vec2, pos2};

/// The grid reaches full strength this far (relative to the threshold) above it.
const FADE_SPAN: f32 = 0.25;
/// Opacity of a fully faded-in line: a mid-grey that reads on light and dark pixels.
const MAX_ALPHA: f32 = 96.0;

/// Positions of the grid lines for one image, in screen points.
///
/// Every value is the leading edge of a line exactly one physical pixel thick,
/// aligned to the physical pixel grid, so the line is crisp and covers the exact
/// image pixel boundary (the boundary lies inside the line's pixel).
#[derive(Debug, Default, PartialEq)]
pub(super) struct GridLines {
    /// Left edges of vertical lines.
    pub xs: Vec<f32>,
    /// Top edges of horizontal lines.
    pub ys: Vec<f32>,
    /// Line thickness: one physical pixel.
    pub thickness: f32,
}

/// 0 below the threshold, rising linearly to 1 a quarter of the threshold above
/// it. Both values are zoom percentages, in the same units.
pub(super) fn grid_strength(zoom_percent: f32, threshold_percent: u32) -> f32 {
    let threshold = threshold_percent as f32;
    if zoom_percent.is_nan() || zoom_percent < threshold || threshold <= 0.0 {
        return 0.0;
    }
    ((zoom_percent - threshold) / (threshold * FADE_SPAN)).clamp(0.0, 1.0)
}

/// Interior pixel boundaries of an `image`-sized grid that fall inside `clip`.
///
/// `origin` is the screen position of the image's top-left corner and `scale`
/// the size of one image pixel in points. The image's outer edges are left to
/// the canvas outline. Returns nothing when `scale` is not positive.
pub(super) fn grid_lines(
    origin: Pos2,
    scale: Vec2,
    image: [u32; 2],
    clip: Rect,
    pixels_per_point: f32,
) -> GridLines {
    let ppp = pixels_per_point;
    let axis = |origin: f32, scale: f32, count: u32, min: f32, max: f32| -> Vec<f32> {
        if scale.is_nan() || scale <= 0.0 || ppp.is_nan() || ppp <= 0.0 || count < 2 {
            return Vec::new();
        }
        let first = ((min - origin) / scale).ceil().max(1.0);
        let last = ((max - origin) / scale).floor().min((count - 1) as f32);
        if last.is_nan() || last < first {
            return Vec::new();
        }
        (first as u32..=last as u32)
            .map(|i| ((origin + i as f32 * scale) * ppp).floor() / ppp)
            .collect()
    };
    GridLines {
        xs: axis(origin.x, scale.x, image[0], clip.left(), clip.right()),
        ys: axis(origin.y, scale.y, image[1], clip.top(), clip.bottom()),
        thickness: 1.0 / ppp.max(f32::MIN_POSITIVE),
    }
}

/// Paint the grid across `extent` (normally the visible part of the image) with
/// one mesh, however many lines there are.
pub(super) fn paint_grid(painter: &Painter, lines: &GridLines, extent: Rect, strength: f32) {
    let alpha = (MAX_ALPHA * strength.clamp(0.0, 1.0)).round() as u8;
    if alpha == 0 || !extent.is_positive() || (lines.xs.is_empty() && lines.ys.is_empty()) {
        return;
    }
    let color = Color32::from_rgba_unmultiplied(128, 128, 128, alpha);
    let mut mesh = Mesh::default();
    for &x in &lines.xs {
        mesh.add_colored_rect(
            Rect::from_min_max(
                pos2(x, extent.top()),
                pos2(x + lines.thickness, extent.bottom()),
            ),
            color,
        );
    }
    for &y in &lines.ys {
        mesh.add_colored_rect(
            Rect::from_min_max(
                pos2(extent.left(), y),
                pos2(extent.right(), y + lines.thickness),
            ),
            color,
        );
    }
    painter.with_clip_rect(extent).add(Shape::mesh(mesh));
}

/// Draw the grid for an image mapped to `origin`/`scale`, if `zoom_percent` is
/// past `threshold_percent`.
pub(super) fn draw(
    painter: &Painter,
    origin: Pos2,
    scale: Vec2,
    image: [u32; 2],
    visible: Rect,
    zoom_percent: f32,
    threshold_percent: u32,
) {
    let strength = grid_strength(zoom_percent, threshold_percent);
    if strength > 0.0 && visible.is_positive() {
        let lines = grid_lines(origin, scale, image, visible, painter.pixels_per_point());
        paint_grid(painter, &lines, visible, strength);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn lines(origin: Pos2, zoom: f32, image: [u32; 2], clip: Rect, ppp: f32) -> GridLines {
        grid_lines(origin, Vec2::splat(zoom), image, clip, ppp)
    }

    #[test]
    fn threshold_switches_the_grid_on_and_fades_it_in() {
        assert_eq!(grid_strength(499.0, 500), 0.0);
        assert_eq!(grid_strength(100.0, 500), 0.0);
        assert_eq!(grid_strength(500.0, 500), 0.0);
        let half = grid_strength(562.5, 500);
        assert!((half - 0.5).abs() < 1e-6);
        assert_eq!(grid_strength(625.0, 500), 1.0);
        assert_eq!(grid_strength(6400.0, 500), 1.0);
        assert_eq!(grid_strength(f32::NAN, 500), 0.0);
        // A higher threshold hides the grid at zooms where a lower one shows it.
        assert!(grid_strength(800.0, 500) > 0.0);
        assert_eq!(grid_strength(800.0, 1600), 0.0);
        // Monotonic through the fade.
        let mut previous = 0.0;
        for zoom in (500..=700).step_by(10) {
            let s = grid_strength(zoom as f32, 500);
            assert!(s >= previous);
            previous = s;
        }
    }

    #[test]
    fn lines_sit_on_every_interior_pixel_boundary_at_unit_scale() {
        let clip = Rect::from_min_size(pos2(100.0, 50.0), vec2(40.0, 30.0));
        let grid = lines(pos2(100.0, 50.0), 5.0, [8, 6], clip, 1.0);
        assert_eq!(
            grid.xs,
            vec![105.0, 110.0, 115.0, 120.0, 125.0, 130.0, 135.0]
        );
        assert_eq!(grid.ys, vec![55.0, 60.0, 65.0, 70.0, 75.0]);
        assert_eq!(grid.thickness, 1.0);
    }

    #[test]
    fn only_visible_lines_are_produced() {
        let origin = pos2(-1000.0, -2000.0);
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 60.0));
        let grid = lines(origin, 10.0, [1000, 1000], clip, 1.0);
        // x = -1000 + 10 i in 0..=100 -> i = 100..=110.
        assert_eq!(grid.xs.len(), 11);
        assert_eq!(grid.xs.first(), Some(&0.0));
        assert_eq!(grid.xs.last(), Some(&100.0));
        // y = -2000 + 10 i in 0..=60 -> i = 200..=206.
        assert_eq!(grid.ys.len(), 7);
        // A huge image never produces more than the viewport can show.
        let big = lines(origin, 10.0, [u32::MAX, u32::MAX], clip, 1.0);
        assert_eq!((big.xs.len(), big.ys.len()), (11, 7));
        // Fully off screen, or an image without interior boundaries.
        let off = Rect::from_min_size(pos2(5000.0, 5000.0), vec2(10.0, 10.0));
        let none = lines(origin, 10.0, [10, 10], off, 1.0);
        assert!(none.xs.is_empty() && none.ys.is_empty());
        assert!(lines(origin, 10.0, [1, 1], clip, 1.0).xs.is_empty());
    }

    #[test]
    fn outer_image_edges_are_not_part_of_the_grid() {
        let clip = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        let grid = lines(pos2(10.0, 10.0), 10.0, [3, 3], clip, 1.0);
        assert_eq!(grid.xs, vec![20.0, 30.0]);
        assert_eq!(grid.ys, vec![20.0, 30.0]);
    }

    #[test]
    fn snapping_covers_the_exact_boundary_at_any_pan_zoom_and_scale() {
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            for zoom in [2.0_f32, 5.0, 7.3, 16.0, 64.0] {
                for pan in [0.0_f32, 0.25, 0.5, 13.37, -77.9] {
                    let origin = pos2(31.0 + pan, 17.0 - pan * 0.5);
                    let clip = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0));
                    let grid = lines(origin, zoom, [4000, 3000], clip, ppp);
                    assert!(!grid.xs.is_empty() && !grid.ys.is_empty());
                    let t = grid.thickness;
                    assert!((t * ppp - 1.0).abs() < 1e-6);
                    let check = |edges: &[f32], origin: f32| {
                        let mut previous = None;
                        for &edge in edges {
                            // Aligned to the physical pixel grid.
                            let physical = edge * ppp;
                            assert!(
                                (physical - physical.round()).abs() < 1e-3,
                                "ppp {ppp} edge {edge}"
                            );
                            // The true boundary lies inside the one-pixel line.
                            let index = ((edge + 0.5 * t - origin) / zoom).round();
                            let exact = origin + index * zoom;
                            assert!(
                                exact >= edge - 1e-3 && exact < edge + t + 1e-3,
                                "ppp {ppp} zoom {zoom} edge {edge} exact {exact}"
                            );
                            if let Some(p) = previous {
                                assert!(edge > p, "lines must not collapse onto each other");
                            }
                            previous = Some(edge);
                        }
                    };
                    check(&grid.xs, origin.x);
                    check(&grid.ys, origin.y);
                }
            }
        }
    }

    #[test]
    fn non_square_scale_and_degenerate_input_are_handled() {
        let clip = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        let grid = grid_lines(pos2(0.0, 0.0), vec2(10.0, 25.0), [20, 20], clip, 1.0);
        assert_eq!(grid.xs.len(), 10);
        assert_eq!(grid.ys, vec![25.0, 50.0, 75.0, 100.0]);
        for bad in [0.0, -3.0, f32::NAN] {
            let none = grid_lines(pos2(0.0, 0.0), Vec2::splat(bad), [20, 20], clip, 1.0);
            assert!(none.xs.is_empty() && none.ys.is_empty());
        }
        assert!(
            lines(pos2(0.0, 0.0), 5.0, [20, 20], clip, 0.0)
                .xs
                .is_empty()
        );
    }
}

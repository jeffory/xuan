//! The Pencil's pixel-exact cursor outline and the `[` / `]` size keys.

use super::ui::UiTest;
use super::*;
use crate::app::canvas::pencil_outline;
use egui::{Key, Pos2, pos2};
use xuan::document::Point;

fn key_sizes(tool: Tool, start: f32, key: Key, presses: usize) -> Vec<f32> {
    let mut ui = UiTest::with_document();
    ui.app_mut().set_tool(tool);
    ui.app_mut().brush.diameter = start;
    let mut sizes = Vec::new();
    for _ in 0..presses {
        ui.key(key);
        sizes.push(ui.app().brush.diameter);
    }
    sizes
}

#[test]
fn bracket_keys_take_the_pencil_down_to_one_pixel_and_stop() {
    let down = key_sizes(Tool::Pencil, 3.0, Key::OpenBracket, 4);
    assert_eq!(down, [2.0, 1.0, 1.0, 1.0]);
}

#[test]
fn bracket_keys_step_the_pencil_back_up_one_pixel_at_a_time() {
    let up = key_sizes(Tool::Pencil, 1.0, Key::CloseBracket, 4);
    assert_eq!(up[..3], [2.0, 3.0, 4.0]);
    assert!(up.windows(2).all(|pair| pair[1] > pair[0]));
}

#[test]
fn brush_keys_keep_the_geometric_steps_and_also_reach_one_pixel() {
    assert_eq!(
        key_sizes(Tool::Brush, 100.0, Key::OpenBracket, 1),
        [87.0]
    );
    assert_eq!(
        key_sizes(Tool::Brush, 100.0, Key::CloseBracket, 1),
        [115.0]
    );
    assert_eq!(
        key_sizes(Tool::Brush, 3.0, Key::OpenBracket, 3),
        [2.0, 1.0, 1.0]
    );
}

#[test]
fn size_slider_range_matches_the_keys() {
    // The Size field in the tool options accepts 1..=2000, as the keys do.
    use crate::app::commands::{larger_diameter, smaller_diameter};
    assert_eq!(smaller_diameter(1.0), 1.0);
    assert_eq!(larger_diameter(2000.0), 2000.0);
}

fn bounds(outline: &[Pos2]) -> (Pos2, Pos2) {
    let min = outline.iter().fold(pos2(f32::MAX, f32::MAX), |a, p| {
        pos2(a.x.min(p.x), a.y.min(p.y))
    });
    let max = outline.iter().fold(pos2(f32::MIN, f32::MIN), |a, p| {
        pos2(a.x.max(p.x), a.y.max(p.y))
    });
    (min, max)
}

#[test]
fn square_outline_covers_the_dab_pixels_for_odd_and_even_sizes() {
    let origin = pos2(100.0, 50.0);
    let zoom = 8.0;
    // Odd: centred on the pixel under the pointer (document pixel 5, 7).
    let odd = pencil_outline(3, true, Point::new(5.3, 7.9), origin, zoom);
    let (min, max) = bounds(&odd);
    assert_eq!((min, max), (pos2(100.0 + 4.0 * 8.0, 50.0 + 6.0 * 8.0), pos2(100.0 + 7.0 * 8.0, 50.0 + 9.0 * 8.0)));
    assert_eq!(odd.len(), 4);
    // Even: centred on the nearest pixel corner (6, 8).
    let even = pencil_outline(4, true, Point::new(5.6, 7.7), origin, zoom);
    let (min, max) = bounds(&even);
    assert_eq!((min, max), (pos2(100.0 + 4.0 * 8.0, 50.0 + 6.0 * 8.0), pos2(100.0 + 8.0 * 8.0, 50.0 + 10.0 * 8.0)));
    // One pixel.
    let one = pencil_outline(1, true, Point::new(0.2, 0.2), origin, zoom);
    assert_eq!(bounds(&one), (origin, origin + egui::vec2(8.0, 8.0)));
}

#[test]
fn outline_vertices_lie_on_pixel_boundaries_and_trace_the_round_tip() {
    let origin = pos2(13.0, 21.0);
    let zoom = 6.0;
    for size in 1..=12u32 {
        for square in [true, false] {
            let pointer = Point::new(9.37, 11.62);
            let outline = pencil_outline(size, square, pointer, origin, zoom);
            for p in &outline {
                for (v, o) in [(p.x, origin.x), (p.y, origin.y)] {
                    let cell = (v - o) / zoom;
                    assert!((cell - cell.round()).abs() < 1e-4, "{size} {square}: {p:?}");
                }
            }
            // Edges are axis-aligned and the enclosed area equals the pixel count.
            let n = outline.len();
            let mut area = 0.0;
            for i in 0..n {
                let (a, b) = (outline[i], outline[(i + 1) % n]);
                assert!(a.x == b.x || a.y == b.y, "diagonal edge at size {size}");
                area += a.x * b.y - b.x * a.y;
            }
            let pixels = xuan::paint::tip_offsets(size, square).len() as f32;
            assert!((area.abs() / 2.0 / (zoom * zoom) - pixels).abs() < 1e-2);
        }
    }
    // A round 5 px dab is not a full 5x5 square: the corners are cut.
    let round = pencil_outline(5, false, Point::new(2.5, 2.5), Pos2::ZERO, 1.0);
    assert!(round.len() > 4);
    assert!(!round.contains(&pos2(0.0, 0.0)));
}

//! Image → Adjustments → Curves: a square plot and the selected point's In / Out readout.

use super::*;
use egui::vec2;

fn curves() -> UiTest {
    let mut ui = UiTest::with_document();
    ui.app_mut().command("fill_fg");
    ui.app_mut().command("curves");
    ui.settle();
    ui
}

fn plot(ui: &UiTest) -> egui::Rect {
    ui.harness.get_by_label("Curve").rect()
}

/// The composite curve's points.
fn points(ui: &UiTest) -> Vec<Point> {
    match ui
        .app()
        .effect
        .as_ref()
        .unwrap()
        .adjustment
        .as_ref()
        .unwrap()
    {
        Adjustment::CurvesChannels { channels } => channels[0].clone(),
        Adjustment::Curves { points } => points.clone(),
        other => panic!("not a curve: {other:?}"),
    }
}

#[test]
fn the_curves_plot_is_square() {
    let ui = curves();
    let rect = plot(&ui);
    assert!(rect.width() >= 200.0, "{rect:?}");
    assert!((rect.width() - rect.height()).abs() < 0.5, "{rect:?}");
    // Nothing is selected yet.
    assert!(ui.has("In –") && ui.has("Out –"));
}

#[test]
fn the_readout_shows_the_selected_points_levels() {
    let mut ui = curves();
    let rect = plot(&ui);
    let before = points(&ui).len();
    ui.click_at(rect.left_bottom() + vec2(rect.width() * 0.5, -rect.height() * 0.5));
    let added = points(&ui);
    assert_eq!(added.len(), before + 1);
    let point = *added.iter().find(|p| (p.x - 0.5).abs() < 0.02).unwrap();
    let [input, output] = crate::app::curves_controls::levels(point);
    assert!(ui.has(&format!("In {input}")), "In {input}");
    assert!(ui.has(&format!("Out {output}")), "Out {output}");

    // Dragging it up updates the readout.
    let from = rect.left_bottom() + vec2(point.x * rect.width(), -point.y * rect.height());
    let to = rect.left_bottom() + vec2(point.x * rect.width(), -rect.height() * 0.75);
    ui.drag(from, to);
    let moved = *points(&ui)
        .iter()
        .find(|p| (p.x - point.x).abs() < 1e-6)
        .unwrap();
    assert!(moved.y > 0.7, "{moved:?}");
    let [input, output] = crate::app::curves_controls::levels(moved);
    assert!(ui.has(&format!("In {input}")), "In {input}");
    assert!(ui.has(&format!("Out {output}")), "Out {output}");
}

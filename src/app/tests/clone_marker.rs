use super::*;
use crate::app::canvas::{CLONE_SOURCE_COLOR, clone_sample_position};

const SOURCE: Point = Point::new(100.0, 50.0);
const OFFSET: Point = Point::new(-30.0, 20.0);
const POINTER: Point = Point::new(10.0, 10.0);

#[test]
fn no_source_means_no_sample() {
    for aligned in [false, true] {
        for stroking in [false, true] {
            assert_eq!(
                clone_sample_position(None, Some(OFFSET), Some(POINTER), aligned, stroking),
                None
            );
        }
    }
}

#[test]
fn before_an_offset_exists_the_sample_is_the_source() {
    for aligned in [false, true] {
        assert_eq!(
            clone_sample_position(Some(SOURCE), None, Some(POINTER), aligned, false),
            Some(SOURCE)
        );
    }
}

#[test]
fn aligned_mode_follows_the_pointer_by_the_offset() {
    assert_eq!(
        clone_sample_position(Some(SOURCE), Some(OFFSET), Some(POINTER), true, false),
        Some(Point::new(-20.0, 30.0))
    );
}

#[test]
fn unaligned_mode_rests_on_the_source_and_follows_during_a_stroke() {
    assert_eq!(
        clone_sample_position(Some(SOURCE), Some(OFFSET), Some(POINTER), false, false),
        Some(SOURCE)
    );
    assert_eq!(
        clone_sample_position(Some(SOURCE), Some(OFFSET), Some(POINTER), false, true),
        Some(Point::new(-20.0, 30.0))
    );
}

#[test]
fn without_a_pointer_the_sample_is_the_source() {
    assert_eq!(
        clone_sample_position(Some(SOURCE), Some(OFFSET), None, true, false),
        Some(SOURCE)
    );
}

fn marker_shapes(output: &egui::FullOutput) -> usize {
    let solid = |color: &egui::epaint::ColorMode| matches!(color, egui::epaint::ColorMode::Solid(c) if *c == CLONE_SOURCE_COLOR);
    output
        .shapes
        .iter()
        .filter(|shape| match &shape.shape {
            egui::Shape::Path(path) => solid(&path.stroke.color),
            egui::Shape::LineSegment { stroke, .. } => stroke.color == CLONE_SOURCE_COLOR,
            _ => false,
        })
        .count()
}

#[test]
fn the_source_marker_is_drawn_only_with_the_clone_tool() {
    let (context, mut app) = app();
    app.sessions.push(Session::new(
        Document::new(200, 200).unwrap(),
        "Clone".into(),
        None,
    ));
    app.clone_source = Some(Point::new(100.0, 100.0));

    app.tool = Tool::Clone;
    frame(&context, &mut app);
    assert!(marker_shapes(&frame(&context, &mut app)) >= 3);

    app.tool = Tool::Brush;
    frame(&context, &mut app);
    assert_eq!(marker_shapes(&frame(&context, &mut app)), 0);
    assert_eq!(app.clone_source, Some(Point::new(100.0, 100.0)));

    app.tool = Tool::Clone;
    frame(&context, &mut app);
    assert!(marker_shapes(&frame(&context, &mut app)) >= 3);
}

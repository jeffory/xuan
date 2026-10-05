use super::*;
use crate::app::eyedropper::{BUBBLE_SIZE, SampleSize, SampleSource, bubble_rect};
use egui::{Rect, pos2};

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// 20x16 document, left half red and right half blue, at 8x zoom.
fn dropper_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [20, 16];
    app.new_document();
    let mut pixels = RgbaImage::new(20, 16);
    for (x, _, pixel) in pixels.enumerate_pixels_mut() {
        *pixel = image::Rgba(if x < 10 { RED } else { BLUE });
    }
    app.session_mut()
        .unwrap()
        .document
        .insert(Layer::image("Halves", pixels));
    app.set_tool(Tool::Dropper);
    let session = app.session_mut().unwrap();
    session.zoom = 8.0;
    session.fit = false;
    frame(&context, &mut app);
    (context, app)
}

fn at(app: &EditorApp, x: f32, y: f32) -> Pos2 {
    app.canvas_rect.unwrap().min + Vec2::new(x, y) * app.session().unwrap().zoom
}

fn touch(context: &egui::Context, app: &mut EditorApp, x: f32, y: f32, pressed: Option<bool>) {
    let position = at(app, x, y);
    pointer_frame(context, app, position, pressed, egui::Modifiers::NONE);
}

fn bounds() -> Rect {
    Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))
}

#[test]
fn bubble_sits_up_and_right_of_the_pointer() {
    let rect = bubble_rect(pos2(100.0, 150.0), BUBBLE_SIZE, bounds());
    assert_eq!(rect.size(), BUBBLE_SIZE);
    assert!(rect.left() > 100.0 && rect.bottom() < 150.0);
}

#[test]
fn bubble_flips_near_edges_and_stays_inside() {
    let b = bounds();
    let right = bubble_rect(pos2(395.0, 150.0), BUBBLE_SIZE, b);
    assert!(right.right() < 395.0, "flips left of the pointer");
    let top = bubble_rect(pos2(100.0, 5.0), BUBBLE_SIZE, b);
    assert!(top.top() > 5.0, "flips below the pointer");
    let corner = bubble_rect(pos2(399.0, 2.0), BUBBLE_SIZE, b);
    assert!(corner.right() < 399.0 && corner.top() > 2.0);
    for p in [
        pos2(0.0, 0.0),
        pos2(400.0, 300.0),
        pos2(400.0, 0.0),
        pos2(0.0, 300.0),
        pos2(200.0, 150.0),
    ] {
        let r = bubble_rect(p, BUBBLE_SIZE, b);
        assert!(b.contains_rect(r), "{p:?} -> {r:?}");
    }
    // A bounds smaller than the bubble does not panic.
    let tiny = Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0));
    let _ = bubble_rect(pos2(5.0, 5.0), BUBBLE_SIZE, tiny);
}

#[test]
fn press_drag_release_commits_the_live_colour() {
    let (context, mut app) = dropper_app();
    app.brush.color = [1, 2, 3, 255];
    touch(&context, &mut app, 2.5, 4.5, Some(true));
    assert_eq!(app.brush.color, RED);
    assert_eq!(app.dropper.unwrap().previous, [1, 2, 3, 255]);
    touch(&context, &mut app, 15.5, 4.5, None);
    assert_eq!(app.brush.color, BLUE, "updates live while dragging");
    assert_eq!(app.dropper.unwrap().previous, [1, 2, 3, 255]);
    touch(&context, &mut app, 15.5, 4.5, Some(false));
    assert!(app.dropper.is_none());
    assert_eq!(app.brush.color, BLUE);
    assert_eq!(app.session().unwrap().history.names().count(), 0);
}

#[test]
fn a_plain_click_samples_once() {
    let (context, mut app) = dropper_app();
    touch(&context, &mut app, 12.5, 4.5, Some(true));
    touch(&context, &mut app, 12.5, 4.5, Some(false));
    assert_eq!(app.brush.color, BLUE);
    assert!(app.dropper.is_none());
}

#[test]
fn escape_during_a_drag_restores_the_previous_colour() {
    let (context, mut app) = dropper_app();
    app.brush.color = [9, 8, 7, 255];
    let none = egui::Modifiers::NONE;
    touch(&context, &mut app, 2.5, 4.5, Some(true));
    assert_eq!(app.brush.color, RED);
    keyboard_frame(
        &context,
        &mut app,
        vec![text_key(egui::Key::Escape, none)],
        none,
    );
    assert_eq!(app.brush.color, [9, 8, 7, 255]);
    assert!(app.dropper.is_none());
    // The still-held button must not resume sampling or commit later.
    touch(&context, &mut app, 15.5, 4.5, None);
    touch(&context, &mut app, 15.5, 4.5, Some(false));
    assert_eq!(app.brush.color, [9, 8, 7, 255]);
}

#[test]
fn leaving_the_document_keeps_the_last_colour() {
    let (_, mut app) = dropper_app();
    app.dropper_press(Point::new(2.5, 4.5));
    app.dropper_update(Point::new(-5.0, 4.5));
    assert_eq!(app.brush.color, RED);
    app.dropper_update(Point::new(50.0, 4.5));
    assert_eq!(app.brush.color, RED);
}

#[test]
fn filter_layer_is_rendered_once_across_many_samples() {
    let (context, mut app) = dropper_app();
    app.session_mut()
        .unwrap()
        .document
        .layers
        .last_mut()
        .unwrap()
        .filter = Some(Filter::GaussianBlur { radius: 1.0 });
    app.session_mut().unwrap().invalidate();
    touch(&context, &mut app, 1.5, 4.5, Some(true));
    for x in 2..18 {
        touch(&context, &mut app, x as f32 + 0.5, 4.5, None);
    }
    touch(&context, &mut app, 17.5, 4.5, Some(false));
    assert_eq!(app.session().unwrap().sample_renders, 1);
    // Hovering afterwards reuses the same render.
    for x in 0..20 {
        assert!(app.sample_color(Point::new(x as f32 + 0.5, 3.5)).is_some());
    }
    assert_eq!(app.session().unwrap().sample_renders, 1);

    // A document change invalidates the cache.
    app.edit("Test edit", |doc| {
        doc.layers.last_mut().unwrap().opacity = 0.5;
        Ok(())
    });
    app.sample_color(Point::new(1.5, 1.5));
    assert_eq!(app.session().unwrap().sample_renders, 2);
    app.command("undo");
    app.sample_color(Point::new(1.5, 1.5));
    assert_eq!(app.session().unwrap().sample_renders, 3);
}

#[test]
fn documents_without_filters_never_need_a_full_render() {
    let (_, mut app) = dropper_app();
    for x in 0..20 {
        assert!(app.sample_color(Point::new(x as f32 + 0.5, 1.5)).is_some());
    }
    assert_eq!(app.session().unwrap().sample_renders, 0);
}

#[test]
fn sample_size_averages_and_source_selects_the_layer() {
    let (_, mut app) = dropper_app();
    // 3x3 on the red/blue boundary: six red pixels and three blue (x=9 | x=10).
    app.dropper_size = SampleSize::Point;
    assert_eq!(app.sample_color(Point::new(9.5, 5.5)), Some(RED));
    app.dropper_size = SampleSize::Three;
    assert_eq!(
        app.sample_color(Point::new(9.5, 5.5)),
        Some([170, 0, 85, 255])
    );
    app.dropper_size = SampleSize::Five;
    assert_eq!(
        app.sample_color(Point::new(9.5, 5.5)),
        Some([153, 0, 102, 255])
    );
    // Samples beyond the document edge are ignored rather than darkening the mean.
    assert_eq!(app.sample_color(Point::new(0.5, 0.5)), Some(RED));

    // Current layer: select the bottom (transparent background) layer.
    app.dropper_size = SampleSize::Point;
    let background = app.session().unwrap().document.layers[0].id;
    app.session_mut().unwrap().document.active = Some(background);
    app.dropper_source = SampleSource::CurrentLayer;
    assert_eq!(app.sample_color(Point::new(2.5, 2.5)), Some([0, 0, 0, 0]));
    app.dropper_source = SampleSource::AllLayers;
    assert_eq!(app.sample_color(Point::new(2.5, 2.5)), Some(RED));
}

#[test]
fn i_selects_the_eyedropper() {
    let (context, mut app) = app_with_document();
    app.set_tool(Tool::Brush);
    let none = egui::Modifiers::NONE;
    keyboard_frame(&context, &mut app, vec![text_key(egui::Key::I, none)], none);
    assert_eq!(app.tool, Tool::Dropper);
}

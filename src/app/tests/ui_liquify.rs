//! Liquify in the options bar: the Blur / Smudge / Liquify switch, the
//! Strength slider in place of Opacity, and a push on the canvas.
use super::*;

#[test]
fn the_bar_switches_to_liquify_and_a_drag_pushes_pixels() {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [80, 60];
    ui.app_mut().new_document();
    let session = ui.app_mut().session_mut().unwrap();
    session.document.active_mut().unwrap().pixels =
        Some(Arc::new(RgbaImage::from_fn(80, 60, |x, _| {
            image::Rgba(if x < 40 { [0, 0, 0, 255] } else { [255; 4] })
        })));
    session.history = History::default();
    ui.settle();
    ui.key(egui::Key::R);
    assert_eq!(ui.app().tool, Tool::Blur);
    assert!(!ui.has("Strength"));
    ui.click("Liquify");
    assert_eq!(ui.app().blur_mode, PaintMode::Liquify);
    assert!(ui.has("Strength"));
    ui.click("Smudge");
    assert_eq!(ui.app().blur_mode, PaintMode::Smudge);
    assert!(!ui.has("Strength"));
    ui.click("Liquify");

    ui.app_mut().brush.diameter = 30.0;
    ui.app_mut().brush.hardness = 0.5;
    ui.app_mut().brush.opacity = 1.0;
    let rect = ui.app().canvas_rect.unwrap();
    let zoom = ui.app().session().unwrap().zoom;
    let at = |x: f32, y: f32| rect.min + Vec2::new(x, y) * zoom;
    ui.drag(at(32.0, 30.0), at(46.0, 30.0));
    let session = ui.app().session().unwrap();
    let pixels = session.document.active().unwrap().pixels.clone().unwrap();
    assert!(
        pixels.get_pixel(43, 30)[0] < 60,
        "{:?}",
        pixels.get_pixel(43, 30)
    );
    assert_eq!(pixels.get_pixel(43, 2).0, [255; 4]);
    assert_eq!(session.history.undo_name(), Some("Liquify"));
}

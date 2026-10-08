//! Filter → Camera Raw Filter… as the user sees it: Develop's workspace on a pixel layer,
//! without the RAW-only controls, where Apply changes the layer and Cancel leaves it alone.
use super::*;
use std::time::{Duration, Instant};
use xuan::{document::Point, raw::DevelopSettings};

const MENU_ITEM: &str = "Camera Raw Filter… Ctrl+Shift+A";

/// A 40 × 30 document whose layer is a gradient with a transparent column.
fn photo() -> UiTest {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [40, 30];
    ui.app_mut().new_document();
    let session = ui.app_mut().session_mut().unwrap();
    session.document.active_mut().unwrap().pixels =
        Some(Arc::new(RgbaImage::from_fn(40, 30, |x, y| {
            image::Rgba([
                (x * 5) as u8,
                (y * 7) as u8,
                90,
                if x == 3 { 0 } else { 255 },
            ])
        })));
    session.history = History::default();
    ui.settle();
    ui
}

fn pixels(ui: &UiTest) -> Arc<RgbaImage> {
    ui.app()
        .session()
        .unwrap()
        .document
        .active()
        .unwrap()
        .pixels
        .clone()
        .unwrap()
}

/// Runs frames until `done`, while the filter's worker reads the layer or applies it.
#[track_caller]
fn wait(ui: &mut UiTest, done: impl Fn(&EditorApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(ui.app()) {
        if let Some(error) = ui.app().develop.as_ref().and_then(|d| d.error.clone()) {
            panic!("{error}");
        }
        assert!(Instant::now() < deadline, "the Camera Raw Filter timed out");
        std::thread::sleep(Duration::from_millis(5));
        ui.settle();
    }
}

/// Opens the filter from the Filter menu and waits for its preview.
fn open_filter(ui: &mut UiTest) {
    ui.open_menu("Filter");
    assert!(ui.enabled(MENU_ITEM));
    ui.click(MENU_ITEM);
    assert!(ui.app().develop.as_ref().is_some_and(|d| d.is_filter()));
    wait(ui, |app| app.develop.as_ref().is_some_and(|d| d.ready()));
    ui.settle();
}

#[test]
fn the_filter_hides_raw_only_controls_and_apply_changes_the_layer_in_one_undo_step() {
    let mut ui = photo();
    let original = pixels(&ui);
    open_filter(&mut ui);
    assert!(ui.has("Apply"));
    assert!(!ui.has("Develop"));
    assert!(!ui.has("16-bit TIFF…"));
    for raw_only in ["Negative", "Info", "Lens"] {
        assert!(!ui.has(raw_only), "{raw_only} is RAW Develop's");
    }
    for shared in [
        "Basic", "Tone", "Detail", "Optics", "Masks", "Exposure", "Clipping",
    ] {
        assert!(ui.has(shared), "{shared} is missing");
    }
    ui.click("Optics");
    assert!(ui.has("Vignetting") && ui.has("Defringe"));
    assert!(!ui.has("Distortion") && !ui.has("Straighten") && !ui.has("Uncrop"));
    // Rotating would change the layer's size: the Develop shortcut does nothing here.
    ui.app_mut().command("develop_rotate_right");
    assert_eq!(ui.app().develop.as_ref().unwrap().settings.quarter_turns, 0);

    ui.app_mut().develop.as_mut().unwrap().settings.exposure = 1.0;
    ui.settle();
    ui.click("Apply");
    wait(&mut ui, |app| app.develop.is_none());
    let result = pixels(&ui);
    assert_eq!(result.dimensions(), original.dimensions());
    // One stop brighter: sRGB(2 × linear(100)) = 138.2 of 255.
    assert_eq!(original.get_pixel(20, 0)[0], 100);
    assert_eq!(result.get_pixel(20, 0)[0], 138);
    assert_eq!(result.get_pixel(20, 0)[3], 255);
    assert_eq!(
        result.get_pixel(3, 5),
        original.get_pixel(3, 5),
        "transparent pixels stay"
    );
    let session = ui.app().session().unwrap();
    assert_eq!(session.history.undo_name(), Some("Camera Raw Filter"));
    assert_eq!(ui.app().status, "Camera Raw Filter applied");

    ui.press(egui::Modifiers::CTRL, egui::Key::Z);
    assert_eq!(pixels(&ui), original);
}

#[test]
fn cancel_leaves_the_layer_and_history_untouched() {
    let mut ui = photo();
    let original = pixels(&ui);
    ui.press(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::A);
    assert!(ui.app().develop.as_ref().is_some_and(|d| d.is_filter()));
    wait(&mut ui, |app| {
        app.develop.as_ref().is_some_and(|d| d.ready())
    });
    ui.app_mut().develop.as_mut().unwrap().settings.contrast = 60.0;
    ui.settle();
    ui.click("Cancel");
    assert!(ui.app().develop.is_none());
    assert!(Arc::ptr_eq(&pixels(&ui), &original));
    assert_eq!(ui.app().session().unwrap().history.undo_name(), None);
    assert_eq!(ui.app().status, "Camera Raw Filter cancelled");
}

#[test]
fn apply_changes_only_the_selection() {
    let mut ui = photo();
    let original = pixels(&ui);
    ui.app_mut().session_mut().unwrap().document.selection = Some(Arc::new(
        xuan::selection::rectangle(40, 30, Point::new(0.0, 0.0), Point::new(20.0, 30.0), false),
    ));
    open_filter(&mut ui);
    ui.app_mut().develop.as_mut().unwrap().settings.exposure = -1.0;
    ui.settle();
    ui.click("Apply");
    wait(&mut ui, |app| app.develop.is_none());
    let result = pixels(&ui);
    assert!(result.get_pixel(10, 10)[0] < original.get_pixel(10, 10)[0]);
    for (x, y) in [(25, 10), (39, 29), (30, 0)] {
        assert_eq!(result.get_pixel(x, y), original.get_pixel(x, y));
    }
}

#[test]
fn reset_returns_to_the_filters_neutral_settings() {
    let mut ui = photo();
    open_filter(&mut ui);
    let develop = ui.app().develop.as_ref().unwrap();
    assert_eq!(develop.settings, DevelopSettings::camera_raw_filter());
    assert_ne!(develop.settings, DevelopSettings::default());
    ui.app_mut().develop.as_mut().unwrap().settings.saturation = 40.0;
    ui.settle();
    ui.click("Reset");
    assert_eq!(
        ui.app().develop.as_ref().unwrap().settings,
        DevelopSettings::camera_raw_filter()
    );
}

#[test]
fn the_menu_item_needs_an_unlocked_pixel_layer() {
    let mut ui = photo();
    ui.open_menu("Filter");
    assert!(ui.enabled(MENU_ITEM));
    ui.key(egui::Key::Escape);
    ui.app_mut()
        .session_mut()
        .unwrap()
        .document
        .active_mut()
        .unwrap()
        .locked = true;
    ui.settle();
    ui.open_menu("Filter");
    assert!(!ui.enabled(MENU_ITEM));
    ui.key(egui::Key::Escape);
    ui.press(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::A);
    assert!(ui.app().develop.is_none());

    let mut ui = UiTest::new();
    ui.open_menu("Filter");
    assert!(!ui.enabled(MENU_ITEM), "no document");
}

//! Upstream Compositor's Vignette, Bloom / Glow, Dither and Tonal Contrast in the Filter menu,
//! the Filter dialog, filter layers and the selection.
use super::*;

fn text_shown(output: &egui::FullOutput, label: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label))
}

fn click(context: &egui::Context, app: &mut EditorApp, label: &str) {
    let at = layer_label(context, app, label) + Vec2::splat(5.0);
    pointer_frame(context, app, at, Some(true), egui::Modifiers::NONE);
    pointer_frame(context, app, at, Some(false), egui::Modifiers::NONE);
}

fn filter_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [48, 40];
    app.new_document();
    app.brush.color = [210, 140, 60, 255];
    app.command("fill_fg");
    frame(&context, &mut app);
    (context, app)
}

#[test]
fn filter_menu_lists_and_opens_the_compositor_filters() {
    let (context, mut app) = filter_app();
    click(&context, &mut app, "Filter");
    let output = frame(&context, &mut app);
    for name in ["Vignette…", "Bloom / Glow…", "Dither…", "Tonal Contrast…"] {
        assert!(
            text_shown(&output, name),
            "{name} is not in the Filter menu"
        );
    }
    click(&context, &mut app, "Bloom / Glow…");
    let edit = app.effect.as_ref().expect("the Filter dialog");
    assert_eq!(edit.filter, Some(Filter::BLOOM));
    assert!(!edit.as_layer);
}

/// Each opens its settings with upstream's defaults and, as a filter layer, adds one layer in
/// one undo step that keeps the settings.
#[test]
fn compositor_filters_become_filter_layers_with_their_settings() {
    for (filter, label) in [
        (Filter::VIGNETTE, "Midpoint"),
        (Filter::BLOOM, "Radius"),
        (Filter::Dither(Box::default()), "Style"),
        (Filter::TONAL_CONTRAST, "Midtones"),
    ] {
        let (context, mut app) = filter_app();
        let layers = app.session().unwrap().document.layers.len();
        let revision = app.session().unwrap().history.revision;
        app.start_filter_layer(filter.clone());
        // A window is laid out on its first frame and drawn on the next.
        frame(&context, &mut app);
        let output = frame(&context, &mut app);
        assert!(text_shown(&output, label), "{}: no {label}", filter.name());
        click(&context, &mut app, "Apply");
        assert!(app.dialog.is_none(), "{}", filter.name());
        let session = app.session().unwrap();
        assert_eq!(session.document.layers.len(), layers + 1);
        let layer = session.document.active().unwrap();
        assert_eq!(layer.filter.as_ref(), Some(&filter));
        assert_eq!(layer.name, filter.name());
        assert_eq!(session.history.revision, revision + 1);
        app.command("undo");
        assert_eq!(app.session().unwrap().document.layers.len(), layers);
        assert!(app.error.is_none(), "{:?}", app.error);
    }
}

/// Dither's controls follow its style: halftones have a cell size, ASCII its characters.
#[test]
fn dither_controls_follow_the_style() {
    use xuan::effects::{DitherSettings, DitherStyle};
    let (context, mut app) = filter_app();
    for (style, shown, hidden) in [
        (DitherStyle::Atkinson, "Diffusion", "Cell Size"),
        (DitherStyle::HalftoneDots, "Cell Size", "Diffusion"),
        (DitherStyle::Ascii, "Characters", "Pixel Size"),
        (DitherStyle::Scanlines, "Wobble", "Levels"),
    ] {
        app.start_filter_layer(Filter::Dither(Box::new(DitherSettings {
            style,
            ..Default::default()
        })));
        frame(&context, &mut app);
        let output = frame(&context, &mut app);
        assert!(text_shown(&output, shown), "{style:?}: no {shown}");
        assert!(!text_shown(&output, hidden), "{style:?}: {hidden}");
        keyboard_frame(
            &context,
            &mut app,
            vec![text_key(egui::Key::Escape, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        );
        assert!(app.effect.is_none());
    }
}

/// From the Filter menu the filter changes the layer's pixels inside the selection only, as
/// `effects::apply_filter` does, in one undo step.
#[test]
fn compositor_filters_apply_inside_the_selection() {
    for filter in [
        Filter::VIGNETTE,
        Filter::TONAL_CONTRAST,
        Filter::Dither(Box::default()),
    ] {
        let (context, mut app) = filter_app();
        // A ramp, so every filter has something to change.
        {
            let session = app.session_mut().unwrap();
            let layer = session.document.active_mut().unwrap();
            layer.pixels = Some(Arc::new(image::RgbaImage::from_fn(48, 40, |x, y| {
                image::Rgba([(x * 5) as u8, (y * 6) as u8, 120, 255])
            })));
            session.document.selection = Some(Arc::new(xuan::selection::rectangle(
                48,
                40,
                Point::new(0.0, 0.0),
                Point::new(24.0, 40.0),
                false,
            )));
            session.invalidate();
        }
        let original = app.session().unwrap().document.clone();
        let mut expected = original.clone();
        xuan::effects::apply_filter(&mut expected, &filter, false).unwrap();
        let revision = app.session().unwrap().history.revision;
        app.start_filter(filter.clone());
        frame(&context, &mut app);
        super::wait_for_filter_preview(&context, &mut app);
        click(&context, &mut app, "Apply");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.effect.is_some() {
            frame(&context, &mut app);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let session = app.session().unwrap();
        assert_eq!(session.history.revision, revision + 1, "{}", filter.name());
        let pixels = session.document.active().unwrap().pixels.clone().unwrap();
        assert_eq!(Some(&pixels), expected.active().unwrap().pixels.as_ref());
        let before = original.active().unwrap().pixels.clone().unwrap();
        for (x, y, pixel) in pixels.enumerate_pixels() {
            if x >= 25 {
                assert_eq!(
                    pixel,
                    before.get_pixel(x, y),
                    "{} at ({x}, {y})",
                    filter.name()
                );
            }
        }
        assert_ne!(pixels, before, "{}", filter.name());
    }
}

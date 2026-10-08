//! The Channels pane's state: targeting channels so edits change only them, channel views on
//! the canvas, Load Channel as Selection and pasting into a channel.
use super::*;
use xuan::channels::{Channel, Channels};

/// A 20 × 16 document whose layer holds different data in every channel of every pixel.
fn packed_app() -> (egui::Context, EditorApp) {
    let (context, mut app) = app();
    app.dimensions = [20, 16];
    app.new_document();
    let session = app.session_mut().unwrap();
    session.document.active_mut().unwrap().pixels = Some(Arc::new(packed()));
    session.history = History::default();
    (context, app)
}

fn packed() -> RgbaImage {
    RgbaImage::from_fn(20, 16, |x, y| {
        image::Rgba([
            (x * 12) as u8,
            (y * 15 + 4) as u8,
            ((x + y) * 7) as u8,
            (250 - x * 3 - y * 2) as u8,
        ])
    })
}

fn pixels(app: &EditorApp) -> RgbaImage {
    let layer = app.session().unwrap().document.active().unwrap();
    (**layer.pixels.as_ref().unwrap()).clone()
}

fn targets(app: &EditorApp) -> (Channels, Channels) {
    let session = app.session().unwrap();
    (session.channel_targets, session.channel_view)
}

/// Every pixel keeps `kept` channels of `before`; returns whether any other channel changed.
fn only_changed(before: &RgbaImage, after: &RgbaImage, kept: &[usize]) -> bool {
    assert_eq!(before.dimensions(), after.dimensions());
    let mut changed = false;
    for (b, a) in before.pixels().zip(after.pixels()) {
        for &c in kept {
            assert_eq!(a[c], b[c], "channel {c} changed: {b:?} → {a:?}");
        }
        changed |= b != a;
    }
    changed
}

#[test]
fn a_brush_stroke_with_only_green_targeted_changes_only_green() {
    let (context, mut app) = packed_app();
    app.command("channel_green");
    assert_eq!(
        targets(&app),
        (
            Channels::only(Channel::Green),
            Channels::only(Channel::Green)
        )
    );
    app.set_tool(Tool::Brush);
    app.brush.color = [255, 0, 255, 255];
    app.brush.opacity = 1.0;
    app.brush.diameter = 6.0;
    drag(
        &context,
        &mut app,
        Point::new(5.0, 8.0),
        Point::new(15.0, 8.0),
        egui::Modifiers::NONE,
    );
    frame(&context, &mut app);
    assert!(app.error.is_none(), "{:?}", app.error);
    let before = packed();
    assert!(only_changed(&before, &pixels(&app), &[0, 2, 3]));
    assert_eq!(app.session().unwrap().history.names().count(), 1);
    app.command("undo");
    assert_eq!(pixels(&app), before);
}

#[test]
fn the_canvas_shows_the_targeted_edit_while_it_is_being_painted() {
    let (context, mut app) = packed_app();
    app.command("channel_red");
    app.set_tool(Tool::Brush);
    app.brush.color = [0, 255, 255, 255];
    app.brush.diameter = 6.0;
    frame(&context, &mut app);
    let rect = app.canvas_rect.unwrap();
    let zoom = app.session().unwrap().zoom;
    let at = |x: f32, y: f32| rect.min + Vec2::new(x, y) * zoom;
    pointer_frame(
        &context,
        &mut app,
        at(4.0, 8.0),
        Some(true),
        egui::Modifiers::NONE,
    );
    pointer_frame(
        &context,
        &mut app,
        at(14.0, 8.0),
        None,
        egui::Modifiers::NONE,
    );
    // Mid-stroke, at the next frame's refresh: the open edit already keeps the other channels.
    pointer_frame(
        &context,
        &mut app,
        at(14.0, 8.0),
        None,
        egui::Modifiers::NONE,
    );
    assert!(app.session().unwrap().history.pending_document().is_some());
    assert!(only_changed(&packed(), &pixels(&app), &[1, 2, 3]));
    pointer_frame(
        &context,
        &mut app,
        at(14.0, 8.0),
        Some(false),
        egui::Modifiers::NONE,
    );
    assert!(only_changed(&packed(), &pixels(&app), &[1, 2, 3]));
}

#[test]
fn fill_into_the_alpha_channel_keeps_the_colours() {
    let (_, mut app) = packed_app();
    app.command("channel_alpha");
    app.brush.color = [0, 0, 0, 255];
    app.command("fill_fg");
    assert!(app.error.is_none(), "{:?}", app.error);
    let after = pixels(&app);
    assert!(only_changed(&packed(), &after, &[0, 1, 2]));
    assert!(after.pixels().all(|p| p[3] == 255));
}

#[test]
fn channel_commands_target_and_show_channels_and_shift_extends() {
    let (_, mut app) = packed_app();
    assert_eq!(targets(&app), (Channels::ALL, Channels::COLOR));
    app.command("channel_blue");
    assert_eq!(targets(&app).0, Channels::only(Channel::Blue));
    assert_eq!(app.status, "Editing channels: B");
    app.target_channel(Some(Channel::Red), true);
    let red_blue = Channels::only(Channel::Red).with(Channel::Blue, true);
    assert_eq!(targets(&app), (red_blue, red_blue));
    // Shift-clicking the last targeted channels away returns to the composite.
    app.target_channel(Some(Channel::Red), true);
    app.target_channel(Some(Channel::Blue), true);
    assert_eq!(targets(&app), (Channels::ALL, Channels::COLOR));
    // Shift-click while every channel is targeted targets just that one.
    app.target_channel(Some(Channel::Alpha), true);
    assert_eq!(targets(&app).0, Channels::only(Channel::Alpha));
    app.command("channel_composite");
    assert_eq!(targets(&app), (Channels::ALL, Channels::COLOR));
    assert_eq!(app.status, "Editing every channel");
    // The state belongs to each document.
    app.command("channel_green");
    app.new_document();
    assert_eq!(targets(&app), (Channels::ALL, Channels::COLOR));
}

#[test]
fn eyes_change_the_view_but_never_hide_every_channel() {
    let (_, mut app) = packed_app();
    // Hiding the colours with alpha hidden would leave nothing.
    app.toggle_channel_view(None);
    assert_eq!(targets(&app).1, Channels::COLOR);
    app.toggle_channel_view(Some(Channel::Alpha));
    assert_eq!(targets(&app).1, Channels::ALL);
    app.toggle_channel_view(None);
    assert_eq!(targets(&app).1, Channels::only(Channel::Alpha));
    app.toggle_channel_view(Some(Channel::Alpha));
    assert_eq!(targets(&app).1, Channels::only(Channel::Alpha));
    app.toggle_channel_view(None);
    assert_eq!(targets(&app).1, Channels::ALL);
    app.toggle_channel_view(Some(Channel::Green));
    assert_eq!(targets(&app).1, Channels::ALL.with(Channel::Green, false));
    // The view never changes the targets.
    assert_eq!(targets(&app).0, Channels::ALL);
}

#[test]
fn viewing_alpha_alone_shows_it_as_grey_on_the_canvas() {
    let (context, mut app) = packed_app();
    app.command("channel_alpha");
    frame(&context, &mut app);
    let session = app.session().unwrap();
    let shown = session.composite.as_ref().expect("a CPU preview in tests");
    assert_eq!(shown.dimensions(), (20, 16));
    for (s, v) in packed().pixels().zip(shown.pixels()) {
        assert_eq!(v.0, [s[3], s[3], s[3], 255]);
    }
    assert!(session.channel_view_pixels.is_some());
    // Back to the composite: the layer as it is.
    app.command("channel_composite");
    frame(&context, &mut app);
    let session = app.session().unwrap();
    assert_eq!(
        session.composite.as_deref(),
        Some(&render::render(&session.document))
    );
    assert!(session.channel_view_pixels.is_none());
}

#[test]
fn loading_red_as_a_selection_gives_coverage_equal_to_red() {
    let (_, mut app) = packed_app();
    // Only with one channel targeted.
    assert!(!(commands::find("load_channel_selection").unwrap().enabled)(&app));
    app.command("channel_red");
    assert!((commands::find("load_channel_selection").unwrap().enabled)(
        &app
    ));
    app.command("load_channel_selection");
    let session = app.session().unwrap();
    let selection = session.document.selection.as_ref().unwrap();
    for (x, y, value) in selection.enumerate_pixels() {
        assert_eq!(value[0], packed().get_pixel(x, y)[0]);
    }
    assert_eq!(session.history.undo_name(), Some("Load Channel Selection"));
    // Ctrl-click loads any channel, whatever is targeted.
    app.load_channel_selection(Channel::Alpha);
    let selection = app.session().unwrap().document.selection.clone().unwrap();
    assert_eq!(selection.get_pixel(3, 2)[0], packed().get_pixel(3, 2)[3]);
    app.command("undo");
    app.command("undo");
    assert!(app.session().unwrap().document.selection.is_none());
}

#[test]
fn a_greyscale_image_pastes_into_the_targeted_channel() {
    use super::clipboard::ClipboardContent;

    let (_, mut app) = packed_app();
    app.command("channel_green");
    let grey = RgbaImage::from_fn(4, 2, |x, _| {
        let v = (x * 60) as u8;
        image::Rgba([v, v, v, 255])
    });
    app.paste_content(ClipboardContent::Image(grey));
    assert!(app.error.is_none(), "{:?}", app.error);
    let session = app.session().unwrap();
    assert_eq!(session.document.layers.len(), 1);
    assert_eq!(session.history.undo_name(), Some("Paste into Channel"));
    let after = pixels(&app);
    assert!(only_changed(&packed(), &after, &[0, 2, 3]));
    // Centred: the image's top-left lands on (8, 7).
    assert_eq!(after.get_pixel(8, 7)[1], 0);
    assert_eq!(after.get_pixel(11, 8)[1], 180);
    assert_eq!(after.get_pixel(7, 7)[1], packed().get_pixel(7, 7)[1]);

    // With every channel targeted, a paste is a new layer as before.
    app.command("channel_composite");
    app.paste_content(ClipboardContent::Image(RgbaImage::new(2, 2)));
    assert_eq!(app.session().unwrap().document.layers.len(), 2);
}

#[test]
fn edits_that_are_not_pixel_edits_of_the_layer_are_left_whole() {
    let (_, mut app) = packed_app();
    app.command("channel_green");
    app.command("new_layer");
    let document = &app.session().unwrap().document;
    assert_eq!(document.layers.len(), 2);
    // A new blank layer painted with green targeted keeps its other channels at zero.
    app.brush.color = [200, 100, 50, 255];
    app.command("fill_fg");
    let after = pixels(&app);
    assert!(after.pixels().all(|p| p.0 == [0, 100, 0, 0]));
}

//! Upstream Compositor bugs that looked as if they did not affect Xuan (issue 122), each tried
//! the way its upstream report reproduces it. The Magic Wand outline at 1% zoom and toggling
//! layers of a large PSD are performance checks and are not covered here; the canvas edits'
//! handling of effects is also checked in `layer_effects/tests.rs`.

use super::units::{commit, enter};
use super::*;
use egui::{Key, Modifiers};
use xuan::layer_effects::{LayerEffects, ShadowEffect, StrokeEffect};

/// Ctrl as the native backends report it on Linux and Windows, where it is also the command key
/// that text fields read.
const CTRL: Modifiers = Modifiers::CTRL.plus(Modifiers::COMMAND);
const CTRL_SHIFT: Modifiers = CTRL.plus(Modifiers::SHIFT);

/// An editor with a 200 × 120 document.
fn editor() -> UiTest {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [200, 120];
    ui.app_mut().new_document();
    ui.settle();
    ui
}

/// Opens the Text window on new text and types `content` over its placeholder.
fn add_text(ui: &mut UiTest, content: &str) {
    ui.key(Key::T);
    ui.click("Add text…");
    assert_eq!(ui.app().dialog, Some(Dialog::Text));
    ui.type_keys(content);
    assert_eq!(text(ui).content, content);
}

/// The text the Text window is editing.
fn text(ui: &UiTest) -> &xuan::text::TextStyle {
    &ui.app().text_edit.as_ref().unwrap().style
}

fn active(ui: &UiTest) -> &Layer {
    ui.app().session().unwrap().document.active().unwrap()
}

fn history(ui: &UiTest) -> Vec<String> {
    let history = &ui.app().session().unwrap().history;
    history.names().map(str::to_owned).collect()
}

/// The window manager asks to close the window, as Alt+F4 does.
fn ask_to_close(ui: &mut UiTest) {
    ui.harness
        .input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    ui.harness.step();
}

/// Clicks the button labelled `label` in the window titled `window`, where another window has
/// one too, and runs the frame that handles it.
fn click_in(ui: &mut UiTest, window: &str, label: &str) {
    let area = ui
        .ctx()
        .memory(|memory| memory.area_rect(egui::Id::new(window)))
        .unwrap_or_else(|| panic!("no {window} window"));
    ui.harness
        .query_all_by_role_and_label(Role::Button, label)
        .find(|node| area.contains_rect(node.rect()))
        .unwrap_or_else(|| panic!("no {label} button in {window}"))
        .click();
    ui.harness.step();
}

/// Upstream Compositor#127: quitting while the Text window was open did nothing, or lost the
/// text. Here closing the window asks first (the window manager's close, as below, and the title
/// bar's close button both go through `begin_quit`), as for any unsaved edit, even when nothing
/// else in the document changed. Cancel goes back to the text as typed; Save writes it, still live text,
/// before the window closes. Ctrl+Q and File → Quit are not covered: they are unavailable while
/// the Text window is open (see issue 122).
#[test]
fn quitting_with_the_text_window_open_asks_first_and_saves_live_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Poster.xuan");
    let mut ui = editor();
    // A project that has a file already, so Save needs no file dialog.
    ui.app_mut().session_mut().unwrap().path = Some(path.clone());
    add_text(&mut ui, "Hello");
    assert!(!ui.app().session().unwrap().history.edited());

    ask_to_close(&mut ui);
    assert!(!ui.close_requested());
    ui.settle();
    assert!(ui.app().close_app);
    assert!(ui.has("Save your changes?"));
    click_in(&mut ui, "Save your changes?", "Cancel");
    ui.settle();
    assert!(!ui.app().close_app);
    assert!(!ui.has("Save your changes?"));
    assert_eq!(ui.app().dialog, Some(Dialog::Text));
    assert_eq!(text(&ui).content, "Hello");

    ask_to_close(&mut ui);
    ui.settle();
    click_in(&mut ui, "Save your changes?", "Save");
    assert!(ui.close_requested());
    let saved = io::load(&path).unwrap();
    let layer = saved.layers.iter().find(|l| l.text.is_some()).unwrap();
    assert_eq!(layer.text.as_ref().unwrap().content, "Hello");
}

/// Upstream Compositor#127 also lost or rasterized the text being edited when the document
/// was saved, closed, switched away from, resized or filtered mid-edit. While the Text window
/// is open none of that is offered: the menus are disabled, their shortcuts do nothing and the
/// tabs stay put, until the text is applied.
#[test]
fn the_text_window_holds_off_document_commands_until_it_closes() {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [200, 120];
    for title in ["Alpha", "Beta"] {
        ui.app_mut().new_document();
        ui.app_mut().session_mut().unwrap().title = title.into();
    }
    ui.settle();
    ui.app_mut().command_trace = Some(Vec::new());
    // Without the Text window, the shortcuts below run their commands.
    ui.press(Modifiers::CTRL, Key::S);
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&["save".to_owned()][..])
    );
    ui.app_mut().command_trace = Some(Vec::new());

    add_text(&mut ui, "Hello");
    for menu in ["File", "Image", "Filter"] {
        assert!(
            !ui.enabled(menu),
            "{menu} is enabled behind the Text window"
        );
    }
    // With the text field focused, keys go to the field (Ctrl+W deletes a word there), so try
    // them with the focus elsewhere in the window.
    ui.ctx().memory_mut(|memory| {
        if let Some(focused) = memory.focused() {
            memory.surrender_focus(focused);
        }
    });
    ui.settle();
    for (modifiers, key) in [
        (Modifiers::CTRL, Key::S),
        (Modifiers::CTRL.plus(Modifiers::SHIFT), Key::S),
        (Modifiers::CTRL, Key::W),
        (Modifiers::CTRL, Key::N),
        (Modifiers::CTRL, Key::PageUp),
    ] {
        ui.press(modifiers, key);
    }
    assert_eq!(ui.app().command_trace.as_deref(), Some(&[][..]));
    ui.click("Alpha");
    assert_eq!(ui.app().session().unwrap().title, "Beta");
    assert_eq!(ui.app().sessions.len(), 2);
    assert_eq!(ui.app().dialog, Some(Dialog::Text));
    assert_eq!(text(&ui).content, "Hello");
    assert!(active(&ui).text.is_some());

    // Once the text is applied, the menus and shortcuts are back.
    ui.press(Modifiers::CTRL, Key::Enter);
    assert_eq!(ui.app().dialog, None);
    assert_eq!(active(&ui).text.as_ref().unwrap().content, "Hello");
    assert!(ui.enabled("File"));
    ui.press(Modifiers::CTRL, Key::S);
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&["save".to_owned()][..])
    );
}

/// Upstream Compositor#77: Undo did nothing in a text box. In the Text window's text field,
/// Ctrl+Z and Ctrl+Shift+Z undo and redo the typing and leave the document's history alone.
#[test]
fn undo_in_the_text_field_undoes_typing_not_the_document() {
    let mut ui = editor();
    // An edit that the document's Undo would take back.
    ui.app_mut().command("fill_fg");
    ui.settle();
    let before = history(&ui);
    assert_eq!(before.len(), 1);
    add_text(&mut ui, "Hello");

    ui.press(CTRL, Key::Z);
    assert_eq!(text(&ui).content, "Text");
    assert_eq!(history(&ui), before);
    assert_eq!(ui.app().dialog, Some(Dialog::Text));
    ui.press(CTRL_SHIFT, Key::Z);
    assert_eq!(text(&ui).content, "Hello");
    assert_eq!(history(&ui), before);
    assert_eq!(active(&ui).text.as_ref().unwrap().content, "Hello");
}

/// Upstream Compositor#150 previews a font on the canvas text while the pointer is over it in
/// the font list. Xuan draws each family in its own typeface in the list and changes the text
/// only when a font is picked or reached with the arrow keys: hovering leaves the canvas as it
/// was.
#[test]
fn hovering_fonts_in_the_list_does_not_change_the_canvas() {
    let mut ui = editor();
    add_text(&mut ui, "Hello");
    let before = active(&ui).clone();
    let style = text(&ui).clone();
    ui.harness
        .get_by_role_and_label(Role::ComboBox, &style.family)
        .click();
    ui.settle();
    assert!(egui::Popup::is_any_open(&ui.ctx()));
    let families = ui.app().text_renderer.as_ref().unwrap().families().to_vec();
    let shown: Vec<_> = families
        .iter()
        .filter(|family| ui.harness.query_all_by_label(family).next().is_some())
        .cloned()
        .collect();
    assert!(!shown.is_empty());
    for family in &shown {
        ui.harness
            .query_all_by_label(family)
            .last()
            .unwrap()
            .hover();
        ui.settle();
        let layer = active(&ui);
        assert_eq!(text(&ui), &style, "hovering {family}");
        assert_eq!(layer.text, before.text, "hovering {family}");
        assert_eq!(layer.pixels, before.pixels, "hovering {family}");
        assert!(egui::Popup::is_any_open(&ui.ctx()));
    }
    ui.key(Key::Escape);
    assert!(!egui::Popup::is_any_open(&ui.ctx()));
    assert_eq!(ui.app().dialog, Some(Dialog::Text));
    assert_eq!(text(&ui), &style);
    assert_eq!(active(&ui).pixels, before.pixels);

    // Picking another family does change the text, so the rows hovered above were live options.
    // (Skipped where the bundled font is the only one installed.)
    if let Some(other) = shown.iter().find(|family| **family != style.family) {
        ui.harness
            .get_by_role_and_label(Role::ComboBox, &style.family)
            .click();
        ui.settle();
        ui.harness.get_by_label(other).click();
        ui.settle();
        assert_eq!(&text(&ui).family, other);
        assert_eq!(&active(&ui).text.as_ref().unwrap().family, other);
    }
}

/// Upstream Compositor#43: with Auto Select, clicking a small layer over a full-canvas
/// background, then the background, then the small layer again left the background selected.
/// Xuan takes the topmost layer under the pointer every time, by pixels or by bounds, and a
/// drag that starts on the small layer moves it.
#[test]
fn auto_select_finds_a_layer_over_a_full_canvas_background_again() {
    for ignore_transparent_pixels in [true, false] {
        let mut ui = editor();
        let background = Layer::image(
            "Background",
            RgbaImage::from_pixel(200, 120, image::Rgba([240, 240, 240, 255])),
        );
        let mut badge = Layer::image(
            "Badge",
            RgbaImage::from_pixel(40, 30, image::Rgba([200, 30, 30, 255])),
        );
        badge.transform.x = 100.0;
        badge.transform.y = 60.0;
        let (back, front) = (background.id, badge.id);
        let app = ui.app_mut();
        app.config.snap.enabled = false;
        app.ignore_transparent_pixels = ignore_transparent_pixels;
        let document = &mut app.session_mut().unwrap().document;
        document.layers = vec![background, badge];
        document.select(back, false);
        ui.settle();
        ui.key(Key::V);
        assert!(ui.app().auto_select);
        let zoom = ui.app().session().unwrap().zoom;
        let origin = ui.app().canvas_rect.unwrap().min;
        let at = |x: f32, y: f32| origin + Vec2::new(x, y) * zoom;
        let (on_badge, on_background) = (at(120.0, 75.0), at(30.0, 30.0));
        for (step, (pos, expected)) in [
            (on_badge, front),
            (on_background, back),
            (on_badge, front),
            (on_background, back),
        ]
        .into_iter()
        .enumerate()
        {
            ui.click_at(pos);
            assert_eq!(
                ui.app().session().unwrap().document.active,
                Some(expected),
                "click {step}, ignoring transparent pixels: {ignore_transparent_pixels}"
            );
        }
        // The background is selected; dragging from the badge picks up the badge.
        ui.drag(on_badge, on_badge + Vec2::new(20.0, 0.0));
        let document = &ui.app().session().unwrap().document;
        assert_eq!(document.active, Some(front));
        assert!(document.layers[1].transform.x > 100.0);
        assert_eq!(
            (
                document.layers[0].transform.x,
                document.layers[0].transform.y
            ),
            (0.0, 0.0)
        );
    }
}

/// A stroke and a sharp drop shadow, in layer pixels.
fn styled() -> LayerEffects {
    LayerEffects {
        stroke: Some(StrokeEffect {
            size: 3.0,
            color: [255, 0, 0],
            ..StrokeEffect::default()
        }),
        drop_shadow: Some(ShadowEffect {
            distance: 6.0,
            blur: 0.0,
            opacity: 1.0,
            ..ShadowEffect::DROP
        }),
        ..LayerEffects::default()
    }
}

/// Upstream Compositor#180: Image Size and Canvas Size dropped every layer's effects and turned
/// text into pixels. Through Image → Image Size… and Canvas Size… here, the text layer keeps
/// its effects, its text and its own pixels; Image Size scales it, effects and all.
#[test]
fn image_and_canvas_size_keep_effects_and_live_text() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = editor();
    ui.isolate_config(directory.path());
    add_text(&mut ui, "Hello");
    ui.press(Modifiers::CTRL, Key::Enter);
    assert_eq!(ui.app().dialog, None);
    let id = active(&ui).id;
    ui.app_mut()
        .session_mut()
        .unwrap()
        .document
        .active_mut()
        .unwrap()
        .effects = Some(styled());
    ui.settle();
    let before = active(&ui).clone();

    ui.open_menu("Image");
    ui.click("Image Size…");
    enter(&mut ui, 0, "400");
    enter(&mut ui, 1, "240");
    assert_eq!(ui.app().dimensions, [400, 240]);
    commit(&mut ui, "Resize");
    let document = &ui.app().session().unwrap().document;
    assert_eq!((document.width, document.height), (400, 240));
    let layer = document.layers.iter().find(|l| l.id == id).unwrap();
    assert_eq!(layer.effects, Some(styled()));
    assert_eq!(layer.text, before.text);
    assert!(Arc::ptr_eq(
        layer.pixels.as_ref().unwrap(),
        before.pixels.as_ref().unwrap()
    ));
    assert_eq!(layer.transform.width, before.transform.width * 2.0);

    ui.open_menu("Image");
    ui.click("Canvas Size…");
    enter(&mut ui, 0, "500");
    commit(&mut ui, "Resize");
    let document = &ui.app().session().unwrap().document;
    assert_eq!(document.width, 500);
    let layer = document.layers.iter().find(|l| l.id == id).unwrap();
    assert_eq!(layer.effects, Some(styled()));
    assert_eq!(layer.text, before.text);
    assert!(Arc::ptr_eq(
        layer.pixels.as_ref().unwrap(),
        before.pixels.as_ref().unwrap()
    ));
}

/// Upstream Compositor#180 also lost a layer's effects when its selected pixels were moved
/// (Transform Selection). Xuan has no floating selection to merge back: Ctrl+dragging inside a
/// selection with a selection tool lifts the pixels into a new layer, and the layer they came
/// from keeps its effects.
#[test]
fn moving_selected_pixels_keeps_the_layers_effects() {
    let mut ui = editor();
    let mut art = Layer::image(
        "Art",
        RgbaImage::from_pixel(100, 60, image::Rgba([20, 120, 200, 255])),
    );
    art.transform.x = 20.0;
    art.transform.y = 20.0;
    art.effects = Some(styled());
    let id = art.id;
    let app = ui.app_mut();
    app.config.snap.enabled = false;
    let document = &mut app.session_mut().unwrap().document;
    document.layers = vec![art];
    document.select(id, false);
    document.selection = Some(Arc::new(GrayImage::from_fn(200, 120, |x, y| {
        image::Luma([if (30..60).contains(&x) && (30..50).contains(&y) {
            255
        } else {
            0
        }])
    })));
    ui.settle();
    ui.key(Key::M);
    assert_eq!(ui.app().tool, Tool::Marquee);
    let zoom = ui.app().session().unwrap().zoom;
    let origin = ui.app().canvas_rect.unwrap().min;
    let (from, to) = (
        origin + Vec2::new(40.0, 40.0) * zoom,
        origin + Vec2::new(80.0, 40.0) * zoom,
    );
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: CTRL,
    };
    ui.harness.input_mut().modifiers = CTRL;
    ui.harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(from));
    ui.harness.step();
    ui.harness.input_mut().events.push(button(from, true));
    ui.harness.step();
    for step in 1..=4 {
        let pos = from + (to - from) * (step as f32 / 4.0);
        ui.harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(pos));
        ui.harness.step();
    }
    ui.harness.input_mut().events.push(button(to, false));
    ui.harness.step();
    ui.harness.input_mut().modifiers = Modifiers::NONE;
    ui.settle();

    let document = &ui.app().session().unwrap().document;
    assert_eq!(document.layers.len(), 2, "the pixels were not lifted");
    let source = document.layers.iter().find(|l| l.id == id).unwrap();
    assert_eq!(source.effects, Some(styled()));
    // The lifted pixels left a hole in the layer they came from.
    let pixels = source.pixels.as_ref().unwrap();
    assert_eq!(pixels.get_pixel(25, 15)[3], 0);
    assert_eq!(pixels.get_pixel(5, 5)[3], 255);
    let lifted = document.layers.iter().find(|l| l.id != id).unwrap();
    assert!(lifted.transform.x > 30.0, "{:?}", lifted.transform);
}

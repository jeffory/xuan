//! The Channels pane: opening it from its header, targeting and showing channels with clicks,
//! eyes and Ctrl+2…6, Ctrl-click and Select → Load Channel as Selection.
use super::*;
use xuan::channels::{Channel, Channels};

/// A 20 × 16 document with a packed layer and the Channels pane expanded.
fn channels_ui() -> UiTest {
    let mut ui = UiTest::with_document();
    let pixels = RgbaImage::from_fn(20, 16, |x, y| {
        image::Rgba([(x * 12) as u8, (y * 15) as u8, 90, (255 - x * 4) as u8])
    });
    let session = ui.app_mut().session_mut().unwrap();
    session.document.active_mut().unwrap().pixels = Some(Arc::new(pixels));
    session.invalidate();
    ui.settle();
    assert!(
        ui.app()
            .config
            .panes
            .get(xuan::panes::CHANNELS)
            .unwrap()
            .collapsed
    );
    assert!(!ui.has("Composite"));
    ui.click_role(Role::Button, "Channels");
    assert!(ui.app().pane_open(xuan::panes::CHANNELS));
    ui
}

fn state(ui: &UiTest) -> (Channels, Channels) {
    let session = ui.app().session().unwrap();
    (session.channel_targets, session.channel_view)
}

fn selected(ui: &UiTest, label: &str) -> bool {
    ui.harness.get_by_label(label).accesskit_node().toggled()
        == Some(egui::accesskit::Toggled::True)
}

/// Clicks a row with modifiers held, as Shift- or Ctrl-click.
fn click_with(ui: &mut UiTest, label: &str, modifiers: egui::Modifiers) {
    let pos = ui.harness.get_by_label(label).rect().center();
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    ui.harness.input_mut().modifiers = modifiers;
    ui.harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(pos));
    ui.harness.step();
    ui.harness.input_mut().events.push(button(true));
    ui.harness.step();
    ui.harness.input_mut().events.push(button(false));
    ui.harness.step();
    ui.harness.input_mut().modifiers = egui::Modifiers::NONE;
    ui.settle();
}

#[test]
fn clicking_a_channel_targets_and_shows_it_and_the_composite_restores_both() {
    let mut ui = channels_ui();
    assert!(selected(&ui, "Composite"));
    for label in ["Red", "Green", "Blue", "Alpha"] {
        assert!(ui.has(label), "{label}");
        assert!(!selected(&ui, label));
    }
    ui.click("Green");
    let green = Channels::only(Channel::Green);
    assert_eq!(state(&ui), (green, green));
    assert!(selected(&ui, "Green") && !selected(&ui, "Composite"));
    // The header names the targets; the status bar says what is edited.
    assert_eq!(
        ui.app().pane_detail(xuan::panes::CHANNELS).as_deref(),
        Some("G")
    );
    assert_eq!(ui.app().status, "Editing channels: G");
    // The canvas shows green as grey.
    let shown = ui.app().session().unwrap().channel_view_pixels.clone();
    assert!(shown.is_some_and(|p| p.get_pixel(0, 3).0 == [45, 45, 45, 255]));

    click_with(&mut ui, "Alpha", egui::Modifiers::SHIFT);
    let green_alpha = green.with(Channel::Alpha, true);
    assert_eq!(state(&ui), (green_alpha, green_alpha));
    assert!(selected(&ui, "Alpha") && selected(&ui, "Green"));

    ui.click("Composite");
    assert_eq!(state(&ui), (Channels::ALL, Channels::COLOR));
    assert!(ui.app().session().unwrap().channel_view_pixels.is_none());
}

#[test]
fn eyes_show_and_hide_channels_without_retargeting() {
    let mut ui = channels_ui();
    ui.click_role(Role::CheckBox, "Show Alpha");
    assert_eq!(state(&ui), (Channels::ALL, Channels::ALL));
    ui.click_role(Role::CheckBox, "Show Red");
    assert_eq!(state(&ui).1, Channels::ALL.with(Channel::Red, false));
    assert!(ui.app().session().unwrap().channel_view_pixels.is_some());
    // The composite eye brings the colours back.
    ui.click_role(Role::CheckBox, "Show Composite");
    assert_eq!(state(&ui), (Channels::ALL, Channels::ALL));
}

#[test]
fn shortcuts_and_the_view_menu_target_channels() {
    let mut ui = channels_ui();
    ui.press(egui::Modifiers::CTRL, egui::Key::Num3);
    assert_eq!(state(&ui).0, Channels::only(Channel::Red));
    ui.press(egui::Modifiers::CTRL, egui::Key::Num6);
    assert_eq!(state(&ui).0, Channels::only(Channel::Alpha));
    ui.open_menu("View");
    ui.click("Channels ⏵");
    ui.click("Edit Blue Channel Ctrl+5");
    assert_eq!(state(&ui).0, Channels::only(Channel::Blue));
    ui.press(egui::Modifiers::CTRL, egui::Key::Num2);
    assert_eq!(state(&ui), (Channels::ALL, Channels::COLOR));
}

#[test]
fn ctrl_click_and_the_select_menu_load_a_channel_as_the_selection() {
    let mut ui = channels_ui();
    click_with(&mut ui, "Red", egui::Modifiers::CTRL);
    // Ctrl-click loads without retargeting.
    assert_eq!(state(&ui).0, Channels::ALL);
    let selection = ui.app().session().unwrap().document.selection.clone();
    assert_eq!(selection.unwrap().get_pixel(5, 0)[0], 60);

    ui.open_menu("Select");
    assert!(!ui.enabled("Load Channel as Selection"));
    ui.key(egui::Key::Escape);
    ui.click("Alpha");
    ui.open_menu("Select");
    ui.click("Load Channel as Selection");
    let selection = ui.app().session().unwrap().document.selection.clone();
    assert_eq!(selection.unwrap().get_pixel(5, 0)[0], 235);
}

#[test]
fn layers_without_their_own_pixels_have_no_channels() {
    let mut ui = channels_ui();
    // A text layer's pixels are drawn from its text.
    let session = ui.app_mut().session_mut().unwrap();
    session.document.active_mut().unwrap().text = Some(xuan::text::TextStyle::default());
    ui.settle();
    assert!(ui.has("Select a pixel layer to see its channels"));
    assert!(!ui.has("Composite"));
}

#[test]
fn thumbnails_are_made_again_only_when_the_layer_changes() {
    let mut ui = channels_ui();
    let renders = |ui: &UiTest| ui.app().session().unwrap().channel_thumbnails.renders;
    let first = renders(&ui);
    assert!(first >= 1);
    ui.harness.run_steps(5);
    assert_eq!(renders(&ui), first);
    ui.app_mut().command("fill_fg");
    ui.settle();
    assert_eq!(renders(&ui), first + 1);
    // A layer that has not been painted yet keeps its blank thumbnails too.
    ui.app_mut().command("new_layer");
    ui.settle();
    let blank = renders(&ui);
    ui.harness.run_steps(5);
    assert_eq!(renders(&ui), blank);
}

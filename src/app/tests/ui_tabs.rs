//! The document tab bar, driven like a user: keys, clicks, the context menu and dragging.

use super::*;
use egui::{Key, Modifiers, PointerButton};

const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);

/// Three clean documents, Alpha, Beta and Gamma; Gamma, the last opened, is selected.
fn three_tabs() -> UiTest {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [20, 16];
    for title in ["Alpha", "Beta", "Gamma"] {
        ui.app_mut().new_document();
        let session = ui.app_mut().session_mut().unwrap();
        session.title = title.into();
    }
    ui.settle();
    ui
}

fn titles(ui: &UiTest) -> Vec<String> {
    ui.app().sessions.iter().map(|s| s.title.clone()).collect()
}

fn selected(ui: &UiTest) -> String {
    ui.app().session().unwrap().title.clone()
}

fn tab_rect(ui: &UiTest, title: &str) -> egui::Rect {
    ui.harness.get_by_label(title).rect()
}

#[test]
fn keyboard_commands_switch_tabs() {
    let mut ui = three_tabs();
    assert_eq!(selected(&ui), "Gamma");
    ui.press(Modifiers::CTRL, Key::Tab);
    assert_eq!(selected(&ui), "Alpha", "Ctrl+Tab wraps round");
    ui.press(CTRL_SHIFT, Key::Tab);
    assert_eq!(selected(&ui), "Gamma");
    ui.press(Modifiers::CTRL, Key::PageUp);
    assert_eq!(selected(&ui), "Beta");
    ui.press(Modifiers::CTRL, Key::PageDown);
    assert_eq!(selected(&ui), "Gamma");
    ui.press(Modifiers::ALT, Key::Num1);
    assert_eq!(selected(&ui), "Alpha");
    ui.press(Modifiers::ALT, Key::Num2);
    assert_eq!(selected(&ui), "Beta");
    ui.press(Modifiers::ALT, Key::Num9);
    assert_eq!(selected(&ui), "Gamma", "Alt+9 is the last tab");
    // No fifth tab: nothing changes.
    ui.press(Modifiers::ALT, Key::Num5);
    assert_eq!(selected(&ui), "Gamma");
}

#[test]
fn clicking_selects_and_middle_click_closes() {
    let mut ui = three_tabs();
    ui.click("Alpha");
    assert_eq!(selected(&ui), "Alpha");
    ui.harness
        .get_by_label("Beta")
        .click_button(PointerButton::Middle);
    ui.settle();
    assert_eq!(titles(&ui), ["Alpha", "Gamma"]);
    // The close button closes too.
    ui.click("Close Gamma");
    assert_eq!(titles(&ui), ["Alpha"]);
}

#[test]
fn a_dirty_tab_asks_before_closing() {
    let mut ui = three_tabs();
    ui.app_mut().command("new_layer");
    ui.settle();
    ui.harness
        .get_by_label("Gamma")
        .click_button(PointerButton::Middle);
    ui.settle();
    assert!(ui.has("Discard changes"));
    ui.click("Discard changes");
    assert_eq!(titles(&ui), ["Alpha", "Beta"]);
}

#[test]
fn the_context_menu_closes_others_and_to_the_right_and_copies_the_path() {
    let mut ui = three_tabs();
    ui.app_mut().sessions[1].path = Some(PathBuf::from("/photos/beta.xuan"));
    ui.settle();
    ui.harness.get_by_label("Beta").click_secondary();
    ui.settle();
    ui.click_and_stop("Copy Path");
    assert_eq!(ui.copied_text().as_deref(), Some("/photos/beta.xuan"));

    ui.harness.get_by_label("Alpha").click_secondary();
    ui.settle();
    ui.click("Close Tabs to the Right");
    assert_eq!(titles(&ui), ["Alpha"]);

    let mut ui = three_tabs();
    ui.harness.get_by_label("Beta").click_secondary();
    ui.settle();
    assert!(!ui.enabled("Copy Path"), "an unsaved document has no path");
    ui.click("Close Other Tabs");
    assert_eq!(titles(&ui), ["Beta"]);
}

#[test]
fn closing_others_stops_when_an_unsaved_prompt_is_cancelled() {
    let mut ui = three_tabs();
    ui.app_mut().current = 0;
    ui.app_mut().command("new_layer");
    ui.app_mut().current = 1;
    ui.settle();
    ui.app_mut().command("close_other_tabs");
    ui.settle();
    // Alpha has changes and asks; Cancel keeps it and Gamma.
    assert!(ui.has("Discard changes"));
    ui.click("Cancel");
    assert_eq!(titles(&ui), ["Alpha", "Beta", "Gamma"]);
}

#[test]
fn reopen_closed_tab_brings_back_the_last_closed_file() {
    let directory = tempfile::tempdir().unwrap();
    let image = directory.path().join("reopen.png");
    RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
        .save(&image)
        .unwrap();
    let mut ui = three_tabs();
    ui.app_mut().open_path(&image, false);
    ui.settle();
    let title = selected(&ui);
    assert_eq!(ui.app().sessions.len(), 4);
    ui.press(Modifiers::CTRL, Key::W);
    assert_eq!(ui.app().sessions.len(), 3);
    ui.press(CTRL_SHIFT, Key::T);
    assert_eq!(ui.app().sessions.len(), 4);
    assert_eq!(selected(&ui), title);
    assert_eq!(
        ui.app().session().unwrap().source.as_deref(),
        Some(image.as_path())
    );
}

#[test]
fn dragging_a_tab_reorders_the_documents() {
    let mut ui = three_tabs();
    let alpha = tab_rect(&ui, "Alpha");
    let gamma = tab_rect(&ui, "Gamma");
    // Drop Alpha just past Gamma's middle: it becomes the last tab and stays selected.
    ui.click("Alpha");
    ui.drag(
        alpha.center(),
        gamma.center() + egui::vec2(gamma.width() * 0.4, 0.0),
    );
    assert_eq!(titles(&ui), ["Beta", "Gamma", "Alpha"]);
    assert_eq!(selected(&ui), "Alpha");
    // And back to the front.
    let beta = tab_rect(&ui, "Beta");
    let alpha = tab_rect(&ui, "Alpha");
    ui.drag(alpha.center(), beta.left_center() + egui::vec2(4.0, 0.0));
    assert_eq!(titles(&ui), ["Alpha", "Beta", "Gamma"]);
}

#[test]
fn double_clicking_the_empty_bar_starts_a_new_document() {
    let mut ui = three_tabs();
    ui.app_mut().command_trace = Some(Vec::new());
    let gamma = tab_rect(&ui, "Gamma");
    let pos = gamma.right_center() + egui::vec2(60.0, 0.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    ui.harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(pos));
    ui.harness.step();
    // Both clicks in one frame: the harness advances a quarter second per frame, more than
    // egui's double-click delay.
    for pressed in [true, false, true, false] {
        ui.harness.input_mut().events.push(button(pressed));
    }
    ui.settle();
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&["new".to_owned()][..])
    );
}

#[test]
fn many_tabs_overflow_into_a_list() {
    let mut ui = UiTest::new();
    ui.app_mut().dimensions = [8, 8];
    for index in 0..30 {
        ui.app_mut().new_document();
        ui.app_mut().session_mut().unwrap().title = format!("Doc{index:02}");
    }
    ui.settle();
    assert!(ui.has("List all tabs"));
    // The selected (last) tab was scrolled into view; the first is out of sight.
    assert!(ui.has("Doc29"));
    assert!(!ui.has("Doc00"));
    ui.click("List all tabs");
    ui.click("Doc00");
    assert_eq!(selected(&ui), "Doc00");
    assert!(ui.has_role(egui::accesskit::Role::Button, "Close Doc00"));

    // With a few tabs there is nothing to list.
    let ui = three_tabs();
    assert!(!ui.has("List all tabs"));
}

#[test]
fn the_tab_bar_sits_between_the_tool_options_and_the_canvas() {
    let mut ui = three_tabs();
    ui.app_mut().tool = Tool::Move;
    ui.settle();
    let options = ui.harness.get_by_label("Auto Select").rect();
    let tab = tab_rect(&ui, "Gamma");
    let canvas = ui.app().canvas_rect.expect("the canvas was drawn");
    assert!(
        options.bottom() <= tab.top(),
        "tool options {options:?} above the tabs {tab:?}"
    );
    assert!(
        tab.bottom() <= canvas.top(),
        "tabs {tab:?} above the document {canvas:?}"
    );
    assert!((34.0..=36.5).contains(&tab.height()), "about 36 px tall");
}

#[test]
fn the_new_canvas_button_is_square_and_still_works() {
    let mut ui = three_tabs();
    let gamma = tab_rect(&ui, "Gamma");
    // The zoom-in button has the same name, so pick the one right after the last tab.
    let plus = ui
        .harness
        .get_all_by_label("+")
        .map(|node| node.rect())
        .find(|r| (r.center().y - gamma.center().y).abs() < 1.0 && r.left() >= gamma.right())
        .expect("a + button in the tab bar");
    assert!((plus.width() - plus.height()).abs() < 1.0, "square");
    ui.app_mut().command_trace = Some(Vec::new());
    ui.click_at(plus.center());
    assert_eq!(
        ui.app().command_trace.as_deref(),
        Some(&["new".to_owned()][..])
    );
}

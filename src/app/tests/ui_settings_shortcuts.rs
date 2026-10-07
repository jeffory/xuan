//! Settings keeps one size and aligned controls (issue 85); Help → Keyboard Shortcuts is the
//! Settings list, read-only, with the same search (issue 86).

use super::*;
use crate::app::settings::{SettingsPage, show_settings_page};
use egui::accesskit::Role;

const MINIMUM: Vec2 = Vec2::new(850.0, 560.0);

fn window_rect(ui: &UiTest, id: &str) -> egui::Rect {
    ui.ctx()
        .memory(|memory| memory.area_rect(egui::Id::new(id)))
        .unwrap_or_else(|| panic!("no {id} window"))
}

fn rect(ui: &UiTest, label: &str) -> egui::Rect {
    ui.harness.get_by_label(label).rect()
}

fn open_settings(size: Vec2, page: SettingsPage) -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::sized(size);
    ui.isolate_config(directory.path());
    show_settings_page(&ui.ctx(), page);
    ui.app_mut().command("settings");
    ui.settle();
    (directory, ui)
}

#[test]
fn settings_window_keeps_its_height_on_every_page() {
    for size in [MINIMUM, Vec2::new(1280.0, 860.0)] {
        let (_directory, mut ui) = open_settings(size, SettingsPage::General);
        let general = window_rect(&ui, "app_settings");
        for page in [
            SettingsPage::Appearance,
            SettingsPage::Selection,
            SettingsPage::Keyboard,
            SettingsPage::General,
        ] {
            show_settings_page(&ui.ctx(), page);
            ui.settle();
            let rect = window_rect(&ui, "app_settings");
            assert!(
                (rect.height() - general.height()).abs() < 0.5,
                "{page:?} at {size:?}: {rect:?} is not as tall as {general:?}"
            );
            assert!(
                (rect.top() - general.top()).abs() < 0.5,
                "{page:?} at {size:?} moved the window"
            );
        }
    }
}

/// The left edge of every drop-down and checkbox inside the Settings window.
fn control_lefts(ui: &UiTest) -> Vec<f32> {
    use egui_kittest::kittest::Queryable as _;
    let window = window_rect(ui, "app_settings");
    [Role::ComboBox, Role::CheckBox]
        .into_iter()
        .flat_map(|role| ui.harness.query_all_by_role(role))
        .map(|node| node.rect())
        .filter(|rect| window.contains_rect(*rect))
        .map(|rect| rect.left())
        .collect()
}

#[test]
fn general_and_appearance_controls_start_at_the_same_x() {
    let (_directory, mut ui) = open_settings(Vec2::new(1280.0, 860.0), SettingsPage::General);
    let mut all = control_lefts(&ui);
    assert!(all.len() >= 3, "General has few controls: {all:?}");
    show_settings_page(&ui.ctx(), SettingsPage::Appearance);
    ui.settle();
    let appearance = control_lefts(&ui);
    assert!(
        appearance.len() >= 4,
        "Appearance has few controls: {appearance:?}"
    );
    all.extend(appearance);
    for x in &all {
        assert!(
            (x - all[0]).abs() <= 1.0,
            "controls do not share a left edge: {all:?}"
        );
    }
}

#[test]
fn keyboard_shortcuts_stays_on_one_line_in_the_nav() {
    let (_directory, ui) = open_settings(Vec2::new(1280.0, 860.0), SettingsPage::General);
    let long = rect(&ui, "Keyboard Shortcuts");
    let short = rect(&ui, "Selection");
    assert!(
        (long.height() - short.height()).abs() < 1.0,
        "Keyboard Shortcuts wraps: {long:?} against {short:?}"
    );
}

fn open_shortcuts(size: Vec2) -> UiTest {
    let mut ui = UiTest::sized(size);
    ui.key(egui::Key::F1);
    assert!(ui.app().dialog == Some(Dialog::Shortcuts));
    ui
}

#[test]
fn f1_has_a_search_box_that_filters_the_list() {
    let mut ui = open_shortcuts(Vec2::new(1280.0, 860.0));
    assert!(ui.has("Fill Foreground"));
    assert!(ui.has("Undo"));
    ui.type_in_text_field("Fill");
    assert!(ui.has("Fill Foreground"));
    assert!(!ui.has("Undo"));
    ui.type_in_text_field("zzzzzz");
    assert!(ui.has("No shortcuts match."));
    assert!(!ui.has("Fill Foreground"));
}

#[test]
fn f1_rows_show_the_action_then_its_keys() {
    let ui = open_shortcuts(Vec2::new(1280.0, 860.0));
    let action = rect(&ui, "Fill Foreground");
    let key = rect(&ui, "Alt+Backspace");
    assert!(
        action.right() <= key.left(),
        "the action {action:?} is not before its key {key:?}"
    );
    assert!((action.center().y - key.center().y).abs() < 4.0);
    // Several keys are separate chips.
    assert!(ui.has("Ctrl+Shift+Z"));
    assert!(ui.has("Ctrl+Y"));
}

#[test]
fn f1_list_scrolls_in_a_small_window_and_the_last_rows_are_reachable() {
    let mut ui = open_shortcuts(MINIMUM);
    let dialog = window_rect(&ui, "shortcuts");
    let last = "Apply crop / Cancel gesture";
    let visible = |ui: &UiTest| {
        let rect = rect(ui, last);
        dialog.contains_rect(rect) && ui.ctx().content_rect().contains_rect(rect)
    };
    assert!(
        !visible(&ui),
        "the whole list fits, so there is nothing to scroll"
    );
    // The Done button stays in the dialog.
    assert!(dialog.contains_rect(rect(&ui, "Done")));
    let over = dialog.center();
    ui.harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(over));
    ui.settle();
    for _ in 0..10 {
        ui.harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            modifiers: egui::Modifiers::NONE,
        });
        ui.settle();
    }
    assert!(visible(&ui), "the last row cannot be scrolled into view");
}

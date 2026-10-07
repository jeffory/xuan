//! Dialog layout: buttons that stay in sight in a small window (issue 74), a title bar as wide
//! as the window (issue 76), and one footer layout (issue 82).

use super::*;
use crate::app::chrome::{ButtonLayout, WindowButton};

/// The app's minimum window size.
const MINIMUM: Vec2 = Vec2::new(850.0, 560.0);
/// The title and menu bar along the top of the window.
const MENU_BAR_BOTTOM: f32 = 32.0;

fn small_with_document() -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::sized(MINIMUM);
    ui.isolate_config(directory.path());
    ui.app_mut().dimensions = [32, 24];
    ui.app_mut().new_document();
    ui.app_mut().command("fill_fg");
    ui.settle();
    (directory, ui)
}

fn window_rect(ui: &UiTest, id: &str) -> egui::Rect {
    ui.ctx()
        .memory(|memory| memory.area_rect(egui::Id::new(id)))
        .unwrap_or_else(|| panic!("no {id} window"))
}

fn button_rect(ui: &UiTest, label: &str) -> egui::Rect {
    ui.harness
        .get_by_role_and_label(Role::Button, label)
        .rect()
}

/// The dialog sits below the menu bar, inside the window, and its commit button is on it.
#[track_caller]
fn assert_commit_visible(ui: &UiTest, window: &str, commit: &str) {
    let dialog = window_rect(ui, window);
    let button = button_rect(ui, commit);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, MINIMUM);
    assert!(
        dialog.top() >= MENU_BAR_BOTTOM,
        "{window} covers the menu bar: {dialog:?}"
    );
    assert!(
        screen.contains_rect(dialog),
        "{window} is off screen: {dialog:?}"
    );
    assert!(
        dialog.contains_rect(button),
        "{window}'s {commit} {button:?} is outside the dialog {dialog:?}"
    );
}

#[test]
fn small_window_keeps_dialog_buttons_in_sight() {
    let (_directory, mut ui) = small_with_document();
    ui.app_mut().command("levels");
    ui.settle();
    assert_commit_visible(&ui, "Levels", "Apply");
    ui.click("Cancel");

    ui.app_mut().export_format = "jpg".into();
    ui.app_mut().command("export");
    ui.settle();
    assert_commit_visible(&ui, "Export image", "Export…");
    ui.click("Cancel");

    ui.app_mut().command("settings");
    ui.settle();
    assert_commit_visible(&ui, "app_settings", "Done");
    ui.click("Done");

    ui.app_mut().command("plugins");
    ui.settle();
    assert_commit_visible(&ui, "plugin_manager", "Done");
}

#[test]
fn settings_keyboard_page_scrolls_above_its_footer() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::sized(MINIMUM);
    ui.isolate_config(directory.path());
    ui.app_mut().command("settings");
    ui.settle();
    ui.click("Keyboard Shortcuts");
    assert_commit_visible(&ui, "app_settings", "Done");
}

/// A Plugins folder path long enough to widen the window if it weren't elided.
fn plugins_with_long_folder(layout: ButtonLayout) -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let deep = directory.path().join("a-very-long-folder-name-".repeat(12));
    let mut ui = UiTest::new();
    ui.isolate_config(&deep);
    ui.app_mut().config.title_bar = xuan::config::TitleBar::Compact;
    ui.app_mut().button_layout = layout;
    ui.app_mut().command("plugins");
    ui.settle();
    (directory, ui)
}

#[test]
fn plugins_close_button_sits_at_the_window_edge() {
    use WindowButton::*;
    let (_directory, ui) = plugins_with_long_folder(ButtonLayout::default());
    let dialog = window_rect(&ui, "plugin_manager");
    let close = button_rect(&ui, "Close panel");
    assert!(
        dialog.width() < 700.0,
        "the folder path widened the window: {dialog:?}"
    );
    assert!(
        (dialog.right() - close.right()).abs() < 10.0,
        "close {close:?} is not at the right edge of {dialog:?}"
    );

    let left = ButtonLayout {
        left: vec![Close],
        right: vec![Minimize, Maximize],
    };
    let (_directory, ui) = plugins_with_long_folder(left);
    let dialog = window_rect(&ui, "plugin_manager");
    let close = button_rect(&ui, "Close panel");
    assert!(
        (close.left() - dialog.left()).abs() < 10.0,
        "close {close:?} is not at the left edge of {dialog:?}"
    );
}

/// Content wider than the window's default width widens the window, and the title bar and its
/// close button follow it.
#[test]
fn title_bar_spans_content_wider_than_the_default_width() {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1280.0, 860.0))
        .build_state(
            |ctx, app: &mut Option<EditorApp>| {
                let Some(app) = app else { return };
                app.config.title_bar = xuan::config::TitleBar::Compact;
                app.publish_dialog_chrome(ctx);
                let mut open = true;
                crate::app::widgets::Window::new("Wide")
                    .default_width(300.0)
                    .open(&mut open)
                    .show_with_footer(
                        ctx,
                        |ui| {
                            ui.add(egui::Label::new("wide ".repeat(40)).extend());
                        },
                        |ui, ()| {
                            crate::app::widgets::button(ui, "Footer");
                        },
                    );
            },
            None,
        );
    *harness.state_mut() = Some(EditorApp::with_context(
        &harness.ctx,
        Vec::new(),
        false,
        None,
    ));
    harness.run_steps(3);
    let dialog = harness
        .ctx
        .memory(|memory| memory.area_rect(egui::Id::new("Wide")))
        .unwrap();
    let close = harness
        .get_by_role_and_label(Role::Button, "Close panel")
        .rect();
    assert!(dialog.width() > 400.0, "{dialog:?}");
    assert!(
        (dialog.right() - close.right()).abs() < 10.0,
        "close {close:?} is not at the right edge of {dialog:?}"
    );
}

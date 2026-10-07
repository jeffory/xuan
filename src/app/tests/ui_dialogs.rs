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
    ui.harness.get_by_role_and_label(Role::Button, label).rect()
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

/// The buttons a dialog footer should have, by accessible name.
struct Footer<'a> {
    commit: Option<&'a str>,
    cancel: Option<&'a str>,
    destructive: Option<&'a str>,
}

/// [Cancel][`commit`].
const fn footer(commit: &'static str) -> Footer<'static> {
    Footer {
        commit: Some(commit),
        cancel: Some("Cancel"),
        destructive: None,
    }
}

/// Only [`commit`], as on alerts and settings windows.
const fn single(commit: &'static str) -> Footer<'static> {
    Footer {
        commit: Some(commit),
        cancel: None,
        destructive: None,
    }
}

/// Checks the standard footer: the commit button at the dialog's bottom right, Cancel just left
/// of it on the same row, and a destructive button at the far left.
#[track_caller]
fn assert_footer_order(ui: &UiTest, window: &str, footer: Footer) {
    let dialog = window_rect(ui, window);
    let inset = 24.0;
    let near = |a: f32, b: f32| (a - b).abs() < 3.0;
    let commit = footer.commit.map(|label| button_rect(ui, label));
    let cancel = footer.cancel.map(|label| button_rect(ui, label));
    let rightmost = commit.or(cancel).expect("a footer button");
    assert!(
        near(rightmost.right(), dialog.right() - inset),
        "{window}: the rightmost button {rightmost:?} is not at the right of {dialog:?}"
    );
    assert!(
        near(rightmost.bottom(), dialog.bottom() - inset),
        "{window}: {rightmost:?} is not at the bottom of {dialog:?}"
    );
    if let (Some(commit), Some(cancel)) = (commit, cancel) {
        assert!(
            cancel.right() < commit.left() && commit.left() - cancel.right() < 16.0,
            "{window}: Cancel {cancel:?} is not just left of the commit button {commit:?}"
        );
        assert!(near(cancel.center().y, commit.center().y), "{window}");
    }
    if let Some(label) = footer.destructive {
        let destructive = button_rect(ui, label);
        assert!(
            near(destructive.left(), dialog.left() + inset),
            "{window}: {label} {destructive:?} is not at the left of {dialog:?}"
        );
        assert!(
            near(destructive.center().y, rightmost.center().y),
            "{window}"
        );
        let leftmost = cancel.unwrap_or(rightmost);
        assert!(
            leftmost.left() - destructive.right() > 40.0,
            "{window}: {label} sits next to the other buttons"
        );
    }
}

#[test]
fn dialogs_use_the_standard_footer_order() {
    use xuan::effects::Filter;
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    ui.app_mut().command("fill_fg");
    ui.settle();
    let cases = [
        ("levels", "Levels", footer("Apply")),
        ("hue", "Hue/Saturation", footer("Apply")),
        ("curves", "Curves", footer("Apply")),
        ("blur", "Gaussian Blur", footer("Apply")),
        ("layer_effects", "layer_effects", footer("Apply")),
        ("text", "Text", footer("Apply")),
        ("new", "New canvas", footer("Create canvas")),
        ("canvas_size", "Canvas size", footer("Resize")),
        ("image_size", "Image size", footer("Resize")),
        ("export", "Export image", footer("Export…")),
        ("grid_settings", "grid_settings", footer("Apply")),
        ("color_range", "color_range", footer("Apply")),
        ("settings", "app_settings", single("Done")),
        ("plugins", "plugin_manager", single("Done")),
    ];
    for (command, window, buttons) in cases {
        let close = if buttons.cancel.is_some() {
            "Cancel"
        } else {
            "Done"
        };
        match command {
            "blur" => ui
                .app_mut()
                .start_filter(Filter::GaussianBlur { radius: 4.0 }),
            "text" => ui.app_mut().start_text(None, Point::default()),
            _ => ui.app_mut().command(command),
        }
        ui.settle();
        // A filter's preview runs on a worker; its "Updating preview…" note
        // changes the footer's height, so wait for it before measuring or
        // clicking, or a loaded test machine clicks where Cancel just was.
        for _ in 0..500 {
            if !ui
                .app()
                .effect
                .as_ref()
                .is_some_and(|edit| edit.filter_preview.busy())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            ui.settle();
        }
        ui.settle();
        assert_footer_order(&ui, window, buttons);
        ui.click(close);
        assert!(
            ui.app().dialog.is_none() && ui.app().color_range.is_none(),
            "{window} stayed open"
        );
    }

    ui.app_mut().command("plugins");
    ui.settle();
    ui.click("Install…");
    assert_footer_order(
        &ui,
        "plugin_install",
        Footer {
            commit: None,
            cancel: Some("Cancel"),
            destructive: None,
        },
    );
    ui.click("Cancel");
    ui.app_mut().dialog = None;
    ui.settle();

    ui.app_mut().error = Some("Something went wrong.".into());
    ui.settle();
    assert_footer_order(&ui, "Couldn't complete the operation", single("OK"));
    ui.click("OK");

    ui.app_mut().command("fill_fg");
    ui.app_mut().close_tab = Some(0);
    ui.settle();
    assert_footer_order(
        &ui,
        "Save your changes?",
        Footer {
            destructive: Some("Discard changes"),
            ..footer("Save")
        },
    );
    ui.click("Cancel");
}

#[test]
fn keyboard_reassign_prompt_puts_cancel_before_reassign() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    ui.press(egui::Modifiers::CTRL, egui::Key::Comma);
    ui.click("Keyboard Shortcuts");
    ui.type_in_text_field("merge");
    ui.click("Merge Down / Selected: Ctrl+E");
    ui.press(egui::Modifiers::CTRL, egui::Key::S);
    let reassign = button_rect(&ui, "Reassign");
    let cancel = button_rect(&ui, "Cancel");
    assert!(cancel.right() < reassign.left(), "{cancel:?} {reassign:?}");
    assert!((cancel.center().y - reassign.center().y).abs() < 1.0);
}

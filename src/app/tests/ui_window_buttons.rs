//! Compact window buttons drawn from fixture theme assets, in the order the desktop asks for.

use super::*;
use crate::app::{
    chrome::{ButtonLayout, WindowButton},
    window_theme::{Env, WindowTheme, tests::Fixture},
};

fn compact(layout: ButtonLayout) -> (tempfile::TempDir, Fixture, UiTest) {
    let (fixture, env) = Fixture::new(true);
    for name in ["close", "minimize", "maximize"] {
        fixture.png(
            &format!("home/.config/gtk-3.0/assets/titlebutton-{name}.png"),
            [200, 200, 200, 255],
            20,
        );
    }
    fixture.write(
        "home/.config/gtk-3.0/window_decorations.css",
        ".titlebar button.titlebutton.close { background-image: url(\"assets/titlebutton-close.png\") }\n\
         .titlebar button.titlebutton.minimize { background-image: url(\"assets/titlebutton-minimize.png\") }\n\
         .titlebar button.titlebutton.maximize { background-image: url(\"assets/titlebutton-maximize.png\") }\n",
    );
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    ui.app_mut().config.title_bar = xuan::config::TitleBar::Compact;
    ui.app_mut().button_layout = layout;
    ui.app_mut().window_theme = shared(WindowTheme::new(env));
    ui.settle();
    (directory, fixture, ui)
}

fn shared(theme: WindowTheme) -> crate::app::chrome::SharedWindowTheme {
    std::sync::Arc::new(std::sync::Mutex::new(Some(theme)))
}

fn textures(ui: &UiTest) -> usize {
    let guard = ui.app().window_theme.lock().unwrap();
    guard.as_ref().unwrap().loaded_textures()
}

fn x_of(ui: &UiTest, label: &str) -> f32 {
    ui.harness.get_by_label(label).rect().center().x
}

#[test]
fn compact_buttons_follow_the_configured_order_and_use_the_theme_images() {
    use WindowButton::*;
    let layout = ButtonLayout {
        left: vec![Close],
        right: vec![Maximize, Minimize],
    };
    let (_directory, _fixture, ui) = compact(layout);
    let (close, maximize, minimize) = (
        x_of(&ui, "Close window"),
        x_of(&ui, "Maximize window"),
        x_of(&ui, "Minimize window"),
    );
    assert!(close < 200.0, "close sits on the left: {close}");
    assert!(
        maximize > 1000.0 && minimize > maximize,
        "{maximize} {minimize}"
    );
    // The three buttons come from the theme's PNGs; nothing fell back to the glyphs.
    assert_eq!(textures(&ui), 3);
}

#[test]
fn builtin_setting_and_missing_assets_draw_the_glyphs() {
    let (_directory, _fixture, mut ui) = compact(ButtonLayout::default());
    assert_eq!(textures(&ui), 3);
    // "Built-in": no theme texture is needed.
    let (_directory, _fixture, mut builtin) = compact(ButtonLayout::default());
    builtin.app_mut().config.window_buttons = xuan::config::WindowButtons::BuiltIn;
    builtin.app_mut().window_theme = shared(WindowTheme::new(Env::default()));
    builtin.settle();
    assert_eq!(textures(&builtin), 0);
    assert!(builtin.has("Close window"));
    // Match desktop theme with nothing installed: the glyphs, and still clickable.
    ui.app_mut().window_theme = shared(WindowTheme::new(Env::default()));
    ui.settle();
    assert_eq!(textures(&ui), 0);
    assert!(ui.has("Minimize window") && ui.has("Maximize window") && ui.has("Close window"));
}

/// The x of a dialog's close button relative to the centre of the centred dialog.
fn dialog_close_side(ui: &UiTest) -> f32 {
    let centre = ui.ctx().content_rect().center().x;
    let x = x_of(ui, "Close panel");
    assert!(
        (x - centre).abs() < 340.0,
        "the button is on the dialog: {x} vs {centre}"
    );
    x - centre
}

fn dialog(style: xuan::config::TitleBar, layout: ButtonLayout) -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    ui.app_mut().config.title_bar = style;
    ui.app_mut().button_layout = layout;
    ui.press(egui::Modifiers::CTRL, egui::Key::Comma);
    (directory, ui)
}

#[test]
fn dialog_close_button_follows_the_button_layout_side() {
    use WindowButton::*;
    use xuan::config::TitleBar;
    let right = ButtonLayout::default();
    let left = ButtonLayout {
        left: vec![Close, Minimize],
        right: vec![Maximize],
    };
    for style in [TitleBar::Compact, TitleBar::System] {
        let (_directory, ui) = dialog(style, right.clone());
        assert!(dialog_close_side(&ui) > 0.0, "{style:?} right layout");
        let (_directory, mut ui) = dialog(style, left.clone());
        assert!(dialog_close_side(&ui) < 0.0, "{style:?} left layout");
        // The close button works and keeps its accessible name.
        assert!(ui.has_role(egui::accesskit::Role::Button, "Close panel"));
        ui.click("Close panel");
        assert!(!ui.has("Close panel"));
    }
}

#[test]
fn macos_dialog_close_dot_stays_on_the_left_whatever_the_layout() {
    let (_directory, mut ui) = dialog(xuan::config::TitleBar::MacOs, ButtonLayout::default());
    assert!(dialog_close_side(&ui) < 0.0);
    assert!(ui.has_role(egui::accesskit::Role::Button, "Close panel"));
    ui.click("Close panel");
    assert!(!ui.has("Close panel"));
}

#[test]
fn changing_the_title_bar_style_updates_an_open_dialog() {
    use xuan::config::TitleBar;
    let (_directory, mut ui) = dialog(TitleBar::MacOs, ButtonLayout::default());
    assert!(dialog_close_side(&ui) < 0.0);
    ui.app_mut().config.title_bar = TitleBar::Compact;
    ui.settle();
    assert!(dialog_close_side(&ui) > 0.0);
}

//! Settings → Appearance → Theme, driven like a user.

use super::*;
use crate::app::theme::Palette;
use egui::{Key, Modifiers};

#[test]
fn switching_the_theme_in_settings_recolours_the_interface_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    // Tests never ask the desktop, so System means dark.
    assert_eq!(ui.ctx().palette(), Palette::DARK);
    ui.press(Modifiers::CTRL, Key::Comma);
    ui.click("Appearance");
    ui.click("System");
    ui.click("Light mode");
    assert_eq!(ui.app().config.theme, xuan::config::Theme::Light);
    assert_eq!(ui.ctx().palette(), Palette::LIGHT);
    assert!(!ui.ctx().style().visuals.dark_mode);
    assert_eq!(ui.ctx().style().visuals.panel_fill, Palette::LIGHT.panel);

    ui.click("Light mode");
    ui.click("Dark mode");
    assert_eq!(ui.ctx().palette(), Palette::DARK);
    assert!(ui.ctx().style().visuals.dark_mode);

    ui.click("Done");
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    assert_eq!(saved.theme, xuan::config::Theme::Dark);
}

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

#[test]
fn the_system_accent_can_be_turned_off_in_settings() {
    use crate::app::system_theme::SystemTheme;
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    let green = egui::Color32::from_rgb(0x3a, 0x94, 0x4a);
    let ctx = ui.ctx();
    ui.app_mut().watch_system_theme(
        Box::new(move || SystemTheme {
            dark: Some(true),
            accent: Some(green),
        }),
        Some(ctx),
    );
    ui.settle();
    assert_eq!(ui.ctx().palette(), Palette::DARK.with_accent(green));

    ui.press(Modifiers::CTRL, Key::Comma);
    ui.click("Appearance");
    ui.click("Use system accent colour");
    assert!(!ui.app().config.system_accent);
    assert_eq!(ui.ctx().palette(), Palette::DARK);
    ui.click("Done");
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    assert!(!saved.system_accent);
}

/// Runs frames until `done` holds, as the app would while the desktop changes.
fn wait_for(ui: &mut UiTest, mut done: impl FnMut(&UiTest) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done(ui) {
        assert!(
            std::time::Instant::now() < deadline,
            "the palette never changed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        ui.settle();
    }
}

#[test]
fn the_palette_follows_the_desktop_while_xuan_runs() {
    use crate::app::system_theme::SystemTheme;
    use std::sync::{Arc, Mutex};

    let desktop = Arc::new(Mutex::new(SystemTheme {
        dark: Some(false),
        accent: None,
    }));
    let mut ui = UiTest::with_document();
    let source = Arc::clone(&desktop);
    let ctx = ui.ctx();
    ui.app_mut()
        .watch_system_theme(Box::new(move || *source.lock().unwrap()), Some(ctx));
    // The first answer is waited for, so the next frame is already light.
    ui.settle();
    assert_eq!(ui.ctx().palette(), Palette::LIGHT);

    // The desktop switches to dark with an orange accent.
    let orange = egui::Color32::from_rgb(233, 84, 32);
    *desktop.lock().unwrap() = SystemTheme {
        dark: Some(true),
        accent: Some(orange),
    };
    wait_for(&mut ui, |ui| ui.ctx().palette().dark);
    assert_eq!(ui.ctx().palette(), Palette::DARK.with_accent(orange));

    // A fixed theme ignores the desktop's light/dark preference.
    ui.app_mut().config.theme = xuan::config::Theme::Light;
    ui.settle();
    assert_eq!(ui.ctx().palette(), Palette::LIGHT.with_accent(orange));

    // The desktop stops answering: Xuan's own colours return, dark for System.
    ui.app_mut().config.theme = xuan::config::Theme::System;
    *desktop.lock().unwrap() = SystemTheme::default();
    wait_for(&mut ui, |ui| ui.ctx().palette() == Palette::DARK);
}

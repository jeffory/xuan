//! Settings → General → Language offers every built-in locale by its own name, and a locale
//! dropped in beside them can be picked, with English for the strings it lacks (issue 120).

use super::*;
use crate::app::settings::{SettingsPage, show_settings_page};
use egui::{Key, Modifiers};
use xuan::config::Language;

/// Whether any widget has this label; the settings sidebar repeats the page's heading.
fn shows(ui: &UiTest, label: &str) -> bool {
    ui.harness.query_all_by_label(label).next().is_some()
}

#[test]
fn the_language_picker_offers_every_locale_and_applies_the_choice_at_once() {
    let directory = tempfile::tempdir().unwrap();
    xuan::i18n::add_locale("en-XA", include_str!("../../../testdata/locales/en-XA.tsv"));
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    show_settings_page(&ui.ctx(), SettingsPage::General);
    ui.press(Modifiers::CTRL, Key::Comma);
    assert!(shows(&ui, "Language changes apply immediately."));

    ui.click("English");
    for language in xuan::i18n::languages() {
        assert!(
            shows(&ui, language.name()),
            "{} is not offered",
            language.tag()
        );
    }
    ui.click("Ƥšéûðö");
    assert_eq!(ui.app().config.language, Language::new("en-XA"));
    // Translated strings show at once; the rest stay English.
    assert!(shows(&ui, "[Ĝéñéŕåļ]"));
    assert!(shows(&ui, "Language"));
    assert!(!shows(&ui, "General"));

    ui.click("Ƥšéûðö");
    ui.click("简体中文");
    assert!(shows(&ui, "常规"));
    assert!(shows(&ui, "语言"));
    ui.click("完成");
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    assert_eq!(saved.language, Language::new("zh-CN"));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.toml"))
            .unwrap()
            .lines()
            .find(|line| line.starts_with("language")),
        Some("language = \"zh-CN\"")
    );
    xuan::i18n::set_language(&Language::english());
}

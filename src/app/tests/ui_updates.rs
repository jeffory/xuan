//! Help → Check for Updates… and Settings → General → Check for updates (issue 118), clicked
//! through with a fake GitHub.

use super::super::update_check::{FakeGitHub, current_tag, newer_tag};
use super::*;
use egui::{Key, Modifiers};

/// An editor that asks `github` for releases, with its preferences in a temporary folder.
fn editor(github: &std::sync::Arc<FakeGitHub>) -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    ui.app_mut().updates.transport = Some(github.clone());
    (directory, ui)
}

/// Runs frames until the check under way has answered and its dialog has drawn.
fn wait(ui: &mut UiTest) {
    for _ in 0..1000 {
        ui.settle();
        if ui.app().updates.running.is_none() {
            ui.settle();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the update check never answered");
}

fn check_from_the_help_menu(ui: &mut UiTest) {
    ui.open_menu("Help");
    ui.click("Check for Updates…");
    wait(ui);
}

/// The address the last frame asked the system to open.
fn opened_url(ui: &UiTest) -> Option<String> {
    ui.harness
        .output()
        .platform_output
        .commands
        .iter()
        .find_map(|command| match command {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
}

#[test]
fn a_newer_release_shows_its_notes_and_download_opens_its_page() {
    let notes = "## What's new\n- **Faster** brushes, see [the docs](https://evil.example)\n";
    let github = FakeGitHub::release(&newer_tag(), notes);
    let (_directory, mut ui) = editor(&github);
    check_from_the_help_menu(&mut ui);
    assert!(ui.has("A new version of Xuan is available."));
    assert!(ui.has("What's new"));
    assert!(ui.has("Faster brushes, see the docs"));
    assert!(ui.has("Later") && ui.has("Skip This Version"));
    ui.harness
        .get_by_role_and_label(Role::Button, "Download")
        .click();
    ui.harness.step();
    let page = format!(
        "https://github.com/{}/releases/tag/{}",
        xuan::update::REPOSITORY,
        newer_tag()
    );
    assert_eq!(opened_url(&ui), Some(page));
    ui.settle();
    assert!(!ui.has("A new version of Xuan is available."));
    assert!(ui.app().dialog.is_none());
    assert_eq!(github.requests(), 1);
}

#[test]
fn skip_this_version_hides_that_version_from_the_daily_check_only() {
    let newer = newer_tag();
    let github = FakeGitHub::release(&newer, "");
    let (directory, mut ui) = editor(&github);
    check_from_the_help_menu(&mut ui);
    assert!(ui.has("This release has no notes."));
    ui.click("Skip This Version");
    assert!(ui.app().dialog.is_none());
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    let skipped = xuan::update::parse_version(&newer).unwrap().to_string();
    assert_eq!(saved.updates.skipped.as_deref(), Some(skipped.as_str()));
    assert_eq!(opened_url(&ui), None);

    // The daily check stays quiet about it (a day later: the check above counts)...
    ui.app_mut().config.updates.check = true;
    ui.app_mut().start_daily_update_check();
    assert_eq!(github.requests(), 1, "checked less than a day ago");
    ui.app_mut().config.updates.last_check = None;
    ui.app_mut().start_daily_update_check();
    wait(&mut ui);
    assert_eq!(github.requests(), 2);
    assert!(ui.app().dialog.is_none());

    // ...but not about the release after it.
    let later = xuan::update::parse_version(&newer).unwrap();
    let after = FakeGitHub::release(&format!("v{}.{}.1", later.major, later.minor), "");
    ui.app_mut().updates.transport = Some(after.clone());
    ui.app_mut().config.updates.last_check = None;
    ui.app_mut().start_daily_update_check();
    wait(&mut ui);
    assert_eq!(after.requests(), 1);
    assert_eq!(
        ui.app().dialog,
        Some(Dialog::Update),
        "{:?}",
        ui.app().updates.view
    );
    assert!(ui.has("A new version of Xuan is available."));
    ui.click("Later");
    assert!(ui.app().dialog.is_none());
}

#[test]
fn up_to_date_and_offline_say_so_and_close_with_ok() {
    let github = FakeGitHub::release(&current_tag(), "");
    let (_directory, mut ui) = editor(&github);
    check_from_the_help_menu(&mut ui);
    assert!(ui.has("Xuan is up to date."));
    ui.click("OK");
    assert!(ui.app().dialog.is_none());

    let github = FakeGitHub::offline();
    let (_directory, mut ui) = editor(&github);
    check_from_the_help_menu(&mut ui);
    assert!(ui.has("Couldn't check for updates."));
    // Escape closes it too.
    ui.key(Key::Escape);
    assert!(ui.app().dialog.is_none());
}

#[test]
fn turning_the_setting_on_checks_and_shows_the_release_after_settings_close() {
    let github = FakeGitHub::release(&newer_tag(), "");
    let (directory, mut ui) = editor(&github);
    ui.press(Modifiers::CTRL, Key::Comma);
    assert!(ui.has_role(Role::CheckBox, "Check for updates"));
    assert_eq!(github.requests(), 0, "opening Settings asks nothing");
    ui.click_role(Role::CheckBox, "Check for updates");
    assert!(ui.app().config.updates.check);
    wait(&mut ui);
    assert_eq!(github.requests(), 1);
    // Settings stays in front until it is closed.
    assert_eq!(ui.app().dialog, Some(Dialog::Settings));
    ui.click("Done");
    assert_eq!(ui.app().dialog, Some(Dialog::Update));
    assert!(ui.has("A new version of Xuan is available."));
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    assert!(saved.updates.check && saved.updates.last_check.is_some());

    // Turning it off and on again within the day does not ask again.
    ui.click("Later");
    ui.press(Modifiers::CTRL, Key::Comma);
    ui.click_role(Role::CheckBox, "Check for updates");
    ui.click_role(Role::CheckBox, "Check for updates");
    ui.settle();
    assert_eq!(github.requests(), 1);
}

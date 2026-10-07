//! File → Open Recent (issue 112) and the remembered Move options (issue 119).

use super::*;
use crate::app::recent::{LABEL_CHARS, elide_path, safe_path};
use std::path::PathBuf;
use xuan::config::{Config, MAX_RECENT_FILES};

fn image(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    image::RgbaImage::from_pixel(6, 4, image::Rgba([200, 30, 30, 255]))
        .save(&path)
        .unwrap();
    path
}

fn isolated() -> (tempfile::TempDir, UiTest) {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::new();
    ui.isolate_config(directory.path());
    (directory, ui)
}

fn recent(ui: &UiTest) -> Vec<PathBuf> {
    ui.app().config.recent_files.clone()
}

fn saved_config(directory: &Path) -> Config {
    Config::load(&directory.join("config.toml")).unwrap()
}

#[test]
fn opening_adds_to_the_front_without_duplicates_and_is_capped() {
    let (directory, mut ui) = isolated();
    let files: Vec<_> = (0..MAX_RECENT_FILES + 3)
        .map(|n| image(directory.path(), &format!("p{n}.png")))
        .collect();
    for file in &files {
        ui.app_mut().open_path(file, false);
    }
    let list = recent(&ui);
    assert_eq!(list.len(), MAX_RECENT_FILES);
    assert_eq!(list[0], *files.last().unwrap());
    // Opening an older one again moves it up, once.
    ui.app_mut().open_path(&files[10], false);
    let list = recent(&ui);
    assert_eq!(list[0], files[10]);
    assert_eq!(list.iter().filter(|p| **p == files[10]).count(), 1);
    assert_eq!(list.len(), MAX_RECENT_FILES);
    assert_eq!(saved_config(directory.path()).recent_files, list);
}

#[test]
fn a_file_that_fails_to_open_is_not_remembered_and_layers_are_not_listed() {
    let (directory, mut ui) = isolated();
    let broken = directory.path().join("broken.png");
    std::fs::write(&broken, b"not an image").unwrap();
    ui.app_mut().open_path(&broken, false);
    assert!(ui.app().error.is_some());
    assert!(recent(&ui).is_empty());
    ui.app_mut().error = None;
    let good = image(directory.path(), "good.png");
    ui.app_mut().open_path(&good, false);
    let layer = image(directory.path(), "layer.png");
    ui.app_mut().open_path(&layer, true);
    assert_eq!(recent(&ui), [good]);
}

#[test]
fn saving_adds_the_project() {
    let (directory, mut ui) = isolated();
    ui.app_mut().dimensions = [8, 8];
    ui.app_mut().new_document();
    let project = directory.path().join("saved.xuan");
    ui.app_mut().session_mut().unwrap().path = Some(project.clone());
    assert!(ui.app_mut().save_current(false));
    assert_eq!(recent(&ui), [project.clone()]);
    assert_eq!(saved_config(directory.path()).recent_files, [project]);
}

#[test]
fn the_list_survives_a_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let mut config = Config::default();
    config.save(&path).unwrap();
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("recent_files")
    );
    config.recent_files = vec!["/a/b.png".into(), "/c/d e.xuan".into()];
    config.save(&path).unwrap();
    assert_eq!(Config::load(&path).unwrap(), config);
}

#[test]
fn clicking_an_entry_opens_it_and_clear_empties_the_list() {
    let (directory, mut ui) = isolated();
    let file = image(directory.path(), "pic.png");
    ui.app_mut().config.recent_files = vec![file.clone()];
    ui.settle();
    ui.open_menu("File");
    ui.click("Open Recent ⏵");
    let label = elide_path(&file, LABEL_CHARS);
    assert!(ui.enabled(&label));
    ui.click(&label);
    assert_eq!(ui.app().sessions.len(), 1);
    assert_eq!(
        ui.app().session().unwrap().source.as_deref(),
        Some(file.as_path())
    );
    assert_eq!(recent(&ui), [file.clone()]);

    // Choosing it again switches to the tab instead of opening a second one.
    ui.app_mut().dimensions = [8, 8];
    ui.app_mut().new_document();
    ui.settle();
    ui.open_menu("File");
    ui.click("Open Recent ⏵");
    ui.click(&label);
    assert_eq!(ui.app().sessions.len(), 2);
    assert_eq!(ui.app().current, 0);

    ui.open_menu("File");
    ui.click("Open Recent ⏵");
    ui.click("Clear Recently Opened");
    assert!(recent(&ui).is_empty());
    assert!(saved_config(directory.path()).recent_files.is_empty());
}

#[test]
fn a_missing_file_is_greyed_out_and_leaves_the_list_when_chosen_anyway() {
    let (directory, mut ui) = isolated();
    let gone = directory.path().join("gone.png");
    ui.app_mut().config.recent_files = vec![gone.clone()];
    ui.settle();
    ui.open_menu("File");
    ui.click("Open Recent ⏵");
    assert!(!ui.enabled(&elide_path(&gone, LABEL_CHARS)));

    ui.app_mut().open_recent(&gone);
    assert!(ui.app().error.is_some());
    assert!(recent(&ui).is_empty());
    assert!(ui.app().sessions.is_empty());
}

#[test]
fn the_palette_commands_follow_the_list() {
    let (directory, mut ui) = isolated();
    assert!(!ui.app().command_enabled("open_latest_recent"));
    assert!(!ui.app().command_enabled("clear_recent"));
    let file = image(directory.path(), "latest.png");
    ui.app_mut().config.recent_files = vec![file];
    assert!(ui.app().command_enabled("open_latest_recent"));
    ui.app_mut().run_command("open_latest_recent");
    assert_eq!(ui.app().sessions.len(), 1);
    ui.app_mut().run_command("clear_recent");
    assert!(recent(&ui).is_empty());
}

#[test]
fn long_paths_are_elided_and_control_characters_defused() {
    let long = Path::new("/home/someone/Pictures/holidays/2026/summer/beach/IMG_0001.png");
    let label = elide_path(long, 30);
    assert_eq!(label.chars().count(), 30, "{label}");
    assert!(label.starts_with('…') && label.ends_with("IMG_0001.png"));
    let short = Path::new("/a/b.png");
    assert_eq!(elide_path(short, 30), safe_path(short));
    let name = "x".repeat(80) + ".png";
    assert_eq!(elide_path(Path::new(&name), 20).chars().count(), 20);
    let nasty = Path::new("/tmp/evil\nname\u{1b}[31m.png");
    assert!(!safe_path(nasty).chars().any(char::is_control));
    assert!(!elide_path(nasty, 10).chars().any(char::is_control));
}

#[test]
fn move_options_persist_and_are_applied_on_startup() {
    let (directory, mut ui) = isolated();
    assert!(ui.app().auto_select && ui.app().ignore_transparent_pixels && ui.app().show_controls);
    ui.app_mut().auto_select = false;
    ui.app_mut().ignore_transparent_pixels = false;
    ui.app_mut().show_controls = false;
    ui.settle();
    let saved = saved_config(directory.path());
    assert!(!saved.auto_select && !saved.ignore_transparent_pixels && !saved.show_controls);

    // A new launch reads them back.
    let (_other, mut fresh) = isolated();
    fresh.app_mut().config = saved;
    fresh.app_mut().apply_move_options();
    assert!(!fresh.app().auto_select);
    assert!(!fresh.app().ignore_transparent_pixels);
    assert!(!fresh.app().show_controls);
}

#[test]
fn the_move_checkboxes_save_when_clicked() {
    let (directory, mut ui) = isolated();
    ui.app_mut().dimensions = [20, 16];
    ui.app_mut().new_document();
    ui.settle();
    ui.click("Auto Select");
    assert!(!ui.app().auto_select);
    assert!(!saved_config(directory.path()).auto_select);
    assert!(saved_config(directory.path()).show_controls);
}

#[test]
fn old_configuration_files_get_todays_defaults() {
    let old: Config = toml::from_str("pixel_grid = false\n").unwrap();
    assert!(old.recent_files.is_empty());
    assert!(old.auto_select && old.ignore_transparent_pixels && old.show_controls);
}

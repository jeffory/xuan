//! Reloading a project another program changed on disk, with the real file watcher.
//!
//! The changes arrive on the watcher's thread, so each test waits for them with a deadline,
//! running frames meanwhile, instead of sleeping for a fixed time.

use super::*;
use std::time::{Duration, Instant};

const CHANGED: &str = "This file was changed by another program.";

/// A 20x16 project with three blank layers, A, B and C, saved as `name` in `directory`.
fn saved_project(directory: &Path, name: &str) -> PathBuf {
    let mut document = Document::new(20, 16).unwrap();
    document.layers = ["A", "B", "C"]
        .map(|layer| Layer::blank(layer, 20, 16))
        .into();
    document.select(document.layers[2].id, false);
    let path = directory.join(name);
    io::save(&document, &path).unwrap();
    path
}

/// Replaces the project at `path` the way a careful script does: written beside it, then
/// renamed over it. `change` edits what the file holds.
fn replace_file(path: &Path, change: impl FnOnce(&mut Document)) {
    let mut document = io::load(path).unwrap();
    change(&mut document);
    let temporary = path.with_extension("part");
    io::save(&document, &temporary).unwrap();
    std::fs::rename(temporary, path).unwrap();
}

fn rename_b(document: &mut Document) {
    document.layers[1].name = "Renamed".into();
}

/// Opens `path` and waits until its file is followed.
fn open(ui: &mut UiTest, path: &Path) {
    assert!(ui.app_mut().open_path(path, false));
    ui.settle();
    ui.app().file_watch.flush();
}

/// Runs frames until `done`, failing the test after a generous deadline.
#[track_caller]
fn wait_until(ui: &mut UiTest, what: &str, done: impl Fn(&UiTest) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        ui.harness.step();
        if done(ui) {
            ui.settle();
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn layer_names(ui: &UiTest) -> Vec<String> {
    let session = ui.app().session().unwrap();
    session
        .document
        .layers
        .iter()
        .map(|l| l.name.clone())
        .collect()
}

#[test]
fn an_unchanged_project_reloads_in_place_keeping_the_view_and_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = saved_project(directory.path(), "project.xuan");
    let mut ui = UiTest::new();
    open(&mut ui, &path);
    let b = ui.app().session().unwrap().document.layers[1].id;
    // An edit, saved: there is undo history but nothing unsaved.
    ui.app_mut().command("new_layer");
    assert!(ui.app_mut().save_current(false));
    let session = ui.app_mut().session_mut().unwrap();
    let id = session.document.id;
    session.document.select(b, false);
    session.zoom = 2.5;
    session.pan = egui::vec2(30.0, -20.0);
    session.fit = false;
    ui.settle();
    assert!(ui.app().session().unwrap().history.undo_name().is_some());

    replace_file(&path, rename_b);
    wait_until(&mut ui, "the reload", |ui| {
        layer_names(ui).contains(&"Renamed".to_owned())
    });

    let app = ui.app();
    assert_eq!(app.sessions.len(), 1, "the same tab");
    let session = app.session().unwrap();
    assert_eq!(session.document.id, id);
    assert_eq!(session.title, "project");
    assert_eq!(
        (session.zoom, session.pan, session.fit),
        (2.5, egui::vec2(30.0, -20.0), false)
    );
    assert_eq!(session.document.selected, [b].into());
    assert_eq!(session.document.active, Some(b));
    assert_eq!(session.history.undo_name(), None, "undo history is cleared");
    assert!(!session.history.dirty());
    assert!(!ui.has(CHANGED));
}

#[test]
fn unsaved_changes_show_the_bar_and_keep_mine_keeps_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = saved_project(directory.path(), "project.xuan");
    let mut ui = UiTest::new();
    open(&mut ui, &path);
    ui.app_mut().command("new_layer");
    ui.settle();
    let edited = layer_names(&ui);

    replace_file(&path, rename_b);
    wait_until(&mut ui, "the bar", |ui| ui.has(CHANGED));
    assert_eq!(
        layer_names(&ui),
        edited,
        "nothing is lost while the bar asks"
    );
    assert!(ui.has("Reload"));

    ui.click("Keep mine");
    assert!(!ui.has(CHANGED));
    let session = ui.app().session().unwrap();
    assert!(session.external.is_none());
    assert!(session.history.dirty());
    assert_eq!(layer_names(&ui), edited);
    // Undo still has the edit.
    ui.app_mut().command("undo");
    assert_eq!(layer_names(&ui), ["A", "B", "C"]);
}

#[test]
fn reload_on_the_bar_takes_the_files_content() {
    let directory = tempfile::tempdir().unwrap();
    let path = saved_project(directory.path(), "project.xuan");
    let mut ui = UiTest::new();
    open(&mut ui, &path);
    ui.app_mut().command("new_layer");
    ui.settle();

    replace_file(&path, rename_b);
    wait_until(&mut ui, "the bar", |ui| ui.has(CHANGED));
    ui.click("Reload");
    assert!(!ui.has(CHANGED));
    assert_eq!(layer_names(&ui), ["A", "Renamed", "C"]);
    let session = ui.app().session().unwrap();
    assert!(!session.history.dirty());
    assert_eq!(session.history.undo_name(), None);
    assert_eq!(ui.app().sessions.len(), 1);
}

#[test]
fn saving_from_xuan_and_touching_the_file_do_not_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let path = saved_project(directory.path(), "project.xuan");
    let mut ui = UiTest::new();
    open(&mut ui, &path);
    ui.app_mut().command("new_layer");
    assert!(ui.app_mut().save_current(false));
    // Unsaved again, so a change would show the bar rather than reload quietly.
    ui.app_mut().command("new_layer");
    ui.settle();
    let edited = layer_names(&ui);

    wait_until(&mut ui, "the check after saving", |ui| {
        !ui.app().file_watch.reported.is_empty()
    });
    let checks = ui.app().file_watch.reported.len();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(file);
    wait_until(&mut ui, "the check after touching", |ui| {
        ui.app().file_watch.reported.len() > checks
    });

    assert!(!ui.has(CHANGED));
    assert!(ui.app().session().unwrap().external.is_none());
    assert_eq!(layer_names(&ui), edited);
}

#[test]
fn a_change_waits_for_the_open_dialog() {
    let directory = tempfile::tempdir().unwrap();
    let path = saved_project(directory.path(), "project.xuan");
    let mut ui = UiTest::new();
    open(&mut ui, &path);
    ui.app_mut().dialog = Some(Dialog::About);

    replace_file(&path, rename_b);
    wait_until(&mut ui, "the change", |ui| {
        ui.app().session().unwrap().external.is_some()
    });
    assert_eq!(layer_names(&ui), ["A", "B", "C"]);
    assert!(!ui.has(CHANGED), "nothing unsaved, so nothing to ask");

    ui.app_mut().dialog = None;
    ui.settle();
    assert_eq!(layer_names(&ui), ["A", "Renamed", "C"]);
}

#[test]
fn closing_a_tab_stops_following_its_file() {
    let directory = tempfile::tempdir().unwrap();
    let first = saved_project(directory.path(), "first.xuan");
    let second = saved_project(directory.path(), "second.xuan");
    let mut ui = UiTest::new();
    open(&mut ui, &first);
    open(&mut ui, &second);
    let ids: Vec<Uuid> = ui.app().sessions.iter().map(|s| s.document.id).collect();
    let mut watched = ui.app().file_watch.watched();
    watched.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(watched, expected);
    // Images are not followed.
    let image = directory.path().join("image.png");
    image::RgbaImage::new(4, 4).save(&image).unwrap();
    open(&mut ui, &image);
    assert_eq!(ui.app().file_watch.watched().len(), 2);

    ui.app_mut().current = 0;
    ui.press(egui::Modifiers::CTRL, egui::Key::W);
    assert_eq!(ui.app().sessions.len(), 2);
    assert_eq!(ui.app().file_watch.watched(), [ids[1]]);
    ui.app().file_watch.flush();

    // The closed tab's file first, so a report for it would arrive first.
    replace_file(&first, rename_b);
    replace_file(&second, rename_b);
    wait_until(&mut ui, "the second file's reload", |ui| {
        ui.app().sessions[0].document.layers[1].name == "Renamed"
    });
    let reported = &ui.app().file_watch.reported;
    assert!(
        !reported.contains(&ids[0]),
        "the closed tab's file is not read"
    );
    assert!(reported.contains(&ids[1]));
}

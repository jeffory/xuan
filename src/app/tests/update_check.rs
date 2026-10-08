//! Checking for updates (issue 118) with a fake GitHub: when checks run, what they show, and
//! that neither startup nor the UI waits for the network.

use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use xuan::plugins::models::{Reply, Transport};

/// GitHub's releases API as the fake answers it: a tag, an error, or nothing until released.
pub(super) struct FakeGitHub {
    answer: Mutex<Option<std::result::Result<Vec<u8>, String>>>,
    requests: AtomicUsize,
    /// While set, a request waits here, as one over a dead network does.
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl FakeGitHub {
    /// Answers with a release tagged `tag` whose notes are `notes`.
    pub(super) fn release(tag: &str, notes: &str) -> Arc<Self> {
        let json = serde_json::json!({ "tag_name": tag, "name": tag, "body": notes });
        Self::answering(Ok(serde_json::to_vec(&json).unwrap()))
    }

    /// Fails as a computer without a network does.
    pub(super) fn offline() -> Arc<Self> {
        Self::answering(Err("Cannot connect: network unreachable".into()))
    }

    fn answering(answer: std::result::Result<Vec<u8>, String>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(Some(answer)),
            requests: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }

    pub(super) fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Transport for FakeGitHub {
    fn get(&self, url: &url::Url, _size: u64) -> anyhow::Result<Reply> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            url.as_str(),
            xuan::update::api_url(xuan::update::REPOSITORY)?.as_str()
        );
        if let Some(gate) = self.gate.lock().unwrap().take() {
            let _ = gate.recv();
        }
        let answer =
            (self.answer.lock().unwrap().clone()).ok_or_else(|| anyhow::anyhow!("no answer"))?;
        let body = answer.map_err(anyhow::Error::msg)?;
        Ok(Reply {
            status: 200,
            location: None,
            length: Some(body.len() as u64),
            body: Box::new(std::io::Cursor::new(body)),
        })
    }
}

/// A tag newer than this build, and this build's own.
pub(super) fn newer_tag() -> String {
    let current = xuan::update::current_version().unwrap();
    format!("v{}.0.0", current.major + 1)
}

pub(super) fn current_tag() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

/// An app that asks `github` for releases, with its preferences in a temporary folder.
fn editor(github: &Arc<FakeGitHub>) -> (tempfile::TempDir, egui::Context, EditorApp) {
    let directory = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    app.config_path = Some(directory.path().join("config.toml"));
    app.updates.transport = Some(github.clone());
    (directory, context, app)
}

/// Runs frames until the check under way has answered.
fn wait_for_answer(context: &egui::Context, app: &mut EditorApp) {
    for _ in 0..1000 {
        frame(context, app);
        if app.updates.running.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the update check never answered");
}

fn available(app: &EditorApp) -> Option<&xuan::update::Release> {
    match &app.updates.view {
        Some(updates::View::Available(release, _)) => Some(release),
        _ => None,
    }
}

#[test]
fn with_the_check_off_no_request_is_made() {
    let github = FakeGitHub::release(&newer_tag(), "");
    let (_directory, context, mut app) = editor(&github);
    assert!(!app.config.updates.check, "off by default");
    app.start_daily_update_check();
    for _ in 0..5 {
        frame(&context, &mut app);
    }
    assert_eq!(github.requests(), 0);
    assert!(app.updates.running.is_none() && app.dialog.is_none());
    assert_eq!(app.config.updates.last_check, None);
}

#[test]
fn the_daily_check_shows_a_newer_release_once_a_day() {
    let github = FakeGitHub::release(&newer_tag(), "## New\n- Things");
    let (directory, context, mut app) = editor(&github);
    app.config.updates.check = true;
    app.start_daily_update_check();
    wait_for_answer(&context, &mut app);
    assert_eq!(github.requests(), 1);
    assert_eq!(app.dialog, Some(Dialog::Update));
    let release = available(&app).expect("the newer release");
    assert_eq!(release.tag, newer_tag());
    // When it ran is saved, so the next start within a day does not ask again.
    let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
    let last = saved.updates.last_check.expect("recorded");
    assert!(xuan::update::now().abs_diff(last) < 60);
    app.start_daily_update_check();
    frame(&context, &mut app);
    assert_eq!(github.requests(), 1);
    // A day later it does.
    app.config.updates.last_check = Some(last - xuan::update::INTERVAL_SECONDS);
    app.updates.view = None;
    app.dialog = None;
    app.start_daily_update_check();
    wait_for_answer(&context, &mut app);
    assert_eq!(github.requests(), 2);
}

#[test]
fn the_daily_check_is_quiet_unless_it_finds_something_new() {
    let newer = newer_tag();
    for (github, skipped) in [
        (FakeGitHub::release(&current_tag(), ""), None),
        (FakeGitHub::release("v0.0.1", ""), None),
        (FakeGitHub::offline(), None),
        (FakeGitHub::answering(Ok(b"<html>".to_vec())), None),
        (FakeGitHub::release(&newer, ""), Some(newer.clone())),
    ] {
        let (_directory, context, mut app) = editor(&github);
        app.config.updates.check = true;
        app.config.updates.skipped = skipped;
        app.start_daily_update_check();
        wait_for_answer(&context, &mut app);
        frame(&context, &mut app);
        assert_eq!(github.requests(), 1);
        assert!(app.dialog.is_none(), "{:?}", app.updates.view);
        assert!(app.updates.view.is_none());
        assert!(app.error.is_none());
    }
}

#[test]
fn checking_from_the_help_menu_shows_every_answer() {
    let newer = newer_tag();
    // Up to date.
    let github = FakeGitHub::release(&current_tag(), "");
    let (_directory, context, mut app) = editor(&github);
    app.command("check_updates");
    assert_eq!(app.dialog, Some(Dialog::Update));
    assert_eq!(app.updates.view, Some(updates::View::Checking));
    wait_for_answer(&context, &mut app);
    assert_eq!(app.updates.view, Some(updates::View::UpToDate));
    // Offline: says so, in the dialog rather than as an error.
    let github = FakeGitHub::offline();
    let (_directory, context, mut app) = editor(&github);
    app.command("check_updates");
    wait_for_answer(&context, &mut app);
    match &app.updates.view {
        Some(updates::View::Failed(error)) => assert!(error.contains("unreachable"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert!(app.error.is_none());
    // A skipped version is still shown when asked for, and it checks even with the daily
    // check off.
    let github = FakeGitHub::release(&newer, "");
    let (_directory, context, mut app) = editor(&github);
    app.config.updates.skipped = Some(newer.clone());
    app.command("check_updates");
    wait_for_answer(&context, &mut app);
    assert_eq!(
        available(&app).map(|r| r.tag.as_str()),
        Some(newer.as_str())
    );
    assert_eq!(github.requests(), 1);
}

#[test]
fn closing_the_dialog_while_checking_makes_the_answer_quiet() {
    let github = FakeGitHub::release(&current_tag(), "");
    let (sender, gate) = std::sync::mpsc::channel();
    *github.gate.lock().unwrap() = Some(gate);
    let (_directory, context, mut app) = editor(&github);
    app.command("check_updates");
    frame(&context, &mut app);
    app.dialog = None;
    frame(&context, &mut app);
    assert!(app.updates.view.is_none());
    sender.send(()).unwrap();
    wait_for_answer(&context, &mut app);
    frame(&context, &mut app);
    assert!(app.dialog.is_none() && app.updates.view.is_none());
}

#[test]
fn startup_and_the_ui_do_not_wait_for_the_network() {
    let github = FakeGitHub::release(&newer_tag(), "");
    let (sender, gate) = std::sync::mpsc::channel();
    *github.gate.lock().unwrap() = Some(gate);
    let (_directory, context, mut app) = editor(&github);
    app.config.updates.check = true;
    let start = std::time::Instant::now();
    app.start_daily_update_check();
    for _ in 0..10 {
        frame(&context, &mut app);
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(app.updates.running.is_some(), "still waiting");
    assert!(app.dialog.is_none());
    // A second check while one is under way does not ask again.
    app.command("check_updates");
    frame(&context, &mut app);
    sender.send(()).unwrap();
    wait_for_answer(&context, &mut app);
    assert_eq!(github.requests(), 1);
    assert!(available(&app).is_some());
}

#[test]
fn a_newer_release_waits_for_other_dialogs_to_close() {
    let github = FakeGitHub::release(&newer_tag(), "");
    let (_directory, context, mut app) = editor(&github);
    app.config.updates.check = true;
    app.dialog = Some(Dialog::About);
    app.start_daily_update_check();
    wait_for_answer(&context, &mut app);
    assert_eq!(app.dialog, Some(Dialog::About));
    app.dialog = None;
    frame(&context, &mut app);
    assert_eq!(app.dialog, Some(Dialog::Update));
}

#[test]
fn tests_never_reach_the_network() {
    let (context, mut app) = app();
    app.command("check_updates");
    wait_for_answer(&context, &mut app);
    match &app.updates.view {
        Some(updates::View::Failed(error)) => assert!(error.contains("no network"), "{error}"),
        other => panic!("{other:?}"),
    }
}

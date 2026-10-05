//! UI interaction tests: the real `EditorApp` driven through `egui_kittest`.
//!
//! Unlike the other app tests, these never call `command()` or poke state to start a flow. They
//! click widgets found by their AccessKit label and press real key chords, so they notice a menu
//! item that lost its wiring, a button that is disabled when it should not be, or a shortcut that a
//! focused widget swallows. See "Writing UI tests" in `docs/DEVELOPMENT.md`.

use super::*;
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

#[path = "ui_shortcuts.rs"]
mod shortcuts;

/// An `EditorApp` running inside a kittest harness at the usual 1280x860 window size.
pub(super) struct UiTest {
    harness: Harness<'static, Option<EditorApp>>,
}

impl UiTest {
    /// An empty editor: no documents open.
    pub(super) fn new() -> Self {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1280.0, 860.0))
            .build_state(
                |ctx, app: &mut Option<EditorApp>| {
                    if let Some(app) = app {
                        app.show(ctx);
                    }
                },
                None,
            );
        // The app installs fonts and the theme on the context it is built with, so it must be built
        // from the harness's own context rather than a throwaway one.
        *harness.state_mut() = Some(EditorApp::with_context(
            &harness.ctx,
            Vec::new(),
            false,
            None,
        ));
        let mut test = Self { harness };
        test.settle();
        test
    }

    /// An editor with one blank 20x16 document.
    pub(super) fn with_document() -> Self {
        let mut test = Self::new();
        test.app_mut().dimensions = [20, 16];
        test.app_mut().new_document();
        test.settle();
        test
    }

    pub(super) fn app(&self) -> &EditorApp {
        self.harness.state().as_ref().unwrap()
    }

    pub(super) fn app_mut(&mut self) -> &mut EditorApp {
        self.harness.state_mut().as_mut().unwrap()
    }

    pub(super) fn ctx(&self) -> egui::Context {
        self.harness.ctx.clone()
    }

    /// Runs a few frames so queued input is processed and floating windows have measured themselves.
    pub(super) fn settle(&mut self) {
        self.harness.run_steps(3);
    }

    pub(super) fn has(&self, label: &str) -> bool {
        self.harness.query_by_label(label).is_some()
    }

    /// Clicks the widget with this exact accessibility label, then settles.
    #[track_caller]
    pub(super) fn click(&mut self, label: &str) {
        self.harness.get_by_label(label).click();
        self.settle();
    }

    /// Clicks without the trailing frames, so `output()` still shows the frame that handled it.
    #[track_caller]
    pub(super) fn click_and_stop(&mut self, label: &str) {
        self.harness.get_by_label(label).click();
        self.harness.step();
    }

    /// Like `click`, for labels that several widgets share.
    #[allow(dead_code)]
    #[track_caller]
    pub(super) fn click_role(&mut self, role: Role, label: &str) {
        self.harness.get_by_role_and_label(role, label).click();
        self.settle();
    }

    /// Whether the labelled widget is enabled (panics if it is not on screen).
    #[track_caller]
    pub(super) fn enabled(&self, label: &str) -> bool {
        !self
            .harness
            .get_by_label(label)
            .accesskit_node()
            .is_disabled()
    }

    /// Opens a menu bar menu such as "File" by clicking it.
    pub(super) fn open_menu(&mut self, menu: &str) {
        self.click(menu);
    }

    /// Presses and releases a key chord, as the keyboard would.
    pub(super) fn press(&mut self, modifiers: egui::Modifiers, key: egui::Key) {
        self.harness.key_press_modifiers(modifiers, key);
        self.settle();
    }

    pub(super) fn key(&mut self, key: egui::Key) {
        self.press(egui::Modifiers::NONE, key);
    }

    /// Delivers files dropped onto the window.
    pub(super) fn drop_files(&mut self, paths: &[&Path]) {
        self.harness.input_mut().dropped_files = paths
            .iter()
            .map(|path| egui::DroppedFile {
                path: Some(path.to_path_buf()),
                ..Default::default()
            })
            .collect();
        self.settle();
    }

    /// Whether the last frame asked the window system to close the window.
    pub(super) fn close_requested(&self) -> bool {
        self.harness
            .output()
            .viewport_output
            .values()
            .any(|viewport| {
                viewport
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Close))
            })
    }
}

// Menu items are labelled "<name> <shortcut hint>" for assistive technology, so matching the whole
// label also pins the shortcut hint shown to the user.
const OPEN: &str = "Open… Ctrl+O";
const SAVE: &str = "Save Ctrl+S";
const SAVE_AS: &str = "Save As… Ctrl+Shift+S";
const EXPORT: &str = "Export Image… Ctrl+Alt+Shift+S";
const CLOSE: &str = "Close Project Ctrl+W";
const QUIT: &str = "Quit Ctrl+Q";

fn dirty_document() -> UiTest {
    let mut ui = UiTest::with_document();
    ui.app_mut().command("fill_fg");
    ui.settle();
    assert!(ui.app().session().unwrap().history.dirty());
    ui
}

#[test]
fn file_menu_lists_every_entry_disabled_without_a_document() {
    let mut ui = UiTest::new();
    ui.open_menu("File");
    for label in [OPEN, QUIT] {
        assert!(ui.enabled(label), "{label} should be enabled");
    }
    for label in [SAVE, SAVE_AS, EXPORT, CLOSE] {
        assert!(!ui.enabled(label), "{label} needs a document");
    }
}

#[test]
fn file_menu_enables_document_entries_once_a_document_is_open() {
    let mut ui = UiTest::with_document();
    ui.open_menu("File");
    for label in [OPEN, SAVE, SAVE_AS, EXPORT, CLOSE, QUIT] {
        assert!(ui.enabled(label), "{label} should be enabled");
    }
}

#[test]
fn file_menu_close_project_closes_a_clean_document() {
    let mut ui = UiTest::with_document();
    ui.open_menu("File");
    ui.click(CLOSE);
    assert!(ui.app().sessions.is_empty());
    assert!(ui.app().close_tab.is_none());
}

#[test]
fn file_menu_quit_asks_the_window_to_close() {
    let mut ui = UiTest::new();
    ui.open_menu("File");
    ui.click_and_stop(QUIT);
    assert!(ui.close_requested());
}

#[test]
fn file_menu_quit_with_unsaved_changes_prompts_instead_of_closing() {
    let mut ui = dirty_document();
    ui.open_menu("File");
    ui.click_and_stop(QUIT);
    assert!(!ui.close_requested());
    ui.settle();
    assert!(ui.app().close_app);
    assert!(ui.has("Save your changes?"));
    ui.click("Cancel");
    assert!(!ui.app().close_app);
    assert_eq!(ui.app().sessions.len(), 1);
}

#[test]
fn closing_a_dirty_document_prompts_and_cancel_keeps_it() {
    let mut ui = dirty_document();
    ui.open_menu("File");
    ui.click(CLOSE);
    assert_eq!(ui.app().close_tab, Some(0));
    assert!(ui.has("Save your changes?"));
    // The prompt blocks the rest of the window: the File menu is disabled behind it.
    assert!(!ui.enabled("File"));

    ui.click("Cancel");
    assert!(ui.app().close_tab.is_none());
    assert_eq!(ui.app().sessions.len(), 1);
    assert!(!ui.has("Save your changes?"));
}

#[test]
fn closing_a_dirty_document_discard_closes_it() {
    let mut ui = dirty_document();
    ui.open_menu("File");
    ui.click(CLOSE);
    ui.click("Discard changes");
    assert!(ui.app().sessions.is_empty());
    assert!(ui.app().close_tab.is_none());
    assert!(!ui.has("Save your changes?"));
}

#[test]
fn ctrl_w_on_a_dirty_document_prompts() {
    let mut ui = dirty_document();
    ui.press(egui::Modifiers::CTRL, egui::Key::W);
    assert_eq!(ui.app().close_tab, Some(0));
    assert!(ui.has("Save your changes?"));
}

mod develop {
    use super::*;
    use crate::app::develop::{CanvasTool, tests::ready};

    fn develop_ui() -> UiTest {
        let mut ui = UiTest::new();
        let develop = ready(&ui.ctx());
        ui.app_mut().develop = Some(develop);
        ui.settle();
        ui
    }

    fn tool(ui: &UiTest) -> CanvasTool {
        ui.app().develop.as_ref().unwrap().tool
    }

    #[test]
    fn reset_restores_default_settings_and_leaves_pick_mode() {
        let mut ui = develop_ui();
        {
            let develop = ui.app_mut().develop.as_mut().unwrap();
            develop.settings.contrast = 25.0;
            develop.tool = CanvasTool::WhiteBalance;
        }
        ui.click("Reset");
        let develop = ui.app().develop.as_ref().unwrap();
        assert_eq!(develop.settings, Default::default());
        assert_eq!(develop.tool, CanvasTool::None);
    }

    #[test]
    fn pick_modes_are_exclusive_and_toggle_off() {
        let mut ui = develop_ui();
        ui.click("Masks");
        ui.click("+ Linear");
        assert_eq!(tool(&ui), CanvasTool::DrawMask);
        ui.click("Basic");
        ui.click("Pick neutral");
        assert_eq!(tool(&ui), CanvasTool::WhiteBalance);
        ui.click("Pick neutral");
        assert_eq!(tool(&ui), CanvasTool::None);
    }

    #[test]
    fn escape_leaves_pick_mode() {
        let mut ui = develop_ui();
        ui.click("Pick neutral");
        assert_eq!(tool(&ui), CanvasTool::WhiteBalance);
        ui.key(egui::Key::Escape);
        assert_eq!(tool(&ui), CanvasTool::None);
        // The develop session itself survives Escape.
        assert!(ui.app().develop.is_some());
    }
}

mod drop_prompt {
    use super::*;

    /// A document with one image dropped on it, so the prompt is showing.
    fn prompted() -> (tempfile::TempDir, UiTest) {
        let (directory, image, _) = drop_fixture();
        let mut ui = UiTest::with_document();
        ui.drop_files(&[&image]);
        assert!(ui.app().dialog == Some(Dialog::DropChoice));
        assert!(ui.has("Add dropped files"));
        (directory, ui)
    }

    #[test]
    fn enter_inserts_as_a_layer() {
        let (_directory, mut ui) = prompted();
        let layers = ui.app().session().unwrap().document.layers.len();
        ui.key(egui::Key::Enter);
        assert!(ui.app().dialog.is_none());
        assert_eq!(ui.app().sessions.len(), 1);
        assert_eq!(
            ui.app().session().unwrap().document.layers.len(),
            layers + 1
        );
    }

    #[test]
    fn escape_cancels() {
        let (_directory, mut ui) = prompted();
        let layers = ui.app().session().unwrap().document.layers.len();
        ui.key(egui::Key::Escape);
        assert!(ui.app().dialog.is_none());
        assert_eq!(ui.app().sessions.len(), 1);
        assert_eq!(ui.app().session().unwrap().document.layers.len(), layers);
    }

    #[test]
    fn buttons_insert_open_or_cancel() {
        let (_directory, mut ui) = prompted();
        let layers = ui.app().session().unwrap().document.layers.len();
        ui.click("Insert as layer");
        assert_eq!(
            ui.app().session().unwrap().document.layers.len(),
            layers + 1
        );

        let (_directory, mut ui) = prompted();
        ui.click("Open as new document");
        assert_eq!(ui.app().sessions.len(), 2);

        let (_directory, mut ui) = prompted();
        ui.click("Cancel");
        assert_eq!(ui.app().sessions.len(), 1);
        assert_eq!(ui.app().session().unwrap().document.layers.len(), layers);
    }

    #[test]
    fn shortcuts_are_blocked_while_the_prompt_is_open() {
        let (_directory, mut ui) = prompted();
        ui.press(egui::Modifiers::CTRL, egui::Key::W);
        assert!(ui.app().close_tab.is_none());
        assert!(ui.app().dialog == Some(Dialog::DropChoice));
    }
}

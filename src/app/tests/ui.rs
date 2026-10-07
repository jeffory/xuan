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

#[path = "ui_keybindings.rs"]
mod keybindings;

#[path = "ui_palette.rs"]
mod palette;

#[path = "ui_appearance.rs"]
mod appearance;

#[path = "ui_tabs.rs"]
mod tabs;

#[cfg(target_os = "linux")]
#[path = "ui_window_buttons.rs"]
mod window_buttons;

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

    /// Like `has`, for labels that several widgets share.
    pub(super) fn has_role(&self, role: Role, label: &str) -> bool {
        self.harness.query_by_role_and_label(role, label).is_some()
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

    /// Clicks the only text field on screen and types `text` into it.
    #[track_caller]
    pub(super) fn type_in_text_field(&mut self, text: &str) {
        self.harness.get_by_role(Role::TextInput).click();
        self.harness.step();
        self.harness.get_by_role(Role::TextInput).type_text(text);
        self.settle();
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

    /// Types `text` as the keyboard would, into whatever has focus.
    pub(super) fn type_keys(&mut self, text: &str) {
        self.harness
            .input_mut()
            .events
            .push(egui::Event::Text(text.to_owned()));
        self.settle();
    }

    /// Clicks the primary button at `pos`.
    pub(super) fn click_at(&mut self, pos: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(pos));
        self.harness.step();
        self.harness.input_mut().events.push(button(true));
        self.harness.step();
        self.harness.input_mut().events.push(button(false));
        self.settle();
    }

    pub(super) fn key(&mut self, key: egui::Key) {
        self.press(egui::Modifiers::NONE, key);
    }

    /// Presses the primary button at `from`, moves to `to` in a few steps and releases there.
    pub(super) fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(from));
        self.harness.step();
        self.harness.input_mut().events.push(button(from, true));
        self.harness.step();
        for step in 1..=4 {
            let pos = from + (to - from) * (step as f32 / 4.0);
            self.harness
                .input_mut()
                .events
                .push(egui::Event::PointerMoved(pos));
            self.harness.step();
        }
        self.harness.input_mut().events.push(button(to, false));
        self.settle();
    }

    /// Keeps preferences changed by the test in `directory` instead of the user's own file.
    pub(super) fn isolate_config(&mut self, directory: &Path) {
        self.app_mut().config_path = Some(directory.join("config.toml"));
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

    /// Text the last frame put on the clipboard.
    pub(super) fn copied_text(&self) -> Option<String> {
        self.harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
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

mod rulers_and_guides {
    use super::*;
    use crate::app::rulers::RULER_SIZE;
    use egui::{Key, Modifiers, Pos2, pos2};
    use xuan::layout::GuideAxis;

    fn document() -> (tempfile::TempDir, UiTest) {
        let directory = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(directory.path());
        (directory, ui)
    }

    /// The screen position of document point (`x`, `y`).
    fn at(ui: &UiTest, x: f32, y: f32) -> Pos2 {
        let origin = ui.app().canvas_rect.unwrap().min;
        let zoom = ui.app().session().unwrap().zoom;
        origin + egui::vec2(x, y) * zoom
    }

    fn guides(ui: &UiTest) -> Vec<xuan::layout::Guide> {
        ui.app().session().unwrap().document.guides.clone()
    }

    #[test]
    fn ctrl_r_toggles_the_rulers() {
        let (directory, mut ui) = document();
        let before = ui.app().canvas_viewport.unwrap();
        assert!(!ui.app().config.rulers);
        ui.press(Modifiers::CTRL, Key::R);
        assert!(ui.app().config.rulers);
        // The rulers take a strip along the top and left of the canvas.
        let with_rulers = ui.app().canvas_viewport.unwrap();
        assert_eq!(
            with_rulers.min,
            before.min + egui::vec2(RULER_SIZE, RULER_SIZE)
        );
        assert_eq!(with_rulers.max, before.max);
        // The choice is remembered.
        let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
        assert!(saved.rulers);
        // R alone is still the Blur tool.
        assert!(ui.app().tool != Tool::Blur);
        ui.press(Modifiers::CTRL, Key::R);
        assert!(!ui.app().config.rulers);
        assert_eq!(ui.app().canvas_viewport.unwrap(), before);
    }

    #[test]
    fn dragging_from_a_ruler_creates_a_guide_and_dropping_it_back_deletes_it() {
        let (_directory, mut ui) = document();
        ui.open_menu("View");
        ui.click("Rulers Ctrl+R");
        assert!(ui.app().config.rulers);
        let viewport = ui.app().canvas_viewport.unwrap();
        let top_ruler = pos2(viewport.center().x, viewport.top() - RULER_SIZE / 2.0);
        let left_ruler = pos2(viewport.left() - RULER_SIZE / 2.0, viewport.center().y);

        // Out of the top ruler: a horizontal guide where the pointer is released.
        ui.drag(top_ruler, at(&ui, 6.0, 5.0));
        let created = guides(&ui);
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].axis, GuideAxis::Horizontal);
        assert!((created[0].position - 5.0).abs() < 1e-3, "{created:?}");
        assert!(ui.app().guide_drag.is_none());
        assert_eq!(
            ui.app().session().unwrap().history.undo_name(),
            Some("New Guide")
        );

        // Out of the left ruler: a vertical guide.
        ui.drag(left_ruler, at(&ui, 3.0, 12.0));
        let created = guides(&ui);
        assert_eq!(created.len(), 2);
        assert_eq!(created[1].axis, GuideAxis::Vertical);
        assert!((created[1].position - 3.0).abs() < 1e-3);

        // With the Move tool, a guide can be moved...
        assert!(ui.app().tool == Tool::Move);
        ui.drag(at(&ui, 6.0, 5.0), at(&ui, 6.0, 11.0));
        let moved = guides(&ui);
        assert!((moved[0].position - 11.0).abs() < 1e-3, "{moved:?}");
        assert_eq!(moved[0].id, created[0].id);
        // ...without moving the layer under it.
        assert_eq!(
            ui.app().session().unwrap().document.layers[0].transform.y,
            0.0
        );

        // Dropped back on a ruler, it is deleted.
        ui.drag(at(&ui, 6.0, 11.0), top_ruler);
        let left = guides(&ui);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].axis, GuideAxis::Vertical);
        ui.press(Modifiers::CTRL, Key::Z);
        assert_eq!(guides(&ui).len(), 2);
    }

    #[test]
    fn locked_or_hidden_guides_cannot_be_dragged() {
        let (_directory, mut ui) = document();
        ui.press(Modifiers::CTRL, Key::R);
        let viewport = ui.app().canvas_viewport.unwrap();
        let top_ruler = pos2(viewport.center().x, viewport.top() - RULER_SIZE / 2.0);
        ui.press(Modifiers::CTRL | Modifiers::ALT, Key::Semicolon);
        assert!(ui.app().config.lock_guides);
        ui.drag(top_ruler, at(&ui, 6.0, 5.0));
        assert!(guides(&ui).is_empty());
        ui.press(Modifiers::CTRL | Modifiers::ALT, Key::Semicolon);
        ui.drag(top_ruler, at(&ui, 6.0, 5.0));
        assert_eq!(guides(&ui).len(), 1);
        // Hidden guides are not grabbed.
        ui.press(Modifiers::CTRL, Key::Semicolon);
        assert!(!ui.app().config.show_guides);
        ui.drag(at(&ui, 6.0, 5.0), at(&ui, 6.0, 9.0));
        assert_eq!(guides(&ui)[0].position, 5.0);
    }

    #[test]
    fn view_clear_guides_removes_them_in_one_undoable_step() {
        let (_directory, mut ui) = document();
        ui.open_menu("View");
        assert!(!ui.enabled("Clear Guides"), "nothing to clear yet");
        ui.key(Key::Escape);
        ui.press(Modifiers::CTRL, Key::R);
        let viewport = ui.app().canvas_viewport.unwrap();
        ui.drag(
            pos2(viewport.center().x, viewport.top() - 4.0),
            at(&ui, 4.0, 4.0),
        );
        ui.drag(
            pos2(viewport.left() - 4.0, viewport.center().y),
            at(&ui, 13.0, 4.0),
        );
        assert_eq!(guides(&ui).len(), 2);

        ui.open_menu("View");
        ui.click("Clear Guides");
        assert!(guides(&ui).is_empty());
        assert_eq!(
            ui.app().session().unwrap().history.undo_name(),
            Some("Clear Guides")
        );
        ui.press(Modifiers::CTRL, Key::Z);
        assert_eq!(guides(&ui).len(), 2);
    }

    #[test]
    fn snap_to_menu_toggles_each_target() {
        let (_directory, mut ui) = document();
        let before = ui.app().config.snap;
        ui.open_menu("View");
        ui.click("Snap To ⏵");
        ui.click("Grid");
        assert_eq!(ui.app().config.snap.grid, !before.grid);
        ui.press(Modifiers::CTRL | Modifiers::SHIFT, Key::Semicolon);
        assert!(!ui.app().config.snap.enabled);
        ui.press(Modifiers::CTRL, Key::Quote);
        assert!(ui.app().config.show_grid);
    }

    #[test]
    fn grid_settings_previews_cancels_restores_defaults_and_applies() {
        use xuan::layout::GridSettings;
        let (directory, mut ui) = document();
        assert!(!ui.app().showing_grid());
        ui.open_menu("View");
        ui.click("Grid Settings…");
        assert!(ui.app().dialog == Some(Dialog::GridSettings));
        // The grid shows while the dialog is open, and follows the fields.
        assert!(ui.app().showing_grid());
        ui.app_mut().grid_edit.as_mut().unwrap().draft.spacing = 32;
        ui.settle();
        assert_eq!(ui.app().grid_settings().spacing, 32);
        ui.click("Cancel");
        assert!(ui.app().dialog.is_none());
        assert_eq!(ui.app().grid_settings(), GridSettings::default());
        assert!(!ui.app().showing_grid());

        ui.open_menu("View");
        ui.click("Grid Settings…");
        {
            let draft = &mut ui.app_mut().grid_edit.as_mut().unwrap().draft;
            draft.spacing = 100;
            draft.subdivisions = 4;
            draft.opacity = 80;
        }
        ui.settle();
        ui.click("Restore Defaults");
        assert_eq!(ui.app().grid_edit.unwrap().draft, GridSettings::default());
        // More subdivisions than pixels can't be applied.
        ui.app_mut().grid_edit.as_mut().unwrap().draft.subdivisions = 64;
        ui.app_mut().grid_edit.as_mut().unwrap().draft.spacing = 10;
        ui.settle();
        assert!(!ui.enabled("OK"));
        ui.app_mut().grid_edit.as_mut().unwrap().draft.subdivisions = 8;
        ui.app_mut().grid_edit.as_mut().unwrap().draft.spacing = 100;
        ui.settle();
        ui.click("OK");
        assert!(ui.app().dialog.is_none());
        let expected = GridSettings {
            spacing: 100,
            ..GridSettings::default()
        };
        // Kept in the project as an undo step, and as the default for other projects.
        assert_eq!(ui.app().session().unwrap().document.grid, Some(expected));
        assert_eq!(ui.app().config.grid, expected);
        let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
        assert_eq!(saved.grid, expected);
        // As upstream, the grid's visibility goes back to what it was.
        assert!(!ui.app().showing_grid());
        ui.press(Modifiers::CTRL, Key::Z);
        assert_eq!(ui.app().session().unwrap().document.grid, None);
    }
}

mod layer_appearance {
    use super::*;
    use egui::Key;

    /// Focuses the layer panel's opacity slider (the slider level with the "Opacity" label).
    fn focus_opacity(ui: &mut UiTest) {
        let label = ui.harness.get_by_label("Opacity").rect().center().y;
        let slider = ui
            .harness
            .get_all_by_role(Role::Slider)
            .find(|slider| (slider.rect().center().y - label).abs() < 12.0)
            .expect("an opacity slider");
        assert!(!slider.accesskit_node().is_disabled());
        slider.focus();
        ui.settle();
    }

    fn active(ui: &UiTest) -> &xuan::document::Layer {
        ui.app().session().unwrap().document.active().unwrap()
    }

    #[test]
    fn a_folder_takes_an_opacity_but_not_a_blend_mode() {
        let mut ui = UiTest::with_document();
        ui.app_mut().command("group");
        ui.settle();
        assert!(active(&ui).group);
        // Blending stays with each layer (upstream's canEditAppearance), so the menu is off.
        assert!(!ui.enabled("Normal"));
        focus_opacity(&mut ui);
        for _ in 0..10 {
            ui.key(Key::ArrowLeft);
        }
        let opacity = active(&ui).opacity;
        assert!(opacity < 1.0, "{opacity}");
        // The folder dims what is inside it.
        let document = &ui.app().session().unwrap().document;
        let child = document
            .layers
            .iter()
            .find(|l| l.parent == Some(document.active.unwrap()))
            .unwrap();
        let point = xuan::document::Point::new(5.0, 5.0);
        assert_eq!(
            xuan::render::inherited_coverage(document, child, point),
            opacity
        );
        // Each key press is its own undo step.
        ui.app_mut().command("undo");
        ui.settle();
        assert!(active(&ui).opacity > opacity);
    }

    #[test]
    fn the_blend_menu_lists_photoshops_modes_in_its_order() {
        let mut ui = UiTest::with_document();
        ui.click("Normal");
        for name in [
            "Dissolve",
            "Linear Burn",
            "Linear Dodge (Add)",
            "Hard Mix",
            "Divide",
        ] {
            assert!(ui.has(name), "{name}");
        }
        let y = |ui: &UiTest, name: &str| ui.harness.get_by_label(name).rect().top();
        assert!(y(&ui, "Dissolve") < y(&ui, "Darken"));
        assert!(y(&ui, "Linear Burn") < y(&ui, "Darker Color"));
        assert!(y(&ui, "Pin Light") < y(&ui, "Hard Mix"));
        assert!(y(&ui, "Divide") < y(&ui, "Hue"));
        // The menu opens where it fits the window.
        let screen = ui.ctx().content_rect();
        assert!(screen.contains_rect(ui.harness.get_by_label("Soft Light").rect()));
        ui.click("Soft Light");
        assert_eq!(active(&ui).blend, xuan::blend::BlendMode::SoftLight);
    }

    #[test]
    fn layer_effects_are_added_edited_and_kept_on_the_layer() {
        use xuan::layer_effects::{EffectKind, ShadowEffect};
        let mut ui = UiTest::with_document();
        ui.app_mut().command("fill_fg");
        ui.settle();
        let pixels = active(&ui).pixels.clone();
        ui.open_menu("Layer");
        ui.click("Layer Effects…");
        assert!(ui.app().dialog == Some(Dialog::LayerEffects));
        // Not the adjustment and filter dialog.
        assert!(ui.app().effect.is_none());
        ui.click("Show Drop Shadow");
        // Shown live on the canvas while the dialog is open.
        let effects = active(&ui).effects.clone().unwrap();
        assert_eq!(effects.drop_shadow, Some(ShadowEffect::DROP));
        let shows =
            |ui: &UiTest, label: &str| ui.harness.query_all_by_label(label).next().is_some();
        assert!(shows(&ui, "Distance") && shows(&ui, "Angle") && shows(&ui, "Blur"));
        ui.click("Show Stroke");
        assert!(shows(&ui, "Outside") && shows(&ui, "Inside"));
        ui.click("Inside");
        ui.click("OK");
        assert!(ui.app().dialog.is_none());
        let effects = active(&ui).effects.clone().unwrap();
        assert!(effects.is_enabled(EffectKind::DropShadow));
        assert!(effects.stroke.unwrap().inside);
        // The pixels are untouched; the effects are one undo step.
        assert!(std::sync::Arc::ptr_eq(
            active(&ui).pixels.as_ref().unwrap(),
            pixels.as_ref().unwrap()
        ));
        // Editable again later: hiding one keeps its settings, Cancel restores it.
        ui.open_menu("Layer");
        ui.click("Layer Effects…");
        ui.click("Show Drop Shadow");
        assert!(
            !active(&ui)
                .effects
                .as_ref()
                .unwrap()
                .is_enabled(EffectKind::DropShadow)
        );
        ui.click("Cancel");
        assert!(
            active(&ui)
                .effects
                .as_ref()
                .unwrap()
                .is_enabled(EffectKind::DropShadow)
        );
        ui.app_mut().command("undo");
        ui.settle();
        assert_eq!(active(&ui).effects, None);
    }

    #[test]
    fn generated_layers_show_their_model_provenance_read_only_with_a_copy_button() {
        let mut ui = UiTest::with_document();
        ui.app_mut().command("fill_fg");
        ui.settle();
        // Ordinary layers have no such section.
        assert!(!ui.has("Generation"));
        let layer = ui
            .app_mut()
            .session_mut()
            .unwrap()
            .document
            .active_mut()
            .unwrap();
        layer.generated = Some(xuan::document::Generated {
            plugin: "mock".into(),
            version: "1".into(),
            action: "echo".into(),
            inputs: serde_json::json!({}),
            source: None,
            source_hash: None,
            created: String::new(),
        });
        ui.settle();
        ui.click("Generation");
        assert!(ui.has("Plugin: mock (echo)"));
        assert!(ui.has("The plugin reported no model details."));
        assert!(!ui.has_role(Role::Button, "Copy"));

        let record = xuan::provenance::Provenance {
            model: Some("sdxl.safetensors".into()),
            seed: Some(42),
            steps: Some(30),
            extra: [("lora".to_string(), serde_json::json!("detail"))].into(),
            ..Default::default()
        };
        let layer = ui
            .app_mut()
            .session_mut()
            .unwrap()
            .document
            .active_mut()
            .unwrap();
        layer.provenance = Some(record.clone());
        ui.settle();
        assert!(ui.has("model: sdxl.safetensors"));
        assert!(ui.has("steps: 30") && ui.has("seed: 42") && ui.has("extra.lora: detail"));
        assert!(!ui.has("The plugin reported no model details."));
        ui.harness
            .get_by_role_and_label(Role::Button, "Copy")
            .click();
        ui.harness.step();
        let copied = ui.copied_text().expect("the JSON was copied");
        assert_eq!(
            serde_json::from_str::<xuan::provenance::Provenance>(&copied).unwrap(),
            record
        );
        // Showing it changes nothing in the document.
        assert_eq!(active(&ui).provenance, Some(record));
    }

    #[test]
    fn black_white_and_color_balance_layers_come_from_the_layer_menu() {
        use xuan::document::Adjustment;
        let mut ui = UiTest::with_document();
        ui.open_menu("Layer");
        ui.click("New Adjustment Layer ⏵");
        ui.click("Black & White");
        assert!(ui.has("Reds") && ui.has("Magentas") && ui.has("Tint"));
        ui.click("Apply");
        assert_eq!(active(&ui).adjustment, Some(Adjustment::BLACK_WHITE));

        ui.open_menu("Layer");
        ui.click("New Adjustment Layer ⏵");
        ui.click("Color Balance");
        assert!(ui.has("Cyan – Red") && ui.has("Preserve Luminosity"));
        // Each tonal range has its own sliders.
        ui.click("Shadows");
        ui.click("Preserve Luminosity");
        ui.click("Apply");
        assert_eq!(
            active(&ui).adjustment,
            Some(Adjustment::ColorBalance {
                shadows: [0.0; 3],
                midtones: [0.0; 3],
                highlights: [0.0; 3],
                preserve_luminosity: false,
            })
        );
    }
}

mod window_menu {
    use super::*;
    use xuan::panes::{LAYERS, NAVIGATOR};

    fn ids(ui: &UiTest) -> Vec<String> {
        ui.app()
            .config
            .panes
            .0
            .iter()
            .map(|pane| pane.id.clone())
            .collect()
    }

    #[test]
    fn the_navigator_is_a_pane_above_layers_by_default() {
        let directory = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(directory.path());
        assert_eq!(ids(&ui), [NAVIGATOR, LAYERS]);
        // Its header is the sidebar's, and its body draws the thumbnail.
        assert!(ui.has("Navigator"));
        assert_eq!(ui.app().session().unwrap().navigator.renders, 1);
        assert!(ui.app().navigator_view.is_some());

        // Window lists both panes; unticking Navigator hides it and is remembered.
        ui.open_menu("Window");
        assert!(
            !ui.harness
                .get_by_role_and_label(Role::CheckBox, "Layers")
                .accesskit_node()
                .is_disabled()
        );
        ui.click_role(Role::CheckBox, "Navigator");
        assert!(ui.app().config.panes.get(NAVIGATOR).unwrap().hidden);
        assert!(!ui.has("Navigator"));
        let saved = xuan::config::Config::load(&directory.path().join("config.toml")).unwrap();
        assert!(saved.panes.get(NAVIGATOR).unwrap().hidden);

        // Reset Panel Layout brings it back above Layers.
        ui.app_mut().config.panes.move_pane(0, 2);
        assert_eq!(ids(&ui), [LAYERS, NAVIGATOR]);
        ui.open_menu("Window");
        ui.click("Reset Panel Layout");
        assert_eq!(ids(&ui), [NAVIGATOR, LAYERS]);
        assert!(!ui.app().config.panes.get(NAVIGATOR).unwrap().hidden);
        assert!(ui.has("Navigator"));
    }

    #[test]
    fn a_collapsed_navigator_stops_drawing_and_asking_for_repaints() {
        let mut ui = UiTest::with_document();
        ui.app_mut().config.panes.toggle_collapsed(NAVIGATOR);
        ui.settle();
        ui.app_mut().navigator_view = None;
        let renders = ui.app().session().unwrap().navigator.renders;
        ui.press(egui::Modifiers::CTRL, egui::Key::Plus);
        assert!(ui.app().navigator_view.is_none());
        assert_eq!(ui.app().session().unwrap().navigator.renders, renders);
        assert!(!ui.harness.ctx.has_requested_repaint());
    }
}

mod about {
    use super::*;

    #[test]
    fn about_shows_the_build_and_offers_to_copy_it() {
        let mut ui = UiTest::new();
        ui.app_mut().dialog = Some(Dialog::About);
        ui.settle();
        let build = xuan::buildinfo::current();
        // Dev builds lead with "Build" (or "Development build"); releases with "Version".
        let shown = build.display();
        assert!(ui.has(&shown), "About should show {shown:?}");
        if build.release {
            assert!(shown.starts_with("Version"));
        } else {
            assert!(shown.starts_with("Build") || shown == "Development build");
        }
        assert!(ui.has("Copy version"));
        ui.click("Copy version");
    }
}

mod color_range {
    use super::*;

    fn halves() -> UiTest {
        let mut ui = UiTest::with_document();
        let pixels = RgbaImage::from_fn(20, 16, |x, _| {
            image::Rgba(if x < 10 {
                [220, 20, 20, 255]
            } else {
                [20, 20, 220, 255]
            })
        });
        ui.app_mut()
            .session_mut()
            .unwrap()
            .document
            .insert(Layer::image("Halves", pixels));
        ui.settle();
        ui
    }

    fn at(ui: &UiTest, x: f32, y: f32) -> egui::Pos2 {
        let session = ui.app().session().unwrap();
        ui.app().canvas_rect.unwrap().min + Vec2::new(x, y) * session.zoom
    }

    fn selected(ui: &UiTest) -> Option<Vec<u8>> {
        let session = ui.app().session().unwrap();
        session
            .document
            .selection
            .as_ref()
            .map(|s| s.as_raw().clone())
    }

    #[test]
    fn picking_a_colour_previews_the_selection_and_ok_keeps_it() {
        let mut ui = halves();
        let revision = ui.app().session().unwrap().history.revision;
        ui.open_menu("Select");
        ui.click("Color Range…");
        assert!(ui.app().color_range.is_some());
        assert!(ui.has("Fuzziness"));
        // The menus' commands wait while the dialog is open.
        assert!(!(commands::find("select_all").unwrap().enabled)(ui.app()));
        let pos = at(&ui, 3.5, 4.5);
        ui.click_at(pos);
        let left: Vec<u8> = (0..20 * 16)
            .map(|i| if i % 20 < 10 { 255 } else { 0 })
            .collect();
        assert_eq!(selected(&ui), Some(left.clone()));
        ui.click_role(Role::CheckBox, "Invert");
        let right: Vec<u8> = left.iter().map(|v| 255 - v).collect();
        assert_eq!(selected(&ui), Some(right.clone()));
        ui.click("OK");
        assert!(ui.app().color_range.is_none());
        assert_eq!(selected(&ui), Some(right));
        assert_eq!(ui.app().session().unwrap().history.revision, revision + 1);
        // The tool from before is back.
        assert_ne!(ui.app().tool, Tool::Dropper);
    }

    #[test]
    fn cancel_puts_back_the_old_selection() {
        let mut ui = halves();
        ui.open_menu("Select");
        ui.click("Color Range…");
        let pos = at(&ui, 15.5, 4.5);
        ui.click_at(pos);
        assert!(selected(&ui).is_some());
        ui.click("Cancel");
        assert!(ui.app().color_range.is_none());
        assert_eq!(selected(&ui), None);
        assert_ne!(
            ui.app().session().unwrap().history.undo_name(),
            Some("Color Range")
        );
    }
}

mod selection_providers {
    use super::*;
    use xuan::plugins::manifest::Capability;

    #[test]
    fn settings_choose_a_provider_and_remember_it() {
        let config = tempfile::tempdir().unwrap();
        let plugin = tempfile::tempdir().unwrap();
        std::fs::write(
            plugin.path().join("plugin.toml"),
            "[plugin]\nid = \"seg\"\nname = \"Seg\"\nversion = \"1\"\ncommand = [\"sh\", \"x\"]\n\n\
             [[provides]]\ncapability = \"select_subject\"\naction = \"segment\"\n\n\
             [[actions]]\nid = \"segment\"\nlabel = \"Segment\"\nsource = { from = \"composite\" }\n",
        )
        .unwrap();
        let mut ui = UiTest::with_document();
        ui.isolate_config(config.path());
        let manifest = xuan::plugins::Manifest::load(plugin.path()).unwrap();
        ui.app_mut().install_plugins(vec![manifest], vec![]);
        ui.app_mut().command("settings");
        ui.settle();
        ui.click("Selection");
        assert!(ui.has("Select Subject provider"));
        // The first of the three pop-ups is Select Subject's.
        ui.harness
            .query_all_by_label("Built-in")
            .next()
            .unwrap()
            .click();
        ui.settle();
        ui.click("Seg (seg)");
        assert_eq!(
            ui.app().config.providers.get(Capability::SelectSubject),
            Some("seg")
        );
        assert_eq!(
            ui.app().config.providers.get(Capability::ObjectSelect),
            None
        );
        ui.click("Done");
        let saved = xuan::config::Config::load(&config.path().join("config.toml")).unwrap();
        assert_eq!(saved.providers.get(Capability::SelectSubject), Some("seg"));
    }
}

/// Background jobs and status messages share the right of the status bar.
mod status_bar {
    use super::*;

    fn job(
        app: &EditorApp,
        label: &str,
        progress: Option<f32>,
        message: &str,
    ) -> crate::app::plugins::PluginJob {
        crate::app::plugins::PluginJob {
            id: uuid::Uuid::new_v4(),
            plugin: "mock".into(),
            action: "echo".into(),
            label: label.into(),
            document: app.session().unwrap().document.id,
            _work_dir: xuan::plugins::private_dir("xuan-job-").unwrap(),
            prepared: xuan::plugins::jobs::Prepared::none(),
            regions: Vec::new(),
            inputs: serde_json::json!({}),
            into: xuan::plugins::manifest::ResultInto::Layer,
            mask_to_regions: false,
            progress,
            message: message.into(),
            cancelled: false,
            consented: false,
            provider: None,
        }
    }

    #[test]
    fn running_jobs_show_in_the_status_bar_with_a_count() {
        let mut ui = UiTest::with_document();
        let first = job(ui.app(), "Generate Image…", None, "Running on Comfy Cloud");
        ui.app_mut().plugins.jobs.push(first);
        ui.settle();
        // No floating window: the job is a line in the status bar.
        assert!(ui.has("Generate Image · Running on Comfy Cloud"));
        assert!(!ui.has("1 of 1"));
        let second = job(ui.app(), "Edit Image…", Some(0.5), "");
        ui.app_mut().plugins.jobs.push(second);
        ui.settle();
        assert!(ui.has("1 of 2"));
        // The count lists every job, each with its own Cancel.
        ui.click("1 of 2");
        assert!(ui.has_role(Role::Label, "Generate Image"));
        assert!(ui.has_role(Role::Label, "Edit Image"));
        assert_eq!(ui.harness.query_all_by_label("Cancel").count(), 3);
    }

    #[test]
    fn status_messages_show_for_a_few_seconds_then_the_tool_hint_returns() {
        let mut ui = UiTest::with_document();
        let hint = ui.app().tool.hint().to_owned();
        assert!(ui.has(&hint));
        ui.app_mut().status = "Mock (plugin mock): Saved\nsecond line".into();
        ui.settle();
        assert!(ui.has("Mock (plugin mock): Saved"));
        assert!(!ui.has(&hint));
        // Once the message is old, the hint is back.
        ui.app_mut().status_shown.1 = -100.0;
        ui.settle();
        assert!(!ui.has("Mock (plugin mock): Saved"));
        assert!(ui.has(&hint));
    }
}

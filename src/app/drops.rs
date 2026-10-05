use std::path::{Path, PathBuf};

use xuan::i18n::tr;

use super::{Dialog, EditorApp, widgets};

/// A drop waiting on the "insert as layer or open as new document" choice.
pub(super) struct DropPrompt {
    images: Vec<PathBuf>,
    projects: Vec<PathBuf>,
}

fn is_project(path: &Path) -> bool {
    path.is_dir() || path.extension().is_some_and(|e| e == "xuan")
}

impl EditorApp {
    /// True while something else owns the window and keyboard shortcuts and drops must wait.
    pub(super) fn drops_blocked(&self) -> bool {
        self.dialog.is_some()
            || self.develop_close_requested.is_some()
            || self.job.is_some()
            || self.error.is_some()
            || self.close_tab.is_some()
            || self.close_app
    }

    /// Queues one drop (all files dropped together) for handling.
    pub(super) fn queue_drop(&mut self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.pending_drops.push_back(paths);
        }
    }

    /// Handles queued drops until one needs a dialog or the window becomes busy.
    pub(super) fn process_drops(&mut self) {
        // Develop cannot take layers or other documents, so drops wait for it to close.
        while !self.drops_blocked() && self.develop.is_none() {
            let Some(paths) = self.pending_drops.pop_front() else {
                return;
            };
            self.handle_drop(paths);
        }
    }

    fn handle_drop(&mut self, paths: Vec<PathBuf>) {
        let (projects, images): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| is_project(p));
        if images.is_empty() || self.sessions.is_empty() {
            for path in projects.into_iter().chain(images) {
                self.open_path(&path, false);
            }
            return;
        }
        // Projects open after the choice so layers land in the document that was open.
        self.drop_prompt = Some(DropPrompt { images, projects });
        self.dialog = Some(Dialog::DropChoice);
    }

    pub(super) fn drop_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &self.drop_prompt else {
            self.dialog = None;
            return;
        };
        let count = prompt.images.len();
        let mut choice = None;
        widgets::Window::new(tr("Add dropped files"))
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label(if count == 1 {
                    tr("Insert the dropped file as a layer, or open it as a new document?")
                } else {
                    tr("Insert the dropped files as layers, or open them as new documents?")
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, tr("Cancel")).clicked() {
                        choice = Some(None);
                    }
                    if widgets::button(ui, tr("Open as new document")).clicked() {
                        choice = Some(Some(false));
                    }
                    if widgets::primary_button(ui, tr("Insert as layer")).clicked() {
                        choice = Some(Some(true));
                    }
                });
            });
        if choice.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                choice = Some(None);
            } else if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                choice = Some(Some(true));
            }
        }
        let Some(choice) = choice else { return };
        let prompt = self.drop_prompt.take().unwrap();
        self.finish_drop(prompt, choice);
    }

    fn finish_drop(&mut self, prompt: DropPrompt, choice: Option<bool>) {
        self.dialog = None;
        if let Some(as_layer) = choice {
            for path in &prompt.images {
                self.open_path(path, as_layer);
            }
        }
        for path in &prompt.projects {
            self.open_path(path, false);
        }
    }
}

#[cfg(test)]
impl EditorApp {
    /// Resolves the open prompt like a button press: `None` cancels.
    pub(super) fn drop_choose_for_test(&mut self, choice: Option<bool>) {
        let prompt = self.drop_prompt.take().unwrap();
        self.finish_drop(prompt, choice);
    }
}

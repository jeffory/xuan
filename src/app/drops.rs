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

/// A `.cube` file, which adds a Color Lookup adjustment layer rather than opening.
fn is_lookup_table(path: &Path) -> bool {
    !path.is_dir()
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cube"))
}

impl EditorApp {
    /// True while something else owns the window and keyboard shortcuts and drops must wait.
    pub(super) fn drops_blocked(&self) -> bool {
        self.dialog.is_some()
            || self.color_range.is_some()
            || !self.photoshop_imports.is_empty()
            || self.develop_close_requested.is_some()
            || self.job.is_some()
            || self.error.is_some()
            || self.close_tab.is_some()
            || self.close_app
    }

    /// Queues one drop (all files dropped together) for handling.
    pub(super) fn queue_drop(&mut self, paths: Vec<PathBuf>) {
        // A folder or zip dropped on Manage Plugins or the install window is
        // a plugin to install, not a document to open.
        if matches!(self.dialog, Some(Dialog::Plugins | Dialog::PluginInstall))
            && let [path] = paths.as_slice()
            && super::plugin_install::installable(path)
        {
            self.stage_plugin_install(&path.clone());
            return;
        }
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

    fn handle_drop(&mut self, mut paths: Vec<PathBuf>) {
        // A lookup table opens the adjustment dialog; the rest of the drop waits for it.
        if let Some(index) = paths.iter().position(|p| is_lookup_table(p)) {
            let table = paths.remove(index);
            if !paths.is_empty() {
                self.pending_drops.push_front(paths);
            }
            if self.sessions.is_empty() {
                self.error = Some(tr("Open an image before adding a colour lookup table").into());
            } else {
                self.load_color_lookup(&table, true);
            }
            return;
        }
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
            .default_width(420.0)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.label(if count == 1 {
                        tr("Insert the dropped file as a layer, or open it as a new document?")
                    } else {
                        tr("Insert the dropped files as layers, or open them as new documents?")
                    });
                },
                |ui, ()| {
                    let mut open = false;
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(tr("Insert as layer")),
                        |ui| {
                            open = widgets::button(ui, tr("Open as new document")).clicked();
                        },
                    );
                    if response.cancel {
                        choice = Some(None);
                    } else if open {
                        choice = Some(Some(false));
                    } else if response.commit {
                        choice = Some(Some(true));
                    }
                },
            );
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
        match choice {
            Some(true) => self.insert_files(&prompt.images),
            Some(false) => {
                for path in &prompt.images {
                    self.open_path(path, false);
                }
            }
            None => {}
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

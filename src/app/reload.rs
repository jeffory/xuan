//! Reloading an open project when another program changes its file (issue #117).
//!
//! Every tab whose project is a `.xuan` file is followed by a [`FileWatcher`], started when the
//! first one opens. When a script, a sync tool, a Git checkout or an agent writes the file, the
//! watcher loads the new content on its own thread and wakes the editor (see `xuan::watch`
//! for what counts as a change). Then, by [`respond`]:
//!
//! - a document without unsaved changes is replaced in place by [`replace_document`]: the tab,
//!   zoom, pan and the selected layers that still exist are kept, and undo starts afresh;
//! - a document with unsaved changes keeps them, and a bar above the canvas offers **Reload**
//!   or **Keep mine**;
//! - a document in the middle of an edit (a drag, an open dialog, a running job) waits for it.
//!
//! Images opened from PNG, JPEG, Photoshop and the other formats become new projects with no
//! file of their own until they are saved as `.xuan`, so only `.xuan` files are followed.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use egui::RichText;
use uuid::Uuid;
use xuan::{
    document::Document,
    history::History,
    i18n::tr,
    io,
    watch::{ContentHash, FileWatcher, Outcome, Report},
};

use super::{EditorApp, Session, theme::PaletteExt as _, widgets};

/// What to do with a document whose file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Response {
    /// Replace it with the file's content.
    Reload,
    /// Keep it and ask, with the bar.
    Ask,
    /// Keep it until the edit in progress is finished, then decide again.
    Wait,
}

/// What to do with a changed document that has unsaved changes (`dirty`) or an edit in
/// progress (`busy`).
pub(super) fn respond(dirty: bool, busy: bool) -> Response {
    if dirty {
        Response::Ask
    } else if busy {
        Response::Wait
    } else {
        Response::Reload
    }
}

/// Whether the editor follows the file at `path` for changes.
pub(super) fn followed(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xuan"))
}

/// Puts `document`, just loaded from the session's file, in place of the session's. The view
/// stays as it was, and so do the selected layers and the selection outline when the file
/// still has them; undo history starts again.
pub(super) fn replace_document(session: &mut Session, mut document: Document) {
    let old = &session.document;
    document.id = old.id;
    document.promote_image_masks();
    let exists = |id: &Uuid| document.layers.iter().any(|layer| layer.id == *id);
    let selected: std::collections::HashSet<Uuid> =
        old.selected.iter().copied().filter(exists).collect();
    if !selected.is_empty() {
        let active = old.active.filter(|id| selected.contains(id)).or_else(|| {
            document
                .layers
                .iter()
                .rev()
                .map(|layer| layer.id)
                .find(|id| selected.contains(id))
        });
        document.selected = selected;
        document.active = active;
    }
    // The selection outline is not saved in the file, so the file cannot have meant to drop it.
    if (document.width, document.height) == (old.width, old.height) {
        document.selection = old.selection.clone();
    }
    session
        .collapsed
        .retain(|id| document.layers.iter().any(|layer| layer.id == *id));
    session.document = document;
    session.history = History::default();
    session.external = None;
    session.thumbnails.clear();
    session.navigator = Default::default();
    session.composite = None;
    session.invalidate();
}

/// The file watcher and the files it follows, by document.
pub(super) struct FileWatch {
    watcher: Option<FileWatcher>,
    /// The watcher could not start (the system's limit on watches, say), so files are not
    /// followed in this run.
    unavailable: bool,
    /// Woken from the watcher's thread when a file changed.
    repaint: egui::Context,
    /// The file each document is followed at.
    watched: HashMap<Uuid, PathBuf>,
    /// The documents of the reports received so far, in order; tests wait on them.
    #[cfg(test)]
    pub reported: Vec<Uuid>,
}

impl FileWatch {
    pub fn new(repaint: egui::Context) -> Self {
        Self {
            watcher: None,
            unavailable: false,
            repaint,
            watched: HashMap::new(),
            #[cfg(test)]
            reported: Vec::new(),
        }
    }

    fn watcher(&mut self) -> Option<&FileWatcher> {
        if self.watcher.is_none() && !self.unavailable {
            let repaint = self.repaint.clone();
            match FileWatcher::spawn(move || repaint.request_repaint()) {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(error) => {
                    eprintln!("Not following open projects for changes: {error:#}");
                    self.unavailable = true;
                }
            }
        }
        self.watcher.as_ref()
    }

    /// Follows the files of `documents` (document id and project path), and stops following
    /// those of documents that closed or moved to another file.
    pub fn sync<'a>(&mut self, documents: impl Iterator<Item = (Uuid, &'a Path)> + Clone) {
        let gone: Vec<Uuid> = self
            .watched
            .iter()
            .filter(|(id, path)| {
                !documents
                    .clone()
                    .any(|(other, file)| other == **id && file == path.as_path())
            })
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.watched.remove(&id);
            if let Some(watcher) = &self.watcher {
                watcher.unwatch(id);
            }
        }
        for (id, path) in documents {
            if followed(path) && !self.watched.contains_key(&id) {
                if let Some(watcher) = self.watcher() {
                    watcher.watch(id, path, None);
                }
                self.watched.insert(id, path.to_path_buf());
            }
        }
    }

    /// Notes that Xuan is about to write `path`, hashing to `hash`, for document `id`, so the
    /// change is known as its own.
    fn expect(&mut self, id: Uuid, path: &Path, hash: ContentHash) {
        if !followed(path) {
            return;
        }
        if let Some(watcher) = self.watcher() {
            watcher.watch(id, path, Some(hash));
        }
        self.watched.insert(id, path.to_path_buf());
    }

    /// Forgets what is known of `id`'s file, so it is followed afresh.
    fn forget(&mut self, id: Uuid) {
        if self.watched.remove(&id).is_some()
            && let Some(watcher) = &self.watcher
        {
            watcher.unwatch(id);
        }
    }

    /// Reports of files still followed where they were read.
    fn poll(&mut self) -> Vec<Report> {
        let mut reports = Vec::new();
        while let Some(report) = self.watcher.as_ref().and_then(FileWatcher::try_recv) {
            #[cfg(test)]
            self.reported.push(report.id);
            let current = self.watched.get(&report.id).is_some_and(|path| {
                std::path::absolute(path).unwrap_or_else(|_| path.clone()) == report.path
            });
            if current {
                reports.push(report);
            }
        }
        reports
    }

    /// The documents whose files are followed.
    #[cfg(test)]
    pub fn watched(&self) -> Vec<Uuid> {
        self.watched.keys().copied().collect()
    }

    /// Waits until the watcher follows every file asked for so far.
    #[cfg(test)]
    pub fn flush(&self) {
        if let Some(watcher) = &self.watcher {
            watcher.flush();
        }
    }
}

impl Session {
    /// Saves the document as the project at `path`, which it is from then on.
    pub(super) fn save_project(
        &mut self,
        path: &Path,
        watch: &mut FileWatch,
    ) -> anyhow::Result<()> {
        let id = self.document.id;
        let saved = io::save_hashed(&self.document, path, |hash| watch.expect(id, path, hash));
        if let Err(error) = saved {
            // The file may be as it was, or not; the watcher reads it again.
            watch.forget(id);
            return Err(error);
        }
        self.title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into();
        self.path = Some(path.to_path_buf());
        self.history.mark_saved();
        // What was saved replaces the file's other content.
        self.external = None;
        Ok(())
    }
}

impl EditorApp {
    /// Follows the open projects' files and takes in their changes; once a frame.
    pub(super) fn follow_files(&mut self) {
        let documents = self
            .sessions
            .iter()
            .filter_map(|s| Some((s.document.id, s.path.as_deref()?)));
        self.file_watch.sync(documents);
        for report in self.file_watch.poll() {
            if let Outcome::Changed(document) = report.outcome
                && let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|s| s.document.id == report.id)
            {
                session.external = Some(document);
            }
        }
        for index in 0..self.sessions.len() {
            if self.sessions[index].external.is_some()
                && respond(
                    self.sessions[index].history.dirty(),
                    self.reload_blocked(index),
                ) == Response::Reload
            {
                self.reload(index);
            }
        }
    }

    /// Whether an edit in progress uses document `index`, so it must not be replaced now.
    fn reload_blocked(&self, index: usize) -> bool {
        let id = self.sessions[index].document.id;
        self.job.is_some()
            || self.develop.is_some()
            || self.plugins.proposal.is_some()
            || self.plugins.jobs.iter().any(|job| job.document == id)
            || (index == self.current
                && (self.gesture.is_some()
                    || self.dialog.is_some()
                    || self.effect.is_some()
                    || self.layer_effects.is_some()
                    || self.stroke.is_some()
                    || self.color_range.is_some()
                    || self.text_edit.is_some()
                    || self.rename.is_some()))
    }

    /// Replaces document `index` with the file's content waiting for it.
    fn reload(&mut self, index: usize) {
        let session = &mut self.sessions[index];
        if let Some(document) = session.external.take() {
            replace_document(session, *document);
            self.status = format!("{} {}", tr("Reloaded"), session.title);
        }
    }

    /// The bar above the canvas when the current document's file changed and the document
    /// has unsaved changes.
    pub(super) fn reload_bar(&mut self, ctx: &egui::Context) {
        let Some(session) = self.session() else {
            return;
        };
        if session.external.is_none() || !session.history.dirty() {
            return;
        }
        let enabled = !self.reload_blocked(self.current);
        let p = ctx.palette();
        let mut choice = None;
        egui::TopBottomPanel::top("reload_bar")
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .stroke(egui::Stroke::new(1.0_f32, p.divider)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("●").color(p.warning));
                    ui.label(tr("This file was changed by another program."));
                    ui.add_enabled_ui(enabled, |ui| {
                        if widgets::primary_button(ui, tr("Reload")).clicked() {
                            choice = Some(true);
                        }
                        if widgets::button(ui, tr("Keep mine")).clicked() {
                            choice = Some(false);
                        }
                    });
                });
            });
        match choice {
            Some(true) => self.reload(self.current),
            Some(false) => {
                if let Some(session) = self.session_mut() {
                    session.external = None;
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsaved_changes_are_asked_about_and_edits_waited_for() {
        assert_eq!(respond(false, false), Response::Reload);
        assert_eq!(respond(true, false), Response::Ask);
        assert_eq!(respond(true, true), Response::Ask);
        assert_eq!(respond(false, true), Response::Wait);
    }

    /// A 20x16 project with three blank layers, A, B and C.
    fn project() -> Document {
        let mut document = Document::new(20, 16).unwrap();
        document.layers = ["A", "B", "C"]
            .map(|name| xuan::document::Layer::blank(name, 20, 16))
            .into();
        document
    }

    fn session(document: Document) -> Session {
        Session::new(document, "Project".into(), Some("/w/project.xuan".into()))
    }

    #[test]
    fn reloading_keeps_the_view_and_the_layers_still_there() {
        let original = project();
        let [a, b, c] = [0, 1, 2].map(|i| original.layers[i].id);
        let mut session = session(original.clone());
        let id = session.document.id;
        session.zoom = 3.0;
        session.pan = egui::vec2(12.0, -4.0);
        session.fit = false;
        session.document.select(a, false);
        session.document.select(c, true);
        session.document.selection = Some(std::sync::Arc::new(image::GrayImage::new(20, 16)));
        session.collapsed.extend([b, c]);
        session.history.begin("Edit", &session.document);
        session.history.commit();
        session.history.mark_saved();
        session.external = Some(Box::new(original.clone()));

        let mut changed = original;
        changed.layers.retain(|layer| layer.id != c);
        changed.layers[1].name = "Renamed".into();
        changed.select(b, false);
        replace_document(&mut session, changed);

        let document = &session.document;
        assert_eq!(document.id, id, "the same tab");
        assert_eq!(document.layers[1].name, "Renamed");
        assert_eq!(document.selected, [a].into(), "C is gone, A is kept");
        assert_eq!(document.active, Some(a));
        assert!(document.selection.is_some());
        assert_eq!(
            (session.zoom, session.pan, session.fit),
            (3.0, egui::vec2(12.0, -4.0), false)
        );
        assert_eq!(session.collapsed, [b].into());
        assert_eq!(session.history.undo_name(), None, "undo starts afresh");
        assert!(!session.history.dirty());
        assert!(session.external.is_none());
    }

    #[test]
    fn without_the_selected_layers_the_files_selection_is_used() {
        let original = project();
        let mut session = session(original.clone());
        session.document.select(original.layers[2].id, false);
        session.document.selection = Some(std::sync::Arc::new(image::GrayImage::new(20, 16)));

        let changed = Document::new(30, 16).unwrap();
        let layer = changed.layers[0].id;
        replace_document(&mut session, changed);
        assert_eq!(session.document.selected, [layer].into());
        assert_eq!(session.document.active, Some(layer));
        assert!(
            session.document.selection.is_none(),
            "an outline drawn for another canvas size is dropped"
        );
    }

    #[test]
    fn only_xuan_projects_are_followed() {
        assert!(followed(Path::new("/w/a.xuan")));
        assert!(followed(Path::new("A.XUAN")));
        assert!(!followed(Path::new("/w/a.png")));
        assert!(!followed(Path::new("/w/xuan")));
    }
}

//! Files a plugin asks Xuan to save, export or open. None of them happens
//! silently: `file/save_as` and `file/export` show the native save dialog,
//! prefilled with the plugin's suggested name, so the user picks (or
//! cancels) the path and the system dialog confirms any overwrite;
//! `file/open` names the file in a prompt the user must accept. A plugin can
//! never save over a document's file, export somewhere the user did not
//! choose, or open a file behind the user's back. See "Files the user
//! chooses" in `docs/PLUGINS.md`.
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

use egui::RichText;
use serde_json::{Value, json};
use uuid::Uuid;
use xuan::{
    i18n::tr,
    io,
    plugins::protocol::{self, Id, Request, RpcError},
};

use super::theme::PaletteExt as _;
use super::{Dialog, EditorApp, plugins::one_line, widgets};

/// Requests that wait for the user's choice of a file.
pub(super) const FILE_METHODS: [&str; 3] = ["file/save_as", "file/export", "file/open"];

/// Image formats `file/export` writes, by the extension Xuan gives them.
const EXPORT_FORMATS: [&str; 4] = ["png", "jpg", "tiff", "webp"];

/// What a plugin asked for.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FileAction {
    /// The native save dialog for a document: as a project (`export: None`)
    /// or as an image in the given format.
    Save {
        document: Uuid,
        export: Option<String>,
        name: String,
    },
    /// Open this file as a document once the user confirms it.
    Open { path: PathBuf },
}

/// A file request waiting for its turn, or for the user's answer.
#[derive(Clone, Debug)]
pub(super) struct FileRequest {
    pub plugin: String,
    pub id: Id,
    pub action: FileAction,
}

/// What the native save dialog is asked to show.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SaveDialog {
    pub title: String,
    pub file_name: String,
    pub filter_name: String,
    pub extensions: Vec<String>,
}

/// Shows the save dialog. The editor uses the system's; tests answer it.
pub(super) type SaveDialogHook = Arc<dyn Fn(&SaveDialog) -> Option<PathBuf>>;

/// The file name a plugin suggests, made safe for a save dialog: only the
/// last path component, without its extension, control characters or path
/// separators, and at most 100 characters.
fn suggested_name(suggested: Option<&str>, fallback: &str) -> String {
    let name = suggested
        .map(|text| text.rsplit(['/', '\\']).next().unwrap_or_default())
        .map(|text| {
            Path::new(text)
                .file_stem()
                .map_or(String::new(), |stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let clean: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(100)
        .collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    if clean.is_empty() {
        fallback.to_owned()
    } else {
        clean.to_owned()
    }
}

impl EditorApp {
    /// Check a file request's params before it waits for the user.
    fn file_action(&self, request: &Request) -> Result<FileAction, RpcError> {
        let params = &request.params;
        let string = |key: &str| params.get(key).and_then(Value::as_str);
        match request.method.as_str() {
            "file/open" => {
                let path =
                    string("path").ok_or_else(|| RpcError::invalid_params("`path` is required"))?;
                let path = Path::new(path);
                if !path.is_absolute() {
                    return Err(RpcError::invalid_params("`path` must be absolute"));
                }
                // The prompt names the file that will really be opened.
                let resolved = std::fs::canonicalize(path).map_err(|error| {
                    RpcError::invalid_params(format!("Cannot open {}: {error}", path.display()))
                })?;
                if !std::fs::metadata(&resolved).is_ok_and(|m| m.is_file() || m.is_dir()) {
                    return Err(RpcError::invalid_params(format!(
                        "{} is not a regular file or a project folder",
                        resolved.display()
                    )));
                }
                Ok(FileAction::Open { path: resolved })
            }
            method => {
                let index = match string("document") {
                    Some(id) => {
                        let id = Uuid::parse_str(id).map_err(|_| {
                            RpcError::invalid_params("`document` must be a document id")
                        })?;
                        self.sessions
                            .iter()
                            .position(|s| s.document.id == id)
                            .ok_or_else(|| RpcError::invalid_params("No such open document"))?
                    }
                    None if self.sessions.is_empty() => {
                        return Err(RpcError::invalid_params("No document is open"));
                    }
                    None => self.current,
                };
                let session = &self.sessions[index];
                let export = if method == "file/export" {
                    let format = string("format").unwrap_or("png").to_ascii_lowercase();
                    let format = match format.as_str() {
                        "jpeg" => "jpg".to_owned(),
                        "tif" => "tiff".to_owned(),
                        _ => format,
                    };
                    if !EXPORT_FORMATS.contains(&format.as_str()) {
                        return Err(RpcError::invalid_params(format!(
                            "`format` must be one of {}",
                            EXPORT_FORMATS.join(", ")
                        )));
                    }
                    Some(format)
                } else {
                    None
                };
                Ok(FileAction::Save {
                    document: session.document.id,
                    export,
                    name: suggested_name(string("suggested_name"), &session.title),
                })
            }
        }
    }

    /// Queue a file request from a plugin; it is answered once the user chose.
    pub(super) fn queue_file_request(&mut self, plugin: &str, request: Request) {
        // After the user cancelled, the plugin may not ask again for a while.
        let cooling = (self.plugins.file_refused_at.get(plugin))
            .is_some_and(|at| at.elapsed() < super::plugin_sessions::COOLDOWN);
        if cooling {
            let error = RpcError::new(
                protocol::CANCELLED,
                "The user cancelled this plugin's last file request just now; it may ask again later",
            );
            self.respond_to_plugin(plugin, request.id, Err(error));
            return;
        }
        let action = self.file_action(&request).and_then(|action| {
            // One at a time per plugin, so a plugin cannot stack up dialogs.
            let waiting = (self.plugins.file_requests.iter())
                .chain(&self.plugins.file_prompt)
                .any(|r| r.plugin == plugin);
            if waiting {
                Err(RpcError::new(
                    protocol::INVALID_REQUEST,
                    "Another file request of this plugin is waiting for the user",
                ))
            } else {
                Ok(action)
            }
        });
        match action {
            Ok(action) => self.plugins.file_requests.push_back(FileRequest {
                plugin: plugin.to_owned(),
                id: request.id,
                action,
            }),
            Err(error) => self.respond_to_plugin(plugin, request.id, Err(error)),
        }
    }

    fn respond_to_plugin(&mut self, plugin: &str, id: Id, result: Result<Value, RpcError>) {
        if let Some(process) = self.plugins.process_mut(plugin) {
            let _ = process.respond(id, result);
        }
    }

    /// Handle the next file request when nothing else is open.
    pub(super) fn release_file_requests(&mut self) {
        // Requests of plugins that stopped are dropped.
        let still: VecDeque<FileRequest> = std::mem::take(&mut self.plugins.file_requests)
            .into_iter()
            .filter(|request| self.plugins.running(&request.plugin))
            .collect();
        self.plugins.file_requests = still;
        if self.dialog.is_some() || self.plugins.file_prompt.is_some() || self.job.is_some() {
            return;
        }
        let Some(request) = self.plugins.file_requests.pop_front() else {
            return;
        };
        match &request.action {
            FileAction::Open { .. } => {
                self.plugins.file_prompt = Some(request);
                self.dialog = Some(Dialog::PluginFile);
            }
            FileAction::Save {
                document,
                export,
                name,
            } => {
                let result =
                    self.save_for_plugin(&request.plugin, *document, export.as_deref(), name);
                self.respond_to_plugin(&request.plugin, request.id, result);
            }
        }
    }

    /// The save dialog for a plugin's `file/save_as` or `file/export`, then
    /// the save itself.
    fn save_for_plugin(
        &mut self,
        plugin: &str,
        document: Uuid,
        export: Option<&str>,
        name: &str,
    ) -> Result<Value, RpcError> {
        let failed =
            |error: anyhow::Error| RpcError::new(protocol::INTERNAL_ERROR, format!("{error:#}"));
        let Some(index) = self.sessions.iter().position(|s| s.document.id == document) else {
            return Err(RpcError::invalid_params("The document was closed"));
        };
        let source = self.plugins.source(plugin);
        let extension = export.unwrap_or("xuan");
        let dialog = SaveDialog {
            title: match export {
                Some(_) => format!("{} · {source}", tr("Export Image")),
                None => format!("{} · {source}", tr("Save Project")),
            },
            file_name: format!("{name}.{extension}"),
            filter_name: if export.is_some() {
                extension.to_uppercase()
            } else {
                "xuan project".to_owned()
            },
            extensions: vec![extension.to_owned()],
        };
        let chosen = match &self.plugins.save_dialog {
            Some(hook) => hook(&dialog),
            None => rfd::FileDialog::new()
                .set_title(&dialog.title)
                .add_filter(&dialog.filter_name, &dialog.extensions)
                .set_file_name(&dialog.file_name)
                .save_file(),
        };
        let Some(mut path) = chosen else {
            (self.plugins.file_refused_at).insert(plugin.to_owned(), std::time::Instant::now());
            return Err(RpcError::new(
                protocol::CANCELLED,
                "The user cancelled the save dialog",
            ));
        };
        if path.extension().is_none() {
            path.set_extension(extension);
        }
        let session = &mut self.sessions[index];
        match export {
            Some(_) => {
                let chosen = (path.extension().and_then(|e| e.to_str()))
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let known = ["png", "jpg", "jpeg", "tif", "tiff", "webp"];
                if !known.contains(&chosen.as_str()) {
                    return Err(RpcError::invalid_params(format!(
                        "Cannot export as .{chosen}; choose PNG, JPEG, TIFF or WebP"
                    )));
                }
                io::export(&session.document, &path, self.jpeg_quality).map_err(failed)?;
                self.status = format!("{} {} · {source}", tr("Exported"), path.display());
            }
            None => {
                io::save(&session.document, &path).map_err(failed)?;
                // Saved as the user chose, the project now lives there.
                session.title = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into();
                session.path = Some(path.clone());
                session.history.mark_saved();
                self.status = format!("{} · {source}", tr("Project saved"));
            }
        }
        Ok(json!({"path": path}))
    }

    /// Apply the answer to the open `file/open` prompt.
    pub(super) fn answer_file_prompt(&mut self, open: bool) {
        if self.dialog == Some(Dialog::PluginFile) {
            self.dialog = None;
        }
        let Some(request) = self.plugins.file_prompt.take() else {
            return;
        };
        let FileAction::Open { path } = &request.action else {
            return;
        };
        if !open {
            (self.plugins.file_refused_at)
                .insert(request.plugin.clone(), std::time::Instant::now());
        }
        let result = if !open {
            Err(RpcError::new(
                protocol::CANCELLED,
                "The user did not open the file",
            ))
        } else if !std::fs::metadata(path).is_ok_and(|m| m.is_file() || m.is_dir()) {
            Err(RpcError::invalid_params(format!(
                "{} is no longer there",
                path.display()
            )))
        } else {
            let before = self.sessions.len();
            let error = self.error.take();
            self.open_path(path, false);
            match self.error.clone() {
                Some(message) if self.sessions.len() == before => {
                    Err(RpcError::new(protocol::INTERNAL_ERROR, message))
                }
                _ => {
                    if self.error.is_none() {
                        self.error = error;
                    }
                    Ok(json!({
                        "ok": true,
                        "document": self.session().map(|s| s.document.id),
                    }))
                }
            }
        };
        self.respond_to_plugin(&request.plugin, request.id, result);
    }

    pub(super) fn plugin_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.plugins.file_prompt.clone() else {
            self.dialog = None;
            return;
        };
        if !self.plugins.running(&request.plugin) {
            self.plugins.file_prompt = None;
            self.dialog = None;
            return;
        }
        let FileAction::Open { path } = &request.action else {
            self.answer_file_prompt(false);
            return;
        };
        let source = self.plugins.source(&request.plugin);
        let mut open = true;
        let mut answer = None;
        widgets::Window::new(tr("Open a file?"))
            .id(("plugin_file", &request.plugin))
            .default_width(440.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add(
                    egui::Label::new(format!("{source} {}", tr("asks Xuan to open this file:")))
                        .wrap(),
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(one_line(&path.display().to_string(), 1000)).monospace(),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(tr(
                            "It opens as a new document. Open it only if you expected this.",
                        ))
                        .small()
                        .color(ui.palette().muted),
                    )
                    .wrap(),
                );
                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::primary_button(ui, tr("Open")).clicked() {
                            answer = Some(true);
                        }
                        if widgets::button(ui, tr("Cancel")).clicked() {
                            answer = Some(false);
                        }
                    });
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
            answer = Some(false);
        }
        if let Some(open) = answer {
            self.answer_file_prompt(open);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::suggested_name;

    #[test]
    fn suggested_names_are_plain_file_names() {
        assert_eq!(suggested_name(Some("Holiday"), "Untitled"), "Holiday");
        assert_eq!(suggested_name(Some("../../etc/passwd"), "x"), "passwd");
        assert_eq!(suggested_name(Some("C:\\Users\\a\\b.png"), "x"), "b");
        assert_eq!(suggested_name(Some("a\u{7}b*?.xuan"), "x"), "ab");
        assert_eq!(suggested_name(Some(".hidden"), "x"), "hidden");
        assert_eq!(suggested_name(Some("..."), "x"), "x");
        assert_eq!(suggested_name(Some("   "), "Untitled"), "Untitled");
        assert_eq!(suggested_name(None, "Untitled"), "Untitled");
        assert_eq!(suggested_name(Some(&"y".repeat(300)), "x").len(), 100);
    }
}

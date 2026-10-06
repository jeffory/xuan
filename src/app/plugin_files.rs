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

/// The one answer to a `file/open` path that cannot be offered.
const CANNOT_OPEN: &str =
    "This path cannot be opened: it is missing, unreadable, or not an image file or project";

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
    Open {
        /// The path as the plugin gave it.
        requested: PathBuf,
        /// What it resolved to when the prompt was shown: what the user saw.
        path: PathBuf,
    },
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
    /// The folder to open in: the one the user chose, when the dialog asks
    /// again because the name lacked the extension.
    pub directory: Option<PathBuf>,
}

/// Shows the save dialog. The editor uses the system's; tests answer it.
pub(super) type SaveDialogHook = Arc<dyn Fn(&SaveDialog) -> Option<PathBuf>>;

/// The file name a plugin suggests, made safe for a save dialog: only the
/// last path component, without its extension, control, bidi or invisible
/// characters or path separators, at most 100 characters, and never a name
/// Windows reserves for a device. The fallback (the document's title) is
/// cleaned the same way.
fn suggested_name(suggested: Option<&str>, fallback: &str) -> String {
    let stem = |text: &str| {
        let last = text.rsplit(['/', '\\']).next().unwrap_or_default();
        Path::new(last)
            .file_stem()
            .map_or(String::new(), |stem| stem.to_string_lossy().into_owned())
    };
    suggested
        .and_then(|text| plain_file_name(&stem(text)))
        .or_else(|| plain_file_name(fallback))
        .unwrap_or_else(|| "Untitled".to_owned())
}

/// Characters that are invisible or reorder text: bidi embeddings,
/// overrides, isolates and marks (which can make `gpj.exe` read as
/// `exe.jpg`), zero-width characters and the byte order mark.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// Whether Windows treats a name as a device (`CON`, `NUL`, `COM1`, …),
/// whatever its extension and case, which would write to the device.
fn windows_device(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or_default().trim_end();
    let base = base.to_ascii_uppercase();
    if matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return true;
    }
    let mut chars = base.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: String = chars.collect();
    matches!(prefix.as_str(), "COM" | "LPT")
        && matches!(
            rest.as_str(),
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
}

/// A name cleaned for a save dialog, or `None` when nothing is left.
fn plain_file_name(name: &str) -> Option<String> {
    let clean: String = name
        .chars()
        .filter(|&c| {
            !c.is_control()
                && !invisible(c)
                && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(100)
        .collect();
    // Windows drops trailing dots and spaces, and a leading dot hides a file.
    let clean = clean
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' '])
        .trim();
    if clean.is_empty() {
        None
    } else if windows_device(clean) {
        Some(format!("_{clean}"))
    } else {
        Some(clean.to_owned())
    }
}

/// Image extensions `file/export` accepts in the name the user chose.
const EXPORT_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "tif", "tiff", "webp"];

/// Whether a path the user chose names the kind of file being written: a
/// `.xuan` project, or an image with an extension Xuan exports.
fn saves_as(path: &Path, export: Option<&str>) -> bool {
    let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    match export {
        Some(_) => EXPORT_EXTENSIONS.contains(&extension.as_str()),
        None => extension == "xuan",
    }
}

/// A message for a plugin with the folders of `path` taken out: the plugin
/// learns the name of the file the user chose, never where it is. Errors
/// from writing the file (a temporary file next to it, say) name the folder.
fn without_folders(message: &str, path: &Path) -> String {
    let mut message = message.to_owned();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // As `Display` shows paths, and as `Debug` quotes them (`\\` doubled).
    let forms = |p: &Path| {
        let debug = format!("{p:?}");
        let debug = debug.trim_matches('"').to_owned();
        [p.display().to_string(), debug]
    };
    for form in forms(path) {
        message = message.replace(&form, &name);
    }
    // Not the root: taking out `/` would mangle the whole message.
    let parent = (path.parent()).filter(|p| p.parent().is_some() && !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        for form in forms(parent) {
            // Longest first, so an escaped separator is not left half there.
            for separator in ["\\\\", "\\", "/"] {
                message = message.replace(&format!("{form}{separator}"), "");
            }
            message = message.replace(&form, "the chosen folder");
        }
    }
    message
}

/// Whether the requested path still resolves to the file the prompt showed,
/// and that is still a regular file or a project folder.
fn still_the_shown_file(requested: &Path, shown: &Path) -> bool {
    std::fs::canonicalize(requested).is_ok_and(|now| now == shown)
        && std::fs::canonicalize(shown).is_ok_and(|now| now == shown)
        && std::fs::metadata(shown).is_ok_and(|m| m.is_file() || m.is_dir())
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
                // The prompt names the file that will really be opened. One
                // answer for a missing, unreadable or unsuitable path, so a
                // plugin cannot probe what exists.
                let refused = || RpcError::invalid_params(CANNOT_OPEN);
                let resolved = std::fs::canonicalize(path).map_err(|_| refused())?;
                if !std::fs::metadata(&resolved).is_ok_and(|m| m.is_file() || m.is_dir()) {
                    return Err(refused());
                }
                Ok(FileAction::Open {
                    requested: path.to_path_buf(),
                    path: resolved,
                })
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
        // One at a time per plugin, so a plugin cannot stack up dialogs;
        // checked before the path is looked at.
        let waiting = (self.plugins.file_requests.iter())
            .chain(&self.plugins.file_prompt)
            .any(|r| r.plugin == plugin);
        let action = if waiting {
            Err(RpcError::new(
                protocol::INVALID_REQUEST,
                "Another file request of this plugin is waiting for the user",
            ))
        } else {
            self.file_action(&request)
        };
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
        let Some(index) = self.sessions.iter().position(|s| s.document.id == document) else {
            return Err(RpcError::invalid_params("The document was closed"));
        };
        let source = self.plugins.source(plugin);
        let extension = export.unwrap_or("xuan");
        let mut dialog = SaveDialog {
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
            directory: None,
        };
        let cancelled = |app: &mut Self| {
            (app.plugins.file_refused_at).insert(plugin.to_owned(), std::time::Instant::now());
            Err(RpcError::new(
                protocol::CANCELLED,
                "The user cancelled the save dialog",
            ))
        };
        let Some(mut path) = self.show_save_dialog(&dialog) else {
            return cancelled(self);
        };
        // The file is written exactly where the user confirmed, so the
        // system dialog's overwrite question was about this file. A name
        // without the right extension is never changed behind the user's
        // back: the dialog asks again with the extension added.
        if !saves_as(&path, export) {
            let chosen = path.file_name().unwrap_or_default().to_string_lossy();
            dialog.file_name = format!("{chosen}.{extension}");
            dialog.directory = path.parent().map(Path::to_path_buf);
            let Some(again) = self.show_save_dialog(&dialog) else {
                return cancelled(self);
            };
            if !saves_as(&again, export) {
                let name = again.file_name().unwrap_or_default().to_string_lossy();
                return Err(RpcError::invalid_params(match export {
                    Some(_) => format!(
                        "Cannot export as {name}; choose a name ending in .png, .jpg, .tiff or .webp. Nothing was written"
                    ),
                    None => format!(
                        "Cannot save the project as {name}; choose a name ending in .xuan. Nothing was written"
                    ),
                }));
            }
            path = again;
        }
        let failed = |error: anyhow::Error| {
            RpcError::new(
                protocol::INTERNAL_ERROR,
                without_folders(&format!("{error:#}"), &path),
            )
        };
        let session = &mut self.sessions[index];
        match export {
            Some(_) => {
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
        // Only the file name: where the user keeps files is not the
        // plugin's business.
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        Ok(json!({"name": name}))
    }

    /// The save dialog: the system's, or the tests' answer.
    fn show_save_dialog(&self, dialog: &SaveDialog) -> Option<PathBuf> {
        match &self.plugins.save_dialog {
            Some(hook) => hook(dialog),
            None => {
                let mut native = rfd::FileDialog::new()
                    .set_title(&dialog.title)
                    .add_filter(&dialog.filter_name, &dialog.extensions)
                    .set_file_name(&dialog.file_name);
                if let Some(directory) = &dialog.directory {
                    native = native.set_directory(directory);
                }
                native.save_file()
            }
        }
    }

    /// Apply the answer to the open `file/open` prompt.
    pub(super) fn answer_file_prompt(&mut self, open: bool) {
        if self.dialog == Some(Dialog::PluginFile) {
            self.dialog = None;
        }
        let Some(request) = self.plugins.file_prompt.take() else {
            return;
        };
        let FileAction::Open { requested, path } = &request.action else {
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
        } else if !still_the_shown_file(requested, path) {
            // Swapped (or a link in its path retargeted) while the prompt
            // was open: not what the user agreed to open.
            Err(RpcError::invalid_params(
                "The file changed while the user was asked; nothing was opened",
            ))
        } else {
            let before = self.sessions.len();
            let error = self.error.take();
            self.open_path(path, false);
            match self.error.clone() {
                Some(message) if self.sessions.len() == before => {
                    // The plugin named the file, but not where its links
                    // lead: only file names go back.
                    let message = without_folders(&message, path);
                    let message = without_folders(&message, requested);
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
        let FileAction::Open { path, .. } = &request.action else {
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

    #[test]
    fn suggested_names_lose_bidi_and_invisible_characters() {
        // "photo", U+202E, "gnp.exe" would show as "photoexe.png".
        assert_eq!(
            suggested_name(Some("photo\u{202E}gnp.exe.png"), "x"),
            "photognp.exe"
        );
        for c in [
            '\u{200B}', '\u{200E}', '\u{2066}', '\u{2069}', '\u{061C}', '\u{FEFF}',
        ] {
            assert_eq!(suggested_name(Some(&format!("a{c}b")), "x"), "ab", "{c:?}");
        }
        assert_eq!(
            suggested_name(Some("\u{202E}\u{200B}"), "Untitled"),
            "Untitled"
        );
        // Windows drops trailing dots and spaces itself.
        assert_eq!(suggested_name(Some("name. .png"), "x"), "name");
        // The fallback, a document's title, is cleaned too.
        assert_eq!(suggested_name(None, "a\u{202E}b"), "ab");
        assert_eq!(suggested_name(None, "\u{200B}"), "Untitled");
    }

    #[test]
    fn suggested_names_are_never_windows_devices() {
        for name in [
            "CON",
            "con",
            "Prn",
            "AUX",
            "nul",
            "COM1",
            "com9",
            "LPT1",
            "lpt0",
            "COM¹",
            "CONIN$",
            "conout$",
            "NUL .txt",
            "con.tar.gz",
        ] {
            let suggested = suggested_name(Some(name), "x");
            assert!(suggested.starts_with('_'), "{name}: {suggested}");
        }
        assert_eq!(suggested_name(Some("con.png"), "x"), "_con");
        assert_eq!(suggested_name(None, "NUL"), "_NUL");
        for name in ["CONSOLE", "COM10", "LPT", "nullable", "Aux2", "icon"] {
            assert_eq!(suggested_name(Some(name), "x"), name);
        }
    }

    #[test]
    fn plugin_errors_keep_the_file_name_and_drop_its_folders() {
        use super::without_folders;
        use std::path::Path;
        let path = Path::new("/home/user/Private Stuff/out.png");
        let message =
            "No such file or directory (os error 2) at path \"/home/user/Private Stuff/.tmpAbC1\"";
        assert_eq!(
            without_folders(message, path),
            "No such file or directory (os error 2) at path \".tmpAbC1\""
        );
        assert_eq!(
            without_folders("Could not open /home/user/Private Stuff/out.png", path),
            "Could not open out.png"
        );
        assert_eq!(
            without_folders("in /home/user/Private Stuff", path),
            "in the chosen folder"
        );
        // A file in the root keeps the message whole.
        assert_eq!(
            without_folders("a/b /x.png", Path::new("/x.png")),
            "a/b x.png"
        );
    }

    #[test]
    fn only_the_expected_extension_is_written() {
        use super::saves_as;
        use std::path::Path;
        assert!(saves_as(Path::new("/a/b.xuan"), None));
        assert!(saves_as(Path::new("/a/b.XUAN"), None));
        assert!(!saves_as(Path::new("/a/b"), None));
        assert!(!saves_as(Path::new("/a/b.png"), None));
        for name in ["b.png", "b.JPG", "b.jpeg", "b.tif", "b.tiff", "b.webp"] {
            assert!(saves_as(&Path::new("/a").join(name), Some("png")), "{name}");
        }
        assert!(!saves_as(Path::new("/a/b.bmp"), Some("png")));
        assert!(!saves_as(Path::new("/a/b"), Some("png")));
    }
}

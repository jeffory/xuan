//! Files a plugin asks Xuan to save, export or open. Without a `path`,
//! `file/save_as` and `file/export` show the native save dialog, prefilled
//! with the plugin's suggested name, so the user picks (or cancels) the path
//! and the system dialog confirms any overwrite. With a `path` (an LLM cannot
//! operate the system dialog), and for `file/save` (the document back to its
//! own project file), Xuan shows its own prompt naming the plugin, the file,
//! its folder and whether it replaces a file: **Save**, **Always Allow** or
//! **Cancel**. Always Allow is `save_without_asking` in the plugin's grant:
//! later writes happen without the prompt, shown in the status bar and the
//! plugin's log, except replacing a file Xuan did not write for this
//! plugin in this run, which still asks. Replacing any file needs `overwrite: true`. `file/open`
//! names the file in a prompt the user must accept. See "Files the user
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

/// The one answer to a `path` whose file name is not plain.
const UNPLAIN_NAME: &str = "The file name in `path` must be a plain name: no control, bidi or invisible characters, none of \\ / : * ? \" < > |, no leading dot or trailing dot or space, and not a device name such as CON. Nothing was written";

/// Requests that wait for the user's choice of a file.
pub(super) const FILE_METHODS: [&str; 4] =
    ["file/save_as", "file/export", "file/save", "file/open"];

/// Image formats `file/export` writes, by the extension Xuan gives them (`ora` is layered
/// OpenRaster).
const EXPORT_FORMATS: [&str; 5] = ["png", "jpg", "tiff", "webp", "ora"];

/// What a plugin asked for.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FileAction {
    /// The native save dialog for a document: as a project (`export: None`)
    /// or as an image in the given format.
    Save {
        document: Uuid,
        export: Option<String>,
        encoding: Encoding,
        name: String,
    },
    /// Write a document to a path the plugin named, after Xuan's own
    /// prompt or under the grant's `save_without_asking`.
    Write(Write),
    /// Open this file as a document once the user confirms it.
    Open {
        /// The path as the plugin gave it.
        requested: PathBuf,
        /// What it resolved to when the prompt was shown: what the user saw.
        path: PathBuf,
    },
}

/// A document written where the plugin said.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Write {
    pub document: Uuid,
    /// `None` saves the project; `Some(format)` exports an image.
    pub export: Option<String>,
    /// `file/save`: the document's own project file, as Ctrl+S.
    pub in_place: bool,
    /// The file: the plugin's folder resolved (links followed), then the
    /// plain file name it gave.
    pub path: PathBuf,
    /// Whether a file was there when the request was checked.
    pub replaces: bool,
    /// How an exported image is encoded.
    pub encoding: Encoding,
}

/// The `quality` and `lossless` a plugin gave `file/export`. What it leaves
/// out follows the Export dialog, and the dialog keeps the user's choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Encoding {
    /// JPEG and lossy WebP quality, 1–100. Given for WebP without
    /// `lossless`, it means lossy.
    pub quality: Option<u8>,
    /// Lossless WebP; other formats ignore it.
    pub lossless: Option<bool>,
}

impl Encoding {
    /// Read `quality` and `lossless` from a request's params.
    fn parse(params: &Value) -> Result<Self, RpcError> {
        let quality = match params.get("quality") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                (value.as_u64())
                    .and_then(|quality| u8::try_from(quality).ok())
                    .filter(|quality| io::EXPORT_QUALITY.contains(quality))
                    .ok_or_else(|| {
                        RpcError::invalid_params("`quality` must be a whole number from 1 to 100")
                    })?,
            ),
        };
        let lossless = match params.get("lossless") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(lossless)) => Some(*lossless),
            Some(_) => {
                return Err(RpcError::invalid_params("`lossless` must be true or false"));
            }
        };
        Ok(Self { quality, lossless })
    }

    /// The Export dialog's options with this request's choices on top. A plugin's export is
    /// always at the document's size, whatever the dialog's Scale.
    pub(super) fn options(self, dialog: io::ExportOptions) -> io::ExportOptions {
        let quality = self.quality;
        io::ExportOptions {
            jpeg_quality: quality.unwrap_or(dialog.jpeg_quality),
            webp_quality: quality.unwrap_or(dialog.webp_quality),
            webp_lossless: (self.lossless).unwrap_or(quality.is_none() && dialog.webp_lossless),
            scale: 100,
        }
    }
}

/// The user's answer to Xuan's own file prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileAnswer {
    Cancel,
    /// Open, or Save.
    Accept,
    /// Save, and save without asking from now on.
    Always,
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

/// Characters that are invisible or reorder text; see
/// [`xuan::plugins::manifest::invisible`].
pub(super) fn invisible(c: char) -> bool {
    xuan::plugins::manifest::invisible(c)
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
const EXPORT_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "tif", "tiff", "webp", "ora"];

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

/// The format Xuan exports for an image extension.
fn export_format(extension: &str) -> Option<&'static str> {
    match extension.to_ascii_lowercase().as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "tif" | "tiff" => Some("tiff"),
        "webp" => Some("webp"),
        "ora" => Some("ora"),
        _ => None,
    }
}

/// The format `file/export` asked for, normalized.
fn requested_format(format: &str) -> Result<String, RpcError> {
    let format = format.to_ascii_lowercase();
    let format = match format.as_str() {
        "jpeg" => "jpg".to_owned(),
        "tif" => "tiff".to_owned(),
        _ => format,
    };
    if EXPORT_FORMATS.contains(&format.as_str()) {
        Ok(format)
    } else {
        Err(RpcError::invalid_params(format!(
            "`format` must be one of {}",
            EXPORT_FORMATS.join(", ")
        )))
    }
}

/// Whether a file is at `target`: `Ok(true)` for a regular file, refused for
/// a folder, a link or anything else, `Ok(false)` when nothing is there.
fn existing(target: &Path, name: &str) -> Result<bool, RpcError> {
    match std::fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(RpcError::invalid_params(format!(
            "{name} is a folder or a link, not a file. Nothing was written"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(RpcError::invalid_params(format!(
            "Cannot check {name}. Nothing was written"
        ))),
    }
}

/// Check a `path` a plugin gave `file/save_as` (`export: None`) or
/// `file/export` (`export: Some(format)`, `None` when it gave no `format`):
/// where the file goes, its format, and whether it replaces a file. Errors
/// name the file at most, never a folder.
fn write_target(
    path: &Path,
    export: Option<Option<&str>>,
    overwrite: bool,
) -> Result<(PathBuf, Option<String>, bool), RpcError> {
    let invalid = |message: String| RpcError::invalid_params(message);
    if !path.is_absolute() {
        return Err(invalid("`path` must be absolute".into()));
    }
    // Written as given, so a name is refused rather than cleaned: what the
    // prompt shows and what is written are the plugin's own name.
    let name = (path.file_name().and_then(|name| name.to_str()))
        .filter(|name| plain_file_name(name).as_deref() == Some(*name))
        .ok_or_else(|| invalid(UNPLAIN_NAME.into()))?;
    let extension = (Path::new(name).extension())
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let export = match export {
        None if extension.eq_ignore_ascii_case("xuan") => None,
        None => {
            return Err(invalid(format!(
                "Cannot save the project as {name}; the name must end in .xuan. Nothing was written"
            )));
        }
        Some(format) => {
            let Some(found) = export_format(extension) else {
                return Err(invalid(format!(
                    "Cannot export as {name}; the name must end in .png, .jpg, .jpeg, .tif, .tiff, .webp or .ora. Nothing was written"
                )));
            };
            if let Some(format) = format
                && format != found
            {
                return Err(invalid(format!(
                    "Cannot export {format} as {name}; the extension picks the format, so leave out `format` or make them agree. Nothing was written"
                )));
            }
            Some(found.to_owned())
        }
    };
    let folder = (path.parent())
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .filter(|folder| folder.is_dir())
        .ok_or_else(|| {
            invalid(format!(
                "The folder for {name} does not exist; Xuan does not create folders. Nothing was written"
            ))
        })?;
    let target = folder.join(name);
    let replaces = existing(&target, name)?;
    if replaces && !overwrite {
        return Err(invalid(format!(
            "{name} already exists; pass `overwrite: true` to replace it. Nothing was written"
        )));
    }
    Ok((target, export, replaces))
}

/// The file name of a path, for messages.
fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Whether the file is still what the user was asked about: its folder still
/// resolves to itself, and no file appeared where there was none.
fn still_the_target(write: &Write) -> Result<(), RpcError> {
    if write.in_place {
        return Ok(());
    }
    let name = file_name(&write.path);
    let changed = || {
        RpcError::invalid_params(format!(
            "The place to write {name} changed while the user was asked. Nothing was written"
        ))
    };
    let folder = write.path.parent().ok_or_else(changed)?;
    if std::fs::canonicalize(folder).ok().as_deref() != Some(folder) {
        return Err(changed());
    }
    if existing(&write.path, &name)? && !write.replaces {
        return Err(RpcError::invalid_params(format!(
            "{name} appeared while the user was asked, and was not replaced. Nothing was written"
        )));
    }
    Ok(())
}

impl super::plugins::PluginState {
    /// Note a file Xuan wrote for this plugin's request, so the plugin may
    /// replace it without asking once it is allowed to save without asking.
    pub(super) fn remember_written(&mut self, plugin: &str, path: &Path) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        (self.written_files.entry(plugin.to_owned()).or_default()).insert(path);
    }
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
                let document = session.document.id;
                let given = |key: &str| params.get(key).is_some_and(|value| !value.is_null());
                if method == "file/save" {
                    if let Some(key) = [
                        "path",
                        "suggested_name",
                        "format",
                        "overwrite",
                        "quality",
                        "lossless",
                    ]
                    .into_iter()
                    .find(|key| given(key))
                    {
                        return Err(RpcError::invalid_params(format!(
                            "`file/save` takes no `{key}`: it saves to the document's own file; use `file/save_as` for another file"
                        )));
                    }
                    let Some(path) = session.path.clone().filter(|path| saves_as(path, None))
                    else {
                        return Err(RpcError::invalid_params(
                            "The document has no .xuan project file yet; save it with `file/save_as` first",
                        ));
                    };
                    return Ok(FileAction::Write(Write {
                        document,
                        export: None,
                        in_place: true,
                        path,
                        replaces: true,
                        encoding: Encoding::default(),
                    }));
                }
                let format = string("format").map(requested_format).transpose()?;
                let encoding = if method == "file/export" {
                    Encoding::parse(params)?
                } else if let Some(key) = ["quality", "lossless"].into_iter().find(|key| given(key))
                {
                    return Err(RpcError::invalid_params(format!(
                        "`{method}` takes no `{key}`: it saves a project; use `file/export` for an image"
                    )));
                } else {
                    Encoding::default()
                };
                if given("path") {
                    let path = string("path")
                        .ok_or_else(|| RpcError::invalid_params("`path` must be a string"))?;
                    let overwrite = match params.get("overwrite") {
                        None | Some(Value::Null) => false,
                        Some(Value::Bool(overwrite)) => *overwrite,
                        Some(_) => {
                            return Err(RpcError::invalid_params(
                                "`overwrite` must be true or false",
                            ));
                        }
                    };
                    let export = (method == "file/export").then_some(format.as_deref());
                    let (path, export, replaces) =
                        write_target(Path::new(path), export, overwrite)?;
                    return Ok(FileAction::Write(Write {
                        document,
                        export,
                        in_place: false,
                        path,
                        replaces,
                        encoding,
                    }));
                }
                let export =
                    (method == "file/export").then(|| format.unwrap_or_else(|| "png".into()));
                Ok(FileAction::Save {
                    document,
                    export,
                    encoding,
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
        // Another dialog may have replaced the open prompt (a menu command,
        // opening a document): its request still waits, so show it again.
        if let Some(prompt) = &self.plugins.file_prompt
            && self.dialog.is_none()
        {
            if self.plugins.running(&prompt.plugin) {
                self.dialog = Some(Dialog::PluginFile);
            } else {
                self.plugins.file_prompt = None;
            }
        }
        if self.dialog.is_some() || self.plugins.file_prompt.is_some() || self.job.is_some() {
            return;
        }
        let Some(request) = self.plugins.file_requests.pop_front() else {
            return;
        };
        match &request.action {
            FileAction::Write(write) if self.writes_without_asking(&request.plugin, write) => {
                let write = write.clone();
                let result = self.write_for_plugin(&request.plugin, &write, false);
                self.respond_to_plugin(&request.plugin, request.id, result);
            }
            FileAction::Open { .. } | FileAction::Write(_) => {
                self.plugins.file_prompt = Some(request);
                self.dialog = Some(Dialog::PluginFile);
            }
            FileAction::Save {
                document,
                export,
                encoding,
                name,
            } => {
                let result = self.save_for_plugin(
                    &request.plugin,
                    *document,
                    export.as_deref(),
                    *encoding,
                    name,
                );
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
        encoding: Encoding,
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
                        "Cannot export as {name}; choose a name ending in .png, .jpg, .tiff, .webp or .ora. Nothing was written"
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
                let options = encoding.options(self.export_options);
                io::export(&session.document, &path, &options).map_err(failed)?;
                self.status = format!("{} {} · {source}", tr("Exported"), path.display());
            }
            None => {
                // Saved as the user chose, the project now lives there.
                session
                    .save_project(&path, &mut self.file_watch)
                    .map_err(failed)?;
                self.status = format!("{} · {source}", tr("Project saved"));
            }
        }
        self.plugins.remember_written(plugin, &path);
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

    /// Whether the plugin's grant lets it save and export without asking.
    pub(super) fn saves_without_asking(&self, plugin: &str) -> bool {
        self.plugin_granted(plugin)
            && self
                .stored_grant(plugin)
                .is_some_and(|grant| grant.save_without_asking)
    }

    /// Turn "save and export without asking" on or off for a plugin.
    pub(super) fn set_save_without_asking(&mut self, plugin: &str, on: bool) {
        if on && !self.plugin_granted(plugin) {
            return;
        }
        if let Some(grant) = (self.config.plugins.get_mut(plugin)).and_then(|c| c.grant.as_mut())
            && grant.save_without_asking != on
        {
            grant.save_without_asking = on;
            self.save_config();
        }
    }

    /// Whether this write happens without the prompt: the grant allows it,
    /// and it writes a new file, the document's own project, or a file Xuan
    /// wrote for this plugin in this run. Replacing any other file asks,
    /// even then: one the user saved, or another plugin's.
    fn writes_without_asking(&self, plugin: &str, write: &Write) -> bool {
        self.saves_without_asking(plugin)
            && (write.in_place
                || !write.replaces
                || (self.plugins.written_files.get(plugin))
                    .is_some_and(|files| files.contains(&write.path)))
    }

    /// Write a document where the plugin said, once the user agreed or the
    /// grant allows it. `asked` says which, in the answer and the notice.
    fn write_for_plugin(
        &mut self,
        plugin: &str,
        write: &Write,
        asked: bool,
    ) -> Result<Value, RpcError> {
        let Some(index) = (self.sessions.iter()).position(|s| s.document.id == write.document)
        else {
            return Err(RpcError::invalid_params("The document was closed"));
        };
        still_the_target(write)?;
        let path = &write.path;
        let name = file_name(path);
        let failed = |error: anyhow::Error| {
            RpcError::new(
                protocol::INTERNAL_ERROR,
                without_folders(&format!("{error:#}"), path),
            )
        };
        let source = self.plugins.source(plugin);
        let session = &mut self.sessions[index];
        let done = match write.export {
            Some(_) => {
                let options = write.encoding.options(self.export_options);
                io::export(&session.document, path, &options).map_err(failed)?;
                if asked {
                    tr("Exported")
                } else {
                    tr("Exported without asking:")
                }
            }
            None => {
                // Saved as the plugin named it and the user allowed, the
                // project now lives there, as with Save As.
                session
                    .save_project(path, &mut self.file_watch)
                    .map_err(failed)?;
                if asked {
                    tr("Saved")
                } else {
                    tr("Saved without asking:")
                }
            }
        };
        self.plugins.remember_written(plugin, path);
        // The user sees each write, with the whole path: in the status bar,
        // and for writes nobody was asked about, in the plugin's log too.
        self.status = format!("{done} {} · {source}", path.display());
        if !asked && let Some(process) = self.plugins.process_mut(plugin) {
            process.note(format!("{done} {}", path.display()));
        }
        Ok(json!({"name": name, "asked": asked}))
    }

    /// Apply the answer to the open file prompt: `file/open`, or a write to
    /// a path the plugin named.
    pub(super) fn answer_file(&mut self, answer: FileAnswer) {
        if self.dialog == Some(Dialog::PluginFile) {
            self.dialog = None;
        }
        // A withdrawn request took its prompt with it: a late answer finds
        // nothing here.
        let Some(request) = self.plugins.file_prompt.take() else {
            return;
        };
        if answer == FileAnswer::Cancel {
            (self.plugins.file_refused_at)
                .insert(request.plugin.clone(), std::time::Instant::now());
        }
        let result = match &request.action {
            FileAction::Save { .. } => return,
            FileAction::Write(write) => match answer {
                FileAnswer::Cancel => Err(RpcError::new(
                    protocol::CANCELLED,
                    "The user did not allow saving the file",
                )),
                FileAnswer::Accept | FileAnswer::Always => {
                    if answer == FileAnswer::Always {
                        self.set_save_without_asking(&request.plugin, true);
                    }
                    let write = write.clone();
                    self.write_for_plugin(&request.plugin, &write, true)
                }
            },
            FileAction::Open { requested, path } => {
                if answer == FileAnswer::Cancel {
                    Err(RpcError::new(
                        protocol::CANCELLED,
                        "The user did not open the file",
                    ))
                } else if !still_the_shown_file(requested, path) {
                    // Swapped (or a link in its path retargeted) while the
                    // prompt was open: not what the user agreed to open.
                    Err(RpcError::invalid_params(
                        "The file changed while the user was asked; nothing was opened",
                    ))
                } else {
                    let before = self.sessions.len();
                    let error = self.error.take();
                    self.open_path(path, false);
                    match self.error.clone() {
                        Some(message) if self.sessions.len() == before => {
                            // The plugin named the file, but not where its
                            // links lead: only file names go back.
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
        let answer = match &request.action {
            FileAction::Open { path, .. } => self.open_prompt(ctx, &request.plugin, path),
            FileAction::Write(write) => self.write_prompt(ctx, &request.plugin, write),
            FileAction::Save { .. } => Some(FileAnswer::Cancel),
        };
        if let Some(answer) = answer {
            self.answer_file(answer);
        }
    }

    /// "Open a file?", naming the plugin and the file.
    fn open_prompt(&self, ctx: &egui::Context, plugin: &str, path: &Path) -> Option<FileAnswer> {
        let source = self.plugins.source(plugin);
        let mut open = true;
        let mut answer = None;
        widgets::Window::new(tr("Open a file?"))
            .id(("plugin_file", plugin))
            .default_width(440.0)
            .open(&mut open)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.add(
                        egui::Label::new(format!(
                            "{source} {}",
                            tr("asks Xuan to open this file:")
                        ))
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
                },
                |ui, ()| {
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(tr("Open")),
                        |_| {},
                    );
                    if response.commit {
                        answer = Some(FileAnswer::Accept);
                    } else if response.cancel {
                        answer = Some(FileAnswer::Cancel);
                    }
                },
            );
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
            answer = Some(FileAnswer::Cancel);
        }
        answer
    }

    /// Xuan's own prompt in place of the save dialog, for a write to a path
    /// the plugin named: the plugin, the document, the file name, its folder,
    /// and whether it replaces a file.
    fn write_prompt(&self, ctx: &egui::Context, plugin: &str, write: &Write) -> Option<FileAnswer> {
        let source = self.plugins.source(plugin);
        let title = (self.sessions.iter())
            .find(|s| s.document.id == write.document)
            .map(|s| one_line(&s.title, 120))
            .unwrap_or_default();
        let (heading, lead) = match (&write.export, write.in_place) {
            (Some(_), _) => (tr("Export an image?"), tr("asks Xuan to export")),
            (None, true) => (
                tr("Save the project?"),
                tr("asks Xuan to save, to its project file,"),
            ),
            (None, false) => (tr("Save a file?"), tr("asks Xuan to save")),
        };
        let folder = (write.path.parent())
            .map(|folder| one_line(&folder.display().to_string(), 1000))
            .unwrap_or_default();
        let mut open = true;
        let mut answer = None;
        widgets::Window::new(heading)
            .id(("plugin_file", plugin))
            .default_width(460.0)
            .open(&mut open)
            .show_with_footer(ctx, |ui| {
                ui.add(egui::Label::new(format!("{source} {lead} “{title}”:")).wrap());
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(one_line(&file_name(&write.path), 300))
                            .monospace()
                            .strong(),
                    )
                    .wrap(),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} {folder}", tr("in")))
                            .monospace()
                            .small(),
                    )
                    .wrap(),
                );
                ui.add_space(4.0);
                if write.replaces {
                    ui.add(
                        egui::Label::new(
                            RichText::new(tr("This replaces the existing file."))
                                .color(ui.palette().warning),
                        )
                        .wrap(),
                    );
                } else {
                    ui.label(tr("This writes a new file."));
                }
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(tr(
                            "Always Allow lets this plugin save and export without asking until you turn it off in Plugins → Manage Plugins…. It still asks before replacing a file it did not write since Xuan started.",
                        ))
                        .small()
                        .color(ui.palette().muted),
                    )
                    .wrap(),
                );
            }, |ui, ()| {
                let mut always = false;
                let response =
                    widgets::dialog_footer(ui, widgets::FooterButtons::commit(tr("Save")), |ui| {
                        always = widgets::button(ui, tr("Always Allow")).clicked();
                    });
                if always {
                    answer = Some(FileAnswer::Always);
                } else if response.commit {
                    answer = Some(FileAnswer::Accept);
                } else if response.cancel {
                    answer = Some(FileAnswer::Cancel);
                }
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || !open {
            answer = Some(FileAnswer::Cancel);
        }
        answer
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

    #[test]
    fn export_quality_and_lossless_are_checked_and_default_to_the_dialog() {
        use super::Encoding;
        use serde_json::json;
        use xuan::io::ExportOptions;
        let parse = |params| Encoding::parse(&params);
        assert_eq!(parse(json!({})).unwrap(), Encoding::default());
        let nulls = json!({"quality": null, "lossless": null});
        assert_eq!(parse(nulls).unwrap(), Encoding::default());
        assert_eq!(
            parse(json!({"quality": 80, "lossless": false})).unwrap(),
            Encoding {
                quality: Some(80),
                lossless: Some(false)
            }
        );
        for quality in [json!(1), json!(100)] {
            assert!(parse(json!({ "quality": quality })).is_ok(), "{quality}");
        }
        for quality in [
            json!(0),
            json!(101),
            json!(256),
            json!(-5),
            json!(80.5),
            json!("80"),
            json!(true),
        ] {
            let error = parse(json!({ "quality": quality })).unwrap_err();
            assert!(error.message.contains("1 to 100"), "{quality}: {error:?}");
        }
        for lossless in [json!(1), json!("true")] {
            let error = parse(json!({ "lossless": lossless })).unwrap_err();
            assert!(error.message.contains("true or false"), "{error:?}");
        }

        // What a request leaves out follows the dialog.
        let dialog = ExportOptions {
            jpeg_quality: 70,
            webp_quality: 60,
            webp_lossless: true,
            scale: 100,
        };
        assert_eq!(Encoding::default().options(dialog), dialog);
        // The dialog's Scale is not the plugin's.
        let scaled = ExportOptions {
            scale: 50,
            ..dialog
        };
        assert_eq!(Encoding::default().options(scaled), dialog);
        let lossy_dialog = ExportOptions {
            webp_lossless: false,
            ..dialog
        };
        assert_eq!(Encoding::default().options(lossy_dialog), lossy_dialog);
        // A quality means lossy WebP, unless `lossless` says otherwise.
        let quality = Encoding {
            quality: Some(80),
            lossless: None,
        };
        assert_eq!(
            quality.options(dialog),
            ExportOptions {
                jpeg_quality: 80,
                webp_quality: 80,
                webp_lossless: false,
                scale: 100,
            }
        );
        let exact = Encoding {
            lossless: Some(true),
            ..quality
        };
        assert!(exact.options(lossy_dialog).webp_lossless);
        let lossy = Encoding {
            quality: None,
            lossless: Some(false),
        };
        assert_eq!(lossy.options(dialog), lossy_dialog);
    }
}

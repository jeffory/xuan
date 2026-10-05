//! Installing a plugin from a folder or a zip archive into the user plugins
//! directory. See "Installing" in `docs/PLUGINS.md`.
//!
//! Nothing reaches the plugins directory before the user has reviewed it.
//! [`Installer::prepare`] copies the folder, or extracts the archive, into a
//! private staging folder and validates the copy there; [`Installer::commit`]
//! then moves exactly that copy into place, building it in a hidden sibling
//! folder and renaming it, so a failed install leaves nothing half-copied.
//!
//! An archive is untrusted input: entries must be plain files and folders
//! with relative names that stay inside the plugin, and the archive must
//! stay within [`Limits`]. Installing does not allow the plugin to run; it
//! asks for that the first time it starts, as any other plugin does.
use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use super::{Manifest, manifest::MANIFEST_FILE};

/// How large a plugin may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Files and folders.
    pub entries: usize,
    /// Bytes in all files together, once extracted.
    pub total: u64,
    /// Bytes in any one file.
    pub file: u64,
    /// How many times larger than its compressed size an archive entry may
    /// be, for entries above [`RATIO_FLOOR`]. Larger ratios are zip bombs.
    pub ratio: u64,
}

impl Limits {
    pub const DEFAULT: Self = Self {
        entries: 10_000,
        total: 512 << 20,
        file: 256 << 20,
        ratio: 100,
    };
}

/// Archive entries smaller than this are not held to [`Limits::ratio`]: a
/// small file of repeated bytes compresses that well without being a bomb.
pub const RATIO_FLOOR: u64 = 1 << 20;

/// Programs a plugin's command may name without shipping them: interpreters
/// looked up on `PATH`. Any other command must be a file in the plugin folder.
pub const INTERPRETERS: &[&str] = &[
    "python3",
    "python",
    "py",
    "uv",
    "node",
    "deno",
    "bun",
    "ruby",
    "perl",
    "sh",
    "bash",
    "pwsh",
    "powershell",
];

const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

/// Installs plugins into one plugins directory.
#[derive(Clone, Debug)]
pub struct Installer {
    /// Where plugins are installed, `<plugins dir>/<id>`.
    pub plugins_dir: PathBuf,
    /// Every directory plugins load from, to find the same id elsewhere.
    pub search_dirs: Vec<PathBuf>,
    pub limits: Limits,
    /// Where the private staging folders go.
    pub temp_dir: PathBuf,
}

/// A plugin copied into a private staging folder and validated, waiting for
/// the user to install it. Dropping it removes the staging folder.
#[derive(Debug)]
pub struct Staged {
    /// The manifest; its `dir` is the staged copy.
    pub manifest: Manifest,
    /// The folder or archive the user picked.
    pub source: PathBuf,
    /// The folder it is installed to, `<plugins dir>/<id>`.
    pub target: PathBuf,
    /// Whether `target` already holds this plugin, which is then replaced.
    pub update: bool,
    /// The installed plugin an update replaces, if its manifest still loads.
    pub previous: Option<Manifest>,
    _staging: tempfile::TempDir,
}

impl Installer {
    pub fn new(plugins_dir: PathBuf, search_dirs: Vec<PathBuf>) -> Self {
        Self {
            plugins_dir,
            search_dirs,
            limits: Limits::DEFAULT,
            temp_dir: std::env::temp_dir(),
        }
    }

    /// Copy or extract `source` (a folder or a `.zip`) into a private
    /// staging folder and check it: its layout, its manifest, its command and
    /// where it would go. Nothing is written to the plugins directory.
    pub fn prepare(&self, source: &Path) -> Result<Staged> {
        let staging = super::private_dir_in("xuan-plugin-", &self.temp_dir)
            .context("Cannot create a temporary folder")?;
        let content = staging.path().join("content");
        fs::create_dir(&content).context("Cannot create a temporary folder")?;
        let metadata =
            fs::metadata(source).with_context(|| format!("Cannot read {}", source.display()))?;
        if metadata.is_dir() {
            copy_tree(source, &content, &self.limits, &mut Budget::default())
        } else {
            extract_zip(source, &content, &self.limits)
        }
        .with_context(|| format!("Cannot install {}", source.display()))?;
        let root = plugin_root(&content)?;
        let text = fs::read_to_string(root.join(MANIFEST_FILE))
            .with_context(|| format!("Cannot read {MANIFEST_FILE}"))?;
        let manifest =
            Manifest::parse(&text, &root).with_context(|| format!("Invalid {MANIFEST_FILE}"))?;
        check_command(&manifest)?;
        let target = self.plugins_dir.join(&manifest.plugin.id);
        let previous = self.check_target(&manifest.plugin.id, &target)?;
        Ok(Staged {
            source: source.to_path_buf(),
            target,
            update: previous.is_some(),
            previous: previous.flatten(),
            manifest,
            _staging: staging,
        })
    }

    /// Move a staged plugin into `<plugins dir>/<id>`, replacing the folder
    /// only when it holds the same plugin. The copy is built in a hidden
    /// sibling folder and renamed into place.
    pub fn commit(&self, staged: Staged) -> Result<PathBuf> {
        let id = &staged.manifest.plugin.id;
        let target = &staged.target;
        let update = self.check_target(id, target)?.is_some();
        ensure!(
            update == staged.update,
            "{} changed since the plugin was reviewed; try again",
            target.display()
        );
        fs::create_dir_all(&self.plugins_dir)
            .with_context(|| format!("Cannot create {}", self.plugins_dir.display()))?;
        let building = tempfile::Builder::new()
            .prefix(&format!(".{id}.installing-"))
            .tempdir_in(&self.plugins_dir)
            .with_context(|| format!("Cannot write to {}", self.plugins_dir.display()))?;
        copy_tree(
            &staged.manifest.dir,
            building.path(),
            &self.limits,
            &mut Budget::default(),
        )
        .with_context(|| format!("Cannot copy the plugin into {}", self.plugins_dir.display()))?;
        let building = building.keep();
        let moved = if update {
            let old = self
                .plugins_dir
                .join(format!(".{id}.replaced-{}", uuid::Uuid::new_v4().simple()));
            match fs::rename(target, &old) {
                Ok(()) => match fs::rename(&building, target) {
                    Ok(()) => {
                        let _ = fs::remove_dir_all(&old);
                        Ok(())
                    }
                    Err(error) => {
                        let _ = fs::rename(&old, target);
                        Err(error)
                    }
                },
                Err(error) => Err(error),
            }
        } else if fs::symlink_metadata(target).is_ok() {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the folder appeared while installing",
            ))
        } else {
            fs::rename(&building, target)
        };
        if let Err(error) = moved {
            let _ = fs::remove_dir_all(&building);
            return Err(error).with_context(|| format!("Cannot install into {}", target.display()));
        }
        Ok(target.clone())
    }

    /// Whether the plugin `id` may go to `target`: `None` when the folder is
    /// free, `Some(previous manifest)` when it holds the same plugin, which
    /// is then updated. Another folder with that id, on any search
    /// directory, or a target holding something else, refuses the install.
    fn check_target(&self, id: &str, target: &Path) -> Result<Option<Option<Manifest>>> {
        let own = canonical(target);
        for dir in &self.search_dirs {
            for folder in super::plugin_folders(dir) {
                if folder_id(&folder).as_deref() == Some(id) && canonical(&folder) != own {
                    bail!(
                        "{}: {}. It is not installed; remove one of them first.",
                        super::conflict_message(id),
                        folder.display()
                    );
                }
            }
        }
        match fs::symlink_metadata(target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("Cannot read {}", target.display())),
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir() && folder_id(target).as_deref() == Some(id),
                    "{} already exists and does not hold the plugin `{id}`; it is left as it is",
                    target.display()
                );
                Ok(Some(Manifest::load(target).ok()))
            }
        }
    }
}

impl Staged {
    /// The staged copy of the plugin folder.
    pub fn root(&self) -> &Path {
        &self.manifest.dir
    }
}

/// `path` canonicalized, or its canonical parent joined with its name when
/// it does not exist yet.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// The `plugin.id` in a folder's manifest, read without validating the rest,
/// so a broken or incompatible plugin still counts as that plugin.
fn folder_id(folder: &Path) -> Option<String> {
    let text = fs::read_to_string(folder.join(MANIFEST_FILE)).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("plugin")?.get("id")?.as_str().map(str::to_owned)
}

/// The folder with `plugin.toml`: the top of the content or its only folder.
fn plugin_root(content: &Path) -> Result<PathBuf> {
    if content.join(MANIFEST_FILE).is_file() {
        return Ok(content.to_path_buf());
    }
    let entries: Vec<PathBuf> = fs::read_dir(content)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| !path.ends_with(".DS_Store"))
        .collect();
    if let [only] = entries.as_slice()
        && only.is_dir()
        && only.join(MANIFEST_FILE).is_file()
    {
        return Ok(only.clone());
    }
    bail!("There is no {MANIFEST_FILE} at its top level or in its single top-level folder")
}

/// Whether `path` is relative and stays where it starts.
fn stays_inside(path: &Path) -> bool {
    !path.has_root()
        && !path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn is_interpreter(program: &str) -> bool {
    let lower = program.to_ascii_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    INTERPRETERS.contains(&name)
        || name
            .strip_prefix("python3.")
            .is_some_and(|minor| !minor.is_empty() && minor.bytes().all(|b| b.is_ascii_digit()))
}

/// The command must run a file inside the plugin folder (the same rule the
/// host uses to start it), or a known interpreter, and its arguments must not
/// point outside the folder. The file need not exist yet: a setup script may
/// create it (see "Setup convention" in `docs/PLUGINS.md`).
fn check_command(manifest: &Manifest) -> Result<()> {
    let command = &manifest.plugin.command;
    let program = &command[0];
    let path = Path::new(program);
    ensure!(
        stays_inside(path),
        "Its command `{program}` is outside the plugin folder"
    );
    if super::host::resolve(program, &manifest.dir) == path {
        ensure!(
            is_interpreter(program),
            "Its command `{program}` is neither a file in the plugin folder nor a known interpreter ({})",
            INTERPRETERS.join(", ")
        );
    }
    for argument in &command[1..] {
        let path = Path::new(argument);
        ensure!(
            !path.has_root()
                && !path.is_absolute()
                && !path.components().any(|c| c == Component::ParentDir),
            "Its command argument `{argument}` points outside the plugin folder"
        );
    }
    Ok(())
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{} MiB", bytes >> 20)
    } else {
        format!("{bytes} bytes")
    }
}

/// Entries and bytes written so far, held to [`Limits`].
#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
}

impl Budget {
    fn entry(&mut self, limits: &Limits) -> Result<()> {
        self.entries += 1;
        ensure!(
            self.entries <= limits.entries,
            "It has more than {} files and folders",
            limits.entries
        );
        Ok(())
    }

    fn bytes(&mut self, bytes: u64, limits: &Limits) -> Result<()> {
        self.bytes = self.bytes.saturating_add(bytes);
        ensure!(
            self.bytes <= limits.total,
            "It is larger than {} in total",
            size_text(limits.total)
        );
        Ok(())
    }
}

/// Copy a folder of plain files and folders. Links and special files are
/// refused, setuid and setgid bits dropped, and the executable bit kept.
fn copy_tree(from: &Path, to: &Path, limits: &Limits, budget: &mut Budget) -> Result<()> {
    let mut entries = fs::read_dir(from)
        .with_context(|| format!("Cannot read {}", from.display()))?
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let destination = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)?;
        let kind = metadata.file_type();
        budget.entry(limits)?;
        if kind.is_symlink() {
            bail!(
                "{} is a link; a plugin can hold only files and folders",
                path.display()
            );
        } else if kind.is_dir() {
            fs::create_dir(&destination)?;
            copy_tree(&path, &destination, limits, budget)?;
        } else if kind.is_file() {
            ensure!(
                metadata.len() <= limits.file,
                "{} is larger than {}",
                path.display(),
                size_text(limits.file)
            );
            let mut source =
                fs::File::open(&path).with_context(|| format!("Cannot read {}", path.display()))?;
            let mut file = fs::File::create_new(&destination)?;
            let copied = io::copy(&mut (&mut source).take(limits.file + 1), &mut file)?;
            ensure!(
                copied <= limits.file,
                "{} is larger than {}",
                path.display(),
                size_text(limits.file)
            );
            budget.bytes(copied, limits)?;
            drop(file);
            set_mode(&destination, executable(&metadata))?;
        } else {
            bail!(
                "{} is not a plain file or folder (a device, pipe or socket)",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &fs::Metadata) -> bool {
    false
}

/// `rwxr-xr-x` for executables, `rw-r--r--` for other files: never setuid,
/// setgid or sticky, whatever the source said.
#[cfg(unix)]
fn set_mode(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}

/// Names Windows reserves for devices, with or without an extension.
fn reserved_on_windows(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
        })
}

/// An archive entry's name as a relative path inside the plugin, or `None`
/// for entries that are skipped (`__MACOSX` metadata, the root itself).
/// Absolute names, drive letters, UNC paths and `..` are refused, as are
/// names Windows cannot create, so an archive installs the same everywhere.
fn entry_path(name: &str) -> Result<Option<PathBuf>> {
    let text = name.replace('\\', "/");
    let refuse = || anyhow::anyhow!("The archive entry {name:?} is not a safe relative path");
    if text.starts_with('/')
        || text
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(refuse());
    }
    let mut path = PathBuf::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(refuse()),
            _ if part.ends_with(['.', ' ']) || reserved_on_windows(part) => return Err(refuse()),
            _ => path.push(part),
        }
    }
    if path.as_os_str().is_empty() || path.starts_with("__MACOSX") {
        return Ok(None);
    }
    Ok(Some(path))
}

/// Names seen in an archive, by their lower-case form, to refuse two that a
/// case-insensitive file system would merge, or a file that is also a folder.
#[derive(Default)]
struct Names(HashMap<String, (String, bool)>);

impl Names {
    fn add(&mut self, path: &Path, is_dir: bool) -> Result<()> {
        let parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        for end in 1..=parts.len() {
            let name = parts[..end].join("/");
            let dir = end < parts.len() || is_dir;
            match self.0.get(&name.to_lowercase()) {
                None => {
                    self.0.insert(name.to_lowercase(), (name, dir));
                }
                Some((seen, seen_dir)) if *seen == name && *seen_dir && dir => {}
                Some((seen, _)) => bail!(
                    "The archive has both {seen:?} and {name:?}, which clash: a name is both a file and a folder, or names differ only by letter case"
                ),
            }
        }
        Ok(())
    }
}

/// The number of central directory records from `start`, counting at most
/// `cap`. The reader keeps one entry per name, so more records than entries
/// means repeated names.
fn central_records(path: &Path, start: u64, cap: usize) -> Result<usize> {
    let mut file = io::BufReader::new(fs::File::open(path)?);
    file.seek(SeekFrom::Start(start))?;
    let mut header = [0u8; 46];
    let mut count = 0;
    while count < cap && file.read_exact(&mut header).is_ok() && header[..4] == *b"PK\x01\x02" {
        let length = |at: usize| i64::from(u16::from_le_bytes([header[at], header[at + 1]]));
        file.seek_relative(length(28) + length(30) + length(32))?;
        count += 1;
    }
    Ok(count)
}

/// Extract an untrusted zip archive into the empty folder `to`.
fn extract_zip(path: &Path, to: &Path, limits: &Limits) -> Result<()> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(io::BufReader::new(file))
        .context("It is neither a folder nor a zip archive")?;
    let records = central_records(path, archive.central_directory_start(), limits.entries + 1)?;
    ensure!(
        archive.len() <= limits.entries && records <= limits.entries,
        "It has more than {} files and folders",
        limits.entries
    );
    ensure!(
        records == archive.len(),
        "The archive has several entries with the same name"
    );
    let mut declared = 0u64;
    for index in 0..archive.len() {
        declared = declared.saturating_add(archive.by_index_raw(index)?.size());
    }
    ensure!(
        declared <= limits.total,
        "It is larger than {} in total",
        size_text(limits.total)
    );
    let mut names = Names::default();
    let mut budget = Budget::default();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        let Some(relative) = entry_path(&name)? else {
            continue;
        };
        let mode = entry.unix_mode();
        let kind = mode.map_or(0, |mode| mode & S_IFMT);
        ensure!(
            kind != S_IFLNK,
            "The archive entry {name:?} is a link; a plugin can hold only files and folders"
        );
        ensure!(
            matches!(kind, 0 | S_IFREG | S_IFDIR),
            "The archive entry {name:?} is not a plain file or folder"
        );
        ensure!(
            !entry.encrypted(),
            "The archive entry {name:?} is encrypted"
        );
        let is_dir = kind == S_IFDIR || name.ends_with(['/', '\\']);
        names.add(&relative, is_dir)?;
        budget.entry(limits)?;
        let destination = to.join(&relative);
        if is_dir {
            fs::create_dir_all(&destination)?;
            continue;
        }
        let size = entry.size();
        ensure!(
            size <= limits.file,
            "The archive entry {name:?} is larger than {}",
            size_text(limits.file)
        );
        ensure!(
            size < RATIO_FLOOR || size / entry.compressed_size().max(1) <= limits.ratio,
            "The archive entry {name:?} expands more than {} times; it looks like a zip bomb",
            limits.ratio
        );
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create_new(&destination)?;
        let written = io::copy(&mut (&mut entry).take(size + 1), &mut file)
            .with_context(|| format!("Cannot extract {name:?}"))?;
        ensure!(
            written == size,
            "The archive entry {name:?} is not the size the archive says"
        );
        budget.bytes(written, limits)?;
        drop(file);
        set_mode(&destination, mode.is_some_and(|mode| mode & 0o111 != 0))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// CRLF line ends, as an editor on Windows saves them.
    const MANIFEST: &str = "[plugin]\r\nid = \"demo\"\r\nname = \"Demo\"\r\nversion = \"1.0.0\"\r\ncommand = [\"python3\", \"main.py\"]\r\n";

    fn manifest(version: &str, extra: &str) -> String {
        format!("{}{extra}", MANIFEST.replace("1.0.0", version))
    }

    /// A temporary home: `plugins`, `tmp` (staging) and `src` (what is
    /// installed from). Never the real configuration folder.
    struct Home {
        dir: tempfile::TempDir,
        installer: Installer,
    }

    impl Home {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            for sub in ["tmp", "src"] {
                fs::create_dir(dir.path().join(sub)).unwrap();
            }
            let plugins = dir.path().join("plugins");
            let installer = Installer {
                plugins_dir: plugins.clone(),
                search_dirs: vec![plugins],
                limits: Limits::DEFAULT,
                temp_dir: dir.path().join("tmp"),
            };
            Self { dir, installer }
        }

        fn src(&self, name: &str) -> PathBuf {
            self.dir.path().join("src").join(name)
        }

        fn write_zip(&self, bytes: &[u8]) -> PathBuf {
            let path = self.src("plugin.zip");
            fs::write(&path, bytes).unwrap();
            path
        }

        /// A plugin folder under `src` with these files.
        fn folder(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let root = self.src(name);
            for (file, text) in files {
                let path = root.join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, text).unwrap();
            }
            root
        }

        fn plugins(&self) -> &Path {
            &self.installer.plugins_dir
        }

        /// No staging folder and no hidden install folder is left.
        #[track_caller]
        fn assert_clean(&self) {
            let staging: Vec<_> = fs::read_dir(self.dir.path().join("tmp")).unwrap().collect();
            assert!(staging.is_empty(), "staging left: {staging:?}");
            if let Ok(entries) = fs::read_dir(self.plugins()) {
                let hidden: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
                    .collect();
                assert!(hidden.is_empty(), "hidden folders left: {hidden:?}");
            }
        }

        #[track_caller]
        fn refused(&self, source: &Path, expected: &str) -> String {
            let error = format!("{:#}", self.installer.prepare(source).unwrap_err());
            assert!(error.contains(expected), "{expected:?} not in {error:?}");
            self.assert_clean();
            error
        }
    }

    /// Builds archives, including ones a careful writer would not make.
    struct Archive(zip::ZipWriter<io::Cursor<Vec<u8>>>);

    impl Archive {
        fn new() -> Self {
            Self(zip::ZipWriter::new(io::Cursor::new(Vec::new())))
        }

        fn with_manifest() -> Self {
            let mut archive = Self::new();
            archive.file("plugin.toml", MANIFEST.as_bytes());
            archive
        }

        fn file(&mut self, name: &str, data: &[u8]) -> &mut Self {
            let options = SimpleFileOptions::default().unix_permissions(0o644);
            self.0.start_file(name, options).unwrap();
            self.0.write_all(data).unwrap();
            self
        }

        fn dir(&mut self, name: &str) -> &mut Self {
            self.0
                .add_directory(name, SimpleFileOptions::default())
                .unwrap();
            self
        }

        fn symlink(&mut self, name: &str, target: &str) -> &mut Self {
            self.0
                .add_symlink(name, target, SimpleFileOptions::default())
                .unwrap();
            self
        }

        fn finish(self) -> Vec<u8> {
            self.0.finish().unwrap().into_inner()
        }
    }

    /// Set the Unix mode an archive's central directory gives `name`; the
    /// writer keeps only permission bits, so file types and setuid need this.
    fn set_mode_bits(bytes: &mut [u8], name: &str, mode: u32) {
        let mut at = 0;
        let mut found = false;
        while let Some(offset) = bytes[at..].windows(4).position(|w| w == b"PK\x01\x02") {
            let start = at + offset;
            let length = u16::from_le_bytes([bytes[start + 28], bytes[start + 29]]) as usize;
            if &bytes[start + 46..start + 46 + length] == name.as_bytes() {
                bytes[start + 5] = 3; // made by Unix
                bytes[start + 38..start + 42].copy_from_slice(&(mode << 16).to_le_bytes());
                found = true;
            }
            at = start + 4;
        }
        assert!(found, "{name} not in the archive");
    }

    #[test]
    fn hostile_archives_are_refused_and_leave_nothing_behind() {
        let home = Home::new();
        let with = |name: &str| {
            let mut archive = Archive::with_manifest();
            archive.file(name, b"x");
            archive.finish()
        };
        for (name, expected) in [
            ("../x", "not a safe relative path"),
            ("a/../../x", "not a safe relative path"),
            ("/etc/x", "not a safe relative path"),
            ("C:\\x", "not a safe relative path"),
            ("C:/x", "not a safe relative path"),
            ("\\\\server\\share\\x", "not a safe relative path"),
            ("\\x", "not a safe relative path"),
            ("CON.txt", "not a safe relative path"),
            ("a\u{1}b", "not a safe relative path"),
        ] {
            home.refused(&home.write_zip(&with(name)), expected);
        }

        let mut archive = Archive::with_manifest();
        archive.symlink("link", "/etc/passwd");
        home.refused(&home.write_zip(&archive.finish()), "is a link");

        // Character and block devices, pipes and sockets.
        for kind in [0o020_000, 0o060_000, 0o010_000, 0o140_000] {
            let mut bytes = with("dev");
            set_mode_bits(&mut bytes, "dev", kind | 0o644);
            home.refused(&home.write_zip(&bytes), "not a plain file or folder");
        }

        let mut archive = Archive::with_manifest();
        archive.file("main.py", b"a").file("MAIN.py", b"b");
        home.refused(&home.write_zip(&archive.finish()), "letter case");
        let mut archive = Archive::with_manifest();
        archive.file("lib", b"a").file("Lib/x.py", b"b");
        home.refused(&home.write_zip(&archive.finish()), "letter case");
        let mut archive = Archive::with_manifest();
        archive.dir("lib/").dir("LIB/");
        home.refused(&home.write_zip(&archive.finish()), "letter case");

        // The reader keeps one entry per name, so a repeated name would
        // otherwise slip through unseen.
        let mut archive = Archive::with_manifest();
        archive.file("a.py", b"first").file("b.py", b"second");
        let mut bytes = archive.finish();
        let positions: Vec<usize> = (0..bytes.len() - 4)
            .filter(|&i| &bytes[i..i + 4] == b"b.py")
            .collect();
        for i in positions {
            bytes[i] = b'a';
        }
        home.refused(&home.write_zip(&bytes), "same name");

        home.refused(&home.write_zip(b"not a zip at all"), "zip archive");
        assert!(
            !home.plugins().exists(),
            "nothing reaches the plugins folder"
        );
    }

    #[test]
    fn archives_are_held_to_the_limits() {
        let mut home = Home::new();
        home.installer.limits = Limits {
            entries: 5,
            total: 10_000,
            file: 4_000,
            ratio: 100,
        };
        let mut archive = Archive::with_manifest();
        for i in 0..5 {
            archive.file(&format!("f{i}"), b"x");
        }
        home.refused(&home.write_zip(&archive.finish()), "more than 5 files");

        let mut archive = Archive::with_manifest();
        archive
            .file("a", &[1; 3_500])
            .file("b", &[2; 3_500])
            .file("c", &[3; 3_500]);
        home.refused(
            &home.write_zip(&archive.finish()),
            "larger than 10000 bytes in total",
        );

        let mut archive = Archive::with_manifest();
        archive.file("big", &[1; 4_001]);
        home.refused(&home.write_zip(&archive.finish()), "larger than 4000 bytes");

        // A folder is held to the same limits.
        let folder = home.folder(
            "many",
            &[
                ("plugin.toml", MANIFEST),
                ("a", "1"),
                ("b", "2"),
                ("c", "3"),
                ("d", "4"),
                ("e", "5"),
            ],
        );
        home.refused(&folder, "more than 5 files");

        let mut home = Home::new();
        let mut archive = Archive::with_manifest();
        archive.file("zeros.bin", &vec![0; 4 << 20]);
        home.refused(&home.write_zip(&archive.finish()), "zip bomb");
        // Small, very compressible files are fine.
        let mut archive = Archive::with_manifest();
        archive.file("zeros.bin", &vec![0; 64 << 10]);
        let staged = home
            .installer
            .prepare(&home.write_zip(&archive.finish()))
            .unwrap();
        assert_eq!(
            fs::metadata(staged.root().join("zeros.bin")).unwrap().len(),
            64 << 10
        );
        drop(staged);
        home.assert_clean();
        home.installer.limits.entries = 1;
        let error = home
            .installer
            .prepare(&home.write_zip(&Archive::with_manifest().finish()))
            .map(|_| ());
        assert!(error.is_ok(), "{error:?}");
    }

    #[cfg(unix)]
    #[test]
    fn executable_bits_are_kept_and_setuid_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let home = Home::new();
        let mut archive = Archive::with_manifest();
        archive
            .file("run.sh", b"#!/bin/sh\n")
            .file("data.bin", b"d")
            .file("tool", b"t");
        let mut bytes = archive.finish();
        set_mode_bits(&mut bytes, "run.sh", S_IFREG | 0o4755);
        set_mode_bits(&mut bytes, "data.bin", S_IFREG | 0o2644);
        set_mode_bits(&mut bytes, "tool", S_IFREG | 0o1700);
        let staged = home.installer.prepare(&home.write_zip(&bytes)).unwrap();
        let mode = |root: &Path, name: &str| {
            fs::metadata(root.join(name)).unwrap().permissions().mode() & 0o7777
        };
        assert_eq!(mode(staged.root(), "run.sh"), 0o755);
        assert_eq!(mode(staged.root(), "data.bin"), 0o644);
        assert_eq!(mode(staged.root(), "tool"), 0o755);
        let target = home.installer.commit(staged).unwrap();
        assert_eq!(mode(&target, "run.sh"), 0o755);
        assert_eq!(mode(&target, "data.bin"), 0o644);
        assert_eq!(mode(&target, "plugin.toml"), 0o644);
        home.assert_clean();

        // From a folder too.
        let home = Home::new();
        let folder = home.folder("setuid", &[("plugin.toml", MANIFEST), ("run.sh", "x")]);
        fs::set_permissions(folder.join("run.sh"), fs::Permissions::from_mode(0o4755)).unwrap();
        let target = home
            .installer
            .commit(home.installer.prepare(&folder).unwrap())
            .unwrap();
        assert_eq!(mode(&target, "run.sh"), 0o755);
    }

    #[test]
    fn a_valid_archive_installs_from_its_root_or_single_folder() {
        for layout in ["", "demo-1.0/"] {
            let home = Home::new();
            let mut archive = Archive::new();
            if !layout.is_empty() {
                archive.dir(layout);
            }
            archive
                .file(&format!("{layout}plugin.toml"), MANIFEST.as_bytes())
                .file(&format!("{layout}main.py"), b"print('hi')\n")
                .file(&format!("{layout}lib/util.py"), b"")
                .file("__MACOSX/._plugin.toml", b"resource fork");
            let source = home.write_zip(&archive.finish());
            let staged = home.installer.prepare(&source).unwrap();
            assert_eq!(staged.manifest.plugin.id, "demo");
            assert_eq!(staged.source, source);
            assert_eq!(staged.target, home.plugins().join("demo"));
            assert!(!staged.update);
            // Nothing is in the plugins directory before the commit.
            assert!(!home.plugins().exists());
            let target = home.installer.commit(staged).unwrap();
            assert_eq!(
                fs::read_to_string(target.join("main.py")).unwrap(),
                "print('hi')\n"
            );
            assert!(target.join("lib/util.py").is_file());
            assert!(!target.join("__MACOSX").exists());
            let (manifests, errors) = super::super::discover(&[home.plugins().to_path_buf()]);
            assert_eq!(manifests.len(), 1, "{errors:?}");
            assert_eq!(manifests[0].dir, target);
            home.assert_clean();
        }
    }

    #[test]
    fn a_valid_folder_installs_and_layouts_without_a_manifest_are_refused() {
        let home = Home::new();
        let folder = home.folder(
            "demo-src",
            &[("plugin.toml", MANIFEST), ("main.py", "pass\n")],
        );
        let staged = home.installer.prepare(&folder).unwrap();
        // The source can change after the review; what was reviewed is installed.
        fs::write(folder.join("main.py"), "changed").unwrap();
        let target = home.installer.commit(staged).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("main.py")).unwrap(),
            "pass\n"
        );
        home.assert_clean();

        let nested = home.folder("nested", &[("outer/demo/plugin.toml", MANIFEST)]);
        home.refused(&nested, "no plugin.toml");
        let two = home.folder("two", &[("a/plugin.toml", MANIFEST), ("b/x", "")]);
        home.refused(&two, "no plugin.toml");
        home.refused(&home.src("missing"), "Cannot read");

        #[cfg(unix)]
        {
            let linked = home.folder("linked", &[("plugin.toml", MANIFEST)]);
            std::os::unix::fs::symlink("/etc/passwd", linked.join("passwd")).unwrap();
            home.refused(&linked, "is a link");
        }
    }

    #[test]
    fn manifests_and_commands_are_checked_before_anything_is_copied() {
        let home = Home::new();
        let install = |extra: &str, command: &str| {
            let text = manifest("1.0.0", extra).replace("[\"python3\", \"main.py\"]", command);
            let folder = home.folder("checked", &[("plugin.toml", &text)]);
            let result = home.installer.prepare(&folder).map(|_| ());
            fs::remove_dir_all(&folder).unwrap();
            result.map_err(|error| format!("{error:#}"))
        };
        let ok = "[\"python3\", \"main.py\"]";
        assert!(install("", ok).is_ok());
        assert!(install("", "[\"bin/tool\", \"--x\"]").is_ok());
        assert!(install("", "[\".venv/bin/python\", \"main.py\"]").is_ok());
        assert!(install("", "[\"python3.12\", \"main.py\"]").is_ok());
        assert!(install("", "[\"node.exe\", \"main.js\"]").is_ok());
        for (command, expected) in [
            ("[\"/usr/bin/curl\"]", "outside the plugin folder"),
            ("[\"../evil\"]", "outside the plugin folder"),
            ("[\"bin/../../evil\"]", "outside the plugin folder"),
            ("[\"curl\", \"x\"]", "nor a known interpreter"),
            ("[\"python3\", \"/tmp/x.py\"]", "points outside"),
            ("[\"python3\", \"../x.py\"]", "points outside"),
        ] {
            let error = install("", command).unwrap_err();
            assert!(error.contains(expected), "{command}: {error}");
        }
        // A bare name that is a file in the folder runs from the folder.
        let folder = home.folder(
            "local",
            &[
                (
                    "plugin.toml",
                    &MANIFEST.replace("[\"python3\", \"main.py\"]", "[\"tool\"]"),
                ),
                ("tool", ""),
            ],
        );
        assert!(home.installer.prepare(&folder).is_ok());

        let error = install("", "[]").unwrap_err();
        assert!(error.contains("Invalid plugin.toml"), "{error}");
        let error = install("", "[\"python3\"]\nid = 2").unwrap_err();
        assert!(error.contains("Invalid plugin.toml"), "{error}");
        home.assert_clean();
    }

    #[test]
    fn requires_xuan_is_checked_when_installing() {
        let home = Home::new();
        let range = format!(">={}", super::super::manifest::XUAN_VERSION);
        let text = MANIFEST.replace(
            "command",
            &format!("requires_xuan = \"{range}\"\r\ncommand"),
        );
        let folder = home.folder("new-enough", &[("plugin.toml", &text)]);
        assert!(home.installer.prepare(&folder).is_ok());
        let text = MANIFEST.replace("command", "requires_xuan = \"<0.0.1\"\r\ncommand");
        let folder = home.folder("too-new", &[("plugin.toml", &text)]);
        let error = home.refused(&folder, "requires Xuan <0.0.1");
        assert!(
            error.contains(&format!(
                "this is Xuan {}",
                super::super::manifest::XUAN_VERSION
            )),
            "{error}"
        );
    }

    #[test]
    fn the_same_plugin_is_updated_in_place_and_others_are_never_overwritten() {
        let mut home = Home::new();
        let v1 = home.folder(
            "v1",
            &[("plugin.toml", &manifest("1.0.0", "")), ("old.py", "")],
        );
        let target = home
            .installer
            .commit(home.installer.prepare(&v1).unwrap())
            .unwrap();
        let v2 = home.folder(
            "v2",
            &[("plugin.toml", &manifest("2.0.0", "")), ("new.py", "")],
        );
        let staged = home.installer.prepare(&v2).unwrap();
        assert!(staged.update);
        assert_eq!(staged.previous.as_ref().unwrap().plugin.version, "1.0.0");
        assert_eq!(home.installer.commit(staged).unwrap(), target);
        assert!(target.join("new.py").is_file());
        assert!(!target.join("old.py").exists(), "the folder is replaced");
        assert_eq!(Manifest::load(&target).unwrap().plugin.version, "2.0.0");
        home.assert_clean();

        // An update whose installed manifest no longer loads still updates.
        fs::write(
            target.join("plugin.toml"),
            "[plugin]\nid = \"demo\"\nname = \"\"\n",
        )
        .unwrap();
        let staged = home.installer.prepare(&v2).unwrap();
        assert!(staged.update && staged.previous.is_none());

        // The folder appearing or going away after the review stops the commit.
        fs::remove_dir_all(&target).unwrap();
        let error = home.installer.commit(staged).unwrap_err().to_string();
        assert!(error.contains("changed since"), "{error}");
        home.assert_clean();

        // The same id in another plugin folder, such as one on
        // XUAN_PLUGIN_PATH, is a conflict: neither would load.
        let elsewhere = home.dir.path().join("dev");
        fs::create_dir_all(elsewhere.join("demo-dev")).unwrap();
        fs::write(elsewhere.join("demo-dev/plugin.toml"), MANIFEST).unwrap();
        home.installer.search_dirs.insert(0, elsewhere.clone());
        let error = home.refused(&v2, &super::super::conflict_message("demo"));
        assert!(error.contains("demo-dev"), "{error}");
        home.installer.search_dirs.remove(0);

        // A folder named after the id holding something else is left alone.
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("plugin.toml"),
            MANIFEST.replace("\"demo\"", "\"other\""),
        )
        .unwrap();
        home.refused(&v2, "does not hold the plugin `demo`");
        assert!(target.join("plugin.toml").is_file());
        fs::remove_dir_all(&target).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("notes.txt"), "mine").unwrap();
        home.refused(&v2, "does not hold the plugin `demo`");
        assert_eq!(
            fs::read_to_string(target.join("notes.txt")).unwrap(),
            "mine"
        );
        fs::remove_dir_all(&target).unwrap();
        fs::write(&target, "a file").unwrap();
        home.refused(&v2, "does not hold the plugin `demo`");
    }

    #[test]
    fn hidden_folders_are_not_loaded() {
        let home = Home::new();
        let hidden = home.plugins().join(".demo.installing-x");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("plugin.toml"), MANIFEST).unwrap();
        assert!(super::super::plugin_folders(home.plugins()).is_empty());
        let (manifests, errors) = super::super::discover(&[home.plugins().to_path_buf()]);
        assert!(manifests.is_empty() && errors.is_empty());
    }

    /// A copy that fails part way leaves no partial folder, and an update
    /// that fails keeps the installed plugin.
    #[cfg(unix)]
    #[test]
    fn a_failed_copy_leaves_no_partial_folder() {
        use std::os::unix::fs::PermissionsExt;
        let home = Home::new();
        let v1 = home.folder(
            "v1",
            &[("plugin.toml", &manifest("1.0.0", "")), ("a.py", "1")],
        );
        let fail = |staged: &Staged| {
            let file = staged.root().join("z.py");
            fs::write(&file, "unreadable").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
            // Someone who can read it anyway (root) cannot run this test.
            fs::File::open(&file).is_err()
        };
        let staged = home.installer.prepare(&v1).unwrap();
        if !fail(&staged) {
            return;
        }
        let error = format!("{:#}", home.installer.commit(staged).unwrap_err());
        assert!(error.contains("Cannot copy"), "{error}");
        assert!(!home.plugins().join("demo").exists());
        home.assert_clean();

        let target = home
            .installer
            .commit(home.installer.prepare(&v1).unwrap())
            .unwrap();
        let v2 = home.folder("v2", &[("plugin.toml", &manifest("2.0.0", ""))]);
        let staged = home.installer.prepare(&v2).unwrap();
        assert!(fail(&staged));
        assert!(home.installer.commit(staged).is_err());
        assert_eq!(Manifest::load(&target).unwrap().plugin.version, "1.0.0");
        assert_eq!(fs::read_to_string(target.join("a.py")).unwrap(), "1");
        home.assert_clean();
    }
}

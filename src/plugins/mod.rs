//! Plugins: external programs that add actions, panes and file formats. See
//! `docs/PLUGINS.md` for the manifest and the protocol.
pub mod edits;
pub mod host;
pub mod install;
pub mod jobs;
pub mod manifest;
pub mod models;
pub mod protocol;
pub mod sandbox;
pub mod ui;

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub use manifest::Manifest;

pub const PATH_VARIABLE: &str = "XUAN_PLUGIN_PATH";

/// Names the bundled plugins folder when Xuan cannot find it from its own
/// executable. The AppImage's `AppRun` sets it: there the executable the
/// system reports is the dynamic loader that starts Xuan.
pub const BUNDLED_VARIABLE: &str = "XUAN_BUNDLED_PLUGINS";

/// What a grant stores instead of the folder for a bundled plugin: this,
/// joined with the plugin's folder name. No plugin folder has this path
/// (real grants are absolute, and Windows names cannot hold `<` or `>`), and
/// it stays the same when Xuan is upgraded, moved, or its AppImage is
/// mounted somewhere else.
pub const BUNDLED_GRANT_DIR: &str = "<bundled>";

/// A plugin folder that could not be loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadError {
    pub dir: PathBuf,
    pub error: String,
}

/// Where plugins are looked for: `XUAN_PLUGIN_PATH` entries first, then the
/// `plugins` folder next to the configuration file, then the plugins that
/// come with Xuan (`bundled`, see [`bundled_dir`]).
pub fn plugin_dirs(
    config_dir: Option<&Path>,
    path_variable: Option<&OsStr>,
    bundled: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path_variable
        .map(|value| {
            std::env::split_paths(value)
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default();
    if let Some(config_dir) = config_dir {
        dirs.push(config_dir.join("plugins"));
    }
    if let Some(bundled) = bundled {
        dirs.retain(|dir| !is_bundled(dir, Some(bundled)));
        dirs.push(bundled.to_path_buf());
    }
    dirs
}

/// The folder of plugins installed with Xuan: `XUAN_BUNDLED_PLUGINS` when it
/// is set, otherwise [`bundled_dir_for`] Xuan's executable. Canonicalized
/// when it exists. Plugins there still need the user's approval to run.
pub fn bundled_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        // Canonicalized so a symlink to the executable (in /usr/local/bin, say) still finds
        // the folder beside the real one. Not on Windows, where it adds a `\\?\` prefix.
        let exe = std::env::current_exe()
            .ok()
            .map(|exe| if cfg!(windows) { exe } else { canonical(&exe) });
        let dir = bundled_dir_from(
            std::env::var_os(BUNDLED_VARIABLE).as_deref(),
            exe.as_deref(),
        )?;
        Some(canonical(&dir))
    })
    .clone()
}

/// [`bundled_dir`] from the variable's value and the executable's path. An
/// empty or relative value is ignored.
pub fn bundled_dir_from(variable: Option<&OsStr>, exe: Option<&Path>) -> Option<PathBuf> {
    match variable.map(Path::new) {
        Some(dir) if dir.is_absolute() => Some(dir.to_path_buf()),
        _ => bundled_dir_for(exe?),
    }
}

/// Where the packages put the bundled plugins, from the executable's path:
/// `<prefix>/lib/xuan/plugins` for `<prefix>/bin/xuan` on Linux (`/usr` for
/// the deb and rpm, the tar archive's folder or its `install.sh` prefix, the
/// AppImage's `usr`), `plugins` next to `xuan.exe` on Windows, and
/// `Contents/Resources/plugins` for `Xuan.app/Contents/MacOS/xuan` on macOS.
pub fn bundled_dir_for(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if cfg!(windows) {
        return Some(bin.join("plugins"));
    }
    let parent = bin.parent()?;
    if cfg!(target_os = "macos")
        && bin.file_name() == Some(OsStr::new("MacOS"))
        && parent.file_name() == Some(OsStr::new("Contents"))
    {
        return Some(parent.join("Resources").join("plugins"));
    }
    Some(parent.join("lib").join("xuan").join("plugins"))
}

/// Folders added to the end of `PATH` on macOS, where they are missing: apps started
/// from Finder or the Dock get only the system's folders, so plugin interpreters from
/// Homebrew (`/opt/homebrew/bin` on Apple Silicon, `/usr/local/bin` on Intel) or the
/// python.org installer would not be found.
pub const MACOS_EXTRA_PATH: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// `existing` with each of `extra` appended unless it is already listed, or `None`
/// when nothing is missing or the folders cannot be joined into one value.
pub fn path_with(existing: Option<&OsStr>, extra: &[&str]) -> Option<std::ffi::OsString> {
    let mut dirs: Vec<PathBuf> = existing
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let missing: Vec<PathBuf> = extra
        .iter()
        .map(PathBuf::from)
        .filter(|dir| !dirs.contains(dir))
        .collect();
    if missing.is_empty() {
        return None;
    }
    dirs.extend(missing);
    std::env::join_paths(dirs).ok()
}

/// Names the folder of the Python plugin SDK (`xuan_plugin.py`). Xuan sets
/// it for every plugin it starts, with the folder first on `PYTHONPATH`;
/// set before Xuan starts, it replaces the folder Xuan would find itself.
pub const SDK_VARIABLE: &str = "XUAN_PLUGIN_SDK";

/// The Python SDK's module file, which a folder must hold to be the SDK's.
pub const SDK_MODULE: &str = "xuan_plugin.py";

/// The folder of the Python plugin SDK that matches this Xuan, see
/// [`sdk_dir_from`]. Canonicalized when it exists.
pub fn sdk_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("sdk")
            .join("python");
        let dir = sdk_dir_from(
            std::env::var_os(SDK_VARIABLE).as_deref(),
            bundled_dir().as_deref(),
            Some(&checkout),
        )?;
        Some(canonical(&dir))
    })
    .clone()
}

/// [`sdk_dir`] from `XUAN_PLUGIN_SDK`'s value, the bundled plugins folder
/// and the source checkout Xuan was built from. An absolute value of the
/// variable wins. Otherwise the SDK is `sdk/python` beside the bundled
/// plugins folder (`<prefix>/lib/xuan/sdk/python` on Linux, `sdk\python`
/// next to `xuan.exe` on Windows), or, in a development build run from its
/// checkout, the checkout's `sdk/python`; each only when it holds
/// [`SDK_MODULE`].
pub fn sdk_dir_from(
    variable: Option<&OsStr>,
    bundled: Option<&Path>,
    checkout: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(dir) = variable.map(Path::new).filter(|dir| dir.is_absolute()) {
        return Some(dir.to_path_buf());
    }
    let shipped = bundled
        .and_then(Path::parent)
        .map(|lib| lib.join("sdk").join("python"));
    shipped
        .into_iter()
        .chain(checkout.map(Path::to_path_buf))
        .find(|dir| dir.join(SDK_MODULE).is_file())
}

/// The environment that puts the SDK folder `sdk` on a plugin's import
/// path: `XUAN_PLUGIN_SDK` naming it, and `PYTHONPATH` with it first and the
/// `existing` value after it. Nothing without a folder; without
/// `PYTHONPATH` when the folder cannot be joined into one.
pub fn sdk_env(sdk: Option<&Path>, existing: Option<&OsStr>) -> Vec<(String, String)> {
    // A folder that is not Unicode would reach the plugin garbled.
    let Some(sdk) = sdk.and_then(|sdk| without_verbatim_prefix(sdk).to_str().map(str::to_owned))
    else {
        return Vec::new();
    };
    let mut env = vec![(SDK_VARIABLE.to_owned(), sdk.clone())];
    // A folder holding the separator cannot be one entry of the list, and
    // an existing value that is not Unicode is left as it is.
    let separator = if cfg!(windows) { ";" } else { ":" };
    if sdk.contains(separator) {
        return env;
    }
    let path = match existing.filter(|rest| !rest.is_empty()).map(OsStr::to_str) {
        None => Some(sdk),
        Some(Some(rest)) => Some(format!("{sdk}{separator}{rest}")),
        Some(None) => None,
    };
    env.extend(path.map(|path| ("PYTHONPATH".to_owned(), path)));
    env
}

/// `path` without the `\\?\` that canonicalizing adds on Windows before a
/// drive letter, which Python's imports do not expect.
fn without_verbatim_prefix(path: &Path) -> &Path {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => Path::new(rest),
        _ => path,
    }
}

/// Whether `dir` is the bundled plugins folder `bundled`.
pub fn is_bundled(dir: &Path, bundled: Option<&Path>) -> bool {
    bundled.is_some_and(|bundled| dir == bundled || canonical(dir) == canonical(bundled))
}

/// The folder a grant stores for the plugin folder `dir`: its canonical
/// path, or for a folder directly in the bundled plugins folder,
/// [`BUNDLED_GRANT_DIR`] joined with the folder's name.
pub fn grant_dir(dir: &Path, bundled: Option<&Path>) -> PathBuf {
    let dir = canonical(dir);
    match (dir.parent(), dir.file_name()) {
        (Some(parent), Some(name)) if is_bundled(parent, bundled) => {
            Path::new(BUNDLED_GRANT_DIR).join(name)
        }
        _ => dir,
    }
}

/// Whether a grant's folder is a bundled plugin's (see [`grant_dir`]).
pub fn is_bundled_grant_dir(dir: &Path) -> bool {
    dir.starts_with(BUNDLED_GRANT_DIR)
}

/// `path` canonicalized, or as it is when it does not exist.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The folder plugins keep their persistent data in.
pub fn data_dir(config_dir: &Path, plugin: &str) -> PathBuf {
    config_dir.join("plugin-data").join(plugin)
}

/// A new temporary folder only the user can open (0700 on Unix), for files
/// the host and a plugin exchange. Other local users cannot read the pixels
/// in it or plant files there.
pub fn private_dir(prefix: &str) -> std::io::Result<tempfile::TempDir> {
    private_dir_in(prefix, &std::env::temp_dir())
}

/// [`private_dir`] inside `parent`.
pub fn private_dir_in(prefix: &str, parent: &Path) -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(parent)
}

/// The plugin folders directly under `dir`: those holding a `plugin.toml`.
/// Hidden folders (a name starting with `.`) are skipped; the installer
/// builds a plugin in one before moving it into place.
pub fn plugin_folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .filter(|path| path.join(manifest::MANIFEST_FILE).is_file())
        .collect();
    folders.sort();
    folders
}

/// Why a plugin is not loaded, or not installed, when another folder has a
/// plugin with the same id.
pub fn conflict_message(id: &str) -> String {
    format!("another folder also has a plugin with the id `{id}`")
}

/// The `plugin.id` in a folder's manifest, read without validating the rest,
/// so a broken or incompatible plugin still counts as that plugin.
pub fn folder_id(folder: &Path) -> Option<String> {
    let text = std::fs::read_to_string(folder.join(manifest::MANIFEST_FILE)).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("plugin")?.get("id")?.as_str().map(str::to_owned)
}

/// Load every `*/plugin.toml` under the directories. Folders that share an
/// identifier are all reported as errors and none of them is loaded, so one
/// folder can never take over another's grant, settings or secrets.
///
/// The bundled folder (`bundled`, see [`bundled_dir`]) is the exception: a
/// plugin there is left out, without an error, when a folder in any other
/// directory has the same id, even one that does not load. A copy the user
/// installed replaces the one that came with Xuan.
pub fn discover(dirs: &[PathBuf], bundled: Option<&Path>) -> (Vec<Manifest>, Vec<LoadError>) {
    let (bundled_dirs, other_dirs): (Vec<&PathBuf>, Vec<&PathBuf>) =
        dirs.iter().partition(|dir| is_bundled(dir, bundled));
    let replaced: Vec<String> = other_dirs
        .iter()
        .flat_map(|dir| plugin_folders(dir))
        .filter_map(|folder| folder_id(&folder))
        .collect();
    let mut manifests: Vec<Manifest> = Vec::new();
    let mut errors = Vec::new();
    for dir in dirs {
        let in_bundled = bundled_dirs.contains(&dir);
        for folder in plugin_folders(dir) {
            if in_bundled && folder_id(&folder).is_some_and(|id| replaced.contains(&id)) {
                continue;
            }
            match Manifest::load(&folder) {
                Ok(manifest) => manifests.push(manifest),
                Err(error) => errors.push(LoadError {
                    dir: folder,
                    error: format!("{error:#}"),
                }),
            }
        }
    }
    let mut duplicates: Vec<String> = Vec::new();
    for (index, manifest) in manifests.iter().enumerate() {
        if manifests[..index]
            .iter()
            .any(|m| m.plugin.id == manifest.plugin.id)
            && !duplicates.contains(&manifest.plugin.id)
        {
            duplicates.push(manifest.plugin.id.clone());
        }
    }
    manifests.retain(|manifest| {
        if !duplicates.contains(&manifest.plugin.id) {
            return true;
        }
        errors.push(LoadError {
            dir: manifest.dir.clone(),
            error: format!(
                "{}; neither is loaded",
                conflict_message(&manifest.plugin.id)
            ),
        });
        false
    });
    manifests.sort_by(|a, b| {
        a.plugin
            .name
            .to_lowercase()
            .cmp(&b.plugin.name.to_lowercase())
    });
    (manifests, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str =
        "[plugin]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"1\"\ncommand = [\"true\"]\n";

    fn write(dir: &Path, folder: &str, id: &str, name: &str) {
        let path = dir.join(folder);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join(manifest::MANIFEST_FILE),
            MINIMAL.replace("{id}", id).replace("{name}", name),
        )
        .unwrap();
    }

    #[test]
    fn discovers_manifests_in_order_and_reports_broken_ones() {
        let user = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        write(user.path(), "b", "beta", "Beta");
        write(user.path(), "a", "alpha", "alpha");
        write(user.path(), "dup", "beta", "Beta again");
        write(extra.path(), "dev", "gamma", "Gamma");
        std::fs::create_dir_all(user.path().join("broken")).unwrap();
        std::fs::write(user.path().join("broken/plugin.toml"), "[plugin]\nid = 1\n").unwrap();
        std::fs::create_dir_all(user.path().join("not-a-plugin")).unwrap();
        let dirs = plugin_dirs(
            Some(user.path().parent().unwrap()),
            Some(OsStr::new(extra.path().to_str().unwrap())),
            None,
        );
        assert_eq!(dirs[0], extra.path());
        assert!(dirs[1].ends_with("plugins"));
        let dirs = vec![extra.path().to_path_buf(), user.path().to_path_buf()];
        let (manifests, errors) = discover(&dirs, None);
        let ids: Vec<_> = manifests.iter().map(|m| m.plugin.id.as_str()).collect();
        // Two folders claim `beta`: neither is loaded.
        assert_eq!(ids, ["alpha", "gamma"]);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors.iter().any(|e| e.dir.ends_with("broken")));
        for folder in ["b", "dup"] {
            assert!(
                errors
                    .iter()
                    .any(|e| e.dir.ends_with(folder) && e.error.contains("neither is loaded")),
                "{errors:?}"
            );
        }
        // The same id in a XUAN_PLUGIN_PATH folder cannot shadow a user plugin.
        write(extra.path(), "shadow", "alpha", "Alpha");
        let (manifests, _) = discover(&dirs, None);
        assert!(manifests.iter().all(|m| m.plugin.id != "alpha"));
        assert!(
            discover(&[PathBuf::from("/nonexistent/xuan")], None)
                .0
                .is_empty()
        );
        assert_eq!(plugin_dirs(None, None, None), Vec::<PathBuf>::new());
        assert_eq!(
            data_dir(Path::new("/c"), "p"),
            PathBuf::from("/c/plugin-data/p")
        );
    }

    #[test]
    fn the_bundled_folder_is_searched_last_and_found_from_the_executable() {
        let config = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        let dirs = plugin_dirs(
            Some(config.path()),
            Some(OsStr::new(extra.path().to_str().unwrap())),
            Some(bundled.path()),
        );
        assert_eq!(
            dirs,
            [
                extra.path().to_path_buf(),
                config.path().join("plugins"),
                bundled.path().to_path_buf()
            ]
        );
        // Named on XUAN_PLUGIN_PATH as well, it is still searched once, last.
        let both = std::env::join_paths([bundled.path(), extra.path()]).unwrap();
        let dirs = plugin_dirs(Some(config.path()), Some(&both), Some(bundled.path()));
        assert_eq!(
            dirs,
            [
                extra.path().to_path_buf(),
                config.path().join("plugins"),
                bundled.path().to_path_buf()
            ]
        );
        assert_eq!(
            plugin_dirs(None, None, Some(bundled.path())),
            [bundled.path()]
        );

        if cfg!(windows) {
            assert_eq!(
                bundled_dir_for(Path::new(r"C:\Apps\Xuan\xuan.exe")),
                Some(PathBuf::from(r"C:\Apps\Xuan\plugins"))
            );
        } else {
            for (exe, dir) in [
                ("/usr/bin/xuan", "/usr/lib/xuan/plugins"),
                ("/home/u/.local/bin/xuan", "/home/u/.local/lib/xuan/plugins"),
                (
                    "/tmp/.mount_XuanAb12/usr/bin/xuan",
                    "/tmp/.mount_XuanAb12/usr/lib/xuan/plugins",
                ),
            ] {
                assert_eq!(bundled_dir_for(Path::new(exe)), Some(PathBuf::from(dir)));
            }
            // In the app bundle on macOS; a `bin/xuan` layout elsewhere on the Mac.
            let app = "/Applications/Xuan.app/Contents/MacOS/xuan";
            let resources = if cfg!(target_os = "macos") {
                "/Applications/Xuan.app/Contents/Resources/plugins"
            } else {
                "/Applications/Xuan.app/Contents/lib/xuan/plugins"
            };
            assert_eq!(
                bundled_dir_for(Path::new(app)),
                Some(PathBuf::from(resources))
            );
        }
        let exe = config.path().join("bin").join("xuan");
        // The variable wins when it is an absolute path; otherwise it is ignored.
        assert_eq!(
            bundled_dir_from(Some(bundled.path().as_os_str()), Some(&exe)),
            Some(bundled.path().to_path_buf())
        );
        for ignored in ["", "relative/plugins"] {
            assert_eq!(
                bundled_dir_from(Some(OsStr::new(ignored)), Some(&exe)),
                bundled_dir_for(&exe)
            );
        }
        assert_eq!(bundled_dir_from(None, None), None);
    }

    #[test]
    fn the_python_sdk_is_found_beside_the_bundled_plugins_or_in_the_checkout() {
        let prefix = tempfile::tempdir().unwrap();
        let bundled = prefix.path().join("lib").join("xuan").join("plugins");
        let shipped = prefix
            .path()
            .join("lib")
            .join("xuan")
            .join("sdk")
            .join("python");
        let checkout = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(&bundled).unwrap();
        // Neither holds the SDK yet.
        std::fs::create_dir_all(&shipped).unwrap();
        assert_eq!(
            sdk_dir_from(None, Some(&bundled), Some(checkout.path())),
            None
        );
        assert_eq!(sdk_dir_from(None, None, None), None);
        // The checkout's is the fallback for a development build.
        std::fs::write(checkout.path().join(SDK_MODULE), "").unwrap();
        assert_eq!(
            sdk_dir_from(None, Some(&bundled), Some(checkout.path())),
            Some(checkout.path().to_path_buf())
        );
        // The one installed with Xuan wins over it.
        std::fs::write(shipped.join(SDK_MODULE), "").unwrap();
        assert_eq!(
            sdk_dir_from(None, Some(&bundled), Some(checkout.path())),
            Some(shipped.clone())
        );
        // The variable wins when it is an absolute path; otherwise it is ignored.
        let chosen = tempfile::tempdir().unwrap();
        assert_eq!(
            sdk_dir_from(Some(chosen.path().as_os_str()), Some(&bundled), None),
            Some(chosen.path().to_path_buf())
        );
        for ignored in ["", "relative/sdk"] {
            assert_eq!(
                sdk_dir_from(Some(OsStr::new(ignored)), Some(&bundled), None),
                Some(shipped.clone())
            );
        }
    }

    #[test]
    fn the_sdk_goes_first_on_the_python_path_and_keeps_the_rest() {
        let sdk = tempfile::tempdir().unwrap();
        let sdk_text = sdk.path().to_str().unwrap().to_owned();
        let separator = if cfg!(windows) { ";" } else { ":" };
        let other = std::env::temp_dir().join("elsewhere");
        let existing = format!("{}{separator}{}", other.display(), other.display());
        let env = sdk_env(Some(sdk.path()), Some(OsStr::new(&existing)));
        assert_eq!(
            env,
            [
                (SDK_VARIABLE.to_owned(), sdk_text.clone()),
                (
                    "PYTHONPATH".to_owned(),
                    format!("{sdk_text}{separator}{existing}")
                ),
            ]
        );
        // The split value names the SDK first.
        let python_path = &env[1].1;
        assert_eq!(
            std::env::split_paths(python_path).next(),
            Some(sdk.path().to_path_buf())
        );
        // Without an existing value it is the SDK alone.
        for existing in [None, Some(OsStr::new(""))] {
            assert_eq!(
                sdk_env(Some(sdk.path()), existing),
                [
                    (SDK_VARIABLE.to_owned(), sdk_text.clone()),
                    ("PYTHONPATH".to_owned(), sdk_text.clone()),
                ]
            );
        }
        // Without an SDK neither is set.
        assert!(sdk_env(None, Some(OsStr::new(&existing))).is_empty());
        // A folder whose name holds the separator only gets the variable.
        let odd = sdk.path().join(format!("a{separator}b"));
        assert_eq!(
            sdk_env(Some(&odd), None),
            [(SDK_VARIABLE.to_owned(), odd.to_str().unwrap().to_owned())]
        );
        assert_eq!(
            without_verbatim_prefix(Path::new(r"\\?\C:\Program Files\Xuan\sdk\python")),
            Path::new(r"C:\Program Files\Xuan\sdk\python")
        );
        assert_eq!(
            without_verbatim_prefix(Path::new(r"\\?\UNC\server\share")),
            Path::new(r"\\?\UNC\server\share")
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_homebrew_folders_are_appended_to_the_path() {
        let finder = OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin");
        assert_eq!(
            path_with(Some(finder), &MACOS_EXTRA_PATH).unwrap(),
            "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin"
        );
        // A folder already listed keeps its place, and nothing changes when none is missing.
        let shell = OsStr::new("/usr/local/bin:/usr/bin:/opt/homebrew/bin");
        assert_eq!(path_with(Some(shell), &MACOS_EXTRA_PATH), None);
        assert_eq!(
            path_with(
                Some(OsStr::new("/usr/local/bin:/usr/bin")),
                &MACOS_EXTRA_PATH
            )
            .unwrap(),
            "/usr/local/bin:/usr/bin:/opt/homebrew/bin"
        );
        assert_eq!(
            path_with(None, &MACOS_EXTRA_PATH).unwrap(),
            "/opt/homebrew/bin:/usr/local/bin"
        );
    }

    #[test]
    fn a_plugin_elsewhere_replaces_the_bundled_one_with_the_same_id() {
        let user = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        write(bundled.path(), "mcp-server", "mcp-server", "MCP Server");
        write(bundled.path(), "other", "other", "Other");
        let dirs = plugin_dirs(None, None, Some(bundled.path()));
        let dirs = [vec![user.path().to_path_buf()], dirs].concat();
        let (manifests, errors) = discover(&dirs, Some(bundled.path()));
        assert!(errors.is_empty(), "{errors:?}");
        let dirs_of = |manifests: &[Manifest]| -> Vec<PathBuf> {
            manifests.iter().map(|m| m.dir.clone()).collect()
        };
        assert_eq!(
            dirs_of(&manifests),
            [
                bundled.path().join("mcp-server"),
                bundled.path().join("other")
            ]
        );

        // The user's copy loads instead of the bundled one, without an error.
        write(user.path(), "my-mcp", "mcp-server", "MCP Server (mine)");
        let (manifests, errors) = discover(&dirs, Some(bundled.path()));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            dirs_of(&manifests),
            [user.path().join("my-mcp"), bundled.path().join("other")]
        );

        // A user copy that does not load still replaces it: the error is
        // the user's, and the bundled copy does not quietly take its place.
        std::fs::write(
            user.path().join("my-mcp/plugin.toml"),
            "[plugin]\nid = \"mcp-server\"\nname = 3\n",
        )
        .unwrap();
        let (manifests, errors) = discover(&dirs, Some(bundled.path()));
        assert_eq!(dirs_of(&manifests), [bundled.path().join("other")]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].dir, user.path().join("my-mcp"));

        // So does a XUAN_PLUGIN_PATH copy.
        std::fs::remove_dir_all(user.path().join("my-mcp")).unwrap();
        write(extra.path(), "dev", "other", "Other (dev)");
        let dirs = plugin_dirs(None, Some(extra.path().as_os_str()), Some(bundled.path()));
        let (manifests, errors) = discover(&dirs, Some(bundled.path()));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            dirs_of(&manifests),
            [bundled.path().join("mcp-server"), extra.path().join("dev")]
        );

        // Two bundled folders with one id are still a conflict.
        write(bundled.path(), "mcp-copy", "mcp-server", "MCP Server");
        let (manifests, errors) = discover(&dirs, Some(bundled.path()));
        assert_eq!(dirs_of(&manifests), [extra.path().join("dev")]);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().all(|e| e.error.contains("neither is loaded")));

        // Without a bundled folder, the same folders conflict as before.
        let (_, errors) = discover(&dirs, None);
        assert_eq!(errors.len(), 4, "{errors:?}");
    }

    #[test]
    fn grants_name_a_bundled_plugin_by_its_folder_name() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        for root in [first.path(), second.path(), user.path()] {
            write(root, "mcp-server", "mcp-server", "MCP Server");
        }
        std::fs::create_dir_all(first.path().join("mcp-server/nested")).unwrap();
        let marker = Path::new(BUNDLED_GRANT_DIR).join("mcp-server");
        // Another install or AppImage mount point keeps the grant's folder.
        for bundled in [first.path(), second.path()] {
            let dir = grant_dir(&bundled.join("mcp-server"), Some(bundled));
            assert_eq!(dir, marker);
            assert!(is_bundled_grant_dir(&dir));
        }
        // A folder that is not directly in the bundled folder keeps its path.
        let user_dir = std::fs::canonicalize(user.path().join("mcp-server")).unwrap();
        assert_eq!(
            grant_dir(&user.path().join("mcp-server"), Some(first.path())),
            user_dir
        );
        assert!(!is_bundled_grant_dir(&user_dir));
        assert_eq!(grant_dir(&user.path().join("mcp-server"), None), user_dir);
        assert_eq!(
            grant_dir(&first.path().join("mcp-server/nested"), Some(first.path())),
            std::fs::canonicalize(first.path().join("mcp-server/nested")).unwrap()
        );
        assert_ne!(
            grant_dir(&first.path().join("mcp-server"), None),
            marker,
            "only the bundled folder counts"
        );
    }
}

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
        let exe = std::env::current_exe().ok();
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
/// AppImage's `usr`), and `plugins` next to `xuan.exe` on Windows.
pub fn bundled_dir_for(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    if cfg!(windows) {
        Some(bin.join("plugins"))
    } else {
        Some(bin.parent()?.join("lib").join("xuan").join("plugins"))
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

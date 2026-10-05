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

/// A plugin folder that could not be loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadError {
    pub dir: PathBuf,
    pub error: String,
}

/// Where plugins are looked for: `XUAN_PLUGIN_PATH` entries first, then the
/// `plugins` folder next to the configuration file.
pub fn plugin_dirs(config_dir: Option<&Path>, path_variable: Option<&OsStr>) -> Vec<PathBuf> {
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
    dirs
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

/// Load every `*/plugin.toml` under the directories. Folders that share an
/// identifier are all reported as errors and none of them is loaded, so one
/// folder can never take over another's grant, settings or secrets.
pub fn discover(dirs: &[PathBuf]) -> (Vec<Manifest>, Vec<LoadError>) {
    let mut manifests: Vec<Manifest> = Vec::new();
    let mut errors = Vec::new();
    for dir in dirs {
        for folder in plugin_folders(dir) {
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
        );
        assert_eq!(dirs[0], extra.path());
        assert!(dirs[1].ends_with("plugins"));
        let dirs = vec![extra.path().to_path_buf(), user.path().to_path_buf()];
        let (manifests, errors) = discover(&dirs);
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
        let (manifests, _) = discover(&dirs);
        assert!(manifests.iter().all(|m| m.plugin.id != "alpha"));
        assert!(discover(&[PathBuf::from("/nonexistent/xuan")]).0.is_empty());
        assert_eq!(plugin_dirs(None, None), Vec::<PathBuf>::new());
        assert_eq!(
            data_dir(Path::new("/c"), "p"),
            PathBuf::from("/c/plugin-data/p")
        );
    }
}

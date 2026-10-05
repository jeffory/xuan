//! User preferences, independent of projects and egui's window persistence.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
}

impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::SimplifiedChinese => "简体中文",
        }
    }
}

/// How the main window draws its title bar and window controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TitleBar {
    /// Native decorations from the window manager; the menu bar is a normal panel.
    #[serde(rename = "system")]
    System,
    /// Client-side title bar holding the menus, with monochrome controls.
    #[serde(rename = "compact")]
    Compact,
    /// Client-side title bar with macOS traffic-light controls on the left.
    #[serde(rename = "macos")]
    MacOs,
}

impl Default for TitleBar {
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Compact
        }
    }
}

impl TitleBar {
    pub const ALL: [Self; 3] = [Self::System, Self::Compact, Self::MacOs];

    /// The app draws the title bar itself, with system decorations turned off.
    pub fn client_side(self) -> bool {
        self != Self::System
    }

    /// Untranslated display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Compact => "Compact",
            Self::MacOs => "macOS",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub language: Language,
    pub title_bar: TitleBar,
    /// Right sidebar arrangement; see [`crate::panes`].
    pub panes: crate::panes::Layout,
    /// Per-plugin state keyed by plugin identifier.
    pub plugins: BTreeMap<String, PluginConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    pub enabled: bool,
    /// The user accepted the permissions the manifest declared.
    pub granted: bool,
    /// Values for the settings the manifest declares, by setting identifier.
    pub settings: toml::Table,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            granted: false,
            settings: toml::Table::new(),
        }
    }
}

/// Plugin secrets such as API keys, kept out of `config.toml` in a file that
/// only the owner can read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secrets(pub BTreeMap<String, BTreeMap<String, String>>);

impl Secrets {
    pub fn path() -> Result<PathBuf> {
        Ok(Config::path()?.with_file_name("secrets.toml"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("Cannot parse {}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("Cannot read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("Secrets path has no parent")?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(toml::to_string_pretty(&self.0)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)
            .with_context(|| format!("Cannot save {}", path.display()))?;
        Ok(())
    }

    pub fn get(&self, plugin: &str, key: &str) -> Option<&str> {
        self.0.get(plugin)?.get(key).map(String::as_str)
    }

    pub fn set(&mut self, plugin: &str, key: &str, value: &str) {
        if value.is_empty() {
            if let Some(map) = self.0.get_mut(plugin) {
                map.remove(key);
                if map.is_empty() {
                    self.0.remove(plugin);
                }
            }
        } else {
            self.0
                .entry(plugin.into())
                .or_default()
                .insert(key.into(), value.into());
        }
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let variable = if cfg!(windows) {
            "APPDATA"
        } else {
            "XDG_CONFIG_HOME"
        };
        config_path(
            std::env::var_os(variable).map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }

    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut config: Self = toml::from_str(&text)
                    .with_context(|| format!("Cannot parse {}", path.display()))?;
                config.panes.sanitize();
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("Cannot read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        // Preserve unknown preferences. An invalid existing file is never overwritten.
        let mut table = match fs::read_to_string(path) {
            Ok(text) => toml::from_str::<toml::Table>(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            Err(error) => return Err(error.into()),
        };
        if let toml::Value::Table(own) = toml::Value::try_from(self)? {
            table.extend(own);
        }
        let parent = path.parent().context("Configuration path has no parent")?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(toml::to_string_pretty(&table)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)
            .with_context(|| format!("Cannot save {}", path.display()))?;
        Ok(())
    }
}

fn config_path(config_home: Option<PathBuf>, home: Option<PathBuf>) -> Result<PathBuf> {
    // The XDG specification requires absolute paths; ignore empty/relative values.
    if let Some(path) = config_home.filter(|p| p.is_absolute()) {
        return Ok(path.join("xuan/config.toml"));
    }
    if !cfg!(windows)
        && let Some(path) = home.filter(|p| p.is_absolute())
    {
        return Ok(path.join(".config/xuan/config.toml"));
    }
    bail!("Cannot locate the user configuration directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_roundtrip_preserves_unknown_options() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("xuan/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let chinese = Config {
            language: Language::SimplifiedChinese,
            title_bar: TitleBar::MacOs,
            ..Config::default()
        };
        chinese.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), chinese);
        fs::write(&path, "language = 'zh-CN'\n[future]\noption = 42\n").unwrap();
        Config::default().save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("option = 42"));
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn invalid_preferences_are_reported_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "language = [broken").unwrap();
        assert!(Config::load(&path).is_err());
        assert!(Config::default().save(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "language = [broken");
        assert!(toml::from_str::<Config>("language = 'unknown'").is_err());
    }

    #[test]
    fn settings_from_older_releases_load_with_the_default_title_bar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "language = 'zh-CN'\n").unwrap();
        assert_eq!(
            Config::load(&path).unwrap(),
            Config {
                language: Language::SimplifiedChinese,
                title_bar: TitleBar::default(),
                ..Config::default()
            }
        );
    }

    #[test]
    fn title_bar_style_persists_and_defaults_to_compact() {
        let config: Config = toml::from_str("language = 'en'").unwrap();
        assert_eq!(config.title_bar, TitleBar::default());
        if !cfg!(target_os = "macos") {
            assert_eq!(TitleBar::default(), TitleBar::Compact);
        }
        for style in TitleBar::ALL {
            let config = Config {
                title_bar: style,
                ..Config::default()
            };
            let text = toml::to_string(&config).unwrap();
            assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
        }
        let config: Config = toml::from_str("title_bar = 'system'").unwrap();
        assert_eq!(config.title_bar, TitleBar::System);
        assert!(!TitleBar::System.client_side());
        assert!(TitleBar::Compact.client_side() && TitleBar::MacOs.client_side());
    }

    #[test]
    fn secrets_are_stored_separately_with_owner_only_access() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("xuan/secrets.toml");
        assert_eq!(Secrets::load(&path).unwrap(), Secrets::default());
        let mut secrets = Secrets::default();
        secrets.set("comfy", "api_key", "sk-123");
        secrets.save(&path).unwrap();
        assert_eq!(
            Secrets::load(&path).unwrap().get("comfy", "api_key"),
            Some("sk-123")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        secrets.set("comfy", "api_key", "");
        assert!(secrets.0.is_empty());
        let mut config = Config::default();
        config
            .plugins
            .entry("comfy".into())
            .or_default()
            .settings
            .insert("max_side".into(), toml::Value::Integer(1024));
        let config_path = dir.path().join("xuan/config.toml");
        config.save(&config_path).unwrap();
        assert_eq!(Config::load(&config_path).unwrap(), config);
        assert!(
            fs::read_to_string(&config_path)
                .unwrap()
                .contains("[plugins.comfy.settings]")
        );
    }

    #[test]
    fn uses_platform_config_home() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_path_buf();
        assert_eq!(
            config_path(Some(base.clone()), None).unwrap(),
            base.join("xuan/config.toml")
        );
        if !cfg!(windows) {
            assert_eq!(
                config_path(Some("relative".into()), Some(base.clone())).unwrap(),
                base.join(".config/xuan/config.toml")
            );
        }
        assert!(config_path(None, None).is_err());
    }
}

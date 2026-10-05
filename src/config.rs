//! User preferences, independent of projects and egui's window persistence.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::layout::{GridSettings, SnapSettings};

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

/// Lowest and highest zoom, in percent, at which the pixel grid may start to show.
pub const PIXEL_GRID_PERCENT_RANGE: std::ops::RangeInclusive<u32> = 200..=6400;
pub const DEFAULT_PIXEL_GRID_PERCENT: u32 = 500;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub language: Language,
    pub title_bar: TitleBar,
    /// View → Pixel Grid.
    pub pixel_grid: bool,
    /// Zoom, in percent, from which the pixel grid is drawn.
    pub pixel_grid_percent: u32,
    /// View → Rulers.
    pub rulers: bool,
    /// View → Show → Grid: the layout grid, separate from the pixel grid.
    pub show_grid: bool,
    /// View → Show → Guides.
    pub show_guides: bool,
    /// View → Lock Guides.
    pub lock_guides: bool,
    /// View → Snap and View → Snap To.
    pub snap: SnapSettings,
    /// The layout grid for projects without one of their own (View → Grid Settings…).
    pub grid: GridSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: Language::default(),
            title_bar: TitleBar::default(),
            pixel_grid: true,
            pixel_grid_percent: DEFAULT_PIXEL_GRID_PERCENT,
            // Upstream's defaults: rulers and grid hidden, guides shown and unlocked.
            rulers: false,
            show_grid: false,
            show_guides: true,
            lock_guides: false,
            snap: SnapSettings::default(),
            grid: GridSettings::default(),
        }
    }
}

impl Config {
    /// The grid threshold, forced into the supported range even for hand-edited files.
    pub fn pixel_grid_percent(&self) -> u32 {
        self.pixel_grid_percent.clamp(
            *PIXEL_GRID_PERCENT_RANGE.start(),
            *PIXEL_GRID_PERCENT_RANGE.end(),
        )
    }

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
                toml::from_str(&text).with_context(|| format!("Cannot parse {}", path.display()))
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
        table.insert("language".into(), toml::Value::try_from(self.language)?);
        table.insert("title_bar".into(), toml::Value::try_from(self.title_bar)?);
        table.insert("pixel_grid".into(), toml::Value::Boolean(self.pixel_grid));
        table.insert(
            "pixel_grid_percent".into(),
            toml::Value::Integer(self.pixel_grid_percent().into()),
        );
        table.insert("rulers".into(), toml::Value::Boolean(self.rulers));
        table.insert("show_grid".into(), toml::Value::Boolean(self.show_grid));
        table.insert("show_guides".into(), toml::Value::Boolean(self.show_guides));
        table.insert("lock_guides".into(), toml::Value::Boolean(self.lock_guides));
        table.insert("snap".into(), toml::Value::try_from(self.snap)?);
        table.insert(
            "grid".into(),
            toml::Value::try_from(self.grid.normalized())?,
        );
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
            pixel_grid: false,
            pixel_grid_percent: 1200,
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
    fn pixel_grid_defaults_persist_and_older_files_still_load() {
        let defaults = Config::default();
        assert!(defaults.pixel_grid);
        assert_eq!(defaults.pixel_grid_percent, 500);
        // Files from before the pixel grid existed.
        let old: Config = toml::from_str("language = 'zh-CN'\ntitle_bar = 'system'\n").unwrap();
        assert_eq!(old.title_bar, TitleBar::System);
        assert!(old.pixel_grid);
        assert_eq!(old.pixel_grid_percent(), 500);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "language = 'en'\n").unwrap();
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        // A partial new-style file keeps the other default.
        let partial: Config = toml::from_str("pixel_grid = false").unwrap();
        assert!(!partial.pixel_grid && partial.pixel_grid_percent == 500);

        let custom = Config {
            pixel_grid: false,
            pixel_grid_percent: 800,
            ..Config::default()
        };
        custom.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), custom);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("pixel_grid = false") && text.contains("pixel_grid_percent = 800"));
    }

    #[test]
    fn pixel_grid_threshold_is_clamped_to_the_supported_range() {
        for (stored, effective) in [
            (0, 200),
            (199, 200),
            (200, 200),
            (6400, 6400),
            (99999, 6400),
        ] {
            let config = Config {
                pixel_grid_percent: stored,
                ..Config::default()
            };
            assert_eq!(config.pixel_grid_percent(), effective);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config {
            pixel_grid_percent: 50,
            ..Config::default()
        }
        .save(&path)
        .unwrap();
        assert_eq!(Config::load(&path).unwrap().pixel_grid_percent, 200);
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

    #[test]
    fn view_aids_default_like_upstream_and_older_files_still_load() {
        let defaults = Config::default();
        assert!(!defaults.rulers && !defaults.show_grid && !defaults.lock_guides);
        assert!(defaults.show_guides);
        assert_eq!(defaults.snap, SnapSettings::default());
        assert!(defaults.snap.enabled && defaults.snap.guides && !defaults.snap.grid);
        assert!(defaults.snap.layers && defaults.snap.bounds);
        assert_eq!(defaults.grid, GridSettings::default());

        // A file written before rulers, guides and the layout grid existed.
        let old: Config =
            toml::from_str("language = 'zh-CN'\ntitle_bar = 'system'\npixel_grid = false\n")
                .unwrap();
        assert_eq!(
            old,
            Config {
                language: Language::SimplifiedChinese,
                title_bar: TitleBar::System,
                pixel_grid: false,
                ..Config::default()
            }
        );
        // Partial tables keep the other defaults.
        let partial: Config =
            toml::from_str("[snap]\ngrid = true\n[grid]\nspacing = 100\n").unwrap();
        assert!(partial.snap.grid && partial.snap.enabled && partial.snap.layers);
        assert_eq!(partial.grid.spacing, 100);
        assert_eq!(partial.grid.subdivisions, 8);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let custom = Config {
            rulers: true,
            show_grid: true,
            show_guides: false,
            lock_guides: true,
            snap: SnapSettings {
                enabled: false,
                guides: false,
                grid: true,
                layers: false,
                bounds: false,
            },
            grid: GridSettings {
                spacing: 32,
                subdivisions: 2,
                color: crate::layout::GridColor::Cyan,
                style: crate::layout::GridStyle::Dots,
                opacity: 80,
                ..GridSettings::default()
            },
            ..Config::default()
        };
        custom.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), custom);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("rulers = true") && text.contains("[snap]"));
        assert!(text.contains("[grid]") && text.contains("style = \"dots\""));
    }
}

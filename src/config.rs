//! User preferences, independent of projects and egui's window persistence.
use std::{
    collections::BTreeMap,
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

/// Where a Compact title bar gets its minimize, maximize and close buttons from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowButtons {
    /// The images of the desktop's own theme, when it has any (Linux).
    #[serde(rename = "theme")]
    Theme,
    /// The glyphs drawn by Xuan.
    #[serde(rename = "builtin")]
    BuiltIn,
}

impl Default for WindowButtons {
    fn default() -> Self {
        if cfg!(target_os = "linux") {
            Self::Theme
        } else {
            Self::BuiltIn
        }
    }
}

impl WindowButtons {
    pub const ALL: [Self; 2] = [Self::Theme, Self::BuiltIn];

    /// Untranslated display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Theme => "Match desktop theme",
            Self::BuiltIn => "Built-in",
        }
    }
}

/// Light or dark interface colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    /// Follow the desktop's light or dark preference; dark when it states none.
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// Untranslated display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light mode",
            Self::Dark => "Dark mode",
        }
    }
}

/// Lowest and highest zoom, in percent, at which the pixel grid may start to show.
pub const PIXEL_GRID_PERCENT_RANGE: std::ops::RangeInclusive<u32> = 200..=6400;
pub const DEFAULT_PIXEL_GRID_PERCENT: u32 = 500;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub language: Language,
    /// Settings → Appearance → Theme.
    pub theme: Theme,
    /// Settings → Appearance → "Use system accent colour": highlights take the desktop's
    /// accent instead of Xuan's blue, when the desktop has one.
    pub system_accent: bool,
    pub title_bar: TitleBar,
    /// Where Compact's window buttons come from.
    pub window_buttons: WindowButtons,
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
    /// Right sidebar arrangement; see [`crate::panes`].
    pub panes: crate::panes::Layout,
    /// Per-plugin state keyed by plugin identifier.
    pub plugins: BTreeMap<String, PluginConfig>,
    /// Key bindings the user changed, by command id: `merge = "Ctrl+E"`, `""` for none, or a
    /// list of shortcuts. Commands not listed keep their defaults, including new defaults of
    /// later releases. The editor interprets the values and ignores ones it does not know.
    pub keybindings: toml::Table,
    /// Command ids run from the command palette, most recent first.
    pub recent_commands: Vec<String>,
    /// "Disable plugins that use the network": plugins whose manifest declares
    /// network hosts do not start and their actions are unavailable.
    #[serde(default)]
    pub disable_network_plugins: bool,
    /// "Block network for plugins that don't declare it": on Linux, plugins
    /// whose manifest declares no network hosts start under a seccomp filter
    /// that keeps them from opening network sockets (see
    /// [`crate::plugins::sandbox`]). `None` until the user changes it, so it
    /// follows [`BLOCK_UNDECLARED_NETWORK_DEFAULT`]; read it with
    /// [`Config::block_undeclared_network`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_undeclared_network: Option<bool>,
    /// Settings → Selection: which plugin, if any, stands in for Select Subject, Remove
    /// Background and the Magic tool's Object mode. Built-in when absent.
    #[serde(default, skip_serializing_if = "Providers::is_default")]
    pub providers: Providers,
}

/// The plugin chosen for each replaceable algorithm, by plugin id; `None` is the
/// built-in one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select_subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_select: Option<String>,
}

impl Providers {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The plugin chosen for `capability`.
    pub fn get(&self, capability: crate::plugins::manifest::Capability) -> Option<&str> {
        use crate::plugins::manifest::Capability;
        match capability {
            Capability::SelectSubject => self.select_subject.as_deref(),
            Capability::RemoveBackground => self.remove_background.as_deref(),
            Capability::ObjectSelect => self.object_select.as_deref(),
        }
    }

    pub fn set(
        &mut self,
        capability: crate::plugins::manifest::Capability,
        plugin: Option<String>,
    ) {
        use crate::plugins::manifest::Capability;
        *match capability {
            Capability::SelectSubject => &mut self.select_subject,
            Capability::RemoveBackground => &mut self.remove_background,
            Capability::ObjectSelect => &mut self.object_select,
        } = plugin;
    }
}

/// "Block network for plugins that don't declare it" for users who never
/// changed it. Off while the setting is opt-in. Making it the default is a
/// matter of setting this to `true`: configurations only store the setting
/// once the user changes it, so everyone else gets the new default.
pub const BLOCK_UNDECLARED_NETWORK_DEFAULT: bool = false;

impl Default for Config {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme: Theme::default(),
            system_accent: true,
            title_bar: TitleBar::default(),
            window_buttons: WindowButtons::default(),
            pixel_grid: true,
            pixel_grid_percent: DEFAULT_PIXEL_GRID_PERCENT,
            // Upstream's defaults: rulers and grid hidden, guides shown and unlocked.
            rulers: false,
            show_grid: false,
            show_guides: true,
            lock_guides: false,
            snap: SnapSettings::default(),
            grid: GridSettings::default(),
            panes: crate::panes::Layout::default(),
            plugins: BTreeMap::new(),
            keybindings: toml::Table::new(),
            recent_commands: Vec::new(),
            disable_network_plugins: false,
            block_undeclared_network: None,
            providers: Providers::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    pub enabled: bool,
    /// What the user allowed to run. A plugin whose folder, command or
    /// permissions no longer match runs only after the user reviews it again.
    /// Configurations from before grants were recorded (`granted = true`)
    /// have none, so those plugins are reviewed again too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant: Option<PluginGrant>,
    /// Values for the settings the manifest declares, by setting identifier.
    pub settings: toml::Table,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            grant: None,
            settings: toml::Table::new(),
        }
    }
}

/// A plugin the user allowed to run, exactly as they reviewed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginGrant {
    /// The plugin folder, canonicalized.
    pub dir: PathBuf,
    pub command: Vec<String>,
    pub permissions: crate::plugins::manifest::Permissions,
    /// "Don't ask again for this plugin" in the prompt shown before document
    /// data goes to a plugin that declares network hosts. It belongs to this
    /// grant: a plugin whose folder, command or permissions change is
    /// reviewed again and starts without it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub send_without_asking: bool,
    /// Auto mode for a plugin that declares `edit_prompt = "session"`: its
    /// direct edits no longer wait for the user to allow each session. It
    /// belongs to this grant like `send_without_asking`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub edit_without_asking: bool,
    /// "Always allow" in the prompt shown when a plugin asks to save or
    /// export to a path it names (`file/save_as`, `file/export` or
    /// `file/save` with a `path`): its writes no longer wait for the user,
    /// except to replace a file Xuan did not write in this run. It belongs
    /// to this grant like `send_without_asking`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub save_without_asking: bool,
}

impl PluginGrant {
    /// Whether both grants allow the same plugin: the same folder, command
    /// and permissions. The answers stored with a grant do not count.
    pub fn covers(&self, other: &Self) -> bool {
        self.dir == other.dir
            && self.command == other.command
            && self.permissions == other.permissions
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
            // The parser's message quotes the offending line, which holds a
            // secret; only its position is reported.
            Ok(text) => toml::from_str(&text).map_err(|error| {
                let line = error
                    .span()
                    .map(|span| text.as_bytes()[..span.start.min(text.len())]
                        .iter()
                        .filter(|&&byte| byte == b'\n')
                        .count()
                        + 1);
                match line {
                    Some(line) => anyhow::anyhow!(
                        "Cannot parse {}: invalid TOML on line {line} (not shown, as it may hold a secret)",
                        path.display()
                    ),
                    None => anyhow::anyhow!("Cannot parse {}: invalid TOML", path.display()),
                }
            }),
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

    /// Forget every secret of a plugin.
    pub fn clear(&mut self, plugin: &str) -> bool {
        self.0.remove(plugin).is_some()
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
    /// The grid threshold, forced into the supported range even for hand-edited files.
    pub fn pixel_grid_percent(&self) -> u32 {
        self.pixel_grid_percent.clamp(
            *PIXEL_GRID_PERCENT_RANGE.start(),
            *PIXEL_GRID_PERCENT_RANGE.end(),
        )
    }

    /// "Block network for plugins that don't declare it", with the release
    /// default for users who never changed it.
    pub fn block_undeclared_network(&self) -> bool {
        self.block_undeclared_network
            .unwrap_or(BLOCK_UNDECLARED_NETWORK_DEFAULT)
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
        table.insert("language".into(), toml::Value::try_from(self.language)?);
        table.insert("theme".into(), toml::Value::try_from(self.theme)?);
        table.insert(
            "system_accent".into(),
            toml::Value::Boolean(self.system_accent),
        );
        table.insert("title_bar".into(), toml::Value::try_from(self.title_bar)?);
        table.insert(
            "window_buttons".into(),
            toml::Value::try_from(self.window_buttons)?,
        );
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
        table.insert("panes".into(), toml::Value::try_from(&self.panes)?);
        table.insert("plugins".into(), toml::Value::try_from(&self.plugins)?);
        table.insert(
            "disable_network_plugins".into(),
            toml::Value::Boolean(self.disable_network_plugins),
        );
        match self.block_undeclared_network {
            Some(block) => {
                table.insert(
                    "block_undeclared_network".into(),
                    toml::Value::Boolean(block),
                );
            }
            None => {
                table.remove("block_undeclared_network");
            }
        }
        if self.keybindings.is_empty() {
            table.remove("keybindings");
        } else {
            table.insert(
                "keybindings".into(),
                toml::Value::Table(self.keybindings.clone()),
            );
        }
        if self.providers.is_default() {
            table.remove("providers");
        } else {
            table.insert("providers".into(), toml::Value::try_from(&self.providers)?);
        }
        if self.recent_commands.is_empty() {
            table.remove("recent_commands");
        } else {
            table.insert(
                "recent_commands".into(),
                toml::Value::try_from(&self.recent_commands)?,
            );
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
    fn window_buttons_default_to_the_theme_on_linux_and_older_files_still_load() {
        assert_eq!(
            Config::default().window_buttons,
            if cfg!(target_os = "linux") {
                WindowButtons::Theme
            } else {
                WindowButtons::BuiltIn
            }
        );
        // A file from before the setting existed.
        let old: Config = toml::from_str("language = 'zh-CN'\ntitle_bar = 'compact'\n").unwrap();
        assert_eq!(old.window_buttons, WindowButtons::default());
        for choice in WindowButtons::ALL {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            Config {
                window_buttons: choice,
                ..Config::default()
            }
            .save(&path)
            .unwrap();
            assert_eq!(Config::load(&path).unwrap().window_buttons, choice);
        }
        let builtin: Config = toml::from_str("window_buttons = 'builtin'").unwrap();
        assert_eq!(builtin.window_buttons, WindowButtons::BuiltIn);
    }

    #[test]
    fn theme_defaults_to_the_system_and_older_files_still_load() {
        assert_eq!(Config::default().theme, Theme::System);
        // A file from before the setting existed.
        let old: Config =
            toml::from_str("language = 'zh-CN'\r\ntitle_bar = 'compact'\r\n").unwrap();
        assert_eq!(old.theme, Theme::System);
        for choice in Theme::ALL {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            Config {
                theme: choice,
                ..Config::default()
            }
            .save(&path)
            .unwrap();
            assert_eq!(Config::load(&path).unwrap().theme, choice);
        }
        let light: Config = toml::from_str("theme = 'light'").unwrap();
        assert_eq!(light.theme, Theme::Light);
    }

    #[test]
    fn system_accent_defaults_on_and_older_files_still_load() {
        assert!(Config::default().system_accent);
        // A file from before the setting existed (and before the Theme setting).
        let old: Config = toml::from_str("language = 'zh-CN'\r\nrulers = true\r\n").unwrap();
        assert!(old.system_accent);
        assert!(old.rulers);
        for choice in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            Config {
                system_accent: choice,
                ..Config::default()
            }
            .save(&path)
            .unwrap();
            assert_eq!(Config::load(&path).unwrap().system_accent, choice);
        }
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
    fn network_settings_from_older_releases_load_and_roundtrip() {
        // A configuration from before offline mode and send consent, with a grant.
        let old = "language = 'en'\n\n[plugins.comfy]\nenabled = true\n\n\
                   [plugins.comfy.grant]\ndir = '/p/comfy'\ncommand = ['python3', 'main.py']\n\n\
                   [plugins.comfy.grant.permissions]\nnetwork = ['example.com']\n";
        let config: Config = toml::from_str(&old.replace('\n', "\r\n")).unwrap();
        assert!(!config.disable_network_plugins);
        let grant = config.plugins["comfy"].grant.clone().unwrap();
        assert!(!grant.send_without_asking);
        assert!(!grant.edit_without_asking);
        assert_eq!(
            grant.permissions.edit_prompt,
            crate::plugins::manifest::EditPrompt::None
        );
        assert_eq!(grant.permissions.network, ["example.com"]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut changed = config.clone();
        changed.disable_network_plugins = true;
        changed.plugins.get_mut("comfy").unwrap().grant = Some(PluginGrant {
            send_without_asking: true,
            ..grant.clone()
        });
        changed.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("disable_network_plugins = true"), "{text}");
        assert!(text.contains("send_without_asking = true"), "{text}");
        assert_eq!(Config::load(&path).unwrap(), changed);
        // The stored answer does not change which plugin the grant covers.
        let stored = changed.plugins["comfy"].grant.as_ref().unwrap();
        assert!(stored.covers(&grant) && *stored != grant);
        // Without the answer the field is left out, as in older files.
        config.save(&path).unwrap();
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("send_without_asking")
        );
    }

    #[test]
    fn blocking_the_network_is_stored_only_once_chosen() {
        // Older files, and files of users who never changed it, follow the
        // release default.
        let old: Config =
            toml::from_str("language = 'en'\ndisable_network_plugins = true\n").unwrap();
        assert_eq!(old.block_undeclared_network, None);
        assert_eq!(
            old.block_undeclared_network(),
            BLOCK_UNDECLARED_NETWORK_DEFAULT
        );
        assert_eq!(
            Config::default().block_undeclared_network(),
            BLOCK_UNDECLARED_NETWORK_DEFAULT
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        old.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("block_undeclared_network"), "{text}");
        assert_eq!(Config::load(&path).unwrap(), old);

        for chosen in [true, false] {
            let config = Config {
                block_undeclared_network: Some(chosen),
                ..old.clone()
            };
            config.save(&path).unwrap();
            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.contains(&format!("block_undeclared_network = {chosen}")),
                "{text}"
            );
            let loaded = Config::load(&path).unwrap();
            assert_eq!(loaded, config);
            assert_eq!(loaded.block_undeclared_network(), chosen);
        }
        // Going back to "not chosen" removes it from the file.
        old.save(&path).unwrap();
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("block_undeclared_network")
        );
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
        // A broken file is reported without quoting the secret on the bad line.
        fs::write(&path, "[comfy]\napi_key = \"sk-live-123\nother = 1\n").unwrap();
        let error = format!("{:#}", Secrets::load(&path).unwrap_err());
        assert!(!error.contains("sk-live"), "{error}");
        assert!(error.contains("line 2"), "{error}");
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

    #[test]
    fn key_bindings_keep_only_overrides_and_older_files_still_load() {
        // A file from before customisable key bindings.
        let old: Config = toml::from_str("language = 'zh-CN'\npixel_grid = false\n").unwrap();
        assert!(old.keybindings.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::default().save(&path).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("keybindings"));

        let mut custom = Config::default();
        custom
            .keybindings
            .insert("merge".into(), toml::Value::String("Ctrl+Shift+M".into()));
        custom.keybindings.insert(
            "invert_selection".into(),
            toml::Value::String(String::new()),
        );
        custom.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[keybindings]") && text.contains("merge = \"Ctrl+Shift+M\""));
        assert!(text.contains("invert_selection = \"\""));
        assert_eq!(Config::load(&path).unwrap(), custom);

        // Unknown commands and odd values load as they are; the editor ignores them.
        fs::write(
            &path,
            "[keybindings]\nfuture_command = 'Ctrl+K'\nmerge = 42\n",
        )
        .unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.keybindings.len(), 2);
        // Clearing every override removes the table again.
        Config::default().save(&path).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("keybindings"));
    }

    #[test]
    fn providers_default_to_built_in_and_round_trip() {
        use crate::plugins::manifest::Capability;
        // Files from before providers existed load with every algorithm built in, and
        // a configuration that never chose one does not write the table.
        let old: Config = toml::from_str("language = 'zh-CN'\npixel_grid = false\n").unwrap();
        assert_eq!(old.providers, Providers::default());
        assert!(
            !toml::to_string(&Config::default())
                .unwrap()
                .contains("providers")
        );
        let mut config = Config::default();
        config
            .providers
            .set(Capability::SelectSubject, Some("select-bright".into()));
        let text = toml::to_string(&config).unwrap();
        assert!(
            text.contains("[providers]\nselect_subject = \"select-bright\""),
            "{text}"
        );
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(
            back.providers.get(Capability::SelectSubject),
            Some("select-bright")
        );
        assert_eq!(back.providers.get(Capability::RemoveBackground), None);
        // Saving writes the table, and choosing Built-in again removes it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap().providers, config.providers);
        config.providers.set(Capability::SelectSubject, None);
        config.save(&path).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("providers"));
        // Unknown capabilities from a later release are ignored, CRLF too.
        let later: Config =
            toml::from_str("[providers]\r\nobject_select = \"seg\"\r\nfuture = \"x\"\r\n").unwrap();
        assert_eq!(later.providers.get(Capability::ObjectSelect), Some("seg"));
    }
}

//! Linux: the desktop portal first, then KDE, GNOME and GTK settings.
//!
//! Every source is turned into a [`Readings`] value, and [`resolve`] combines them; both the
//! parsers and `resolve` are pure functions with fixtures in the tests. Only [`Desktop`] does
//! I/O: one D-Bus call per key (with a timeout, over a connection it keeps), at most two
//! `gsettings` calls, and reading `kdeglobals` and the GTK `settings.ini`.

use egui::Color32;
use zbus::zvariant::Value;

use super::SystemTheme;
use crate::app::window_theme::{Ini, desktop_is, parse_color, process_config_home};

/// Unwraps the variant-in-a-variant that the deprecated `Read` method returns.
fn unwrap_variant<'a, 'b>(value: &'b Value<'a>) -> &'b Value<'a> {
    match value {
        Value::Value(inner) => unwrap_variant(inner),
        other => other,
    }
}

/// The portal's `org.freedesktop.appearance color-scheme`: 1 prefers dark, 2 prefers light,
/// 0 (or anything else) states no preference.
pub(in crate::app) fn portal_color_scheme(value: &Value<'_>) -> Option<bool> {
    match unwrap_variant(value) {
        Value::U32(1) => Some(true),
        Value::U32(2) => Some(false),
        _ => None,
    }
}

/// The portal's `accent-color`: an `(ddd)` of sRGB components in 0–1. Values outside that
/// range mean "not set".
pub(in crate::app) fn portal_accent(value: &Value<'_>) -> Option<Color32> {
    let Value::Structure(structure) = unwrap_variant(value) else {
        return None;
    };
    let channels: Vec<f64> = structure
        .fields()
        .iter()
        .map(|field| match field {
            Value::F64(v) => Some(*v),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let [r, g, b] = channels[..] else {
        return None;
    };
    let byte = |v: f64| (0.0..=1.0).contains(&v).then(|| (v * 255.0).round() as u8);
    Some(Color32::from_rgb(byte(r)?, byte(g)?, byte(b)?))
}

/// GNOME's `org.gnome.desktop.interface color-scheme`.
pub(in crate::app) fn gnome_color_scheme(value: &str) -> Option<bool> {
    match value {
        "prefer-dark" => Some(true),
        "prefer-light" => Some(false),
        _ => None,
    }
}

/// GNOME 47's `org.gnome.desktop.interface accent-color` names, as libadwaita draws them.
pub(in crate::app) fn gnome_accent(name: &str) -> Option<Color32> {
    let [r, g, b] = match name {
        "blue" => [0x35, 0x84, 0xe4],
        "teal" => [0x21, 0x90, 0xa4],
        "green" => [0x3a, 0x94, 0x4a],
        "yellow" => [0xc8, 0x88, 0x00],
        "orange" => [0xed, 0x5b, 0x00],
        "red" => [0xe6, 0x2d, 0x42],
        "pink" => [0xd5, 0x61, 0x99],
        "purple" => [0x91, 0x41, 0xac],
        "slate" => [0x6f, 0x83, 0x96],
        _ => return None,
    };
    Some(Color32::from_rgb(r, g, b))
}

/// KDE's colour scheme, from `kdeglobals`: dark when the window background is dark, and the
/// accent colour (`[General] AccentColor`, else the selection colour).
pub(in crate::app) fn kde_theme(kdeglobals: &str) -> SystemTheme {
    let ini = Ini::parse(kdeglobals);
    let color = |section: &str, key: &str| {
        ini.get(section, key)
            .and_then(parse_color)
            .map(|[r, g, b]| Color32::from_rgb(r, g, b))
    };
    SystemTheme {
        dark: color("Colors:Window", "BackgroundNormal")
            .map(|bg| crate::app::theme::luminance(bg) < 0.18),
        accent: color("General", "AccentColor")
            .or_else(|| color("Colors:Selection", "BackgroundNormal")),
    }
}

/// Whether a GTK theme is dark, by its name: `Adwaita-dark`, `Breeze-Dark`, `Yaru-dark`.
pub(in crate::app) fn gtk_theme_dark(name: &str) -> Option<bool> {
    let name = name.trim().to_ascii_lowercase();
    (!name.is_empty()).then(|| name.ends_with("dark") || name.contains("-dark"))
}

/// Everything read from the desktop, before it is combined.
#[derive(Clone, Debug, Default)]
pub(in crate::app) struct Readings {
    pub portal_dark: Option<bool>,
    pub portal_accent: Option<Color32>,
    /// `kdeglobals`, on KDE.
    pub kdeglobals: Option<String>,
    /// `gsettings get org.gnome.desktop.interface color-scheme`, unquoted.
    pub gnome_color_scheme: Option<String>,
    /// The same for `accent-color`.
    pub gnome_accent: Option<String>,
    /// `~/.config/gtk-3.0/settings.ini` (or gtk-4.0).
    pub gtk_settings: Option<String>,
    /// `gsettings get org.gnome.desktop.interface gtk-theme`, unquoted.
    pub gsettings_gtk_theme: Option<String>,
}

/// Combines the readings, most authoritative first: the portal, KDE, GNOME, the GTK theme.
pub(in crate::app) fn resolve(r: &Readings) -> SystemTheme {
    let portal = SystemTheme {
        dark: r.portal_dark,
        accent: r.portal_accent,
    };
    let kde = r.kdeglobals.as_deref().map(kde_theme).unwrap_or_default();
    let gnome = SystemTheme {
        dark: r.gnome_color_scheme.as_deref().and_then(gnome_color_scheme),
        accent: r.gnome_accent.as_deref().and_then(gnome_accent),
    };
    let gtk_ini = r.gtk_settings.as_deref().map(Ini::parse);
    let gtk_setting = |key: &str| gtk_ini.as_ref().and_then(|ini| ini.get("Settings", key));
    let prefer_dark = gtk_setting("gtk-application-prefer-dark-theme")
        .filter(|v| *v == "1" || v.eq_ignore_ascii_case("true"))
        .map(|_| true);
    let gtk = SystemTheme {
        dark: prefer_dark.or_else(|| {
            gtk_setting("gtk-theme-name")
                .or(r.gsettings_gtk_theme.as_deref())
                .and_then(gtk_theme_dark)
        }),
        accent: None,
    };
    portal.or(kde).or(gnome).or(gtk)
}

/// The real desktop. Keeps its D-Bus connection between readings.
#[derive(Default)]
pub(in crate::app) struct Desktop {
    bus: Option<zbus::blocking::Connection>,
}

impl Desktop {
    fn portal(&mut self, key: &str) -> Option<zbus::zvariant::OwnedValue> {
        if self.bus.is_none() {
            self.bus = zbus::blocking::connection::Builder::session()
                .ok()?
                .method_timeout(std::time::Duration::from_millis(300))
                .build()
                .ok();
        }
        let bus = self.bus.as_ref()?;
        let call = |method: &str| {
            bus.call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.Settings"),
                method,
                &("org.freedesktop.appearance", key),
            )
        };
        // `ReadOne` arrived in version 2 of the interface; `Read` wraps the value once more.
        let reply = call("ReadOne").or_else(|_| call("Read"));
        if let Err(zbus::Error::InputOutput(_)) = &reply {
            // The bus went away; connect again next time.
            self.bus = None;
        }
        reply.ok()?.body().deserialize().ok()
    }

    fn readings(&mut self) -> Readings {
        let mut r = Readings {
            portal_dark: self
                .portal("color-scheme")
                .and_then(|v| portal_color_scheme(&v)),
            portal_accent: self.portal("accent-color").and_then(|v| portal_accent(&v)),
            ..Readings::default()
        };
        if r.portal_dark.is_some() && r.portal_accent.is_some() {
            return r;
        }
        let config = process_config_home();
        let read = |path: std::path::PathBuf| {
            std::fs::metadata(&path)
                .ok()
                .filter(|m| m.is_file() && m.len() <= crate::app::window_theme::MAX_FILE_BYTES)
                .and_then(|_| std::fs::read_to_string(path).ok())
        };
        let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| desktop_is(&d, "KDE"));
        if kde {
            r.kdeglobals = read(config.join("kdeglobals"));
        } else {
            [r.gnome_color_scheme, r.gnome_accent, r.gsettings_gtk_theme] =
                crate::app::window_theme::gsettings(["color-scheme", "accent-color", "gtk-theme"]);
        }
        r.gtk_settings = read(config.join("gtk-3.0/settings.ini"))
            .or_else(|| read(config.join("gtk-4.0/settings.ini")));
        r
    }
}

impl super::Source for Desktop {
    fn read(&mut self) -> SystemTheme {
        resolve(&self.readings())
    }
}

//! Window-button artwork taken from the desktop theme (issue #47).
//!
//! Compact title bars draw the minimize, maximize and close buttons with the
//! images the desktop itself uses, found at run time, in this order:
//!
//! 1. KDE only: the GTK decoration CSS that kde-gtk-config writes
//!    (`~/.config/gtk-{3,4}.0/window_decorations.css` and `gtk.css`). File
//!    names come from the CSS, never from a guess.
//! 2. The current GTK theme's `gtk.css` (any desktop).
//! 3. The `window-*-symbolic` icons of the current icon theme, tinted.
//!
//! When none of them yields a minimize, maximize and close image, the caller
//! draws its built-in glyphs. Everything here takes its directories and
//! environment through [`Env`], so tests use fixture folders.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use egui::{Color32, Vec2};

/// Largest image or stylesheet read from a theme.
pub(super) const MAX_FILE_BYTES: u64 = 1 << 20;
/// How often the files behind the current images are checked for changes.
pub(super) const RECHECK: Duration = Duration::from_secs(2);
/// Nested `@import` depth followed in a stylesheet.
const MAX_IMPORT_DEPTH: usize = 4;
/// Largest rasterised image edge, in pixels.
const MAX_IMAGE_EDGE: usize = 512;
/// Icon size asked of the icon theme, in points.
const ICON_SIZE: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Kind {
    Minimize,
    Maximize,
    Restore,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum State {
    Normal,
    Hover,
    /// Pressed.
    Active,
    /// The window is not focused.
    Backdrop,
}

/// Modification time and size of a file, the identity of its contents for caching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Asset {
    pub path: PathBuf,
    pub stamp: Stamp,
    /// Device pixels per point the image was drawn for (`@2` assets are 2).
    pub scale: u32,
    /// A monochrome icon that takes the header foreground colour.
    pub symbolic: bool,
}

impl Asset {
    fn is_svg(&self) -> bool {
        self.path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    KdeGtkConfig,
    GtkTheme,
    IconTheme,
}

/// The directories and settings the lookup reads. [`Env::default`] is empty,
/// which finds nothing.
#[derive(Clone, Debug, Default)]
pub(super) struct Env {
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub data_dirs: Vec<PathBuf>,
    /// `XDG_CURRENT_DESKTOP` names KDE.
    pub kde: bool,
    /// `gsettings get org.gnome.desktop.interface gtk-theme`, if answered in time.
    pub gsettings_gtk_theme: Option<String>,
    /// The same for `icon-theme`.
    pub gsettings_icon_theme: Option<String>,
}

/// Whether `XDG_CURRENT_DESKTOP` (a colon-separated list) names `name`.
pub(super) fn desktop_is(value: &str, name: &str) -> bool {
    value
        .split(':')
        .any(|d| d.trim().eq_ignore_ascii_case(name))
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `$XDG_CONFIG_HOME`, or `~/.config`.
pub(super) fn process_config_home() -> PathBuf {
    absolute_env("XDG_CONFIG_HOME")
        .or_else(|| absolute_env("HOME").map(|h| h.join(".config")))
        .unwrap_or_default()
}

impl Env {
    #[cfg_attr(test, allow(dead_code))]
    pub(super) fn from_process() -> Self {
        let home = absolute_env("HOME").unwrap_or_default();
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into())
            .split(':')
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .collect();
        let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| desktop_is(&d, "KDE"));
        let (gtk, icons) = if kde {
            (None, None)
        } else {
            gsettings_themes()
        };
        Self {
            config_home: process_config_home(),
            data_home: absolute_env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
            home,
            data_dirs,
            kde,
            gsettings_gtk_theme: gtk,
            gsettings_icon_theme: icons,
        }
    }

    /// Directories the lookup may read from.
    fn roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![
            self.config_home.clone(),
            self.data_home.clone(),
            self.home.join(".themes"),
            self.home.join(".icons"),
        ];
        roots.extend(self.data_dirs.iter().cloned());
        roots
            .into_iter()
            .filter(|p| p.is_absolute())
            .map(|p| normalize(&p))
            .collect()
    }

    fn theme_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.data_home.join("themes"), self.home.join(".themes")];
        dirs.extend(self.data_dirs.iter().map(|d| d.join("themes")));
        dirs
    }

    fn icon_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.data_home.join("icons"), self.home.join(".icons")];
        dirs.extend(self.data_dirs.iter().map(|d| d.join("icons")));
        dirs
    }
}

/// The GNOME GTK theme and icon theme, asked of `gsettings` with one shared timeout.
#[cfg_attr(test, allow(dead_code))]
fn gsettings_themes() -> (Option<String>, Option<String>) {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let get = |key: &str| {
            std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", key])
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .trim_matches('\'')
                        .to_owned()
                })
                .filter(|v| !v.is_empty())
        };
        let _ = send.send((get("gtk-theme"), get("icon-theme")));
    });
    receive
        .recv_timeout(Duration::from_millis(300))
        .unwrap_or_default()
}

/// Removes `.` and resolves `..` without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// File access, with a record of every path consulted

struct Probe {
    roots: Vec<PathBuf>,
    seen: RefCell<Vec<(PathBuf, Option<Stamp>)>>,
}

impl Probe {
    fn new(env: &Env) -> Self {
        Self {
            roots: env.roots(),
            seen: RefCell::default(),
        }
    }

    /// Whether `path` lies inside the config, theme and icon directories. The check is
    /// lexical, so `..` cannot leave them; a symlink there may point anywhere, but only
    /// small `.png`, `.svg`, `.css` and `.ini` style files are ever read.
    fn confined(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    /// The identity of the regular file at `path`, if it is inside the allowed directories.
    fn stat(&self, path: &Path) -> Option<Stamp> {
        let path = normalize(path);
        if !self.confined(&path) {
            return None;
        }
        let stamp = std::fs::metadata(&path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| Stamp {
                mtime: m.modified().ok(),
                len: m.len(),
            });
        let mut seen = self.seen.borrow_mut();
        if !seen.iter().any(|(p, _)| *p == path) {
            seen.push((path, stamp));
        }
        stamp
    }

    /// Text of a file of at most [`MAX_FILE_BYTES`]; a binary or unreadable file is `None`.
    fn read_text(&self, path: &Path) -> Option<String> {
        let stamp = self.stat(path)?;
        if stamp.len > MAX_FILE_BYTES {
            return None;
        }
        std::fs::read_to_string(normalize(path)).ok()
    }

    /// A usable image file: allowed, `.png` or `.svg`, and not empty or too large.
    fn asset(&self, path: &Path, scale: u32, symbolic: bool) -> Option<Asset> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        if ext != "png" && ext != "svg" {
            return None;
        }
        let stamp = self.stat(path)?;
        (stamp.len > 0 && stamp.len <= MAX_FILE_BYTES).then(|| Asset {
            path: normalize(path),
            stamp,
            scale: scale.max(1),
            symbolic,
        })
    }

    fn into_seen(self) -> Vec<(PathBuf, Option<Stamp>)> {
        self.seen.into_inner()
    }
}

// ---------------------------------------------------------------------------
// INI files (kwinrc, kdeglobals, settings.ini, index.theme)

#[derive(Default)]
pub(super) struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub(super) fn parse(text: &str) -> Self {
        let mut ini = Self::default();
        let mut current: Option<usize> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                ini.sections.push((name.trim().to_owned(), Vec::new()));
                current = Some(ini.sections.len() - 1);
            } else if let (Some(index), Some((key, value))) = (current, line.split_once('=')) {
                ini.sections[index]
                    .1
                    .push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }
        ini
    }

    /// The last value of `key` in `section`.
    pub(super) fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .iter()
            .filter(|(name, _)| name == section)
            .flat_map(|(_, entries)| entries.iter().rev())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// `r,g,b[,a]` (KDE) or `#rrggbb`.
pub(super) fn parse_color(value: &str) -> Option<[u8; 3]> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let n = u32::from_str_radix(hex, 16).ok()?;
        return Some([(n >> 16) as u8, (n >> 8) as u8, n as u8]);
    }
    let parts: Vec<u8> = value
        .split(',')
        .map(|p| p.trim().parse::<u8>())
        .collect::<Result<_, _>>()
        .ok()?;
    (parts.len() == 3 || parts.len() == 4).then(|| [parts[0], parts[1], parts[2]])
}

/// Foreground colours of the title bar, from `kdeglobals`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Tints {
    pub active: Option<[u8; 3]>,
    pub inactive: Option<[u8; 3]>,
}

impl Tints {
    pub(super) fn from_kdeglobals(text: &str) -> Self {
        let ini = Ini::parse(text);
        let color = |section: &str, key: &str| ini.get(section, key).and_then(parse_color);
        Self {
            active: color("Colors:Header", "ForegroundNormal")
                .or_else(|| color("WM", "activeForeground")),
            inactive: color("Colors:Header", "ForegroundInactive")
                .or_else(|| color("WM", "inactiveForeground")),
        }
    }

    /// The icon colour on a title bar of colour `bg`. The desktop's colour is
    /// dropped for `fallback` when it would be hard to read on `bg`.
    pub(super) fn color(&self, backdrop: bool, bg: Color32, fallback: Color32) -> Color32 {
        let wanted = if backdrop { self.inactive } else { self.active };
        wanted
            .map(|[r, g, b]| Color32::from_rgb(r, g, b))
            .filter(|c| contrast(*c, bg) >= 3.0)
            .unwrap_or(fallback)
    }
}

fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

// ---------------------------------------------------------------------------
// A forgiving CSS reader for the title-button rules

/// One image a rule names.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ImageRef {
    File { path: PathBuf, scale: u32 },
    Icon(String),
}

#[derive(Debug)]
struct Rule {
    kind: Kind,
    state: State,
    images: Vec<ImageRef>,
}

/// Drops `/* … */` comments.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The first of `targets` outside quotes and parentheses.
fn find_top(text: &str, targets: &[char]) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && targets.contains(&c) {
            return Some(i);
        }
    }
    None
}

/// Index of the `)` closing a call whose arguments start at `text`.
fn call_end(text: &str) -> usize {
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            if depth == 0 {
                return i;
            }
            depth -= 1;
        }
    }
    text.len()
}

/// Index just past the `}` that closes the `{` at `open`, or the end if unclosed.
fn block_end(text: &str, open: usize) -> usize {
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return open + i + 1;
            }
        }
    }
    text.len()
}

fn unquote(text: &str) -> &str {
    let text = text.trim();
    for q in ['"', '\''] {
        if let Some(inner) = text.strip_prefix(q).and_then(|t| t.strip_suffix(q)) {
            return inner;
        }
    }
    text
}

fn suffix_scale(path: &Path) -> u32 {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    stem.rsplit_once('@')
        .and_then(|(_, n)| n.trim_end_matches('x').parse().ok())
        .filter(|n| (1..=4).contains(n))
        .unwrap_or(1)
}

/// The images named by a `background-image`, `background` or `-gtk-icon-source` value.
/// Inside `-gtk-scaled(1x, 2x)` every URL counts; otherwise only the first (top) layer.
fn parse_images(value: &str, base: &Path) -> Vec<ImageRef> {
    let scaled = value.contains("-gtk-scaled(");
    let mut out = Vec::new();
    let mut rest = value;
    loop {
        let url = rest.find("url(").map(|i| (i, 4, false));
        let icon = rest.find("-gtk-icontheme(").map(|i| (i, 15, true));
        let Some((at, len, is_icon)) = [url, icon].into_iter().flatten().min_by_key(|m| m.0) else {
            break;
        };
        let after = &rest[at + len..];
        let end = call_end(after);
        let argument = unquote(&after[..end]);
        if is_icon {
            if !argument.is_empty() {
                out.push(ImageRef::Icon(argument.to_owned()));
            }
        } else if let Some(path) = css_path(argument, base) {
            let scale = if scaled {
                out.len() as u32 + 1
            } else {
                suffix_scale(&path)
            };
            out.push(ImageRef::File { path, scale });
        }
        rest = after.get(end + 1..).unwrap_or("");
        if !scaled && !out.is_empty() {
            break;
        }
    }
    out.sort_by_key(|image| match image {
        ImageRef::File { scale, .. } => *scale,
        ImageRef::Icon(_) => 0,
    });
    out
}

/// A stylesheet URL as a path; only relative and `file://` locations count.
fn css_path(url: &str, base: &Path) -> Option<PathBuf> {
    let url = url.trim();
    let url = url.strip_prefix("file://").unwrap_or(url);
    if url.is_empty()
        || url.contains("://")
        || url.starts_with("data:")
        || url.starts_with("resource:")
    {
        return None;
    }
    let url = url.split(['?', '#']).next()?;
    Some(base.join(url))
}

/// Class names (`.close`) and pseudo-classes (`:hover`) of one compound selector.
fn compound_parts(compound: &str) -> (Vec<String>, Vec<String>) {
    // `:not(…)` and `[attr]` parts never decide which button a rule is for.
    let mut cleaned = String::new();
    let mut depth = 0usize;
    for c in compound.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => cleaned.push(c),
            _ => {}
        }
    }
    let mut classes = Vec::new();
    let mut pseudos = Vec::new();
    let mut chars = cleaned.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '.' && c != ':' {
            continue;
        }
        if c == ':' && chars.peek() == Some(&':') {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&n) = chars.peek() {
            if n.is_ascii_alphanumeric() || n == '-' || n == '_' {
                name.push(n);
                chars.next();
            } else {
                break;
            }
        }
        if c == '.' {
            classes.push(name);
        } else {
            pseudos.push(name);
        }
    }
    (classes, pseudos)
}

/// The button and state a selector styles, if it is a title-button selector.
fn classify(selector: &str) -> Option<(Kind, State)> {
    let compounds: Vec<&str> = selector
        .split(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~'))
        .filter(|c| !c.is_empty())
        .collect();
    let subject = compounds.last()?;
    let (classes, pseudos) = compound_parts(subject);
    let button = classes.iter().find_map(|c| match c.as_str() {
        "close" => Some(Kind::Close),
        "minimize" => Some(Kind::Minimize),
        "maximize" => Some(Kind::Maximize),
        "restore" => Some(Kind::Restore),
        _ => None,
    })?;
    let parts: Vec<_> = compounds.iter().map(|c| compound_parts(c)).collect();
    let is_button = subject.starts_with("button") || classes.iter().any(|c| c == "titlebutton");
    let in_titlebar = parts.iter().any(|(classes, _)| {
        classes
            .iter()
            .any(|c| c == "titlebutton" || c == "titlebar")
    }) || compounds.iter().any(|c| {
        let element = c.split(['.', ':']).next().unwrap_or("");
        matches!(element, "headerbar" | "windowcontrols")
    });
    if !is_button || !in_titlebar {
        return None;
    }
    let maximized = parts
        .iter()
        .any(|(classes, _)| classes.iter().any(|c| c == "maximized"))
        || pseudos.iter().any(|p| p == "checked");
    let kind = match button {
        Kind::Maximize if maximized => Kind::Restore,
        other => other,
    };
    let pseudo_on_any = |name: &str| parts.iter().any(|(_, p)| p.iter().any(|p| p == name));
    if pseudo_on_any("disabled") || pseudo_on_any("insensitive") {
        return None;
    }
    let state = match (
        pseudo_on_any("hover"),
        pseudo_on_any("active"),
        pseudo_on_any("backdrop"),
    ) {
        (false, false, false) => State::Normal,
        (_, true, false) => State::Active,
        (true, false, false) => State::Hover,
        (false, false, true) => State::Backdrop,
        // Combined backdrop states are rare and not worth a state of their own.
        _ => return None,
    };
    Some((kind, state))
}

struct CssReader<'a> {
    probe: &'a Probe,
    seen: Vec<PathBuf>,
    rules: Vec<Rule>,
}

impl CssReader<'_> {
    fn read(&mut self, path: &Path, depth: usize) {
        let path = normalize(path);
        if depth > MAX_IMPORT_DEPTH || self.seen.contains(&path) {
            return;
        }
        self.seen.push(path.clone());
        let Some(text) = self.probe.read_text(&path) else {
            return;
        };
        let text = strip_comments(&text);
        let base = path.parent().unwrap_or(Path::new("")).to_owned();
        let mut rest = text.as_str();
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '}' || c == ';');
            if rest.is_empty() {
                break;
            }
            if rest.starts_with('@') {
                let Some(end) = find_top(rest, &[';', '{']) else {
                    break;
                };
                if rest[end..].starts_with(';') {
                    if let Some(statement) = rest[..end].strip_prefix("@import") {
                        let target = statement.find("url(").map_or_else(
                            || unquote(statement),
                            |i| unquote(&statement[i + 4..][..call_end(&statement[i + 4..])]),
                        );
                        if let Some(file) = css_path(target, &base) {
                            self.read(&file, depth + 1);
                        }
                    }
                    rest = &rest[end + 1..];
                } else {
                    rest = &rest[block_end(rest, end)..];
                }
                continue;
            }
            let Some(open) = find_top(rest, &['{']) else {
                break;
            };
            let close = block_end(rest, open);
            let body_end = if rest[..close].ends_with('}') {
                close - 1
            } else {
                close
            };
            self.rule(
                &rest[..open],
                &rest[open + 1..body_end.max(open + 1)],
                &base,
            );
            rest = &rest[close..];
        }
    }

    fn rule(&mut self, selectors: &str, body: &str, base: &Path) {
        let mut images = Vec::new();
        let mut rest = body;
        while !rest.trim().is_empty() {
            let end = find_top(rest, &[';']).unwrap_or(rest.len());
            if let Some((property, value)) = rest[..end].split_once(':')
                && matches!(
                    property.trim().to_ascii_lowercase().as_str(),
                    "background-image" | "background" | "-gtk-icon-source"
                )
            {
                let parsed = parse_images(value, base);
                if !parsed.is_empty() {
                    images = parsed;
                }
            }
            rest = rest.get(end + 1..).unwrap_or("");
        }
        if images.is_empty() {
            return;
        }
        let mut selectors = selectors;
        loop {
            let end = find_top(selectors, &[',']).unwrap_or(selectors.len());
            if let Some((kind, state)) = classify(&selectors[..end]) {
                self.rules.push(Rule {
                    kind,
                    state,
                    images: images.clone(),
                });
            }
            match selectors.get(end + 1..) {
                Some(next) => selectors = next,
                None => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Freedesktop icon themes

struct IconThemes<'a> {
    env: &'a Env,
    probe: &'a Probe,
    primary: String,
}

impl IconThemes<'_> {
    /// `name.svg` or `name.png` from the theme, its `Inherits` chain, then hicolor.
    fn find(&self, name: &str) -> Option<PathBuf> {
        let mut visited = Vec::new();
        self.find_in(&self.primary, name, &mut visited)
            .or_else(|| self.find_in("hicolor", name, &mut visited))
    }

    fn find_in(&self, theme: &str, name: &str, visited: &mut Vec<String>) -> Option<PathBuf> {
        if theme.is_empty() || theme.contains(['/', '\\']) || visited.iter().any(|v| v == theme) {
            return None;
        }
        visited.push(theme.to_owned());
        let mut parents: Vec<String> = Vec::new();
        for base in self.env.icon_dirs() {
            let dir = base.join(theme);
            let Some(text) = self.probe.read_text(&dir.join("index.theme")) else {
                continue;
            };
            let ini = Ini::parse(&text);
            let mut best: Option<(i32, PathBuf)> = None;
            let directories = ini.get("Icon Theme", "Directories").unwrap_or("");
            for sub in directories
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let distance = directory_distance(&ini, sub, ICON_SIZE);
                for ext in ["svg", "png"] {
                    let file = dir.join(sub).join(format!("{name}.{ext}"));
                    if self.probe.stat(&file).is_some() {
                        if best.as_ref().is_none_or(|(d, _)| distance < *d) {
                            best = Some((distance, file));
                        }
                        break;
                    }
                }
            }
            if let Some((_, file)) = best {
                return Some(file);
            }
            parents.extend(
                ini.get("Icon Theme", "Inherits")
                    .unwrap_or("")
                    .split(',')
                    .map(|p| p.trim().to_owned())
                    .filter(|p| !p.is_empty()),
            );
        }
        parents
            .iter()
            .find_map(|parent| self.find_in(parent, name, visited))
    }
}

/// The Icon Theme spec's `DirectorySizeDistance` for a one-point scale.
fn directory_distance(ini: &Ini, section: &str, wanted: i32) -> i32 {
    let number = |key: &str| ini.get(section, key).and_then(|v| v.parse::<i32>().ok());
    let size = number("Size").unwrap_or(ICON_SIZE);
    let scale = number("Scale").unwrap_or(1).max(1);
    let (min, max) = (
        number("MinSize").unwrap_or(size),
        number("MaxSize").unwrap_or(size),
    );
    let threshold = number("Threshold").unwrap_or(2);
    let size = size * scale;
    let (min, max) = (min * scale, max * scale);
    match ini.get(section, "Type").unwrap_or("Threshold") {
        "Fixed" => (size - wanted).abs(),
        "Scalable" if wanted < min => min - wanted,
        "Scalable" if wanted > max => wanted - max,
        "Scalable" => 0,
        _ if wanted < size - threshold => (size - threshold) - wanted,
        _ if wanted > size + threshold => wanted - (size + threshold),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Resolution

/// How the highlight for a state without its own image is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Highlight {
    None,
    Hover,
    Pressed,
}

/// The image to draw for one button state.
#[derive(Debug)]
pub(super) struct Pick<'a> {
    pub asset: &'a Asset,
    /// The theme has no image for this state: draw this highlight behind the other one.
    pub highlight: Highlight,
    /// The theme has no backdrop image: dim the normal one.
    pub dim: bool,
}

#[derive(Debug)]
pub(super) struct Resolved {
    /// Which lookup step found the images (checked by the tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub source: Source,
    images: BTreeMap<(Kind, State), Vec<Asset>>,
    pub tints: Tints,
}

impl Resolved {
    fn candidate(&self, kind: Kind, state: State, ppp: f32) -> Option<&Asset> {
        let list = self.images.get(&(kind, state))?;
        let wanted = ppp.ceil().max(1.0) as u32;
        list.iter().find(|a| a.scale >= wanted).or(list.last())
    }

    /// The image for `kind` in `state`, falling back to the normal image. A
    /// button without a normal image is `None`: the caller draws its own.
    pub(super) fn pick(&self, kind: Kind, state: State, ppp: f32) -> Option<Pick<'_>> {
        let normal = self.candidate(kind, State::Normal, ppp)?;
        let plain = |asset| Pick {
            asset,
            highlight: Highlight::None,
            dim: false,
        };
        Some(match state {
            State::Normal => plain(normal),
            State::Hover => self.candidate(kind, State::Hover, ppp).map_or(
                Pick {
                    asset: normal,
                    highlight: Highlight::Hover,
                    dim: false,
                },
                plain,
            ),
            State::Active => self.candidate(kind, State::Active, ppp).map_or_else(
                || Pick {
                    asset: self.candidate(kind, State::Hover, ppp).unwrap_or(normal),
                    highlight: Highlight::Pressed,
                    dim: false,
                },
                plain,
            ),
            State::Backdrop => self.candidate(kind, State::Backdrop, ppp).map_or(
                Pick {
                    asset: normal,
                    highlight: Highlight::None,
                    dim: true,
                },
                plain,
            ),
        })
    }

    #[cfg(test)]
    pub(super) fn path(&self, kind: Kind, state: State) -> Option<&Path> {
        self.images
            .get(&(kind, state))
            .and_then(|l| l.first())
            .map(|a| a.path.as_path())
    }
}

/// Turns parsed rules into assets; `None` unless minimize, maximize and close all have a
/// normal image. Later rules override earlier ones, as in a stylesheet.
fn build(
    source: Source,
    rules: &[Rule],
    probe: &Probe,
    icons: &IconThemes<'_>,
    tints: Tints,
) -> Option<Resolved> {
    let mut images: BTreeMap<(Kind, State), Vec<Asset>> = BTreeMap::new();
    for rule in rules {
        let assets: Vec<Asset> = rule
            .images
            .iter()
            .filter_map(|image| match image {
                ImageRef::File { path, scale } => probe.asset(path, *scale, false),
                ImageRef::Icon(name) => icons
                    .find(name)
                    .and_then(|path| probe.asset(&path, 1, true)),
            })
            .collect();
        if !assets.is_empty() {
            images.insert((rule.kind, rule.state), assets);
        }
    }
    [Kind::Minimize, Kind::Maximize, Kind::Close]
        .iter()
        .all(|&k| images.contains_key(&(k, State::Normal)))
        .then_some(Resolved {
            source,
            images,
            tints,
        })
}

fn gtk_setting(env: &Env, probe: &Probe, key: &str) -> Option<String> {
    ["gtk-3.0", "gtk-4.0"].iter().find_map(|v| {
        let text = probe.read_text(&env.config_home.join(v).join("settings.ini"))?;
        Ini::parse(&text)
            .get("Settings", key)
            .map(str::to_owned)
            .filter(|v| !v.is_empty())
    })
}

fn icon_theme_name(env: &Env, probe: &Probe) -> String {
    let kde = env
        .kde
        .then(|| probe.read_text(&env.config_home.join("kdeglobals")))
        .flatten()
        .and_then(|text| Ini::parse(&text).get("Icons", "Theme").map(str::to_owned));
    kde.or_else(|| gtk_setting(env, probe, "gtk-icon-theme-name"))
        .or_else(|| env.gsettings_icon_theme.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| if env.kde { "breeze" } else { "Adwaita" }.into())
}

fn resolve(env: &Env, probe: &Probe) -> Option<Resolved> {
    let tints = env
        .kde
        .then(|| probe.read_text(&env.config_home.join("kdeglobals")))
        .flatten()
        .map(|text| Tints::from_kdeglobals(&text))
        .unwrap_or_default();
    let icons = IconThemes {
        env,
        probe,
        primary: icon_theme_name(env, probe),
    };
    let css = |files: &[PathBuf]| {
        let mut reader = CssReader {
            probe,
            seen: Vec::new(),
            rules: Vec::new(),
        };
        for file in files {
            reader.read(file, 0);
        }
        reader.rules
    };

    // 1. kde-gtk-config's rendering of the current KWin decoration.
    if env.kde {
        for version in ["gtk-3.0", "gtk-4.0"] {
            let dir = env.config_home.join(version);
            let rules = css(&[dir.join("window_decorations.css"), dir.join("gtk.css")]);
            if let Some(found) = build(Source::KdeGtkConfig, &rules, probe, &icons, tints) {
                return Some(found);
            }
        }
    }

    // 2. The GTK theme's own stylesheet.
    let theme =
        gtk_setting(env, probe, "gtk-theme-name").or_else(|| env.gsettings_gtk_theme.clone());
    if let Some(theme) = theme.filter(|t| !t.is_empty() && !t.contains(['/', '\\'])) {
        let dark = gtk_setting(env, probe, "gtk-application-prefer-dark-theme")
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        for themes in env.theme_dirs() {
            for version in ["gtk-3.0", "gtk-4.0"] {
                let dir = themes.join(&theme).join(version);
                let mut files = vec![dir.join("gtk.css")];
                if dark {
                    files.push(dir.join("gtk-dark.css"));
                }
                let rules = css(&files);
                if let Some(found) = build(Source::GtkTheme, &rules, probe, &icons, tints) {
                    return Some(found);
                }
            }
        }
    }

    // 3. The icon theme's symbolic window icons.
    let rules: Vec<Rule> = [
        (Kind::Minimize, "window-minimize-symbolic"),
        (Kind::Maximize, "window-maximize-symbolic"),
        (Kind::Restore, "window-restore-symbolic"),
        (Kind::Close, "window-close-symbolic"),
    ]
    .into_iter()
    .map(|(kind, name)| Rule {
        kind,
        state: State::Normal,
        images: vec![ImageRef::Icon(name.into())],
    })
    .collect();
    build(Source::IconTheme, &rules, probe, &icons, tints)
}

// ---------------------------------------------------------------------------
// Loading, caching

/// Decodes an asset to pixels at `ppp`, tinting a symbolic icon. `None` for anything malformed.
pub(super) fn rasterise(
    bytes: &[u8],
    svg: bool,
    ppp: f32,
    tint: Option<Color32>,
) -> Option<egui::ColorImage> {
    let mut image = if svg {
        let options = resvg::usvg::Options::default();
        egui_extras::image::load_svg_bytes_with_size(
            bytes,
            egui::SizeHint::Scale(ppp.into()),
            &options,
        )
        .ok()?
    } else {
        {
            let rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                .ok()?
                .to_rgba8();
            egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            )
        }
    };
    let [w, h] = image.size;
    if w == 0 || h == 0 || w > MAX_IMAGE_EDGE || h > MAX_IMAGE_EDGE {
        return None;
    }
    if let Some(tint) = tint {
        for pixel in &mut image.pixels {
            let alpha = pixel.a();
            *pixel = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), alpha);
        }
    }
    Some(image)
}

/// What identifies one cached texture: the file's contents (path, mtime, size), the display
/// scale and the tint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct TextureKey {
    path: PathBuf,
    stamp: Stamp,
    ppp_bits: u32,
    tint: Option<[u8; 3]>,
}

impl TextureKey {
    pub(super) fn new(asset: &Asset, ppp: f32, tint: Option<Color32>) -> Self {
        Self {
            path: asset.path.clone(),
            stamp: asset.stamp,
            ppp_bits: ppp.to_bits(),
            tint: tint.map(|c| [c.r(), c.g(), c.b()]),
        }
    }
}

pub(super) struct WindowTheme {
    env: Env,
    resolved: Option<Resolved>,
    fingerprint: Vec<(PathBuf, Option<Stamp>)>,
    checked: Option<Instant>,
    /// Counts reloads, so callers can tell the artwork changed.
    pub generation: u64,
    textures: HashMap<TextureKey, Option<(egui::TextureHandle, Vec2)>>,
}

impl WindowTheme {
    pub(super) fn new(env: Env) -> Self {
        let mut theme = Self {
            env,
            resolved: None,
            fingerprint: Vec::new(),
            checked: None,
            generation: 0,
            textures: HashMap::new(),
        };
        theme.reload(Instant::now());
        theme
    }

    fn reload(&mut self, now: Instant) {
        let probe = Probe::new(&self.env);
        self.resolved = resolve(&self.env, &probe);
        self.fingerprint = probe.into_seen();
        self.checked = Some(now);
        self.generation += 1;
        self.textures.clear();
    }

    /// Reloads when a file the current images came from, or a file that could have
    /// provided better ones, changed. Looks at the disk at most once per [`RECHECK`].
    pub(super) fn refresh(&mut self, now: Instant) {
        if self
            .checked
            .is_some_and(|at| now.saturating_duration_since(at) < RECHECK)
        {
            return;
        }
        self.checked = Some(now);
        let probe = Probe::new(&self.env);
        let changed = self
            .fingerprint
            .iter()
            .any(|(path, stamp)| probe.stat(path) != *stamp);
        if changed {
            self.reload(now);
        }
    }

    pub(super) fn resolved(&self) -> Option<&Resolved> {
        self.resolved.as_ref()
    }

    /// The texture for `asset` at `ppp` and its size in points, loading it on first use.
    pub(super) fn texture(
        &mut self,
        ctx: &egui::Context,
        asset: &Asset,
        ppp: f32,
        tint: Option<Color32>,
    ) -> Option<(egui::TextureId, Vec2)> {
        let tint = tint.filter(|_| asset.symbolic);
        if self.textures.len() > 64 {
            self.textures.clear();
        }
        let entry = self
            .textures
            .entry(TextureKey::new(asset, ppp, tint))
            .or_insert_with(|| {
                let bytes = read_capped(&asset.path)?;
                let image = rasterise(&bytes, asset.is_svg(), ppp, tint)?;
                let pixels = Vec2::new(image.size[0] as f32, image.size[1] as f32);
                let points = if asset.is_svg() {
                    pixels / ppp
                } else {
                    pixels / asset.scale as f32
                };
                let handle = ctx.load_texture(
                    format!("window-button:{}", asset.path.display()),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                Some((handle, points))
            });
        entry.as_ref().map(|(handle, size)| (handle.id(), *size))
    }

    #[cfg(test)]
    pub(super) fn loaded_textures(&self) -> usize {
        self.textures.values().flatten().count()
    }
}

fn read_capped(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_FILE_BYTES).then_some(bytes)
}

#[cfg(test)]
pub(in crate::app) mod tests;

//! File → Open Recent: the files opened or saved lately, kept in the configuration, and the
//! Move tool's options, which are remembered the same way.
//!
//! Reopen Closed Tab (see [`super::tabs`]) is a different thing: it brings back the tabs closed
//! in this session. Open Recent lasts across launches and also lists files whose tab is still
//! open.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use xuan::config::push_recent_file;

use super::EditorApp;

/// Longest label, in characters, an entry of the Open Recent menu gets.
pub(super) const LABEL_CHARS: usize = 48;
/// How long a check of which recent files exist stays valid. Files may be on a slow share, so
/// the open menu does not look at the disk every frame.
const EXISTS_TTL: Duration = Duration::from_secs(2);

/// What the menu shows for `path`: control characters (a file name may hold a line break) made
/// harmless, and a long path cut in the middle so the file name stays readable. The full path
/// goes in the tooltip, see [`safe_path`].
pub(super) fn elide_path(path: &Path, max_chars: usize) -> String {
    let full = safe_path(path);
    if full.chars().count() <= max_chars {
        return full;
    }
    let name = safe_name(path);
    let name_chars = name.chars().count();
    let separator = std::path::MAIN_SEPARATOR;
    // "…" and the separator before the name.
    if name_chars + 2 >= max_chars {
        let keep: String = name.chars().take(max_chars.saturating_sub(1)).collect();
        return format!("{keep}…");
    }
    let parent = path.parent().map(safe_path).unwrap_or_default();
    let budget = max_chars - name_chars - 2;
    let skip = parent.chars().count().saturating_sub(budget);
    let tail: String = parent.chars().skip(skip).collect();
    format!("…{tail}{separator}{name}")
}

/// `path` as text with control characters replaced, for labels and tooltips.
pub(super) fn safe_path(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn safe_name(path: &Path) -> String {
    path.file_name()
        .map(|name| safe_path(Path::new(name)))
        .unwrap_or_else(|| safe_path(path))
}

/// Which of the recent files exist, as of `at`.
#[derive(Default)]
pub(super) struct RecentExists {
    checked: Option<Instant>,
    found: Vec<(PathBuf, bool)>,
}

impl EditorApp {
    /// Puts `path` at the front of File → Open Recent and remembers it.
    pub(super) fn remember_recent(&mut self, path: &Path) {
        // Absolute, so the same file opened by a relative path is not listed twice.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let before = self.config.recent_files.clone();
        push_recent_file(&mut self.config.recent_files, &path);
        if self.config.recent_files != before {
            self.recent_exists.checked = None;
            self.save_config();
        }
    }

    /// Whether each recent file still exists, looked up at most every [`EXISTS_TTL`].
    pub(super) fn recent_status(&mut self) -> Vec<(PathBuf, bool)> {
        let cache = &mut self.recent_exists;
        let stale = cache.checked.is_none_or(|t| t.elapsed() > EXISTS_TTL)
            || cache.found.len() != self.config.recent_files.len()
            || cache
                .found
                .iter()
                .zip(&self.config.recent_files)
                .any(|((a, _), b)| a != b);
        if stale {
            cache.found = self
                .config
                .recent_files
                .iter()
                .map(|p| (p.clone(), p.exists()))
                .collect();
            cache.checked = Some(Instant::now());
        }
        cache.found.clone()
    }

    /// Opens an entry of File → Open Recent, or switches to its tab when it is already open.
    /// A file that fails to open (it was moved or deleted since the menu looked) leaves the
    /// list, as the error says why.
    pub(super) fn open_recent(&mut self, path: &Path) {
        if let Some(index) = self
            .sessions
            .iter()
            .position(|s| s.path.as_deref() == Some(path) || s.source.as_deref() == Some(path))
        {
            self.current = index;
            let id = self.sessions[index].document.id;
            self.activate_tab(super::tabs::TabKey::Document(id));
            self.remember_recent(path);
            return;
        }
        if !self.open_path(path, false) {
            self.forget_recent(path);
        }
    }

    pub(super) fn forget_recent(&mut self, path: &Path) {
        let before = self.config.recent_files.len();
        self.config.recent_files.retain(|p| p != path);
        if self.config.recent_files.len() != before {
            self.recent_exists.checked = None;
            self.save_config();
        }
    }

    /// File → Open Recent → Clear Recently Opened.
    pub(super) fn clear_recent_files(&mut self) {
        if !self.config.recent_files.is_empty() {
            self.config.recent_files.clear();
            self.recent_exists.checked = None;
            self.save_config();
        }
    }

    /// Takes the Move tool's options from the configuration (at startup).
    pub(super) fn apply_move_options(&mut self) {
        self.auto_select = self.config.auto_select;
        self.ignore_transparent_pixels = self.config.ignore_transparent_pixels;
        self.show_controls = self.config.show_controls;
    }

    /// Remembers changes to the Move tool's options, whichever control made them.
    pub(super) fn sync_move_options(&mut self) {
        let config = &self.config;
        if (config.auto_select, config.ignore_transparent_pixels, config.show_controls)
            != (self.auto_select, self.ignore_transparent_pixels, self.show_controls)
        {
            self.config.auto_select = self.auto_select;
            self.config.ignore_transparent_pixels = self.ignore_transparent_pixels;
            self.config.show_controls = self.show_controls;
            self.save_config();
        }
    }
}

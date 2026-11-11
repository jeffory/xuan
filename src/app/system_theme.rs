//! The desktop's light/dark preference and accent colour (issue #48).
//!
//! The rest of the interface sees only [`SystemTheme`]. Each platform has one backend, chosen
//! in [`detect`], the only place with `cfg(target_os = …)`:
//!
//! - **Linux** ([`linux`]): the xdg-desktop-portal `org.freedesktop.appearance` settings, then
//!   KDE's `kdeglobals`, GNOME's `gsettings`, and the GTK theme name.
//! - **Windows** ([`windows`]): `AppsUseLightTheme` and the DWM accent colour in the registry.
//! - **macOS**: not yet; see [`detect`].
//!
//! [`Watcher`] reads the backend on a background thread every few seconds, so a change of the
//! desktop theme reaches Xuan without a restart and without ever blocking a frame.

use std::{
    sync::{Arc, Weak, mpsc},
    time::Duration,
};

use egui::Color32;

#[cfg(target_os = "linux")]
pub(super) mod linux;
#[cfg(any(windows, test))]
pub(super) mod windows;

/// What the desktop asks of applications. `None` means it doesn't say, or couldn't be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SystemTheme {
    /// `Some(true)` for dark, `Some(false)` for light.
    pub dark: Option<bool>,
    pub accent: Option<Color32>,
}

impl SystemTheme {
    /// Each field from `self`, or from `fallback` where `self` has none.
    #[cfg(any(target_os = "linux", test))]
    pub fn or(self, fallback: Self) -> Self {
        Self {
            dark: self.dark.or(fallback.dark),
            accent: self.accent.or(fallback.accent),
        }
    }
}

/// How often the desktop is asked again (in tests, often, so they finish quickly).
pub(super) const POLL_INTERVAL: Duration = if cfg!(test) {
    Duration::from_millis(20)
} else {
    Duration::from_secs(3)
};
/// How long startup waits for the first answer before going on with the default palette.
pub(super) const FIRST_READ_TIMEOUT: Duration = Duration::from_millis(300);

/// A way of reading the desktop's theme. Implemented by the platform backend and by tests.
pub(super) trait Source: Send + 'static {
    fn read(&mut self) -> SystemTheme;
}

impl<F: FnMut() -> SystemTheme + Send + 'static> Source for F {
    fn read(&mut self) -> SystemTheme {
        self()
    }
}

/// The platform's backend.
pub(super) fn detect() -> Box<dyn Source> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::Desktop::default())
    }
    #[cfg(windows)]
    {
        Box::new(windows::read)
    }
    #[cfg(target_os = "macos")]
    {
        // TODO(macOS): read `NSApp.effectiveAppearance` (dark when its best match among
        // `NSAppearanceNameAqua` / `NSAppearanceNameDarkAqua` is DarkAqua) and
        // `NSColor.controlAccentColor` converted to sRGB. See DEVELOPMENT.md, "Platform
        // theme backends".
        Box::new(SystemTheme::default)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        Box::new(SystemTheme::default)
    }
}

/// Reads a [`Source`] on its own thread, now and every `interval`, and hands over changes.
pub(super) struct Watcher {
    changes: mpsc::Receiver<SystemTheme>,
    /// The thread stops once this is dropped.
    _alive: Arc<()>,
    latest: SystemTheme,
}

impl Watcher {
    /// Starts reading `source`. Waits at most `first_wait` for the first answer, so the first
    /// frame can already use the desktop's colours; a slower answer arrives through
    /// [`Watcher::poll`]. `repaint` is woken whenever the theme changes.
    pub fn spawn(
        mut source: Box<dyn Source>,
        interval: Duration,
        first_wait: Duration,
        repaint: Option<egui::Context>,
    ) -> Self {
        let (send, changes) = mpsc::channel();
        let alive = Arc::new(());
        let weak: Weak<()> = Arc::downgrade(&alive);
        let spawned = std::thread::Builder::new()
            .name("xuan-system-theme".into())
            .spawn(move || {
                let mut last = None;
                while weak.strong_count() > 0 {
                    let theme = source.read();
                    if last != Some(theme) {
                        last = Some(theme);
                        if send.send(theme).is_err() {
                            break;
                        }
                        if let Some(ctx) = &repaint {
                            ctx.request_repaint();
                        }
                    }
                    std::thread::sleep(interval);
                }
            });
        let mut watcher = Self {
            changes,
            _alive: alive,
            latest: SystemTheme::default(),
        };
        if spawned.is_ok()
            && let Ok(theme) = watcher.changes.recv_timeout(first_wait)
        {
            watcher.latest = theme;
        }
        watcher
    }

    /// The newest reading, and whether it differs from the one returned before.
    pub fn poll(&mut self) -> (SystemTheme, bool) {
        let before = self.latest;
        while let Ok(theme) = self.changes.try_recv() {
            self.latest = theme;
        }
        (self.latest, self.latest != before)
    }
}

#[cfg(test)]
mod tests;

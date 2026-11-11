//! Build metadata baked in by `build.rs`, shown in About and `--version`.

/// What distinguishes one build from another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildInfo<'a> {
    pub release: bool,
    pub version: &'a str,
    /// Short commit hash, when known.
    pub commit: Option<&'a str>,
    /// Commit date (`YYYY-MM-DD`), when known.
    pub date: Option<&'a str>,
    pub dirty: bool,
}

/// The metadata of this binary.
pub fn current() -> BuildInfo<'static> {
    let some = |s: &'static str| (!s.is_empty()).then_some(s);
    BuildInfo {
        release: env!("XUAN_BUILD_CHANNEL_VALUE") == "release",
        version: env!("CARGO_PKG_VERSION"),
        commit: some(env!("XUAN_BUILD_COMMIT_VALUE")),
        date: some(env!("XUAN_BUILD_DATE_VALUE")),
        dirty: env!("XUAN_BUILD_DIRTY_VALUE") == "1",
    }
}

impl BuildInfo<'_> {
    /// The text for About, with the words passed through `tr`
    /// ("Version 0.3.0", "Build ad6382c · 2026-10-05", "Development build").
    pub fn display_with(&self, tr: impl Fn(&str) -> String) -> String {
        if self.release {
            return format!("{} {}", tr("Version"), self.version);
        }
        let Some(commit) = self.commit else {
            return tr("Development build");
        };
        let mut text = format!("{} {commit}", tr("Build"));
        if let Some(date) = self.date {
            text.push_str(" · ");
            text.push_str(date);
        }
        if self.dirty {
            text.push_str(" (");
            text.push_str(&tr("modified"));
            text.push(')');
        }
        text
    }

    /// The English text for About.
    pub fn display(&self) -> String {
        self.display_with(str::to_owned)
    }

    /// The version clap prints after the program name ("0.3.0",
    /// "build ad6382c (2026-10-05)", "development build").
    pub fn cli_version(&self) -> String {
        if self.release {
            return self.version.to_owned();
        }
        let Some(commit) = self.commit else {
            return "development build".to_owned();
        };
        let mut text = format!("build {commit}");
        if let Some(date) = self.date {
            text.push_str(&format!(" ({date})"));
        }
        if self.dirty {
            text.push_str(" +dirty");
        }
        text
    }

    /// The full `xuan --version` line, also what the About dialog copies.
    pub fn cli(&self) -> String {
        format!("xuan {}", self.cli_version())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(release: bool, commit: Option<&'static str>, dirty: bool) -> BuildInfo<'static> {
        BuildInfo {
            release,
            version: "0.3.0",
            commit,
            date: commit.map(|_| "2026-10-05"),
            dirty,
        }
    }

    #[test]
    fn release_shows_the_version_only() {
        let release = info(true, Some("ad6382c"), true);
        assert_eq!(release.display(), "Version 0.3.0");
        assert_eq!(release.cli(), "xuan 0.3.0");
    }

    #[test]
    fn dev_shows_commit_and_date_instead_of_the_version() {
        let dev = info(false, Some("ad6382c"), false);
        assert_eq!(dev.display(), "Build ad6382c · 2026-10-05");
        assert_eq!(dev.cli(), "xuan build ad6382c (2026-10-05)");
    }

    #[test]
    fn dirty_dev_builds_are_marked() {
        let dirty = info(false, Some("ad6382c"), true);
        assert_eq!(dirty.display(), "Build ad6382c · 2026-10-05 (modified)");
        assert_eq!(dirty.cli(), "xuan build ad6382c (2026-10-05) +dirty");
    }

    #[test]
    fn unknown_builds_say_so() {
        let unknown = info(false, None, false);
        assert_eq!(unknown.display(), "Development build");
        assert_eq!(unknown.cli(), "xuan development build");
    }

    #[test]
    fn missing_date_is_omitted() {
        let mut dev = info(false, Some("ad6382c"), false);
        dev.date = None;
        assert_eq!(dev.display(), "Build ad6382c");
        assert_eq!(dev.cli(), "xuan build ad6382c");
    }
}

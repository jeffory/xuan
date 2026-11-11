//! Embeds build metadata (commit, dirty flag, date, channel) for the About dialog and
//! `--version`. Every lookup is best-effort: without git, outside a checkout (source
//! tarballs) or in a worktree the build still succeeds and the metadata is simply absent.
//! Also embeds every interface language in `assets/locales` (see `src/i18n.rs`).

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "src/i18n/locale_files.rs"]
mod locale_files;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// Seconds since the epoch to a civil date (Howard Hinnant's algorithm).
fn civil_date(epoch_seconds: u64) -> String {
    let z = (epoch_seconds / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Resolves the directory that holds `HEAD` (`.git` is a file in a worktree).
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text.strip_prefix("gitdir:")?.trim();
    Some(root.join(target))
}

fn watch_git(root: &Path) {
    let Some(dir) = git_dir(root) else { return };
    let head = dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    let Ok(text) = std::fs::read_to_string(&head) else {
        return;
    };
    let Some(reference) = text.strip_prefix("ref:") else {
        return;
    };
    let reference = reference.trim();
    // Branch refs live in the common dir for worktrees.
    let common = std::fs::read_to_string(dir.join("commondir"))
        .ok()
        .map(|c| dir.join(c.trim()))
        .unwrap_or_else(|| dir.clone());
    println!("cargo:rerun-if-changed={}", dir.join(reference).display());
    println!(
        "cargo:rerun-if-changed={}",
        common.join(reference).display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        common.join("packed-refs").display()
    );
}

/// Writes `locales.rs` to `OUT_DIR`: a `(tag, contents)` slice of every `<tag>.tsv`, so dropping a
/// file into `assets/locales` adds a language on the next build.
fn embed_locales(root: &Path) {
    let dir = root.join("assets/locales");
    // A directory is watched with everything in it.
    println!("cargo:rerun-if-changed={}", dir.display());
    println!("cargo:rerun-if-changed=src/i18n/locale_files.rs");
    let locale_files::LocaleFiles { found, skipped } = match locale_files::locale_files(&dir) {
        Ok(files) => files,
        Err(error) => {
            println!("cargo:warning=Cannot list {}: {error}", dir.display());
            Default::default()
        }
    };
    for name in skipped {
        println!(
            "cargo:warning=assets/locales/{name} is not named after a language tag such as uk.tsv or pt-BR.tsv, so it is not built in"
        );
    }
    let mut code = String::from("&[\n");
    for (tag, path) in found {
        code.push_str(&format!(
            "    ({tag:?}, include_str!({:?})),\n",
            path.display().to_string()
        ));
    }
    code.push_str("]\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("locales.rs"), code).expect("cannot write locales.rs");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Source archives carry the commit they were cut from in this file.
    println!("cargo:rerun-if-changed=.xuan-build-commit");
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    embed_locales(&root);
    let in_git = git(&["rev-parse", "--git-dir"]).is_some();
    if in_git {
        watch_git(&root);
    }

    let injected = env("XUAN_BUILD_COMMIT");
    let commit = injected
        .clone()
        .or_else(|| in_git.then(|| git(&["rev-parse", "HEAD"])).flatten())
        .or_else(|| std::fs::read_to_string(root.join(".xuan-build-commit")).ok())
        .map(|c| c.trim().chars().take(7).collect::<String>())
        .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_hexdigit()));

    // Only a checkout can be dirty, and an injected commit (CI) is taken as clean.
    let dirty = in_git
        && injected.is_none()
        && git(&["status", "--porcelain", "--untracked-files=no"]).is_some();

    let date = env("XUAN_BUILD_DATE")
        .or_else(|| {
            in_git
                .then(|| git(&["show", "-s", "--format=%cs", "HEAD"]))
                .flatten()
        })
        .or_else(|| {
            let epoch = env("SOURCE_DATE_EPOCH")?.parse().ok()?;
            Some(civil_date(epoch))
        })
        .filter(|_| commit.is_some());

    let channel = if env("XUAN_BUILD_CHANNEL").as_deref() == Some("release") {
        "release"
    } else {
        "dev"
    };

    println!("cargo:rustc-env=XUAN_BUILD_CHANNEL_VALUE={channel}");
    println!(
        "cargo:rustc-env=XUAN_BUILD_COMMIT_VALUE={}",
        commit.unwrap_or_default()
    );
    println!(
        "cargo:rustc-env=XUAN_BUILD_DATE_VALUE={}",
        date.unwrap_or_default()
    );
    println!("cargo:rustc-env=XUAN_BUILD_DIRTY_VALUE={}", u8::from(dirty));
}

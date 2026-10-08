//! Finds the locale files in a directory. `build.rs` includes this file to embed every
//! `assets/locales/<tag>.tsv`, so it uses only `std`.
use std::path::{Path, PathBuf};

/// `tag` as a canonical language tag (`zh-CN`, `sr-Latn`, `en-XA`), accepting `_` for `-` and
/// any letter case; `None` when it is not a language tag. A tag is a 2 or 3 letter language,
/// then optionally a 4 letter script, a 2 letter or 3 digit region and 5 to 8 character variants.
pub fn canonical_tag(tag: &str) -> Option<String> {
    let mut parts = tag.trim().split(['-', '_']);
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut canonical = language.to_ascii_lowercase();
    // Script, region and variants come in this order, each at most once but for variants.
    let mut stage = 0;
    for part in parts {
        canonical.push('-');
        let letters = part.chars().all(|c| c.is_ascii_alphabetic());
        if stage < 1 && part.len() == 4 && letters {
            stage = 1;
            canonical.push_str(&part[..1].to_ascii_uppercase());
            canonical.push_str(&part[1..].to_ascii_lowercase());
        } else if stage < 2
            && ((part.len() == 2 && letters)
                || (part.len() == 3 && part.chars().all(|c| c.is_ascii_digit())))
        {
            stage = 2;
            canonical.push_str(&part.to_ascii_uppercase());
        } else if (5..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric()) {
            stage = 3;
            canonical.push_str(&part.to_ascii_lowercase());
        } else {
            return None;
        }
    }
    Some(canonical)
}

/// What [`locale_files`] finds in a directory.
#[derive(Debug, Default)]
pub struct LocaleFiles {
    /// `(tag, path)` of each `<tag>.tsv`, sorted by tag.
    pub found: Vec<(String, PathBuf)>,
    /// The names of `.tsv` files skipped because their name is not a canonical language tag.
    pub skipped: Vec<String>,
}

/// The locale files in `dir`. Files other than `.tsv` are ignored.
#[cfg_attr(not(test), allow(dead_code))]
pub fn locale_files(dir: &Path) -> std::io::Result<LocaleFiles> {
    let mut files = LocaleFiles::default();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("tsv") || !path.is_file() {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        match canonical_tag(stem) {
            Some(tag) if tag == stem => files.found.push((tag, path)),
            _ => files.skipped.push(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
            ),
        }
    }
    files.found.sort();
    files.skipped.sort();
    Ok(files)
}

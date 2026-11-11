//! Checking for a newer release: Help → Check for Updates…, and once a day
//! when Settings → General → Check for updates is on (it is off unless the
//! user turns it on). A check is one GET of the GitHub releases API for
//! [`REPOSITORY`], over https with the plugin model downloader's client
//! ([`crate::plugins::models::Https`]), with no cookies, credentials or
//! anything about the user's files. The answer is bounded in size and time
//! and parsed defensively; nothing is downloaded or installed. The user
//! reads the release notes and opens the release page if they want it.
//!
//! Everything here is free of the UI so it can be tested without a network:
//! the transport is passed in. See "Updates" in `docs/USAGE.md`.
use std::{cmp::Ordering, io::Read, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::Deserialize;
use url::Url;

use crate::plugins::models::{Transport, follow};

/// The GitHub repository whose releases are checked: the one the builds are
/// published from. A fork's builds can name their own with
/// `XUAN_UPDATE_REPOSITORY=owner/name` at build time.
pub const REPOSITORY: &str = match option_env!("XUAN_UPDATE_REPOSITORY") {
    Some(repository) => repository,
    None => "jeffory/xuan",
};

/// How long the automatic check waits after the last one: a day.
pub const INTERVAL_SECONDS: u64 = 24 * 60 * 60;
/// The most a check may take, connecting and reading the answer included.
pub const TIME_LIMIT: Duration = Duration::from_secs(20);
/// The largest answer read. GitHub's for one release is a few kilobytes,
/// plus its notes and the list of downloads.
pub const MAX_RESPONSE: u64 = 1024 * 1024;
/// Release notes longer than this many bytes are cut, at a line break when
/// there is one.
pub const MAX_NOTES: usize = 64 * 1024;
/// A release title longer than this many bytes is cut.
pub const MAX_TITLE: usize = 200;
/// Longer tags are not versions Xuan publishes.
const MAX_TAG: usize = 64;

/// The API address of `repository`'s latest release. Drafts and pre-releases
/// are never the latest, so only stable releases are offered.
pub fn api_url(repository: &str) -> Result<Url> {
    ensure!(
        valid_repository(repository),
        "{repository:?} is not a GitHub repository name"
    );
    Ok(Url::parse(&format!(
        "https://api.github.com/repos/{repository}/releases/latest"
    ))?)
}

/// `owner/name`, each made of the characters GitHub allows.
fn valid_repository(repository: &str) -> bool {
    let part = |text: &str| {
        !text.is_empty()
            && text.len() <= 100
            && text != "."
            && text != ".."
            && (text.bytes()).all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    matches!(repository.split_once('/'), Some((owner, name)) if part(owner) && part(name))
}

/// The page of release `tag` on GitHub. Built here rather than taken from the
/// answer, so a link Xuan opens always goes to the repository's own releases.
pub fn release_page(repository: &str, tag: &str) -> String {
    format!("https://github.com/{repository}/releases/tag/{tag}")
}

/// The version a release tag names: `v0.6.0`, `0.6.0` or `v1.0.0-beta.2`.
/// `None` for anything else, including tags too long to be a version.
pub fn parse_version(tag: &str) -> Option<Version> {
    if tag.len() > MAX_TAG {
        return None;
    }
    let text = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    Version::parse(text).ok()
}

/// This build's version.
pub fn current_version() -> Option<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).ok()
}

/// Whether `candidate` comes after `current`. Build metadata (`+…`) does not
/// count, and a pre-release comes before its release (`1.0.0-rc.1` <
/// `1.0.0`), as semantic versioning orders them.
pub fn is_newer(candidate: &Version, current: &Version) -> bool {
    candidate.cmp_precedence(current) == Ordering::Greater
}

/// A published release, as much of it as Xuan shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The tag as published, such as `v0.6.0`.
    pub tag: String,
    /// The release's title; the tag when it has none.
    pub title: String,
    /// The release notes, Markdown, at most [`MAX_NOTES`] bytes.
    pub notes: String,
    /// The release page on GitHub; see [`release_page`].
    pub page: String,
}

/// The fields of GitHub's answer that are read. Everything else is ignored.
#[derive(Deserialize)]
struct Answer {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Read GitHub's answer for the latest release of `repository`.
pub fn parse_release(repository: &str, json: &[u8]) -> Result<Release> {
    let answer: Answer =
        serde_json::from_slice(json).context("The answer is not a GitHub release")?;
    ensure!(
        !answer.draft && !answer.prerelease,
        "The latest release is a draft or pre-release"
    );
    let tag = answer.tag_name.trim();
    let version = parse_version(tag)
        .with_context(|| format!("The release tag {:?} is not a version", truncate(tag, 80)))?;
    let title = (answer.name.as_deref().map(str::trim))
        .filter(|name| !name.is_empty())
        .map_or_else(
            || tag.to_owned(),
            |name| one_line(truncate(name, MAX_TITLE)),
        );
    let notes = cut_notes(answer.body.as_deref().unwrap_or_default());
    Ok(Release {
        version,
        tag: tag.to_owned(),
        title,
        notes,
        page: release_page(repository, tag),
    })
}

/// The longest start of `text` of at most `max` bytes that ends on a
/// character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` with control characters, line breaks included, made spaces.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Notes cut to [`MAX_NOTES`] bytes, at the last line break when there is
/// one, with Windows line ends made plain.
fn cut_notes(notes: &str) -> String {
    let notes = notes.replace("\r\n", "\n");
    if notes.len() <= MAX_NOTES {
        return notes;
    }
    let cut = truncate(&notes, MAX_NOTES);
    let cut = cut.rfind('\n').map_or(cut, |end| &cut[..end]);
    format!("{cut}\n\n…")
}

/// Read all of `body`, failing when it is longer than `limit` bytes.
pub fn read_limited(body: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    body.take(limit.saturating_add(1))
        .read_to_end(&mut data)
        .context("Cannot read the answer")?;
    ensure!(
        data.len() as u64 <= limit,
        "The answer is longer than {limit} bytes"
    );
    Ok(data)
}

/// Ask GitHub for the latest release of `repository`.
pub fn fetch_latest(transport: &dyn Transport, repository: &str) -> Result<Release> {
    let url = api_url(repository)?;
    let (url, reply) = follow(transport, url, MAX_RESPONSE, || false)?;
    match reply.status {
        200 => {}
        404 => bail!("{repository} has no published releases"),
        403 | 429 => bail!("GitHub is limiting requests; try again later"),
        status => bail!("{url} answered with HTTP status {status}"),
    }
    if let Some(length) = reply.length {
        ensure!(
            length <= MAX_RESPONSE,
            "The answer is longer than {MAX_RESPONSE} bytes"
        );
    }
    let json = read_limited(reply.body, MAX_RESPONSE)?;
    parse_release(repository, &json)
}

/// Whether the automatic check is due: never checked, a day since the last
/// check, or the clock moved back more than a day since (so a wrong clock
/// cannot stop checks for good).
pub fn due(last_check: Option<u64>, now: u64) -> bool {
    let Some(last) = last_check else {
        return true;
    };
    now.abs_diff(last) >= INTERVAL_SECONDS
}

/// Whether the automatic check shows `release` to someone running `current`
/// who chose Skip This Version for `skipped`: only a newer release, and not
/// the skipped one. A newer release than the skipped one is shown again.
pub fn offer(release: &Release, current: &Version, skipped: Option<&str>) -> bool {
    let skipped = skipped.and_then(parse_version);
    is_newer(&release.version, current)
        && skipped.is_none_or(|skipped| release.version.cmp_precedence(&skipped) != Ordering::Equal)
}

/// Seconds since 1970 (UTC); 0 when the clock is before then.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

/// One part of release notes, for drawing them without a Markdown engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// `#` to `######`: the level, 1 to 6, and the text.
    Heading(u8, String),
    /// A `-`, `*` or `+` list item, or a numbered one (with its number), and
    /// how many levels it is indented.
    Item {
        marker: Option<String>,
        depth: usize,
        text: String,
    },
    /// A fenced code block, as written.
    Code(String),
    /// Consecutive lines of text, joined.
    Paragraph(String),
    /// `---`, `***` or `___`.
    Rule,
}

/// The blocks of the Markdown `notes`, with links shown as their text and
/// emphasis and code markers removed. Images, HTML and comments are left
/// out; nothing in the notes is fetched or followed.
pub fn blocks(notes: &str) -> Vec<Block> {
    let notes = strip_comments(notes);
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut code: Option<Vec<&str>> = None;
    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join(" ")));
            paragraph.clear();
        }
    };
    for line in notes.lines() {
        let trimmed = line.trim();
        if let Some(lines) = &mut code {
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                blocks.push(Block::Code(lines.join("\n")));
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut paragraph, &mut blocks);
            code = Some(Vec::new());
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some((level, text)) = heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(level, inline(text)));
            continue;
        }
        if let Some((marker, text)) = list_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let indent = line.len() - line.trim_start().len();
            blocks.push(Block::Item {
                marker,
                depth: (indent / 2).min(4),
                text: inline(text),
            });
            continue;
        }
        let text = inline(trimmed.trim_start_matches('>').trim());
        if !text.is_empty() {
            paragraph.push(text);
        }
    }
    flush(&mut paragraph, &mut blocks);
    if let Some(lines) = code {
        blocks.push(Block::Code(lines.join("\n")));
    }
    blocks
}

/// `notes` without HTML comments, such as the one GitHub's generated notes
/// start with.
fn strip_comments(notes: &str) -> String {
    let mut out = String::with_capacity(notes.len());
    let mut rest = notes;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") {
            Some(end) => &rest[start + end + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn is_rule(line: &str) -> bool {
    let mut marks = line.chars().filter(|c| !c.is_whitespace());
    let Some(first) = marks.next() else {
        return false;
    };
    let rest: Vec<char> = marks.collect();
    matches!(first, '-' | '*' | '_') && rest.len() >= 2 && rest.iter().all(|&c| c == first)
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    let text = &line[level..];
    ((1..=6).contains(&level) && (text.is_empty() || text.starts_with(' ')))
        .then(|| (level as u8, text.trim().trim_end_matches('#').trim_end()))
}

fn list_item(line: &str) -> Option<(Option<String>, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(bullet) {
            return Some((None, text.trim()));
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &line[digits..];
        if let Some(text) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((Some(format!("{}.", &line[..digits])), text.trim()));
        }
    }
    None
}

/// A line of Markdown as plain text: `[text](url)` becomes `text`, images
/// and HTML tags are dropped, and the `**`, `~~` and `` ` `` markers are
/// removed. Single `*` and `_` stay: they are as often text as emphasis.
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '!'
            && let Some((_, end)) = link(&rest[1..])
        {
            rest = &rest[1 + end..];
            continue;
        }
        if c == '['
            && let Some((label, end)) = link(rest)
        {
            out.push_str(label);
            rest = &rest[end..];
            continue;
        }
        if c == '<'
            && let Some(end) = rest.find('>')
            && (rest[1..].chars().next()).is_some_and(|c| c.is_ascii_alphabetic() || c == '/')
        {
            // An autolink keeps its address; an HTML tag goes.
            let inner = &rest[1..end];
            if inner.starts_with("https://") || inner.starts_with("http://") {
                out.push_str(inner);
            }
            rest = &rest[end + 1..];
            continue;
        }
        if let Some(marker) = ["**", "~~", "`"].into_iter().find(|m| rest.starts_with(m)) {
            rest = &rest[marker.len()..];
            continue;
        }
        out.push(if c.is_control() { ' ' } else { c });
        rest = &rest[c.len_utf8()..];
    }
    out.trim().to_owned()
}

/// When `text` starts with the link `[label](url)`: its label, and where the
/// link ends.
fn link(text: &str) -> Option<(&str, usize)> {
    let label = text.strip_prefix('[')?;
    let close = label.find(']')?;
    if label[..close].contains('[') {
        return None;
    }
    let target = label[close + 1..].strip_prefix('(')?;
    let end = target.find(')')?;
    Some((&label[..close], 1 + close + 2 + end + 1))
}

#[cfg(test)]
mod tests;

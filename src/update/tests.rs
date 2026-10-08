//! The update check against a fake transport, which covers every rule
//! without a network, and against a local server that never answers, which
//! checks that [`Https::with_limit`] gives up in time.
use std::{
    collections::HashMap,
    io::Cursor,
    net::TcpListener,
    sync::Mutex,
    time::{Duration, Instant},
};

use super::*;
use crate::plugins::models::{Https, Reply};

const REPO: &str = "jeffory/xuan";
const API: &str = "https://api.github.com/repos/jeffory/xuan/releases/latest";

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn answer(tag: &str, notes: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "tag_name": tag,
        "name": format!("Xuan {tag}"),
        "body": notes,
        "draft": false,
        "prerelease": false,
        "html_url": "https://evil.example/download",
        "assets": [{"name": "xuan.zip", "browser_download_url": "https://evil.example/x"}],
    }))
    .unwrap()
}

/// What the fake serves at one URL.
#[derive(Clone)]
enum Page {
    /// A status and body, with a matching `Content-Length`.
    Body(u16, Vec<u8>),
    /// A body with no `Content-Length` that never ends.
    Endless,
    /// A body whose `Content-Length` claims this much.
    Claims(u64),
    Redirect(String),
    /// The transport fails, as it does offline.
    Offline,
}

#[derive(Default)]
struct Fake {
    pages: HashMap<String, Page>,
    requested: Mutex<Vec<String>>,
}

impl Fake {
    fn with(mut self, url: &str, page: Page) -> Self {
        self.pages.insert(url.into(), page);
        self
    }

    fn requested(&self) -> Vec<String> {
        self.requested.lock().unwrap().clone()
    }
}

struct Endless;

impl Read for Endless {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        buffer.fill(b' ');
        Ok(buffer.len())
    }
}

impl Transport for Fake {
    fn get(&self, url: &Url, _size: u64) -> Result<Reply> {
        self.requested.lock().unwrap().push(url.to_string());
        let page = self.pages.get(url.as_str()).context("no such page")?;
        Ok(match page {
            Page::Body(status, data) => Reply {
                status: *status,
                location: None,
                length: Some(data.len() as u64),
                body: Box::new(Cursor::new(data.clone())),
            },
            Page::Endless => Reply {
                status: 200,
                location: None,
                length: None,
                body: Box::new(Endless),
            },
            Page::Claims(length) => Reply {
                status: 200,
                location: None,
                length: Some(*length),
                body: Box::new(std::io::empty()),
            },
            Page::Redirect(to) => Reply {
                status: 301,
                location: Some(to.clone()),
                length: None,
                body: Box::new(std::io::empty()),
            },
            Page::Offline => bail!("Cannot connect: network unreachable"),
        })
    }
}

#[test]
fn tags_are_versions_with_or_without_a_v() {
    assert_eq!(parse_version("v0.6.0"), Some(v("0.6.0")));
    assert_eq!(parse_version("V0.6.0"), Some(v("0.6.0")));
    assert_eq!(parse_version("0.6.0"), Some(v("0.6.0")));
    assert_eq!(parse_version("v1.0.0-beta.2"), Some(v("1.0.0-beta.2")));
    assert_eq!(parse_version("v1.0.0+build.5"), Some(v("1.0.0+build.5")));
    for malformed in [
        "",
        "v",
        "vv1.0.0",
        "latest",
        "v1.0",
        "1",
        "v1.0.0.0",
        "v01.0.0",
        " v1.0.0",
        "v1.0.0 ",
        "v-1.0.0",
        "v1.0.0-",
        "v1.0.0-beta..1",
        "v1.0.0\n",
        "v１.0.0",
        "v99999999999999999999.0.0",
    ] {
        assert_eq!(parse_version(malformed), None, "{malformed:?}");
    }
    let long = format!("v1.0.0-{}", "a".repeat(100));
    assert_eq!(parse_version(&long), None);
    assert!(current_version().is_some());
}

#[test]
fn newer_follows_semantic_versioning_precedence() {
    assert!(is_newer(&v("0.6.0"), &v("0.5.0")));
    assert!(is_newer(&v("0.5.1"), &v("0.5.0")));
    assert!(is_newer(&v("1.0.0"), &v("0.99.99")));
    assert!(is_newer(&v("0.10.0"), &v("0.9.0")), "numbers, not text");
    assert!(!is_newer(&v("0.5.0"), &v("0.5.0")));
    assert!(!is_newer(&v("0.4.0"), &v("0.5.0")));
    // A pre-release comes before its release, and after the one before.
    assert!(is_newer(&v("1.0.0"), &v("1.0.0-rc.1")));
    assert!(!is_newer(&v("1.0.0-rc.1"), &v("1.0.0")));
    assert!(is_newer(&v("1.0.0-rc.1"), &v("0.9.0")));
    assert!(is_newer(&v("1.0.0-rc.2"), &v("1.0.0-rc.1")));
    assert!(is_newer(&v("1.0.0-rc.10"), &v("1.0.0-rc.9")));
    // Build metadata does not make a version newer.
    assert!(!is_newer(&v("0.5.0+abc"), &v("0.5.0")));
    assert!(!is_newer(&v("0.5.0"), &v("0.5.0+abc")));
}

#[test]
fn a_release_is_read_and_its_page_is_built_not_taken_from_the_answer() {
    let release = parse_release(REPO, &answer("v0.6.0", "## New\r\n- Things")).unwrap();
    assert_eq!(
        release,
        Release {
            version: v("0.6.0"),
            tag: "v0.6.0".into(),
            title: "Xuan v0.6.0".into(),
            notes: "## New\n- Things".into(),
            page: "https://github.com/jeffory/xuan/releases/tag/v0.6.0".into(),
        }
    );
    // Missing or null title and notes.
    let bare = parse_release(REPO, br#"{"tag_name": "0.7.0", "name": null}"#).unwrap();
    assert_eq!((bare.title.as_str(), bare.notes.as_str()), ("0.7.0", ""));
    let blank = parse_release(REPO, br#"{"tag_name": "0.7.0", "name": "  "}"#).unwrap();
    assert_eq!(blank.title, "0.7.0");
    // A title is one line.
    let title = parse_release(REPO, br#"{"tag_name": "0.7.0", "name": "A\nB\u0000"}"#).unwrap();
    assert_eq!(title.title, "A B ");
}

#[test]
fn hostile_answers_are_errors_not_panics() {
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let deep_object = format!(
        r#"{{"tag_name": "v1.0.0", "x": {}{}}}"#,
        "{\"a\":".repeat(10_000),
        "}".repeat(10_000)
    );
    let cases: Vec<&[u8]> = vec![
        b"",
        b"null",
        b"[]",
        b"\"v1.0.0\"",
        b"{}",
        b"{\"tag_name\": null}",
        b"{\"tag_name\": 1}",
        b"{\"tag_name\": \"v1.0.0\"",
        b"{\"tag_name\": \"v1.0.0\", \"body\": 5}",
        b"{\"tag_name\": \"v1.0.0\", \"name\": []}",
        b"{\"tag_name\": \"v1.0.0\", \"draft\": \"no\"}",
        b"{\"tag_name\": \"v1.0.0\", \"draft\": true}",
        b"{\"tag_name\": \"v1.0.0\", \"prerelease\": true}",
        b"{\"tag_name\": \"nightly\"}",
        b"{\"tag_name\": \"../../evil\"}",
        b"{\"tag_name\": \"v1.0.0?x=<script>\"}",
        b"{\"tag_name\": \"\xff\xfe\"}",
        b"\xef\xbb\xbf{\"tag_name\": \"v1.0.0\"}",
        b"<html>rate limited</html>",
        deep.as_bytes(),
        deep_object.as_bytes(),
    ];
    for json in cases {
        assert!(
            parse_release(REPO, json).is_err(),
            "{:?}",
            String::from_utf8_lossy(&json[..json.len().min(60)])
        );
    }
    // A bad tag is quoted in the error, but not all of a long one.
    let long = format!(r#"{{"tag_name": "{}"}}"#, "x".repeat(10_000));
    let error = parse_release(REPO, long.as_bytes())
        .unwrap_err()
        .to_string();
    assert!(error.len() < 200, "{error}");
}

#[test]
fn long_titles_and_notes_are_cut_on_character_and_line_boundaries() {
    let title = "標".repeat(MAX_TITLE);
    let notes = format!("{}\n", "说明".repeat(10)).repeat(MAX_NOTES / 10);
    let json = serde_json::to_vec(&serde_json::json!({
        "tag_name": "v1.0.0", "name": title, "body": notes,
    }))
    .unwrap();
    let release = parse_release(REPO, &json).unwrap();
    assert!(release.title.len() <= MAX_TITLE);
    assert!(release.title.chars().all(|c| c == '標'));
    assert!(release.notes.len() <= MAX_NOTES + 8);
    assert!(release.notes.ends_with("说明\n\n…"), "cut at a line break");
    // One enormous line is cut inside it.
    let json = serde_json::to_vec(&serde_json::json!({
        "tag_name": "v1.0.0", "body": "é".repeat(MAX_NOTES),
    }))
    .unwrap();
    let release = parse_release(REPO, &json).unwrap();
    assert!(release.notes.len() <= MAX_NOTES + 8);
}

#[test]
fn reading_stops_past_the_limit() {
    assert_eq!(read_limited(Cursor::new(b"12345"), 5).unwrap(), b"12345");
    assert!(read_limited(Cursor::new(b"123456"), 5).is_err());
    assert!(read_limited(Endless, 1000).is_err());
    assert_eq!(read_limited(std::io::empty(), 0).unwrap(), b"");
    assert!(read_limited(Cursor::new(b"1"), 0).is_err());
}

#[test]
fn the_latest_release_is_fetched_with_one_get() {
    let fake = Fake::default().with(API, Page::Body(200, answer("v0.6.0", "Notes")));
    let release = fetch_latest(&fake, REPO).unwrap();
    assert_eq!(release.version, v("0.6.0"));
    assert_eq!(fake.requested(), [API]);
}

#[test]
fn failures_are_errors_with_a_reason() {
    let error = |page: Page| {
        let fake = Fake::default().with(API, page);
        fetch_latest(&fake, REPO).unwrap_err().to_string()
    };
    assert!(error(Page::Offline).contains("unreachable"));
    assert!(error(Page::Body(404, b"{}".to_vec())).contains("no published releases"));
    assert!(error(Page::Body(403, b"{}".to_vec())).contains("limiting"));
    assert!(error(Page::Body(429, b"{}".to_vec())).contains("limiting"));
    assert!(error(Page::Body(500, b"{}".to_vec())).contains("status 500"));
    assert!(error(Page::Body(200, b"not json".to_vec())).contains("not a GitHub release"));
    // Too long, whether the server says so or not.
    assert!(error(Page::Claims(MAX_RESPONSE + 1)).contains("longer than"));
    assert!(error(Page::Endless).contains("longer than"));
    let big = answer("v1.0.0", &"x".repeat(MAX_RESPONSE as usize));
    assert!(error(Page::Body(200, big)).contains("longer than"));
}

#[test]
fn redirects_stay_on_https() {
    // A renamed repository redirects; that is followed.
    let moved = "https://api.github.com/repositories/1/releases/latest";
    let fake = Fake::default()
        .with(API, Page::Redirect(moved.into()))
        .with(moved, Page::Body(200, answer("v0.6.0", "")));
    assert_eq!(fetch_latest(&fake, REPO).unwrap().version, v("0.6.0"));
    assert_eq!(fake.requested(), [API, moved]);
    // To http it is not, and nothing is fetched from there.
    let insecure = "http://api.github.com/x";
    let fake = Fake::default()
        .with(API, Page::Redirect(insecure.into()))
        .with(insecure, Page::Body(200, answer("v0.6.0", "")));
    let error = fetch_latest(&fake, REPO).unwrap_err().to_string();
    assert!(error.contains("not https"), "{error}");
    assert_eq!(fake.requested(), [API]);
    // A redirect loop ends.
    let fake = Fake::default().with(API, Page::Redirect(API.into()));
    assert!(fetch_latest(&fake, REPO).is_err());
    assert_eq!(fake.requested().len(), 6);
}

#[test]
fn only_plain_repository_names_are_asked_about() {
    assert_eq!(api_url(REPO).unwrap().as_str(), API);
    assert!(api_url("some-one/my.fork_2").is_ok());
    for bad in [
        "",
        "xuan",
        "/xuan",
        "jeffory/",
        "a/b/c",
        "../x",
        "a/..",
        "a b/c",
        "a/b?x=1",
        "a/b#c",
        "a@evil.example/b",
        "a:1/b",
    ] {
        assert!(api_url(bad).is_err(), "{bad:?}");
        let fake = Fake::default();
        assert!(fetch_latest(&fake, bad).is_err());
        assert!(fake.requested().is_empty(), "{bad:?}");
    }
    assert!(api_url(REPOSITORY).is_ok(), "the built-in repository");
}

#[test]
fn the_check_is_due_once_a_day() {
    let day = INTERVAL_SECONDS;
    let now = 1_790_000_000;
    assert!(due(None, now));
    assert!(due(None, 0));
    assert!(!due(Some(now), now));
    assert!(!due(Some(now - day + 1), now));
    assert!(due(Some(now - day), now));
    assert!(due(Some(0), now));
    // A clock that went back a little does not check again; one that went back
    // further does, so a wrong clock cannot stop checks for good.
    assert!(!due(Some(now + 60), now));
    assert!(due(Some(now + day), now));
    assert!(due(Some(u64::MAX), now));
    assert!(due(Some(u64::MAX), 0));
    assert!(due(Some(0), u64::MAX));
}

#[test]
fn skip_this_version_suppresses_that_version_only() {
    let release = |version: &str| Release {
        version: v(version),
        tag: format!("v{version}"),
        title: String::new(),
        notes: String::new(),
        page: String::new(),
    };
    let current = v("0.5.0");
    assert!(offer(&release("0.6.0"), &current, None));
    assert!(!offer(&release("0.5.0"), &current, None));
    assert!(!offer(&release("0.4.0"), &current, None));
    assert!(!offer(&release("0.6.0"), &current, Some("0.6.0")));
    assert!(!offer(&release("0.6.0"), &current, Some("v0.6.0")));
    assert!(offer(&release("0.6.1"), &current, Some("0.6.0")));
    assert!(offer(&release("0.7.0"), &current, Some("0.6.0")));
    // A skipped version that is not one skips nothing.
    assert!(offer(&release("0.6.0"), &current, Some("whatever")));
    assert!(offer(&release("0.6.0"), &current, Some("")));
}

#[test]
fn https_gives_up_on_a_server_that_never_answers() {
    // Accepts connections and holds them open without a word.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().take(4) {
            held.push(stream);
        }
        std::thread::sleep(Duration::from_secs(30));
    });
    let https = Https::with_certificates(&[]).with_limit(Duration::from_millis(300));
    let url = Url::parse(&format!("https://localhost:{port}/releases/latest")).unwrap();
    let start = Instant::now();
    assert!(https.get(&url, MAX_RESPONSE).is_err());
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn release_notes_become_plain_blocks() {
    let notes = "<!-- Release notes generated using configuration in .github/release.yml -->\n\
        ## What's Changed\n\
        * feat: **bold** and `code` by @someone in https://github.com/jeffory/xuan/pull/12\n\
        * See [the docs](https://example.com/docs) ![shot](https://example.com/a.png)\n\
        \x20\x20- nested <b>item</b>\n\
        1. first\n\
        \n\
        Some text\n\
        on two lines.\n\
        > quoted\n\
        \n\
        ---\n\
        ```sh\n\
        xuan --demo\n\
        ```\n\
        #hashtag stays text\n\
        **Full Changelog**: <https://github.com/jeffory/xuan/compare/v0.4.0...v0.5.0>";
    assert_eq!(
        blocks(notes),
        [
            Block::Heading(2, "What's Changed".into()),
            Block::Item {
                marker: None,
                depth: 0,
                text: "feat: bold and code by @someone in https://github.com/jeffory/xuan/pull/12"
                    .into()
            },
            Block::Item {
                marker: None,
                depth: 0,
                text: "See the docs".into()
            },
            Block::Item {
                marker: None,
                depth: 1,
                text: "nested item".into()
            },
            Block::Item {
                marker: Some("1.".into()),
                depth: 0,
                text: "first".into()
            },
            Block::Paragraph("Some text on two lines. quoted".into()),
            Block::Rule,
            Block::Code("xuan --demo".into()),
            Block::Paragraph(
                "#hashtag stays text Full Changelog: https://github.com/jeffory/xuan/compare/v0.4.0...v0.5.0"
                    .into()
            ),
        ]
    );
    // An unclosed fence or comment keeps what is there and drops nothing else.
    assert_eq!(
        blocks("text\n```\ncode"),
        [Block::Paragraph("text".into()), Block::Code("code".into())]
    );
    assert_eq!(
        blocks("before <!-- never closed\n# gone"),
        [Block::Paragraph("before".into())]
    );
    assert!(blocks("").is_empty());
}

#[test]
fn markdown_that_is_not_quite_markdown_does_not_panic() {
    for notes in [
        "[",
        "[]",
        "[](",
        "[a](",
        "![",
        "!",
        "![](",
        "<",
        "<a",
        "<>",
        "< b>",
        "*",
        "**",
        "~",
        "~~~",
        "`",
        "```",
        "#",
        "#######",
        "- ",
        "-",
        "1.",
        "1234567890. x",
        "<!--",
        "-->",
        "<!---->",
        "[a]]](b)",
        "[[a](b)](c)",
        "é[ü](ñ)ß<ä>`ö`",
        "\u{0}\u{7}\u{1b}[31m",
        "\t\t- tabbed",
    ] {
        let _ = blocks(notes);
        let _ = blocks(&notes.repeat(1000));
    }
    assert_eq!(
        blocks("é[ü](ñ)ß<ä>`ö`"),
        [Block::Paragraph("éüß<ä>ö".into())]
    );
    assert_eq!(blocks("[a]]](b)"), [Block::Paragraph("[a]]](b)".into())]);
}

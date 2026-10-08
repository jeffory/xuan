//! Checks the locale files against the source: every `tr("…")`, `tr_args("…", …)` and
//! `tr_plural("…", …)` key is in `en.tsv`, every key in `en.tsv` is still a string in `src/`, and
//! every locale reads cleanly, translates only those keys and keeps their placeholders.
//! `scripts/check-locales.sh` reports what each locale has left to translate.
use super::{
    ENGLISH, british,
    catalog::{Catalog, escape},
    placeholders,
    plural::{self, Plural},
};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
};

/// A string literal: its value and the byte offset of its opening quote or prefix.
#[derive(Debug, PartialEq, Eq)]
struct Literal {
    start: usize,
    text: String,
}

/// The string literals of Rust source, skipping comments and character literals. Unusual
/// escapes are kept as written rather than rejected; the source has compiled, so they are rare.
fn string_literals(source: &str) -> Vec<Literal> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut i = 0;
    let ident =
        |at: usize| at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
    while i < bytes.len() {
        let rest = &source[i..];
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            let mut depth = 0;
            let mut j = i;
            while j < bytes.len() {
                if source[j..].starts_with("/*") {
                    depth += 1;
                    j += 2;
                } else if source[j..].starts_with("*/") {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            i = j;
        } else if let Some((prefix, hashes)) = raw_string_start(rest).filter(|_| !ident(i)) {
            let body = i + prefix;
            let close = format!("\"{}", "#".repeat(hashes));
            let end = source[body..]
                .find(&close)
                .map_or(bytes.len(), |e| body + e);
            literals.push(Literal {
                start: i,
                text: source[body..end].into(),
            });
            i = (end + close.len()).min(bytes.len());
        } else if rest.starts_with('"') || (rest.starts_with("b\"") && !ident(i)) {
            let start = i;
            i += if rest.starts_with('b') { 2 } else { 1 };
            let mut text = String::new();
            let mut chars = source[i..].char_indices();
            let mut end = bytes.len();
            while let Some((offset, c)) = chars.next() {
                match c {
                    '"' => {
                        end = i + offset + 1;
                        break;
                    }
                    '\\' => match chars.next().map(|(_, c)| c) {
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        Some('r') => text.push('\r'),
                        Some('0') => text.push('\0'),
                        Some('u') => {
                            let digits: String = chars
                                .by_ref()
                                .map(|(_, c)| c)
                                .take_while(|&c| c != '}')
                                .filter(|c| c.is_ascii_hexdigit())
                                .collect();
                            let code = u32::from_str_radix(&digits, 16).ok();
                            text.extend(code.and_then(char::from_u32));
                        }
                        Some('x') => {
                            let digits: String = chars.by_ref().take(2).map(|(_, c)| c).collect();
                            let code = u32::from_str_radix(&digits, 16).ok();
                            text.extend(code.and_then(char::from_u32));
                        }
                        // A line continuation skips the line break and the next line's indent.
                        Some('\n') => {
                            let next = &source[i + offset + 2..];
                            let skipped = next
                                .find(|c: char| !c.is_whitespace())
                                .unwrap_or(next.len());
                            i += offset + 2 + skipped;
                            // Offsets count from `i` again.
                            chars = source[i..].char_indices();
                        }
                        Some(c) => text.push(c),
                        None => {}
                    },
                    c => text.push(c),
                }
            }
            literals.push(Literal { start, text });
            i = end;
        } else if rest.starts_with('\'') {
            // A character literal, or a lifetime or label, which has no closing quote.
            let mut chars = rest.char_indices().skip(1);
            i += match chars.next() {
                Some((_, '\\')) => rest
                    .get(3..)
                    .and_then(|r| r.find('\''))
                    .map_or(1, |e| e + 4),
                Some((offset, c)) if rest[offset + c.len_utf8()..].starts_with('\'') => {
                    offset + c.len_utf8() + 1
                }
                _ => 1,
            };
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    literals
}

/// For `r"`, `r#"`, `br##"` and so on: the length of the opening and the number of hashes.
fn raw_string_start(rest: &str) -> Option<(usize, usize)> {
    let after = rest.strip_prefix("br").or_else(|| rest.strip_prefix('r'))?;
    let hashes = after.len() - after.trim_start_matches('#').len();
    after[hashes..]
        .starts_with('"')
        .then(|| (rest.len() - after.len() + hashes + 1, hashes))
}

/// The keys of the translation calls in `source`: literals that are the first argument of `tr`,
/// `tr_args` or `tr_plural`.
fn translated_keys(source: &str) -> Vec<String> {
    string_literals(source)
        .into_iter()
        .filter(|literal| {
            let before = source[..literal.start].trim_end();
            ["tr(", "tr_args(", "tr_plural("].iter().any(|call| {
                before
                    .strip_suffix(call)
                    .is_some_and(|head| !head.ends_with(|c: char| c.is_alphanumeric() || c == '_'))
            })
        })
        .map(|literal| british(&literal.text).to_owned())
        .collect()
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}

/// `(file, source)` of every Rust file in `src/`.
fn sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.strip_prefix(root()).unwrap().display().to_string();
            // As rustc reads it, whatever line endings the checkout has.
            let source = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
            (name, source)
        })
        .collect()
}

fn english() -> Catalog {
    let (catalog, problems) = Catalog::parse(include_str!("../../assets/locales/en.tsv"));
    assert!(problems.is_empty(), "en.tsv: {problems:?}");
    catalog
}

#[test]
fn literals_are_read_like_rust() {
    let source = r####"
        // tr("in a comment")
        /* tr("in a /* nested */ block") */
        let a = tr("plain \"quoted\" \\ \n \t \u{1F600} \x41");
        let b = tr(r#"raw "with" quotes"#) + tr(r"raw");
        let c = ('"', '\'', '\u{22}', 'x', b'"');
        fn f<'a>(x: &'a str) -> &'a str { tr_args("after a lifetime {x}", &[]) }
        let d = attr("not a call");
        let e = i18n::tr(
            "continued \
             line",
        );
        let f = tr_plural("{n} cats", n) + br##"bytes"## + "中文";
        let g = tr("unterminated
    "####;
    let texts: Vec<_> = string_literals(source)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(
        texts,
        [
            "plain \"quoted\" \\ \n \t 😀 A",
            "raw \"with\" quotes",
            "raw",
            "after a lifetime {x}",
            "not a call",
            "continued line",
            "{n} cats",
            "bytes",
            "中文",
            "unterminated\n    ",
        ]
    );
    assert_eq!(
        translated_keys(source),
        [
            "plain \"quoted\" \\ \n \t 😀 A",
            "raw \"with\" quotes",
            "raw",
            "after a lifetime {x}",
            "continued line",
            "{n} cats",
            "unterminated\n    ",
        ]
    );
    for broken in [
        "\"", "r#\"", "'", "/*", "\"\\", "\"\\u{", "\"\\x", "b", "'\\",
    ] {
        string_literals(broken);
    }
}

#[test]
fn every_translated_string_is_in_the_english_catalog() {
    let english = english();
    let mut missing = BTreeSet::new();
    for (file, source) in sources() {
        for key in translated_keys(&source) {
            if english.get(&key).is_none() {
                missing.insert(format!("{}  ({file})", escape(&key)));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "Add these strings to assets/locales/en.tsv, one per line (a plural's singular goes \
         on a line of its own: \"{{n}} things<TAB>{{n}} thing<TAB>one\"):\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn every_english_key_is_still_used() {
    let literals: HashSet<String> = sources()
        .iter()
        .flat_map(|(_, source)| string_literals(source))
        .map(|literal| literal.text)
        .collect();
    let stale: Vec<_> = english()
        .keys
        .into_iter()
        .filter(|key| !literals.contains(key))
        .map(|key| escape(&key))
        .collect();
    assert!(
        stale.is_empty(),
        "No string in src/ matches these keys of assets/locales/en.tsv; remove them, and their \
         translations, or update them to the new text:\n{}",
        stale.join("\n")
    );
}

/// Everything wrong with a locale file, given the English catalog.
fn locale_problems(tag: &str, source: &str, english: &Catalog) -> Vec<String> {
    let (catalog, problems) = Catalog::parse(source);
    let mut found: Vec<_> = problems.iter().map(ToString::to_string).collect();
    if tag != ENGLISH && catalog.name.is_none() {
        found.push("no @name line giving the language's own name".into());
    }
    let allowed = plural::categories(tag);
    for key in &catalog.keys {
        let Some(source) = english.get(key) else {
            found.push(format!("“{}” is not in en.tsv", escape(key)));
            continue;
        };
        let forms = catalog.get(key).unwrap();
        let plural = source.get(Plural::One).is_some();
        for category in forms.categories() {
            let text = forms.get(category).unwrap();
            if category != Plural::Other && !plural {
                found.push(format!(
                    "“{}” has a {} form, but en.tsv gives it no singular, so it is not a plural",
                    escape(key),
                    category.name()
                ));
            } else if plural && category != Plural::Other && !allowed.contains(&category) {
                found.push(format!(
                    "“{}”: {tag} has no {} plural (it has {})",
                    escape(key),
                    category.name(),
                    allowed
                        .iter()
                        .map(|p| p.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if tag == ENGLISH && category != Plural::One {
                found.push(format!(
                    "“{}”: en.tsv lists keys, and gives only the singular of plurals",
                    escape(key)
                ));
            }
            if placeholders(text) != placeholders(key) {
                found.push(format!(
                    "“{}”: the translation's placeholders {:?} differ from the key's {:?}",
                    escape(key),
                    placeholders(text),
                    placeholders(key)
                ));
            }
        }
    }
    found
}

#[test]
fn every_locale_reads_cleanly_and_translates_known_keys() {
    let english = english();
    let fixture = ("en-XA", include_str!("../../testdata/locales/en-XA.tsv"));
    for (tag, source) in super::bundled_sources().iter().chain([&fixture]) {
        let problems = locale_problems(tag, source, &english);
        assert!(
            problems.is_empty(),
            "assets/locales/{tag}.tsv:\n{}",
            problems.join("\n")
        );
    }
}

#[test]
fn locale_checks_catch_mistakes() {
    let english = Catalog::parse("Save\n{n} files\t{n} file\tone\n{} of {}\n").0;
    assert_eq!(
        locale_problems(
            "uk",
            "Save\tx\nSave\ty\nOpen\tx\n{n} files\t{} x\tfew\n{n} files\tx\ttwo\n\
             Save\tx\tone\n{} of {}\t{}\n",
            &english
        ),
        [
            "line 2: “Save” is listed twice",
            "no @name line giving the language's own name",
            "“Save” has a one form, but en.tsv gives it no singular, so it is not a plural",
            "“Open” is not in en.tsv",
            "“{n} files”: uk has no two plural (it has one, few, many)",
            "“{n} files”: the translation's placeholders [] differ from the key's [\"{n}\"]",
            "“{n} files”: the translation's placeholders [\"{}\"] differ from the key's [\"{n}\"]",
            "“{} of {}”: the translation's placeholders [\"{}\"] differ from the key's [\"{}\", \"{}\"]",
        ]
    );
    assert_eq!(
        locale_problems("en", "Save\tSauver\n", &english),
        ["“Save”: en.tsv lists keys, and gives only the singular of plurals"]
    );
}

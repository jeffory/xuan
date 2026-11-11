//! UI translations. Document data and command identifiers stay language independent.
//!
//! UI text is written in English and passed through [`tr`], [`tr_args`] or [`tr_plural`], which
//! look it up in the current language. Every `assets/locales/<tag>.tsv` is built in as a
//! language (see [`catalog`] for the format and `docs/TRANSLATING.md` for the process). `en.tsv`
//! lists every English key and the singular of each plural; any string a locale leaves out is
//! shown in English.
pub mod catalog;
mod locale_files;
pub mod plural;
#[cfg(test)]
mod source_keys;

use crate::config::Language;
use catalog::Catalog;
use plural::Plural;
use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};

pub(crate) use locale_files::canonical_tag;

/// `(tag, contents)` of each `assets/locales/<tag>.tsv`, sorted by tag; written by `build.rs`.
static BUNDLED: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/locales.rs"));

/// The tag of the language the keys are written in.
pub const ENGLISH: &str = "en";

/// An interface language.
#[derive(Debug)]
struct Locale {
    tag: String,
    catalog: Catalog,
}

impl Locale {
    /// Lines that cannot be read are skipped; the tests check the bundled files are clean.
    fn new(tag: &str, source: &str) -> Self {
        Self {
            tag: tag.into(),
            catalog: Catalog::parse(source).0,
        }
    }
}

fn bundled() -> &'static [Locale] {
    static LOCALES: OnceLock<Vec<Locale>> = OnceLock::new();
    LOCALES.get_or_init(|| {
        BUNDLED
            .iter()
            .map(|(tag, source)| Locale::new(tag, source))
            .collect()
    })
}

thread_local! {
    // UI threads select their own language, so headless tests remain isolated. `None` is English.
    static LANGUAGE: Cell<Option<&'static Locale>> = const { Cell::new(None) };
    static ADDED: RefCell<Vec<&'static Locale>> = const { RefCell::new(Vec::new()) };
}

fn find(tag: &str) -> Option<&'static Locale> {
    ADDED
        .with_borrow(|added| added.iter().rev().copied().find(|l| l.tag == tag))
        .or_else(|| bundled().iter().find(|l| l.tag == tag))
}

/// Adds a locale on this thread only, as if `<tag>.tsv` were built in, in place of a built-in one
/// with the same tag. Lets tests use a locale fixture without shipping it.
#[doc(hidden)]
pub fn add_locale(tag: &str, source: &str) {
    let Some(tag) = canonical_tag(tag) else {
        return;
    };
    let locale: &'static Locale = Box::leak(Box::new(Locale::new(&tag, source)));
    ADDED.with_borrow_mut(|added| added.push(locale));
}

/// The contents of each built-in locale file, by tag.
pub fn bundled_sources() -> &'static [(&'static str, &'static str)] {
    BUNDLED
}

/// The languages to offer: English, then the others by tag.
pub fn languages() -> Vec<Language> {
    let mut tags: Vec<&str> = bundled().iter().map(|l| l.tag.as_str()).collect();
    ADDED.with_borrow(|added| tags.extend(added.iter().map(|l| l.tag.as_str())));
    tags.push(ENGLISH);
    tags.sort_by_key(|&tag| (tag != ENGLISH, tag));
    tags.dedup();
    tags.into_iter().map(Language::new).collect()
}

/// The name `language` has for itself, when this build has it.
pub fn native_name(language: &Language) -> Option<&'static str> {
    if language.tag() == ENGLISH {
        return Some("English");
    }
    find(language.tag())?.catalog.name.as_deref()
}

/// Shows the interface in `language` on this thread: English when this build lacks it.
pub fn set_language(language: &Language) {
    LANGUAGE.set(find(language.tag()).filter(|l| l.tag != ENGLISH));
}

/// Names that document data and file formats spell the American way (blend modes, adjustment
/// and effect kinds), shown with the UI's British spelling.
fn british(text: &str) -> &str {
    match text {
        "Color" => "Colour",
        "Color Dodge" => "Colour Dodge",
        "Color Burn" => "Colour Burn",
        "Darker Color" => "Darker Colour",
        "Lighter Color" => "Lighter Colour",
        "Color Balance" => "Colour Balance",
        "Color Lookup" => "Colour Lookup",
        "Color Overlay" => "Colour Overlay",
        _ => text,
    }
}

/// `text` in the current language.
pub fn tr(text: &str) -> &str {
    let text = british(text);
    LANGUAGE
        .get()
        .and_then(|locale| locale.catalog.get(text)?.get(Plural::Other))
        .unwrap_or(text)
}

/// `text` in the current language, with each `{name}` in it replaced by the value given for that
/// name. Placeholders without a value are left as they are, and values are not searched.
pub fn tr_args(text: &str, args: &[(&str, &str)]) -> String {
    fill(tr(text), args)
}

/// `text`, a count's English plural such as `"{n} layers"`, in the plural form `count` takes in
/// the current language, with `{n}` replaced by `count`. `en.tsv` holds the English singular.
pub fn tr_plural(text: &str, count: u64) -> String {
    let text = british(text);
    let form = |locale: &'static Locale| {
        let forms = locale.catalog.get(text)?;
        forms
            .get(plural::category(&locale.tag, count))
            .or_else(|| forms.get(Plural::Other))
    };
    let translated = LANGUAGE
        .get()
        .and_then(form)
        .or_else(|| find(ENGLISH).and_then(form));
    fill(translated.unwrap_or(text), &[("n", &count.to_string())])
}

fn fill(text: &str, args: &[(&str, &str)]) -> String {
    let mut filled = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        filled.push_str(&rest[..open]);
        rest = &rest[open..];
        let value = rest.find('}').and_then(|close| {
            let name = &rest[1..close];
            let (_, value) = args.iter().find(|(arg, _)| *arg == name)?;
            Some((close, value))
        });
        match value {
            Some((close, value)) => {
                filled.push_str(value);
                rest = &rest[close + 1..];
            }
            None => {
                filled.push('{');
                rest = &rest[1..];
            }
        }
    }
    filled.push_str(rest);
    filled
}

/// The `{name}` placeholders in `text`, sorted, for checking that a translation keeps them.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        rest = &rest[open..];
        match rest.find('}') {
            Some(close)
                if rest[1..close]
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_') =>
            {
                found.push(&rest[..=close]);
                rest = &rest[close + 1..];
            }
            _ => rest = &rest[1..],
        }
    }
    found.sort_unstable();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../testdata/locales/en-XA.tsv");

    #[test]
    fn languages_switch_live_and_fall_back_to_english() {
        set_language(&Language::new("zh-CN"));
        assert_eq!(tr("Settings"), "设置");
        let name = String::from("a user's layer name");
        assert_eq!(tr(&name), name);
        set_language(&Language::english());
        assert_eq!(tr("Settings"), "Settings");
        assert_eq!(tr("Color Dodge"), "Colour Dodge");
        // A tag this build has no locale for shows English.
        set_language(&Language::new("xx"));
        assert_eq!(tr("Settings"), "Settings");
        assert_eq!(native_name(&Language::new("xx")), None);
        assert_eq!(Language::new("xx").name(), "xx");
        set_language(&Language::english());
    }

    #[test]
    fn a_dropped_in_locale_is_offered_by_its_own_name_with_english_fallback() {
        let pseudo = Language::new("en-XA");
        add_locale("en_xa", FIXTURE);
        add_locale("not a tag", FIXTURE);
        let languages = languages();
        assert_eq!(languages[0], Language::english());
        assert!(languages.contains(&Language::new("zh-CN")));
        assert_eq!(languages.iter().filter(|l| **l == pseudo).count(), 1);
        assert_eq!(native_name(&pseudo), Some("Ƥšéûðö"));
        assert_eq!(pseudo.name(), "Ƥšéûðö");
        assert_eq!(native_name(&Language::new("zh-CN")), Some("简体中文"));
        assert_eq!(native_name(&Language::english()), Some("English"));
        set_language(&pseudo);
        assert_eq!(tr("Settings"), "[Šéţţîñĝš]");
        assert_eq!(tr("Language"), "Language");
        assert_eq!(tr("Colour"), "[Çöļöûŕ]");
        assert_eq!(tr("Color"), "[Çöļöûŕ]");
        set_language(&Language::english());
    }

    #[test]
    fn arguments_fill_placeholders_once() {
        set_language(&Language::english());
        assert_eq!(
            tr_args(
                "{key} is already used by “{command}”.",
                &[("key", "Ctrl+{command}"), ("command", "Save")]
            ),
            "Ctrl+{command} is already used by “Save”."
        );
        assert_eq!(fill("{a} {b} {} {", &[("a", "1")]), "1 {b} {} {");
        assert_eq!(fill("}{a}{{a}}", &[("a", "x")]), "}x{x}");
        assert_eq!(fill("中{a}文", &[("a", "é")]), "中é文");
        add_locale("en-XA", FIXTURE);
        set_language(&Language::new("en-XA"));
        assert_eq!(
            tr_args("{index} of {count}", &[("index", "2"), ("count", "3")]),
            "[3 öƒ 2]"
        );
        set_language(&Language::english());
        assert_eq!(
            placeholders("{b} {a} {} {not one} {a"),
            ["{a}", "{b}", "{}"]
        );
    }

    #[test]
    fn plurals_take_the_form_of_the_language() {
        set_language(&Language::english());
        assert_eq!(tr_plural("{n} actions", 1), "1 action");
        assert_eq!(tr_plural("{n} actions", 0), "0 actions");
        assert_eq!(tr_plural("{n} actions", 2), "2 actions");
        let unknown = String::from("not a catalog key {n}");
        assert_eq!(tr_plural(&unknown, 1), "not a catalog key 1");
        add_locale("en-XA", FIXTURE);
        set_language(&Language::new("en-XA"));
        assert_eq!(tr_plural("{n} panes", 1), "[1 þåñé]");
        assert_eq!(tr_plural("{n} panes", 7), "[7 þåñéš]");
        // Missing from the locale: English, in English's form.
        assert_eq!(tr_plural("{n} formats", 1), "1 format");
        assert_eq!(tr_plural("{n} formats", 3), "3 formats");
        // A language with one form needs one line; one with three falls back to `other`, then
        // to English, for a form it lacks.
        add_locale("ja", "@name\tpseudo\n{n} actions\t<{n}>\n");
        set_language(&Language::new("ja"));
        assert_eq!(tr_plural("{n} actions", 1), "<1>");
        assert_eq!(tr_plural("{n} actions", 5), "<5>");
        add_locale("uk", "@name\tpseudo\n{n} actions\t<{n} one>\tone\n");
        set_language(&Language::new("uk"));
        assert_eq!(tr_plural("{n} actions", 21), "<21 one>");
        assert_eq!(tr_plural("{n} actions", 1), "<1 one>");
        assert_eq!(tr_plural("{n} actions", 5), "5 actions");
        set_language(&Language::english());
    }

    #[test]
    fn locales_are_per_thread() {
        add_locale("en-XA", FIXTURE);
        set_language(&Language::new("en-XA"));
        std::thread::spawn(|| {
            assert!(!languages().contains(&Language::new("en-XA")));
            assert_eq!(tr("Settings"), "Settings");
        })
        .join()
        .unwrap();
        set_language(&Language::english());
    }

    #[test]
    fn discovery_finds_tagged_files_only() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "uk.tsv",
            "pt-BR.tsv",
            "zh_CN.tsv",
            "notes.txt",
            "Read me.tsv",
        ] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("de.tsv")).unwrap();
        let locale_files::LocaleFiles { found, skipped } =
            locale_files::locale_files(dir.path()).unwrap();
        let tags: Vec<_> = found.iter().map(|(tag, _)| tag.as_str()).collect();
        assert_eq!(tags, ["pt-BR", "uk"]);
        assert_eq!(found[1].1, dir.path().join("uk.tsv"));
        assert_eq!(skipped, ["Read me.tsv", "zh_CN.tsv"]);
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/locales");
        let locale_files::LocaleFiles { found, skipped } =
            locale_files::locale_files(&fixture).unwrap();
        assert_eq!(found, [("en-XA".into(), fixture.join("en-XA.tsv"))]);
        assert!(skipped.is_empty());
        assert!(locale_files::locale_files(&dir.path().join("missing")).is_err());
        let built: Vec<_> = BUNDLED.iter().map(|(tag, _)| *tag).collect();
        assert!(
            built.contains(&"en") && built.contains(&"zh-CN"),
            "{built:?}"
        );
    }

    #[test]
    fn tags_are_canonical() {
        for (tag, canonical) in [
            ("en", Some("en")),
            ("zh-CN", Some("zh-CN")),
            ("zh_cn", Some("zh-CN")),
            (" ZH-hans-cn ", Some("zh-Hans-CN")),
            ("es-419", Some("es-419")),
            ("en-xa", Some("en-XA")),
            ("sl-rozaj-biske", Some("sl-rozaj-biske")),
            ("", None),
            ("e", None),
            ("engl", None),
            ("en-", None),
            ("en--GB", None),
            ("en-GB-Latn", None),
            ("en-GB-US", None),
            ("中文", None),
            ("en-ÉÉ", None),
            ("../uk", None),
        ] {
            assert_eq!(canonical_tag(tag).as_deref(), canonical, "{tag:?}");
        }
    }
}

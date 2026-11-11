//! The locale file format: UTF-8, one string per line, tab separated (`⇥` below).
//!
//! ```text
//! # A comment.
//! @name⇥Ƥšéûðö
//! Settings⇥[Šéţţîñĝš]
//! Untranslated, so shown in English
//! {n} layers⇥[{n} ļåýéŕš]
//! {n} layers⇥[{n} ļåýéŕ]⇥one
//! ```
//!
//! A line is the English key, then optionally its translation, then optionally the plural
//! category the translation is for (`zero`, `one`, `two`, `few`, `many`, `other`; a line without
//! one is `other`). A key alone, or with an empty translation, is listed but untranslated. Inside a
//! field `\t`, `\n` and `\\` stand for a tab, a line break and a backslash; `\#` and `\@` for a `#`
//! or `@` that starts a key. `@name` gives the language's name in that language.
use super::plural::Plural;
use std::collections::{HashMap, HashSet};

/// A line of a locale file that was skipped, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// 1-based.
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// The translations of one key, indexed by [`Plural::index`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Forms([Option<String>; 6]);

impl Forms {
    pub fn get(&self, plural: Plural) -> Option<&str> {
        self.0[plural.index()].as_deref()
    }

    /// The categories with a translation.
    pub fn categories(&self) -> impl Iterator<Item = Plural> + '_ {
        Plural::ALL.into_iter().filter(|&p| self.get(p).is_some())
    }
}

/// A parsed locale file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    /// `@name`: the language's own name for itself.
    pub name: Option<String>,
    /// Every key in the file, in file order, once each.
    pub keys: Vec<String>,
    translations: HashMap<String, Forms>,
}

impl Catalog {
    /// Parses a locale file. Lines that cannot be read are skipped and reported; parsing never
    /// fails, so a broken line costs one string rather than the language.
    pub fn parse(source: &str) -> (Self, Vec<Problem>) {
        let mut catalog = Self::default();
        let mut problems = Vec::new();
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        // Each key and plural category, translated or not.
        let mut listed = HashSet::new();
        for (index, line) in source.lines().enumerate() {
            let line_number = index + 1;
            if let Err(message) = catalog.add_line(line, &mut listed) {
                problems.push(Problem {
                    line: line_number,
                    message,
                });
            }
        }
        (catalog, problems)
    }

    fn add_line(
        &mut self,
        line: &str,
        listed: &mut HashSet<(String, Plural)>,
    ) -> Result<(), String> {
        if line.trim().is_empty() || line.starts_with('#') {
            return Ok(());
        }
        if let Some(meta) = line.strip_prefix('@') {
            let (field, value) = meta.split_once('\t').unwrap_or((meta, ""));
            let value = unescape(value)?;
            return match field {
                "name" if value.trim().is_empty() => Err("@name is empty".into()),
                "name" if self.name.is_some() => Err("@name is given twice".into()),
                "name" => {
                    self.name = Some(value);
                    Ok(())
                }
                _ => Err(format!("unknown setting @{field}")),
            };
        }
        let mut fields = line.split('\t');
        let key = unescape(fields.next().unwrap_or_default())?;
        let value = fields.next().map(unescape).transpose()?.unwrap_or_default();
        let plural = match fields.next() {
            None => Plural::Other,
            Some(name) => {
                Plural::parse(name).ok_or_else(|| format!("unknown plural category “{name}”"))?
            }
        };
        if fields.next().is_some() {
            return Err("more than three tab-separated fields".into());
        }
        if key.is_empty() {
            return Err("empty key".into());
        }
        if value.is_empty() && plural != Plural::Other {
            return Err(format!("empty {} form", plural.name()));
        }
        if !listed.insert((key.clone(), plural)) {
            return Err(format!("“{key}” is listed twice"));
        }
        if !self.translations.contains_key(&key) {
            self.keys.push(key.clone());
        }
        let forms = self.translations.entry(key).or_default();
        if !value.is_empty() {
            forms.0[plural.index()] = Some(value);
        }
        Ok(())
    }

    /// The translations of `key`, when the file lists it.
    pub fn get(&self, key: &str) -> Option<&Forms> {
        self.translations.get(key)
    }

    /// Whether `key` has a translation.
    pub fn translates(&self, key: &str) -> bool {
        self.get(key)
            .is_some_and(|forms| forms.categories().next().is_some())
    }

    /// Every translation, of every key and plural category.
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.translations
            .values()
            .flat_map(|forms| forms.0.iter().flatten().map(String::as_str))
    }
}

/// Decodes the escapes of a field; see the module documentation.
fn unescape(field: &str) -> Result<String, String> {
    if !field.contains('\\') {
        return Ok(field.to_owned());
    }
    let mut text = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => text.push('\t'),
            Some('n') => text.push('\n'),
            Some('\\') => text.push('\\'),
            Some(c @ ('#' | '@')) if text.is_empty() => text.push(c),
            Some(c) => return Err(format!("unknown escape \\{c}")),
            None => return Err("a backslash ends the field".into()),
        }
    }
    Ok(text)
}

/// Encodes `text` as a field, the inverse of [`unescape`].
pub fn escape(text: &str) -> String {
    let mut field = String::with_capacity(text.len());
    if text.starts_with(['#', '@']) {
        field.push('\\');
    }
    for c in text.chars() {
        match c {
            '\t' => field.push_str("\\t"),
            '\n' => field.push_str("\\n"),
            '\\' => field.push_str("\\\\"),
            c => field.push(c),
        }
    }
    field
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problems(source: &str) -> Vec<String> {
        Catalog::parse(source)
            .1
            .into_iter()
            .map(|p| p.to_string())
            .collect()
    }

    #[test]
    fn lines_give_keys_translations_and_plural_forms() {
        let (catalog, problems) = Catalog::parse(
            "\u{feff}# comment\r\n@name\tPseudo\r\n\r\nSettings\t[Šéţţîñĝš]\r\nUntranslated\nEmpty\t\n\
             {n} layers\t[{n} ļåýéŕš]\n{n} layers\t[{n} ļåýéŕ]\tone\n",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(catalog.name.as_deref(), Some("Pseudo"));
        assert_eq!(
            catalog.keys,
            ["Settings", "Untranslated", "Empty", "{n} layers"]
        );
        assert_eq!(
            catalog.get("Settings").unwrap().get(Plural::Other),
            Some("[Šéţţîñĝš]")
        );
        assert!(catalog.translates("Settings"));
        assert!(!catalog.translates("Untranslated"));
        assert!(!catalog.translates("Empty"));
        assert!(!catalog.translates("Missing"));
        let layers = catalog.get("{n} layers").unwrap();
        assert_eq!(layers.get(Plural::One), Some("[{n} ļåýéŕ]"));
        assert_eq!(
            layers.categories().collect::<Vec<_>>(),
            [Plural::One, Plural::Other]
        );
        let mut texts: Vec<_> = catalog.texts().collect();
        texts.sort();
        assert_eq!(texts, ["[{n} ļåýéŕ]", "[{n} ļåýéŕš]", "[Šéţţîñĝš]"]);
    }

    #[test]
    fn escapes_round_trip() {
        let source = "Line\\none\\ttab \\\\ back\tA\\nB\n\\#hash\t#\n\\@at\t@ \\# \n";
        let (catalog, _) = Catalog::parse(source);
        assert_eq!(problems(source), ["line 3: unknown escape \\#"]);
        assert_eq!(
            catalog
                .get("Line\none\ttab \\ back")
                .unwrap()
                .get(Plural::Other),
            Some("A\nB")
        );
        assert_eq!(catalog.get("#hash").unwrap().get(Plural::Other), Some("#"));
        assert!(catalog.get("@at").is_none());
        for text in [
            "plain",
            "a\tb\nc\\d",
            "#start",
            "@start",
            "\\#",
            "end\\",
            "",
        ] {
            assert_eq!(unescape(&escape(text)).as_deref(), Ok(text), "{text:?}");
        }
    }

    #[test]
    fn bad_lines_are_skipped_and_reported() {
        assert_eq!(
            problems(
                "Same\tA\nSame\tB\n\tNo key\nPlural\tx\tseveral\nToo\tmany\tone\textra\n\
                 Trailing\tslash\\\nBad\t\\q\n@name\n@name\tX\n@name\tY\n@colour\tred\n\
                 Empty form\t\tone\nListed\nListed\n{n} things\tx\tone\n{n} things\ty\tone\n"
            ),
            [
                "line 2: “Same” is listed twice",
                "line 3: empty key",
                "line 4: unknown plural category “several”",
                "line 5: more than three tab-separated fields",
                "line 6: a backslash ends the field",
                "line 7: unknown escape \\q",
                "line 8: @name is empty",
                "line 10: @name is given twice",
                "line 11: unknown setting @colour",
                "line 12: empty one form",
                "line 14: “Listed” is listed twice",
                "line 16: “{n} things” is listed twice",
            ]
        );
        let (catalog, _) = Catalog::parse("Same\tA\nSame\tB\n");
        assert_eq!(catalog.get("Same").unwrap().get(Plural::Other), Some("A"));
    }

    #[test]
    fn hostile_input_never_panics() {
        // Deterministic noise: random bytes, weighted towards the format's own characters.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let alphabet = "\t\n\r\\#@ abn{}é中\u{feff}\u{0}\u{202e}";
        let alphabet: Vec<char> = alphabet.chars().collect();
        for _ in 0..2_000 {
            let mut source = String::new();
            for _ in 0..(state % 64) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                source.push(alphabet[(state % alphabet.len() as u64) as usize]);
            }
            let (catalog, problems) = Catalog::parse(&source);
            assert!(problems.iter().all(|p| p.line >= 1));
            for key in &catalog.keys {
                assert!(catalog.get(key).is_some());
            }
        }
        let long = format!("{}\t{}\n", "k".repeat(1 << 20), "v".repeat(1 << 20));
        assert_eq!(Catalog::parse(&long).0.keys.len(), 1);
        let bytes: Vec<u8> = (0..=255).cycle().take(4096).collect();
        Catalog::parse(&String::from_utf8_lossy(&bytes));
    }
}

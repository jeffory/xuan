//! Plural categories of whole numbers, after the CLDR cardinal rules
//! (<https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html>).
//! Counts in the interface are never fractional, so only the integer rules are kept.

/// A CLDR plural category: the third column of a locale line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Plural {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

impl Plural {
    pub const ALL: [Self; 6] = [
        Self::Zero,
        Self::One,
        Self::Two,
        Self::Few,
        Self::Many,
        Self::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::One => "one",
            Self::Two => "two",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    pub(super) fn index(self) -> usize {
        self as usize
    }
}

/// The rule families that differ for whole numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// No plural forms.
    Other,
    /// one: 1.
    One,
    /// one: 0 and 1.
    ZeroOne,
    /// one: ends in 1 but not 11; few: ends in 2 to 4 but not 12 to 14; many: the rest.
    EastSlavic,
    /// As [`Rule::EastSlavic`], with other in place of many.
    SouthSlavic,
    /// one: 1; few: ends in 2 to 4 but not 12 to 14; many: the rest.
    Polish,
    /// one: 1; few: 2 to 4.
    Czech,
    /// zero, one, two; few: ends in 03 to 10; many: ends in 11 to 99.
    Arabic,
    /// one: 1; two: 2.
    Hebrew,
    /// one: ends in 1 but not 11 to 19; few: ends in 2 to 9 but not 11 to 19.
    Lithuanian,
    /// zero: ends in 0 or 11 to 19; one: ends in 1 but not 11.
    Latvian,
    /// one: 1; few: 0, or other numbers ending in 01 to 19.
    Romanian,
    /// one: ends in 01; two: ends in 02; few: ends in 03 or 04.
    Slovenian,
}

fn rule(language: &str) -> Rule {
    // The language subtag of a canonical tag.
    let language = language.split('-').next().unwrap_or_default();
    match language {
        "zh" | "ja" | "ko" | "vi" | "th" | "id" | "ms" | "lo" | "my" | "km" | "yue" | "jv" => {
            Rule::Other
        }
        "fr" | "pt" | "hi" | "bn" | "fa" | "gu" | "kn" | "zu" | "am" | "as" => Rule::ZeroOne,
        "ru" | "uk" | "be" => Rule::EastSlavic,
        "hr" | "sr" | "bs" => Rule::SouthSlavic,
        "pl" => Rule::Polish,
        "cs" | "sk" => Rule::Czech,
        "ar" => Rule::Arabic,
        "he" => Rule::Hebrew,
        "lt" => Rule::Lithuanian,
        "lv" => Rule::Latvian,
        "ro" | "mo" => Rule::Romanian,
        "sl" => Rule::Slovenian,
        _ => Rule::One,
    }
}

/// The plural category of `n` in the language of the tag `language`.
pub fn category(language: &str, n: u64) -> Plural {
    let (n10, n100) = (n % 10, n % 100);
    let teen = (11..=19).contains(&n100);
    match rule(language) {
        Rule::Other => Plural::Other,
        Rule::One if n == 1 => Plural::One,
        Rule::ZeroOne if n <= 1 => Plural::One,
        Rule::EastSlavic | Rule::SouthSlavic if n10 == 1 && n100 != 11 => Plural::One,
        Rule::EastSlavic | Rule::SouthSlavic | Rule::Polish
            if (2..=4).contains(&n10) && !(12..=14).contains(&n100) =>
        {
            Plural::Few
        }
        Rule::EastSlavic => Plural::Many,
        Rule::Polish if n == 1 => Plural::One,
        Rule::Polish => Plural::Many,
        Rule::Czech if n == 1 => Plural::One,
        Rule::Czech if (2..=4).contains(&n) => Plural::Few,
        Rule::Arabic => match n {
            0 => Plural::Zero,
            1 => Plural::One,
            2 => Plural::Two,
            _ if (3..=10).contains(&n100) => Plural::Few,
            _ if n100 >= 11 => Plural::Many,
            _ => Plural::Other,
        },
        Rule::Hebrew if n == 1 => Plural::One,
        Rule::Hebrew if n == 2 => Plural::Two,
        Rule::Lithuanian if n10 == 1 && !teen => Plural::One,
        Rule::Lithuanian if n10 >= 2 && !teen => Plural::Few,
        Rule::Latvian if n10 == 0 || teen => Plural::Zero,
        Rule::Latvian if n10 == 1 && n100 != 11 => Plural::One,
        Rule::Romanian if n == 1 => Plural::One,
        Rule::Romanian if n == 0 || (1..=19).contains(&n100) => Plural::Few,
        Rule::Slovenian if n100 == 1 => Plural::One,
        Rule::Slovenian if n100 == 2 => Plural::Two,
        Rule::Slovenian if (3..=4).contains(&n100) => Plural::Few,
        _ => Plural::Other,
    }
}

/// The categories whole numbers fall into in the language of the tag `language`: the plural forms
/// a translation can give.
pub fn categories(language: &str) -> &'static [Plural] {
    use Plural::*;
    match rule(language) {
        Rule::Other => &[Other],
        Rule::One | Rule::ZeroOne => &[One, Other],
        Rule::EastSlavic | Rule::Polish => &[One, Few, Many],
        Rule::SouthSlavic | Rule::Czech | Rule::Lithuanian | Rule::Romanian => &[One, Few, Other],
        Rule::Arabic => &[Zero, One, Two, Few, Many, Other],
        Rule::Hebrew => &[One, Two, Other],
        Rule::Latvian => &[Zero, One, Other],
        Rule::Slovenian => &[One, Two, Few, Other],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms(language: &str, numbers: &[u64]) -> Vec<&'static str> {
        numbers
            .iter()
            .map(|&n| category(language, n).name())
            .collect()
    }

    #[test]
    fn whole_numbers_follow_the_cldr_rules() {
        let numbers = [0, 1, 2, 3, 5, 11, 12, 21, 22, 25, 101, 111, 1_000_000];
        assert_eq!(
            forms("en", &numbers),
            [
                "other", "one", "other", "other", "other", "other", "other", "other", "other",
                "other", "other", "other", "other"
            ]
        );
        assert!(forms("zh-CN", &numbers).iter().all(|&f| f == "other"));
        assert_eq!(forms("fr", &[0, 1, 2]), ["one", "one", "other"]);
        assert_eq!(
            forms("uk", &numbers),
            [
                "many", "one", "few", "few", "many", "many", "many", "one", "few", "many", "one",
                "many", "many"
            ]
        );
        assert_eq!(
            forms("sr-Latn", &[1, 3, 5, 11]),
            ["one", "few", "other", "other"]
        );
        assert_eq!(
            forms("pl", &[1, 2, 5, 21, 22]),
            ["one", "few", "many", "many", "few"]
        );
        assert_eq!(
            forms("cs", &[1, 3, 5, 22]),
            ["one", "few", "other", "other"]
        );
        assert_eq!(
            forms("ar", &[0, 1, 2, 3, 10, 11, 99, 100, 102, 103]),
            [
                "zero", "one", "two", "few", "few", "many", "many", "other", "other", "few"
            ]
        );
        assert_eq!(forms("he", &[1, 2, 3]), ["one", "two", "other"]);
        assert_eq!(
            forms("lt", &[1, 2, 10, 11, 21]),
            ["one", "few", "other", "other", "one"]
        );
        assert_eq!(
            forms("lv", &[0, 1, 11, 21, 2]),
            ["zero", "one", "zero", "one", "other"]
        );
        assert_eq!(
            forms("ro", &[0, 1, 2, 19, 20, 101]),
            ["few", "one", "few", "few", "other", "few"]
        );
        assert_eq!(
            forms("sl", &[1, 2, 3, 5, 101]),
            ["one", "two", "few", "other", "one"]
        );
        // Languages without their own rule pluralise like English.
        assert_eq!(forms("eo", &[1, 2]), ["one", "other"]);
        assert_eq!(category("en", u64::MAX), Plural::Other);
    }

    #[test]
    fn every_category_a_rule_gives_is_one_it_lists() {
        for language in [
            "en", "zh", "fr", "uk", "sr", "pl", "cs", "ar", "he", "lt", "lv", "ro", "sl",
        ] {
            for n in 0..=1_000 {
                assert!(
                    categories(language).contains(&category(language, n)),
                    "{language} {n}"
                );
            }
        }
        for plural in Plural::ALL {
            assert_eq!(Plural::parse(plural.name()), Some(plural));
        }
        assert_eq!(Plural::parse("One"), None);
    }
}

# Translating Xuan

Xuan's interface is written in English. Each other language is one text file in
[`assets/locales`](../assets/locales), named after its language tag: `zh-CN.tsv` is
Simplified Chinese. Every file there is built in and offered in **Settings → General →
Language** under the language's own name. Strings a file leaves out are shown in English, so a
partial translation is useful from its first line.

## Add a language

1. Pick the [BCP 47 tag](https://www.w3.org/International/articles/language-tags/): the
   language alone where one variant serves everyone (`uk`, `de`, `ja`), with a region or script
   where they differ (`pt-BR`, `zh-TW`, `sr-Latn`). The file name must be the tag in its usual
   case; a build warns about and leaves out a `.tsv` file that isn't named this way.
2. Copy `assets/locales/en.tsv`, the list of every English string, to `assets/locales/<tag>.tsv`.
3. Replace `English` on the `@name` line with the language's name in that language, such as
   `Українська`, and remove the comment lines at the top.
4. Translate: after each English string, type a tab and the translation. Leave a line as it is
   to show that string in English for now. The few lines that already have two tabs, such as
   `{n} panes<TAB>{n} pane<TAB>one`, carry the English singular of a count: translate them
   (see Plurals below) or delete them, since as they are they would show the English word.
5. Build and try it: `cargo run --release`, then choose the language in Settings → General.
   It applies at once.
6. Check it (see below), then send a pull request with the file.

Translate from the running interface where you can: many strings are short labels whose meaning
depends on where they appear. `docs/USAGE.md` describes each tool and dialog.

## The file format

UTF-8, one string per line, fields separated by tabs:

```text
# A comment
@name<TAB>Ƥšéûðö
Settings<TAB>[Šéţţîñĝš]
A string still to translate
{index} of {count}<TAB>[{count} öƒ {index}]
{n} panes<TAB>[{n} þåñéš]
{n} panes<TAB>[{n} þåñé]<TAB>one
```

(This is `testdata/locales/en-XA.tsv`, a pseudo-locale the tests use: it marks translated text
without being a real language.)

- **Keys** are the English text exactly, including punctuation such as `…` and `“”`. Don't
  edit them; a key that doesn't match `en.tsv` is never used, and the tests reject it.
- **Placeholders** in braces, `{}` or `{name}`, are replaced with a file name, number or key
  while Xuan runs. Keep every one; move named ones wherever the sentence needs them.
- **Escapes**: write `\t` for a tab, `\n` for a line break and `\\` for a backslash. Write `\#`
  or `\@` for a `#` or `@` that starts a string, since a line starting with `#` is a comment
  and one starting with `@` is a setting. `@name` is the only setting.
- **Plurals.** A key with `{n}` whose `en.tsv` line has a third field (`one`) is a count. The
  plain line gives the `other` form; add a line with a third field for each other form your
  language has, from `zero`, `one`, `two`, `few` and `many`. Xuan picks the form by the
  [CLDR plural rules](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html)
  for whole numbers; `src/i18n/plural.rs` lists the languages it knows, and others pluralise as
  English does (`one` for 1, `other` for the rest). A language without plurals, such as Chinese,
  needs only the plain line. A missing form falls back to `other`, then to English. Ukrainian,
  for example, uses `one` (1, 21, 31…), `few` (2–4, 22–24…) and `many` (0, 5–20, 25…):

  ```text
  {n} panes<TAB>{n} …<TAB>one
  {n} panes<TAB>{n} …<TAB>few
  {n} panes<TAB>{n} …<TAB>many
  ```

## Check a translation

```sh
scripts/check-locales.sh uk            # what is left to translate, and stale lines
scripts/check-locales.sh --summary     # progress of every language
cargo test --locked --lib i18n         # the file reads cleanly and keeps its placeholders
cargo test --locked --bin xuan fonts   # the bundled fonts can draw every translation
```

`check-locales.sh` lists the strings still in English and the **stale** lines: keys that
`en.tsv` no longer has because the English text changed or was removed. Update a stale line's
key to the new English text, or delete it. The script exits with an error while there are stale
lines.

`cargo test` (and CI) fails when a locale file has a line it can't read, a key that isn't in
`en.tsv`, a translation that drops or renames a placeholder, a plural form the language doesn't
have, no `@name` line, or text the bundled fonts can't draw.

## Fonts and scripts

Xuan draws its interface with two bundled fonts, Inter and Droid Sans Fallback, falling back
from one to the next, plus egui's own defaults (Hack for monospace text):

| Script | Font | Status |
| --- | --- | --- |
| Latin, including accented Latin (Czech, Polish, Turkish…) | Inter; Hack in monospace text | Covered |
| Vietnamese | Inter | Covered, but monospace text such as plugin logs lacks the stacked accents |
| Greek | Inter; Hack in monospace text | Covered |
| Cyrillic (Ukrainian, Russian, Belarusian, Serbian, Kazakh…) | Inter; Hack in monospace text | Covered |
| Chinese (Simplified and Traditional), Japanese (kanji and kana) | Droid Sans Fallback | Covered |
| Korean Hangul | — | Not covered |
| Arabic, Hebrew | — | Not covered, and needs right-to-left layout |
| Devanagari (Hindi, Marathi, Nepali…), Bengali, Thai and other Indic and Southeast Asian scripts | — | Not covered, and needs text shaping |

Fonts are the smaller problem for the uncovered scripts. egui, Xuan's interface toolkit, lays
text out left to right one character at a time: it has no bidirectional layout, so Arabic and
Hebrew would read backwards, and no shaping, so Arabic letters would not join and Devanagari
conjuncts and vowel signs would not form. A translation in those scripts needs that support in
egui first. Korean needs only a font with Hangul.

To bundle a font: add it under `assets/fonts` with its licence (it must be OFL, Apache, MIT or
similar), list it in `THIRD_PARTY.md`, and add it to the fallback list in `theme::apply` in
`src/app/theme.rs`. Fonts for a whole script can add megabytes to every download; prefer one
subset to the script you need. Then update the table above and the font test in
`src/app/tests.rs`, which checks this table.

## For developers: making strings translatable

- Pass UI text through `xuan::i18n::tr("…")`. For values in a sentence use
  `tr_args("Import “{name}”?", &[("name", &name)])`, so a translation can move them, and for
  counts `tr_plural("{n} layers", count)`. Don't build sentences from translated pieces.
- Add each new string to `assets/locales/en.tsv`, one per line, and a plural's singular as
  `{n} layers<TAB>{n} layer<TAB>one`. `cargo test` scans `src/` for `tr`, `tr_args` and
  `tr_plural` calls and prints any string missing from `en.tsv`, ready to paste. It also
  reports strings in `en.tsv` that no longer appear in the source.
- Strings that reach `tr` through a variable, such as an enum's `name()`, are invisible to that
  scan; add them to `en.tsv` by hand. A test checks the command labels and categories.
- Changing an English string changes its key, and `cargo test` fails until each locale file
  follows: update the key in its line when the meaning is unchanged, such as a punctuation fix,
  or remove the line so the string shows in English until it is translated again.
- Keep identifiers, file formats and document content independent of the interface language.

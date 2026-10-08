//! Style runs: letters of a text layer drawn with another font family, colour, weight or
//! slant than the layer's own (format 17).
//!
//! Runs count letters as Unicode scalar values (Rust `char`s) from the start of the text, so
//! every position is a valid place to split it. A run lists only what it changes; letters
//! outside every run use the layer's style. Editing keeps runs normalised: sorted, not
//! overlapping, non-empty, each changing something, and adjacent runs that change the same
//! things merged into one.

use std::ops::Range;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::TextStyle;

/// The most runs one text layer may hold.
pub const MAX_TEXT_RUNS: usize = 4096;

/// What a run of letters changes from its layer's style; `None` keeps the layer's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
}

impl RunStyle {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `self` with what `change` sets laid over it.
    fn with(&self, change: &RunStyle) -> RunStyle {
        RunStyle {
            family: change.family.clone().or_else(|| self.family.clone()),
            color: change.color.or(self.color),
            bold: change.bold.or(self.bold),
            italic: change.italic.or(self.italic),
        }
    }

    /// `self` without what `change` sets.
    fn without(&self, change: &RunStyle) -> RunStyle {
        RunStyle {
            family: self.family.clone().filter(|_| change.family.is_none()),
            color: self.color.filter(|_| change.color.is_none()),
            bold: self.bold.filter(|_| change.bold.is_none()),
            italic: self.italic.filter(|_| change.italic.is_none()),
        }
    }
}

/// Letters `start..end` (counted in `char`s) and what they change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRun {
    pub start: usize,
    pub end: usize,
    #[serde(flatten)]
    pub style: RunStyle,
}

/// One letter's style once its run is applied to the layer's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetterStyle {
    pub family: String,
    pub color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
}

/// A family name the text engine can look up: not blank and of a bounded length.
pub(super) fn valid_family(family: &str) -> bool {
    !family.trim().is_empty() && family.len() <= 1024
}

impl TextStyle {
    pub(super) fn validate_runs(&self) -> Result<()> {
        if self.runs.is_empty() {
            return Ok(());
        }
        ensure!(
            self.runs.len() <= MAX_TEXT_RUNS,
            "A text layer may have at most {MAX_TEXT_RUNS} style runs"
        );
        let letters = self.content.chars().count();
        let mut end = 0;
        for run in &self.runs {
            ensure!(
                run.start >= end && run.start < run.end && run.end <= letters,
                "Text style runs must be sorted, not overlap and lie inside the text"
            );
            ensure!(
                run.style.family.as_deref().is_none_or(valid_family),
                "Invalid font family"
            );
            end = run.end;
        }
        Ok(())
    }

    /// The style of the letter at `index` (a `char` index).
    pub fn letter_style(&self, index: usize) -> LetterStyle {
        let run = self
            .runs
            .iter()
            .find(|run| (run.start..run.end).contains(&index))
            .map(|run| &run.style);
        self.resolve(run.unwrap_or(&RunStyle::default()))
    }

    /// `run` applied to the layer's style.
    pub fn resolve(&self, run: &RunStyle) -> LetterStyle {
        LetterStyle {
            family: run.family.clone().unwrap_or_else(|| self.family.clone()),
            color: run.color.unwrap_or(self.color),
            bold: run.bold.unwrap_or(self.bold),
            italic: run.italic.unwrap_or(self.italic),
        }
    }

    /// The text cut where its style changes, as byte ranges with what each part changes; the
    /// parts cover the whole text in order.
    pub fn spans(&self) -> Vec<(Range<usize>, RunStyle)> {
        let mut bytes: Vec<usize> = self.content.char_indices().map(|(i, _)| i).collect();
        bytes.push(self.content.len());
        let byte = |letter: usize| bytes[letter.min(bytes.len() - 1)];
        let mut spans = Vec::with_capacity(self.runs.len() * 2 + 1);
        let mut at = 0;
        for run in &self.runs {
            let (start, end) = (byte(run.start), byte(run.end));
            if start > at {
                spans.push((at..start, RunStyle::default()));
            }
            if end > start.max(at) {
                spans.push((start.max(at)..end, run.style.clone()));
                at = end;
            }
        }
        if at < self.content.len() || spans.is_empty() {
            spans.push((at..self.content.len(), RunStyle::default()));
        }
        spans
    }

    /// Change the style of the letters in `letters` (`char` indices). A range covering the
    /// whole text changes the layer's style instead, as [`Self::set_style_all`] does.
    pub fn set_run_style(&mut self, letters: Range<usize>, change: &RunStyle) {
        let count = self.content.chars().count();
        let (start, end) = (letters.start.min(count), letters.end.min(count));
        if start == 0 && end == count {
            self.set_style_all(change);
            return;
        }
        if start >= end || change.is_empty() {
            return;
        }
        let (mut table, mut letters) = self.letters(count);
        // Each style found in the range gets a changed copy at the end of the table.
        let mut changed: Vec<(u32, u32)> = Vec::new();
        for letter in &mut letters[start..end] {
            let index = match changed.iter().find(|(from, _)| from == letter) {
                Some(&(_, to)) => to,
                None => {
                    table.push(table[*letter as usize].with(change));
                    let to = (table.len() - 1) as u32;
                    changed.push((*letter, to));
                    to
                }
            };
            *letter = index;
        }
        self.runs = runs_from_letters(&table, &letters);
        self.normalize_runs();
    }

    /// Change the style of the whole layer: what `change` sets becomes the layer's own, and
    /// runs stop changing it.
    pub fn set_style_all(&mut self, change: &RunStyle) {
        if let Some(family) = &change.family {
            self.family = family.clone();
        }
        if let Some(color) = change.color {
            self.color = color;
        }
        if let Some(bold) = change.bold {
            self.bold = bold;
        }
        if let Some(italic) = change.italic {
            self.italic = italic;
        }
        for run in &mut self.runs {
            run.style = run.style.without(change);
        }
        self.normalize_runs();
    }

    /// Replace the text with `content`, keeping each unchanged letter's style. Letters typed
    /// in take the style of the letter before them (or, at the start, after them), as in
    /// other editors.
    pub fn replace_content(&mut self, content: String) {
        if self.runs.is_empty() || content == self.content {
            self.content = content;
            return;
        }
        let old: Vec<char> = self.content.chars().collect();
        let new: Vec<char> = content.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let (table, letters) = self.letters(old.len());
        let inserted = new.len() - prefix - suffix;
        let inherited = if prefix > 0 {
            letters[prefix - 1]
        } else {
            letters.get(old.len() - suffix).copied().unwrap_or(0)
        };
        let mut remapped = Vec::with_capacity(new.len());
        remapped.extend_from_slice(&letters[..prefix]);
        remapped.extend(std::iter::repeat_n(inherited, inserted));
        remapped.extend_from_slice(&letters[old.len() - suffix..]);
        self.content = content;
        self.runs = runs_from_letters(&table, &remapped);
        self.normalize_runs();
    }

    /// Put the runs in normal form: inside the text, sorted, not overlapping, each changing
    /// something the layer's style does not already have, and adjacent equal runs merged.
    /// The number of runs is left to [`TextStyle::validate`] to bound, so no letter quietly
    /// loses its style.
    pub fn normalize_runs(&mut self) {
        if self.runs.is_empty() {
            return;
        }
        let count = self.content.chars().count();
        let mut runs = std::mem::take(&mut self.runs);
        runs.sort_by_key(|run| run.start);
        let mut normal: Vec<TextRun> = Vec::with_capacity(runs.len());
        let mut covered = 0;
        for mut run in runs {
            run.start = run.start.max(covered);
            run.end = run.end.min(count);
            if run.start >= run.end {
                continue;
            }
            covered = run.end;
            let style = &mut run.style;
            if style
                .family
                .as_ref()
                .is_some_and(|f| *f == self.family || !valid_family(f))
            {
                style.family = None;
            }
            if style.color == Some(self.color) {
                style.color = None;
            }
            if style.bold == Some(self.bold) {
                style.bold = None;
            }
            if style.italic == Some(self.italic) {
                style.italic = None;
            }
            if style.is_empty() {
                continue;
            }
            match normal.last_mut() {
                Some(last) if last.end == run.start && last.style == run.style => {
                    last.end = run.end;
                }
                _ => normal.push(run),
            }
        }
        self.runs = normal;
    }

    /// Each letter's style as an index into a table whose first entry changes nothing.
    fn letters(&self, count: usize) -> (Vec<RunStyle>, Vec<u32>) {
        let mut table = vec![RunStyle::default()];
        let mut letters = vec![0_u32; count];
        for run in &self.runs {
            let index = match table.iter().position(|style| *style == run.style) {
                Some(index) => index,
                None => {
                    table.push(run.style.clone());
                    table.len() - 1
                }
            } as u32;
            let (start, end) = (run.start.min(count), run.end.min(count));
            for letter in letters.get_mut(start..end).into_iter().flatten() {
                *letter = index;
            }
        }
        (table, letters)
    }
}

/// Runs from each letter's index into `table`, before normalising.
fn runs_from_letters(table: &[RunStyle], letters: &[u32]) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    for (index, &letter) in letters.iter().enumerate() {
        let style = &table[letter as usize];
        if style.is_empty() {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.end == index && last.style == *style => last.end = index + 1,
            _ => runs.push(TextRun {
                start: index,
                end: index + 1,
                style: style.clone(),
            }),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    fn style(content: &str) -> TextStyle {
        TextStyle {
            content: content.into(),
            ..Default::default()
        }
    }

    fn color(color: [u8; 4]) -> RunStyle {
        RunStyle {
            color: Some(color),
            ..Default::default()
        }
    }

    fn family(name: &str) -> RunStyle {
        RunStyle {
            family: Some(name.into()),
            ..Default::default()
        }
    }

    fn run(start: usize, end: usize, style: RunStyle) -> TextRun {
        TextRun { start, end, style }
    }

    #[test]
    fn changing_a_range_splits_and_merges_runs() {
        let mut text = style("APPle");
        text.set_run_style(0..3, &color(RED));
        assert_eq!(text.runs, vec![run(0, 3, color(RED))]);
        // A font on part of the red run splits it.
        text.set_run_style(1..4, &family("Menlo"));
        assert_eq!(
            text.runs,
            vec![
                run(0, 1, color(RED)),
                run(
                    1,
                    3,
                    RunStyle {
                        family: Some("Menlo".into()),
                        color: Some(RED),
                        ..Default::default()
                    }
                ),
                run(3, 4, family("Menlo")),
            ]
        );
        // Setting the layer's own family back merges the red letters again.
        text.set_run_style(0..5 - 1, &family(&text.family.clone()));
        assert_eq!(text.runs, vec![run(0, 3, color(RED))]);
        // Neighbouring ranges with the same change become one run.
        text.set_run_style(3..4, &color(RED));
        assert_eq!(text.runs, vec![run(0, 4, color(RED))]);
        assert_eq!(text.letter_style(2).color, RED);
        assert_eq!(text.letter_style(4).color, text.color);
        text.validate().unwrap();
    }

    #[test]
    fn whole_text_and_layer_changes_clear_runs() {
        let mut text = style("APPle");
        text.set_run_style(0..3, &color(RED));
        text.set_run_style(3..5, &family("Menlo"));
        // Changing the layer's colour leaves the fonts and recolours every letter.
        text.set_style_all(&color(BLUE));
        assert_eq!(text.color, BLUE);
        assert_eq!(text.runs, vec![run(3, 5, family("Menlo"))]);
        // Selecting every letter changes the layer's own style.
        text.set_run_style(0..5, &family("Menlo"));
        assert_eq!(text.family, "Menlo");
        assert!(text.runs.is_empty());
        // Empty ranges and empty changes do nothing; ranges past the end are clamped.
        text.set_run_style(2..2, &color(RED));
        text.set_run_style(1..3, &RunStyle::default());
        assert!(text.runs.is_empty());
        text.set_run_style(3..99, &color(RED));
        assert_eq!(text.runs, vec![run(3, 5, color(RED))]);
    }

    #[test]
    fn editing_the_text_keeps_letter_styles() {
        let mut text = style("APPle");
        text.set_run_style(0..3, &color(RED));
        // Typing after the red letters continues the red run, as in other editors.
        text.replace_content("APPPle".into());
        assert_eq!(text.runs, vec![run(0, 4, color(RED))]);
        // Typing at the start takes the first letter's style.
        text.replace_content("xAPPPle".into());
        assert_eq!(text.runs, vec![run(0, 5, color(RED))]);
        // Typing after a plain letter stays plain.
        text.replace_content("xAPPPle!".into());
        assert_eq!(text.runs, vec![run(0, 5, color(RED))]);
        // Deleting letters shrinks or removes runs.
        text.replace_content("xle!".into());
        assert_eq!(text.runs, vec![run(0, 1, color(RED))]);
        text.replace_content("le".into());
        assert!(text.runs.is_empty());
        // Replacing a selection inside a run keeps the run around it.
        let mut text = style("héllo wörld");
        text.set_run_style(6..11, &family("Menlo"));
        text.replace_content("héllo wörld, wide 字".into());
        assert_eq!(text.runs, vec![run(6, 19, family("Menlo"))]);
        text.replace_content("héllo w字".into());
        assert_eq!(text.runs, vec![run(6, 8, family("Menlo"))]);
        text.validate().unwrap();
    }

    #[test]
    fn spans_cover_the_text_in_bytes() {
        let mut text = style("aé字b");
        assert_eq!(
            text.spans(),
            vec![(0..text.content.len(), RunStyle::default())]
        );
        text.set_run_style(1..3, &color(RED));
        assert_eq!(
            text.spans(),
            vec![
                (0..1, RunStyle::default()),
                (1..6, color(RED)),
                (6..7, RunStyle::default()),
            ]
        );
        text.set_run_style(3..4, &color(BLUE));
        assert_eq!(text.spans().last(), Some(&(6..7, color(BLUE))));
        let empty = style("");
        assert_eq!(empty.spans(), vec![(0..0, RunStyle::default())]);
    }

    #[test]
    fn hostile_runs_are_refused_or_normalised() {
        let mut text = style("Hello");
        for runs in [
            vec![run(0, 6, color(RED))],
            vec![run(2, 2, color(RED))],
            vec![run(3, 1, color(RED))],
            vec![run(0, 3, color(RED)), run(2, 4, color(BLUE))],
            vec![run(3, 4, color(RED)), run(0, 1, color(BLUE))],
            vec![run(0, 1, family(" "))],
            vec![run(0, 1, family(&"x".repeat(1025)))],
            vec![run(usize::MAX - 1, usize::MAX, color(RED))],
        ] {
            text.runs = runs.clone();
            assert!(text.validate().is_err(), "{runs:?}");
            // Normalising makes any of them valid.
            text.normalize_runs();
            text.validate().unwrap();
        }
        let mut long = style(&"a".repeat(MAX_TEXT_RUNS * 2 + 2));
        long.runs = (0..=MAX_TEXT_RUNS)
            .map(|i| run(i * 2, i * 2 + 1, color(RED)))
            .collect();
        assert!(long.validate().is_err());
        long.normalize_runs();
        assert_eq!(long.runs.len(), MAX_TEXT_RUNS + 1);
        assert!(long.validate().is_err());
        // Restyling every other letter can go past the limit too; the text then fails
        // validation (which the Text window shows) rather than losing styles.
        let mut striped = style(&"a".repeat(MAX_TEXT_RUNS * 2 + 2));
        for i in 0..=MAX_TEXT_RUNS {
            striped.set_run_style(i * 2..i * 2 + 1, &color(RED));
        }
        assert_eq!(striped.runs, long.runs);
        assert!(striped.validate().is_err());
        long.runs.pop();
        long.validate().unwrap();
        // Runs that change nothing are dropped.
        text.runs = vec![run(0, 2, color(text.color)), run(2, 3, RunStyle::default())];
        text.normalize_runs();
        assert!(text.runs.is_empty());
    }

    #[test]
    fn runs_round_trip_through_json() {
        let mut text = style("APPle");
        text.set_run_style(0..3, &color(RED));
        text.set_run_style(
            3..5,
            &RunStyle {
                family: Some("Menlo".into()),
                bold: Some(true),
                italic: Some(true),
                ..Default::default()
            },
        );
        let json = serde_json::to_value(&text).unwrap();
        assert_eq!(
            json["runs"],
            serde_json::json!([
                {"start": 0, "end": 3, "color": [255, 0, 0, 255]},
                {"start": 3, "end": 5, "family": "Menlo", "bold": true, "italic": true},
            ])
        );
        assert_eq!(serde_json::from_value::<TextStyle>(json).unwrap(), text);
        // Text without runs keeps the old keys and still reads.
        let plain = serde_json::to_value(style("Plain")).unwrap();
        assert!(plain.get("runs").is_none());
        assert!(
            serde_json::from_value::<TextStyle>(plain)
                .unwrap()
                .runs
                .is_empty()
        );
        for bad in [
            serde_json::json!([{"start": -1, "end": 2, "color": [0, 0, 0, 255]}]),
            serde_json::json!([{"start": 0, "end": 2, "color": [0, 0, 0, 256]}]),
            serde_json::json!([{"start": 0, "end": 2, "bold": "yes"}]),
            serde_json::json!({"start": 0}),
        ] {
            let mut json = serde_json::to_value(&text).unwrap();
            json["runs"] = bad;
            assert!(serde_json::from_value::<TextStyle>(json).is_err());
        }
    }
}

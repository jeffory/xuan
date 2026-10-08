//! The command palette (Ctrl+K): a searchable list over the command registry.
//!
//! The registry decides what exists, what is enabled and how it runs
//! ([`EditorApp::command_enabled`], [`EditorApp::run_command`]), so the palette cannot drift from
//! the menus. This file holds the fuzzy matcher, the ranking, the small state machine and the
//! overlay.

use egui::{
    Align2, Color32, FontId, Key, Modifiers, Order, Sense, TextEdit, WidgetInfo, WidgetType,
    text::{LayoutJob, TextFormat},
};
use xuan::i18n::tr;

use super::{
    EditorApp,
    commands::{Category, Entry, Kind},
    shortcuts, theme,
};

/// Recent commands kept in the configuration.
pub(super) const MAX_RECENT: usize = 10;
/// Rows moved by PageUp and PageDown.
const PAGE: usize = 8;
const ROW_HEIGHT: f32 = 26.0;
const WIDTH: f32 = 560.0;
const LIST_HEIGHT: f32 = 340.0;
/// The id of the filter field.
pub(super) const FILTER_ID: &str = "command_palette_filter";

// ---------------------------------------------------------------------------------------------
// Matcher
// ---------------------------------------------------------------------------------------------

/// A successful match: higher scores are better.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Match {
    pub score: i32,
    /// Character (not byte) positions in the text that matched, ascending.
    pub indices: Vec<usize>,
}

const MATCH: i32 = 16;
const START_BONUS: i32 = 12;
const WORD_BONUS: i32 = 10;
const CAMEL_BONUS: i32 = 8;
const PREFIX_BONUS: i32 = 8;
const CONSECUTIVE_BONUS: i32 = 14;
const GAP_PENALTY: i32 = 2;
const GAP_CAP: i32 = 10;
const LEADING_CAP: i32 = 6;
const EXACT_BONUS: i32 = 20;

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Bonus for matching the character at `j`: the start of the text or of a word.
fn boundary_bonus(text: &[char], j: usize) -> i32 {
    if j == 0 {
        return START_BONUS;
    }
    let (previous, current) = (text[j - 1], text[j]);
    if !previous.is_alphanumeric() {
        WORD_BONUS
    } else if previous.is_lowercase() && current.is_uppercase() {
        CAMEL_BONUS
    } else {
        0
    }
}

/// Fuzzy-matches `query` (one word, case-insensitive) against `text`: every query character must
/// appear in order. Matches at the start of the text or of a word, next to the previous match
/// and at the very start score higher; gaps cost a little. Returns the best-scoring alignment
/// with the positions it used, so a caller can highlight them. Works on characters, so CJK text
/// matches like any other.
pub(super) fn fuzzy_match(query: &str, text: &str) -> Option<Match> {
    let query: Vec<char> = query.chars().map(fold).collect();
    let original: Vec<char> = text.chars().collect();
    let folded: Vec<char> = original.iter().copied().map(fold).collect();
    let (m, n) = (query.len(), folded.len());
    if m == 0 {
        return Some(Match {
            score: 0,
            indices: Vec::new(),
        });
    }
    if m > n {
        return None;
    }
    // best[i][j]: the best score with query[i] matched at text[j], and where query[i - 1] was.
    let mut best: Vec<Vec<Option<(i32, usize)>>> = vec![vec![None; n]; m];
    for i in 0..m {
        for j in i..n {
            if folded[j] != query[i] {
                continue;
            }
            let here = MATCH + boundary_bonus(&original, j);
            if i == 0 {
                let prefix = if j == 0 { PREFIX_BONUS } else { 0 };
                best[0][j] = Some((here + prefix - (j as i32).min(LEADING_CAP), 0));
                continue;
            }
            let mut found: Option<(i32, usize)> = None;
            for k in (i - 1)..j {
                let Some((score, _)) = best[i - 1][k] else {
                    continue;
                };
                let link = if k + 1 == j {
                    CONSECUTIVE_BONUS
                } else {
                    -((j - k - 1) as i32 * GAP_PENALTY).min(GAP_CAP)
                };
                let total = score + here + link;
                if found.is_none_or(|(best, _)| total > best) {
                    found = Some((total, k));
                }
            }
            best[i][j] = found;
        }
    }
    let (mut j, (mut score, _)) = (0..n)
        .filter_map(|j| best[m - 1][j].map(|entry| (j, entry)))
        .max_by_key(|&(j, (score, _))| (score, std::cmp::Reverse(j)))?;
    let mut indices = vec![0; m];
    for i in (0..m).rev() {
        indices[i] = j;
        j = best[i][j].map_or(0, |(_, previous)| previous);
    }
    if m == n {
        score += EXACT_BONUS;
    }
    Some(Match { score, indices })
}

/// Matches a (possibly several-word) `query` against a label and other texts that also name the
/// command, each with a penalty. Every word must match somewhere; the highlight covers the
/// characters of the label.
pub(super) fn match_command(query: &str, label: &str, others: &[(&str, i32)]) -> Option<Match> {
    let mut total = 0;
    let mut indices = Vec::new();
    for word in query.split_whitespace() {
        let in_label = fuzzy_match(word, label);
        let elsewhere = others
            .iter()
            .filter_map(|(text, penalty)| fuzzy_match(word, text).map(|m| m.score - penalty))
            .max();
        match (in_label, elsewhere) {
            (Some(label), other) if other.is_none_or(|other| label.score >= other) => {
                total += label.score;
                indices.extend(label.indices);
            }
            (_, Some(other)) => total += other,
            (Some(label), None) => {
                total += label.score;
                indices.extend(label.indices);
            }
            (None, None) => return None,
        }
    }
    indices.sort_unstable();
    indices.dedup();
    Some(Match {
        score: total,
        indices,
    })
}

// ---------------------------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------------------------

/// Where a row sits when the filter is empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Group {
    Recent,
    Category(Category),
    /// A filtered list is just ordered by score.
    Result,
}

/// A command shown in the palette.
#[derive(Clone, Debug)]
pub(super) struct Row {
    /// Position in `Keymap::entries`.
    pub entry: usize,
    pub group: Group,
    /// Label characters to highlight.
    pub indices: Vec<usize>,
}

/// The commands for `query` in palette order. Commands outside the current scope are left out.
/// An empty query lists `recent` commands first, then the rest by category; otherwise rows are
/// ordered by match score, then how recently they were used, then registry order.
pub(super) fn rank(
    entries: &[Entry],
    developing: bool,
    query: &str,
    recent: &[String],
) -> Vec<Row> {
    let in_scope = |entry: &Entry| entry.scope().active(developing);
    let recency = |entry: &Entry| recent.iter().position(|id| *id == entry.id);
    if query.trim().is_empty() {
        let mut rows = Vec::new();
        for id in recent {
            if let Some((entry, _)) = entries
                .iter()
                .enumerate()
                .find(|(_, e)| e.id == *id && in_scope(e))
                && !rows.iter().any(|row: &Row| row.entry == entry)
            {
                rows.push(Row {
                    entry,
                    group: Group::Recent,
                    indices: Vec::new(),
                });
            }
        }
        for category in Category::ALL {
            for (index, entry) in entries.iter().enumerate() {
                if entry.category() == category
                    && in_scope(entry)
                    && !rows.iter().any(|row| row.entry == index)
                {
                    rows.push(Row {
                        entry: index,
                        group: Group::Category(category),
                        indices: Vec::new(),
                    });
                }
            }
        }
        return rows;
    }
    let mut scored: Vec<(i32, Option<usize>, Row)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if !in_scope(entry) {
            continue;
        }
        let english = match &entry.kind {
            Kind::Builtin(command) => command.label,
            Kind::Plugin { .. } => "",
        };
        let category = entry.category();
        let mut others: Vec<(&str, i32)> = vec![(english, 2), (&entry.id, 6)];
        others.extend(entry.aliases().iter().map(|alias| (*alias, 4)));
        others.push((tr(category.name()), 10));
        others.push((category.name(), 10));
        if let Some(found) = match_command(query, entry.label(), &others) {
            scored.push((
                found.score,
                recency(entry),
                Row {
                    entry: index,
                    group: Group::Result,
                    indices: found.indices,
                },
            ));
        }
    }
    // Stable sort: ties keep registry order.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.unwrap_or(usize::MAX).cmp(&b.1.unwrap_or(usize::MAX)))
    });
    scored.into_iter().map(|(_, _, row)| row).collect()
}

/// `recent` after running `id`: it moves to the front and the list stays short.
pub(super) fn push_recent(recent: &mut Vec<String>, id: &str) {
    recent.retain(|r| r != id);
    recent.insert(0, id.to_owned());
    recent.truncate(MAX_RECENT);
}

// ---------------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------------

/// A key that moves the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Move {
    Up,
    Down,
    PageUp,
    PageDown,
}

/// What the palette asks of the app after a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Stay,
    Close,
    /// Close the palette and run this command.
    Run(String),
}

/// What the palette remembers while open.
#[derive(Default)]
pub(super) struct Palette {
    pub query: String,
    /// The highlighted row of the list.
    pub selected: usize,
    /// The selection moved by key and must be scrolled into view.
    pub reveal: bool,
}

impl Palette {
    /// Moves the selection within `len` rows. Up and Down wrap around; pages stop at the ends.
    pub fn step(&mut self, movement: Move, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let last = len - 1;
        self.selected = self.selected.min(last);
        self.selected = match movement {
            Move::Up if self.selected == 0 => last,
            Move::Up => self.selected - 1,
            Move::Down if self.selected == last => 0,
            Move::Down => self.selected + 1,
            Move::PageUp => self.selected.saturating_sub(PAGE),
            Move::PageDown => (self.selected + PAGE).min(last),
        };
        self.reveal = true;
    }

    /// The filter changed: start from the best match again.
    pub fn filter_changed(&mut self) {
        self.selected = 0;
        self.reveal = true;
    }
}

/// Opens the palette, or closes it when it is open. Not while a dialog or job needs the window.
pub(super) fn toggle(app: &mut EditorApp) {
    if app.palette.take().is_some() {
        return;
    }
    if !app.drops_blocked() {
        app.palette = Some(Palette::default());
    }
}

// ---------------------------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------------------------

/// Keys the palette takes from the input before the filter field sees them.
#[derive(Default)]
struct Keys {
    close: bool,
    enter: bool,
    moves: Vec<Move>,
}

fn take_keys(ctx: &egui::Context, toggle_keys: &[super::commands::Chord]) -> Keys {
    let mut keys = Keys::default();
    ctx.input_mut(|input| {
        // Ctrl+K again closes it, whatever it was rebound to.
        for chord in toggle_keys {
            if shortcuts::consume_exact(input, chord.mods, chord.key) {
                keys.close = true;
            }
        }
        if input.consume_key(Modifiers::NONE, Key::Escape) {
            keys.close = true;
        }
        // In the order they were pressed.
        let pressed: Vec<Key> = input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if *modifiers == Modifiers::NONE => Some(*key),
                _ => None,
            })
            .collect();
        for key in pressed {
            let movement = match key {
                Key::ArrowUp => Some(Move::Up),
                Key::ArrowDown => Some(Move::Down),
                Key::PageUp => Some(Move::PageUp),
                Key::PageDown => Some(Move::PageDown),
                Key::Enter => {
                    keys.enter = true;
                    None
                }
                _ => continue,
            };
            input.consume_key(Modifiers::NONE, key);
            keys.moves.extend(movement);
        }
    });
    keys
}

/// The label with `indices` highlighted.
fn label_job(label: &str, indices: &[usize], color: Color32, font: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(font.clone(), color);
    let mut highlighted = TextFormat::simple(font, theme::ACCENT);
    highlighted.underline = egui::Stroke::new(1.0_f32, theme::ACCENT);
    let mut run = String::new();
    let mut run_highlighted = false;
    for (position, c) in label.chars().enumerate() {
        let hit = indices.binary_search(&position).is_ok();
        if hit != run_highlighted && !run.is_empty() {
            let format = if run_highlighted {
                &highlighted
            } else {
                &plain
            };
            job.append(&run, 0.0, format.clone());
            run.clear();
        }
        run_highlighted = hit;
        run.push(c);
    }
    if !run.is_empty() {
        job.append(&run, 0.0, if run_highlighted { highlighted } else { plain });
    }
    job
}

impl EditorApp {
    /// Shows the palette when it is open and carries out what the user asked of it.
    pub(super) fn command_palette(&mut self, ctx: &egui::Context) {
        let Some(mut palette) = self.palette.take() else {
            return;
        };
        // A dialog or job that appeared since (a plugin, a drop) takes over the window.
        if self.drops_blocked() {
            return;
        }
        let keys = take_keys(ctx, self.keymap.keys("command_palette"));
        let outcome = self.palette_frame(ctx, &mut palette, keys);
        match outcome {
            Outcome::Stay => self.palette = Some(palette),
            Outcome::Close => {}
            Outcome::Run(id) => {
                push_recent(&mut self.config.recent_commands, &id);
                self.save_config();
                self.run_command(&id);
            }
        }
    }

    fn palette_frame(&mut self, ctx: &egui::Context, palette: &mut Palette, keys: Keys) -> Outcome {
        let developing = self.develop.is_some();
        let mut outcome = Outcome::Stay;
        if keys.close {
            return Outcome::Close;
        }
        let area = egui::Area::new(egui::Id::new("command_palette"))
            .kind(egui::UiKind::Modal)
            .sense(Sense::hover())
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 64.0))
            .order(Order::Foreground)
            .interactable(true);
        let frame = egui::Frame::popup(&ctx.style())
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0_f32, theme::DIVIDER))
            .corner_radius(8)
            .inner_margin(8);
        let response = egui::Modal::new(egui::Id::new("command_palette"))
            .area(area)
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(70))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                let id = egui::Id::new(FILTER_ID);
                let filter = ui.add(
                    TextEdit::singleline(&mut palette.query)
                        .id(id)
                        .hint_text(tr("Type a command…"))
                        .desired_width(f32::INFINITY)
                        .margin(egui::vec2(8.0, 6.0)),
                );
                if filter.changed() {
                    palette.filter_changed();
                }
                // Typing goes to the filter whatever was clicked.
                ui.memory_mut(|memory| memory.request_focus(id));

                let rows = rank(
                    self.keymap.entries(),
                    developing,
                    &palette.query,
                    &self.config.recent_commands,
                );
                for movement in &keys.moves {
                    palette.step(*movement, rows.len());
                }
                palette.selected = palette.selected.min(rows.len().saturating_sub(1));
                if keys.enter
                    && let Some(row) = rows.get(palette.selected)
                {
                    let entry = &self.keymap.entries()[row.entry];
                    if self.command_enabled(&entry.id) {
                        outcome = Outcome::Run(entry.id.clone());
                    }
                }

                ui.add_space(6.0);
                if rows.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(tr("No matching commands")).color(theme::MUTED));
                    ui.add_space(8.0);
                    return;
                }
                let clicked = self.palette_list(ui, palette, &rows);
                if let Some(index) = clicked {
                    let id = self.keymap.entries()[rows[index].entry].id.clone();
                    outcome = Outcome::Run(id);
                }
                palette.reveal = false;
            });
        if response.backdrop_response.clicked() && outcome == Outcome::Stay {
            outcome = Outcome::Close;
        }
        outcome
    }

    /// The scrolling list. Returns the row clicked, if it can run.
    fn palette_list(&self, ui: &mut egui::Ui, palette: &Palette, rows: &[Row]) -> Option<usize> {
        let mut clicked = None;
        let filtering = !palette.query.trim().is_empty();
        egui::ScrollArea::vertical()
            .id_salt("command_palette_list")
            .max_height(LIST_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut previous = None;
                for (index, row) in rows.iter().enumerate() {
                    if !filtering && previous != Some(row.group) {
                        let name = match row.group {
                            Group::Recent => tr("Recent"),
                            Group::Category(category) => tr(category.name()),
                            Group::Result => "",
                        };
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(name).small().color(theme::MUTED));
                    }
                    previous = Some(row.group);
                    let entry = &self.keymap.entries()[row.entry];
                    let enabled = self.command_enabled(&entry.id);
                    let selected = index == palette.selected;
                    if self.palette_row(ui, entry, row, enabled, selected, palette.reveal) {
                        clicked = Some(index);
                    }
                }
            });
        clicked
    }

    /// One row. Returns whether it was clicked while enabled.
    fn palette_row(
        &self,
        ui: &mut egui::Ui,
        entry: &Entry,
        row: &Row,
        enabled: bool,
        selected: bool,
        reveal: bool,
    ) -> bool {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        let category = tr(entry.category().name());
        let label = entry.label();
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, enabled, format!("{label}, {category}"))
        });
        if selected && reveal {
            ui.scroll_to_rect(rect, None);
        }
        if !ui.is_rect_visible(rect) {
            return false;
        }
        let fill = if selected {
            Some(theme::ACCENT.gamma_multiply(0.35))
        } else if response.hovered() {
            Some(Color32::from_white_alpha(18))
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, 4.0, fill);
        }
        let (text, muted) = if enabled {
            (theme::TEXT, theme::MUTED)
        } else {
            (
                theme::MUTED.gamma_multiply(0.7),
                theme::MUTED.gamma_multiply(0.5),
            )
        };
        let font = FontId::proportional(14.0);
        let galley = ui
            .painter()
            .layout_job(label_job(label, &row.indices, text, font.clone()));
        let left = rect.left() + 8.0;
        let label_width = galley.size().x;
        ui.painter().galley(
            egui::pos2(left, rect.center().y - galley.size().y / 2.0),
            galley,
            text,
        );
        let shortcut = entry.shortcut();
        let mut right = rect.right() - 8.0;
        if !shortcut.is_empty() {
            let shortcut_rect = ui.painter().text(
                egui::pos2(right, rect.center().y),
                Align2::RIGHT_CENTER,
                shortcut,
                FontId::proportional(12.0),
                muted,
            );
            right = shortcut_rect.left() - 12.0;
        }
        let category_left = left + label_width + 12.0;
        if category_left < right {
            let galley =
                ui.painter()
                    .layout_no_wrap(category.to_owned(), FontId::proportional(12.0), muted);
            if category_left + galley.size().x <= right {
                ui.painter().galley(
                    egui::pos2(category_left, rect.center().y - galley.size().y / 2.0),
                    galley,
                    muted,
                );
            }
        }
        enabled && response.clicked()
    }
}

//! The document tab bar (issues #48 and #50).
//!
//! Issue #50 restyled them after KDE's (Breeze) tabs and moved the bar directly above the
//! canvas. Tabs are flat, with a document icon, 1 px separators and a lighter active tab with
//! an accent line; the others light up under the pointer.
//! Tabs share the width between [`MIN_TAB`] and [`MAX_TAB`]; when even the narrowest no longer
//! fit, the strip scrolls, with arrows and a "List all tabs" menu at its end. Tabs can be
//! dragged into a new order, closed with the middle button, and have a context menu. The
//! keyboard commands (Ctrl+Tab, Alt+1…9, Ctrl+Page Up/Down, Ctrl+Shift+T) are in the
//! command registry.
//!
//! The layout and overflow maths are pure functions, tested on their own.

use std::path::{Path, PathBuf};

use egui::{FontId, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use uuid::Uuid;
use xuan::i18n::tr;

use super::{
    EditorApp,
    theme::{Palette, PaletteExt},
    widgets,
};

/// Narrowest and widest tab, in points (Firefox's are 76 and 225).
pub(super) const MIN_TAB: f32 = 76.0;
pub(super) const MAX_TAB: f32 = 225.0;
pub(super) const TAB_HEIGHT: f32 = 36.0;
/// Space inside a tab before its icon.
const PADDING: f32 = 8.0;
/// The document icon's size.
const ICON: f32 = 16.0;
/// Space between two tabs: a 1 px separator.
pub(super) const GAP: f32 = 1.0;
/// Closed documents remembered for Reopen Closed Tab.
const CLOSED_LIMIT: usize = 20;
/// Width of the overflow arrows and the list button.
const STRIP_BUTTON: f32 = 24.0;
/// Room kept on the right of the bar for the zoom buttons.
const ZOOM_BUTTONS: f32 = 228.0;

/// One tab: a document, or a RAW Develop session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TabKey {
    Document(Uuid),
    Raw(Uuid),
}

// ---------------------------------------------------------------------------
// Layout maths

/// The width of each of `count` tabs sharing `available` points, and of all of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Strip {
    pub tab: f32,
    pub content: f32,
}

impl Strip {
    pub fn new(count: usize, available: f32) -> Self {
        if count == 0 {
            return Self {
                tab: MAX_TAB,
                content: 0.0,
            };
        }
        let gaps = GAP * (count - 1) as f32;
        let tab = ((available - gaps) / count as f32).clamp(MIN_TAB, MAX_TAB);
        Self {
            tab,
            content: tab * count as f32 + gaps,
        }
    }

    /// Whether the tabs no longer fit in `available`, so the strip scrolls.
    pub fn overflows(self, available: f32) -> bool {
        self.content > available + 0.5
    }

    /// The left edge of tab `index`, from the start of the strip.
    pub fn left(self, index: usize) -> f32 {
        index as f32 * (self.tab + GAP)
    }

    /// The scroll offset nearest `offset` that keeps the strip inside a `visible`-wide view.
    pub fn clamp_scroll(self, offset: f32, visible: f32) -> f32 {
        offset.clamp(0.0, (self.content - visible).max(0.0))
    }

    /// The scroll offset nearest `offset` that shows all of tab `index`.
    pub fn reveal(self, offset: f32, index: usize, visible: f32) -> f32 {
        let left = self.left(index);
        let right = left + self.tab;
        let offset = if left < offset {
            left
        } else if right > offset + visible {
            right - visible
        } else {
            offset
        };
        self.clamp_scroll(offset, visible)
    }

    /// Where a tab dropped at `x` (from the start of the strip) goes: the gap before tab
    /// `slot`, 0 to `count`.
    pub fn drop_slot(self, x: f32, count: usize) -> usize {
        let pitch = self.tab + GAP;
        (((x + GAP / 2.0) / pitch).round().max(0.0) as usize).min(count)
    }
}

/// The index a tab moved from `from` to gap `slot` ends up at.
pub(super) fn moved_index(from: usize, slot: usize) -> usize {
    if slot > from { slot - 1 } else { slot }
}

/// The tab after (or before) `current` of `count`, wrapping round.
pub(super) fn cycle(current: usize, count: usize, forward: bool) -> usize {
    match (count, forward) {
        (0, _) => 0,
        (_, true) => (current + 1) % count,
        (_, false) => (current + count - 1) % count,
    }
}

/// Tab `n` (1-based) for the Alt+1…9 keys: 9 is always the last tab, as in browsers.
pub(super) fn nth(n: usize, count: usize) -> Option<usize> {
    match n {
        _ if count == 0 => None,
        9 => Some(count - 1),
        1..=8 if n <= count => Some(n - 1),
        _ => None,
    }
}

/// The tabs Close Others (`right_only` false) or Close Tabs to the Right closes, for the tab at
/// `index` of `count`.
pub(super) fn others(index: usize, count: usize, right_only: bool) -> Vec<usize> {
    (0..count)
        .filter(|&i| if right_only { i > index } else { i != index })
        .collect()
}

// ---------------------------------------------------------------------------
// Painting

/// A tab's look, from its state.
struct TabLook<'a> {
    title: &'a str,
    selected: bool,
    dirty: bool,
    hovered: bool,
    close_hovered: bool,
    dragging: bool,
}

/// Whether the close button shows (else a dirty tab shows its dot there).
fn shows_close(look: &TabLook<'_>) -> bool {
    look.hovered || look.close_hovered || (look.selected && !look.dirty)
}

fn close_rect(tab: Rect) -> Rect {
    Rect::from_center_size(pos2(tab.right() - 18.0, tab.center().y), vec2(20.0, 20.0))
}

/// A small page with a folded corner, drawn in `color`: the document icon at a tab's left.
fn paint_document_icon(painter: &Painter, rect: Rect, color: egui::Color32) {
    let stroke = Stroke::new(1.2_f32, color);
    let r = rect.shrink2(vec2(2.5, 1.0));
    let fold = 4.5;
    let outline = [
        r.left_top(),
        pos2(r.right() - fold, r.top()),
        pos2(r.right(), r.top() + fold),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.add(egui::Shape::line(outline.to_vec(), stroke));
    painter.add(egui::Shape::line(
        vec![
            pos2(r.right() - fold, r.top()),
            pos2(r.right() - fold, r.top() + fold),
            pos2(r.right(), r.top() + fold),
        ],
        stroke,
    ));
}

fn paint_tab(ui: &Ui, painter: &Painter, p: &Palette, rect: Rect, look: &TabLook<'_>) {
    // KDE (Breeze) tabs: flat; the active one a step lighter than the bar, with an accent
    // line on top; a faint fill under the pointer.
    if look.selected {
        painter.rect_filled(rect, 0.0, p.panel);
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(rect.width(), 2.0)),
            0.0,
            p.accent,
        );
    } else if look.hovered {
        painter.rect_filled(rect, 0.0, p.tab_hover);
    }
    let close = close_rect(rect);
    let text = if look.dragging {
        p.muted
    } else if look.selected || look.hovered {
        p.text
    } else {
        p.text.lerp_to_gamma(p.muted, 0.35)
    };
    let icon = Rect::from_center_size(
        pos2(rect.left() + PADDING + ICON / 2.0, rect.center().y),
        vec2(ICON, ICON),
    );
    paint_document_icon(painter, icon, text);
    let text_rect = Rect::from_min_max(
        pos2(icon.right() + 6.0, rect.top()),
        pos2(close.left() - 2.0, rect.bottom()),
    );
    let galley = egui::WidgetText::from(look.title).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        text_rect.width().max(0.0),
        FontId::proportional(12.0),
    );
    painter
        .with_clip_rect(text_rect)
        .galley_with_override_text_color(
            pos2(text_rect.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            text,
        );
    if shows_close(look) {
        if look.close_hovered {
            painter.circle_filled(close.center(), 9.0, p.tab_close_hover);
        }
        let c = close.center();
        let stroke = Stroke::new(1.2_f32, if look.close_hovered { p.text } else { p.muted });
        painter.line_segment([c - vec2(3.5, 3.5), c + vec2(3.5, 3.5)], stroke);
        painter.line_segment([c + vec2(-3.5, 3.5), c + vec2(3.5, -3.5)], stroke);
    } else if look.dirty {
        painter.circle_filled(close.center(), 3.5, p.text);
    }
}

// ---------------------------------------------------------------------------
// The tab bar

/// What the tab bar asked for this frame; carried out once drawing is done.
#[derive(Default)]
struct Actions {
    activate: Option<TabKey>,
    close: Vec<TabKey>,
    reorder: Option<(usize, usize)>,
    command: Option<&'static str>,
    copy_layer: Option<(super::LayerDrag, usize)>,
    copy_path: Option<PathBuf>,
    reveal: Option<PathBuf>,
}

/// State kept between frames.
#[derive(Default)]
pub(super) struct TabStrip {
    /// Scroll offset of an overflowing strip, in points.
    offset: f32,
    /// The selected tab last frame, so a newly selected one is scrolled into view.
    shown: Option<TabKey>,
    /// Paths of closed documents, most recent last.
    pub closed: Vec<PathBuf>,
    /// Tabs waiting to be closed (Close Others, Close Tabs to the Right), one at a time, so
    /// each unsaved one can ask first.
    pub close_queue: std::collections::VecDeque<TabKey>,
    /// The queued tab whose close was last asked for.
    closing: Option<TabKey>,
}

impl EditorApp {
    /// Every tab in order: documents, then RAW sessions by when they were opened.
    pub(super) fn tab_keys(&self) -> Vec<TabKey> {
        let mut raws: Vec<_> = self.develop.iter().chain(&self.inactive_develop).collect();
        raws.sort_by_key(|d| d.opened);
        self.sessions
            .iter()
            .map(|s| TabKey::Document(s.document.id))
            .chain(raws.into_iter().map(|d| TabKey::Raw(d.id)))
            .collect()
    }

    pub(super) fn selected_tab(&self) -> Option<TabKey> {
        match &self.develop {
            Some(d) => Some(TabKey::Raw(d.id)),
            None => self.session().map(|s| TabKey::Document(s.document.id)),
        }
    }

    fn document_index(&self, id: Uuid) -> Option<usize> {
        self.sessions.iter().position(|s| s.document.id == id)
    }

    fn tab_exists(&self, key: TabKey) -> bool {
        self.tab_keys().contains(&key)
    }

    fn tab_title(&self, key: TabKey) -> String {
        match key {
            TabKey::Document(id) => self
                .document_index(id)
                .map(|i| self.sessions[i].title.clone())
                .unwrap_or_default(),
            TabKey::Raw(id) => self
                .develop
                .iter()
                .chain(&self.inactive_develop)
                .find(|d| d.id == id)
                .map(|d| {
                    if d.is_filter() {
                        format!("{} · {}", d.title, tr("Camera Raw"))
                    } else {
                        format!("{} · RAW", d.title)
                    }
                })
                .unwrap_or_default(),
        }
    }

    fn tab_path(&self, key: TabKey) -> Option<&Path> {
        match key {
            TabKey::Document(id) => self.document_index(id).and_then(|i| {
                let session = &self.sessions[i];
                session.path.as_deref().or(session.source.as_deref())
            }),
            TabKey::Raw(_) => None,
        }
    }

    fn tab_dirty(&self, key: TabKey) -> bool {
        match key {
            TabKey::Document(id) => self
                .document_index(id)
                .is_some_and(|i| self.sessions[i].history.edited()),
            TabKey::Raw(id) => self
                .develop
                .iter()
                .chain(&self.inactive_develop)
                .find(|d| d.id == id)
                .is_some_and(|d| {
                    d.asset
                        .as_ref()
                        .is_some_and(|asset| asset.settings != d.settings)
                }),
        }
    }

    pub(super) fn activate_tab(&mut self, key: TabKey) {
        match key {
            TabKey::Document(id) => {
                if let Some(index) = self.document_index(id) {
                    self.cancel_gesture();
                    self.suspend_develop();
                    self.current = index;
                    self.mask_target = false;
                }
            }
            TabKey::Raw(id) => self.activate_develop(id),
        }
    }

    /// Starts closing `key`, through the unsaved-changes prompt where needed.
    fn request_tab_close(&mut self, key: TabKey) {
        match key {
            TabKey::Document(id) => {
                if let Some(index) = self.document_index(id) {
                    self.request_project_close(index);
                }
            }
            TabKey::Raw(id) => {
                self.activate_develop(id);
                self.request_develop_close(super::develop::DevelopClose::Tab);
            }
        }
    }

    /// Queues `keys` to be closed one after another.
    pub(super) fn close_tabs(&mut self, keys: impl IntoIterator<Item = TabKey>) {
        self.tab_strip.close_queue.extend(keys);
        self.process_tab_closes();
    }

    /// Closes the next queued tab once nothing else is being asked. A tab still open after
    /// its prompt was answered was kept (Cancel), which stops the rest.
    pub(super) fn process_tab_closes(&mut self) {
        let busy = self.close_tab.is_some()
            || self.develop_close_requested.is_some()
            || self.job.is_some()
            || self.close_app;
        if busy {
            return;
        }
        if let Some(previous) = self.tab_strip.closing.take()
            && self.tab_exists(previous)
        {
            self.tab_strip.close_queue.clear();
            return;
        }
        while let Some(key) = self.tab_strip.close_queue.pop_front() {
            if self.tab_exists(key) {
                self.tab_strip.closing = Some(key);
                self.request_tab_close(key);
                return;
            }
        }
    }

    /// Remembers a document that is being closed, for Reopen Closed Tab.
    pub(super) fn remember_closed(&mut self, index: usize) {
        if let Some(path) = self
            .sessions
            .get(index)
            .and_then(|s| s.path.clone().or_else(|| s.source.clone()))
        {
            let closed = &mut self.tab_strip.closed;
            closed.retain(|p| *p != path);
            closed.push(path);
            if closed.len() > CLOSED_LIMIT {
                closed.remove(0);
            }
        }
    }

    pub(super) fn reopen_closed_tab(&mut self) {
        while let Some(path) = self.tab_strip.closed.pop() {
            if path.exists() {
                self.open_path(&path, false);
                return;
            }
        }
    }

    /// Ctrl+Tab and friends: `next_tab`, `previous_tab`, `tab_1` … `tab_9`, `reopen_closed_tab`,
    /// `close_other_tabs`, `close_tabs_to_right`. Returns whether `command` was one of them.
    pub(super) fn tab_command(&mut self, command: &str) -> bool {
        let keys = self.tab_keys();
        let current = self
            .selected_tab()
            .and_then(|key| keys.iter().position(|k| *k == key));
        let target = match command {
            "next_tab" | "previous_tab" => {
                current.map(|i| cycle(i, keys.len(), command == "next_tab"))
            }
            "reopen_closed_tab" => {
                self.reopen_closed_tab();
                return true;
            }
            "close_other_tabs" | "close_tabs_to_right" => {
                if let Some(index) = current {
                    let right_only = command == "close_tabs_to_right";
                    let close: Vec<_> = others(index, keys.len(), right_only)
                        .into_iter()
                        .map(|i| keys[i])
                        .collect();
                    self.close_tabs(close);
                }
                return true;
            }
            _ => match command
                .strip_prefix("tab_")
                .and_then(|n| n.parse::<usize>().ok())
            {
                Some(n) => nth(n, keys.len()),
                None => return false,
            },
        };
        if let Some(index) = target {
            self.activate_tab(keys[index]);
        }
        true
    }

    /// Moves the document tab at `from` to `to`, keeping the same document selected.
    fn move_document(&mut self, from: usize, to: usize) {
        if from == to || from >= self.sessions.len() || to >= self.sessions.len() {
            return;
        }
        let current = self.session().map(|s| s.document.id);
        let session = self.sessions.remove(from);
        self.sessions.insert(to, session);
        if let Some(index) = current.and_then(|id| self.document_index(id)) {
            self.current = index;
        }
    }

    pub(super) fn tabs(&mut self, ctx: &egui::Context) {
        self.process_tab_closes();
        let blocked = self.job.is_some()
            || self.dialog.is_some()
            || self.error.is_some()
            || self.close_app
            || self.close_tab.is_some()
            || self.develop_close_requested.is_some();
        let mut actions = Actions::default();
        egui::TopBottomPanel::top("project_tabs")
            .exact_height(TAB_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(ctx.palette().titlebar)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                ui.add_enabled_ui(!blocked, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        self.tab_strip_ui(ui, &mut actions);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add_space(8.0);
                            // Nothing to zoom until a document is open. The strip still
                            // reserves this width, so the tabs do not shift when one opens.
                            if self.session().is_some() || self.develop.is_some() {
                                self.zoom_buttons(ui, &mut actions);
                            }
                        });
                    });
                });
            });
        self.apply_tab_actions(ctx, actions);
    }

    fn zoom_buttons(&self, ui: &mut Ui, actions: &mut Actions) {
        ui.add_enabled_ui(self.develop.as_ref().is_none_or(|d| d.ready()), |ui| {
            // The strip lays out right to left, so the buttons are added in reverse to read
            // "− + 100% Fit", the Navigator's order for − and +.
            if widgets::button(ui, tr("Fit")).clicked() {
                actions.command = Some("fit");
            }
            if widgets::button(ui, "100%").clicked() {
                actions.command = Some("actual");
            }
            if widgets::button(ui, "+")
                .on_hover_text(tr("Zoom in"))
                .clicked()
            {
                actions.command = Some("zoom_in");
            }
            if widgets::button(ui, "−")
                .on_hover_text(tr("Zoom out"))
                .clicked()
            {
                actions.command = Some("zoom_out");
            }
        });
    }

    fn tab_strip_ui(&mut self, ui: &mut Ui, actions: &mut Actions) {
        let p = ui.palette();
        let keys = self.tab_keys();
        let selected = self.selected_tab();
        let documents = self.sessions.len();
        let new_button = TAB_HEIGHT;
        let available = (ui.available_width() - ZOOM_BUTTONS - new_button).max(MIN_TAB);
        let strip = Strip::new(keys.len(), available);
        let overflow = strip.overflows(available);
        let visible = if overflow {
            (available - 3.0 * (STRIP_BUTTON + ui.spacing().item_spacing.x)).max(MIN_TAB)
        } else {
            available
        };
        let strip = if overflow {
            Strip::new(keys.len(), visible)
        } else {
            strip
        };
        // Scroll a newly selected tab into view.
        if selected != self.tab_strip.shown {
            self.tab_strip.shown = selected;
            if let Some(index) = selected.and_then(|key| keys.iter().position(|k| *k == key)) {
                self.tab_strip.offset = strip.reveal(self.tab_strip.offset, index, visible);
            }
        }
        let mut offset = strip.clamp_scroll(self.tab_strip.offset, visible);

        if overflow
            && ui
                .add(widgets::Button::new("‹").min_size(vec2(STRIP_BUTTON, 26.0)))
                .on_hover_text(tr("Scroll tabs left"))
                .clicked()
        {
            offset = strip.clamp_scroll(offset - strip.tab - GAP, visible);
        }

        let width = if overflow {
            visible
        } else {
            strip.content.max(0.0)
        };
        let (viewport, background) =
            ui.allocate_exact_size(vec2(width, TAB_HEIGHT), Sense::click());
        // A double click on the bar's empty part starts a new document, as in browsers.
        let empty = ui.interact(
            Rect::from_min_max(
                viewport.right_top(),
                pos2(
                    viewport.right() + ui.available_width() - ZOOM_BUTTONS,
                    viewport.bottom(),
                ),
            ),
            ui.id().with("tab_bar_empty"),
            Sense::click(),
        );
        if background.double_clicked() || empty.double_clicked() {
            actions.command = Some("new");
        }
        if overflow && ui.rect_contains_pointer(viewport) {
            let delta = ui.input(|i| i.smooth_scroll_delta);
            let delta = if delta.x != 0.0 { delta.x } else { delta.y };
            offset = strip.clamp_scroll(offset - delta, visible);
        }
        let clip = ui
            .painter()
            .clip_rect()
            .intersect(viewport.expand2(vec2(0.0, 2.0)));
        let mut drag = None;
        for (index, &key) in keys.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(viewport.left() + strip.left(index) - offset, viewport.top()),
                vec2(strip.tab, TAB_HEIGHT),
            );
            if !rect.intersects(viewport) {
                continue;
            }
            let title = self.tab_title(key);
            let id = ui.id().with(("tab", key));
            let response = ui.interact(
                rect.intersect(viewport),
                id,
                if matches!(key, TabKey::Document(_)) {
                    Sense::click_and_drag()
                } else {
                    Sense::click()
                },
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    ui.is_enabled(),
                    selected == Some(key),
                    &title,
                )
            });
            let close = ui.interact(
                close_rect(rect).intersect(viewport),
                id.with("close"),
                Sense::click(),
            );
            let close_label = format!("{} {title}", tr("Close"));
            close.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &close_label)
            });
            let dragging = response.dragged();
            let look = TabLook {
                title: &title,
                selected: selected == Some(key),
                dirty: self.tab_dirty(key),
                hovered: response.hovered() || dragging,
                // A focused close button shows, as under the pointer.
                close_hovered: close.hovered() || close.has_focus(),
                dragging,
            };
            paint_tab(ui, &ui.painter().with_clip_rect(clip), &p, rect, &look);
            // Inside the tab: tabs sit edge to edge and the strip clips them.
            widgets::focus_ring_at(
                ui,
                &response,
                rect.intersect(viewport),
                0.0,
                widgets::FocusRing::Inside,
            );
            widgets::focus_ring_at(
                ui,
                &close,
                close_rect(rect).intersect(viewport),
                10.0,
                widgets::FocusRing::Inside,
            );
            if index + 1 < keys.len() {
                // The 1 px separator in the gap after this tab, not after the last.
                let x = rect.right() + GAP / 2.0;
                ui.painter().with_clip_rect(clip).line_segment(
                    [pos2(x, rect.top() + 8.0), pos2(x, rect.bottom() - 8.0)],
                    Stroke::new(GAP, p.divider),
                );
            }
            if let TabKey::Document(_) = key {
                if let Some(layer) = response.dnd_release_payload::<super::LayerDrag>() {
                    actions.copy_layer = Some((*layer, index));
                }
                if response.dnd_hover_payload::<super::LayerDrag>().is_some() {
                    ui.painter().with_clip_rect(clip).rect_stroke(
                        rect,
                        0.0,
                        Stroke::new(2.0_f32, p.accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            let close = close.on_hover_text(tr("Close Tab"));
            if close.clicked() || (response.clicked_by(egui::PointerButton::Middle)) {
                actions.close.push(key);
            } else if response.clicked() {
                actions.activate = Some(key);
            }
            if dragging || response.drag_stopped() {
                drag = Some((index, response.drag_stopped()));
            }
            let tooltip = self
                .tab_path(key)
                .map_or_else(|| title.clone(), |path| path.display().to_string());
            let response = response.on_hover_text(tooltip);
            response.context_menu(|ui| self.tab_menu(ui, key, &keys, actions));
        }

        // Dragging a document tab: an insertion line where it would land.
        if let Some((from, released)) = drag
            && let Some(pointer) = ui.ctx().pointer_interact_pos()
        {
            let x = pointer.x - viewport.left() + offset;
            let slot = strip.drop_slot(x, keys.len()).min(documents);
            let line_x = viewport.left() + strip.left(slot) - offset - GAP / 2.0;
            if released {
                actions.reorder = Some((from, moved_index(from, slot)));
            } else {
                ui.painter().with_clip_rect(clip.expand(2.0)).line_segment(
                    [
                        pos2(line_x, viewport.top() + 2.0),
                        pos2(line_x, viewport.bottom() - 2.0),
                    ],
                    Stroke::new(2.0_f32, p.accent),
                );
                // Scroll when held near either end of an overflowing strip.
                if overflow {
                    if pointer.x < viewport.left() + 20.0 {
                        offset = strip.clamp_scroll(offset - 8.0, visible);
                    } else if pointer.x > viewport.right() - 20.0 {
                        offset = strip.clamp_scroll(offset + 8.0, visible);
                    }
                    ui.ctx().request_repaint();
                }
            }
        }

        if overflow {
            if ui
                .add(widgets::Button::new("›").min_size(vec2(STRIP_BUTTON, 26.0)))
                .on_hover_text(tr("Scroll tabs right"))
                .clicked()
            {
                offset = strip.clamp_scroll(offset + strip.tab + GAP, visible);
            }
            let list = ui.add(widgets::Button::new("⌄").min_size(vec2(STRIP_BUTTON, 26.0)));
            list.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    tr("List all tabs"),
                )
            });
            let list = list.on_hover_text(tr("List all tabs"));
            egui::Popup::menu(&list).show(|ui| {
                ui.set_min_width(220.0);
                for &key in &keys {
                    if widgets::menu_check(ui, selected == Some(key), &self.tab_title(key), "")
                        .clicked()
                    {
                        actions.activate = Some(key);
                        ui.close();
                    }
                }
            });
        }
        self.tab_strip.offset = offset;

        let (rect, new) = ui.allocate_exact_size(vec2(TAB_HEIGHT, TAB_HEIGHT), Sense::click());
        new.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tr("New canvas"))
        });
        if new.hovered() {
            ui.painter().rect_filled(rect, 0.0, p.tab_hover);
        }
        widgets::focus_ring_at(ui, &new, rect, 0.0, widgets::FocusRing::Inside);
        let c = rect.center();
        let stroke = Stroke::new(1.4_f32, p.text);
        ui.painter()
            .line_segment([c - vec2(5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
        ui.painter()
            .line_segment([c - vec2(0.0, 5.0), c + vec2(0.0, 5.0)], stroke);
        if new
            .on_hover_text(match self.keymap.shortcut("new") {
                shortcut if shortcut.is_empty() => tr("New canvas").to_owned(),
                shortcut => format!("{} ({shortcut})", tr("New canvas")),
            })
            .clicked()
        {
            actions.command = Some("new");
        }
    }

    /// A tab's context menu.
    fn tab_menu(&self, ui: &mut Ui, key: TabKey, keys: &[TabKey], actions: &mut Actions) {
        let index = keys.iter().position(|k| *k == key).unwrap_or(0);
        let shortcut = |id: &str| self.keymap.shortcut(id);
        if widgets::menu_check(ui, false, tr("Close Tab"), &shortcut("close")).clicked() {
            actions.close.push(key);
            ui.close();
        }
        ui.add_enabled_ui(keys.len() > 1, |ui| {
            if widgets::menu_check(ui, false, tr("Close Other Tabs"), "").clicked() {
                actions.close.extend(
                    others(index, keys.len(), false)
                        .into_iter()
                        .map(|i| keys[i]),
                );
                ui.close();
            }
        });
        ui.add_enabled_ui(index + 1 < keys.len(), |ui| {
            if widgets::menu_check(ui, false, tr("Close Tabs to the Right"), "").clicked() {
                actions
                    .close
                    .extend(others(index, keys.len(), true).into_iter().map(|i| keys[i]));
                ui.close();
            }
        });
        ui.separator();
        ui.add_enabled_ui(!self.tab_strip.closed.is_empty(), |ui| {
            if widgets::menu_check(
                ui,
                false,
                tr("Reopen Closed Tab"),
                &shortcut("reopen_closed_tab"),
            )
            .clicked()
            {
                actions.command = Some("reopen_closed_tab");
                ui.close();
            }
        });
        ui.separator();
        let path = self.tab_path(key);
        ui.add_enabled_ui(path.is_some(), |ui| {
            if widgets::menu_check(ui, false, tr("Copy Path"), "").clicked() {
                actions.copy_path = path.map(Path::to_path_buf);
                ui.close();
            }
            if widgets::menu_check(ui, false, tr("Show in Folder"), "").clicked() {
                actions.reveal = path.map(Path::to_path_buf);
                ui.close();
            }
        });
    }

    fn apply_tab_actions(&mut self, ctx: &egui::Context, actions: Actions) {
        if let Some((layer, destination)) = actions.copy_layer {
            self.copy_layer_to_project(layer, destination);
        }
        if let Some((from, to)) = actions.reorder {
            self.move_document(from, to);
        }
        if let Some(key) = actions.activate {
            self.activate_tab(key);
        }
        if let Some(path) = actions.copy_path {
            ctx.copy_text(path.display().to_string());
        }
        if let Some(path) = actions.reveal
            && let Err(error) = show_in_folder(&path)
        {
            self.error = Some(format!("{}\n\n{error}", tr("Could not show the file")));
        }
        if !actions.close.is_empty() {
            self.close_tabs(actions.close);
        }
        if let Some(command) = actions.command {
            self.command(command);
        }
    }
}

/// Opens the file manager at `path`, with the file selected where the platform allows.
pub(super) fn show_in_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    {
        // Tests never start a file manager.
        let _ = path;
        Ok(())
    }
    #[cfg(all(not(test), windows))]
    {
        // `explorer /select,` wants the path as one argument with no quotes inside.
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map(drop)
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(drop)
    }
    #[cfg(all(not(test), not(windows), not(target_os = "macos")))]
    {
        // The freedesktop file-manager interface selects the file (Dolphin, Nautilus,
        // Nemo, …); without one, open the folder. Off the UI thread, as D-Bus may be slow.
        let uri = file_uri(path);
        let folder = path.parent().unwrap_or(Path::new("/")).to_path_buf();
        std::thread::spawn(move || {
            let selected = std::process::Command::new("dbus-send")
                .args([
                    "--session",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "--reply-timeout=2000",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                    &format!("array:string:{uri}"),
                    "string:",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !selected {
                let _ = std::process::Command::new("xdg-open").arg(folder).spawn();
            }
        });
        Ok(())
    }
}

/// A `file://` URI for an absolute path, percent-encoding everything but unreserved
/// characters and `/`.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
pub(super) fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests;

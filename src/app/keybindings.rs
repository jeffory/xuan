//! Settings → Keyboard Shortcuts: every command of the registry with its bindings, to search,
//! rebind by pressing keys, clear and reset. Only the changes are stored, in `[keybindings]`.

use super::theme::PaletteExt as _;
use egui::{Button, Event, Key, Modifiers, RichText, Ui, WidgetInfo, WidgetType};
use xuan::i18n::tr;

use super::{
    commands::{self, Category, Chord, Entry, Keymap, Refusal},
    theme, widgets,
};

/// What the page remembers between frames.
#[derive(Default)]
pub(super) struct KeyEditor {
    pub search: String,
    /// The binding waiting for a key press: the command and which of its bindings it replaces
    /// (`None` adds one).
    pub capture: Option<(String, Option<usize>)>,
    /// A chord another command already uses, waiting for Reassign or Cancel.
    pub conflict: Option<Conflict>,
    /// Why the last key press was not assigned.
    pub message: Option<String>,
}

pub(super) struct Conflict {
    pub id: String,
    pub slot: Option<usize>,
    pub chord: Chord,
    /// The command that uses the chord now.
    pub other: String,
}

/// The first key press this frame, removed from the input with everything else typed, so no
/// widget, shortcut or the Settings window's Escape sees it. The clipboard chords arrive as
/// clipboard events on some platforms.
fn take_key_press(ui: &Ui) -> Option<Chord> {
    ui.input_mut(|input| {
        let modifiers = input.modifiers;
        let mut found = None;
        input.events.retain(|event| {
            let chord = match event {
                Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } => pressed.then(|| Chord::from_event(*modifiers, *key)),
                Event::Copy => Some(Chord::from_event(modifiers, Key::C)),
                Event::Cut => Some(Chord::from_event(modifiers, Key::X)),
                Event::Paste(_) => Some(Chord::from_event(modifiers, Key::V)),
                Event::Text(_) => None,
                _ => return true,
            };
            if found.is_none() {
                found = chord.map(|chord| match event {
                    Event::Copy | Event::Cut | Event::Paste(_) => Chord {
                        mods: Modifiers {
                            ctrl: true,
                            ..chord.mods
                        },
                        ..chord
                    },
                    _ => chord,
                });
            }
            false
        });
        found
    })
}

/// `id`'s bindings with `chord` in place of binding `slot`, or added.
fn with_key(keymap: &Keymap, id: &str, slot: Option<usize>, chord: Chord) -> Vec<Chord> {
    let mut keys = keymap.keys(id).to_vec();
    match slot {
        Some(index) if index < keys.len() => keys[index] = chord,
        _ => keys.push(chord),
    }
    let mut unique: Vec<Chord> = Vec::new();
    for key in keys {
        if !unique.iter().any(|k| k.same(key)) {
            unique.push(key);
        }
    }
    unique
}

/// `id`'s bindings without `chord`.
fn without_key(keymap: &Keymap, id: &str, chord: Chord) -> Vec<Chord> {
    let mut keys = keymap.keys(id).to_vec();
    keys.retain(|k| !k.same(chord));
    keys
}

impl KeyEditor {
    /// Assigns the pressed chord to the binding being captured, or explains why not.
    fn finish_capture(&mut self, keymap: &Keymap, overrides: &mut toml::Table, chord: Chord) {
        let Some((id, slot)) = self.capture.take() else {
            return;
        };
        if chord.mods == Modifiers::NONE && chord.key == Key::Escape {
            return;
        }
        if chord.mods == Modifiers::NONE && chord.key == Key::Backspace {
            if let Some(index) = slot {
                let mut keys = keymap.keys(&id).to_vec();
                if index < keys.len() {
                    keys.remove(index);
                    commands::set_override(overrides, keymap, &id, &keys);
                }
            }
            return;
        }
        match keymap.check(&id, chord) {
            Ok(()) => {
                let keys = with_key(keymap, &id, slot, chord);
                commands::set_override(overrides, keymap, &id, &keys);
            }
            Err(Refusal::Reserved) => {
                self.message = Some(
                    tr("{key} is reserved for the editor and cannot be assigned.")
                        .replace("{key}", &chord.label()),
                );
            }
            Err(Refusal::NeedsModifier) => {
                self.message = Some(
                    tr("Single letters and digits are kept for tools. Add Ctrl or Alt.").into(),
                );
            }
            Err(Refusal::Conflict(other)) => {
                self.conflict = Some(Conflict {
                    id,
                    slot,
                    chord,
                    other,
                });
            }
        }
    }

    /// Reassign: the chord moves from the other command to this one.
    fn reassign(&mut self, keymap: &Keymap, overrides: &mut toml::Table) {
        let Some(conflict) = self.conflict.take() else {
            return;
        };
        let others = without_key(keymap, &conflict.other, conflict.chord);
        commands::set_override(overrides, keymap, &conflict.other, &others);
        let keys = with_key(keymap, &conflict.id, conflict.slot, conflict.chord);
        commands::set_override(overrides, keymap, &conflict.id, &keys);
    }

    fn start_capture(&mut self, ui: &Ui, id: &str, slot: Option<usize>) {
        self.capture = Some((id.to_owned(), slot));
        self.conflict = None;
        self.message = None;
        // A focused button would also take Enter or Space.
        ui.memory_mut(|memory| {
            if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            }
        });
    }

    fn capturing(&self, id: &str, slot: Option<usize>) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|(capture, s)| capture == id && *s == slot)
    }
}

pub(super) fn matches_search(entry: &Entry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let english = match &entry.kind {
        commands::Kind::Builtin(command) => command.label,
        commands::Kind::Plugin { .. } => "",
    };
    [
        entry.label(),
        english,
        &entry.id,
        tr(entry.category().name()),
    ]
    .into_iter()
    .chain(entry.aliases().iter().copied())
    .any(|text| text.to_lowercase().contains(query))
        || entry.keys.iter().any(|key| {
            key.label().to_lowercase() == query || key.to_string().to_lowercase() == query
        })
}

/// Sets the accessible label, so tests and screen readers can tell the many small buttons apart.
fn describe(response: &egui::Response, label: String) {
    let enabled = response.enabled();
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &label));
}

/// The page. Changes go to `overrides` (the `[keybindings]` table being edited).
pub(super) fn page(
    ui: &mut Ui,
    keymap: &Keymap,
    editor: &mut KeyEditor,
    overrides: &mut toml::Table,
) {
    if editor.capture.is_some() {
        if let Some(chord) = take_key_press(ui) {
            editor.finish_capture(keymap, overrides, chord);
        }
        ui.ctx().request_repaint();
    } else if editor.conflict.is_some()
        && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
    {
        editor.conflict = None;
    }

    ui.heading(tr("Keyboard Shortcuts"));
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        search_box(ui, &mut editor.search);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let response = ui.add_enabled(!overrides.is_empty(), Button::new(tr("Reset All")));
            if response.clicked() {
                overrides.clear();
                editor.capture = None;
                editor.conflict = None;
                editor.message = None;
            }
        });
    });
    ui.add_space(6.0);
    if let Some(conflict) = &editor.conflict {
        let other = keymap
            .get(&conflict.other)
            .map_or(conflict.other.as_str(), Entry::label);
        let mut answer = None;
        egui::Frame::new()
            .fill(ui.palette().field)
            .corner_radius(theme::BUTTON_RADIUS)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        tr("{key} is already used by “{command}”.")
                            .replace("{key}", &conflict.chord.label())
                            .replace("{command}", other),
                    )
                    .wrap(),
                );
                let response = widgets::dialog_footer(
                    ui,
                    widgets::FooterButtons::commit(tr("Reassign")),
                    |_| {},
                );
                if response.commit {
                    answer = Some(true);
                } else if response.cancel {
                    answer = Some(false);
                }
            });
        match answer {
            Some(true) => editor.reassign(keymap, overrides),
            Some(false) => editor.conflict = None,
            None => {}
        }
    } else if let Some(message) = &editor.message {
        ui.add(egui::Label::new(RichText::new(message).color(ui.palette().muted)).wrap());
    } else {
        ui.add(
            egui::Label::new(
                RichText::new(tr(
                    "Click a shortcut and press keys. Escape cancels; Backspace removes it.",
                ))
                .color(ui.palette().muted),
            )
            .wrap(),
        );
    }
    ui.add_space(6.0);

    let query = editor.search.trim().to_lowercase();
    list(
        ui,
        keymap,
        &query,
        false,
        ui.available_height(),
        |ui, entry| row(ui, entry, editor, overrides),
        |_| {},
    );
}

/// The search box above the list, here and in Help → Keyboard Shortcuts.
pub(super) fn search_box(ui: &mut Ui, search: &mut String) {
    ui.add(
        egui::TextEdit::singleline(search)
            .hint_text(tr("Search commands or keys"))
            .desired_width(240.0),
    );
}

/// The width of the action column, so the keys line up.
const ACTION_WIDTH: f32 = 210.0;

/// The commands matching `query` (only those with keys when `bound_only`) by category in a scroll area `height` tall, each drawn by `row`,
/// then whatever `after` adds. Settings and the F1 reference share it, so they look alike.
fn list(
    ui: &mut Ui,
    keymap: &Keymap,
    query: &str,
    bound_only: bool,
    height: f32,
    mut row: impl FnMut(&mut Ui, &Entry),
    after: impl FnOnce(&mut Ui),
) {
    let output = egui::ScrollArea::vertical()
        .id_salt("keyboard_shortcuts")
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for category in Category::ALL {
                let rows: Vec<&Entry> = keymap
                    .entries()
                    .iter()
                    .filter(|entry| {
                        entry.category() == category
                            && matches_search(entry, query)
                            && !(bound_only && entry.keys.is_empty())
                    })
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(RichText::new(tr(category.name())).color(ui.palette().muted));
                for entry in rows {
                    row(ui, entry);
                }
            }
            after(ui);
        });
    widgets::overflow_fades(ui, &output);
}

/// One command of the read-only reference: the action, then its keys as chips.
pub(super) fn reference_row(ui: &mut Ui, label: &str, keys: &[String]) {
    ui.horizontal(|ui| {
        action_cell(ui, label);
        for key in keys {
            chip(ui, key);
        }
    });
}

fn action_cell(ui: &mut Ui, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(ACTION_WIDTH, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(ACTION_WIDTH);
            ui.add(egui::Label::new(label).truncate());
        },
    );
}

/// A key that cannot be clicked: the same bezel colours as the buttons in Settings.
fn chip(ui: &mut Ui, text: &str) {
    let p = ui.palette();
    egui::Frame::new()
        .fill(p.control)
        .stroke(egui::Stroke::new(1.0, p.control_edge))
        .corner_radius(theme::BUTTON_RADIUS)
        .inner_margin(egui::Margin::symmetric(11, 2))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).color(p.text)).selectable(false));
        });
}

fn row(ui: &mut Ui, entry: &Entry, editor: &mut KeyEditor, overrides: &mut toml::Table) {
    let label = entry.label();
    ui.horizontal(|ui| {
        action_cell(ui, label);
        for (index, chord) in entry.keys.iter().enumerate() {
            let capturing = editor.capturing(&entry.id, Some(index));
            let (text, accessible) = if capturing {
                (tr("Press keys…").to_owned(), tr("Press keys…").to_owned())
            } else {
                (chord.label(), format!("{label}: {}", chord.label()))
            };
            let response = ui.add(Button::new(text).selected(capturing));
            describe(&response, accessible);
            if response.clicked() {
                editor.start_capture(ui, &entry.id, Some(index));
            }
        }
        let adding = editor.capturing(&entry.id, None);
        let response = ui
            .add(Button::new(if adding { tr("Press keys…") } else { "+" }).selected(adding))
            .on_hover_text(tr("Add shortcut"));
        if !adding {
            describe(&response, format!("{}: {label}", tr("Add shortcut")));
        }
        if response.clicked() {
            editor.start_capture(ui, &entry.id, None);
        }
        if entry.customised() {
            let response = ui
                .add(Button::new(tr("Reset")).small())
                .on_hover_text(tr("Restore the default shortcut"));
            describe(&response, format!("{}: {label}", tr("Reset")));
            if response.clicked() {
                overrides.remove(&entry.id);
                editor.capture = None;
            }
        }
    });
}

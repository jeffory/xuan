//! Key dispatch. Commands and their bindings live in the registry (`commands.rs`); this runs the
//! command a key press is bound to, then handles the keys the editor keeps for itself.

use super::{
    EditorApp, Tool,
    commands::{Chord, ctrl_or_cmd},
};
use egui::{Event, InputState, Key, Modifiers};
use xuan::i18n::tr;

/// Removes the first press of exactly this chord from the input. Unlike
/// [`InputState::consume_key`], extra Shift or Alt makes a different chord.
pub(super) fn consume_exact(input: &mut InputState, mods: Modifiers, key: Key) -> bool {
    let chord = Chord { mods, key };
    let mut found = false;
    input.events.retain(|event| {
        let hit = !found
            && matches!(
                event,
                Event::Key { key: k, modifiers: m, pressed: true, .. } if chord.matches(*m, *k)
            );
        found |= hit;
        !hit
    });
    found
}

impl EditorApp {
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.palette.is_some() || ctx.wants_keyboard_input() {
            return;
        }
        let developing = self.develop.is_some();

        // Native backends turn the clipboard chords into these events instead of key presses,
        // including Ctrl+Shift+C. Leave them alone when a text field has focus.
        let clipboard = ctx.input_mut(|input| {
            let mut chords = Vec::new();
            let mods = Chord::from_event(input.modifiers, Key::C).mods;
            input.events.retain(|event| {
                let (key, text) = match event {
                    Event::Copy => (Key::C, None),
                    Event::Cut => (Key::X, None),
                    Event::Paste(text) => (Key::V, Some(text.clone())),
                    _ => return true,
                };
                let mods = Modifiers { ctrl: true, ..mods };
                chords.push((Chord { mods, key }, text));
                false
            });
            chords
        });
        if !clipboard.is_empty() {
            for (chord, text) in clipboard {
                let Some(id) = self
                    .keymap
                    .lookup(chord.mods, chord.key, developing)
                    .map(|entry| entry.id.clone())
                else {
                    continue;
                };
                if !self.command_enabled(&id) {
                    continue;
                }
                if id == "paste" {
                    self.paste_clipboard(text.as_deref());
                } else {
                    self.run_shortcut(&id);
                }
            }
            return;
        }

        // With the Pen, Delete and Backspace remove an anchor rather than clear pixels.
        if self.tool == Tool::Pen
            && !developing
            && (self.pen.draft.is_some() || self.pen.selected.is_some())
            && ctx.input_mut(|i| {
                consume_exact(i, Modifiers::NONE, Key::Delete)
                    || consume_exact(i, Modifiers::NONE, Key::Backspace)
            })
        {
            self.pen_delete();
        }

        // Bound keys, in the order they were pressed. A command ends the frame's key handling;
        // tool keys and other quick actions let later keys through.
        let presses: Vec<(Modifiers, Key)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*modifiers, *key)),
                    _ => None,
                })
                .collect()
        });
        for (mods, key) in presses {
            let Some((id, quick)) = self.keymap.lookup(mods, key, developing).map(|entry| {
                let quick = matches!(
                    &entry.kind,
                    super::commands::Kind::Builtin(command)
                        if !matches!(command.run, super::commands::Run::Command)
                );
                (entry.id.clone(), quick)
            }) else {
                continue;
            };
            ctx.input_mut(|i| consume_exact(i, mods, key));
            if key == Key::Tab {
                // egui moves keyboard focus on any Tab press, even with Ctrl held. A bound
                // Tab chord (Ctrl+Tab) is a command, so keep the focus where it is; it could
                // otherwise land in a text field and swallow the next shortcut.
                ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
            }
            if !self.command_enabled(&id) {
                continue;
            }
            self.run_shortcut(&id);
            if !quick {
                return;
            }
        }

        let pressed = |key| ctx.input(|i| i.key_pressed(key));
        let modifiers = ctx.input(|i| i.modifiers);
        if let Some(develop) = &mut self.develop {
            if pressed(Key::Escape) {
                develop.tool = super::develop::CanvasTool::None;
            }
            return;
        }
        if ctrl_or_cmd(modifiers) {
            return;
        }
        if pressed(Key::Escape) {
            self.cancel_gesture();
            self.crop_rect = None;
            self.polygon.clear();
            if self.tool == Tool::Pen {
                self.pen_escape();
            }
        }
        if pressed(Key::Enter) {
            if let Some((start, end)) = self.crop_rect.take() {
                self.edit(tr("Crop"), |doc| xuan::operations::crop(doc, start, end));
                if let Some(s) = self.session_mut() {
                    s.fit = true;
                }
            } else if self.polygon.len() >= 3 {
                self.finish_polygon();
            } else if self.pen.draft.is_some() {
                self.pen_finish();
            }
        }
        for (index, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
            Key::Num0,
        ]
        .into_iter()
        .enumerate()
        {
            if pressed(key) {
                let opacity = (index + 1) as f32 / 10.0;
                if self.tool.is_brush() || self.tool == Tool::Gradient {
                    self.brush.opacity = opacity;
                } else {
                    self.edit(tr("Layer Opacity"), |doc| {
                        let selected = doc.selected.clone();
                        for l in &mut doc.layers {
                            if selected.contains(&l.id) && !l.group {
                                l.opacity = opacity;
                            }
                        }
                        Ok(())
                    });
                }
            }
        }
        let step = if modifiers.shift { 10.0 } else { 1.0 };
        let mut dx = 0.0;
        let mut dy = 0.0;
        if pressed(Key::ArrowLeft) {
            dx -= step;
        }
        if pressed(Key::ArrowRight) {
            dx += step;
        }
        if pressed(Key::ArrowUp) {
            dy -= step;
        }
        if pressed(Key::ArrowDown) {
            dy += step;
        }
        if dx != 0.0 || dy != 0.0 {
            let mask_target = self.transforming_mask();
            self.edit(tr("Nudge"), |doc| {
                if let Some(mut transform) = xuan::operations::transform_box(doc, mask_target) {
                    transform.x += dx;
                    transform.y += dy;
                    xuan::operations::apply_transform(doc, transform, mask_target)?;
                }
                Ok(())
            });
        }
    }
}

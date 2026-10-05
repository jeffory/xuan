use super::{EditorApp, Tool};
use egui::{Event, InputState, Key, Modifiers};
use xuan::{i18n::tr, plugins::manifest::Shortcut};

const CTRL: Modifiers = Modifiers::CTRL;
const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);
const CTRL_ALT: Modifiers = Modifiers::CTRL.plus(Modifiers::ALT);

/// Built-in command chords. Each is matched with its exact modifiers, so the
/// order does not matter and Ctrl+Shift+I never runs Ctrl+I's command.
const COMMANDS: &[(Modifiers, Key, &str)] = &[
    (CTRL_ALT.plus(Modifiers::SHIFT), Key::S, "export"),
    (CTRL_SHIFT, Key::N, "new_layer"),
    (CTRL_SHIFT, Key::O, "import"),
    (CTRL_SHIFT, Key::S, "save_as"),
    (CTRL_SHIFT, Key::Z, "redo"),
    (CTRL_SHIFT, Key::C, "copy_merged"),
    (CTRL_SHIFT, Key::G, "ungroup"),
    (CTRL_SHIFT, Key::I, "invert_selection"),
    (CTRL_ALT, Key::G, "clip"),
    // Shift+; types a colon on many layouts, so either key toggles snapping.
    (CTRL_SHIFT, Key::Semicolon, "toggle_snap"),
    (CTRL_SHIFT, Key::Colon, "toggle_snap"),
    (CTRL_ALT, Key::Semicolon, "lock_guides"),
    (CTRL, Key::Semicolon, "toggle_guides"),
    (CTRL, Key::Quote, "toggle_grid"),
    (CTRL, Key::R, "toggle_rulers"),
    (CTRL, Key::Comma, "settings"),
    (CTRL, Key::N, "new"),
    (CTRL, Key::O, "open"),
    (CTRL, Key::S, "save"),
    (CTRL, Key::W, "close"),
    (CTRL, Key::Q, "quit"),
    (CTRL, Key::Z, "undo"),
    (CTRL, Key::Y, "redo"),
    (CTRL, Key::J, "duplicate"),
    (CTRL, Key::G, "group"),
    (CTRL, Key::E, "merge"),
    (CTRL, Key::A, "select_all"),
    (CTRL, Key::D, "deselect"),
    (CTRL, Key::I, "invert"),
    (CTRL, Key::L, "levels"),
    (CTRL, Key::U, "hue"),
    (CTRL, Key::M, "curves"),
    (CTRL, Key::C, "copy"),
    (CTRL, Key::X, "cut"),
    (CTRL, Key::V, "paste"),
    (CTRL, Key::Num0, "fit"),
    (CTRL, Key::Num1, "actual"),
    (CTRL, Key::Plus, "zoom_in"),
    (CTRL, Key::Equals, "zoom_in"),
    (CTRL, Key::Minus, "zoom_out"),
    (Modifiers::ALT, Key::Backspace, "fill_fg"),
    (CTRL, Key::Backspace, "fill_bg"),
];

/// Built-in chords handled outside [`COMMANDS`], with what they do.
const OTHER_CHORDS: &[(Modifiers, Key, &str)] = &[
    (CTRL, Key::H, "hide_controls"),
    (CTRL, Key::T, "show_transform"),
    (CTRL, Key::Enter, "apply_text"),
    (Modifiers::SHIFT, Key::F5, "content_fill"),
    (Modifiers::NONE, Key::F1, "shortcuts"),
];

/// Whether pressed modifiers match a chord exactly. Shift is ignored for keys
/// that need it on some common layouts, such as `+` and `'`.
fn chord_matches(pressed: Modifiers, mods: Modifiers, key: Key) -> bool {
    if matches!(key, Key::Plus | Key::Quote) {
        pressed.matches_logically(mods)
    } else {
        pressed.matches_exact(mods)
    }
}

/// Like [`InputState::consume_key`], but extra Shift or Alt makes a different chord.
pub(super) fn consume_exact(input: &mut InputState, mods: Modifiers, key: Key) -> bool {
    let mut found = false;
    input.events.retain(|event| {
        let hit = !found
            && matches!(
                event,
                Event::Key { key: k, modifiers: m, pressed: true, .. }
                    if *k == key && chord_matches(*m, mods, key)
            );
        found |= hit;
        !hit
    });
    found
}

/// The egui chord for a plugin shortcut, or `None` for a key egui does not know.
pub(super) fn plugin_chord(shortcut: &Shortcut) -> Option<(Modifiers, Key)> {
    let key = Key::from_name(&shortcut.key)?;
    let mut mods = Modifiers::NONE;
    if shortcut.ctrl {
        mods = mods.plus(CTRL);
    }
    if shortcut.shift {
        mods = mods.plus(Modifiers::SHIFT);
    }
    if shortcut.alt {
        mods = mods.plus(Modifiers::ALT);
    }
    Some((mods, key))
}

/// The built-in command that already owns this chord, if any.
pub(super) fn builtin_for(mods: Modifiers, key: Key) -> Option<&'static str> {
    COMMANDS
        .iter()
        .chain(OTHER_CHORDS)
        .find(|&&(m, k, _)| k == key && chord_matches(mods, m, key))
        .map(|(_, _, command)| *command)
}

impl EditorApp {
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }

        // Native backends translate clipboard shortcuts into these events,
        // including Ctrl+Shift+C. Leave them alone when a text field has focus.
        let clipboard_commands = ctx.input_mut(|input| {
            let mut commands = Vec::new();
            if self.develop.is_some() {
                return commands;
            }
            input.events.retain(|event| {
                let command = match event {
                    Event::Copy if input.modifiers.shift => "copy_merged",
                    Event::Copy => "copy",
                    Event::Cut => "cut",
                    Event::Paste(text) => {
                        commands.push(("paste", Some(text.clone())));
                        return false;
                    }
                    _ => return true,
                };
                commands.push((command, None));
                false
            });
            commands
        });
        if !clipboard_commands.is_empty() {
            for (command, text) in clipboard_commands {
                if command == "paste" {
                    self.paste_clipboard(text.as_deref());
                } else {
                    self.command(command);
                }
            }
            return;
        }

        let pressed = |key| ctx.input(|i| i.key_pressed(key));
        let modifiers = ctx.input(|i| i.modifiers);
        for &(mods, key, command) in COMMANDS {
            if ctx.input_mut(|i| consume_exact(i, mods, key)) {
                self.command(command);
                return;
            }
        }
        if self.develop.is_none()
            && let Some((plugin, action)) = self.plugin_shortcut(ctx)
        {
            self.start_plugin_action(&plugin, &action);
            return;
        }
        if let Some(develop) = &mut self.develop {
            if pressed(Key::Escape) {
                develop.tool = super::develop::CanvasTool::None;
            }
            if pressed(Key::F1) {
                self.command("shortcuts");
            }
            return;
        }
        if modifiers.ctrl {
            if pressed(Key::H) {
                self.show_controls = !self.show_controls;
            }
            if pressed(Key::T) {
                self.set_tool(Tool::Move);
                self.show_controls = true;
            }
            return;
        }
        if modifiers.shift && pressed(Key::F5) {
            self.command("content_fill");
        }
        if pressed(Key::F1) {
            self.command("shortcuts");
        }
        if pressed(Key::Escape) {
            self.cancel_gesture();
            self.crop_rect = None;
            self.polygon.clear();
        }
        if pressed(Key::Enter) {
            if let Some((start, end)) = self.crop_rect.take() {
                self.edit(tr("Crop"), |doc| xuan::operations::crop(doc, start, end));
                if let Some(s) = self.session_mut() {
                    s.fit = true;
                }
            } else if self.polygon.len() >= 3 {
                self.finish_polygon();
            }
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            if self
                .session()
                .is_some_and(|s| s.document.selection.is_some())
            {
                self.command("clear");
            } else {
                self.command("delete_layer");
            }
        }
        for (key, tool) in [
            (Key::V, Tool::Move),
            (Key::M, Tool::Marquee),
            (Key::L, Tool::Lasso),
            (Key::W, Tool::Wand),
            (Key::C, Tool::Crop),
            (Key::B, Tool::Brush),
            (Key::E, Tool::Erase),
            (Key::J, Tool::Heal),
            (Key::S, Tool::Clone),
            (Key::R, Tool::Blur),
            (Key::G, Tool::Gradient),
            (Key::U, Tool::Shape),
            (Key::T, Tool::Text),
            (Key::I, Tool::Dropper),
            (Key::H, Tool::Hand),
            (Key::Z, Tool::Zoom),
        ] {
            if pressed(key) {
                // B selects the last-used of Brush and Pencil; Shift+B switches between them.
                let tool = if key != Key::B {
                    tool
                } else if !modifiers.shift {
                    self.brush_variant
                } else if self.tool == Tool::Brush {
                    Tool::Pencil
                } else {
                    Tool::Brush
                };
                if modifiers.shift && tool == Tool::Marquee {
                    self.ellipse = !self.ellipse;
                }
                if modifiers.shift && tool == Tool::Lasso {
                    self.polygonal = !self.polygonal;
                }
                if modifiers.shift && tool == Tool::Shape {
                    self.shape_kind = if self.shape_kind == xuan::paint::ShapeKind::Ellipse {
                        xuan::paint::ShapeKind::Rectangle
                    } else {
                        xuan::paint::ShapeKind::Ellipse
                    };
                }
                self.set_tool(tool);
            }
        }
        if pressed(Key::X) {
            std::mem::swap(&mut self.brush.color, &mut self.background);
        }
        if pressed(Key::D) {
            self.brush.color = [0, 0, 0, 255];
            self.background = [255; 4];
        }
        if pressed(Key::OpenBracket) {
            if modifiers.shift {
                self.brush.hardness = (self.brush.hardness - 0.1).max(0.0);
            } else {
                self.brush.diameter = (self.brush.diameter / 1.15).round().max(1.0);
            }
        }
        if pressed(Key::CloseBracket) {
            if modifiers.shift {
                self.brush.hardness = (self.brush.hardness + 0.1).min(1.0);
            } else {
                self.brush.diameter = (self.brush.diameter * 1.15).round().min(2000.0);
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

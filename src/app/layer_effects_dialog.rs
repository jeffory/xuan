//! Layer → Layer Effects…: the stroke, shadows, overlay and glows of one pixel layer, after
//! upstream Compositor's effects panel (Document/LayerEffects.swift). Edits show live on the
//! canvas, are kept on the layer (never baked into its pixels) and can be reopened at any time.
//! OK keeps them as one undo step; Cancel restores what the layer had.

use super::theme::PaletteExt as _;
use egui::RichText;
use uuid::Uuid;
use xuan::{
    i18n::tr,
    layer_effects::{EffectKind, GlowEffect, LayerEffects, ShadowEffect},
};

use super::{Dialog, EditorApp, widgets};

pub(super) struct LayerEffectsEdit {
    pub(super) layer: Uuid,
    /// What the layer had when the dialog opened.
    pub(super) original: Option<LayerEffects>,
    pub(super) effects: LayerEffects,
    pub(super) selected: EffectKind,
}

/// Whether `layer` can take effects: a pixel or text layer, not a folder, mask, adjustment or
/// filter. Unlike upstream, a new layer that is still empty can take them too; they show once it
/// has pixels.
pub(super) fn can_take_effects(layer: &xuan::document::Layer) -> bool {
    !layer.group && !layer.is_effect()
}

fn color_row(ui: &mut egui::Ui, color: &mut [u8; 3], opacity: &mut f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tr("Colour"));
        let mut rgba = [color[0], color[1], color[2], 255];
        if widgets::color_well(ui, &mut rgba).changed() {
            *color = [rgba[0], rgba[1], rgba[2]];
            changed = true;
        }
    });
    changed |= ui
        .add(
            widgets::Slider::new(opacity, 0.0..=1.0)
                .text(tr("Opacity"))
                .percentage(),
        )
        .changed();
    changed
}

fn shadow_controls(ui: &mut egui::Ui, shadow: &mut ShadowEffect) -> bool {
    let mut changed = false;
    changed |= ui
        .add(
            widgets::Slider::new(&mut shadow.angle, -180.0..=180.0)
                .text(tr("Angle"))
                .suffix("°")
                .max_decimals(0),
        )
        .changed();
    changed |= ui
        .add(
            widgets::Slider::new(&mut shadow.distance, 0.0..=1000.0)
                .text(tr("Distance"))
                .suffix(" px")
                .max_decimals(0),
        )
        .changed();
    changed |= ui
        .add(
            widgets::Slider::new(&mut shadow.blur, 0.0..=250.0)
                .text(tr("Blur"))
                .suffix(" px")
                .max_decimals(0),
        )
        .changed();
    changed | color_row(ui, &mut shadow.color, &mut shadow.opacity)
}

fn glow_controls(ui: &mut egui::Ui, glow: &mut GlowEffect) -> bool {
    let changed = ui
        .add(
            widgets::Slider::new(&mut glow.size, 0.0..=250.0)
                .text(tr("Size"))
                .suffix(" px")
                .max_decimals(0),
        )
        .changed();
    changed | color_row(ui, &mut glow.color, &mut glow.opacity)
}

/// The selected effect's settings; false when it has none (it is not on the layer).
fn controls(ui: &mut egui::Ui, effects: &mut LayerEffects, kind: EffectKind) -> bool {
    match kind {
        EffectKind::Stroke => effects.stroke.as_mut().is_some_and(|stroke| {
            let mut changed = ui
                .add(
                    widgets::Slider::new(&mut stroke.size, 1.0..=500.0)
                        .logarithmic(true)
                        .text(tr("Size"))
                        .suffix(" px")
                        .max_decimals(0),
                )
                .changed();
            ui.horizontal(|ui| {
                ui.label(tr("Position"));
                changed |= widgets::segmented(
                    ui,
                    &mut stroke.inside,
                    &[(false, tr("Outside")), (true, tr("Inside"))],
                )
                .changed();
            });
            changed | color_row(ui, &mut stroke.color, &mut stroke.opacity)
        }),
        EffectKind::DropShadow => effects
            .drop_shadow
            .as_mut()
            .is_some_and(|shadow| shadow_controls(ui, shadow)),
        EffectKind::InnerShadow => effects
            .inner_shadow
            .as_mut()
            .is_some_and(|shadow| shadow_controls(ui, shadow)),
        EffectKind::ColorOverlay => effects
            .color_overlay
            .as_mut()
            .is_some_and(|overlay| color_row(ui, &mut overlay.color, &mut overlay.opacity)),
        EffectKind::OuterGlow => effects
            .outer_glow
            .as_mut()
            .is_some_and(|glow| glow_controls(ui, glow)),
        EffectKind::InnerGlow => effects
            .inner_glow
            .as_mut()
            .is_some_and(|glow| glow_controls(ui, glow)),
    }
}

impl EditorApp {
    /// Opens Layer Effects for the active layer, starting on `kind` when given.
    pub(super) fn start_layer_effects(&mut self, kind: Option<EffectKind>) {
        let Some(session) = self.session_mut() else {
            return;
        };
        let Some(layer) = session.document.active().filter(|l| can_take_effects(l)) else {
            self.error = Some(tr("Select a pixel or text layer to add effects").into());
            return;
        };
        let original = layer.effects.clone();
        let effects = original.clone().unwrap_or_default();
        let selected = kind
            .or_else(|| {
                EffectKind::ALL
                    .into_iter()
                    .find(|kind| effects.contains(*kind))
            })
            .unwrap_or(EffectKind::Stroke);
        let id = layer.id;
        session
            .history
            .begin(tr("Layer Effects"), &session.document);
        self.layer_effects = Some(LayerEffectsEdit {
            layer: id,
            original,
            effects,
            selected,
        });
        self.dialog = Some(Dialog::LayerEffects);
    }

    pub(super) fn layer_effects_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.layer_effects.take() else {
            self.dialog = None;
            return;
        };
        let background = [self.background[0], self.background[1], self.background[2]];
        let mut open = true;
        let mut ok = false;
        let mut cancel = false;
        let mut changed = false;
        widgets::Window::new(tr("Layer Effects"))
            .id("layer_effects")
            .open(&mut open)
            .default_width(460.0)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(150.0);
                            for kind in EffectKind::ALL {
                                ui.horizontal(|ui| {
                                    let mut shown = edit.effects.is_enabled(kind);
                                    let label = format!("{} {}", tr("Show"), tr(kind.name()));
                                    if widgets::bare_checkbox(ui, &mut shown, &label)
                                        .on_hover_text(&label)
                                        .changed()
                                    {
                                        if shown && !edit.effects.contains(kind) {
                                            edit.effects.add(kind, background);
                                        }
                                        edit.effects.set_enabled(kind, shown);
                                        edit.selected = kind;
                                        changed = true;
                                    }
                                    if widgets::selected_row(
                                        ui,
                                        edit.selected == kind,
                                        tr(kind.name()),
                                    )
                                    .clicked()
                                    {
                                        edit.selected = kind;
                                    }
                                });
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            // Room for the tallest effect, so the window keeps its size (and its
                            // buttons their place) as effects are switched or added.
                            ui.set_min_height(250.0);
                            widgets::subheading(ui, tr(edit.selected.name()));
                            ui.add_space(4.0);
                            if edit.effects.contains(edit.selected) {
                                changed |= controls(ui, &mut edit.effects, edit.selected);
                                ui.add_space(6.0);
                                if widgets::button(ui, tr("Remove Effect")).clicked() {
                                    edit.effects.remove(edit.selected);
                                    changed = true;
                                }
                            } else {
                                ui.label(
                                    RichText::new(tr("Not on this layer"))
                                        .small()
                                        .color(ui.palette().muted),
                                );
                                if widgets::button(ui, tr("Add Effect")).clicked() {
                                    edit.effects.add(edit.selected, background);
                                    changed = true;
                                }
                            }
                        });
                    });
                    ui.label(
                        RichText::new(tr("Effects stay editable; the pixels are not changed"))
                            .small()
                            .color(ui.palette().muted),
                    );
                },
                |ui, ()| {
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(tr("Apply")),
                        |_| {},
                    );
                    ok = response.commit;
                    cancel = response.cancel;
                },
            );
        if changed && let Some(session) = self.session_mut() {
            if let Some(layer) = session
                .document
                .layers
                .iter_mut()
                .find(|l| l.id == edit.layer)
            {
                layer.effects = (!edit.effects.is_empty()).then(|| edit.effects.clone());
            }
            session.invalidate();
        }
        if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(session) = self.session_mut() {
                session.history.cancel(&mut session.document);
                session.invalidate();
            }
            self.dialog = None;
            return;
        }
        if ok {
            if let Some(session) = self.session_mut() {
                let unchanged = session
                    .document
                    .layers
                    .iter()
                    .find(|l| l.id == edit.layer)
                    .is_none_or(|l| l.effects == edit.original);
                if unchanged {
                    session.history.cancel(&mut session.document);
                } else {
                    session.history.commit();
                }
            }
            self.dialog = None;
            return;
        }
        self.layer_effects = Some(edit);
    }
}

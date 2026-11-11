//! Channels pane: the active layer's colour composite and its red, green, blue and alpha
//! channels, as in Photoshop's Channels panel.
//!
//! Clicking a channel targets it, so edits change only that channel (see
//! [`xuan::channels::protect`]), and shows it alone; Shift-click adds or removes a channel;
//! Ctrl-click loads it as the selection. The eyes choose what the canvas shows without
//! changing the targets. Both are the session's own state and are not saved.
//!
//! It is a sidebar pane ([`xuan::panes::CHANNELS`]), collapsed above Layers by default.
use std::sync::{Arc, Weak};

use egui::{Color32, RichText, Sense, Stroke, StrokeKind, vec2};
use image::RgbaImage;
use uuid::Uuid;
use xuan::{
    channels::{self, Channel, Channels},
    i18n::tr,
};

use super::{EditorApp, commands::ctrl_or_cmd, icons, theme::PaletteExt as _, widgets};

/// Longest side of a channel thumbnail, in points (drawn at twice the pixels).
const THUMBNAIL_SIDE: f32 = 30.0;

/// The pane's rows: the composite (`None`) then each channel.
const ROWS: [Option<Channel>; 5] = [
    None,
    Some(Channel::Red),
    Some(Channel::Green),
    Some(Channel::Blue),
    Some(Channel::Alpha),
];

/// The command that targets a row, for its shortcut hint.
pub(super) fn command(row: Option<Channel>) -> &'static str {
    match row {
        None => "channel_composite",
        Some(Channel::Red) => "channel_red",
        Some(Channel::Green) => "channel_green",
        Some(Channel::Blue) => "channel_blue",
        Some(Channel::Alpha) => "channel_alpha",
    }
}

/// The row a channel command targets.
pub(super) fn row(command: &str) -> Option<Option<Channel>> {
    ROWS.into_iter().find(|row| self::command(*row) == command)
}

fn name(row: Option<Channel>) -> &'static str {
    row.map_or(tr("Composite"), |channel| tr(channel.name()))
}

/// Thumbnails of the active layer's channels, rebuilt when its pixels change.
#[derive(Default)]
pub(super) struct ThumbnailCache {
    /// The layer and the pixels the textures show (none for a layer not painted yet).
    shown: Option<(Uuid, Option<Weak<RgbaImage>>)>,
    textures: Vec<egui::TextureHandle>,
    /// Thumbnail sets made so far; lets tests check the cache.
    pub renders: usize,
}

impl ThumbnailCache {
    fn refresh(&mut self, ctx: &egui::Context, layer: Uuid, pixels: Option<&Arc<RgbaImage>>) {
        if self.shown.as_ref().is_some_and(|(id, shown)| {
            *id == layer && shown.as_ref().map(Weak::as_ptr) == pixels.map(Arc::as_ptr)
        }) && self.textures.len() == ROWS.len()
        {
            return;
        }
        let blank = RgbaImage::new(1, 1);
        let image = pixels.map_or(&blank, |pixels| &**pixels);
        let side = (THUMBNAIL_SIDE * 2.0) as u32;
        self.textures = ROWS
            .into_iter()
            .map(|row| {
                let view = row.map_or(Channels::COLOR, Channels::only);
                let image = channels::thumbnail(image, view, side);
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                );
                ctx.load_texture(
                    format!("channel-{layer}-{}", command(row)),
                    color,
                    egui::TextureOptions::LINEAR,
                )
            })
            .collect();
        self.shown = Some((layer, pixels.map(Arc::downgrade)));
        self.renders += 1;
    }
}

/// What a click on a row asks for.
enum Action {
    Target(Option<Channel>, bool),
    View(Option<Channel>),
    Load(Channel),
}

impl EditorApp {
    /// The body of the Channels pane. Like the other panes it takes no input while a dialog
    /// is open.
    pub(super) fn channels_pane(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let ctx = ui.ctx().clone();
        let Some(session) = self.sessions.get_mut(self.current) else {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.label(RichText::new(tr("No document open")).color(ui.palette().muted));
                });
            return;
        };
        let layer = session
            .document
            .active()
            .filter(|layer| channels::editable(layer))
            .map(|layer| (layer.id, layer.pixels.clone()));
        let Some((layer, pixels)) = layer else {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(tr("Select a pixel layer to see its channels"))
                            .color(ui.palette().muted),
                    );
                });
            return;
        };
        session
            .channel_thumbnails
            .refresh(&ctx, layer, pixels.as_ref());
        let textures = session.channel_thumbnails.textures.clone();
        let (targets, view) = (session.channel_targets, session.channel_view);
        let aspect = pixels
            .as_ref()
            .map_or(1.0, |p| p.width() as f32 / p.height().max(1) as f32);
        let mut action = None;
        ui.add_enabled_ui(enabled, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("channels_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (row, texture) in ROWS.into_iter().zip(&textures) {
                        let selected = match row {
                            None => targets == Channels::ALL,
                            Some(channel) => targets != Channels::ALL && targets.contains(channel),
                        };
                        let visible = match row {
                            None => Channels::COLOR.iter().all(|c| view.contains(c)),
                            Some(channel) => view.contains(channel),
                        };
                        if let Some(clicked) =
                            self.channel_row(ui, row, texture, aspect, selected, visible)
                        {
                            action = Some(clicked);
                        }
                    }
                });
        });
        match action {
            Some(Action::Target(row, extend)) => self.target_channel(row, extend),
            Some(Action::View(row)) => self.toggle_channel_view(row),
            Some(Action::Load(channel)) => self.load_channel_selection(channel),
            None => {}
        }
    }

    fn channel_row(
        &self,
        ui: &mut egui::Ui,
        row: Option<Channel>,
        texture: &egui::TextureHandle,
        aspect: f32,
        selected: bool,
        visible: bool,
    ) -> Option<Action> {
        let width = ui.available_width();
        let label = name(row);
        let mut action = None;
        let response = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .id_salt(("channel", command(row)))
                    .sense(Sense::click()),
                |ui| {
                    ui.style_mut().interaction.selectable_labels = false;
                    egui::Frame::new()
                        .fill(if selected {
                            ui.palette().row_selected
                        } else {
                            ui.palette().panel
                        })
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.set_min_width((width - 16.0).max(0.0));
                            ui.allocate_ui_with_layout(
                                vec2(ui.available_width(), THUMBNAIL_SIDE),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let eye = format!("{} {label}", tr("Show"));
                                    if icons::eye_labelled(ui, visible, &eye).clicked() {
                                        action = Some(Action::View(row));
                                    }
                                    let shortcut = self.keymap.shortcut(command(row));
                                    if !shortcut.is_empty() {
                                        ui.label(
                                            RichText::new(shortcut)
                                                .size(10.0)
                                                .color(ui.palette().muted),
                                        );
                                    }
                                    ui.with_layout(
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            channel_thumbnail(ui, texture, aspect);
                                            ui.label(RichText::new(label).color(if visible {
                                                ui.palette().text
                                            } else {
                                                ui.palette().muted
                                            }));
                                        },
                                    );
                                },
                            );
                        });
                },
            )
            .response;
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
        });
        widgets::focus_ring(ui, &response, 0.0);
        if action.is_none() && response.clicked() {
            let modifiers = ui.input(|i| i.modifiers);
            action = Some(match row {
                Some(channel) if ctrl_or_cmd(modifiers) => Action::Load(channel),
                _ => Action::Target(row, modifiers.shift),
            });
        }
        let hint = match row {
            None => tr("Click to edit every channel"),
            Some(_) => tr(
                "Click to edit only this channel · Shift-click adds it · Ctrl-click loads it as a selection",
            ),
        };
        response.on_hover_text(hint);
        action
    }

    /// Target a channel (`None`: the composite, every channel), as a click on its row does:
    /// alone, also showing only it; with `extend`, added to or removed from the targets.
    pub(super) fn target_channel(&mut self, row: Option<Channel>, extend: bool) {
        let Some(session) = self.session_mut() else {
            return;
        };
        let (targets, view) = match row {
            None => (Channels::ALL, Channels::COLOR),
            Some(channel) if extend && session.channel_targets != Channels::ALL => {
                let targets = session
                    .channel_targets
                    .with(channel, !session.channel_targets.contains(channel));
                if targets.is_empty() {
                    (Channels::ALL, Channels::COLOR)
                } else {
                    (targets, targets)
                }
            }
            Some(channel) => (Channels::only(channel), Channels::only(channel)),
        };
        session.channel_targets = targets;
        session.channel_view = view;
        session.invalidate();
        self.status = if targets == Channels::ALL {
            tr("Editing every channel").into()
        } else {
            format!("{} {}", tr("Editing channels:"), targets.letters())
        };
    }

    /// Show or hide a channel on the canvas (`None`: all three colour channels).
    pub(super) fn toggle_channel_view(&mut self, row: Option<Channel>) {
        let Some(session) = self.session_mut() else {
            return;
        };
        let view = session.channel_view;
        let view = match row {
            None => {
                let shown = Channels::COLOR.iter().all(|c| view.contains(c));
                if shown {
                    // Hiding the colours leaves alpha, if it is shown.
                    let alpha = Channels::only(Channel::Alpha);
                    if view.contains(Channel::Alpha) {
                        alpha
                    } else {
                        view
                    }
                } else {
                    Channels::COLOR.with(Channel::Alpha, view.contains(Channel::Alpha))
                }
            }
            Some(channel) => view.with(channel, !view.contains(channel)),
        };
        // One channel always stays visible.
        if !view.is_empty() {
            session.channel_view = view;
            session.invalidate();
        }
    }

    /// Select → Load Channel as Selection, or a Ctrl-click on a channel.
    pub(super) fn load_channel_selection(&mut self, channel: Channel) {
        let Some(selection) = self
            .session()
            .and_then(|session| channels::selection(&session.document, channel))
        else {
            self.status = tr("Select a pixel layer to load its channel").into();
            return;
        };
        self.edit_selection(tr("Load Channel Selection"), |document| {
            document.selection = Some(Arc::new(selection));
        });
    }
}

fn channel_thumbnail(ui: &mut egui::Ui, texture: &egui::TextureHandle, aspect: f32) {
    let (slot, _) = ui.allocate_exact_size(vec2(THUMBNAIL_SIDE, THUMBNAIL_SIDE), Sense::hover());
    let size = if aspect >= 1.0 {
        vec2(THUMBNAIL_SIDE, THUMBNAIL_SIDE / aspect.max(0.01))
    } else {
        vec2(THUMBNAIL_SIDE * aspect, THUMBNAIL_SIDE)
    }
    .max(vec2(1.0, 1.0));
    let rect = egui::Rect::from_center_size(slot.center(), size);
    widgets::checkerboard(ui, rect, 4.0);
    ui.painter().image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    ui.painter().rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0_f32, ui.palette().thumbnail_edge),
        StrokeKind::Inside,
    );
}

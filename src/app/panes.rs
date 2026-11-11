//! The right sidebar: a stack of collapsible panes that the user can reorder
//! by dragging their headers and resize with the splitters between them. The
//! arrangement lives in the user configuration ([`xuan::panes`]).
use super::theme::PaletteExt as _;
use egui::{Align2, CursorIcon, FontId, Rect, Sense, Stroke, pos2, vec2};
use xuan::{
    i18n::tr,
    panes::{self, MIN_HEIGHT},
};

use super::EditorApp;

const HEADER_HEIGHT: f32 = 26.0;
const SPLITTER_HEIGHT: f32 = 5.0;
/// Least height the fill pane keeps, so the Layers list always has rows to show
/// below its controls and above its footer.
const FILL_MIN_HEIGHT: f32 = 240.0;
/// Panes smaller than this keep their splitter and header usable.
const MIN_DRAWN_HEIGHT: f32 = 24.0;

/// A pane header being dragged to a new position.
pub(super) struct PaneDrag {
    id: String,
    /// Index in the layout the pane would be inserted before.
    insert: Option<usize>,
}

struct Entry {
    id: String,
    title: String,
    detail: Option<String>,
    collapsed: bool,
    height: f32,
    /// The user dragged the splitter, so `height` is kept even if the content is shorter.
    sized: bool,
    fill: bool,
}

/// Where the height a pane's content drew last frame is remembered.
fn drawn_id(id: &str) -> egui::Id {
    egui::Id::new(("pane_drawn_height", id))
}

impl Entry {
    /// Whether the body height follows what the content draws. Built-in panes
    /// size their content to the room they get, so measuring them would
    /// shrink them every frame.
    fn measured(&self) -> bool {
        !self.fill && self.id.starts_with("plugin:")
    }

    /// The body height to reserve: the pane's height, or for a plugin pane
    /// less if its content drew shorter last frame and the user has not sized
    /// it.
    fn reserved(&self, ctx: &egui::Context) -> f32 {
        if self.sized || !self.measured() {
            return self.height;
        }
        let drawn = ctx.data(|data| data.get_temp::<f32>(drawn_id(&self.id)));
        drawn.map_or(self.height, |drawn| {
            drawn.ceil().clamp(MIN_DRAWN_HEIGHT, self.height)
        })
    }
}

impl EditorApp {
    /// Translated title of a pane the editor can show, or `None` for a pane
    /// from a plugin that is not installed.
    pub(super) fn pane_title(&self, id: &str) -> Option<String> {
        match id {
            panes::LAYERS => Some(tr("Layers").into()),
            panes::NAVIGATOR => Some(tr("Navigator").into()),
            panes::CHANNELS => Some(tr("Channels").into()),
            _ => self.plugin_pane_title(id),
        }
    }

    pub(super) fn pane_detail(&self, id: &str) -> Option<String> {
        match id {
            panes::LAYERS => self
                .session()
                .map(|session| session.document.layers.len().to_string()),
            panes::NAVIGATOR => self
                .session()
                .map(|session| super::navigator::format_zoom_percent(session.zoom)),
            // The targeted channels, when not all of them.
            panes::CHANNELS => self
                .session()
                .map(|session| session.channel_targets)
                .filter(|targets| *targets != xuan::channels::Channels::ALL)
                .map(|targets| targets.letters()),
            _ => None,
        }
    }

    /// Whether the pane's body is drawn: shown and expanded.
    pub(super) fn pane_open(&self, id: &str) -> bool {
        self.config
            .panes
            .get(id)
            .is_some_and(|pane| !pane.hidden && !pane.collapsed)
    }

    /// Known panes in layout order with their visibility, for the Window menu.
    pub(super) fn pane_entries(&self) -> Vec<(String, String, bool)> {
        self.config
            .panes
            .0
            .iter()
            .filter_map(|pane| {
                self.pane_title(&pane.id)
                    .map(|title| (pane.id.clone(), title, !pane.hidden))
            })
            .collect()
    }

    pub(super) fn sidebar(&mut self, ctx: &egui::Context) {
        let enabled = self.dialog.is_none() && self.job.is_none() && self.color_range.is_none();
        let known = |id: &str| self.pane_title(id).is_some();
        let fill = self.config.panes.fill_pane(&known).map(str::to_owned);
        let entries: Vec<Entry> = self
            .config
            .panes
            .visible(&known)
            .map(|pane| Entry {
                id: pane.id.clone(),
                title: self.pane_title(&pane.id).unwrap_or_default(),
                detail: self.pane_detail(&pane.id),
                collapsed: pane.collapsed,
                height: pane.body_height(),
                sized: pane.height > 0.0,
                fill: fill.as_deref() == Some(pane.id.as_str()),
            })
            .collect();
        let mut toggle = None;
        let mut resized = None;
        let mut resize_done = false;
        let mut dropped = None;
        egui::SidePanel::right("layers_panel")
            .default_width(252.0)
            .width_range(206.0..=352.0)
            .resizable(true)
            .frame(egui::Frame::new().fill(ctx.palette().panel))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let panel = ui.max_rect();
                let fixed: f32 = entries
                    .iter()
                    .map(|entry| {
                        HEADER_HEIGHT
                            + if entry.collapsed || entry.fill {
                                0.0
                            } else {
                                entry.height + SPLITTER_HEIGHT
                            }
                    })
                    .sum();
                let fill_height =
                    (ui.available_height() - fixed).max(MIN_HEIGHT.max(FILL_MIN_HEIGHT));
                let mut rects = Vec::with_capacity(entries.len());
                for entry in &entries {
                    let top = ui.cursor().top();
                    let header = self.pane_header(ui, entry, enabled);
                    if header.clicked() {
                        toggle = Some(entry.id.clone());
                    }
                    if header.drag_started() {
                        self.pane_drag = Some(PaneDrag {
                            id: entry.id.clone(),
                            insert: None,
                        });
                    }
                    if !entry.collapsed {
                        let height = if entry.fill {
                            fill_height
                        } else {
                            entry.reserved(ctx)
                        };
                        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), height));
                        let body = ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                            ui.spacing_mut().item_spacing.y = 0.0;
                            self.pane_body(ui, &entry.id, enabled);
                        });
                        // A scope leaves the cursor under what it drew; keep the full height.
                        ui.advance_cursor_after_rect(rect);
                        if entry.measured() {
                            // Remember how much the content used, so a short pane gives
                            // the rest of its height to the fill pane next frame.
                            let drawn = body.response.rect.height();
                            let key = drawn_id(&entry.id);
                            let before = ctx.data(|data| data.get_temp::<f32>(key));
                            ctx.data_mut(|data| data.insert_temp(key, drawn));
                            if before.is_none_or(|before| (before - drawn).abs() > 0.5) {
                                ctx.request_repaint();
                            }
                        }
                        if !entry.fill {
                            let splitter = ui.allocate_response(
                                vec2(ui.available_width(), SPLITTER_HEIGHT),
                                Sense::drag(),
                            );
                            let active = splitter.hovered() || splitter.dragged();
                            if active {
                                ctx.set_cursor_icon(CursorIcon::ResizeVertical);
                            }
                            let y = splitter.rect.center().y;
                            ui.painter().hline(
                                splitter.rect.x_range(),
                                y,
                                Stroke::new(
                                    1.0_f32,
                                    if active {
                                        ui.palette().accent
                                    } else {
                                        ui.palette().divider
                                    },
                                ),
                            );
                            if splitter.dragged() {
                                resized =
                                    Some((entry.id.clone(), height + splitter.drag_delta().y));
                            }
                            resize_done |= splitter.drag_stopped();
                        }
                    }
                    rects.push(Rect::from_min_max(
                        pos2(panel.left(), top),
                        pos2(panel.right(), ui.cursor().top()),
                    ));
                }
                if let Some(drag) = &mut self.pane_drag {
                    ctx.set_cursor_icon(CursorIcon::Grabbing);
                    if let Some(pointer) = ctx.pointer_latest_pos() {
                        let slot = rects
                            .iter()
                            .position(|rect| pointer.y < rect.center().y)
                            .unwrap_or(rects.len());
                        let y = match rects.get(slot) {
                            Some(rect) => rect.top(),
                            None => rects.last().map_or(panel.top(), |rect| rect.bottom()),
                        };
                        ui.painter().hline(
                            panel.x_range(),
                            y.clamp(panel.top() + 1.0, panel.bottom() - 1.0),
                            Stroke::new(2.0_f32, ui.palette().accent),
                        );
                        drag.insert = Some(slot);
                    }
                    if ctx.input(|i| i.pointer.any_released()) {
                        dropped = self.pane_drag.take().map(|drag| (drag.id, drag.insert));
                    }
                }
            });

        let mut changed = false;
        if let Some(id) = toggle {
            self.config.panes.toggle_collapsed(&id);
            changed = true;
        }
        if let Some((id, height)) = resized {
            self.config.panes.set_height(&id, height);
            changed |= resize_done;
        }
        if let Some((id, Some(slot))) = dropped {
            // Slots count visible panes; map the one before it back to the layout.
            let to = match entries.get(slot) {
                Some(entry) => self.config.panes.index_of(&entry.id),
                None => Some(self.config.panes.0.len()),
            };
            if let (Some(from), Some(to)) = (self.config.panes.index_of(&id), to) {
                changed |= self.config.panes.move_pane(from, to);
            }
        }
        if changed {
            self.save_config();
        }
    }

    fn pane_header(&self, ui: &mut egui::Ui, entry: &Entry, enabled: bool) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), HEADER_HEIGHT),
            Sense::click_and_drag(),
        );
        let painter = ui.painter();
        let dragging = self
            .pane_drag
            .as_ref()
            .is_some_and(|drag| drag.id == entry.id);
        if response.hovered() || dragging {
            painter.rect_filled(rect, 0.0, ui.palette().pane_hover);
        }
        let center = rect.left_center() + vec2(13.0, 0.0);
        let offsets = if entry.collapsed {
            [vec2(-2.0, -4.0), vec2(2.0, 0.0), vec2(-2.0, 4.0)]
        } else {
            [vec2(-4.0, -2.0), vec2(0.0, 2.0), vec2(4.0, -2.0)]
        };
        painter.add(egui::Shape::line(
            offsets.into_iter().map(|offset| center + offset).collect(),
            Stroke::new(1.5_f32, ui.palette().muted),
        ));
        let text = if enabled {
            ui.palette().text
        } else {
            ui.palette().muted
        };
        painter.text(
            pos2(rect.left() + 26.0, rect.center().y),
            Align2::LEFT_CENTER,
            &entry.title,
            FontId::proportional(13.0),
            text,
        );
        if let Some(detail) = &entry.detail {
            painter.text(
                pos2(rect.right() - 12.0, rect.center().y),
                Align2::RIGHT_CENTER,
                detail,
                FontId::proportional(11.0),
                ui.palette().muted,
            );
        }
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0_f32, ui.palette().divider),
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::CollapsingHeader,
                enabled,
                !entry.collapsed,
                entry.title.clone(),
            )
        });
        response.on_hover_cursor(if dragging {
            CursorIcon::Grabbing
        } else {
            CursorIcon::Default
        })
    }

    fn pane_body(&mut self, ui: &mut egui::Ui, id: &str, enabled: bool) {
        match id {
            panes::LAYERS => self.layers_pane(ui, enabled),
            panes::NAVIGATOR => self.navigator_pane(ui, enabled),
            panes::CHANNELS => self.channels_pane(ui, enabled),
            _ => self.plugin_pane(ui, id, enabled),
        }
    }
}

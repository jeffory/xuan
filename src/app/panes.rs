//! The right sidebar: a stack of collapsible panes that the user can reorder
//! by dragging their headers and resize with the splitters between them. The
//! arrangement lives in the user configuration ([`xuan::panes`]).
use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, pos2, vec2};
use xuan::{
    i18n::tr,
    panes::{self, MIN_HEIGHT},
};

use super::{EditorApp, theme};

const HEADER_HEIGHT: f32 = 26.0;
const SPLITTER_HEIGHT: f32 = 5.0;

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
    fill: bool,
}

impl EditorApp {
    /// Translated title of a pane the editor can show, or `None` for a pane
    /// from a plugin that is not installed.
    pub(super) fn pane_title(&self, id: &str) -> Option<String> {
        match id {
            panes::LAYERS => Some(tr("Layers").into()),
            panes::NAVIGATOR => Some(tr("Navigator").into()),
            _ => self.plugin_pane_title(id),
        }
    }

    fn pane_detail(&self, id: &str) -> Option<String> {
        match id {
            panes::LAYERS => self
                .session()
                .map(|session| session.document.layers.len().to_string()),
            panes::NAVIGATOR => self
                .session()
                .map(|session| super::navigator::format_zoom_percent(session.zoom)),
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
        let enabled = self.dialog.is_none() && self.job.is_none();
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
            .frame(egui::Frame::new().fill(theme::PANEL))
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
                let fill_height = (ui.available_height() - fixed).max(MIN_HEIGHT);
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
                            entry.height
                        };
                        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), height));
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                            ui.spacing_mut().item_spacing.y = 0.0;
                            self.pane_body(ui, &entry.id, enabled);
                        });
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
                                        theme::ACCENT
                                    } else {
                                        theme::DIVIDER
                                    },
                                ),
                            );
                            if splitter.dragged() {
                                resized = Some((
                                    entry.id.clone(),
                                    entry.height + splitter.drag_delta().y,
                                ));
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
                            Stroke::new(2.0_f32, theme::ACCENT),
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
            painter.rect_filled(rect, 0.0, Color32::from_gray(44));
        }
        let center = rect.left_center() + vec2(13.0, 0.0);
        let offsets = if entry.collapsed {
            [vec2(-2.0, -4.0), vec2(2.0, 0.0), vec2(-2.0, 4.0)]
        } else {
            [vec2(-4.0, -2.0), vec2(0.0, 2.0), vec2(4.0, -2.0)]
        };
        painter.add(egui::Shape::line(
            offsets.into_iter().map(|offset| center + offset).collect(),
            Stroke::new(1.5_f32, theme::MUTED),
        ));
        let text = if enabled { theme::TEXT } else { theme::MUTED };
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
                theme::MUTED,
            );
        }
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0_f32, theme::DIVIDER),
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
            _ => self.plugin_pane(ui, id, enabled),
        }
    }
}

//! Drawing a plugin pane from the widget tree the plugin returned.
use std::path::Path;

use egui::RichText;
use serde_json::Value;
use xuan::{
    i18n::tr,
    plugins::ui::{Event, Node},
};

use super::{
    EditorApp,
    plugins::{PaneState, PendingStart},
    theme, widgets,
};

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// What the drawing pass needs besides the tree.
struct Pane<'a> {
    state: &'a mut PaneState,
    plugin_dir: &'a Path,
    ctx: &'a egui::Context,
    event: Option<Event>,
    salt: &'a str,
}

impl EditorApp {
    /// Draw a plugin pane. While a dialog is open (`enabled` is false) it is
    /// shown but takes no input, like the built-in panes.
    pub(super) fn plugin_pane(&mut self, ui: &mut egui::Ui, key: &str, enabled: bool) {
        ui.add_enabled_ui(enabled, |ui| self.plugin_pane_contents(ui, key));
    }

    fn plugin_pane_contents(&mut self, ui: &mut egui::Ui, key: &str) {
        let Some((plugin, _)) = key.strip_prefix("plugin:").and_then(|k| k.split_once('/')) else {
            return;
        };
        let plugin = plugin.to_owned();
        let plugin_dir = self
            .plugins
            .manifest(&plugin)
            .map(|m| m.dir.clone())
            .unwrap_or_default();
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                if !self.plugin_granted(&plugin) {
                    let name = self
                        .plugins
                        .manifest(&plugin)
                        .map(|m| m.plugin.name.clone())
                        .unwrap_or_default();
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{name} {}",
                                tr("needs your permission to run.")
                            ))
                            .color(theme::MUTED),
                        )
                        .wrap(),
                    );
                    if widgets::button(ui, tr("Review Permissions…")).clicked() {
                        self.plugins.permission_request =
                            Some((plugin.clone(), PendingStart::Pane(key.to_owned())));
                        self.dialog = Some(super::Dialog::PluginPermissions);
                    }
                    return;
                }
                let needs_open = self
                    .plugins
                    .panes
                    .get(key)
                    .is_none_or(|s| s.tree.is_none() && !s.pending && s.error.is_none());
                if needs_open {
                    self.render_pane(key, "open", None);
                }
                let state = self.plugins.panes.entry(key.to_owned()).or_default();
                if let Some(error) = &state.error {
                    ui.add(
                        egui::Label::new(RichText::new(error).small().color(theme::MUTED)).wrap(),
                    );
                    if widgets::button(ui, tr("Retry")).clicked() {
                        state.error = None;
                        state.tree = None;
                    }
                }
                let Some(tree) = state.tree.take() else {
                    if state.pending {
                        ui.spinner();
                    }
                    return;
                };
                let event = {
                    let ctx = ui.ctx().clone();
                    let mut pane = Pane {
                        state,
                        plugin_dir: &plugin_dir,
                        ctx: &ctx,
                        event: None,
                        salt: key,
                    };
                    egui::ScrollArea::vertical()
                        .id_salt(("plugin_pane_scroll", key))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            draw(ui, &tree, &mut pane);
                        });
                    pane.event
                };
                let state = self.plugins.panes.entry(key.to_owned()).or_default();
                if state.tree.is_none() {
                    state.tree = Some(tree);
                }
                if let Some(event) = event {
                    self.render_pane(key, "event", Some(event));
                }
            });
    }
}

fn send(pane: &mut Pane, widget: &str, value: Value) {
    pane.event = Some(Event {
        widget: widget.to_owned(),
        value,
    });
}

fn draw(ui: &mut egui::Ui, node: &Node, pane: &mut Pane) {
    match node {
        Node::Column { children, gap } => {
            ui.vertical(|ui| {
                if let Some(gap) = gap {
                    ui.spacing_mut().item_spacing.y = gap.max(0.0);
                }
                for child in children {
                    draw(ui, child, pane);
                }
            });
        }
        Node::Row { children, gap } => {
            ui.horizontal_wrapped(|ui| {
                if let Some(gap) = gap {
                    ui.spacing_mut().item_spacing.x = gap.max(0.0);
                }
                for child in children {
                    draw(ui, child, pane);
                }
            });
        }
        Node::Heading { text } => {
            ui.label(RichText::new(text).strong());
        }
        Node::Label {
            text,
            muted,
            small,
            wrap,
        } => {
            let mut rich = RichText::new(text);
            if *muted {
                rich = rich.color(theme::MUTED);
            }
            if *small {
                rich = rich.small();
            }
            ui.add(egui::Label::new(rich).wrap_mode(if *wrap {
                egui::TextWrapMode::Wrap
            } else {
                egui::TextWrapMode::Truncate
            }));
        }
        Node::Separator => {
            ui.separator();
        }
        Node::Space { size } => ui.add_space(size.clamp(0.0, 400.0)),
        Node::Button {
            id,
            label,
            primary,
            enabled,
        } => {
            let response = ui.add_enabled_ui(*enabled, |ui| {
                if *primary {
                    widgets::primary_button(ui, label)
                } else {
                    widgets::button(ui, label)
                }
            });
            if response.inner.clicked() {
                send(pane, id, Value::Bool(true));
            }
        }
        Node::Checkbox { id, label, value } => {
            let mut checked = *value;
            if widgets::checkbox(ui, &mut checked, label).changed() {
                send(pane, id, Value::Bool(checked));
            }
        }
        Node::Text {
            id,
            value,
            placeholder,
            multiline,
            width,
        } => {
            let draft = pane
                .state
                .drafts
                .entry(id.clone())
                .or_insert_with(|| value.clone());
            let egui_id = egui::Id::new(("plugin_text", pane.salt, id));
            let width = width.unwrap_or(f32::INFINITY);
            let response = if *multiline {
                ui.add(
                    egui::TextEdit::multiline(draft)
                        .id(egui_id)
                        .hint_text(placeholder)
                        .desired_rows(3)
                        .desired_width(width),
                )
            } else {
                ui.add(
                    egui::TextEdit::singleline(draft)
                        .id(egui_id)
                        .hint_text(placeholder)
                        .desired_width(width),
                )
            };
            let submitted = !*multiline
                && response.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (response.lost_focus() || submitted) && draft != value {
                let text = draft.clone();
                send(pane, id, Value::String(text));
            } else if !response.has_focus() && draft != value && !response.changed() {
                // The plugin changed the value while nothing was being typed.
                *draft = value.clone();
            }
        }
        Node::Number {
            id,
            value,
            min,
            max,
            step,
            suffix,
            integer,
        } => {
            let mut number = *value;
            let mut field = widgets::Number::new(&mut number)
                .size(egui::vec2(90.0, 22.0))
                .speed(step.unwrap_or(if *integer { 1.0 } else { 0.1 }))
                .range(min.unwrap_or(f64::MIN)..=max.unwrap_or(f64::MAX))
                .suffix(suffix);
            if *integer {
                field = field.max_decimals(0);
            }
            let response = ui.add(field);
            if (response.changed() && !response.dragged() || response.drag_stopped())
                && number != *value
            {
                send(pane, id, Value::from(number));
            }
        }
        Node::Slider {
            id,
            value,
            min,
            max,
            label,
            suffix,
            logarithmic,
        } => {
            let mut number = *value;
            ui.spacing_mut().slider_width = (ui.available_width() - 110.0).max(60.0);
            let response = ui.add(
                widgets::Slider::new(&mut number, *min..=max.max(*min))
                    .text(label)
                    .suffix(suffix)
                    .logarithmic(*logarithmic),
            );
            if (response.drag_stopped() || (response.changed() && !response.dragged()))
                && number != *value
            {
                send(pane, id, Value::from(number));
            }
        }
        Node::Select { id, value, options } => {
            let selected = options
                .iter()
                .find(|o| &o.id == value)
                .map(|o| {
                    if o.label.is_empty() {
                        o.id.clone()
                    } else {
                        o.label.clone()
                    }
                })
                .unwrap_or_else(|| value.clone());
            let mut choice = value.clone();
            widgets::PopUp::from_id_salt(("plugin_select", pane.salt, id))
                .selected_text(selected)
                .width(ui.available_width().min(220.0))
                .show_ui(ui, |ui| {
                    for option in options {
                        let label = if option.label.is_empty() {
                            &option.id
                        } else {
                            &option.label
                        };
                        widgets::menu_choice(ui, &mut choice, option.id.clone(), label);
                    }
                });
            if &choice != value {
                send(pane, id, Value::String(choice));
            }
        }
        Node::Color { id, value } => {
            let mut color = Node::color(value).unwrap_or([255, 255, 255, 255]);
            if widgets::color_well(ui, &mut color).changed() {
                send(pane, id, Value::String(Node::color_text(color)));
            }
        }
        Node::Image {
            src,
            width,
            height,
            fit,
        } => {
            if let Some(texture) = image_texture(pane, src) {
                let size = texture.size_vec2();
                let available = ui.available_width().max(1.0);
                let mut shown = match (width, height) {
                    (Some(w), Some(h)) => egui::vec2(*w, *h),
                    (Some(w), None) => egui::vec2(*w, *w * size.y / size.x.max(1.0)),
                    (None, Some(h)) => egui::vec2(*h * size.x / size.y.max(1.0), *h),
                    (None, None) => size,
                };
                if *fit && shown.x > available {
                    shown = egui::vec2(available, available * shown.y / shown.x.max(1.0));
                }
                ui.add(egui::Image::new((texture.id(), shown)).corner_radius(3.0));
            } else {
                ui.label(
                    RichText::new(tr("Image unavailable"))
                        .small()
                        .color(theme::MUTED),
                );
            }
        }
        Node::Progress { value, label } => {
            ui.horizontal(|ui| {
                match value {
                    Some(fraction) => {
                        ui.add(
                            egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                                .desired_width(
                                    ui.available_width()
                                        - if label.is_empty() { 0.0 } else { 90.0 },
                                )
                                .desired_height(6.0),
                        );
                    }
                    None => {
                        ui.spinner();
                    }
                }
                if !label.is_empty() {
                    ui.label(RichText::new(label).small().color(theme::MUTED));
                }
            });
        }
        Node::List {
            id,
            items,
            selected,
        } => {
            for item in items {
                let is_selected = selected.as_ref() == Some(&item.id);
                let response = ui.horizontal(|ui| {
                    if let Some(icon) = &item.icon
                        && let Some(texture) = image_texture(pane, icon)
                    {
                        ui.add(
                            egui::Image::new((texture.id(), egui::vec2(24.0, 24.0)))
                                .corner_radius(2.0),
                        );
                    }
                    let response = ui.selectable_label(is_selected, &item.label);
                    if !item.detail.is_empty() {
                        ui.label(RichText::new(&item.detail).small().color(theme::MUTED));
                    }
                    response
                });
                if response.inner.clicked() {
                    send(pane, id, Value::String(item.id.clone()));
                }
            }
        }
        Node::Swatches {
            id,
            colors,
            selected,
        } => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for color in colors {
                    let Some(rgba) = Node::color(color) else {
                        continue;
                    };
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                    ui.painter().rect_filled(
                        rect,
                        3.0,
                        egui::Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]),
                    );
                    if selected.as_ref() == Some(color) {
                        ui.painter().rect_stroke(
                            rect,
                            3.0,
                            egui::Stroke::new(2.0_f32, theme::ACCENT),
                            egui::StrokeKind::Outside,
                        );
                    }
                    let response = response.on_hover_text(color);
                    if response.clicked()
                        && let Some(id) = id
                    {
                        send(pane, id, Value::String(color.clone()));
                    }
                }
            });
        }
        Node::Link { label, url } => {
            if url.starts_with("https://") || url.starts_with("http://") {
                ui.hyperlink_to(label, url);
            } else {
                ui.label(label);
            }
        }
    }
}

/// Load or reuse the texture for an image source: a PNG path (relative to
/// the plugin folder) or a `data:image/png;base64,` URL.
fn image_texture(pane: &mut Pane, src: &str) -> Option<egui::TextureHandle> {
    let (bytes, stamp) = if let Some(data) = src.strip_prefix("data:") {
        if let Some((_, cached)) = pane.state.images.get(src) {
            return Some(cached.clone());
        }
        let encoded = data.split_once(";base64,")?.1;
        (xuan::plugins::ui::decode_base64(encoded)?, None)
    } else {
        let path = Path::new(src);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            pane.plugin_dir.join(path)
        };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some((cached_stamp, cached)) = pane.state.images.get(src)
            && *cached_stamp == modified
            && modified.is_some()
        {
            return Some(cached.clone());
        }
        let metadata = std::fs::metadata(&path).ok()?;
        if metadata.len() as usize > MAX_IMAGE_BYTES {
            return None;
        }
        (std::fs::read(&path).ok()?, modified)
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    if image.width() > 8192 || image.height() > 8192 {
        return None;
    }
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    );
    let texture = pane.ctx.load_texture(
        format!("plugin-pane-{}", pane.state.images.len()),
        color,
        egui::TextureOptions::LINEAR,
    );
    if pane.state.images.len() > 64 {
        pane.state.images.clear();
    }
    pane.state
        .images
        .insert(src.to_owned(), (stamp, texture.clone()));
    Some(texture)
}

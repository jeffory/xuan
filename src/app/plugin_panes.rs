//! Drawing a plugin pane from the widget tree the plugin returned.
use super::theme::PaletteExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use egui::RichText;
use serde_json::Value;
use xuan::{
    i18n::tr,
    plugins::{
        edits::Access,
        ui::{Event, Node},
    },
};

use super::{
    EditorApp,
    plugins::{PaneImage, PaneState, PendingStart},
    widgets,
};

const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Longest side of a pane image.
const MAX_IMAGE_SIDE: u32 = 8192;
/// Images kept per pane before the cache starts over.
const MAX_CACHED_IMAGES: usize = 64;

/// What the drawing pass needs besides the tree.
struct Pane<'a> {
    state: &'a mut PaneState,
    plugin_dir: &'a Path,
    /// The folders image files may come from.
    access: &'a Access,
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
        // Without the file system permission, the plugin's own folders only.
        let access = self.plugins.access(&plugin);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                if self.plugin_offline(&plugin) {
                    let name = self.plugins.source(&plugin);
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{name} {}",
                                tr("uses the network, and plugins that use the network are disabled.")
                            ))
                            .color(ui.palette().muted),
                        )
                        .wrap(),
                    );
                    if widgets::button(ui, tr("Manage Plugins…")).clicked() {
                        self.plugins.manager_selected = Some(plugin.clone());
                        self.dialog = Some(super::Dialog::Plugins);
                    }
                    return;
                }
                if !self.plugin_granted(&plugin) {
                    let name = self.plugins.source(&plugin);
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{name} {}",
                                tr("needs your permission to run.")
                            ))
                            .color(ui.palette().muted),
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
                        egui::Label::new(RichText::new(error).small().color(ui.palette().muted)).wrap(),
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
                        access: &access,
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
                rich = rich.color(ui.palette().muted);
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
            copy,
        } => {
            // A copy button says what it copies, and how many lines.
            let label = match copy {
                Some(text) => copy_label(label, text),
                None => label.clone(),
            };
            let response = ui.add_enabled_ui(*enabled, |ui| {
                let response = if *primary {
                    widgets::primary_button(ui, &label)
                } else {
                    widgets::button(ui, &label)
                };
                match copy {
                    Some(text) => response.on_hover_text(copy_hint(text)),
                    None => response,
                }
            });
            if response.inner.clicked() {
                // Only on the user's click, never on the plugin's own.
                if let Some(text) = copy {
                    ui.ctx().copy_text(text.clone());
                }
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
                        .color(ui.palette().muted),
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
                    ui.label(RichText::new(label).small().color(ui.palette().muted));
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
                        ui.label(
                            RichText::new(&item.detail)
                                .small()
                                .color(ui.palette().muted),
                        );
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
                            egui::Stroke::new(2.0_f32, ui.palette().accent),
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
/// the plugin folder) or a `data:image/png;base64,` URL. Failures are cached
/// like textures, and a file is read again only when it changes.
fn image_texture(pane: &mut Pane, src: &str) -> Option<egui::TextureHandle> {
    let file = (!src.starts_with("data:")).then(|| {
        let path = Path::new(src);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            pane.plugin_dir.join(path)
        }
    });
    // Looking at the file never opens it, so a FIFO cannot block here.
    let stamp = file.as_ref().and_then(|path| {
        std::fs::metadata(path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| (m.modified().ok(), m.len()))
    });
    if let Some(cached) = pane.state.images.get(src)
        && cached.stamp == stamp
    {
        return cached.texture.clone();
    }
    let image = match &file {
        Some(path) => read_pane_image(path, pane.access),
        None => data_url_image(src),
    };
    let texture = image.ok().map(|image| {
        pane.ctx.load_texture(
            format!("plugin-pane-{}-{}", pane.salt, pane.state.images.len()),
            image,
            egui::TextureOptions::LINEAR,
        )
    });
    if pane.state.images.len() >= MAX_CACHED_IMAGES {
        pane.state.images.clear();
    }
    pane.state.images.insert(
        src.to_owned(),
        PaneImage {
            stamp,
            texture: texture.clone(),
        },
    );
    texture
}

/// Read a pane image file: a regular file inside the plugin's folders.
fn read_pane_image(path: &Path, access: &Access) -> Result<egui::ColorImage> {
    let resolved: PathBuf = access.readable(path)?;
    let metadata = std::fs::metadata(&resolved)?;
    ensure!(metadata.is_file(), "not a regular file");
    ensure!(metadata.len() <= MAX_IMAGE_BYTES, "image file too large");
    decode_pane_image(std::fs::read(&resolved)?)
}

fn data_url_image(src: &str) -> Result<egui::ColorImage> {
    let encoded = src
        .strip_prefix("data:")
        .and_then(|data| data.split_once(";base64,"))
        .context("not a base64 data URL")?
        .1;
    ensure!(
        encoded.len() as u64 <= MAX_IMAGE_BYTES * 4 / 3 + 4,
        "image too large"
    );
    decode_pane_image(xuan::plugins::ui::decode_base64(encoded).context("invalid base64")?)
}

/// Decode with the size limits applied before any pixels are allocated.
fn decode_pane_image(bytes: Vec<u8>) -> Result<egui::ColorImage> {
    ensure!(bytes.len() as u64 <= MAX_IMAGE_BYTES, "image too large");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(u64::from(MAX_IMAGE_SIDE) * u64::from(MAX_IMAGE_SIDE) * 8);
    reader.limits(limits);
    let image = reader.decode()?.to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

/// A copy button's label, marked when it copies more than one line.
fn copy_label(label: &str, text: &str) -> String {
    match text.lines().count() {
        0 | 1 => label.to_owned(),
        lines => format!("{label} ({lines} {})", tr("lines")),
    }
}

/// The tooltip of a copy button: exactly what it puts on the clipboard.
fn copy_hint(text: &str) -> String {
    let lines = text.lines().count();
    let lead = if lines > 1 {
        format!("{} {lines} {}", tr("Copies"), tr("lines to the clipboard:"))
    } else {
        tr("Copies to the clipboard:").to_owned()
    };
    format!("{lead}\n{text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use xuan::plugins::manifest::FilesystemAccess;

    #[test]
    fn copy_buttons_show_what_they_copy() {
        assert_eq!(copy_label("Copy URL", "http://x"), "Copy URL");
        assert_eq!(copy_label("Copy JSON", "{\n}\n"), "Copy JSON (2 lines)");
        assert_eq!(copy_hint("abc"), "Copies to the clipboard:\nabc");
        assert_eq!(
            copy_hint("a\nb\nc"),
            "Copies 3 lines to the clipboard:\na\nb\nc"
        );
    }

    fn pane_image(dir: &Path, name: &str, size: u32) -> PathBuf {
        let path = dir.join(name);
        image::RgbaImage::from_pixel(size, size, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        path
    }

    #[test]
    fn pane_images_are_confined_limited_and_cached() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let access = Access::new([dir.path().to_path_buf()], FilesystemAccess::None);
        pane_image(dir.path(), "ok.png", 4);
        let foreign = pane_image(outside.path(), "foreign.png", 4);
        assert!(read_pane_image(&dir.path().join("ok.png"), &access).is_ok());
        assert!(read_pane_image(&foreign, &access).is_err());
        assert!(read_pane_image(dir.path(), &access).is_err());
        // Too large an image is refused from its header.
        let big = image::RgbaImage::new(MAX_IMAGE_SIDE + 1, 1);
        let mut bytes = Vec::new();
        big.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        assert!(decode_pane_image(bytes).is_err());
        assert!(data_url_image("data:image/png;base64,***").is_err());
        #[cfg(unix)]
        {
            let fifo = dir.path().join("fifo.png");
            if std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .is_ok_and(|s| s.success())
            {
                assert!(read_pane_image(&fifo, &access).is_err());
            }
        }

        let ctx = egui::Context::default();
        let mut state = PaneState::default();
        let mut pane = Pane {
            state: &mut state,
            plugin_dir: dir.path(),
            access: &access,
            ctx: &ctx,
            event: None,
            salt: "test",
        };
        assert!(image_texture(&mut pane, "ok.png").is_some());
        assert!(image_texture(&mut pane, &foreign.display().to_string()).is_none());
        // A broken file is remembered as broken until it changes.
        std::fs::write(dir.path().join("broken.png"), b"not a png").unwrap();
        assert!(image_texture(&mut pane, "broken.png").is_none());
        assert!(pane.state.images["broken.png"].texture.is_none());
        assert!(pane.state.images["broken.png"].stamp.is_some());
        assert!(image_texture(&mut pane, "missing.png").is_none());
        assert!(pane.state.images.contains_key("missing.png"));
        pane_image(dir.path(), "broken.png", 6);
        assert!(image_texture(&mut pane, "broken.png").is_some());
        assert!(image_texture(&mut pane, "data:image/png;base64,***").is_none());
        assert!(pane.state.images.contains_key("data:image/png;base64,***"));
    }
}

use egui::{Color32, FontId, Rect, Sense, Stroke, StrokeKind, emath::GuiRounding as _, pos2, vec2};
use xuan::config::WindowButtons;
use xuan::{config::TitleBar, i18n::tr};

use super::{
    EditorApp,
    theme::{self, PaletteExt},
};

/// Size of one compact-style window button.
pub(super) const BUTTON_SIZE: egui::Vec2 = vec2(30.0, 22.0);

pub(super) fn title_bar(
    palette: &theme::Palette,
    radius: u8,
    id: &'static str,
) -> egui::TopBottomPanel {
    egui::TopBottomPanel::top(id).exact_height(32.0).frame(
        egui::Frame::new()
            .fill(palette.titlebar)
            .corner_radius(egui::CornerRadius {
                nw: radius,
                ne: radius,
                sw: 0,
                se: 0,
            })
            .inner_margin(egui::Margin::symmetric(14, 5)),
    )
}

pub(super) fn status_bar(
    palette: &theme::Palette,
    radius: u8,
    id: &'static str,
) -> egui::TopBottomPanel {
    egui::TopBottomPanel::bottom(id).exact_height(30.0).frame(
        egui::Frame::new()
            .fill(palette.panel)
            .corner_radius(egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: radius,
                se: radius,
            })
            .inner_margin(egui::Margin::symmetric(18, 4)),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

/// Which compact-style window buttons sit on each side of the title bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ButtonLayout {
    pub left: Vec<WindowButton>,
    pub right: Vec<WindowButton>,
}

impl Default for ButtonLayout {
    /// Windows, KDE and most GNOME distributions put all three on the right.
    fn default() -> Self {
        Self {
            left: Vec::new(),
            right: vec![
                WindowButton::Minimize,
                WindowButton::Maximize,
                WindowButton::Close,
            ],
        }
    }
}

impl ButtonLayout {
    /// Parses GNOME's `org.gnome.desktop.wm.preferences button-layout`, such as
    /// `'appmenu:minimize,maximize,close'`. Unknown entries (`appmenu`, `icon`,
    /// `spacer`) are skipped. Returns `None` when no button is left, so the
    /// window can always be closed from the title bar.
    pub(super) fn parse_gnome(value: &str) -> Option<Self> {
        let value = value.trim().trim_matches('\'');
        let (left, right) = value.split_once(':').unwrap_or((value, ""));
        let side = |text: &str| -> Vec<WindowButton> {
            text.split(',')
                .filter_map(|name| match name.trim() {
                    "minimize" => Some(WindowButton::Minimize),
                    "maximize" => Some(WindowButton::Maximize),
                    "close" => Some(WindowButton::Close),
                    _ => None,
                })
                .collect()
        };
        let layout = Self {
            left: side(left),
            right: side(right),
        };
        (!layout.left.is_empty() || !layout.right.is_empty()).then_some(layout)
    }

    /// Parses KWin's `[org.kde.kdecoration2]` `ButtonsOnLeft` / `ButtonsOnRight` from
    /// `kwinrc`: `I` is minimize, `A` maximize and `X` close; the other letters (menu,
    /// help, pin, ...) are ignored. A missing key keeps KWin's own default for that
    /// side, and `None` means no button is left, so the window can always be closed.
    #[cfg(target_os = "linux")]
    pub(super) fn parse_kwin(text: &str) -> Option<Self> {
        let ini = super::window_theme::Ini::parse(text);
        let side = |key: &str, default: &str| -> Vec<WindowButton> {
            ini.get("org.kde.kdecoration2", key)
                .unwrap_or(default)
                .chars()
                .filter_map(|letter| match letter {
                    'I' => Some(WindowButton::Minimize),
                    'A' => Some(WindowButton::Maximize),
                    'X' => Some(WindowButton::Close),
                    _ => None,
                })
                .collect()
        };
        let layout = Self {
            left: side("ButtonsOnLeft", "MS"),
            right: side("ButtonsOnRight", "HIAX"),
        };
        (!layout.left.is_empty() || !layout.right.is_empty()).then_some(layout)
    }

    /// Follows KWin's button layout under KDE and GNOME's under GNOME. Elsewhere, or if
    /// the setting is missing or slow to read, keeps the default.
    pub(super) fn from_desktop() -> Self {
        #[cfg(target_os = "linux")]
        if std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|desktop| super::window_theme::desktop_is(&desktop, "KDE"))
        {
            return std::fs::read_to_string(
                super::window_theme::process_config_home().join("kwinrc"),
            )
            .ok()
            .and_then(|text| Self::parse_kwin(&text))
            .unwrap_or_default();
        }
        if !cfg!(target_os = "linux")
            || !std::env::var("XDG_CURRENT_DESKTOP")
                .is_ok_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case("GNOME")))
        {
            return Self::default();
        }
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let output = std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.wm.preferences", "button-layout"])
                .stderr(std::process::Stdio::null())
                .output();
            let _ = send.send(output);
        });
        receive
            .recv_timeout(std::time::Duration::from_millis(300))
            .ok()
            .and_then(Result::ok)
            .filter(|output| output.status.success())
            .and_then(|output| Self::parse_gnome(&String::from_utf8_lossy(&output.stdout)))
            .unwrap_or_default()
    }
}

/// A window theme shared between the main title bar and the dialogs.
pub(super) type SharedWindowTheme =
    std::sync::Arc<std::sync::Mutex<Option<super::window_theme::WindowTheme>>>;

/// What a compact-style button needs to be drawn: the desktop theme's images, unless
/// the user chose the built-in glyphs.
#[derive(Clone)]
pub(super) struct ButtonStyle {
    pub buttons: WindowButtons,
    pub theme: SharedWindowTheme,
}

/// The title bar settings the dialogs in `widgets::Window` follow, published every frame.
#[derive(Clone)]
pub(super) struct DialogChrome {
    pub title_bar: TitleBar,
    pub layout: ButtonLayout,
    pub style: ButtonStyle,
}

/// Paints one compact-style window button: the theme's image when there is one, else
/// the built-in monochrome glyph, with the hover and pressed states.
pub(super) fn paint_window_button(
    ui: &egui::Ui,
    style: &ButtonStyle,
    rect: Rect,
    button: WindowButton,
    maximized: bool,
    response: &egui::Response,
    focused: bool,
) {
    #[cfg(target_os = "linux")]
    if paint_theme_button(style, ui, rect, button, maximized, response, focused) {
        return;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (style.buttons, &style.theme);
    let painter = ui.painter();
    let p = ui.palette();
    if response.hovered() || response.has_focus() {
        let fill = if response.is_pointer_button_down_on() {
            p.pressed
        } else {
            p.hover
        };
        painter.rect_filled(rect.shrink2(vec2(2.0, 0.0)), theme::BUTTON_RADIUS, fill);
    }
    let color = if focused || response.hovered() {
        p.text
    } else {
        p.muted
    };
    let stroke = Stroke::new(1.0_f32, color);
    // Pixel-centered 1 px strokes stay crisp at integer offsets.
    let c = rect
        .center()
        .round_to_pixel_center(painter.pixels_per_point());
    match button {
        WindowButton::Minimize => {
            painter.line_segment([c + vec2(-4.0, 0.0), c + vec2(4.0, 0.0)], stroke);
        }
        WindowButton::Maximize if maximized => {
            // Two overlapping windows: the front one, then the visible
            // top and right edges of the one behind it.
            painter.rect_stroke(
                Rect::from_min_max(c + vec2(-4.0, -2.0), c + vec2(2.0, 4.0)),
                0.0,
                stroke,
                StrokeKind::Middle,
            );
            painter.add(egui::Shape::line(
                vec![
                    c + vec2(-2.0, -2.0),
                    c + vec2(-2.0, -4.0),
                    c + vec2(4.0, -4.0),
                    c + vec2(4.0, 2.0),
                    c + vec2(2.0, 2.0),
                ],
                stroke,
            ));
        }
        WindowButton::Maximize => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(8.0, 8.0)),
                0.0,
                stroke,
                StrokeKind::Middle,
            );
        }
        WindowButton::Close => {
            painter.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], stroke);
            painter.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], stroke);
        }
    }
}

/// Draws the button with the desktop theme's image. `false` means none was found (or
/// the setting is Built-in) and the caller draws its own glyph.
#[cfg(target_os = "linux")]
fn paint_theme_button(
    style: &ButtonStyle,
    ui: &egui::Ui,
    rect: Rect,
    button: WindowButton,
    maximized: bool,
    response: &egui::Response,
    focused: bool,
) -> bool {
    use super::window_theme::{Env, Highlight, Kind, State, WindowTheme};
    if style.buttons != WindowButtons::Theme {
        return false;
    }
    let mut guard = style.theme.lock().unwrap_or_else(|e| e.into_inner());
    let theme = guard.get_or_insert_with(|| {
        // Tests never read the developer's own configuration.
        #[cfg(test)]
        let env = Env::default();
        #[cfg(not(test))]
        let env = Env::from_process();
        WindowTheme::new(env)
    });
    theme.refresh(std::time::Instant::now());
    let ppp = ui.ctx().pixels_per_point();
    let kind = match button {
        WindowButton::Minimize => Kind::Minimize,
        WindowButton::Maximize if maximized => Kind::Restore,
        WindowButton::Maximize => Kind::Maximize,
        WindowButton::Close => Kind::Close,
    };
    let state = if response.is_pointer_button_down_on() {
        State::Active
    } else if response.hovered() || response.has_focus() {
        State::Hover
    } else if !focused {
        State::Backdrop
    } else {
        State::Normal
    };
    let Some(resolved) = theme.resolved() else {
        return false;
    };
    let Some(pick) = resolved.pick(kind, state, ppp) else {
        return false;
    };
    let (asset, highlight, dim) = (pick.asset.clone(), pick.highlight, pick.dim);
    let p = ui.palette();
    // A monochrome close icon without a hover image of its own is drawn as Breeze does:
    // a circle in the scheme's negative colour with the glyph in the title bar colour.
    let close_fill = (kind == Kind::Close && asset.symbolic)
        .then(|| resolved.tints.close_fill(highlight, p.titlebar))
        .flatten();
    let tint = asset.symbolic.then(|| {
        if let Some((_, glyph)) = close_fill {
            return glyph;
        }
        let fallback = if state == State::Backdrop {
            p.muted
        } else {
            p.text
        };
        resolved
            .tints
            .color(state == State::Backdrop, p.titlebar, fallback)
    });
    let Some((texture, size)) = theme.texture(ui.ctx(), &asset, ppp, tint) else {
        return false;
    };
    let painter = ui.painter();
    if let Some((fill, _)) = close_fill {
        let radius = (rect.width().min(rect.height()) / 2.0 - 1.0).max(6.0);
        painter.circle_filled(rect.center(), radius, fill);
    } else if highlight != Highlight::None {
        let fill = if highlight == Highlight::Pressed {
            p.pressed
        } else {
            p.hover
        };
        painter.rect_filled(rect.shrink2(vec2(2.0, 0.0)), theme::BUTTON_RADIUS, fill);
    }
    let fit = (rect.width() / size.x).min(rect.height() / size.y).min(1.0);
    let image = Rect::from_center_size(rect.center().round_to_pixel_center(ppp), size * fit);
    let color = if dim && !asset.symbolic {
        Color32::WHITE.gamma_multiply(0.6)
    } else {
        Color32::WHITE
    };
    painter.image(
        texture,
        image,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        color,
    );
    true
}

impl EditorApp {
    /// The window draws rounded corners only with a client-side title bar on a
    /// window that was created transparent, and never when it fills the screen.
    pub(super) fn window_corner_radius(&self, ctx: &egui::Context) -> u8 {
        if self.config.title_bar.client_side() && self.transparent_window {
            theme::window_corner_radius(ctx)
        } else {
            0
        }
    }

    /// Applies the title bar style's decorations. egui can switch decorations
    /// at runtime; transparency (and so rounded corners) is fixed at startup.
    pub(super) fn sync_decorations(&mut self, ctx: &egui::Context) {
        let decorated = !self.config.title_bar.client_side();
        if self.decorated != decorated {
            self.decorated = decorated;
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(decorated));
        }
    }

    /// Records how the native window was created, before the first frame.
    pub fn set_startup_title_bar(&mut self, style: TitleBar) {
        self.transparent_window = style.client_side();
        self.decorated = !style.client_side();
    }

    /// Title-bar items before the menus.
    pub(super) fn leading_window_controls(&mut self, ui: &mut egui::Ui) {
        match self.config.title_bar {
            TitleBar::System => {}
            TitleBar::MacOs => self.traffic_lights(ui),
            TitleBar::Compact => {
                let buttons = self.button_layout.left.clone();
                if !buttons.is_empty() {
                    self.window_buttons(ui, &buttons);
                    ui.add_space(8.0);
                }
            }
        }
    }

    /// Title-bar items after the menus: the centered title, which also moves
    /// the window, and any right-hand window buttons.
    pub(super) fn trailing_window_controls(&mut self, ui: &mut egui::Ui) {
        let buttons = match self.config.title_bar {
            // The system title bar shows the title and moves the window.
            TitleBar::System => return,
            TitleBar::MacOs => Vec::new(),
            TitleBar::Compact => self.button_layout.right.clone(),
        };
        let reserve = if buttons.is_empty() {
            0.0
        } else {
            buttons.len() as f32 * BUTTON_SIZE.x + ui.spacing().item_spacing.x
        };
        self.titlebar_drag(ui, reserve);
        self.window_buttons(ui, &buttons);
    }

    pub(super) fn button_style(&self) -> ButtonStyle {
        ButtonStyle {
            buttons: self.config.window_buttons,
            theme: self.window_theme.clone(),
        }
    }

    /// Hands the title bar style to the dialogs, which are drawn without access to the app.
    pub(super) fn publish_dialog_chrome(&self, ctx: &egui::Context) {
        let chrome = DialogChrome {
            title_bar: self.config.title_bar,
            layout: self.button_layout.clone(),
            style: self.button_style(),
        };
        ctx.data_mut(|data| data.insert_temp(egui::Id::NULL, chrome));
    }

    /// Compact-style monochrome minimize, maximize and close buttons.
    fn window_buttons(&mut self, ui: &mut egui::Ui, buttons: &[WindowButton]) {
        let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        let style = self.button_style();
        let spacing = ui.spacing().item_spacing.x;
        ui.spacing_mut().item_spacing.x = 0.0;
        for &button in buttons {
            let (rect, response) = ui.allocate_exact_size(BUTTON_SIZE, Sense::click());
            let label = match button {
                WindowButton::Minimize => tr("Minimize window"),
                WindowButton::Maximize if maximized => tr("Restore window"),
                WindowButton::Maximize => tr("Maximize window"),
                WindowButton::Close => tr("Close window"),
            };
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            paint_window_button(ui, &style, rect, button, maximized, &response, focused);
            if response.clicked() {
                self.window_button_action(ui.ctx(), button, maximized);
            }
            response.on_hover_text(label);
        }
        ui.spacing_mut().item_spacing.x = spacing;
    }

    fn window_button_action(&mut self, ctx: &egui::Context, button: WindowButton, maximized: bool) {
        match button {
            WindowButton::Close => self.request_quit(),
            WindowButton::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            WindowButton::Maximize => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized))
            }
        }
    }

    /// Quits through the same flow as a window-manager close: the title-bar
    /// close button, File → Quit and Ctrl+Q all end up here.
    pub(super) fn request_quit(&mut self) {
        if self.begin_quit() {
            self.context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Returns `true` when the window may close now. Otherwise starts the
    /// Develop or unsaved-changes prompt, which closes the window when done.
    pub(super) fn begin_quit(&mut self) -> bool {
        if self.allow_close {
            return true;
        }
        if self.develop.is_some() || !self.inactive_develop.is_empty() {
            self.request_develop_close(super::develop::DevelopClose::Window);
            false
        } else if self.sessions.iter().any(|s| s.history.dirty()) {
            if let Some(job) = &self.job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.close_app = true;
            false
        } else {
            true
        }
    }

    /// macOS-style close, minimize and maximize buttons.
    fn traffic_lights(&mut self, ui: &mut egui::Ui) {
        let focused = ui.input(|i| i.viewport().focused.unwrap_or(true));
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        let (group, _) = ui.allocate_exact_size(vec2(62.0, 22.0), Sense::hover());
        let hovered = ui.rect_contains_pointer(group);
        for (index, color, label) in [
            (0, Color32::from_rgb(255, 95, 87), tr("Close window")),
            (1, Color32::from_rgb(254, 188, 46), tr("Minimize window")),
            (
                2,
                Color32::from_rgb(40, 200, 64),
                if maximized {
                    tr("Restore window")
                } else {
                    tr("Maximize window")
                },
            ),
        ] {
            let center = pos2(group.left() + 7.0 + index as f32 * 20.0, group.center().y);
            let rect = Rect::from_center_size(center, vec2(18.0, 22.0));
            let response = ui.interact(
                rect,
                ui.id().with(("window_control", index)),
                Sense::click(),
            );
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            let color = if focused || hovered {
                color
            } else {
                ui.palette().traffic_inactive
            };
            ui.painter().circle_filled(
                center,
                6.0,
                if response.is_pointer_button_down_on() {
                    color.gamma_multiply(0.75)
                } else {
                    color
                },
            );
            ui.painter().circle_stroke(
                center,
                6.0,
                Stroke::new(0.6_f32, Color32::from_black_alpha(65)),
            );
            if hovered || response.has_focus() {
                let stroke = Stroke::new(1.0_f32, Color32::from_black_alpha(170));
                match index {
                    0 => {
                        ui.painter().line_segment(
                            [center - vec2(2.3, 2.3), center + vec2(2.3, 2.3)],
                            stroke,
                        );
                        ui.painter().line_segment(
                            [center + vec2(-2.3, 2.3), center + vec2(2.3, -2.3)],
                            stroke,
                        );
                    }
                    1 => {
                        ui.painter().line_segment(
                            [center - vec2(3.0, 0.0), center + vec2(3.0, 0.0)],
                            stroke,
                        );
                    }
                    _ => {
                        ui.painter().rect_stroke(
                            Rect::from_center_size(center, vec2(5.0, 5.0)),
                            0.0,
                            stroke,
                            StrokeKind::Inside,
                        );
                    }
                }
            }
            if response.clicked() {
                let button = match index {
                    0 => WindowButton::Close,
                    1 => WindowButton::Minimize,
                    _ => WindowButton::Maximize,
                };
                self.window_button_action(ui.ctx(), button, maximized);
            }
            response.on_hover_text(label);
        }
        ui.add_space(8.0);
    }

    fn titlebar_drag(&self, ui: &mut egui::Ui, reserve: f32) {
        let (rect, response) = ui.allocate_exact_size(
            vec2((ui.available_width() - reserve).max(0.0), 22.0),
            Sense::click_and_drag(),
        );
        let title = if let Some(develop) = &self.develop {
            format!("{} — {}", develop.title, tr("Develop"))
        } else {
            self.session().map_or("Xuan".into(), |s| {
                format!("{}{}", s.title, if s.history.dirty() { "  •" } else { "" })
            })
        };
        let center = pos2(ui.ctx().content_rect().center().x, rect.center().y);
        // Keep the title centered on the window and ellipsize before it reaches the menus.
        let half_width = (center.x - rect.left()).min(rect.right() - center.x) - 12.0;
        if half_width > 0.0 {
            let galley = egui::WidgetText::from(title).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                half_width * 2.0,
                FontId::proportional(12.0),
            );
            ui.painter().galley_with_override_text_color(
                center - galley.size() / 2.0,
                galley,
                ui.palette().muted,
            );
        }
        if response.double_clicked() {
            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        } else if response.drag_started_by(egui::PointerButton::Primary) {
            // A native drag grabs the pointer, so let clicks finish before handing it off.
            start_native_drag(ui.ctx(), egui::ViewportCommand::StartDrag);
        }
    }

    /// Undecorated Wayland/X11 windows need client-provided edge hit targets.
    pub(super) fn window_resize(&self, ctx: &egui::Context) {
        // System decorations come with the window manager's own resize borders.
        if !self.config.title_bar.client_side()
            || ctx.input(|i| {
                i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
            })
        {
            return;
        }
        let screen = ctx.content_rect();
        for (index, (rect, direction, cursor)) in resize_regions(screen).into_iter().enumerate() {
            egui::Area::new(egui::Id::new(("window_resize", index)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .movable(false)
                .show(ctx, |ui| {
                    let (_, response) = ui.allocate_exact_size(rect.size(), Sense::drag());
                    if response.hovered() || response.dragged() {
                        ctx.set_cursor_icon(cursor);
                    }
                    if response.is_pointer_button_down_on()
                        && ui.input(|i| i.pointer.primary_pressed())
                    {
                        start_native_drag(ctx, egui::ViewportCommand::BeginResize(direction));
                    }
                });
        }
    }
}

fn start_native_drag(ctx: &egui::Context, command: egui::ViewportCommand) {
    ctx.send_viewport_cmd(command);
    // The compositor can consume the release event (notably on Wayland).
    // End egui's gesture now so the next drag does not need a clearing click.
    ctx.stop_dragging();
    ctx.input_mut(|input| input.pointer = egui::PointerState::default());
}

fn resize_regions(rect: Rect) -> [(Rect, egui::ResizeDirection, egui::CursorIcon); 8] {
    use egui::{CursorIcon as C, ResizeDirection as D};
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let edge = 4.0;
    let corner = 10.0;
    [
        (
            Rect::from_min_max(pos2(l + corner, t), pos2(r - corner, t + edge)),
            D::North,
            C::ResizeVertical,
        ),
        (
            Rect::from_min_max(pos2(l + corner, b - edge), pos2(r - corner, b)),
            D::South,
            C::ResizeVertical,
        ),
        (
            Rect::from_min_max(pos2(l, t + corner), pos2(l + edge, b - corner)),
            D::West,
            C::ResizeHorizontal,
        ),
        (
            Rect::from_min_max(pos2(r - edge, t + corner), pos2(r, b - corner)),
            D::East,
            C::ResizeHorizontal,
        ),
        (
            Rect::from_min_max(pos2(l, t), pos2(l + corner, t + corner)),
            D::NorthWest,
            C::ResizeNwSe,
        ),
        (
            Rect::from_min_max(pos2(r - corner, t), pos2(r, t + corner)),
            D::NorthEast,
            C::ResizeNeSw,
        ),
        (
            Rect::from_min_max(pos2(l, b - corner), pos2(l + corner, b)),
            D::SouthWest,
            C::ResizeNeSw,
        ),
        (
            Rect::from_min_max(pos2(r - corner, b - corner), pos2(r, b)),
            D::SouthEast,
            C::ResizeNwSe,
        ),
    ]
}

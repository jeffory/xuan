//! Xuan's colours and egui style.
//!
//! Every colour the interface paints comes from a [`Palette`]: a set of semantic roles
//! (surfaces, text, accent, control shading, hover and selection overlays, …) with a
//! dark and a light instance. [`apply`] installs a palette into an egui context, and
//! widgets read it back with [`PaletteExt::palette`] (`ui.palette().text`).
//!
//! Colours drawn *on the document* (selection outlines, transform handles, guides) are
//! not part of the palette: they sit on top of arbitrary image content and keep fixed,
//! high-contrast values in both modes.

use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle};

pub const BUTTON_RADIUS: u8 = 5;

/// Text and handles drawn over the image, whatever the interface theme.
pub const ON_CANVAS: Color32 = Color32::from_rgb(235, 235, 237);

const fn gray(l: u8) -> Color32 {
    Color32::from_gray(l)
}
/// Premultiplied white at `alpha`, as `Color32::from_white_alpha` (which is not `const`).
const fn white(alpha: u8) -> Color32 {
    Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha)
}
const fn black(alpha: u8) -> Color32 {
    Color32::from_black_alpha(alpha)
}
const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The interface colours, by role. Overlays (`hover`, `tab_*`, …) are translucent and
/// painted over the surface below them, so they read on either theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// A dark palette: egui's dark visuals underneath, light text.
    pub dark: bool,

    // Surfaces
    /// Panels, sidebars and the window background.
    pub panel: Color32,
    /// Around the document.
    pub canvas: Color32,
    /// Recessed text and number fields, plot backgrounds.
    pub field: Color32,
    /// The title, menu and tab bars, and floating panel headers.
    pub titlebar: Color32,
    /// Floating windows and pop-up menus.
    pub window: Color32,
    /// Striped rows.
    pub faint: Color32,
    /// Rulers.
    pub ruler: Color32,
    /// The Develop workspace's photo backdrop.
    pub preview: Color32,

    // Lines
    /// Separators between panels and inside them.
    pub divider: Color32,
    /// Window, pop-up and field outlines.
    pub border: Color32,
    /// The line under a floating panel's header.
    pub header_rule: Color32,

    // Text and icons
    pub text: Color32,
    /// Secondary text, hints and idle icons.
    pub muted: Color32,
    /// Hidden or unavailable icons.
    pub disabled: Color32,
    /// Error messages.
    pub error: Color32,
    /// Softer error text (load failures in lists).
    pub error_soft: Color32,
    /// Warnings, such as a grid too dense to draw.
    pub warning: Color32,

    // Accent
    pub accent: Color32,
    /// Default buttons: top and bottom of the gradient.
    pub accent_gradient: [Color32; 2],
    /// Default buttons while pressed.
    pub accent_pressed: [Color32; 2],
    /// A checked checkbox.
    pub check: [Color32; 2],
    /// Marks drawn on the accent: check marks and highlighted menu text.
    pub on_accent: Color32,
    /// Text on an accent-coloured highlight (menu rows under the pointer).
    pub on_accent_text: Color32,
    /// Secondary text (shortcut hints) on an accent highlight.
    pub on_accent_muted: Color32,

    // egui's own widgets (combo boxes, text edits, buttons)
    pub widget_fill: Color32,
    pub widget_weak_fill: Color32,
    pub widget_stroke: Color32,
    pub widget_hover_fill: Color32,
    pub widget_hover_weak_fill: Color32,
    pub widget_hover_stroke: Color32,
    pub widget_active_fill: Color32,
    pub widget_open_fill: Color32,

    // Xuan's bezel controls (buttons, pop-ups, segmented controls)
    /// Push buttons: top and bottom of the gradient.
    pub control: [Color32; 2],
    pub control_hover: [Color32; 2],
    pub control_pressed: [Color32; 2],
    /// The inner highlight round a control.
    pub control_edge: Color32,
    /// The drop shadow under a control.
    pub control_shadow: Color32,
    /// An unchecked checkbox.
    pub checkbox: [Color32; 2],
    pub checkbox_hover: [Color32; 2],
    pub checkbox_edge: Color32,
    pub slider_rail: Color32,
    /// The highlight under a slider rail.
    pub slider_rail_edge: Color32,
    /// Slider knob: top and bottom of the gradient.
    pub thumb: [Color32; 2],
    /// The bottom of the knob while it is dragged.
    pub thumb_pressed: Color32,
    pub thumb_edge: Color32,
    pub thumb_shadow: Color32,
    /// Segmented control track: top and bottom.
    pub segment_track: [Color32; 2],
    pub segment_edge: Color32,
    pub segment_separator: Color32,

    // Shadows
    pub window_shadow: Color32,
    pub popup_shadow: Color32,
    /// Dims the window behind the command palette.
    pub modal_backdrop: Color32,

    // Hover and selection
    /// A borderless button under the pointer (title-bar buttons).
    pub hover: Color32,
    /// The same button while pressed.
    pub pressed: Color32,
    /// A list row under the pointer.
    pub row_hover: Color32,
    /// The rule under a layer row.
    pub row_rule: Color32,
    /// The selected layer row.
    pub row_selected: Color32,
    /// A dragged or hovered pane header.
    pub pane_hover: Color32,
    /// Small icon buttons (layer actions, disclosure triangles) under the pointer.
    pub icon_hover: Color32,
    /// Tool rail: selected and hovered buttons, and the selected button's outline.
    pub tool_selected: Color32,
    pub tool_hover: Color32,
    pub tool_selected_edge: Color32,
    /// The outline of a layer thumbnail.
    pub thumbnail_edge: Color32,
    /// The placeholder behind a layer without pixels.
    pub thumbnail_placeholder: Color32,
    /// A floating panel's close dot, idle and hovered.
    pub close_dot: [Color32; 2],
    /// macOS-style window buttons when the window is not focused.
    pub traffic_inactive: Color32,

    // Document tabs
    pub tab_fill: Color32,
    pub tab_hover: Color32,
    pub tab_selected: Color32,
    pub tab_edge: Color32,
    pub tab_selected_edge: Color32,
    /// The round highlight behind a tab's close button.
    pub tab_close_hover: Color32,

    // Rulers
    pub ruler_tick: Color32,
    pub ruler_label: Color32,
    pub ruler_edge: Color32,
    /// The corner mark where the rulers meet.
    pub ruler_corner: Color32,

    // Canvas
    /// The transparency checkerboard: even and odd squares.
    pub checker: [Color32; 2],
    /// The shadow round the document.
    pub canvas_shadow: Color32,
    /// The document's outline.
    pub canvas_edge: Color32,

    // Plots (Levels, Curves, histograms)
    pub plot: Color32,
    /// Levels' histogram background.
    pub histogram_plot: Color32,
    pub plot_grid: Color32,
    /// The identity diagonal of the curves plot.
    pub plot_diagonal: Color32,
    /// Luminosity and composite-channel curves.
    pub plot_line: Color32,
    /// Red, green and blue curves.
    pub plot_channels: [Color32; 3],
    /// Red, green and blue histograms.
    pub histogram: [Color32; 3],
    /// Levels' histogram, for the luminosity and the three channels.
    pub levels_histogram: [Color32; 4],
}

impl Palette {
    /// Xuan's original, macOS-like dark appearance.
    pub const DARK: Self = Self {
        dark: true,

        panel: gray(36),
        canvas: gray(29),
        field: gray(30),
        titlebar: gray(45),
        window: gray(43),
        faint: gray(40),
        ruler: gray(51),
        preview: gray(50),

        divider: gray(54),
        border: gray(83),
        header_rule: gray(24),

        text: rgb(235, 235, 237),
        muted: rgb(154, 154, 157),
        disabled: gray(80),
        error: rgb(255, 128, 128),
        error_soft: rgb(255, 140, 140),
        warning: rgb(255, 170, 60),

        accent: rgb(10, 132, 255),
        accent_gradient: [rgb(65, 155, 255), rgb(22, 112, 231)],
        accent_pressed: [rgb(30, 103, 210), rgb(24, 89, 183)],
        check: [rgb(62, 151, 255), rgb(24, 113, 228)],
        on_accent: Color32::WHITE,
        on_accent_text: rgb(235, 235, 237),
        on_accent_muted: rgb(154, 154, 157),

        widget_fill: gray(72),
        widget_weak_fill: gray(62),
        widget_stroke: gray(91),
        widget_hover_fill: gray(88),
        widget_hover_weak_fill: gray(78),
        widget_hover_stroke: gray(112),
        widget_active_fill: gray(52),
        widget_open_fill: gray(68),

        control: [gray(86), gray(67)],
        control_hover: [gray(94), gray(75)],
        control_pressed: [gray(74), gray(55)],
        control_edge: white(28),
        control_shadow: black(65),
        checkbox: [gray(78), gray(57)],
        checkbox_hover: [gray(95), gray(71)],
        checkbox_edge: white(45),
        slider_rail: gray(70),
        slider_rail_edge: white(12),
        thumb: [gray(255), gray(221)],
        thumb_pressed: gray(190),
        thumb_edge: gray(175),
        thumb_shadow: black(85),
        segment_track: [gray(47), gray(43)],
        segment_edge: gray(66),
        segment_separator: gray(68),

        window_shadow: black(125),
        popup_shadow: black(110),
        modal_backdrop: black(70),

        hover: white(20),
        pressed: white(36),
        row_hover: white(18),
        row_rule: white(14),
        row_selected: gray(57),
        pane_hover: gray(44),
        icon_hover: gray(55),
        tool_selected: gray(62),
        tool_hover: gray(48),
        tool_selected_edge: gray(80),
        thumbnail_edge: white(75),
        thumbnail_placeholder: gray(48),
        close_dot: [gray(98), gray(143)],
        traffic_inactive: gray(83),

        tab_fill: white(9),
        tab_hover: white(19),
        tab_selected: white(31),
        tab_edge: white(20),
        tab_selected_edge: white(56),
        tab_close_hover: white(22),

        ruler_tick: gray(158),
        ruler_label: gray(199),
        ruler_edge: gray(20),
        ruler_corner: white(71),

        checker: [gray(66), gray(80)],
        canvas_shadow: black(60),
        canvas_edge: gray(17),

        plot: gray(28),
        histogram_plot: gray(27),
        plot_grid: gray(55),
        plot_diagonal: gray(75),
        plot_line: Color32::WHITE,
        plot_channels: [
            Color32::LIGHT_RED,
            Color32::LIGHT_GREEN,
            Color32::LIGHT_BLUE,
        ],
        histogram: [rgb(235, 90, 98), rgb(103, 208, 135), rgb(90, 157, 255)],
        levels_histogram: [
            gray(187),
            rgb(220, 102, 99),
            rgb(110, 192, 117),
            rgb(106, 151, 229),
        ],
    };

    /// A light appearance after Firefox's light theme, KDE Breeze Light and GNOME Adwaita:
    /// near-white surfaces, a slightly darker tab strip, near-black text and a deeper blue
    /// accent that keeps white text readable.
    pub const LIGHT: Self = Self {
        dark: false,

        panel: rgb(246, 246, 248),
        canvas: rgb(206, 206, 210),
        field: Color32::WHITE,
        titlebar: rgb(234, 234, 238),
        window: rgb(251, 251, 252),
        faint: rgb(239, 239, 242),
        ruler: rgb(236, 236, 239),
        preview: rgb(206, 206, 210),

        divider: rgb(218, 218, 223),
        border: rgb(184, 184, 191),
        header_rule: rgb(212, 212, 217),

        text: rgb(28, 28, 31),
        muted: rgb(84, 84, 92),
        disabled: rgb(168, 168, 175),
        error: rgb(192, 28, 40),
        error_soft: rgb(176, 36, 48),
        warning: rgb(150, 82, 0),

        accent: rgb(0, 97, 224),
        accent_gradient: [rgb(28, 120, 240), rgb(0, 92, 214)],
        accent_pressed: [rgb(0, 78, 186), rgb(0, 68, 166)],
        check: [rgb(24, 116, 238), rgb(0, 90, 212)],
        on_accent: Color32::WHITE,
        on_accent_text: Color32::WHITE,
        on_accent_muted: rgb(226, 236, 253),

        widget_fill: rgb(228, 228, 232),
        widget_weak_fill: rgb(236, 236, 240),
        widget_stroke: rgb(184, 184, 191),
        widget_hover_fill: rgb(218, 218, 223),
        widget_hover_weak_fill: rgb(226, 226, 231),
        widget_hover_stroke: rgb(150, 150, 158),
        widget_active_fill: rgb(206, 206, 212),
        widget_open_fill: rgb(222, 222, 227),

        control: [Color32::WHITE, rgb(244, 244, 247)],
        control_hover: [Color32::WHITE, rgb(236, 236, 241)],
        control_pressed: [rgb(226, 226, 231), rgb(216, 216, 222)],
        control_edge: black(46),
        control_shadow: black(22),
        checkbox: [Color32::WHITE, rgb(246, 246, 248)],
        checkbox_hover: [Color32::WHITE, rgb(236, 236, 241)],
        checkbox_edge: black(120),
        slider_rail: rgb(196, 196, 203),
        slider_rail_edge: Color32::TRANSPARENT,
        thumb: [Color32::WHITE, rgb(246, 246, 248)],
        thumb_pressed: rgb(226, 226, 231),
        thumb_edge: rgb(140, 140, 148),
        thumb_shadow: black(40),
        segment_track: [rgb(230, 230, 234), rgb(224, 224, 229)],
        segment_edge: rgb(196, 196, 203),
        segment_separator: rgb(196, 196, 203),

        window_shadow: black(56),
        popup_shadow: black(48),
        modal_backdrop: black(40),

        hover: black(16),
        pressed: black(30),
        row_hover: black(12),
        row_rule: black(16),
        row_selected: rgb(214, 228, 250),
        pane_hover: rgb(232, 232, 237),
        icon_hover: rgb(224, 224, 230),
        tool_selected: rgb(220, 220, 227),
        tool_hover: rgb(232, 232, 237),
        tool_selected_edge: rgb(184, 184, 191),
        thumbnail_edge: black(56),
        thumbnail_placeholder: rgb(222, 222, 227),
        close_dot: [rgb(178, 178, 185), rgb(120, 120, 128)],
        traffic_inactive: rgb(206, 206, 211),

        tab_fill: Color32::TRANSPARENT,
        tab_hover: black(16),
        tab_selected: Color32::WHITE,
        tab_edge: Color32::TRANSPARENT,
        tab_selected_edge: black(34),
        tab_close_hover: black(22),

        ruler_tick: rgb(118, 118, 126),
        ruler_label: rgb(64, 64, 70),
        ruler_edge: rgb(196, 196, 203),
        ruler_corner: black(90),

        checker: [Color32::WHITE, rgb(214, 214, 214)],
        canvas_shadow: black(36),
        canvas_edge: rgb(150, 150, 156),

        plot: Color32::WHITE,
        histogram_plot: Color32::WHITE,
        plot_grid: rgb(226, 226, 231),
        plot_diagonal: rgb(186, 186, 193),
        plot_line: rgb(28, 28, 31),
        plot_channels: [rgb(196, 36, 44), rgb(28, 132, 52), rgb(24, 92, 210)],
        histogram: [rgb(206, 52, 62), rgb(36, 148, 78), rgb(40, 108, 228)],
        levels_histogram: [
            rgb(112, 112, 120),
            rgb(196, 70, 66),
            rgb(58, 148, 70),
            rgb(58, 108, 200),
        ],
    };
}

impl Default for Palette {
    fn default() -> Self {
        Self::DARK
    }
}

fn palette_id() -> egui::Id {
    egui::Id::new("xuan.theme.palette")
}

/// The palette installed by [`apply`]; [`Palette::DARK`] before that.
///
/// It reads the context's memory, so don't call it inside a closure that holds the
/// context's lock for writing (`ctx.input_mut`, `ctx.data_mut`, `ctx.memory_mut`).
pub fn palette(ctx: &egui::Context) -> Palette {
    ctx.data(|data| data.get_temp::<Palette>(palette_id()))
        .unwrap_or_default()
}

/// `ui.palette()` / `ctx.palette()`.
pub trait PaletteExt {
    fn palette(&self) -> Palette;
}

impl PaletteExt for egui::Context {
    fn palette(&self) -> Palette {
        palette(self)
    }
}

impl PaletteExt for egui::Ui {
    fn palette(&self) -> Palette {
        palette(self.ctx())
    }
}

/// WCAG relative luminance of an opaque sRGB colour, 0 (black) to 1 (white).
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio between two opaque colours, 1 to 21.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

pub fn window_corner_radius(ctx: &egui::Context) -> u8 {
    let fills_screen = ctx.input(|i| {
        i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
    });
    if fills_screen { 0 } else { 12 }
}

/// Installs Xuan's fonts once, then [`set_palette`].
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    // Inter is an OFL-licensed, portable substitute for the macOS system font.
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/InterVariable.ttf")).into(),
    );
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, "Inter".into());
    fonts.font_data.insert(
        "Droid Sans Fallback".into(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/DroidSansFallbackFull.ttf"
        ))
        .into(),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("Droid Sans Fallback".into());
    }
    ctx.set_fonts(fonts);
    set_palette(ctx, palette);
}

/// Switches the colours: stores `palette` for [`palette`] and restyles egui's widgets.
pub fn set_palette(ctx: &egui::Context, palette: &Palette) {
    ctx.data_mut(|data| data.insert_temp(palette_id(), *palette));
    let mut style = (*ctx.style()).clone();
    style.visuals = visuals(palette);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 3.0);
    style.spacing.interact_size = egui::vec2(22.0, 22.0);
    style.spacing.icon_width = 14.0;
    style.spacing.icon_width_inner = 8.0;
    style.spacing.slider_width = 100.0;
    style.spacing.slider_rail_height = 3.0;
    style.spacing.menu_margin = egui::Margin::same(5);
    style.spacing.window_margin = egui::Margin::same(24);
    style.spacing.scroll.bar_width = 6.0;
    for (text, size) in [
        (TextStyle::Body, 12.0),
        (TextStyle::Button, 12.0),
        (TextStyle::Small, 11.0),
        (TextStyle::Heading, 21.0),
        (TextStyle::Monospace, 12.0),
    ] {
        style.text_styles.insert(text, FontId::proportional(size));
    }
    ctx.set_style(style);
}

/// egui's visuals for `p`: egui's dark or light defaults with Xuan's colours and shapes.
pub fn visuals(p: &Palette) -> egui::Visuals {
    let mut visuals = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.window;
    visuals.extreme_bg_color = p.field;
    visuals.text_edit_bg_color = Some(p.field);
    visuals.faint_bg_color = p.faint;
    visuals.override_text_color = Some(p.text);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(7);
    visuals.window_stroke = Stroke::new(1.0_f32, p.border);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 2,
        color: p.window_shadow,
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 1,
        color: p.popup_shadow,
    };
    visuals.selection.bg_fill = p.accent.gamma_multiply(0.65);
    visuals.selection.stroke = Stroke::new(1.0_f32, p.text);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.divider);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(BUTTON_RADIUS);
        widget.fg_stroke = Stroke::new(1.0_f32, p.text);
        widget.expansion = 0.0;
    }
    visuals.widgets.inactive.bg_fill = p.widget_fill;
    visuals.widgets.inactive.weak_bg_fill = p.widget_weak_fill;
    visuals.widgets.inactive.bg_stroke = Stroke::new(0.7_f32, p.widget_stroke);
    visuals.widgets.hovered.bg_fill = p.widget_hover_fill;
    visuals.widgets.hovered.weak_bg_fill = p.widget_hover_weak_fill;
    visuals.widgets.hovered.bg_stroke = Stroke::new(0.7_f32, p.widget_hover_stroke);
    visuals.widgets.active.bg_fill = p.widget_active_fill;
    visuals.widgets.active.weak_bg_fill = p.widget_active_fill;
    visuals.widgets.open.weak_bg_fill = p.widget_open_fill;
    visuals
}

pub fn frame(palette: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.panel)
        .inner_margin(egui::Margin::symmetric(18, 10))
}

/// Menu-bar menus: tighter rows, highlighted in the accent colour.
pub fn menu_style(style: &mut egui::Style, palette: &Palette) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = egui::vec2(9.0, 3.0);
    style.spacing.item_spacing.y = 2.0;
    for widget in [
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(4);
        widget.weak_bg_fill = palette.accent;
        widget.bg_fill = palette.accent;
        widget.fg_stroke.color = palette.on_accent_text;
    }
    // Let each row take its state's text colour, so a highlighted row's text sits on the
    // accent in `on_accent_text`.
    style.visuals.override_text_color = None;
}

/// Light or dark, as Settings → Appearance → Theme asks. `system_dark` is the desktop's
/// preference, if known; without one Xuan stays dark.
pub fn palette_for(choice: xuan::config::Theme, system_dark: Option<bool>) -> Palette {
    let dark = match choice {
        xuan::config::Theme::System => system_dark.unwrap_or(true),
        xuan::config::Theme::Light => false,
        xuan::config::Theme::Dark => true,
    };
    if dark { Palette::DARK } else { Palette::LIGHT }
}

#[cfg(test)]
mod tests;

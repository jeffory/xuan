use super::*;

/// The colours `theme.rs` defined as constants before the palette existed.
mod legacy {
    use egui::Color32;
    pub const PANEL: Color32 = Color32::from_rgb(36, 36, 36);
    pub const CANVAS: Color32 = Color32::from_rgb(29, 29, 29);
    pub const FIELD: Color32 = Color32::from_rgb(30, 30, 30);
    pub const DIVIDER: Color32 = Color32::from_rgb(54, 54, 54);
    pub const MUTED: Color32 = Color32::from_rgb(154, 154, 157);
    pub const TEXT: Color32 = Color32::from_rgb(235, 235, 237);
    pub const ACCENT: Color32 = Color32::from_rgb(10, 132, 255);
    pub const TITLEBAR: Color32 = Color32::from_rgb(45, 45, 45);
}

/// egui's visuals exactly as the dark-only `theme::apply` set them up.
fn legacy_visuals() -> egui::Visuals {
    use legacy::*;
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_gray(43);
    v.extreme_bg_color = FIELD;
    v.text_edit_bg_color = Some(FIELD);
    v.faint_bg_color = Color32::from_gray(40);
    v.override_text_color = Some(TEXT);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(7);
    v.window_stroke = Stroke::new(1.0_f32, Color32::from_gray(83));
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 2,
        color: Color32::from_black_alpha(125),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 1,
        color: Color32::from_black_alpha(110),
    };
    v.selection.bg_fill = ACCENT.gamma_multiply(0.65);
    v.selection.stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, DIVIDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    for widget in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(BUTTON_RADIUS);
        widget.fg_stroke = Stroke::new(1.0_f32, TEXT);
        widget.expansion = 0.0;
    }
    v.widgets.inactive.bg_fill = Color32::from_gray(72);
    v.widgets.inactive.weak_bg_fill = Color32::from_gray(62);
    v.widgets.inactive.bg_stroke = Stroke::new(0.7_f32, Color32::from_gray(91));
    v.widgets.hovered.bg_fill = Color32::from_gray(88);
    v.widgets.hovered.weak_bg_fill = Color32::from_gray(78);
    v.widgets.hovered.bg_stroke = Stroke::new(0.7_f32, Color32::from_gray(112));
    v.widgets.active.bg_fill = Color32::from_gray(52);
    v.widgets.active.weak_bg_fill = Color32::from_gray(52);
    v.widgets.open.weak_bg_fill = Color32::from_gray(68);
    v
}

#[test]
fn dark_palette_matches_the_old_constants() {
    let p = Palette::DARK;
    assert!(p.dark);
    assert_eq!(p.panel, legacy::PANEL);
    assert_eq!(p.canvas, legacy::CANVAS);
    assert_eq!(p.field, legacy::FIELD);
    assert_eq!(p.divider, legacy::DIVIDER);
    assert_eq!(p.muted, legacy::MUTED);
    assert_eq!(p.text, legacy::TEXT);
    assert_eq!(p.accent, legacy::ACCENT);
    assert_eq!(p.titlebar, legacy::TITLEBAR);
    assert_eq!(ON_CANVAS, legacy::TEXT);
    // The const helper builds the same premultiplied colour as egui's.
    assert_eq!(white(28), Color32::from_white_alpha(28));
    assert_eq!(Palette::default(), Palette::DARK);
}

#[test]
fn dark_visuals_match_the_old_style() {
    assert_eq!(visuals(&Palette::DARK), legacy_visuals());
}

#[test]
fn palette_reads_back_what_apply_installed() {
    let ctx = egui::Context::default();
    assert_eq!(ctx.palette(), Palette::DARK, "dark before apply");
    let mut custom = Palette::DARK;
    custom.accent = Color32::from_rgb(200, 30, 90);
    set_palette(&ctx, &custom);
    assert_eq!(palette(&ctx), custom);
    assert_eq!(ctx.style().visuals.panel_fill, custom.panel);
    assert_eq!(
        ctx.style().visuals.selection.bg_fill,
        custom.accent.gamma_multiply(0.65)
    );
}

/// `fg` over `bg`, with `fg` blended first when it is translucent.
fn ratio(fg: Color32, bg: Color32) -> f32 {
    contrast_ratio(bg.blend(fg), bg)
}

#[test]
fn text_meets_wcag_aa_in_both_palettes() {
    for p in [Palette::DARK, Palette::LIGHT] {
        let name = if p.dark { "dark" } else { "light" };
        // Body text and secondary text: 4.5:1.
        for (label, fg, bg) in [
            ("text/panel", p.text, p.panel),
            ("text/field", p.text, p.field),
            ("text/titlebar", p.text, p.titlebar),
            ("text/window", p.text, p.window),
            ("text/faint", p.text, p.faint),
            ("text/selected row", p.text, p.row_selected),
            ("muted/panel", p.muted, p.panel),
            ("muted/titlebar", p.muted, p.titlebar),
            ("muted/window", p.muted, p.window),
            ("muted/field", p.muted, p.field),
            ("muted/canvas", p.muted, p.canvas),
            ("error/panel", p.error, p.panel),
            ("error/window", p.error, p.window),
            ("error_soft/window", p.error_soft, p.window),
            ("warning/window", p.warning, p.window),
            ("ruler label/ruler", p.ruler_label, p.ruler),
        ] {
            let r = ratio(fg, bg);
            assert!(r >= 4.5, "{name} {label}: {r:.2}");
        }
        // Controls, plot lines and the accent against surfaces: 3:1.
        for (label, fg, bg) in [
            ("accent/panel", p.accent, p.panel),
            ("accent/window", p.accent, p.window),
            ("plot line/plot", p.plot_line, p.plot),
            ("ruler tick/ruler", p.ruler_tick, p.ruler),
            ("on accent/accent", p.on_accent, p.accent),
            (
                "text/selected tab",
                p.text,
                p.titlebar.blend(p.tab_selected),
            ),
        ] {
            let r = ratio(fg, bg);
            assert!(r >= 3.0, "{name} {label}: {r:.2}");
        }
    }
    // Highlighted menu rows. The dark palette keeps macOS's light-on-blue rows (about 3:1);
    // the light palette's white on a deeper blue reaches 4.5:1.
    let light = Palette::LIGHT;
    assert!(ratio(light.on_accent_text, light.accent) >= 4.5);
    assert!(ratio(light.on_accent_muted, light.accent) >= 4.5);
    assert!(ratio(Palette::DARK.on_accent_text, Palette::DARK.accent) >= 3.0);
}

#[test]
fn light_palette_uses_light_visuals() {
    let v = visuals(&Palette::LIGHT);
    assert!(!v.dark_mode);
    assert_eq!(v.panel_fill, Palette::LIGHT.panel);
    assert_eq!(v.override_text_color, Some(Palette::LIGHT.text));
    assert!(visuals(&Palette::DARK).dark_mode);
}

#[test]
fn the_theme_setting_picks_the_palette() {
    use xuan::config::Theme;
    assert_eq!(palette_for(Theme::Dark, Some(false), None), Palette::DARK);
    assert_eq!(palette_for(Theme::Light, Some(true), None), Palette::LIGHT);
    assert_eq!(
        palette_for(Theme::System, Some(false), None),
        Palette::LIGHT
    );
    assert_eq!(palette_for(Theme::System, Some(true), None), Palette::DARK);
    // Without an answer from the desktop, Xuan stays dark.
    assert_eq!(palette_for(Theme::System, None, None), Palette::DARK);
}

#[test]
fn a_system_accent_replaces_the_blue_and_stays_readable() {
    let accents = [
        Color32::from_rgb(0x35, 0x84, 0xe4), // GNOME blue
        Color32::from_rgb(0xc8, 0x88, 0x00), // GNOME yellow
        Color32::from_rgb(0x3a, 0x94, 0x4a), // GNOME green
        Color32::from_rgb(61, 174, 233),     // Breeze
        Color32::from_rgb(233, 84, 32),      // Ubuntu orange
        Color32::from_rgb(0, 120, 215),      // Windows blue
        Color32::from_rgb(255, 255, 0),
        Color32::from_rgb(20, 20, 60),
        Color32::WHITE,
        Color32::BLACK,
    ];
    for base in [Palette::DARK, Palette::LIGHT] {
        for accent in accents {
            let p = base.with_accent(accent);
            let text = if p.dark { 3.0 } else { 4.5 };
            assert!(
                ratio(p.on_accent_text, p.accent) >= text,
                "{accent:?} on {}",
                p.dark
            );
            assert!(ratio(p.accent, p.panel) >= 3.0, "{accent:?} on {}", p.dark);
            assert!(ratio(p.text, p.row_selected) >= 4.5);
            assert_ne!(p.accent_gradient, base.accent_gradient);
        }
    }
    // A colour that already fits is kept as it is.
    let windows_blue = Color32::from_rgb(0, 103, 192);
    assert_eq!(
        Palette::LIGHT.with_accent(windows_blue).accent,
        windows_blue
    );
    let breeze = Palette::DARK.with_accent(Color32::from_rgb(61, 174, 233));
    assert_ne!(breeze.accent, Palette::DARK.accent);
    assert_eq!(
        palette_for(xuan::config::Theme::Dark, None, Some(windows_blue)),
        Palette::DARK.with_accent(windows_blue)
    );
}

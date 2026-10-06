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

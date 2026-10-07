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

/// egui's visuals as the dark-only `theme::apply` set them up, with the changes made on
/// purpose since: no `override_text_color` and themed weak, error and warning text (#78, #77),
/// and widget outlines that reach 3:1 against the panel (#77).
fn legacy_visuals() -> egui::Visuals {
    use legacy::*;
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_gray(43);
    v.extreme_bg_color = FIELD;
    v.text_edit_bg_color = Some(FIELD);
    v.faint_bg_color = Color32::from_gray(40);
    v.weak_text_color = Some(MUTED);
    v.error_fg_color = Color32::from_rgb(255, 128, 128);
    v.warn_fg_color = Color32::from_rgb(255, 170, 60);
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
    v.widgets.inactive.bg_stroke = Stroke::new(0.7_f32, Color32::from_gray(122));
    v.widgets.hovered.bg_fill = Color32::from_gray(88);
    v.widgets.hovered.weak_bg_fill = Color32::from_gray(78);
    v.widgets.hovered.bg_stroke = Stroke::new(0.7_f32, Color32::from_gray(150));
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
            ("check mark/check top", p.on_accent, p.check[0]),
            ("check mark/check bottom", p.on_accent, p.check[1]),
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
}

/// `fg` (translucent or not) painted over `under`, compared with the surface `bg` round the
/// control: how well a control's outline separates it from what it sits on.
fn edge_ratio(fg: Color32, under: Color32, bg: Color32) -> f32 {
    contrast_ratio(under.blend(fg), bg)
}

/// Every pair from the contrast review (#77), with the ratio it needs: 4.5:1 for text, 3:1 for
/// the outlines that show where a control is. Outlines are checked against both surfaces a
/// control sits on, the panel and the dialog window.
fn contrast_pairs(p: &Palette) -> Vec<(String, f32, f32)> {
    let v = visuals(p);
    let mut pairs: Vec<(String, f32, f32)> = [
        // Default buttons (Done, Apply, OK, Develop): the label on every shade of the
        // gradient, the lightest stop included, and while pressed.
        (
            "primary label/gradient top",
            ratio(p.on_accent_text, p.accent_gradient[0]),
        ),
        (
            "primary label/gradient bottom",
            ratio(p.on_accent_text, p.accent_gradient[1]),
        ),
        (
            "primary label/pressed top",
            ratio(p.on_accent_text, p.accent_pressed[0]),
        ),
        (
            "primary label/pressed bottom",
            ratio(p.on_accent_text, p.accent_pressed[1]),
        ),
        // Highlighted menu rows: `menus.rs` item buttons and `widgets::menu_check`.
        (
            "menu text/highlight",
            ratio(p.on_accent_text, p.accent_fill),
        ),
        (
            "menu shortcut hint/highlight",
            ratio(p.on_accent_muted, p.accent_fill),
        ),
        ("menu shortcut hint/window", ratio(p.muted, p.window)),
        // Error text in dialogs (Paths, plugin install), and egui's own error and warning
        // colours, which come from the palette.
        ("error/window", ratio(p.error, p.window)),
        ("error/panel", ratio(p.error, p.panel)),
        (
            "egui error_fg_color/window",
            ratio(v.error_fg_color, p.window),
        ),
        (
            "egui warn_fg_color/window",
            ratio(v.warn_fg_color, p.window),
        ),
        // The row being edited in a list (Layer Effects, Settings, Plugins): selection fill under
        // the selected row's text, on the dialog window and on a panel (#87).
        (
            "selected row text/window",
            ratio(
                v.selection.stroke.color,
                p.window.blend(v.selection.bg_fill),
            ),
        ),
        (
            "selected row text/panel",
            ratio(v.selection.stroke.color, p.panel.blend(v.selection.bg_fill)),
        ),
        // Hints and weak text (#78).
        ("egui weak text/field", ratio(v.weak_text_color(), p.field)),
    ]
    .into_iter()
    .map(|(label, r)| (label.to_owned(), r, 4.5))
    .collect();
    for (surface, bg) in [("panel", p.panel), ("window", p.window)] {
        for (label, r) in [
            (
                "secondary button edge (top)",
                edge_ratio(p.control_edge, p.control[0], bg),
            ),
            (
                "secondary button edge (bottom)",
                edge_ratio(p.control_edge, p.control[1], bg),
            ),
            (
                "text field border",
                ratio(v.widgets.inactive.bg_stroke.color, bg),
            ),
            ("number field border", ratio(p.widget_stroke, bg)),
            ("slider rail", ratio(p.slider_rail, bg)),
            ("keyboard focus ring", ratio(p.accent, bg)),
        ] {
            pairs.push((format!("{label}/{surface}"), r, 3.0));
        }
    }
    pairs
}

#[test]
fn every_reviewed_pair_meets_its_contrast_in_both_palettes() {
    let mut failures = Vec::new();
    for p in [Palette::DARK, Palette::LIGHT] {
        let name = if p.dark { "dark" } else { "light" };
        for (label, r, needs) in contrast_pairs(&p) {
            if r < needs {
                failures.push(format!("{name} {label}: {r:.2} < {needs}"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The colours a `TextEdit`'s hint and a plain label are painted in once the palette is
/// installed, as their text shapes resolve them.
fn painted_text_colors(p: &Palette) -> (Color32, Color32) {
    fn find(shape: &egui::Shape, needle: &str) -> Option<Color32> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == needle => {
                let section = t.galley.job.sections.first()?.format.color;
                let color = if section == Color32::PLACEHOLDER {
                    t.fallback_color
                } else {
                    section
                };
                Some(t.override_text_color.unwrap_or(color))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, needle)),
            _ => None,
        }
    }
    let ctx = egui::Context::default();
    set_palette(&ctx, p);
    let mut text = String::new();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Body label");
            ui.add(egui::TextEdit::singleline(&mut text).hint_text("Type a command…"));
        });
    });
    let color = |needle: &str| {
        output
            .shapes
            .iter()
            .find_map(|clipped| find(&clipped.shape, needle))
            .unwrap_or_else(|| panic!("no text shape for {needle:?}"))
    };
    (color("Type a command…"), color("Body label"))
}

#[test]
fn text_edit_hints_are_muted_and_labels_keep_the_text_colour() {
    for p in [Palette::DARK, Palette::LIGHT] {
        let name = if p.dark { "dark" } else { "light" };
        let (hint, label) = painted_text_colors(&p);
        assert_eq!(label, p.text, "{name}: a plain label keeps the text colour");
        assert_ne!(
            hint, p.text,
            "{name}: the hint must not look like typed text"
        );
        assert_eq!(hint, p.muted, "{name}: the hint is muted");
        let r = ratio(hint, p.field);
        assert!(r >= 4.5, "{name} hint/field: {r:.2}");
        // Visibly weaker than typed text on the same field.
        let text = ratio(p.text, p.field);
        assert!(
            text / r > 1.5,
            "{name}: hint {r:.2} too close to text {text:.2}"
        );
    }
}

#[test]
fn light_palette_uses_light_visuals() {
    let v = visuals(&Palette::LIGHT);
    assert!(!v.dark_mode);
    assert_eq!(v.panel_fill, Palette::LIGHT.panel);
    assert_eq!(v.override_text_color, None);
    assert_eq!(v.weak_text_color, Some(Palette::LIGHT.muted));
    assert_eq!(v.error_fg_color, Palette::LIGHT.error);
    assert_eq!(v.warn_fg_color, Palette::LIGHT.warning);
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
    // Every reviewed pair holds whatever accent the desktop asks for, Xuan's own included.
    let mut failures = Vec::new();
    for base in [Palette::DARK, Palette::LIGHT] {
        let name = if base.dark { "dark" } else { "light" };
        for accent in accents.into_iter().chain([base.accent]) {
            let p = base.with_accent(accent);
            let mut pairs = contrast_pairs(&p);
            pairs.extend([
                (
                    "text/selected row".into(),
                    ratio(p.text, p.row_selected),
                    4.5,
                ),
                ("check mark/top".into(), ratio(p.on_accent, p.check[0]), 3.0),
                (
                    "check mark/bottom".into(),
                    ratio(p.on_accent, p.check[1]),
                    3.0,
                ),
            ]);
            for (label, r, needs) in pairs {
                if r < needs {
                    failures.push(format!("{name} {accent:?} {label}: {r:.2} < {needs}"));
                }
            }
            if accent != base.accent {
                assert_ne!(p.accent_gradient, base.accent_gradient, "{accent:?}");
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    // A light accent keeps its colour and takes dark text, rather than turning muddy.
    let yellow = Color32::from_rgb(255, 255, 0);
    let p = Palette::DARK.with_accent(yellow);
    assert_eq!(p.accent_fill, yellow);
    assert!(luminance(p.on_accent_text) < 0.1);
    // Xuan's blue keeps light text.
    assert_eq!(
        Palette::LIGHT
            .with_accent(Palette::LIGHT.accent)
            .on_accent_text,
        Color32::WHITE
    );
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

#[test]
fn the_subheading_style_sits_between_body_and_heading() {
    let ctx = egui::Context::default();
    apply(&ctx, &Palette::DARK);
    let style = ctx.style();
    let size = |text: &TextStyle| style.text_styles[text].size;
    let sub = size(&subheading_style());
    assert!(
        size(&TextStyle::Body) < sub && sub < size(&TextStyle::Heading),
        "{sub}"
    );
}

#[test]
fn the_monospace_style_resolves_to_a_monospace_family() {
    let ctx = egui::Context::default();
    apply(&ctx, &Palette::LIGHT);
    let font = ctx.style().text_styles[&TextStyle::Monospace].clone();
    assert_eq!(font.family, egui::FontFamily::Monospace);
    // Narrow and wide letters take the same width: the font is Hack, not Inter.
    let mut widths = (0.0, 0.0);
    let _ = ctx.run(Default::default(), |ctx| {
        widths = ctx.fonts_mut(|f| (f.glyph_width(&font, 'i'), f.glyph_width(&font, 'W')));
    });
    assert!(widths.0 > 0.0);
    assert_eq!(widths.0, widths.1);
}

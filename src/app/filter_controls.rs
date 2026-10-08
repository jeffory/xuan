//! Settings of the filters ported from upstream Compositor (Vignette, Bloom / Glow, Tonal
//! Contrast and Dither), with upstream's ranges, in the Filter dialog.
use egui::Ui;
use xuan::effects::{DitherColors, DitherPixelShape, DitherSettings, DitherStyle, Filter};
use xuan::i18n::tr;

use super::widgets;

fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    suffix: &str,
) -> bool {
    let centered = *range.start() < 0.0;
    let mut slider = widgets::Slider::new(value, range)
        .text(tr(label))
        .suffix(suffix)
        .max_decimals(0);
    if centered {
        slider = slider.centered();
    }
    ui.add(slider).changed()
}

/// A color well for an RGB setting.
fn color(ui: &mut Ui, label: &str, rgb: &mut [u8; 3]) -> bool {
    let mut rgba = [rgb[0], rgb[1], rgb[2], 255];
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tr(label));
        changed = widgets::color_well(ui, &mut rgba).changed();
    });
    if changed {
        *rgb = [rgba[0], rgba[1], rgba[2]];
    }
    changed
}

/// Draws the settings of `filter` if it is one of upstream's; returns whether one changed.
pub(super) fn show(ui: &mut Ui, filter: &mut Filter) -> bool {
    let mut changed = false;
    match filter {
        Filter::Vignette {
            amount,
            color: edge,
            midpoint,
            roundness,
            feather,
            highlights,
        } => {
            changed |= slider(ui, amount, 0.0..=100.0, "Amount", "%");
            changed |= slider(ui, midpoint, 0.0..=100.0, "Midpoint", "");
            changed |= slider(ui, roundness, -100.0..=100.0, "Roundness", "");
            changed |= slider(ui, feather, 0.0..=100.0, "Feather", "");
            changed |= slider(ui, highlights, 0.0..=100.0, "Highlights", "");
            changed |= color(ui, "Colour", edge);
        }
        Filter::Bloom { amount, radius } => {
            changed |= slider(ui, amount, 0.0..=100.0, "Amount", "%");
            changed |= slider(ui, radius, 1.0..=150.0, "Radius", " px");
        }
        Filter::TonalContrast {
            amount,
            radius,
            shadows,
            midtones,
            highlights,
        } => {
            changed |= slider(ui, amount, 0.0..=100.0, "Amount", "%");
            changed |= slider(ui, radius, 1.0..=100.0, "Radius", " px");
            changed |= slider(ui, shadows, -100.0..=100.0, "Shadows", "");
            changed |= slider(ui, midtones, -100.0..=100.0, "Midtones", "");
            changed |= slider(ui, highlights, -100.0..=100.0, "Highlights", "");
        }
        Filter::Dither(settings) => changed |= dither(ui, settings),
        _ => {}
    }
    changed
}

fn dither(ui: &mut Ui, settings: &mut DitherSettings) -> bool {
    let before = settings.clone();
    ui.horizontal(|ui| {
        ui.label(tr("Style"));
        widgets::PopUp::from_id_salt("dither_style")
            .selected_text(tr(settings.style.name()))
            .width(180.0)
            .show_tall_ui(ui, |ui| {
                // Grouped as upstream's menu: diffusion, ordered, halftone, the rest.
                for (index, style) in DitherStyle::ALL.into_iter().enumerate() {
                    if [2, 5, 8].contains(&index) {
                        ui.separator();
                    }
                    widgets::menu_choice(ui, &mut settings.style, style, tr(style.name()));
                }
            });
    });
    let style = settings.style;
    if style.uses_pixel_size() {
        slider(
            ui,
            &mut settings.pixel_size,
            1.0..=32.0,
            "Pixel Size",
            " px",
        );
        if settings.pixel_size > 1.0 {
            widgets::segmented(
                ui,
                &mut settings.pixel_shape,
                &[
                    (DitherPixelShape::Square, tr("Square")),
                    (DitherPixelShape::Dot, tr("Dot")),
                ],
            );
        }
    }
    if style.has_tones() {
        slider(ui, &mut settings.levels, 2.0..=8.0, "Levels", "");
    }
    if style.diffuses() {
        slider(ui, &mut settings.diffusion, 0.0..=100.0, "Diffusion", "%");
    }
    if style.is_halftone() {
        slider(ui, &mut settings.cell_size, 4.0..=64.0, "Cell Size", "");
        slider(ui, &mut settings.angle, -90.0..=90.0, "Angle", "°");
    }
    if style == DitherStyle::Ascii {
        slider(ui, &mut settings.text_size, 6.0..=64.0, "Text Size", " px");
        ui.horizontal(|ui| {
            ui.label(tr("Characters"));
            ui.add(
                egui::TextEdit::singleline(&mut settings.characters)
                    .char_limit(64)
                    .desired_width(160.0),
            );
        });
    }
    if style == DitherStyle::Scanlines {
        slider(
            ui,
            &mut settings.line_spacing,
            2.0..=32.0,
            "Line Spacing",
            " px",
        );
        slider(ui, &mut settings.glow, 0.0..=100.0, "Glow", "%");
        slider(ui, &mut settings.dots, 0.0..=100.0, "Dots", "%");
        slider(ui, &mut settings.wobble, 0.0..=64.0, "Wobble", " px");
    }
    slider(ui, &mut settings.density, -100.0..=100.0, "Density", "");
    slider(ui, &mut settings.contrast, -100.0..=100.0, "Contrast", "");
    widgets::segmented(
        ui,
        &mut settings.colors,
        &[
            (DitherColors::BlackWhite, tr("Black & White")),
            (DitherColors::TwoColors, tr("Two Colours")),
            (DitherColors::Original, tr("Original")),
        ],
    );
    if settings.colors == DitherColors::TwoColors {
        color(ui, "Dark colour", &mut settings.dark);
        color(ui, "Light colour", &mut settings.light);
    }
    if style.draws_marks() {
        widgets::checkbox(ui, &mut settings.light_on_dark, tr("Light on Dark"));
    }
    // Whole numbers, as upstream keeps them.
    for value in [
        &mut settings.pixel_size,
        &mut settings.cell_size,
        &mut settings.text_size,
        &mut settings.line_spacing,
        &mut settings.levels,
    ] {
        *value = value.round();
    }
    settings.characters.retain(|c| c != '\n' && c != '\r');
    *settings != before
}

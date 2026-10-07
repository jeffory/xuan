use super::theme::PaletteExt as _;
use super::widgets;
use egui::RichText;
use xuan::i18n::tr;
use xuan::{
    paint::{PaintMode, ShapeKind},
    retouch::HealMode,
    selection::SelectionMode,
};

use super::eyedropper::{SampleSize, SampleSource};
use super::{EditorApp, Tool, icons, theme};

const DEFAULT_VALUE_WIDTH: f32 = 74.0;
const DEFAULT_VALUE_HEIGHT: f32 = 22.0;
const DEFAULT_PERCENT_VALUE_WIDTH: f32 = 50.0;

/// Draw a toolbar field with optional `width = ...` and `height = ...` overrides.
macro_rules! value {
    (@dimension $default:expr) => { $default };
    (@dimension $default:expr, $custom:expr) => { $custom };
    (
        $ui:expr, $label:expr, $number:expr, $range:expr, $unit:expr
        $(, width = $width:expr)?
        $(, height = $height:expr)?
        $(,)?
    ) => {{
        let ui = &mut *($ui);
        ui.label(RichText::new($label).color(ui.palette().muted));
        ui.add(
            widgets::Number::new($number)
                .size(egui::vec2(
                    value!(@dimension DEFAULT_VALUE_WIDTH $(, $width)?),
                    value!(@dimension DEFAULT_VALUE_HEIGHT $(, $height)?),
                ))
                .speed(1.0)
                .range($range)
                .suffix($unit)
                .max_decimals(1),
        )
        .changed()
    }};
}

impl EditorApp {
    pub(super) fn tool_options(&mut self, ctx: &egui::Context) {
        let mut transform = self
            .session()
            .and_then(|s| xuan::operations::transform_box(&s.document, self.transforming_mask()));
        let mut changed = false;
        egui::TopBottomPanel::top("tool_options")
            .min_height(42.0)
            .frame(theme::frame(&ctx.palette()))
            .show(ctx, |ui| {
                ui.add_enabled_ui(self.dialog.is_none() && self.job.is_none() && self.color_range.is_none(), |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("options_scroll")
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(if self.transforming_mask() {
                                        tr("Mask")
                                    } else {
                                        self.tool.label()
                                    })
                                    .strong()
                                    .size(13.0),
                                );
                                ui.add_space(8.0);
                                match self.tool {
                                    Tool::Move => {
                                        widgets::checkbox(ui, &mut self.auto_select, tr("Auto Select"));
                                        widgets::checkbox(
                                            ui,
                                            &mut self.ignore_transparent_pixels,
                                            tr("Ignore Transparent Pixels"),
                                        )
                                        .on_hover_text(
                                            tr("Select layers only at visible pixels. Uncheck to select anywhere inside a layer's bounds."),
                                        );
                                        widgets::checkbox(
                                            ui,
                                            &mut self.show_controls,
                                            tr("Show Controls"),
                                        );
                                        ui.separator();
                                        if let Some(t) = &mut transform {
                                            changed |= value!(
                                                ui,
                                                "X",
                                                &mut t.x,
                                                -1_000_000.0..=1_000_000.0,
                                                " px",
                                            );
                                            changed |= value!(
                                                ui,
                                                "Y",
                                                &mut t.y,
                                                -1_000_000.0..=1_000_000.0,
                                                " px",
                                            );
                                            let old = *t;
                                            if value!(ui, "W", &mut t.width, 1.0..=300_000.0, " px") {
                                                if self.lock_ratio {
                                                    t.height *= t.width / old.width;
                                                }
                                                changed = true;
                                            }
                                            if value!(ui, "H", &mut t.height, 1.0..=300_000.0, " px") {
                                                if self.lock_ratio {
                                                    t.width *= t.height / old.height;
                                                }
                                                changed = true;
                                            }
                                            widgets::checkbox(ui, &mut self.lock_ratio, tr("Link"));
                                            changed |= value!(
                                                ui,
                                                tr("Angle"),
                                                &mut t.rotation,
                                                -360.0..=360.0,
                                                "°",
                                                width = DEFAULT_PERCENT_VALUE_WIDTH,
                                            );
                                        } else {
                                            ui.label(
                                                RichText::new(tr("Select a layer to transform"))
                                                    .color(ui.palette().muted),
                                            );
                                        }
                                    }
                                    tool if tool.is_brush() => {
                                        if matches!(self.tool, Tool::Brush | Tool::Erase) {
                                            let mut brush_tool = self.tool;
                                            if widgets::segmented(
                                                ui,
                                                &mut brush_tool,
                                                &[(Tool::Brush, tr("Paint")), (Tool::Erase, tr("Erase"))],
                                            )
                                            .changed()
                                            {
                                                self.set_tool(brush_tool);
                                            }
                                        }
                                        if self.tool == Tool::Blur {
                                            widgets::segmented(
                                                ui,
                                                &mut self.blur_mode,
                                                &[
                                                    (PaintMode::Blur, tr("Blur")),
                                                    (PaintMode::Smudge, tr("Smudge")),
                                                ],
                                            );
                                        }
                                        if self.tool == Tool::Heal {
                                            widgets::segmented(
                                                ui,
                                                &mut self.heal_mode,
                                                &[
                                                    (HealMode::ContentAware, tr("Content-Aware")),
                                                    (HealMode::CreateTexture, tr("Create Texture")),
                                                    (HealMode::ProximityMatch, tr("Proximity Match")),
                                                ],
                                            );
                                        }
                                        if self.tool == Tool::Clone {
                                            widgets::checkbox(
                                                ui,
                                                &mut self.clone_aligned,
                                                tr("Aligned"),
                                            );
                                            widgets::segmented(
                                                ui,
                                                &mut self.clone_all,
                                                &[(false, tr("This Layer")), (true, tr("All Layers"))],
                                            );
                                        }
                                        value!(ui, tr("Size"), &mut self.brush.diameter, 1.0..=2000.0, " px", width = 62.0);
                                        if self.tool == Tool::Pencil {
                                            ui.label(tr("Tip"));
                                            widgets::segmented(
                                                ui,
                                                &mut self.brush.square,
                                                &[(false, tr("Round")), (true, tr("Square"))],
                                            );
                                        } else {
                                            ui.label(tr("Hardness"));
                                            ui.add(
                                                widgets::Slider::new(
                                                    &mut self.brush.hardness,
                                                    0.0..=1.0,
                                                )
                                                .value_width(DEFAULT_PERCENT_VALUE_WIDTH)
                                                .percentage(),
                                            );
                                        }
                                        ui.label(tr("Opacity"));
                                        ui.add(
                                            widgets::Slider::new(
                                                &mut self.brush.opacity,
                                                0.01..=1.0,
                                            )
                                            .value_width(DEFAULT_PERCENT_VALUE_WIDTH)
                                            .percentage(),
                                        );
                                        widgets::color_well(ui, &mut self.brush.color);
                                        ui.menu_button(tr("Pen dynamics"), |ui| {
                                            widgets::checkbox(ui, &mut self.pressure_size, tr("Pressure: size"));
                                            widgets::checkbox(ui, &mut self.pressure_opacity, tr("Pressure: opacity"));
                                            widgets::checkbox(ui, &mut self.tilt_shape, tr("Tilt: shape"));
                                        });
                                        if matches!(self.tool, Tool::Brush | Tool::Pencil | Tool::Erase) {
                                            ui.menu_button(tr("Brush dynamics"), |ui| self.brush_dynamics(ui));
                                            ui.menu_button(tr("Symmetry"), |ui| self.brush_symmetry(ui));
                                        }
                                        ui.label(tr("Smoothing"));
                                        ui.add(
                                            widgets::Slider::new(&mut self.brush_smoothing, 0.0..=1.0)
                                                .value_width(DEFAULT_PERCENT_VALUE_WIDTH)
                                                .percentage(),
                                        )
                                        .on_hover_text(
                                            tr("Reduce hand jitter. Higher values make the brush follow farther behind the pointer. 0% turns smoothing off."),
                                        );
                                    }
                                    tool if tool.is_selection() => {
                                        widgets::segmented(
                                            ui,
                                            &mut self.selection_mode,
                                            &[
                                                (SelectionMode::Replace, "New"),
                                                (SelectionMode::Add, tr("Add")),
                                                (SelectionMode::Subtract, tr("Subtract")),
                                                (SelectionMode::Intersect, tr("Intersect")),
                                            ],
                                        );
                                        ui.separator();
                                        match self.tool {
                                            Tool::Marquee => {
                                                widgets::segmented(
                                                    ui,
                                                    &mut self.ellipse,
                                                    &[(false, tr("Rectangle")), (true, tr("Ellipse"))],
                                                );
                                            }
                                            Tool::Lasso => {
                                                widgets::segmented(
                                                    ui,
                                                    &mut self.polygonal,
                                                    &[(false, tr("Freehand")), (true, tr("Polygonal"))],
                                                );
                                            }
                                            Tool::Wand => {
                                                widgets::segmented(
                                                    ui,
                                                    &mut self.wand_object,
                                                    &[(false, tr("Wand")), (true, tr("Object"))],
                                                );
                                                if self.wand_object {
                                                    ui.label(
                                                        egui::RichText::new(tr(
                                                            "Click an object or drag a box around it",
                                                        ))
                                                        .color(ui.palette().muted),
                                                    );
                                                } else {
                                                    ui.label(tr("Tolerance"));
                                                    ui.add(
                                                        widgets::Number::new(&mut self.tolerance)
                                                            .size(egui::vec2(DEFAULT_PERCENT_VALUE_WIDTH, DEFAULT_VALUE_HEIGHT))
                                                            .range(0..=255),
                                                    );
                                                    widgets::checkbox(
                                                        ui,
                                                        &mut self.contiguous,
                                                        tr("Contiguous"),
                                                    );
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    Tool::Gradient => {
                                        widgets::segmented(
                                            ui,
                                            &mut self.radial,
                                            &[(false, tr("Linear")), (true, tr("Radial"))],
                                        );
                                        ui.separator();
                                        widgets::color_well(ui, &mut self.brush.color);
                                        ui.label("→");
                                        widgets::color_well(ui, &mut self.background);
                                        ui.label(tr("Opacity"));
                                        ui.add(
                                            widgets::Slider::new(
                                                &mut self.brush.opacity,
                                                0.0..=1.0,
                                            )
                                            .value_width(50.0)
                                            .percentage(),
                                        );
                                    }
                                    Tool::Shape => {
                                        widgets::segmented(
                                            ui,
                                            &mut self.shape_kind,
                                            &[
                                                (ShapeKind::Rectangle, tr("Rectangle")),
                                                (ShapeKind::RoundedRectangle, tr("Rounded")),
                                                (ShapeKind::Ellipse, tr("Ellipse")),
                                            ],
                                        );
                                        ui.separator();
                                        ui.label(tr("Fill"));
                                        widgets::color_well(ui, &mut self.brush.color);
                                        if self.shape_kind == ShapeKind::RoundedRectangle {
                                            value!(
                                                ui,
                                                tr("Radius"),
                                                &mut self.corner_radius,
                                                0.0..=1000.0,
                                                " px",
                                            );
                                        }
                                    }
                                    Tool::Pen => {
                                        ui.label(
                                            RichText::new(self.pen_status())
                                                .color(ui.palette().muted),
                                        );
                                    }
                                    Tool::Text => self.text_options(ui),
                                    Tool::Crop => {
                                        ui.label(
                                            RichText::new(
                                                tr("Drag a crop area, then press Enter to apply"),
                                            )
                                            .color(ui.palette().muted),
                                        );
                                    }
                                    Tool::Dropper => {
                                        ui.label(tr("Sample"));
                                        widgets::segmented(
                                            ui,
                                            &mut self.dropper_source,
                                            &[
                                                (SampleSource::CurrentLayer, tr("Current layer")),
                                                (SampleSource::AllLayers, tr("All visible layers")),
                                            ],
                                        );
                                        widgets::segmented(
                                            ui,
                                            &mut self.dropper_size,
                                            &[
                                                (SampleSize::Point, tr("Point")),
                                                (SampleSize::Three, tr("3×3")),
                                                (SampleSize::Five, tr("5×5")),
                                            ],
                                        );
                                        widgets::color_well(ui, &mut self.brush.color);
                                    }
                                    Tool::Hand | Tool::Zoom => {
                                        ui.label(
                                        RichText::new(
                                            tr("Scroll to zoom · Space-drag to pan · Ctrl+0 to fit"),
                                        )
                                        .color(ui.palette().muted),
                                    );
                                    }
                                    _ => {}
                                }
                            });
                        });
                });
            });
        if changed && let Some(transform) = transform {
            let mask_target = self.transforming_mask();
            self.edit_continuous(tr("Transform"), |doc| {
                xuan::operations::apply_transform(doc, transform, mask_target)
            });
        }
    }

    pub(super) fn status_bar(&mut self, ctx: &egui::Context) {
        super::chrome::status_bar(&ctx.palette(), self.window_corner_radius(ctx), "status_bar")
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(session) = self.session() {
                        ui.add_sized(
                            [50.0, 22.0],
                            egui::Label::new(
                                RichText::new(format!("{:.1}%", session.zoom * 100.0))
                                    .size(11.0)
                                    .color(ui.palette().muted),
                            )
                            .halign(egui::Align::Min)
                            .truncate(),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "{} × {} px",
                                session.document.width, session.document.height
                            ))
                            .size(11.0)
                            .color(ui.palette().muted),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(tr("sRGB · Transparent"))
                                .size(11.0)
                                .color(ui.palette().muted),
                        );
                    } else {
                        ui.label(
                            RichText::new(tr("Ready when you are"))
                                .size(11.0)
                                .color(ui.palette().muted),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Running jobs, else the latest status message for a
                        // few seconds, else the tool's hint.
                        let jobs = self.running_jobs();
                        if !jobs.is_empty() {
                            self.job_status(ui, &jobs);
                        } else if let Some(status) = self.recent_status(ui.ctx()) {
                            ui.add(egui::Label::new(RichText::new(status).size(11.0)).truncate());
                        } else {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(self.tool.hint())
                                        .size(11.0)
                                        .color(ui.palette().muted),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
            });
    }

    /// The status message while it is new: for `STATUS_SECONDS` after it
    /// last changed. Only its first line is shown.
    fn recent_status(&mut self, ctx: &egui::Context) -> Option<String> {
        const STATUS_SECONDS: f64 = 8.0;
        let now = ctx.input(|i| i.time);
        if self.status != self.status_shown.0 {
            self.status_shown = (self.status.clone(), now);
        }
        let age = now - self.status_shown.1;
        if self.status.is_empty() || age >= STATUS_SECONDS {
            return None;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(STATUS_SECONDS - age));
        self.status.lines().next().map(str::to_owned)
    }

    pub(super) fn tool_rail(&mut self, ctx: &egui::Context) {
        let mut tool = None;
        egui::SidePanel::left("tools")
            .exact_width(56.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(ctx.palette().panel)
                    .inner_margin(egui::Margin::symmetric(10, 16)),
            )
            .show(ctx, |ui| {
                ui.add_enabled_ui(
                    self.dialog.is_none() && self.job.is_none() && self.color_range.is_none(),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 5.0;
                                for t in Tool::ALL {
                                    if t == Tool::Region && self.plugins.action.is_none() {
                                        continue;
                                    }
                                    let shortcut = super::commands::tool_command(t)
                                        .map(|id| self.keymap.shortcut(id))
                                        .unwrap_or_default();
                                    if icons::tool_button(ui, t, self.tool == t, &shortcut)
                                        .clicked()
                                    {
                                        tool = Some(t);
                                    }
                                }
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(5.0);
                                widgets::palette(ui, &mut self.brush.color, &mut self.background);
                            });
                    },
                );
            });
        if let Some(tool) = tool {
            self.set_tool(tool);
        }
    }

    /// Tool options → Brush dynamics: spacing, taper, scatter and jitter.
    fn brush_dynamics(&mut self, ui: &mut egui::Ui) {
        let dynamics = &mut self.brush.dynamics;
        let percent = |ui: &mut egui::Ui, label: &str, value: &mut f32, max: f32, hint: &str| {
            ui.label(label);
            ui.add(
                widgets::Slider::new(value, 0.0..=max)
                    .value_width(DEFAULT_PERCENT_VALUE_WIDTH)
                    .percentage(),
            )
            .on_hover_text(hint);
            ui.end_row();
        };
        egui::Grid::new("brush_dynamics")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                percent(
                    ui,
                    tr("Spacing"),
                    &mut dynamics.spacing,
                    10.0,
                    tr("Distance between dabs as a percentage of the size. 0% paints a continuous stroke."),
                );
                for (label, value) in [
                    (tr("Taper in"), &mut dynamics.taper_in),
                    (tr("Taper out"), &mut dynamics.taper_out),
                ] {
                    ui.label(label);
                    ui.add(
                        widgets::Number::new(value)
                            .size(egui::vec2(DEFAULT_VALUE_WIDTH, DEFAULT_VALUE_HEIGHT))
                            .speed(1.0)
                            .range(0.0..=10_000.0)
                            .suffix(" px")
                            .max_decimals(0),
                    )
                    .on_hover_text(tr(
                        "Length over which the stroke grows at its start or fades at its end. 0 turns the taper off.",
                    ));
                    ui.end_row();
                }
                ui.label("");
                ui.horizontal(|ui| {
                    widgets::checkbox(ui, &mut dynamics.taper_size, tr("Taper: size"));
                    widgets::checkbox(ui, &mut dynamics.taper_opacity, tr("Taper: opacity"));
                });
                ui.end_row();
                percent(
                    ui,
                    tr("Scatter"),
                    &mut dynamics.scatter,
                    10.0,
                    tr("How far dabs scatter from the stroke, as a percentage of the size."),
                );
                ui.label(tr("Count"));
                ui.add(
                    widgets::Number::new(&mut dynamics.count)
                        .size(egui::vec2(DEFAULT_PERCENT_VALUE_WIDTH, DEFAULT_VALUE_HEIGHT))
                        .range(1..=xuan::paint::dynamics::MAX_COUNT),
                )
                .on_hover_text(tr("Dabs painted at each spacing step."));
                ui.end_row();
                percent(
                    ui,
                    tr("Size jitter"),
                    &mut dynamics.size_jitter,
                    1.0,
                    tr("How much smaller each dab may randomly be."),
                );
                percent(
                    ui,
                    tr("Opacity jitter"),
                    &mut dynamics.opacity_jitter,
                    1.0,
                    tr("How much more transparent each dab may randomly be."),
                );
                percent(
                    ui,
                    tr("Hue jitter"),
                    &mut dynamics.hue_jitter,
                    1.0,
                    tr("How far each dab's hue may randomly turn. 100% reaches the opposite hue."),
                );
            });
        if widgets::button(ui, tr("Reset")).clicked() {
            *dynamics = xuan::paint::Dynamics {
                seed: dynamics.seed,
                ..Default::default()
            };
        }
    }

    /// Tool options → Symmetry: mirror or radial copies of each stroke.
    fn brush_symmetry(&mut self, ui: &mut egui::Ui) {
        use xuan::paint::{SymmetryMode, symmetry};
        let size = self
            .session()
            .map(|session| (session.document.width, session.document.height));
        let symmetry = &mut self.brush.symmetry;
        widgets::segmented(
            ui,
            &mut symmetry.mode,
            &[
                (SymmetryMode::Off, tr("Off")),
                (SymmetryMode::Vertical, tr("Vertical")),
                (SymmetryMode::Horizontal, tr("Horizontal")),
                (SymmetryMode::Radial, tr("Radial")),
            ],
        )
        .on_hover_text(tr(
            "Vertical mirrors left and right across the axis, Horizontal mirrors top and bottom, Radial turns the stroke around the centre.",
        ));
        ui.add_space(4.0);
        egui::Grid::new("brush_symmetry")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(tr("Segments"));
                ui.add_enabled(
                    symmetry.mode == SymmetryMode::Radial,
                    widgets::Number::new(&mut symmetry.segments)
                        .size(egui::vec2(
                            DEFAULT_PERCENT_VALUE_WIDTH,
                            DEFAULT_VALUE_HEIGHT,
                        ))
                        .range(symmetry::MIN_SEGMENTS..=symmetry::MAX_SEGMENTS),
                )
                .on_hover_text(tr("Copies of the stroke around the centre."));
                ui.end_row();
                if let Some((width, height)) = size {
                    let mut center = symmetry.center_in(width, height);
                    let mut changed = false;
                    for (label, value, max) in [
                        (tr("Centre X"), &mut center.x, width),
                        (tr("Centre Y"), &mut center.y, height),
                    ] {
                        ui.label(label);
                        changed |= ui
                            .add(
                                widgets::Number::new(value)
                                    .size(egui::vec2(DEFAULT_VALUE_WIDTH, DEFAULT_VALUE_HEIGHT))
                                    .speed(1.0)
                                    .range(0.0..=max as f32)
                                    .suffix(" px")
                                    .max_decimals(1),
                            )
                            .changed();
                        ui.end_row();
                    }
                    if changed {
                        symmetry.center = Some(center);
                    }
                }
            });
        if widgets::button(ui, tr("Centre on canvas")).clicked() {
            symmetry.center = None;
        }
    }
}

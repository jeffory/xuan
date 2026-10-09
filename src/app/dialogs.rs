use super::widgets;
use std::{io::Cursor, sync::Arc};
use xuan::canvas_presets as presets;
use xuan::i18n::tr;

use egui::{RichText, Stroke, vec2};
use xuan::{
    document::{Adjustment, Layer, Point},
    effects::{self, Filter},
    io, operations, render,
    units::{self, Unit},
};

use super::{
    Dialog, EditorApp, size_units,
    surfaces::{SurfaceRun, SurfaceStart},
    theme::PaletteExt,
};
use xuan::plugins::manifest::Surface;

impl EditorApp {
    /// Help → Keyboard Shortcuts: the bindings in effect, from the command registry, and the
    /// keys and gestures the editor handles itself.
    fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut customize = false;
        let mut done = false;
        let search_id = egui::Id::new("shortcuts_search");
        let mut search: String = ctx.data(|d| d.get_temp(search_id).unwrap_or_default());
        // Room for the list: the dialog's bounds less the title, search box, footer and insets.
        let height = (widgets::dialog_bounds(ctx).height() - 190.0).clamp(120.0, 480.0);
        let others = [
            (tr("Drag from a ruler"), tr("New guide")),
            ("1–0", tr("Brush or layer opacity")),
            ("Alt-click", tr("Set clone source")),
            ("Space-drag", tr("Pan canvas")),
            (
                tr("Horizontal wheel / Shift+wheel"),
                tr("Pan canvas horizontally"),
            ),
            (tr("Wheel over a slider or number"), tr("Adjust value")),
            ("Enter / Escape", tr("Apply crop / Cancel gesture")),
        ];
        widgets::Window::new(tr("Keyboard shortcuts"))
            .id("shortcuts")
            .default_width(690.0)
            .open(&mut open)
            .show_with_footer(
                ctx,
                |ui| {
                    super::keybindings::reference(ui, &self.keymap, &mut search, height, &others);
                },
                |ui, ()| {
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::single(tr("Done")),
                        |ui| {
                            customize = widgets::button(ui, tr("Customize…")).clicked();
                        },
                    );
                    done = response.commit;
                },
            );
        ctx.data_mut(|d| d.insert_temp(search_id, search));
        if customize {
            super::settings::show_settings_page(ctx, super::settings::SettingsPage::Keyboard);
            self.dialog = Some(Dialog::Settings);
        } else if !open || done {
            self.dialog = None;
            ctx.data_mut(|d| d.remove::<String>(search_id));
        }
    }

    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.job {
            widgets::Window::new(&job.name).show_with_footer(
                ctx,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("Working…"));
                    });
                    if let Some(progress) = job.progress() {
                        ui.add(egui::ProgressBar::new(progress).show_percentage());
                    }
                },
                |ui, ()| {
                    if widgets::dialog_footer(ui, widgets::FooterButtons::cancel_only(), |_| {})
                        .cancel
                    {
                        job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                },
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(dialog) = self.dialog {
            match dialog {
                Dialog::Settings => self.settings_dialog(ctx),
                Dialog::New | Dialog::CanvasSize | Dialog::ImageSize => {
                    self.size_dialog(ctx, dialog)
                }
                Dialog::Effect => self.effect_dialog(ctx),
                Dialog::Text => self.text_dialog(ctx),
                Dialog::Export => self.export_dialog(ctx),
                Dialog::DropChoice => self.drop_dialog(ctx),
                Dialog::PluginPermissions => self.plugin_permissions_dialog(ctx),
                Dialog::PluginConsent => self.plugin_consent_dialog(ctx),
                Dialog::PluginFile => self.plugin_file_dialog(ctx),
                Dialog::PluginEditSession => self.plugin_edit_session_dialog(ctx),
                Dialog::Plugins => self.plugin_manager_dialog(ctx),
                Dialog::PluginInstall => self.plugin_install_dialog(ctx),
                Dialog::PluginModels => self.plugin_models_dialog(ctx),
                Dialog::PluginProposal => self.plugin_proposal_dialog(ctx),
                Dialog::GridSettings => self.grid_settings_dialog(ctx),
                Dialog::LayerEffects => self.layer_effects_dialog(ctx),
                Dialog::SelectionAmount => self.selection_amount_dialog(ctx),
                Dialog::Trim => self.trim_dialog(ctx),
                Dialog::Stroke => self.stroke_dialog(ctx),
                Dialog::Collage => self.collage_dialog(ctx),
                Dialog::Paths => self.paths_dialog(ctx),
                Dialog::AdjustmentPresets => self.presets_dialog(ctx),
                Dialog::CanvasPresets => self.canvas_presets_dialog(ctx),
                Dialog::Shortcuts => self.shortcuts_dialog(ctx),
                Dialog::Update => self.update_dialog(ctx),
                Dialog::About => self.about_dialog(ctx),
            }
        }
        self.close_dialog(ctx);
        self.photoshop_dialog(ctx);
        if let Some(notice) = self.notice.clone() {
            let mut dismiss = false;
            widgets::Window::new(tr("Imported with changes"))
                .id("import_notice")
                .default_width(460.0)
                .show_with_footer(
                    ctx,
                    |ui| {
                        ui.label(notice);
                    },
                    |ui, ()| {
                        dismiss = widgets::dialog_footer(
                            ui,
                            widgets::FooterButtons::single(tr("OK")),
                            |_| {},
                        )
                        .commit;
                    },
                );
            if dismiss {
                self.notice = None;
            }
        }
        if let Some(error) = self.error.clone() {
            let mut dismiss = false;
            widgets::Window::new(tr("Couldn't complete the operation"))
                .default_width(420.0)
                .show_with_footer(
                    ctx,
                    |ui| {
                        ui.label(error);
                    },
                    |ui, ()| {
                        dismiss = widgets::dialog_footer(
                            ui,
                            widgets::FooterButtons::single(tr("OK")),
                            |_| {},
                        )
                        .commit;
                    },
                );
            if dismiss {
                self.error = None;
            }
        }
    }

    /// Help → About: the icon, name and version as a header, then the credits and links to the
    /// project, its licences and its issue tracker.
    fn about_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let build = xuan::buildinfo::current();
        widgets::Window::new(tr("About Xuan"))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let icon = app_icon(ui.ctx());
                    ui.add(egui::Image::new(&icon).fit_to_exact_size(vec2(64.0, 64.0)));
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.label(RichText::new("Xuan").size(24.0).strong());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let muted = ui.palette().muted;
                            ui.label(
                                RichText::new(build.display_with(|text| tr(text).to_owned()))
                                    .color(muted),
                            );
                            if ui.small_button(tr("Copy version")).clicked() {
                                ui.ctx().copy_text(build.cli());
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(tr("A space for your next composition."));
                    });
                });
                ui.add_space(16.0);
                ui.label(tr("Native image editor · Rust + egui + wgpu"));
                ui.add_space(4.0);
                ui.label(tr("Ported from Compositor by Wonder Assembly LLC."));
                ui.add_space(4.0);
                ui.label(tr("Free and open source, under the MIT licence."));
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (label, url) in about_links(xuan::update::REPOSITORY, &build) {
                        ui.hyperlink_to(tr(label), url);
                    }
                });
            });
        if !open {
            self.dialog = None;
        }
    }

    fn size_dialog(&mut self, ctx: &egui::Context, dialog: Dialog) {
        let title = match dialog {
            Dialog::New => tr("New canvas"),
            Dialog::CanvasSize => tr("Canvas size"),
            _ => tr("Image size"),
        };
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        // New Image can generate the image with a plugin's document action.
        let document_actions = if dialog == Dialog::New {
            self.surface_actions(Surface::Document)
        } else {
            Vec::new()
        };
        if document_actions.is_empty() {
            self.new_image_generate = false;
        }
        let chosen = (self.new_image_action.clone())
            .filter(|(p, a)| {
                document_actions
                    .iter()
                    .any(|a2| a2.plugin == *p && a2.action == *a)
            })
            .or_else(|| {
                document_actions
                    .first()
                    .map(|a| (a.plugin.clone(), a.action.clone()))
            });
        let generating = self.new_image_generate && chosen.is_some();
        // Focus Width, with its value selected, once when the dialog opens; in Generate, the
        // prompt instead, once each time Generate is chosen.
        let focused = egui::Id::new("size_dialog_focused");
        let focus_now = ctx.data(|d| d.get_temp::<Option<(Dialog, bool)>>(focused).flatten())
            != Some((dialog, generating));
        let focus_width = focus_now && !generating;
        if focus_now && generating {
            ctx.data_mut(|d| d.insert_temp(focused, Some((dialog, generating))));
        }
        widgets::Window::new(tr(title))
            .open(&mut open)
            .default_width(410.0)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.add_space(7.0);
                    if !document_actions.is_empty() {
                        let mut generate = self.new_image_generate;
                        widgets::segmented(
                            ui,
                            &mut generate,
                            &[(false, tr("Blank")), (true, tr("Generate"))],
                        );
                        self.new_image_generate = generate;
                        ui.add_space(10.0);
                    }
                    ui.label(
                        RichText::new(if generating {
                            tr("Describe the image; it is made at least this size.")
                        } else if dialog == Dialog::New {
                            tr("A blank space for your next composition.")
                        } else if dialog == Dialog::CanvasSize {
                            tr("Change the canvas bounds and anchor your composition.")
                        } else {
                            tr("Scale the composition while preserving source pixels.")
                        })
                        .color(ui.palette().muted),
                    );
                    ui.add_space(16.0);
                    if dialog == Dialog::New {
                        self.preset_menu(ui);
                        ui.add_space(12.0);
                    }
                    let before = self.dimensions;
                    let resolution = self.resolution;
                    // Image Size without Resample keeps the pixels: Width and Height then set
                    // the print size, through the resolution, and only in print units.
                    let fixed_pixels = dialog == Dialog::ImageSize && !self.size_units.resample;
                    let sides_enabled = !fixed_pixels || self.size_units.unit.is_physical();
                    ui.horizontal(|ui| {
                        for axis in 0..2 {
                            if axis == 1 {
                                ui.add_space(15.0);
                            }
                            ui.vertical(|ui| {
                                ui.label(tr(if axis == 0 { "Width" } else { "Height" }));
                                let field = ui.add_enabled_ui(sides_enabled, |ui| {
                                    self.size_field(ui, axis, fixed_pixels)
                                });
                                let field = field.inner;
                                if axis == 0 && focus_width {
                                    if field.has_focus() {
                                        // The field is editing now: select its text so typing
                                        // replaces it, and stop asking for focus.
                                        let mut state =
                                            egui::TextEdit::load_state(ui.ctx(), field.id)
                                                .unwrap_or_default();
                                        state.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::two(
                                                egui::text::CCursor::new(0),
                                                egui::text::CCursor::new(usize::MAX),
                                            ),
                                        ));
                                        state.store(ui.ctx(), field.id);
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(focused, Some((dialog, false)))
                                        });
                                    } else {
                                        field.request_focus();
                                    }
                                }
                            });
                        }
                        ui.add_space(15.0);
                        ui.vertical(|ui| {
                            ui.label(tr("Units"));
                            let choices: &[Unit] = if dialog == Dialog::New {
                                &Unit::LENGTHS
                            } else {
                                &Unit::ALL
                            };
                            if size_units::unit_menu(ui, &mut self.size_units.unit, choices) {
                                self.size_units.size_edited();
                                self.remember_units();
                            }
                        });
                    });
                    if self.dimensions != before {
                        self.size_units.size_edited();
                    }
                    if dialog == Dialog::New {
                        self.keep_ratio_edit(before);
                        ui.add_space(8.0);
                        self.swap_and_link(ui);
                    } else if dialog == Dialog::ImageSize && !fixed_pixels {
                        self.keep_ratio_edit(before);
                        ui.add_space(8.0);
                        let was = self.keep_ratio;
                        widgets::checkbox(ui, &mut self.keep_ratio, tr("Keep aspect ratio"));
                        if self.keep_ratio && !was {
                            self.ratio = self.dimensions;
                        }
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(tr("Resolution"));
                        let unit = self.size_units.resolution_unit;
                        let mut shown = unit.from_ppi(f64::from(self.resolution));
                        let range = unit.from_ppi(f64::from(units::MIN_RESOLUTION))
                            ..=unit.from_ppi(f64::from(units::MAX_RESOLUTION));
                        let changed = ui
                            .add(
                                widgets::Number::new(&mut shown)
                                    .range(range)
                                    .max_decimals(2)
                                    .suffix(format!(" {}", unit.suffix()))
                                    .parser(move |text| unit.parse(text)),
                            )
                            .changed();
                        let ppi = unit.to_ppi(shown);
                        if changed && units::valid_resolution(ppi) {
                            self.resolution = ppi as f32;
                        }
                        if size_units::resolution_unit_menu(
                            ui,
                            &mut self.size_units.resolution_unit,
                        ) {
                            self.remember_units();
                        }
                    });
                    let keeps_print_size = dialog == Dialog::New
                        || (dialog == Dialog::ImageSize && self.size_units.resample);
                    self.size_units.resolution_changed(
                        &mut self.dimensions,
                        resolution,
                        self.resolution,
                        keeps_print_size,
                    );
                    if dialog == Dialog::ImageSize {
                        ui.add_space(8.0);
                        let was = self.size_units.resample;
                        widgets::checkbox(ui, &mut self.size_units.resample, tr("Resample"))
                            .on_hover_text(tr(
                                "Off, the pixels stay as they are: Width, Height and Resolution change only the print size",
                            ));
                        if was && !self.size_units.resample {
                            self.dimensions = self.size_units.original;
                            self.size_units.size_edited();
                        }
                    } else if dialog == Dialog::CanvasSize {
                        ui.add_space(8.0);
                        widgets::checkbox(ui, &mut self.size_units.relative, tr("Relative"))
                            .on_hover_text(tr(
                                "Width and Height are added to the current size",
                            ));
                    }
                    if dialog == Dialog::CanvasSize {
                        ui.add_space(12.0);
                        ui.label(tr("Anchor"));
                        egui::Grid::new("anchor_grid")
                            .spacing(vec2(3.0, 3.0))
                            .show(ui, |ui| {
                                for y in 0..3 {
                                    for x in 0..3 {
                                        let anchor = [x as f32 * 0.5, y as f32 * 0.5];
                                        if ui
                                            .selectable_label(
                                                self.anchor == anchor,
                                                if self.anchor == anchor { "●" } else { "·" },
                                            )
                                            .clicked()
                                        {
                                            self.anchor = anchor;
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                    }
                    let mut ready = true;
                    if let Some((plugin, action)) = chosen.clone().filter(|_| generating) {
                        ui.add_space(12.0);
                        let offered = (document_actions.iter())
                            .find(|a| a.plugin == plugin && a.action == action);
                        let (label, source) = offered
                            .map(|a| (a.attributed(), a.source.clone()))
                            .unwrap_or_default();
                        if document_actions.len() > 1 {
                            let mut choice = (plugin.clone(), action.clone());
                            widgets::PopUp::from_id_salt("new_image_action")
                                .selected_text(label)
                                .width(280.0)
                                .show_ui(ui, |ui| {
                                    for a in &document_actions {
                                        widgets::menu_choice(
                                            ui,
                                            &mut choice,
                                            (a.plugin.clone(), a.action.clone()),
                                            a.attributed(),
                                        )
                                        .on_hover_text(&a.source);
                                    }
                                });
                            if choice != (plugin.clone(), action.clone()) {
                                self.new_image_action = Some(choice);
                            }
                        } else {
                            // The plugin's words below say whose they are.
                            ui.label(RichText::new(label).small().color(ui.palette().muted))
                                .on_hover_text(source);
                        }
                        ui.add_space(8.0);
                        ready = self.surface_form(
                            ui,
                            &plugin,
                            &action,
                            Surface::Document,
                            "new_image",
                            focus_now,
                        );
                        widgets::checkbox(ui, &mut self.new_image_exact, tr("Exact size"))
                            .on_hover_text(tr(
                                "Make the canvas exactly W × H; the image covers it and can be moved",
                            ));
                    }
                    let valid =
                        xuan::document::validate_size(self.dimensions[0], self.dimensions[1]);
                    ui.add_space(12.0);
                    if let Err(error) = &valid {
                        ui.colored_label(ui.palette().error, error.to_string());
                    } else {
                        ui.label(
                            RichText::new(size_units::summary(
                                self.dimensions,
                                dialog == Dialog::ImageSize,
                            ))
                            .color(ui.palette().muted),
                        );
                        if dialog == Dialog::New && !generating {
                            ui.label(
                                RichText::new(tr("Transparent canvas · sRGB"))
                                    .color(ui.palette().muted),
                            );
                        }
                    }
                    valid.is_ok() && ready
                },
                |ui, valid| {
                    let commit = if generating {
                        tr("Generate image")
                    } else if dialog == Dialog::New {
                        tr("Create canvas")
                    } else {
                        tr("Resize")
                    };
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(commit).enabled(valid),
                        |_| {},
                    );
                    apply = response.commit;
                    cancel = response.cancel;
                },
            );
        if apply
            && generating
            && let Some((plugin, action)) = chosen
        {
            let values = self.surface_values(&plugin, &action).clone();
            let run = SurfaceRun {
                surface: Surface::Document,
                target: (self.dimensions[0], self.dimensions[1]),
                exact: self.new_image_exact,
                resolution: self.resolution,
                boxes: Vec::new(),
            };
            // A permission or consent prompt takes the dialog's place.
            let started = self.run_from_surface(&plugin, &action, &values, Vec::new(), run);
            if started != SurfaceStart::NotStarted && self.dialog == Some(Dialog::New) {
                self.dialog = None;
            }
        } else if apply {
            if dialog == Dialog::New {
                self.new_document();
            } else {
                let [width, height] = self.dimensions;
                let anchor = self.anchor;
                let resolution = self.resolution;
                // Without Resample, or at the same size, only the resolution changes.
                let resample =
                    self.size_units.resample && self.dimensions != self.size_units.original;
                self.edit(title, |doc| {
                    if dialog == Dialog::CanvasSize {
                        operations::canvas_size(doc, width, height, anchor)?;
                    } else if resample {
                        operations::image_size(doc, width, height)?;
                    }
                    doc.resolution = resolution;
                    Ok(())
                });
                if let Some(s) = self.session_mut() {
                    s.fit = true;
                }
                self.dialog = None;
            }
        } else if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dialog = None;
        }
        if self.dialog != Some(dialog) {
            ctx.data_mut(|d| d.remove_temp::<Option<(Dialog, bool)>>(focused));
        }
    }

    /// With Keep aspect ratio, the side not edited follows the one that was.
    fn keep_ratio_edit(&mut self, before: [u32; 2]) {
        let [width, height] = self.dimensions;
        let [ratio_width, ratio_height] = self.ratio;
        if !self.keep_ratio || self.dimensions == before {
            return;
        }
        if width != before[0] {
            self.dimensions[1] = presets::linked_side(ratio_width, ratio_height, width, height);
        } else {
            self.dimensions[0] = presets::linked_side(ratio_height, ratio_width, height, width);
        }
    }

    /// Width (`axis` 0) or Height (1) in the dialog's unit. A value typed with another unit
    /// (`10cm`) is converted. With `fixed_pixels` (Image Size without Resample) a print size
    /// sets the resolution instead of the pixels.
    fn size_field(&mut self, ui: &mut egui::Ui, axis: usize, fixed_pixels: bool) -> egui::Response {
        let units = &self.size_units;
        let (unit, ppi, relative) = (units.unit, self.resolution, units.relative);
        let reference = f64::from(units.original[axis]);
        let mut value = units.shown(self.dimensions[axis], axis, ppi);
        let suffix = if unit == Unit::Percent {
            "%".to_owned()
        } else {
            format!(" {}", unit.suffix())
        };
        let response = ui.add(
            widgets::Number::new(&mut value)
                .range(units.range(axis, ppi))
                .speed(units.step(axis, ppi))
                .max_decimals(unit.decimals())
                .suffix(suffix)
                .parser(move |text| size_units::parse_size(text, unit, ppi, reference, relative)),
        );
        if response.changed() {
            if fixed_pixels {
                if let Some(resolution) = self.size_units.resolution_for(value, axis) {
                    self.resolution = resolution;
                }
            } else if let Some(pixels) = self.size_units.pixels(value, axis, ppi) {
                self.dimensions[axis] = pixels;
            }
        }
        response
    }

    /// Saves the size and resolution units chosen in a size dialog for next time.
    pub(super) fn remember_units(&mut self) {
        let mut units = self.config.units;
        units.size = self.size_units.unit;
        units.resolution = self.size_units.resolution_unit;
        if units != self.config.units {
            self.config.units = units;
            self.save_config();
        }
    }

    /// New canvas: Swap turns portrait into landscape; Keep aspect ratio ties Width to Height.
    fn swap_and_link(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if widgets::button(ui, tr("Swap"))
                .on_hover_text(tr("Swap width and height (portrait / landscape)"))
                .clicked()
            {
                self.dimensions = presets::swapped(self.dimensions);
                self.ratio = presets::swapped(self.ratio);
            }
            let was = self.keep_ratio;
            widgets::checkbox(ui, &mut self.keep_ratio, tr("Keep aspect ratio"));
            if self.keep_ratio && !was {
                self.ratio = self.dimensions;
            }
        });
    }

    fn effect_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.effect.take() else {
            self.dialog = None;
            return;
        };
        let title = edit
            .adjustment
            .as_ref()
            .map(|a| a.name())
            .or_else(|| edit.filter.as_ref().map(|f| f.name()))
            .unwrap_or(tr("Adjustment"));
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let mut changed = false;
        let mut preview_changed = false;
        widgets::Window::new(tr(title))
            .open(&mut open)
            .default_width(440.0)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.add_space(8.0);
                    if let Some(adjustment) = &mut edit.adjustment {
                        match adjustment {
                            Adjustment::HueRanges { settings } => {
                                widgets::PopUp::from_id_salt("hue_range")
                                    .selected_text(xuan::color::HueSettings::RANGES[settings.range])
                                    .show_ui(ui, |ui| {
                                        for (index, name) in
                                            xuan::color::HueSettings::RANGES.iter().enumerate()
                                        {
                                            changed |= widgets::menu_choice(
                                                ui,
                                                &mut settings.range,
                                                index,
                                                *name,
                                            )
                                            .changed();
                                        }
                                    });
                                let values = &mut settings.adjustments[settings.range];
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(
                                            &mut values[0],
                                            if settings.colorize {
                                                0.0..=360.0
                                            } else {
                                                -180.0..=180.0
                                            },
                                        )
                                        .text(tr("Hue"))
                                        .suffix("°")
                                        .centered(),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(
                                            &mut values[1],
                                            if settings.colorize {
                                                0.0..=100.0
                                            } else {
                                                -100.0..=100.0
                                            },
                                        )
                                        .text(tr("Saturation"))
                                        .suffix("%")
                                        .centered(),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(&mut values[2], -100.0..=100.0)
                                            .text(tr("Lightness"))
                                            .suffix("%")
                                            .centered(),
                                    )
                                    .changed();
                                changed |=
                                    widgets::checkbox(ui, &mut settings.colorize, tr("Colourise"))
                                        .changed();
                                if settings.range > 0 {
                                    changed |= widgets::checkbox(
                                        ui,
                                        &mut settings.invert_range,
                                        tr("Invert selected colour range"),
                                    )
                                    .changed();
                                    ui.collapsing(tr("Colour range falloff"), |ui| {
                                        for (index, label) in [
                                            tr("Falloff start"),
                                            tr("Range start"),
                                            tr("Range end"),
                                            tr("Falloff end"),
                                        ]
                                        .iter()
                                        .enumerate()
                                        {
                                            changed |= ui
                                                .add(
                                                    widgets::Slider::new(
                                                        &mut settings.bands[settings.range][index],
                                                        0.0..=360.0,
                                                    )
                                                    .text(*label)
                                                    .suffix("°"),
                                                )
                                                .changed();
                                        }
                                    });
                                }
                            }
                            Adjustment::LevelsChannels { ranges } => {
                                channel_picker(ui, &mut edit.channel);
                                let source = edit.levels_source.get_or_insert_with(|| {
                                    render::render_scaled(&edit.original, 256, 192)
                                });
                                changed |= super::levels_controls::controls(
                                    ui,
                                    &mut ranges[edit.channel],
                                    source,
                                    edit.channel,
                                );
                            }
                            Adjustment::CurvesChannels { channels } => {
                                channel_picker(ui, &mut edit.channel);
                                changed |= curve_editor(ui, &mut channels[edit.channel]);
                            }
                            Adjustment::HueSaturation {
                                hue,
                                saturation,
                                lightness,
                                colorize,
                            } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(hue, -180.0..=180.0)
                                            .text(tr("Hue"))
                                            .suffix("°")
                                            .centered(),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(saturation, -100.0..=100.0)
                                            .text(tr("Saturation"))
                                            .suffix("%")
                                            .centered(),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(lightness, -100.0..=100.0)
                                            .text(tr("Lightness"))
                                            .suffix("%")
                                            .centered(),
                                    )
                                    .changed();
                                changed |=
                                    widgets::checkbox(ui, colorize, tr("Colourise")).changed();
                            }
                            Adjustment::Levels {
                                black,
                                gamma,
                                white,
                                output_black,
                                output_white,
                            } => {
                                let source = edit.levels_source.get_or_insert_with(|| {
                                    render::render_scaled(&edit.original, 256, 192)
                                });
                                let mut range =
                                    [*black, *gamma, *white, *output_black, *output_white];
                                changed |=
                                    super::levels_controls::controls(ui, &mut range, source, 0);
                                [*black, *gamma, *white, *output_black, *output_white] = range;
                            }
                            Adjustment::Curves { points } => {
                                changed |= curve_editor(ui, points);
                            }
                            Adjustment::Exposure {
                                exposure,
                                offset,
                                gamma,
                            } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(exposure, -5.0..=5.0)
                                            .text(tr("Exposure"))
                                            .suffix(" EV")
                                            .centered(),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(offset, -0.5..=0.5)
                                            .text(tr("Offset"))
                                            .centered(),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(widgets::Slider::new(gamma, 0.1..=5.0).text(tr("Gamma")))
                                    .changed();
                            }
                            Adjustment::GradientMap {
                                shadows,
                                highlights,
                            } => {
                                ui.horizontal(|ui| {
                                    ui.label(tr("Shadows"));
                                    changed |= widgets::color_well(ui, shadows).changed();
                                    ui.label(tr("Highlights"));
                                    changed |= widgets::color_well(ui, highlights).changed();
                                });
                            }
                            Adjustment::FilmGrain {
                                amount,
                                size,
                                roughness,
                                seed,
                            } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(amount, 0.0..=100.0)
                                            .text(tr("Amount"))
                                            .suffix("%"),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(size, 0.1..=100.0)
                                            .logarithmic(true)
                                            .text(tr("Size"))
                                            .suffix(" px"),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(roughness, 0.0..=100.0)
                                            .text(tr("Roughness"))
                                            .suffix("%"),
                                    )
                                    .changed();
                                if widgets::button(ui, tr("New pattern")).clicked() {
                                    *seed = seed.wrapping_add(1);
                                    changed = true;
                                }
                            }
                            Adjustment::Grain {
                                amount, monochrome, ..
                            } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(amount, 0.0..=100.0)
                                            .text(tr("Amount"))
                                            .suffix("%"),
                                    )
                                    .changed();
                                changed |= widgets::checkbox(ui, monochrome, tr("Monochromatic"))
                                    .changed();
                            }
                            Adjustment::Invert => {}
                            Adjustment::ColorLookup {
                                name,
                                interpolation,
                                table,
                            } => {
                                changed |=
                                    super::color_lookup::controls(ui, name, interpolation, table);
                            }
                            Adjustment::BlackWhite {
                                weights,
                                tint,
                                tint_hue,
                                tint_saturation,
                            } => {
                                for (weight, name) in weights.iter_mut().zip([
                                    "Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas",
                                ]) {
                                    changed |= ui
                                        .add(
                                            widgets::Slider::new(weight, -200.0..=300.0)
                                                .text(tr(name))
                                                .suffix("%")
                                                .max_decimals(0),
                                        )
                                        .changed();
                                }
                                changed |= widgets::checkbox(ui, tint, tr("Tint")).changed();
                                ui.add_enabled_ui(*tint, |ui| {
                                    changed |= ui
                                        .add(
                                            widgets::Slider::new(tint_hue, 0.0..=360.0)
                                                .text(tr("Hue"))
                                                .suffix("°")
                                                .max_decimals(0),
                                        )
                                        .changed();
                                    changed |= ui
                                        .add(
                                            widgets::Slider::new(tint_saturation, 0.0..=100.0)
                                                .text(tr("Saturation"))
                                                .suffix("%")
                                                .max_decimals(0),
                                        )
                                        .changed();
                                });
                                if widgets::button(ui, tr("Default")).clicked() {
                                    if let Adjustment::BlackWhite {
                                        weights: defaults, ..
                                    } = Adjustment::BLACK_WHITE
                                    {
                                        *weights = defaults;
                                    }
                                    changed = true;
                                }
                            }
                            Adjustment::ColorBalance {
                                shadows,
                                midtones,
                                highlights,
                                preserve_luminosity,
                            } => {
                                let tone_id = ui.id().with("color_balance_tone");
                                let mut tone =
                                    ui.data(|d| d.get_temp::<usize>(tone_id).unwrap_or(1));
                                widgets::segmented(
                                    ui,
                                    &mut tone,
                                    &[
                                        (0, tr("Shadows")),
                                        (1, tr("Midtones")),
                                        (2, tr("Highlights")),
                                    ],
                                );
                                ui.data_mut(|d| d.insert_temp(tone_id, tone));
                                let values = match tone {
                                    0 => shadows,
                                    2 => highlights,
                                    _ => midtones,
                                };
                                for (value, (low, high)) in values.iter_mut().zip([
                                    ("Cyan", "Red"),
                                    ("Magenta", "Green"),
                                    ("Yellow", "Blue"),
                                ]) {
                                    changed |= ui
                                        .add(
                                            widgets::Slider::new(value, -100.0..=100.0)
                                                .text(format!("{} – {}", tr(low), tr(high)))
                                                .max_decimals(0)
                                                .centered(),
                                        )
                                        .changed();
                                }
                                changed |= widgets::checkbox(
                                    ui,
                                    preserve_luminosity,
                                    tr("Preserve Luminosity"),
                                )
                                .changed();
                            }
                        }
                    }
                    if let Some(filter) = &mut edit.filter {
                        ui.add_enabled_ui(!edit.filter_preview.applying, |ui| match filter {
                            Filter::GaussianBlur { radius } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(radius, 0.1..=100.0)
                                            .text(tr("Radius"))
                                            .suffix(" px"),
                                    )
                                    .changed();
                            }
                            Filter::MotionBlur { distance, angle } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(distance, 1.0..=200.0)
                                            .text(tr("Distance"))
                                            .suffix(" px"),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(angle, -180.0..=180.0)
                                            .text(tr("Angle"))
                                            .suffix("°")
                                            .centered(),
                                    )
                                    .changed();
                            }
                            Filter::Noise { amount, monochrome } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(amount, 0.0..=100.0)
                                            .text(tr("Amount"))
                                            .suffix("%"),
                                    )
                                    .changed();
                                changed |= widgets::checkbox(ui, monochrome, tr("Monochromatic"))
                                    .changed();
                            }
                            Filter::LensCorrection {
                                distortion,
                                vignette,
                            } => {
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(distortion, -50.0..=50.0)
                                            .text(tr("Distortion"))
                                            .suffix("%")
                                            .centered(),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        widgets::Slider::new(vignette, -100.0..=100.0)
                                            .text(tr("Vignette"))
                                            .suffix("%")
                                            .centered(),
                                    )
                                    .changed();
                            }
                            other => changed |= super::filter_controls::show(ui, other),
                        });
                    }
                    if edit.as_layer {
                        ui.label(
                            RichText::new(tr("Non-destructive effect layer"))
                                .small()
                                .color(ui.palette().muted),
                        );
                    }
                },
                |ui, ()| {
                    let applying = edit.filter_preview.applying;
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(tr("Apply")).enabled(!applying),
                        |ui| {
                            ui.add_enabled_ui(!applying, |ui| {
                                preview_changed =
                                    widgets::checkbox(ui, &mut edit.preview, tr("Preview"))
                                        .changed();
                            });
                            if edit.filter_preview.busy() {
                                ui.spinner();
                                ui.label(
                                    RichText::new(if applying {
                                        tr("Applying…")
                                    } else {
                                        tr("Updating preview…")
                                    })
                                    .color(ui.palette().muted),
                                );
                            }
                        },
                    );
                    apply = response.commit;
                    cancel = response.cancel;
                },
            );
        changed |= preview_changed;
        if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(s) = self.session_mut() {
                s.history.cancel(&mut s.document);
                s.motion_blur_preview = None;
                s.invalidate();
            }
            self.dialog = None;
            return;
        }
        if edit.filter.is_some() && !edit.as_layer && edit.target.is_none() {
            if !self.update_filter_preview(&mut edit, changed, apply) {
                self.effect = Some(edit);
            }
            return;
        }
        if changed || edit.refresh || apply {
            let mask_target = self.editing_mask();
            if let Some(session) = self.session_mut() {
                session.document = edit.original.clone();
                if edit.preview || apply {
                    let result = if edit.as_layer || edit.target.is_some() {
                        if let Some(target) = edit.target {
                            if let Some(layer) =
                                session.document.layers.iter_mut().find(|l| l.id == target)
                            {
                                layer.adjustment = edit.adjustment.clone();
                                layer.filter = edit.filter.clone();
                            }
                            Ok(())
                        } else {
                            let name = edit
                                .adjustment
                                .as_ref()
                                .map(Adjustment::name)
                                .or_else(|| edit.filter.as_ref().map(Filter::name))
                                .unwrap();
                            let mut layer =
                                Layer::blank(name, session.document.width, session.document.height);
                            layer.adjustment = edit.adjustment.clone();
                            layer.filter = edit.filter.clone();
                            if session.document.selection.is_some() {
                                let mask =
                                    xuan::paint::mask_from_selection(&session.document, &layer);
                                layer.mask = Some(xuan::document::Mask {
                                    pixels: Arc::new(mask),
                                    ..xuan::document::Mask::white()
                                });
                            }
                            session.document.insert(layer);
                            Ok(())
                        }
                    } else if let Some(adjustment) = &edit.adjustment {
                        effects::apply_adjustment(&mut session.document, adjustment, mask_target)
                    } else {
                        Ok(())
                    };
                    if let Err(error) = result {
                        session.document = edit.original.clone();
                        session.history.cancel(&mut session.document);
                        session.invalidate();
                        self.error = Some(error.to_string());
                        self.dialog = None;
                        return;
                    }
                }
                session.invalidate();
            }
            edit.refresh = false;
        }
        if apply {
            if let Some(s) = self.session_mut() {
                s.commit();
            }
            self.dialog = None;
        } else {
            self.effect = Some(edit);
        }
    }

    fn export_dialog(&mut self, ctx: &egui::Context) {
        if self.sessions.is_empty() {
            self.dialog = None;
            return;
        }
        let plugin_formats = self.plugin_export_formats();
        let formats: Vec<(String, String)> = ["png", "jpg", "tiff", "webp", "ora"]
            .iter()
            .map(|f| (f.to_string(), f.to_uppercase()))
            .chain(
                plugin_formats
                    .iter()
                    .filter(|(extension, ..)| {
                        !["png", "jpg", "tiff", "webp", "ora"].contains(&extension.as_str())
                    })
                    .map(|(extension, label, ..)| (extension.clone(), label.clone())),
            )
            .collect();
        if self.export_changed {
            let doc = &self.session().unwrap().document;
            let factor = (700.0 / doc.width.max(doc.height) as f32).min(1.0);
            let image = render::render_scaled(
                doc,
                (doc.width as f32 * factor).max(1.0) as u32,
                (doc.height as f32 * factor).max(1.0) as u32,
            );
            let mut preview = image.clone();
            let mut bytes = Vec::new();
            let options = self.export_options;
            if self.export_format == "jpg" {
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                    &mut bytes,
                    options.jpeg_quality,
                );
                if encoder.encode_image(&render::flatten_white(&image)).is_ok()
                    && let Ok(decoded) = image::load_from_memory(&bytes)
                {
                    preview = decoded.to_rgba8();
                }
            } else if self.export_format == "webp" && !options.webp_lossless {
                if let Ok(encoded) = io::encode_webp(&image, false, options.webp_quality)
                    && let Ok(decoded) = image::load_from_memory(&encoded)
                {
                    preview = decoded.to_rgba8();
                    bytes = encoded;
                }
            } else {
                let _ = image::DynamicImage::ImageRgba8(image)
                    .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png);
            }
            // The preview is at most 700 pixels a side: scale its size up to the
            // document's, a rough guide to the file's.
            let area = |w: u32, h: u32| f64::from(w) * f64::from(h);
            self.export_bytes = (bytes.len() as f64 * area(doc.width, doc.height)
                / area(preview.width(), preview.height()))
            .round() as usize;
            self.export_texture = Some(ctx.load_texture(
                "export_preview",
                egui::ColorImage::from_rgba_unmultiplied(
                    [preview.width() as usize, preview.height() as usize],
                    preview.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            ));
            self.export_changed = false;
        }
        let mut open = true;
        let mut export = false;
        let mut cancel = false;
        widgets::Window::new(tr("Export image"))
            .open(&mut open)
            .default_width(650.0)
            .show_with_footer(
                ctx,
                |ui| {
                    if let Some(texture) = &self.export_texture {
                        let size = texture.size_vec2();
                        let factor = (600.0 / size.x).min(350.0 / size.y).min(1.0);
                        ui.vertical_centered(|ui| {
                            ui.image((texture.id(), size * factor));
                        });
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(tr("Format"));
                        widgets::PopUp::from_id_salt("export_format")
                            .selected_text(self.export_format.to_uppercase())
                            .show_ui(ui, |ui| {
                                for (format, label) in &formats {
                                    self.export_changed |= widgets::menu_choice(
                                        ui,
                                        &mut self.export_format,
                                        format.clone(),
                                        label,
                                    )
                                    .changed();
                                }
                            });
                        if self.export_format == "webp" {
                            self.export_changed |= widgets::checkbox(
                                ui,
                                &mut self.export_options.webp_lossless,
                                tr("Lossless"),
                            )
                            .changed();
                        }
                        let quality = match self.export_format.as_str() {
                            "jpg" => Some(&mut self.export_options.jpeg_quality),
                            "webp" if !self.export_options.webp_lossless => {
                                Some(&mut self.export_options.webp_quality)
                            }
                            _ => None,
                        };
                        if let Some(quality) = quality {
                            ui.spacing_mut().slider_width =
                                (ui.available_width() - widgets::SLIDER_FIELD_WIDTH).max(90.0);
                            self.export_changed |= ui
                                .add(
                                    widgets::Slider::new(quality, io::EXPORT_QUALITY)
                                        .text(tr("Quality"))
                                        .suffix("%"),
                                )
                                .changed();
                        }
                    });
                    let lossy = self.export_format == "jpg"
                        || (self.export_format == "webp" && !self.export_options.webp_lossless);
                    if self.export_format == "ora" {
                        let report = io::ora::export_report(&self.session().unwrap().document);
                        let mut note =
                            tr("OpenRaster keeps layers, folders, blend modes and opacity")
                                .to_owned();
                        if !report.is_empty() {
                            note.push_str(" · ");
                            note.push_str(tr("These are flattened:"));
                            for line in report.lines() {
                                note.push_str(&format!("\n• {line}"));
                            }
                        }
                        ui.label(RichText::new(note).small().color(ui.palette().muted));
                    }
                    if lossy {
                        let note = if self.export_format == "jpg" {
                            tr("JPEG preview · transparency is flattened onto white")
                        } else {
                            tr("WebP preview · transparency is kept")
                        };
                        ui.label(
                            RichText::new(format!(
                                "{note} · {} {}",
                                tr("Estimated size:"),
                                estimated_size(self.export_bytes)
                            ))
                            .small()
                            .color(ui.palette().muted),
                        );
                    }
                },
                |ui, ()| {
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(tr("Export…")),
                        |_| {},
                    );
                    export = response.commit;
                    cancel = response.cancel;
                },
            );
        if export {
            let title = self.session().unwrap().title.clone();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(
                    self.export_format.to_uppercase(),
                    &[self.export_format.as_str()],
                )
                .set_file_name(format!("{title}.{}", self.export_format))
                .save_file()
            {
                match plugin_formats
                    .iter()
                    .find(|(extension, ..)| *extension == self.export_format)
                {
                    // The plugin writes the file in the background and the
                    // status bar reports when it is done.
                    Some((_, _, plugin, format)) => {
                        let (plugin, format) = (plugin.clone(), format.clone());
                        match self.start_plugin_export(&plugin, &format, &path) {
                            Ok(()) => self.dialog = None,
                            Err(error) => self.error = Some(format!("{error:#}")),
                        }
                    }
                    None => match self.export_to(&path) {
                        Ok(notice) => {
                            self.status = format!("{} {}", tr("Exported"), path.display());
                            self.notice = notice;
                            self.dialog = None;
                        }
                        Err(error) => self.error = Some(format!("{error:#}")),
                    },
                }
            }
        }
        if !open || cancel {
            self.dialog = None;
        }
    }

    /// Export the current document in Xuan's own encoders, returning what an OpenRaster
    /// export flattened, for the notice.
    pub(super) fn export_to(&self, path: &std::path::Path) -> anyhow::Result<Option<String>> {
        let document = &self.session().unwrap().document;
        if io::is_openraster(path) {
            Ok(io::ora::export(document, path)?.summary())
        } else {
            io::export(document, path, &self.export_options).map(|()| None)
        }
    }

    fn close_dialog(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        if let Some(index) = self.close_tab {
            if index >= self.sessions.len() {
                self.close_tab = None;
                return;
            }
            if !self.sessions[index].history.dirty() {
                self.remember_closed(index);
                self.sessions.remove(index);
                self.current = self.current.min(self.sessions.len().saturating_sub(1));
                self.close_tab = None;
                return;
            }
        }
        if self.close_tab.is_none() && !self.close_app {
            return;
        }
        let mut choice = None;
        let message = if self.close_app {
            tr("Some projects have unsaved changes.").to_owned()
        } else {
            format!(
                "“{}” — {}",
                self.sessions[self.close_tab.unwrap()].title,
                tr("Unsaved changes")
            )
        };
        widgets::Window::new(tr("Save your changes?")).show_with_footer(
            ctx,
            |ui| {
                ui.label(message);
            },
            |ui, ()| {
                let response = widgets::dialog_footer(
                    ui,
                    widgets::FooterButtons::commit(tr("Save")).destructive(tr("Discard changes")),
                    |_| {},
                );
                if response.cancel {
                    choice = Some(0);
                } else if response.destructive {
                    choice = Some(1);
                } else if response.commit {
                    choice = Some(2);
                }
            },
        );
        match choice {
            Some(0) => {
                self.close_tab = None;
                self.close_app = false;
            }
            Some(1) | Some(2) => {
                let save = choice == Some(2);
                if self.close_app {
                    if save {
                        for index in 0..self.sessions.len() {
                            if self.sessions[index].history.dirty() {
                                self.current = index;
                                if !self.save_current(false) {
                                    return;
                                }
                            }
                        }
                    }
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else if let Some(index) = self.close_tab {
                    if save {
                        self.current = index;
                        if !self.save_current(false) {
                            return;
                        }
                    }
                    self.remember_closed(index);
                    self.sessions.remove(index);
                    self.current = self.current.min(self.sessions.len().saturating_sub(1));
                    self.close_tab = None;
                }
            }
            _ => {}
        }
    }
}

fn channel_picker(ui: &mut egui::Ui, channel: &mut usize) {
    widgets::PopUp::from_id_salt("adjustment_channel")
        .selected_text(["RGB", tr("Red"), tr("Green"), tr("Blue")][*channel])
        .show_ui(ui, |ui| {
            for (index, name) in ["RGB", tr("Red"), tr("Green"), tr("Blue")]
                .iter()
                .enumerate()
            {
                widgets::menu_choice(ui, channel, index, *name);
            }
        });
}

fn curve_editor(ui: &mut egui::Ui, points: &mut Vec<Point>) -> bool {
    let mut changed = false;

    ui.label(
        RichText::new(tr(
            "Click to add a point · Drag points to reshape the curve",
        ))
        .color(ui.palette().muted),
    );
    let (rect, response) =
        ui.allocate_exact_size(vec2(360.0, 220.0), egui::Sense::click_and_drag());
    let palette = ui.palette();
    ui.painter().rect_filled(rect, 4.0, palette.plot);
    for i in 1..4 {
        let t = i as f32 / 4.0;
        ui.painter().line_segment(
            [
                rect.left_top() + vec2(rect.width() * t, 0.0),
                rect.left_bottom() + vec2(rect.width() * t, 0.0),
            ],
            Stroke::new(1.0_f32, palette.plot_grid),
        );
        ui.painter().line_segment(
            [
                rect.left_top() + vec2(0.0, rect.height() * t),
                rect.right_top() + vec2(0.0, rect.height() * t),
            ],
            Stroke::new(1.0_f32, palette.plot_grid),
        );
    }
    let map = |p: Point| {
        egui::pos2(
            rect.left() + p.x * rect.width(),
            rect.bottom() - p.y * rect.height(),
        )
    };
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_top()],
        Stroke::new(1.0_f32, palette.plot_diagonal),
    );
    ui.painter().add(egui::Shape::line(
        (0..=255)
            .map(|i| {
                let x = i as f32 / 255.0;
                map(Point::new(x, effects::curve_value(points, x)))
            })
            .collect(),
        Stroke::new(1.5_f32, palette.text),
    ));
    for p in points.iter() {
        ui.painter().circle_filled(map(*p), 3.0, palette.text);
    }
    if let Some(p) = response.interact_pointer_pos() {
        let point = Point::new(
            ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
            ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0),
        );
        if response.clicked()
            && points.len() < 32
            && !points.iter().any(|v| (v.x - point.x).abs() < 0.025)
        {
            points.push(point);
            points.sort_by(|a, b| a.x.total_cmp(&b.x));
            changed = true;
        }
        if response.dragged()
            && let Some(index) = points
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (a.x - point.x).abs().total_cmp(&(b.x - point.x).abs()))
                .map(|(i, _)| i)
        {
            points[index].y = point.y;
            changed = true;
        }
    }
    changed
}

/// An estimated file size for the Export dialog: "12 KiB", "1.4 MiB".
pub(super) fn estimated_size(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    let bytes = bytes as f64;
    if bytes < KIB * KIB {
        format!("{} KiB", (bytes / KIB).ceil().max(1.0))
    } else {
        format!("{:.1} MiB", bytes / (KIB * KIB))
    }
}

/// Xuan's icon for About, decoded from the bundled 128 px PNG once per context.
fn app_icon(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("about_app_icon");
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let icon = image::load_from_memory(include_bytes!(
        "../../assets/icons/hicolor/128x128/apps/me.silverl.xuan.png"
    ))
    .expect("bundled application icon")
    .to_rgba8();
    let size = [icon.width() as usize, icon.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, icon.as_raw());
    let texture = ctx.load_texture("about_app_icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    texture
}

/// About's links: the project page, the licence notices and a new issue, on `repository`'s
/// GitHub. A release links the notices at its own tag, other builds at the main branch.
pub(super) fn about_links(
    repository: &str,
    build: &xuan::buildinfo::BuildInfo,
) -> [(&'static str, String); 3] {
    let site = format!("https://github.com/{repository}");
    let tree = if build.release {
        format!("v{}", build.version)
    } else {
        "main".to_owned()
    };
    [
        ("Project site", site.clone()),
        (
            "Licences and third-party notices",
            format!("{site}/blob/{tree}/THIRD_PARTY.md"),
        ),
        ("Report a bug", format!("{site}/issues/new")),
    ]
}

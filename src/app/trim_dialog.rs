//! Image → Trim…, after Photoshop's Trim and Compositor's Trim sheet.
use xuan::{
    i18n::tr,
    operations::{self, TrimBasis, TrimSides},
};

use super::{Dialog, EditorApp, widgets};

/// The dialog's choices, remembered for the next time it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TrimSettings {
    pub basis: TrimBasis,
    pub sides: TrimSides,
}

impl Default for TrimSettings {
    fn default() -> Self {
        Self {
            basis: TrimBasis::Transparent,
            sides: TrimSides::default(),
        }
    }
}

fn basis_name(basis: TrimBasis) -> &'static str {
    match basis {
        TrimBasis::Transparent => "Transparent Pixels",
        TrimBasis::TopLeft => "Top Left Pixel Colour",
        TrimBasis::BottomRight => "Bottom Right Pixel Colour",
    }
}

impl EditorApp {
    pub(super) fn open_trim(&mut self) {
        if self.session().is_some() {
            self.dialog = Some(Dialog::Trim);
        }
    }

    /// Trims the canvas by the given choices as one undo step; says so when there is nothing to trim.
    pub(super) fn trim_canvas(&mut self, settings: TrimSettings) {
        let Some(session) = self.session() else {
            return;
        };
        let image = xuan::render::render(&session.document);
        if operations::trim_bounds(&image, settings.basis, settings.sides).is_none() {
            self.status = tr("Nothing to trim").into();
            return;
        }
        self.edit(tr("Trim"), |doc| {
            operations::trim(doc, settings.basis, settings.sides).map(|_| ())
        });
        if let Some(session) = self.session_mut() {
            session.fit = true;
        }
    }

    pub(super) fn trim_dialog(&mut self, ctx: &egui::Context) {
        let mut settings = self.trim_settings;
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        widgets::Window::new(tr("Trim"))
            .id("trim")
            .open(&mut open)
            .default_width(340.0)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.add_space(4.0);
                    ui.label(tr("Based on"));
                    widgets::PopUp::from_id_salt("trim_basis")
                        .selected_text(tr(basis_name(settings.basis)))
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for option in [
                                TrimBasis::Transparent,
                                TrimBasis::TopLeft,
                                TrimBasis::BottomRight,
                            ] {
                                widgets::menu_choice(
                                    ui,
                                    &mut settings.basis,
                                    option,
                                    tr(basis_name(option)),
                                );
                            }
                        });
                    ui.add_space(8.0);
                    ui.label(tr("Trim away"));
                    widgets::checkbox(ui, &mut settings.sides.top, tr("Top"));
                    widgets::checkbox(ui, &mut settings.sides.bottom, tr("Bottom"));
                    widgets::checkbox(ui, &mut settings.sides.left, tr("Left"));
                    widgets::checkbox(ui, &mut settings.sides.right, tr("Right"));
                    let sides = settings.sides;
                    sides.top || sides.bottom || sides.left || sides.right
                },
                |ui, any| {
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(tr("Trim")).enabled(any),
                        |_| {},
                    );
                    apply = response.commit;
                    cancel = response.cancel;
                },
            );
        self.trim_settings = settings;
        if apply {
            self.dialog = None;
            self.trim_canvas(settings);
        } else if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dialog = None;
        }
    }
}

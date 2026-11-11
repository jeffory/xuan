//! Edit → Stroke…: a line of colour along the selection's edge on the active pixel layer, after
//! Photoshop's Stroke dialog. The line shows live on the canvas as the settings change; Apply
//! keeps it as one undo step and Cancel puts the layer back.
//!
//! Growing and shrinking a soft selection by a wide line takes a while on a large canvas, so the
//! line is made on a worker, one at a time for the latest width and location; colour, opacity and
//! Preserve Transparency only paint the line already made again.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

use egui::RichText;
use image::GrayImage;
use xuan::{
    document::Document,
    i18n::tr,
    paint::outline::{self, Outline},
    selection_ops::{self, StrokeLocation},
};

use super::theme::PaletteExt as _;
use super::{Dialog, EditorApp, widgets};

/// The choices the dialog remembers for the next time it opens. The colour is not among
/// them: it starts as the foreground colour each time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct StrokeSettings {
    pub width: u32,
    pub location: StrokeLocation,
    pub opacity: f32,
    pub preserve_transparency: bool,
}

impl Default for StrokeSettings {
    fn default() -> Self {
        Self {
            width: 4,
            location: StrokeLocation::Outside,
            opacity: 1.0,
            preserve_transparency: false,
        }
    }
}

/// What a line is made for: its width and location.
type Shape = (u32, StrokeLocation);

/// A line being made on a worker; dropping it cancels the worker.
struct CoverageJob {
    shape: Shape,
    receive: Receiver<Option<GrayImage>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for CoverageJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The open dialog.
pub(super) struct StrokeEdit {
    pub outline: Outline,
    /// The document as it was when the dialog opened; each preview paints a copy of it.
    original: Document,
    /// The line last made, and what it was made for.
    coverage: Option<(Shape, Arc<GrayImage>)>,
    job: Option<CoverageJob>,
    /// The settings the canvas shows, once a line has been painted.
    shown: Option<Outline>,
    /// Apply was pressed: the stroke is kept as soon as the canvas shows the settings.
    applying: bool,
}

impl StrokeEdit {
    /// Whether the canvas is still to catch up with the settings.
    pub fn busy(&self) -> bool {
        self.shown != Some(self.outline)
    }

    fn shape(&self) -> Shape {
        (self.outline.width, self.outline.location)
    }
}

impl EditorApp {
    /// Opens Edit → Stroke… with the foreground colour and the remembered settings, and starts
    /// on the line for them.
    pub(super) fn open_stroke(&mut self) {
        if !self.command_enabled("stroke") {
            return;
        }
        let settings = self.stroke_settings;
        let [r, g, b, _] = self.brush.color;
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.begin(tr("Stroke"), &session.document);
        let mut edit = StrokeEdit {
            outline: Outline {
                width: settings.width,
                location: settings.location,
                color: [r, g, b],
                opacity: settings.opacity,
                preserve_transparency: settings.preserve_transparency,
            },
            original: session.document.clone(),
            coverage: None,
            job: None,
            shown: None,
            applying: false,
        };
        self.dialog = Some(Dialog::Stroke);
        if self.update_stroke_preview(&mut edit) {
            self.stroke = Some(edit);
        }
    }

    /// Brings the canvas up to the dialog's settings: takes a finished line, starts on one for
    /// a new width or location, and paints the line when it is ready. On an error the edit is
    /// undone, the error shown and the dialog closed, and this returns false.
    pub(super) fn update_stroke_preview(&mut self, edit: &mut StrokeEdit) -> bool {
        let shape = edit.shape();
        if let Some(job) = &edit.job {
            match job.receive.try_recv() {
                Ok(coverage) => {
                    let job = edit.job.take().unwrap();
                    if let Some(coverage) = coverage {
                        edit.coverage = Some((job.shape, Arc::new(coverage)));
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    return self.fail_stroke(tr("The stroke stopped unexpectedly").into());
                }
            }
        }
        let ready = (edit.coverage.as_ref())
            .filter(|(made, _)| *made == shape)
            .map(|(_, coverage)| coverage.clone());
        let Some(coverage) = ready else {
            if edit.job.as_ref().is_none_or(|job| job.shape != shape) {
                // Replacing a job cancels the line for settings that have gone.
                edit.job = Some(self.make_coverage(&edit.original, shape));
            }
            return true;
        };
        if edit.shown == Some(edit.outline) {
            return true;
        }
        let mut document = edit.original.clone();
        if let Err(error) = outline::paint(&mut document, &coverage, &edit.outline) {
            return self.fail_stroke(error.to_string());
        }
        if let Some(session) = self.session_mut() {
            session.document = document;
            session.invalidate();
        }
        edit.shown = Some(edit.outline);
        true
    }

    /// Starts a worker on the line along `document`'s selection.
    fn make_coverage(&self, document: &Document, shape: Shape) -> CoverageJob {
        let selection = document.selection.clone().unwrap_or_default();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (send, receive) = mpsc::channel();
        let context = self.context.clone();
        std::thread::spawn(move || {
            let coverage =
                selection_ops::stroke_coverage(&selection, shape.0, shape.1, &worker_cancel);
            let _ = send.send(coverage);
            context.request_repaint();
        });
        CoverageJob {
            shape,
            receive,
            cancel,
        }
    }

    fn fail_stroke(&mut self, error: String) -> bool {
        if let Some(session) = self.session_mut() {
            session.history.cancel(&mut session.document);
            session.invalidate();
        }
        self.error = Some(error);
        self.stroke = None;
        self.dialog = None;
        false
    }

    pub(super) fn stroke_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.stroke.take() else {
            self.dialog = None;
            return;
        };
        let mut open = true;
        let mut ok = false;
        let mut cancel = false;
        let busy = edit.busy();
        let applying = edit.applying;
        widgets::Window::new(tr("Stroke"))
            .id("stroke")
            .open(&mut open)
            .default_width(320.0)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.add_enabled_ui(!applying, |ui| {
                        stroke_controls(ui, &mut edit.outline);
                    });
                },
                |ui, ()| {
                    let response = widgets::dialog_footer(
                        ui,
                        widgets::FooterButtons::commit(tr("Apply")).enabled(!applying),
                        |ui| {
                            if busy {
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
                    ok = response.commit;
                    cancel = response.cancel;
                },
            );
        if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(session) = self.session_mut() {
                session.history.cancel(&mut session.document);
                session.invalidate();
            }
            self.dialog = None;
            return;
        }
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.wants_keyboard_input();
        edit.applying |= ok || enter;
        if !self.update_stroke_preview(&mut edit) {
            return;
        }
        if edit.applying && !edit.busy() {
            let outline = edit.outline;
            self.stroke_settings = StrokeSettings {
                width: outline.width,
                location: outline.location,
                opacity: outline.opacity,
                preserve_transparency: outline.preserve_transparency,
            };
            if let Some(session) = self.session_mut() {
                session.commit();
            }
            self.status = tr("Stroke").into();
            self.dialog = None;
            return;
        }
        self.stroke = Some(edit);
    }
}

/// The dialog's settings.
fn stroke_controls(ui: &mut egui::Ui, outline: &mut Outline) {
    ui.add(
        widgets::Slider::new(&mut outline.width, 1..=selection_ops::MAX_STROKE_WIDTH)
            .logarithmic(true)
            .text(tr("Width"))
            .suffix(" px"),
    );
    ui.horizontal(|ui| {
        ui.label(tr("Colour"));
        let [r, g, b] = outline.color;
        let mut rgba = [r, g, b, 255];
        if widgets::color_well(ui, &mut rgba).changed() {
            outline.color = [rgba[0], rgba[1], rgba[2]];
        }
    });
    ui.horizontal(|ui| {
        ui.label(tr("Location"));
        widgets::segmented(
            ui,
            &mut outline.location,
            &[
                (StrokeLocation::Inside, tr("Inside")),
                (StrokeLocation::Center, tr("Centre")),
                (StrokeLocation::Outside, tr("Outside")),
            ],
        );
    });
    ui.add(
        widgets::Slider::new(&mut outline.opacity, 0.0..=1.0)
            .text(tr("Opacity"))
            .percentage(),
    );
    widgets::checkbox(
        ui,
        &mut outline.preserve_transparency,
        tr("Preserve transparency"),
    );
}

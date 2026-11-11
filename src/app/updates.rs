//! Help → Check for Updates… and the daily check of Settings → General → Check for updates
//! (issue 118). A check runs [`update::fetch_latest`] on its own thread, so neither startup nor
//! the UI waits for the network, and its answer is picked up by [`EditorApp::poll_updates`].
//! A check the user asked for shows whatever it found; the daily one shows only a newer release
//! that was not skipped, and says nothing when it fails. See "Updates" in `docs/USAGE.md`.
use std::sync::{Arc, mpsc};

use anyhow::{Result, bail};
use egui::RichText;
use url::Url;
use xuan::{
    i18n::tr,
    plugins::models::{Https, Reply, Transport},
    update::{self, Block, Release},
};

use super::{Dialog, EditorApp, theme::PaletteExt as _, widgets};

/// Checks for updates: the one under way, what the dialog shows, and how checks reach GitHub.
#[derive(Default)]
pub(super) struct Updates {
    pub(super) running: Option<Running>,
    /// What the update dialog shows; it opens whenever this is set and no other dialog is.
    pub(super) view: Option<View>,
    /// How checks reach GitHub. Tests put a fake here; otherwise https is used, except in
    /// tests, which have no network.
    pub(super) transport: Option<Arc<dyn Transport>>,
}

pub(super) struct Running {
    answer: mpsc::Receiver<Result<Release, String>>,
    /// Asked for from the Help menu, so any answer is shown, failures included.
    pub(super) manual: bool,
}

/// What the update dialog shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum View {
    Checking,
    /// A newer release, and its notes ready to draw.
    Available(Release, Vec<Block>),
    UpToDate,
    Failed(String),
}

/// What the transport answers in tests that did not set one: there is no network.
struct NoNetwork;

impl Transport for NoNetwork {
    fn get(&self, url: &Url, _size: u64) -> Result<Reply> {
        bail!("Tests have no network: {url}")
    }
}

impl Updates {
    fn transport(&mut self) -> Arc<dyn Transport> {
        self.transport
            .get_or_insert_with(|| {
                if cfg!(test) {
                    Arc::new(NoNetwork)
                } else {
                    Arc::new(Https::new().with_limit(update::TIME_LIMIT))
                }
            })
            .clone()
    }
}

impl EditorApp {
    /// At startup: check when Settings → General → Check for updates is on and a day has passed
    /// since the last check. Screenshot runs never check.
    pub(super) fn start_daily_update_check(&mut self) {
        let settings = &self.config.updates;
        if settings.check
            && self.screenshot.is_none()
            && update::due(settings.last_check, update::now())
        {
            self.begin_update_check(false);
        }
    }

    /// Help → Check for Updates…: check now and show the answer, whatever it is.
    pub(super) fn check_for_updates(&mut self) {
        self.updates.view = Some(View::Checking);
        self.dialog = Some(Dialog::Update);
        self.begin_update_check(true);
    }

    fn begin_update_check(&mut self, manual: bool) {
        if let Some(running) = &mut self.updates.running {
            running.manual |= manual;
            return;
        }
        let mut config = self.config.clone();
        config.updates.last_check = Some(update::now());
        self.config = config;
        self.save_config();
        let transport = self.updates.transport();
        let context = self.context.clone();
        let (send, answer) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("update check".into())
            .spawn(move || {
                let result = update::fetch_latest(transport.as_ref(), update::REPOSITORY)
                    .map_err(|error| format!("{error:#}"));
                let _ = send.send(result);
                context.request_repaint();
            });
        match spawned {
            Ok(_) => self.updates.running = Some(Running { answer, manual }),
            Err(error) if manual => self.updates.view = Some(View::Failed(error.to_string())),
            Err(_) => {}
        }
    }

    /// Picks up the answer of a check that finished, and opens the update dialog when there is
    /// something to show and no other dialog is open.
    pub(super) fn poll_updates(&mut self) {
        // Closing or replacing the dialog while checking makes the check a quiet one.
        if self.dialog != Some(Dialog::Update)
            && !matches!(self.updates.view, None | Some(View::Available(..)))
        {
            self.updates.view = None;
            if let Some(running) = &mut self.updates.running {
                running.manual = false;
            }
        }
        if let Some(running) = &self.updates.running {
            let answer = match running.answer.try_recv() {
                Ok(answer) => Some(answer),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(tr("The update check stopped unexpectedly.").to_owned()))
                }
            };
            if let Some(answer) = answer {
                let manual = running.manual;
                self.updates.running = None;
                self.show_update_answer(answer, manual);
            }
        }
        if self.dialog.is_none()
            && self.job.is_none()
            && matches!(self.updates.view, Some(View::Available(..)))
        {
            self.dialog = Some(Dialog::Update);
        }
    }

    fn show_update_answer(&mut self, answer: Result<Release, String>, manual: bool) {
        let Some(current) = update::current_version() else {
            return;
        };
        let skipped = self.config.updates.skipped.as_deref();
        self.updates.view = match answer {
            Ok(release) if manual && update::is_newer(&release.version, &current) => {
                let blocks = update::blocks(&release.notes);
                Some(View::Available(release, blocks))
            }
            Ok(release) if update::offer(&release, &current, skipped) => {
                let blocks = update::blocks(&release.notes);
                Some(View::Available(release, blocks))
            }
            Ok(_) if manual => Some(View::UpToDate),
            Err(error) if manual => Some(View::Failed(error)),
            // The daily check is quiet unless it found something.
            _ => None,
        };
    }

    /// The update dialog: checking, a newer release with its notes, up to date, or a failure.
    pub(super) fn update_dialog(&mut self, ctx: &egui::Context) {
        let Some(view) = self.updates.view.clone() else {
            self.dialog = None;
            return;
        };
        let mut open = true;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut download = false;
        let mut skip = false;
        let current = env!("CARGO_PKG_VERSION");
        widgets::Window::new(tr("Software update"))
            .id("software_update")
            .default_width(if matches!(view, View::Available(..)) {
                520.0
            } else {
                380.0
            })
            .open(&mut open)
            .show_with_footer(
                ctx,
                |ui| match &view {
                    View::Checking => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tr("Checking for updates…"));
                        });
                    }
                    View::UpToDate => {
                        ui.label(RichText::new(tr("Xuan is up to date.")).strong());
                        ui.add_space(6.0);
                        ui.label(format!("{} {current}", tr("Version")));
                    }
                    View::Failed(error) => {
                        ui.label(RichText::new(tr("Couldn't check for updates.")).strong());
                        ui.add_space(6.0);
                        ui.label(tr("Check your connection and try again later."));
                        ui.add_space(6.0);
                        let text = RichText::new(error).small().color(ui.palette().muted);
                        ui.add(egui::Label::new(text).wrap());
                    }
                    View::Available(release, blocks) => {
                        ui.label(
                            RichText::new(tr("A new version of Xuan is available."))
                                .strong(),
                        );
                        ui.add_space(6.0);
                        ui.label(format!(
                            "{} {} · {} {current}",
                            tr("New version"),
                            release.version,
                            tr("Installed version"),
                        ));
                        ui.add_space(12.0);
                        if release.title != release.tag {
                            ui.label(RichText::new(&release.title).heading());
                            ui.add_space(6.0);
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("release_notes")
                            .max_height(320.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| release_notes(ui, blocks));
                        ui.add_space(8.0);
                        let note = tr(
                            "Download opens the release page in your browser. Nothing is downloaded or installed until you choose a file there.",
                        );
                        let note = RichText::new(note).small().color(ui.palette().muted);
                        ui.add(egui::Label::new(note).wrap());
                    }
                },
                |ui, ()| {
                    let response = match &view {
                        View::Available(..) => widgets::dialog_footer(
                            ui,
                            widgets::FooterButtons::commit(tr("Download"))
                                .cancel_label(tr("Later")),
                            |ui| {
                                skip = widgets::button(ui, tr("Skip This Version")).clicked();
                            },
                        ),
                        View::Checking => widgets::dialog_footer(
                            ui,
                            widgets::FooterButtons::cancel_only(),
                            |_| {},
                        ),
                        _ => widgets::dialog_footer(
                            ui,
                            widgets::FooterButtons::single(tr("OK")),
                            |_| {},
                        ),
                    };
                    download = response.commit && matches!(view, View::Available(..));
                    close |= response.commit || response.cancel;
                },
            );
        if let View::Available(release, _) = &view {
            if download {
                ctx.open_url(egui::OpenUrl::new_tab(&release.page));
            }
            if skip {
                let mut config = self.config.clone();
                config.updates.skipped = Some(release.version.to_string());
                self.config = config;
                self.save_config();
            }
        }
        if !open || close || skip {
            self.updates.view = None;
            self.dialog = None;
            if let Some(running) = &mut self.updates.running {
                running.manual = false;
            }
        }
    }
}

/// Release notes, drawn from their [`Block`]s. Links show as text and are not followed.
fn release_notes(ui: &mut egui::Ui, blocks: &[Block]) {
    if blocks.is_empty() {
        ui.label(RichText::new(tr("This release has no notes.")).color(ui.palette().muted));
        return;
    }
    ui.spacing_mut().item_spacing.y = 4.0;
    for block in blocks {
        match block {
            Block::Heading(level, text) => {
                ui.add_space(6.0);
                let size = if *level <= 2 { 15.0 } else { 13.0 };
                ui.add(egui::Label::new(RichText::new(text).strong().size(size)).wrap());
            }
            Block::Item {
                marker,
                depth,
                text,
            } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(4.0 + 14.0 * *depth as f32);
                    ui.label(marker.as_deref().unwrap_or("•"));
                    ui.add(egui::Label::new(text).wrap());
                });
            }
            Block::Code(text) => {
                egui::Frame::new()
                    .fill(ui.palette().field)
                    .inner_margin(6.0)
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
                    });
            }
            Block::Paragraph(text) => {
                ui.add(egui::Label::new(text).wrap());
            }
            Block::Rule => {
                ui.separator();
            }
        }
    }
}

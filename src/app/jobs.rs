use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{self, Receiver},
};
use xuan::i18n::tr;

use anyhow::Result;
use uuid::Uuid;
use xuan::document::Document;

use super::EditorApp;

pub(super) struct Job {
    pub name: String,
    target: Uuid,
    receive: Receiver<Result<Document, String>>,
    pub cancel: Arc<AtomicBool>,
    /// The fraction done (an `f32`'s bits), for jobs that report it.
    progress: Arc<AtomicU32>,
}

impl Job {
    /// The fraction done, once the job has reported any.
    pub fn progress(&self) -> Option<f32> {
        let value = f32::from_bits(self.progress.load(Ordering::Relaxed));
        (value > 0.0).then_some(value.min(1.0))
    }
}

impl EditorApp {
    pub(super) fn start_job(
        &mut self,
        name: &str,
        operation: impl FnOnce(&mut Document, &AtomicBool) -> Result<()> + Send + 'static,
    ) {
        self.start_progress_job(name, move |document, _, cancel| operation(document, cancel));
    }

    /// Like [`EditorApp::start_job`], for an operation that reports how far it got
    /// (0–1); the job window shows a progress bar.
    pub(super) fn start_progress_job(
        &mut self,
        name: &str,
        operation: impl FnOnce(&mut Document, &(dyn Fn(f32) + Sync), &AtomicBool) -> Result<()>
        + Send
        + 'static,
    ) {
        if self.job.is_some() {
            return;
        }
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.begin(name, &session.document);
        let mut document = session.document.clone();
        let target = document.id;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let worker_progress = progress.clone();
        let repaint = self.context.clone();
        let (send, receive) = mpsc::channel();
        let context = self.context.clone();
        xuan::gpu::spawn(move || {
            let _cancel = xuan::gpu::cancellation(worker_cancel.clone());
            let report = move |fraction: f32| {
                worker_progress.store(fraction.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                repaint.request_repaint();
            };
            let result = operation(&mut document, &report, &worker_cancel)
                .map(|()| document)
                .map_err(|e| e.to_string());
            let _ = send.send(result);
            context.request_repaint();
        });
        self.job = Some(Job {
            name: name.into(),
            target,
            receive,
            cancel,
            progress,
        });
    }

    pub(super) fn poll_job(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.receive.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(tr("The editing worker stopped unexpectedly").into())
            }
        };
        let job = self.job.take().unwrap();
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|s| s.document.id == job.target)
        {
            match result {
                Ok(document) if !job.cancel.load(Ordering::Relaxed) => {
                    session.document = document;
                    session.document.promote_image_masks();
                    session.history.commit();
                    self.status = job.name;
                }
                result => {
                    session.history.cancel(&mut session.document);
                    if job.cancel.load(Ordering::Relaxed) {
                        self.status = tr("Cancelled").into();
                    } else if let Err(error) = result {
                        self.error = Some(error);
                    }
                }
            }
            session.invalidate();
        }
    }
}

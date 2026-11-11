//! `raw/develop`: a plugin develops a camera RAW layer again with new
//! settings. Decoding and rendering a whole camera file takes seconds, so it
//! runs as a background job like Content-Aware Fill, and the request is
//! answered when the job ends rather than at once. See
//! `xuan::plugins::raw` for the checks and the work.
use serde_json::{Value, json};
use uuid::Uuid;
use xuan::{
    i18n::tr,
    plugins::{
        edits,
        manifest::DocumentAccess,
        protocol::{self, Id, Request, RpcError},
        raw,
    },
};

use super::{EditorApp, plugins::one_line};

impl EditorApp {
    /// Serve a plugin request that may go ahead: answer it now, or for
    /// `raw/develop`, start its job, which answers when it ends.
    pub(super) fn serve_request(&mut self, plugin: &str, request: Request) {
        if request.method == "raw/develop" {
            if let Err(error) = self.start_raw_develop(plugin, &request)
                && let Some(process) = self.plugins.process_mut(plugin)
            {
                let _ = process.respond(request.id, Err(error));
            }
            return;
        }
        let result = self.service_request(plugin, &request);
        if let Some(process) = self.plugins.process_mut(plugin) {
            let _ = process.respond(request.id, result);
        }
    }

    /// Check a `raw/develop` request and start its job on the current
    /// document. An error is the answer; otherwise the job answers.
    fn start_raw_develop(&mut self, plugin: &str, request: &Request) -> Result<(), RpcError> {
        if let Some(error) = self.edit_refused(plugin, request) {
            return Err(error);
        }
        let edits_allowed = (self.plugins.manifest(plugin))
            .is_some_and(|m| m.permissions.document == DocumentAccess::Edit);
        if !edits_allowed {
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                "The manifest does not declare document = \"edit\"",
            ));
        }
        if self.dialog.is_some()
            || self.job.is_some()
            || self.gesture.is_some()
            || self.develop.is_some()
        {
            return Err(RpcError::new(
                protocol::INVALID_REQUEST,
                "The editor is busy",
            ));
        }
        let Some(session) = self.session() else {
            return Err(RpcError::new(
                protocol::INVALID_PARAMS,
                "No document is open",
            ));
        };
        let develop = raw::parse(&session.document, &request.params)
            .map_err(|error| RpcError::invalid_params(format!("{error:#}")))?;
        let name = (request.params.get("name").and_then(Value::as_str))
            .map(|name| one_line(name, 120))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| tr("Develop RAW").to_owned());
        let work = develop.clone();
        self.cancel_gesture();
        self.start_job(&name, move |document, cancel| {
            raw::develop(document, &work, xuan::raw::decode, cancel)
        });
        let Some(job) = &mut self.job else {
            return Err(RpcError::new(
                protocol::INTERNAL_ERROR,
                "The develop could not start",
            ));
        };
        job.reply = Some((plugin.to_owned(), request.id.clone(), develop.layer));
        Ok(())
    }

    /// Open a camera RAW file for a plugin as a new document developed with
    /// the default settings, without the Develop tab, whose Apply button a
    /// plugin cannot press. `raw/develop` changes the settings afterwards.
    pub(super) fn open_raw_developed(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        let (asset, decoded) = xuan::raw::open(path)?;
        let pixels = xuan::gpu::scope(self.processor.clone(), || {
            xuan::raw::render(
                &decoded,
                &asset.settings,
                &std::sync::atomic::AtomicBool::new(false),
            )
        })?;
        self.apply_developed(&super::develop::DevelopTarget::New, asset, pixels)
    }

    /// Answer the plugin whose request a job ran for: the developed layer as
    /// `document/get` describes it, or why there is none.
    pub(super) fn answer_job_reply(
        &mut self,
        plugin: &str,
        id: Id,
        document: Uuid,
        layer: Uuid,
        cancelled: bool,
        failed: Option<String>,
    ) {
        let result = if cancelled {
            Err(RpcError::new(
                protocol::CANCELLED,
                "The user cancelled developing the RAW layer",
            ))
        } else if let Some(error) = failed {
            Err(RpcError::new(protocol::INTERNAL_ERROR, error))
        } else {
            (self.sessions.iter())
                .find(|s| s.document.id == document)
                .and_then(|session| {
                    let document = &session.document;
                    let layer = document.layers.iter().find(|l| l.id == layer)?;
                    Some(json!({"ok": true, "layer": edits::describe_layer(document, layer)}))
                })
                .ok_or_else(|| {
                    RpcError::new(
                        protocol::INVALID_PARAMS,
                        "The document or layer was closed while it was developed",
                    )
                })
        };
        if let Some(process) = self.plugins.process_mut(plugin) {
            let _ = process.respond(id, result);
        }
    }
}

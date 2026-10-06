//! Xuan's MCP server: a plugin that serves the Model Context Protocol on
//! 127.0.0.1 so LLM clients such as Claude Code can see and edit the open
//! documents. See `docs/AGENTS-GUIDE.md` and `docs/MCP.md` in the Xuan
//! repository.
mod auth;
mod editor;
mod pane;
mod server;
mod tools;

#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use xuan_plugin::{Host, Plugin, Settings};

use crate::server::{DEFAULT_PORT, Shared, Xuan};

/// The pane's id in `plugin.toml`.
const PANE: &str = "status";

/// The state, made when `initialize` brings the data folder.
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn port(settings: &Settings) -> u16 {
    settings
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or(DEFAULT_PORT)
}

fn apply_settings(settings: &Settings) {
    let shared = SHARED.get_or_init(|| {
        let token = auth::load_or_create(&settings.data_dir).unwrap_or_else(|error| {
            eprintln!("cannot store the token in the data folder: {error}");
            auth::generate()
        });
        let shared = Shared::new(token, settings.data_dir.clone());
        if let Ok(mut current) = shared.port.lock() {
            *current = port(settings);
        }
        Arc::new(shared)
    });
    let wanted = port(settings);
    if let Ok(mut current) = shared.port.lock()
        && *current != wanted
    {
        *current = wanted;
        shared
            .any_port
            .store(false, std::sync::atomic::Ordering::Relaxed);
        // Listen again on the new port (a permit waits if it is starting).
        shared.restart.notify_one();
    }
}

/// Draw the pane again from a background thread.
fn refresher(host: Host, shared: Arc<Shared>) -> Box<dyn Fn() + Send> {
    Box::new(move || {
        let (host, shared) = (host.clone(), shared.clone());
        std::thread::spawn(move || {
            let permissions = host.request("session/status", json!({})).ok();
            let tree = pane::tree(&shared, permissions.as_ref());
            host.notify("pane/update", json!({"pane": PANE, "tree": tree}));
        });
    })
}

fn main() {
    Plugin::new()
        .on_settings(apply_settings)
        .on_start(|host, settings| {
            apply_settings(&settings);
            let Some(shared) = SHARED.get().cloned() else {
                return;
            };
            if let Ok(mut changed) = shared.changed.lock() {
                *changed = Some(refresher(host.clone(), shared.clone()));
            }
            server::run(Xuan {
                editor: Arc::new(editor::HostEditor(host)),
                shared,
            });
        })
        .pane(PANE, |pane| {
            let Some(shared) = SHARED.get() else {
                return Ok(xuan_plugin::ui::column(vec![xuan_plugin::ui::muted(
                    "Starting…",
                )]));
            };
            if pane.widget.as_deref() == Some("any_port") {
                // The user saw the warning and chose to move.
                shared
                    .any_port
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                shared.restart.notify_one();
            }
            if pane.widget.as_deref() == Some("new_token") {
                let dir = shared
                    .data_dir
                    .lock()
                    .map(|d| d.clone())
                    .unwrap_or_default();
                let token = auth::replace(&dir).unwrap_or_else(|_| auth::generate());
                if let Ok(mut current) = shared.token.write() {
                    *current = token;
                }
            }
            let permissions = pane.host.request("session/status", json!({})).ok();
            Ok(pane::tree(shared, permissions.as_ref()))
        })
        .run();
}

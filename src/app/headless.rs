//! `xuan --headless`: the editor without a window, serving the MCP Server
//! plugin so that agents can drive Xuan on a machine with no display, over
//! SSH, or in a container.
//!
//! Nothing here touches the display: no window, no winit event loop, no wgpu
//! device or surface, so no EGL, Vulkan or GL is loaded. Documents are edited
//! by the same code as in the window, with the CPU paths the editor already
//! uses when it has no GPU. The loop does per frame what the window's frame
//! does apart from drawing: it services plugin requests, finishes jobs and
//! records history.
//!
//! Prompts nobody can answer are settled by the command line: launching
//! headless is the user's go-ahead for the plugin's edits, and `--allow-files`
//! for opening and writing the files the client names. The user's settings
//! are read but never written; the plugin's grant lasts for this run only.
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use xuan::plugins::protocol::{self, RpcError};

use super::{
    EditorApp,
    plugin_files::{FileAction, FileAnswer},
    plugins::{grant_for, pane_key},
};

/// The plugin a headless Xuan serves.
pub const MCP_PLUGIN: &str = "mcp-server";
/// The MCP Server's pane: rendering it starts the plugin, as opening it does.
const MCP_PANE: &str = "status";

/// What the command line allowed.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Open the files clients ask to open, and write files to the paths they
    /// name, without a prompt.
    pub allow_files: bool,
    /// Listen on this port instead of the plugin's setting (0: any free one).
    pub port: Option<u16>,
    /// Start with the sample composition.
    pub demo: bool,
}

/// The editor's headless state.
#[derive(Clone, Debug)]
pub struct Headless {
    pub allow_files: bool,
}

/// Set by SIGINT and SIGTERM.
static STOP: AtomicBool = AtomicBool::new(false);

/// Run until interrupted.
pub fn run(paths: Vec<PathBuf>, options: Options) -> Result<()> {
    attach_console();
    install_signal_handlers();
    let context = egui::Context::default();
    // Plugin messages and finished jobs ask for a repaint; here that wakes
    // the loop.
    let (wake, woken) = mpsc::channel::<()>();
    context.set_request_repaint_callback(move |_| {
        let _ = wake.send(());
    });
    let mut app = EditorApp::with_context(&context, vec![], options.demo, None);
    app.headless = Some(Headless {
        allow_files: options.allow_files,
    });
    app.load_config();
    app.load_plugins();
    // Read, never written: a headless run must not change the settings of
    // the Xuan the user runs with a window.
    app.config_path = None;
    report(&mut app);
    app.grant_headless_plugin(options.port, options.allow_files)?;
    for path in &paths {
        app.open_headless(path);
        report(&mut app);
    }
    let data_dir = app
        .plugins
        .data_dir(MCP_PLUGIN)
        .context("Cannot make the MCP Server's data folder")?;
    let connection = data_dir.join("connection.json");
    // An old file names the port of an earlier run.
    let _ = std::fs::remove_file(&connection);
    let pane = pane_key(MCP_PLUGIN, MCP_PANE);
    app.render_pane(&pane, "open", None);
    if let Some(error) = app.plugins.panes.get(&pane).and_then(|p| p.error.clone()) {
        bail!("The MCP Server did not start: {error}");
    }
    eprintln!(
        "Xuan {} is running headless; press Ctrl+C to stop.",
        env!("CARGO_PKG_VERSION")
    );
    let started = Instant::now();
    let mut announced = false;
    let mut warned = false;
    let mut restarts = 0;
    let mut log = Vec::new();
    while !STOP.load(Ordering::Relaxed) {
        app.headless_frame();
        report(&mut app);
        log = print_new_lines(log, app.plugins.log(MCP_PLUGIN));
        if !announced && let Some(url) = listening(&connection) {
            announce(&url, &data_dir.join("token"));
            announced = true;
        }
        if !announced && !warned && started.elapsed() > Duration::from_secs(15) {
            eprintln!(
                "Xuan: the MCP Server is not listening yet. If its port is taken, start Xuan with --mcp-port 0 (any free port) or another port."
            );
            warned = true;
        }
        // A plugin that stopped is started again, as opening its pane would.
        if !app.plugins.running(MCP_PLUGIN) && !app.plugins.starting(MCP_PLUGIN) {
            restarts += 1;
            if restarts > 3 {
                bail!("The MCP Server keeps stopping; see its log above");
            }
            eprintln!("Xuan: the MCP Server stopped; starting it again");
            let _ = std::fs::remove_file(&connection);
            announced = false;
            app.plugins.panes.remove(&pane);
            app.render_pane(&pane, "open", None);
        }
        let busy = app.job.is_some() || app.plugins.starting(MCP_PLUGIN);
        let wait = if busy { 50 } else { 500 };
        if woken.recv_timeout(Duration::from_millis(wait)).is_ok() {
            // Several wakes for one batch of messages need one frame.
            while woken.try_recv().is_ok() {}
        }
    }
    eprintln!("Xuan: stopping");
    app.plugins.stop_all();
    Ok(())
}

impl EditorApp {
    /// Let the MCP Server run for this run only, with its edits and the
    /// images it sends allowed: starting Xuan headless is the user's answer
    /// to those prompts. Writing files without asking needs `--allow-files`.
    fn grant_headless_plugin(&mut self, port: Option<u16>, allow_files: bool) -> Result<()> {
        let Some(manifest) = self.plugins.manifest(MCP_PLUGIN).cloned() else {
            bail!(
                "The MCP Server plugin was not found. Release packages include it; in a source checkout build it with `cargo build --release` in plugins/mcp-server and start Xuan with {}=<checkout>/plugins",
                xuan::plugins::PATH_VARIABLE
            );
        };
        let mut grant = grant_for(&manifest);
        grant.edit_without_asking = true;
        grant.send_without_asking = true;
        grant.save_without_asking = allow_files;
        let config = self.config.plugins.entry(MCP_PLUGIN.into()).or_default();
        config.enabled = true;
        config.grant = Some(grant);
        if let Some(port) = port {
            config
                .settings
                .insert("port".into(), toml::Value::Integer(i64::from(port)));
        }
        if self.plugin_offline(MCP_PLUGIN) {
            bail!(
                "Plugins that use the network are turned off in Xuan's settings, and the MCP Server listens on 127.0.0.1. Turn that setting off in Xuan with a window first"
            );
        }
        Ok(())
    }

    /// Open a file from the command line. A camera RAW file opens developed
    /// with the default settings, as there is no Develop tab to apply.
    fn open_headless(&mut self, path: &Path) {
        if xuan::raw::is_raw(path) {
            if let Err(error) = self.open_raw_developed(path) {
                self.error = Some(format!("{}: {error:#}", path.display()));
            }
        } else {
            self.open_path(path, false);
        }
    }

    /// One frame without drawing: what [`EditorApp::show`] does apart from
    /// the interface.
    pub(super) fn headless_frame(&mut self) {
        xuan::gpu::scope(None, || {
            self.poll_job();
            self.poll_plugins();
            self.answer_headless_prompts();
        });
        if self.job.is_none()
            && self.gesture.is_none()
            && let Some(session) = self.session_mut()
        {
            session.commit();
        }
    }

    /// Settle what would wait for a click: an open file prompt follows
    /// `--allow-files` (requests are refused earlier without it), and any
    /// other dialog is closed, as nobody can see it.
    fn answer_headless_prompts(&mut self) {
        if self.plugins.file_prompt.is_some() {
            let answer = if self.headless.as_ref().is_some_and(|h| h.allow_files) {
                FileAnswer::Accept
            } else {
                FileAnswer::Cancel
            };
            self.answer_file(answer);
        }
        if let Some(dialog) = self.dialog.take() {
            eprintln!("Xuan: closed a {dialog:?} dialog that nobody can answer headless");
        }
    }

    /// Why a plugin's file request cannot go ahead headless: there is no save
    /// dialog, and the open and write prompts need `--allow-files`. `None`
    /// with a window, or when the request may go ahead.
    pub(super) fn headless_file_refusal(&self, action: &FileAction) -> Option<RpcError> {
        let headless = self.headless.as_ref()?;
        let message = match action {
            FileAction::Save { .. } => {
                "Xuan is running headless, without a save dialog: give an absolute `path`"
            }
            FileAction::Open { .. } | FileAction::Write(_) if !headless.allow_files => {
                "Xuan is running headless without --allow-files, so it does not open or write files for clients. Restart it with --allow-files to allow that"
            }
            FileAction::Open { .. } | FileAction::Write(_) => return None,
        };
        Some(RpcError::new(protocol::INVALID_REQUEST, message))
    }
}

/// Print what the window would show: errors and the status line.
fn report(app: &mut EditorApp) {
    if let Some(error) = app.error.take() {
        eprintln!("Xuan: {}", error.replace("\n\n", ": "));
    }
    if !app.status.is_empty() && app.status != app.status_shown.0 {
        eprintln!("Xuan: {}", app.status);
        app.status_shown.0 = app.status.clone();
    }
}

/// Print the lines of `now` that `before` did not have. The log keeps its
/// latest lines only, so the old ones may have scrolled off its start.
fn print_new_lines(before: Vec<String>, now: Vec<String>) -> Vec<String> {
    for line in &now[overlap(&before, &now)..] {
        eprintln!("[{MCP_PLUGIN}] {line}");
    }
    now
}

/// How many lines at the start of `now` end `before`.
fn overlap(before: &[String], now: &[String]) -> usize {
    (0..=before.len().min(now.len()))
        .rev()
        .find(|&n| before[before.len() - n..] == now[..n])
        .unwrap_or(0)
}

/// The URL in the plugin's `connection.json`, once it listens.
fn listening(connection: &Path) -> Option<String> {
    let text = std::fs::read_to_string(connection).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("url")?.as_str().map(str::to_owned)
}

/// How to connect a client. The token stays in its file, readable by this
/// user only, rather than in the terminal's scrollback.
fn announce(url: &str, token: &Path) {
    println!("MCP server: {url}");
    println!("Token file: {}", token.display());
    if cfg!(unix) {
        println!(
            "Claude Code: claude mcp add --transport http xuan {url} --header \"Authorization: Bearer $(cat '{}')\"",
            token.display()
        );
    }
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

#[cfg(target_os = "linux")]
fn install_signal_handlers() {
    extern "C" fn stop(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }
    let handler: extern "C" fn(libc::c_int) = stop;
    // SAFETY: the handler only stores to an atomic, which is signal safe.
    unsafe {
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
    }
}

/// Elsewhere Ctrl+C ends the process at once; the plugin stops when its
/// input closes (and on Windows with the job object that holds it).
#[cfg(not(target_os = "linux"))]
fn install_signal_handlers() {}

/// Release builds on Windows have no console of their own: write to the one
/// Xuan was started from.
#[cfg(windows)]
fn attach_console() {
    // SAFETY: a plain Win32 call; failing (no parent console) is harmless.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

#[cfg(not(windows))]
fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_log_lines_are_printed() {
        let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(overlap(&lines(&["a", "b"]), &lines(&["a", "b", "c"])), 2);
        // "a" scrolled off the start of the log.
        assert_eq!(overlap(&lines(&["a", "b"]), &lines(&["b", "c", "d"])), 1);
        assert_eq!(overlap(&lines(&["a", "b"]), &lines(&["x"])), 0);
        assert_eq!(overlap(&[], &lines(&["x"])), 0);
    }
}

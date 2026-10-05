//! A plugin process: spawn it, exchange messages over its pipes, and collect
//! what it prints to stderr.
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{
    manifest::Manifest,
    protocol::{Id, MAX_LINE, Message, RpcError},
};

const MAX_LOG_LINES: usize = 500;

/// Something the plugin sent, or the reason it stopped sending.
#[derive(Debug)]
pub enum Incoming {
    Message(Message),
    /// A line that is not valid JSON-RPC; the plugin is told and the line is logged.
    Invalid(String),
    /// Its stdout closed: the process exited or crashed.
    Closed,
}

pub struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    incoming: Receiver<Incoming>,
    next_id: i64,
    log: Arc<Mutex<VecDeque<String>>>,
    closed: Arc<AtomicBool>,
}

impl Process {
    /// Start the manifest's command in the plugin folder with extra environment.
    pub fn spawn(manifest: &Manifest, env: &[(String, String)]) -> Result<Self> {
        let (program, args) = manifest
            .plugin
            .command
            .split_first()
            .context("plugin has no command")?;
        let program = resolve(program, &manifest.dir);
        let mut command = Command::new(&program);
        command
            .args(args)
            .current_dir(&manifest.dir)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "Cannot start `{}` for plugin {}",
                program.display(),
                manifest.plugin.id
            )
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("plugin stdout")?;
        let stderr = child.stderr.take().context("plugin stderr")?;
        let (send, incoming) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(VecDeque::new()));
        {
            let send = send.clone();
            let closed = closed.clone();
            let log = log.clone();
            std::thread::Builder::new()
                .name(format!("plugin {} stdout", manifest.plugin.id))
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    let mut line = Vec::new();
                    loop {
                        line.clear();
                        let read = reader
                            .by_ref()
                            .take(MAX_LINE as u64 + 1)
                            .read_until(b'\n', &mut line);
                        match read {
                            Ok(0) | Err(_) => break,
                            Ok(n) if n > MAX_LINE => {
                                push_log(&log, "message exceeds 16 MiB; stopping".into());
                                break;
                            }
                            Ok(_) => {}
                        }
                        let text = String::from_utf8_lossy(&line);
                        let text = text.trim();
                        if text.is_empty() {
                            continue;
                        }
                        let incoming = match Message::parse(text) {
                            Ok(message) => Incoming::Message(message),
                            Err(error) => {
                                push_log(&log, format!("invalid message ({error}): {text}"));
                                Incoming::Invalid(text.to_owned())
                            }
                        };
                        if send.send(incoming).is_err() {
                            break;
                        }
                    }
                    closed.store(true, Ordering::Relaxed);
                    let _ = send.send(Incoming::Closed);
                })
                .context("plugin reader thread")?;
        }
        {
            let log = log.clone();
            std::thread::Builder::new()
                .name(format!("plugin {} stderr", manifest.plugin.id))
                .spawn(move || {
                    for line in BufReader::new(stderr).lines() {
                        match line {
                            Ok(line) => push_log(&log, line),
                            Err(_) => break,
                        }
                    }
                })
                .context("plugin log thread")?;
        }
        Ok(Self {
            child,
            stdin,
            incoming,
            next_id: 1,
            log,
            closed,
        })
    }

    /// Whether the plugin can still receive messages.
    pub fn alive(&mut self) -> bool {
        !self.closed.load(Ordering::Relaxed)
            && self.stdin.is_some()
            && matches!(self.child.try_wait(), Ok(None))
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Id> {
        let id = Id::Number(self.next_id);
        self.next_id += 1;
        self.send(&Message::request(id.clone(), method, params))?;
        Ok(id)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&Message::notification(method, params))
    }

    pub fn respond(&mut self, id: Id, result: Result<Value, RpcError>) -> Result<()> {
        self.send(&Message::response(id, result))
    }

    fn send(&mut self, message: &Message) -> Result<()> {
        let Some(stdin) = &mut self.stdin else {
            bail!("the plugin has stopped");
        };
        let mut line = message.to_line();
        line.push('\n');
        if let Err(error) = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
        {
            self.stdin = None;
            bail!("cannot write to the plugin: {error}");
        }
        Ok(())
    }

    /// Everything received since the last poll.
    pub fn poll(&mut self) -> Vec<Incoming> {
        let mut messages = Vec::new();
        while let Ok(incoming) = self.incoming.try_recv() {
            messages.push(incoming);
        }
        messages
    }

    /// Block until the plugin answers the request `id`, servicing nothing else.
    /// Other messages that arrive meanwhile are returned too, in order.
    pub fn wait_for(
        &mut self,
        id: &Id,
        timeout: std::time::Duration,
    ) -> Result<(Value, Vec<Incoming>)> {
        let deadline = std::time::Instant::now() + timeout;
        let mut others = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.incoming.recv_timeout(remaining) {
                Ok(Incoming::Message(Message::Response(response))) if &response.id == id => {
                    return match (response.result, response.error) {
                        (Some(value), _) => Ok((value, others)),
                        (None, Some(error)) => Err(error.into()),
                        (None, None) => Ok((Value::Null, others)),
                    };
                }
                Ok(Incoming::Closed) => bail!("the plugin exited"),
                Ok(other) => others.push(other),
                Err(mpsc::RecvTimeoutError::Timeout) => bail!("the plugin did not answer in time"),
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("the plugin exited"),
            }
        }
    }

    /// Recent stderr output and host notes, oldest first.
    pub fn log(&self) -> Vec<String> {
        self.log
            .lock()
            .map(|log| log.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn note(&self, line: String) {
        push_log(&self.log, line);
    }

    /// Close stdin so a well-behaved plugin exits, then make sure it does.
    pub fn stop(&mut self) {
        self.stdin = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

fn push_log(log: &Mutex<VecDeque<String>>, line: String) {
    if let Ok(mut log) = log.lock() {
        if log.len() >= MAX_LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }
}

/// A command naming a file inside the plugin folder runs from there; anything
/// else is looked up on `PATH`.
fn resolve(program: &str, dir: &Path) -> std::path::PathBuf {
    let path = Path::new(program);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let local = dir.join(path);
    if path.components().count() > 1 || local.is_file() {
        local
    } else {
        path.to_path_buf()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    /// A shell "plugin" that answers `initialize`, echoes `ping` params and
    /// writes everything else to stderr.
    fn mock(dir: &Path) -> Manifest {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":1}}\n' "$id" ;;
    *'"method":"ping"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"pong":true}}\n' "$id"
                        printf '{"jsonrpc":"2.0","method":"host/log","params":{"message":"pinged"}}\n' ;;
    *'"method":"garbage"'*) printf 'not json\n' ;;
    *'"method":"shutdown"'*) printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"; exit 0 ;;
    *) printf 'unexpected: %s\n' "$line" >&2 ;;
  esac
done
"#;
        std::fs::write(dir.join("plugin.sh"), script).unwrap();
        Manifest::parse(
            "[plugin]\nid = \"mock\"\nname = \"Mock\"\nversion = \"1\"\ncommand = [\"sh\", \"plugin.sh\"]\n",
            dir,
        )
        .unwrap()
    }

    #[test]
    fn exchanges_messages_with_a_child_process_and_logs_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = mock(dir.path());
        let mut process = Process::spawn(&manifest, &[]).unwrap();
        assert!(process.alive());
        let id = process
            .request("initialize", json!({"protocol": 1}))
            .unwrap();
        let (result, others) = process
            .wait_for(&id, std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(result, json!({"protocol": 1}));
        assert!(others.is_empty());

        let ping = process.request("ping", Value::Null).unwrap();
        process.notify("unknown/thing", json!({})).unwrap();
        process.request("garbage", Value::Null).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen_pong = false;
        let mut seen_log = false;
        let mut seen_invalid = false;
        while std::time::Instant::now() < deadline && !(seen_pong && seen_log && seen_invalid) {
            for incoming in process.poll() {
                match incoming {
                    Incoming::Message(Message::Response(r)) if r.id == ping => {
                        assert_eq!(r.result, Some(json!({"pong": true})));
                        seen_pong = true;
                    }
                    Incoming::Message(Message::Notification(n)) if n.method == "host/log" => {
                        seen_log = true
                    }
                    Incoming::Invalid(text) => seen_invalid = text == "not json",
                    other => panic!("unexpected {other:?}"),
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(seen_pong && seen_log && seen_invalid);
        let id = process.request("garbage", Value::Null).unwrap();
        assert!(
            process
                .wait_for(&id, std::time::Duration::from_millis(300))
                .is_err()
        );
        let log = process.log();
        assert!(log.iter().any(|l| l.contains("unexpected")), "{log:?}");
        assert!(log.iter().any(|l| l.contains("invalid message")), "{log:?}");

        let id = process.request("shutdown", Value::Null).unwrap();
        process
            .wait_for(&id, std::time::Duration::from_secs(10))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while process.alive() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!process.alive());
        assert!(process.request("ping", Value::Null).is_err());
    }

    #[test]
    fn missing_programs_fail_to_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::parse(
            "[plugin]\nid = \"m\"\nname = \"M\"\nversion = \"1\"\ncommand = [\"./does-not-exist\"]\n",
            dir.path(),
        )
        .unwrap();
        assert!(Process::spawn(&manifest, &[]).is_err());
        assert_eq!(
            resolve("python3", dir.path()),
            std::path::PathBuf::from("python3")
        );
        assert_eq!(resolve("bin/run", dir.path()), dir.path().join("bin/run"));
    }
}

//! A plugin process: spawn it, exchange messages over its pipes, and collect
//! what it prints to stderr.
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{
    manifest::Manifest,
    protocol::{Id, MAX_LINE, Message, RpcError},
};

const MAX_LOG_LINES: usize = 500;
/// Bytes of plugin output that may wait to be handled. Beyond this the reader
/// stops reading the pipe, so a plugin that floods stdout is slowed down
/// instead of buffered in memory. One larger message is still let through.
pub const QUEUE_BYTES: usize = 32 * 1024 * 1024;
/// Longest stderr line kept; the rest of a longer line is dropped.
pub const MAX_STDERR_LINE: usize = 8 * 1024;
/// Longest log note (`host/log`) kept.
pub const MAX_NOTE: usize = 4 * 1024;
/// How much of an invalid line is logged.
const INVALID_PREVIEW: usize = 200;

/// Called from the reader thread when something arrives, so an idle window
/// wakes up to handle it.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Bytes waiting in the queue between the reader thread and the host.
#[derive(Default)]
struct Budget {
    used: Mutex<usize>,
    changed: Condvar,
    stopped: AtomicBool,
}

impl Budget {
    /// Wait until `bytes` more fit. Returns false once the process is stopped.
    fn reserve(&self, bytes: usize) -> bool {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used > 0 && *used + bytes > QUEUE_BYTES {
            if self.stopped.load(Ordering::Relaxed) {
                return false;
            }
            used = self
                .changed
                .wait_timeout(used, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if self.stopped.load(Ordering::Relaxed) {
            return false;
        }
        *used += bytes;
        true
    }

    fn release(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        *used = used.saturating_sub(bytes);
        self.changed.notify_all();
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.changed.notify_all();
    }

    fn used(&self) -> usize {
        *self.used.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Read one line of at most `cap` bytes, without its line ending. The rest of
/// a longer line is read and dropped; the flag says whether that happened.
/// `None` at the end of the stream.
pub fn read_capped_line<R: BufRead>(
    reader: &mut R,
    cap: usize,
) -> std::io::Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    if reader
        .by_ref()
        .take(cap as u64 + 1)
        .read_until(b'\n', &mut line)?
        == 0
    {
        return Ok(None);
    }
    let truncated = line.len() > cap && line.last() != Some(&b'\n');
    if truncated {
        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                break;
            }
            match buffer.iter().position(|&b| b == b'\n') {
                Some(end) => {
                    reader.consume(end + 1);
                    break;
                }
                None => {
                    let length = buffer.len();
                    reader.consume(length);
                }
            }
        }
    }
    line.truncate(cap);
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(Some((line, truncated)))
}

/// `text` cut to at most `max` bytes on a character boundary.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Request ids are unique across every process this host starts, so a stale
/// request to an earlier process of a plugin never matches a later one's answer.
static NEXT_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

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
    incoming: Receiver<(Incoming, usize)>,
    budget: Arc<Budget>,
    /// Messages a blocking [`Process::wait_for`] received but could not return,
    /// including [`Incoming::Closed`], handed out by the next [`Process::poll`].
    held: Vec<Incoming>,
    log: Arc<Mutex<VecDeque<String>>>,
    closed: Arc<AtomicBool>,
}

impl Process {
    /// Start the manifest's command in the plugin folder with extra environment.
    /// `wake` is called from a background thread whenever the plugin sends
    /// something.
    pub fn spawn(
        manifest: &Manifest,
        env: &[(String, String)],
        wake: Option<Wake>,
    ) -> Result<Self> {
        let wake: Wake = wake.unwrap_or_else(|| Arc::new(|| {}));
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
        let budget = Arc::new(Budget::default());
        {
            let send = send.clone();
            let closed = closed.clone();
            let log = log.clone();
            let budget = budget.clone();
            let wake = wake.clone();
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
                        // Wait while the host has not handled earlier output.
                        let bytes = line.len();
                        if !budget.reserve(bytes) {
                            break;
                        }
                        let incoming = match Message::parse(text) {
                            Ok(message) => Incoming::Message(message),
                            Err(error) => {
                                push_log(
                                    &log,
                                    format!(
                                        "invalid message ({error}): {}",
                                        clip(text, INVALID_PREVIEW)
                                    ),
                                );
                                Incoming::Invalid(clip(text, INVALID_PREVIEW))
                            }
                        };
                        if send.send((incoming, bytes)).is_err() {
                            break;
                        }
                        wake();
                    }
                    closed.store(true, Ordering::Relaxed);
                    let _ = send.send((Incoming::Closed, 0));
                    wake();
                })
                .context("plugin reader thread")?;
        }
        {
            let log = log.clone();
            std::thread::Builder::new()
                .name(format!("plugin {} stderr", manifest.plugin.id))
                .spawn(move || {
                    let mut reader = BufReader::new(stderr);
                    while let Ok(Some((line, truncated))) =
                        read_capped_line(&mut reader, MAX_STDERR_LINE)
                    {
                        let mut line = String::from_utf8_lossy(&line).into_owned();
                        if truncated {
                            line.push('…');
                        }
                        push_log(&log, line);
                    }
                })
                .context("plugin log thread")?;
        }
        Ok(Self {
            child,
            stdin,
            incoming,
            budget,
            held: Vec::new(),
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
        let id = Id::Number(NEXT_ID.fetch_add(1, Ordering::Relaxed));
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
        let mut messages = std::mem::take(&mut self.held);
        while let Ok((incoming, bytes)) = self.incoming.try_recv() {
            self.budget.release(bytes);
            messages.push(incoming);
        }
        messages
    }

    /// Block until the plugin answers the request `id`, servicing nothing else.
    /// Other messages that arrive meanwhile are returned too, in order.
    ///
    /// When the wait fails, those messages, and [`Incoming::Closed`] if the
    /// plugin exited, are kept for the next [`Process::poll`], so the host still
    /// notices the exit and cleans up after the plugin.
    pub fn wait_for(
        &mut self,
        id: &Id,
        timeout: std::time::Duration,
    ) -> Result<(Value, Vec<Incoming>)> {
        let deadline = std::time::Instant::now() + timeout;
        let mut others = Vec::new();
        let error = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let received = self
                .incoming
                .recv_timeout(remaining)
                .map(|(incoming, bytes)| {
                    self.budget.release(bytes);
                    incoming
                });
            match received {
                Ok(Incoming::Message(Message::Response(response))) if &response.id == id => {
                    match (response.result, response.error) {
                        (Some(value), _) => return Ok((value, others)),
                        (None, Some(error)) => break anyhow::Error::from(error),
                        (None, None) => return Ok((Value::Null, others)),
                    }
                }
                Ok(Incoming::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    others.push(Incoming::Closed);
                    break anyhow::anyhow!("the plugin exited");
                }
                Ok(other) => others.push(other),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    break anyhow::anyhow!("the plugin did not answer in time");
                }
            }
        };
        self.held.extend(others);
        Err(error)
    }

    /// Recent stderr output and host notes, oldest first.
    pub fn log(&self) -> Vec<String> {
        self.log
            .lock()
            .map(|log| log.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Add a host note to the log, cut to [`MAX_NOTE`] bytes.
    pub fn note(&self, line: String) {
        push_log(&self.log, clip(&line, MAX_NOTE));
    }

    /// Bytes of output read from the plugin but not yet handled.
    pub fn queued_bytes(&self) -> usize {
        self.budget.used()
    }

    /// Close stdin so a well-behaved plugin exits, then make sure it does.
    pub fn stop(&mut self) {
        self.budget.stop();
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
        let mut process = Process::spawn(&manifest, &[], None).unwrap();
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
    fn an_exit_during_a_blocking_call_is_reported_by_the_next_poll() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.sh"),
            "read -r line\nprintf '{\"jsonrpc\":\"2.0\",\"method\":\"host/log\",\"params\":{}}\\n'\nexit 3\n",
        )
        .unwrap();
        let manifest = Manifest::parse(
            "[plugin]\nid = \"crash\"\nname = \"Crash\"\nversion = \"1\"\ncommand = [\"sh\", \"plugin.sh\"]\n",
            dir.path(),
        )
        .unwrap();
        let mut process = Process::spawn(&manifest, &[], None).unwrap();
        let id = process.request("format/import", json!({})).unwrap();
        let error = process
            .wait_for(&id, std::time::Duration::from_secs(10))
            .unwrap_err();
        assert!(error.to_string().contains("exited"), "{error}");
        let held = process.poll();
        assert!(
            matches!(
                held.first(),
                Some(Incoming::Message(Message::Notification(_)))
            ),
            "{held:?}"
        );
        assert!(matches!(held.last(), Some(Incoming::Closed)), "{held:?}");
        assert!(process.poll().is_empty());
        assert!(!process.alive());
    }

    #[test]
    fn request_ids_are_unique_across_processes() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = mock(dir.path());
        let mut first = Process::spawn(&manifest, &[], None).unwrap();
        let mut second = Process::spawn(&manifest, &[], None).unwrap();
        let a = first.request("ping", Value::Null).unwrap();
        let b = second.request("ping", Value::Null).unwrap();
        let c = first.request("ping", Value::Null).unwrap();
        assert!(a != b && b != c && a != c, "{a:?} {b:?} {c:?}");
    }

    #[test]
    fn long_stderr_lines_and_notes_are_capped() {
        let long = "x".repeat(MAX_STDERR_LINE * 3);
        let input = format!("short\r\n{long}\nnext\n{long}");
        let mut reader = std::io::BufReader::with_capacity(64, input.as_bytes());
        let mut lines = Vec::new();
        while let Some((line, truncated)) = read_capped_line(&mut reader, MAX_STDERR_LINE).unwrap()
        {
            lines.push((line.len(), truncated));
        }
        assert_eq!(
            lines,
            [
                (5, false),
                (MAX_STDERR_LINE, true),
                (4, false),
                (MAX_STDERR_LINE, true)
            ]
        );
        // A line of exactly the cap is kept whole.
        let exact = format!("{}\nend\n", "y".repeat(10));
        let mut reader = exact.as_bytes();
        assert_eq!(
            read_capped_line(&mut reader, 10).unwrap(),
            Some((b"yyyyyyyyyy".to_vec(), false))
        );
        assert_eq!(
            read_capped_line(&mut reader, 10).unwrap(),
            Some((b"end".to_vec(), false))
        );
        assert_eq!(read_capped_line(&mut reader, 10).unwrap(), None);

        let dir = tempfile::tempdir().unwrap();
        // A plugin that writes a 1 MiB stderr line, then waits.
        std::fs::write(
            dir.path().join("plugin.sh"),
            "head -c 1048576 /dev/zero | tr '\\0' 'e' >&2\necho >&2\necho done >&2\nread -r line\n",
        )
        .unwrap();
        let manifest = Manifest::parse(
            "[plugin]\nid = \"noisy\"\nname = \"Noisy\"\nversion = \"1\"\ncommand = [\"sh\", \"plugin.sh\"]\n",
            dir.path(),
        )
        .unwrap();
        let process = Process::spawn(&manifest, &[], None).unwrap();
        process.note("é".repeat(MAX_NOTE));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !process.log().iter().any(|l| l == "done") {
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                process.log().len()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let log = process.log();
        assert!(
            log.iter().all(|l| l.len() <= MAX_STDERR_LINE + 4),
            "a line is too long"
        );
        assert!(log.iter().any(|l| l.starts_with('e') && l.ends_with('…')));
        let note = log.iter().find(|l| l.starts_with('é')).unwrap();
        assert!(note.len() <= MAX_NOTE + 4);
    }

    #[test]
    fn a_flooding_plugin_is_held_to_the_queue_budget_and_wakes_the_host() {
        let dir = tempfile::tempdir().unwrap();
        // 48 messages of 1 MiB each, more than the 32 MiB budget.
        std::fs::write(
            dir.path().join("plugin.sh"),
            r#"i=0
while [ $i -lt 48 ]; do
  printf '{"jsonrpc":"2.0","method":"host/log","params":{"message":"'
  head -c 1048576 /dev/zero | tr '\0' 'm'
  printf '"}}\n'
  i=$((i + 1))
done
"#,
        )
        .unwrap();
        let manifest = Manifest::parse(
            "[plugin]\nid = \"flood\"\nname = \"Flood\"\nversion = \"1\"\ncommand = [\"sh\", \"plugin.sh\"]\n",
            dir.path(),
        )
        .unwrap();
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = wakes.clone();
        let wake: Wake = Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let mut process = Process::spawn(&manifest, &[], Some(wake)).unwrap();
        // Without polling, the reader stops at the budget.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while process.queued_bytes() + (1 << 20) < QUEUE_BYTES {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            process.queued_bytes() <= QUEUE_BYTES,
            "{}",
            process.queued_bytes()
        );
        assert!(wakes.load(Ordering::Relaxed) >= 30);
        // Polling drains the queue and lets the rest through.
        let mut messages = 0;
        let mut closed = false;
        while !closed {
            assert!(std::time::Instant::now() < deadline, "{messages}");
            for incoming in process.poll() {
                match incoming {
                    Incoming::Message(_) => messages += 1,
                    Incoming::Closed => closed = true,
                    Incoming::Invalid(text) => panic!("invalid {}", text.len()),
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(messages, 48);
        assert_eq!(process.queued_bytes(), 0);
    }

    #[test]
    fn missing_programs_fail_to_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::parse(
            "[plugin]\nid = \"m\"\nname = \"M\"\nversion = \"1\"\ncommand = [\"./does-not-exist\"]\n",
            dir.path(),
        )
        .unwrap();
        assert!(Process::spawn(&manifest, &[], None).is_err());
        assert_eq!(
            resolve("python3", dir.path()),
            std::path::PathBuf::from("python3")
        );
        assert_eq!(resolve("bin/run", dir.path()), dir.path().join("bin/run"));
    }
}

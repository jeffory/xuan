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
    /// Taken by [`Process::stop`], which hands it to a reaper.
    child: Option<Child>,
    /// The plugin's whole process tree, so its subprocesses die with it.
    tree: Option<tree::Tree>,
    stdin: Option<ChildStdin>,
    incoming: Receiver<(Incoming, usize)>,
    budget: Arc<Budget>,
    /// Messages a blocking [`Process::wait_for`] received but could not return,
    /// including [`Incoming::Closed`], handed out by the next [`Process::poll`].
    held: Vec<Incoming>,
    log: Arc<Mutex<VecDeque<String>>>,
    closed: Arc<AtomicBool>,
    /// False between `initialize` and its answer. Requests and notifications
    /// sent meanwhile wait in `outbox`, so the plugin sees nothing before
    /// `initialize` has been answered, as the protocol promises.
    ready: bool,
    outbox: Vec<Message>,
}

impl Process {
    /// Start the manifest's command in the plugin folder with extra environment.
    /// `wake` is called from a background thread whenever the plugin sends
    /// something. With `block_network` it starts under the filter of
    /// [`super::sandbox`], or not at all when the filter cannot be installed.
    pub fn spawn(
        manifest: &Manifest,
        env: &[(String, String)],
        wake: Option<Wake>,
        block_network: bool,
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
        tree::prepare(&mut command);
        if block_network {
            super::sandbox::block_network(&mut command).with_context(|| {
                format!("Cannot block the network for plugin {}", manifest.plugin.id)
            })?;
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "Cannot start `{}` for plugin {}{}",
                program.display(),
                manifest.plugin.id,
                if block_network {
                    " with its network blocked"
                } else {
                    ""
                }
            )
        })?;
        let tree = tree::Tree::attach(&child);
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
        if block_network {
            push_log(
                &log,
                "Network blocked by Xuan: opening sockets other than Unix sockets fails with EACCES"
                    .into(),
            );
        }
        Ok(Self {
            child: Some(child),
            tree,
            stdin,
            incoming,
            budget,
            held: Vec::new(),
            log,
            closed,
            ready: true,
            outbox: Vec::new(),
        })
    }

    /// Send `initialize` without waiting for the answer. Until
    /// [`Process::set_ready`], later requests and notifications are held back.
    pub fn initialize(&mut self, params: Value) -> Result<Id> {
        let id = Id::Number(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        // A plugin that exits at once may have closed its stdin already.
        // It is then reported as failing to start, with its log, once its
        // output closes, like one that exits just after reading this.
        if let Err(error) = self.send(&Message::request(id.clone(), "initialize", params)) {
            push_log(&self.log, format!("{error:#}"));
        }
        self.ready = false;
        Ok(id)
    }

    /// Whether `initialize` has been answered.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// `initialize` was answered: send what was held back.
    pub fn set_ready(&mut self) -> Result<()> {
        self.ready = true;
        for message in std::mem::take(&mut self.outbox) {
            self.send(&message)?;
        }
        Ok(())
    }

    /// Whether the plugin can still receive messages.
    pub fn alive(&mut self) -> bool {
        !self.closed.load(Ordering::Relaxed)
            && self.stdin.is_some()
            && (self.child.as_mut()).is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Id> {
        let id = Id::Number(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        self.send_when_ready(Message::request(id.clone(), method, params))?;
        Ok(id)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send_when_ready(Message::notification(method, params))
    }

    fn send_when_ready(&mut self, message: Message) -> Result<()> {
        if self.ready {
            return self.send(&message);
        }
        // Until `initialize` is answered, even a plugin whose stdin closed
        // is still starting: what waits for it fails when it is reported.
        self.outbox.push(message);
        Ok(())
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

    /// Close stdin so a well-behaved plugin exits, and make sure it does,
    /// together with every process it started, without waiting: a
    /// background thread gives it [`STOP_GRACE`] and then kills it.
    pub fn stop(&mut self) {
        self.close();
        if let Some(child) = self.child.take() {
            let tree = self.tree.take();
            let deadline = std::time::Instant::now() + STOP_GRACE;
            std::thread::spawn(move || reap(child, tree, deadline));
        }
    }

    /// Stop several plugins and wait until they are gone, for at most
    /// `grace` in all before they are killed. For quitting, when a
    /// background thread would not outlive the editor.
    pub fn stop_all(processes: impl IntoIterator<Item = Process>, grace: Duration) {
        let mut processes: Vec<Process> = processes.into_iter().collect();
        for process in &mut processes {
            process.close();
        }
        let deadline = std::time::Instant::now() + grace;
        for process in &mut processes {
            if let Some(child) = process.child.take() {
                reap(child, process.tree.take(), deadline);
            }
        }
    }

    fn close(&mut self) {
        self.budget.stop();
        self.stdin = None;
        self.outbox.clear();
    }
}

/// How long a stopped plugin may take to exit on its own.
pub const STOP_GRACE: Duration = Duration::from_secs(1);

/// Wait until `deadline` for the plugin to exit, then kill whatever is left
/// of it. Subprocesses outlive a plugin that exits on its own, so the tree
/// is killed either way.
fn reap(mut child: Child, tree: Option<tree::Tree>, deadline: std::time::Instant) {
    while std::time::Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(tree) = &tree {
        tree.kill();
    }
    let _ = child.kill();
    let _ = child.wait();
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

/// Grouping a plugin with its subprocesses.
#[cfg(unix)]
mod tree {
    use std::process::{Child, Command};

    /// The plugin's process group, which its subprocesses join unless they
    /// leave it on purpose.
    pub struct Tree(rustix::process::Pid);

    /// Start the plugin as the leader of a new process group.
    pub fn prepare(command: &mut Command) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    impl Tree {
        pub fn attach(child: &Child) -> Option<Self> {
            Some(Self(rustix::process::Pid::from_child(child)))
        }

        /// Kill every process left in the group.
        pub fn kill(&self) {
            let _ = rustix::process::kill_process_group(self.0, rustix::process::Signal::KILL);
        }
    }
}

/// Grouping a plugin with its subprocesses.
#[cfg(windows)]
mod tree {
    use std::{
        os::windows::io::AsRawHandle,
        process::{Child, Command},
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
    };

    /// A Job Object holding the plugin. Processes it starts join the job too,
    /// and closing the job kills them all. The plugin joins right after it is
    /// spawned, so only something it starts in its first instant can escape.
    pub struct Tree(HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Tree {}

    pub fn prepare(_command: &mut Command) {}

    impl Tree {
        pub fn attach(child: &Child) -> Option<Self> {
            // SAFETY: plain Win32 calls on a job handle this function owns and
            // on the child's process handle, which outlives the calls.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let tree = Self(job);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let configured = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) != 0;
                let assigned =
                    configured && AssignProcessToJobObject(job, child.as_raw_handle()) != 0;
                assigned.then_some(tree)
            }
        }

        /// Kill every process in the job.
        pub fn kill(&self) {
            // SAFETY: the handle is a valid job until `drop`.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: the handle is owned and closed once; closing it kills
            // what is left in the job.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Elsewhere only the plugin process itself is stopped.
#[cfg(not(any(unix, windows)))]
mod tree {
    use std::process::{Child, Command};

    pub struct Tree;

    pub fn prepare(_command: &mut Command) {}

    impl Tree {
        pub fn attach(_child: &Child) -> Option<Self> {
            None
        }

        pub fn kill(&self) {}
    }
}

/// A command naming a file inside the plugin folder runs from there; anything
/// else is looked up on `PATH`.
pub(crate) fn resolve(program: &str, dir: &Path) -> std::path::PathBuf {
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
        let mut process = Process::spawn(&manifest, &[], None, false).unwrap();
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
        let mut process = Process::spawn(&manifest, &[], None, false).unwrap();
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
        let mut first = Process::spawn(&manifest, &[], None, false).unwrap();
        let mut second = Process::spawn(&manifest, &[], None, false).unwrap();
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
        let process = Process::spawn(&manifest, &[], None, false).unwrap();
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
        let mut process = Process::spawn(&manifest, &[], Some(wake), false).unwrap();
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

    /// Whether a process is gone: it no longer exists or is a zombie nobody
    /// has reaped yet (a container's init may not reap orphans).
    #[cfg(target_os = "linux")]
    fn gone(pid: &str) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => true,
            Ok(stat) => stat
                .rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with(['Z', 'X'])),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stopping_a_plugin_kills_its_subprocesses() {
        let dir = tempfile::tempdir().unwrap();
        // The first plugin exits when stdin closes but leaves a child behind;
        // the second ignores the closed stdin altogether.
        for (id, script) in [
            ("leaves", "sleep 300 &\necho $! > child.pid\nread -r line\n"),
            (
                "stays",
                "trap '' TERM HUP\nsleep 300 &\necho $! > child.pid\nwhile :; do sleep 1; done\n",
            ),
        ] {
            let folder = dir.path().join(id);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("plugin.sh"), script).unwrap();
            let manifest = Manifest::parse(
                &format!(
                    "[plugin]\nid = \"{id}\"\nname = \"N\"\nversion = \"1\"\ncommand = [\"sh\", \"plugin.sh\"]\n"
                ),
                &folder,
            )
            .unwrap();
            let mut process = Process::spawn(&manifest, &[], None, false).unwrap();
            let pid_file = folder.join("child.pid");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let pid = loop {
                if let Ok(text) = std::fs::read_to_string(&pid_file)
                    && !text.trim().is_empty()
                {
                    break text.trim().to_owned();
                }
                assert!(std::time::Instant::now() < deadline, "{id}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert!(!gone(&pid), "{id}");
            // Stopping does not wait for the plugin.
            let started = std::time::Instant::now();
            process.stop();
            assert!(started.elapsed() < std::time::Duration::from_millis(100));
            assert!(!process.alive());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !gone(&pid) {
                assert!(std::time::Instant::now() < deadline, "{id}: {pid} survived");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn stopping_every_plugin_on_quit_waits_only_briefly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.sh"),
            "trap '' TERM HUP\nwhile :; do sleep 1; done\n",
        )
        .unwrap();
        let manifest = Manifest::parse(
            "[plugin]\nid = \"stays\"\nname = \"S\"\nversion = \"1\"\ncommand = [\"sh\", \"plugin.sh\"]\n",
            dir.path(),
        )
        .unwrap();
        let processes: Vec<Process> = (0..3)
            .map(|_| Process::spawn(&manifest, &[], None, false).unwrap())
            .collect();
        let started = std::time::Instant::now();
        Process::stop_all(processes, Duration::from_millis(200));
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn missing_programs_fail_to_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::parse(
            "[plugin]\nid = \"m\"\nname = \"M\"\nversion = \"1\"\ncommand = [\"./does-not-exist\"]\n",
            dir.path(),
        )
        .unwrap();
        assert!(Process::spawn(&manifest, &[], None, false).is_err());
        assert_eq!(
            resolve("python3", dir.path()),
            std::path::PathBuf::from("python3")
        );
        assert_eq!(resolve("bin/run", dir.path()), dir.path().join("bin/run"));
    }

    /// Plugins started with their network blocked, on Linux.
    #[cfg(target_os = "linux")]
    mod network {
        use super::*;
        use crate::plugins::sandbox;

        /// A Python plugin that answers `initialize`, and `probe` with the
        /// errno (0 for success) of each attempt to open a socket, also from
        /// a process it starts.
        const PROBE: &str = r#"
import ctypes, json, os, platform, socket, subprocess, sys

libc = ctypes.CDLL(None, use_errno=True)

def attempt(make):
    try:
        made = make()
    except OSError as error:
        return error.errno
    for s in made if isinstance(made, tuple) else (made,):
        s.close()
    return 0

def syscall(number, *args):
    result = libc.syscall(ctypes.c_long(number), *[ctypes.c_long(a) for a in args])
    if result < 0:
        return ctypes.get_errno()
    os.close(result)
    return 0

def unix_pair_works():
    a, b = socket.socketpair(socket.AF_UNIX)
    a.sendall(b"ping")
    ok = b.recv(4) == b"ping"
    a.close()
    b.close()
    return 0 if ok else -1

CHILD = "import socket\ntry:\n socket.socket(socket.AF_INET, socket.SOCK_STREAM).close(); print(0)\nexcept OSError as e:\n print(e.errno)"

def probe():
    result = {
        "inet": attempt(lambda: socket.socket(socket.AF_INET, socket.SOCK_STREAM)),
        "inet_udp": attempt(lambda: socket.socket(socket.AF_INET, socket.SOCK_DGRAM)),
        "inet6": attempt(lambda: socket.socket(socket.AF_INET6, socket.SOCK_STREAM)),
        "packet": attempt(lambda: socket.socket(socket.AF_PACKET, socket.SOCK_RAW, 0)),
        "unix": attempt(lambda: socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)),
        "unix_pair": unix_pair_works(),
        "inet_pair": attempt(lambda: socket.socketpair(socket.AF_INET)),
        "netlink": attempt(lambda: socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)),
        "io_uring": syscall(425, 1, 0),
        "child_inet": int(subprocess.run([sys.executable, "-c", CHILD], capture_output=True, text=True).stdout.strip() or -1),
    }
    if platform.machine() == "x86_64":
        # socket() through the x32 ABI's syscall number.
        result["x32_inet"] = syscall(0x40000000 | 41, socket.AF_INET, socket.SOCK_STREAM, 0)
    return result

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        result = {"protocol": 1}
    elif method == "probe":
        result = probe()
    else:
        result = None
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
    if method == "shutdown":
        break
"#;

        fn probe_plugin(dir: &Path, network: &str) -> Manifest {
            std::fs::write(dir.join("probe.py"), PROBE).unwrap();
            Manifest::parse(
                &format!(
                    "[plugin]\nid = \"probe\"\nname = \"Probe\"\nversion = \"1\"\ncommand = [\"python3\", \"probe.py\"]\n\n[permissions]\nnetwork = [{network}]\n"
                ),
                dir,
            )
            .unwrap()
        }

        /// Start the plugin, check that it talks JSON-RPC, and return its
        /// probe and its log.
        fn run(manifest: &Manifest, block: bool) -> (Value, Vec<String>) {
            let mut process = Process::spawn(manifest, &[], None, block).unwrap();
            let id = process.initialize(json!({"protocol": 1})).unwrap();
            let (result, _) = process
                .wait_for(&id, Duration::from_secs(20))
                .unwrap_or_else(|e| panic!("{e:#}: {:?}", process.log()));
            assert_eq!(result, json!({"protocol": 1}));
            process.set_ready().unwrap();
            let id = process.request("probe", Value::Null).unwrap();
            let (probe, _) = process
                .wait_for(&id, Duration::from_secs(20))
                .unwrap_or_else(|e| panic!("{e:#}: {:?}", process.log()));
            let id = process.request("shutdown", Value::Null).unwrap();
            process.wait_for(&id, Duration::from_secs(20)).unwrap();
            (probe, process.log())
        }

        /// Whether this kernel and container let a process install a
        /// filter, which testing a blocked plugin needs.
        fn filters_allowed() -> bool {
            match sandbox::available() {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("skipped: seccomp filters are not available here: {error:#}");
                    false
                }
            }
        }

        const EACCES: i64 = 13;

        #[test]
        fn a_plugin_without_hosts_cannot_open_network_sockets_when_blocked() {
            if !filters_allowed() {
                return;
            }
            let dir = tempfile::tempdir().unwrap();
            let manifest = probe_plugin(dir.path(), "");
            assert!(sandbox::blocks_network(true, &manifest.permissions));
            let (probe, log) = run(&manifest, true);
            for blocked in [
                "inet",
                "inet_udp",
                "inet6",
                "packet",
                "inet_pair",
                "io_uring",
                "child_inet",
            ] {
                assert_eq!(probe[blocked], EACCES, "{blocked}: {probe}");
            }
            if cfg!(target_arch = "x86_64") {
                assert_eq!(probe["x32_inet"], EACCES, "{probe}");
            }
            for allowed in ["unix", "unix_pair", "netlink"] {
                assert_eq!(probe[allowed], 0, "{allowed}: {probe}");
            }
            assert!(
                log.iter().any(|l| l.starts_with("Network blocked by Xuan")),
                "{log:?}"
            );
        }

        #[test]
        fn plugins_open_sockets_when_not_blocked_or_declaring_hosts() {
            for (network, setting) in [("", false), ("\"localhost\"", true)] {
                let dir = tempfile::tempdir().unwrap();
                let manifest = probe_plugin(dir.path(), network);
                let block = sandbox::blocks_network(setting, &manifest.permissions);
                assert!(!block, "{network} {setting}");
                let (probe, log) = run(&manifest, block);
                for allowed in ["inet", "unix", "unix_pair", "child_inet"] {
                    assert_eq!(probe[allowed], 0, "{allowed}: {probe}");
                }
                assert!(
                    !log.iter().any(|l| l.contains("Network blocked")),
                    "{log:?}"
                );
            }
        }
    }
}

//! Noticing when another program changes an open project file (issue #117).
//!
//! [`FileWatcher`] follows each file through the operating system's change notifications
//! (inotify, ReadDirectoryChangesW or FSEvents, through the `notify` crate), never by polling.
//! It watches the file's folder rather than the file, because an atomic save renames a new
//! file over the old one, and a watch on the old file would end with it.
//!
//! A change is looked at once the file has been quiet for [`DEBOUNCE`], on the watcher's own
//! thread: the file is read whole and hashed, and only content that differs from what Xuan last
//! loaded or wrote is loaded and reported. So a `touch`, or Xuan's own save (whose hash
//! [`crate::io::save_hashed`] gives before the file is replaced), reports nothing. A file that
//! does not load, such as one still being written, is reported as failed and its last good
//! content is kept as the one to compare with, so the next change tries again.
//!
//! The decisions are made by [`Watches`], which has no threads and touches no files, and is
//! tested on its own; what the editor then does with a change is up to it (see
//! `app/reload.rs`).

use std::{
    collections::HashMap,
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use notify::{
    EventKind, RecursiveMode, Watcher as _,
    event::{AccessKind, AccessMode},
};
use uuid::Uuid;

use crate::{document::Document, io};

/// How long a file must stay quiet after a change before it is read.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// The SHA-256 of a file's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
        Self(digest.as_ref().try_into().expect("SHA-256 is 32 bytes"))
    }

    /// The hash of everything `reader` holds, read in pieces.
    pub fn read(mut reader: impl Read) -> std::io::Result<Self> {
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = vec![0; 1 << 16];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => context.update(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Self(
            context
                .finish()
                .as_ref()
                .try_into()
                .expect("SHA-256 is 32 bytes"),
        ))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..6] {
            write!(f, "{byte:02x}")?;
        }
        f.write_str("…")
    }
}

/// Whether an event can mean the file's content changed. Opening and reading it cannot, and
/// the watcher reads the file itself, so counting those would wake it again and again.
pub fn may_change_content(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// Whether a file that hashes to `current` must be loaded, given the hash Xuan last loaded or
/// wrote (`None` when it never read the file).
pub fn differs(known: Option<ContentHash>, current: ContentHash) -> bool {
    known != Some(current)
}

/// One followed file.
#[derive(Debug)]
struct Watched {
    path: PathBuf,
    /// The folder watched for it, as given and with links resolved: FSEvents names changed
    /// files by their real path.
    folder: PathBuf,
    real_folder: PathBuf,
    name: OsString,
    known: Option<ContentHash>,
    /// When the file will have been quiet long enough to read, after a change.
    due: Option<Instant>,
}

/// The folders to start and stop watching after [`Watches::watch`] or [`Watches::unwatch`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FolderChanges {
    pub add: Option<PathBuf>,
    pub remove: Option<PathBuf>,
}

/// The followed files, by document id, and when each is to be read. No threads, no files.
#[derive(Debug, Default)]
pub struct Watches {
    files: HashMap<Uuid, Watched>,
}

impl Watches {
    /// Follows `path` for `id` (replacing what `id` followed before), taking `known` as the
    /// content Xuan has. `real_folder` is the file's folder with links resolved.
    pub fn watch(
        &mut self,
        id: Uuid,
        path: PathBuf,
        real_folder: PathBuf,
        known: Option<ContentHash>,
    ) -> FolderChanges {
        let folder = folder_of(&path);
        let name = path.file_name().unwrap_or_default().to_owned();
        let previous = self.files.insert(
            id,
            Watched {
                path,
                folder: folder.clone(),
                real_folder,
                name,
                known,
                due: None,
            },
        );
        let previous = previous.map(|watched| watched.folder);
        let watching = previous.as_ref() == Some(&folder)
            || self
                .files
                .iter()
                .any(|(other, w)| *other != id && w.folder == folder);
        let remove = previous.filter(|old| *old != folder && !self.uses(old));
        let add = (!watching).then_some(folder);
        FolderChanges { add, remove }
    }

    /// Stops following `id`'s file.
    pub fn unwatch(&mut self, id: Uuid) -> FolderChanges {
        let remove = self
            .files
            .remove(&id)
            .map(|watched| watched.folder)
            .filter(|folder| !self.uses(folder));
        FolderChanges { add: None, remove }
    }

    fn uses(&self, folder: &Path) -> bool {
        self.files.values().any(|w| w.folder == folder)
    }

    pub fn path(&self, id: Uuid) -> Option<&Path> {
        self.files.get(&id).map(|w| w.path.as_path())
    }

    pub fn known(&self, id: Uuid) -> Option<ContentHash> {
        self.files.get(&id).and_then(|w| w.known)
    }

    /// Notes a change to any of `paths`: the files among them are read once they have been
    /// quiet for [`DEBOUNCE`] from `now`. Whether any of them is followed.
    pub fn changed(&mut self, paths: &[PathBuf], now: Instant) -> bool {
        let mut any = false;
        for watched in self.files.values_mut() {
            let matches = paths.iter().any(|path| {
                path.file_name() == Some(watched.name.as_os_str())
                    && path
                        .parent()
                        .is_some_and(|p| p == watched.folder || p == watched.real_folder)
            });
            if matches {
                watched.due = Some(now + DEBOUNCE);
                any = true;
            }
        }
        any
    }

    /// Reads every file again after [`DEBOUNCE`]: the system dropped events, so any might have
    /// changed.
    pub fn rescan(&mut self, now: Instant) {
        for watched in self.files.values_mut() {
            watched.due = Some(now + DEBOUNCE);
        }
    }

    /// When the next file is to be read.
    pub fn next_due(&self) -> Option<Instant> {
        self.files.values().filter_map(|w| w.due).min()
    }

    /// The files quiet long enough by `now` to read, which are no longer due.
    pub fn take_due(&mut self, now: Instant) -> Vec<(Uuid, PathBuf)> {
        let mut due: Vec<_> = self
            .files
            .iter_mut()
            .filter(|(_, w)| w.due.is_some_and(|due| due <= now))
            .map(|(id, w)| {
                w.due = None;
                (*id, w.path.clone())
            })
            .collect();
        due.sort_by(|a, b| a.1.cmp(&b.1));
        due
    }

    /// Whether `id`'s file, now hashing to `current`, has to be loaded.
    pub fn needs_load(&self, id: Uuid, current: ContentHash) -> bool {
        differs(self.known(id), current)
    }

    /// Records that `path`, hashing to `hash`, loaded for `id`: the content to compare with
    /// from now on. Ignored if `id` has moved on to another file meanwhile.
    pub fn loaded(&mut self, id: Uuid, path: &Path, hash: ContentHash) {
        if let Some(watched) = self.files.get_mut(&id).filter(|w| w.path == path) {
            watched.known = Some(hash);
        }
    }
}

fn folder_of(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

/// What reading a changed file found.
#[derive(Debug)]
pub enum Outcome {
    /// The same content as before, such as after a `touch` or Xuan's own save.
    Unchanged,
    /// New content, loaded.
    Changed(Box<Document>),
    /// The file could not be read or did not load, perhaps because it is still being
    /// written. The document is kept, and the next change is tried again.
    Failed(String),
}

/// What became of one followed file.
#[derive(Debug)]
pub struct Report {
    pub id: Uuid,
    pub path: PathBuf,
    pub outcome: Outcome,
}

enum Message {
    Watch {
        id: Uuid,
        path: PathBuf,
        known: Option<ContentHash>,
    },
    Unwatch(Uuid),
    Event(notify::Result<notify::Event>),
    /// Answered once everything sent before it is done.
    Flush(mpsc::Sender<()>),
    Stop,
}

/// Follows open project files on a background thread; see the module documentation.
pub struct FileWatcher {
    messages: mpsc::Sender<Message>,
    reports: mpsc::Receiver<Report>,
}

impl FileWatcher {
    /// Starts the watcher. `wake` is called, from its thread, after a changed file has been
    /// loaded, so the editor can show it without waiting for input.
    pub fn spawn(wake: impl Fn() + Send + 'static) -> Result<Self> {
        let (messages, receive) = mpsc::channel();
        let events = messages.clone();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = events.send(Message::Event(event));
        })?;
        let (send, reports) = mpsc::channel();
        thread::Builder::new()
            .name("xuan-file-watch".into())
            .spawn(move || run(watcher, receive, send, wake))?;
        Ok(Self { messages, reports })
    }

    /// Follows `path` for document `id`, replacing what `id` followed. `known` is the hash of
    /// the content Xuan has; without it, the file as it is now is taken to be that content.
    pub fn watch(&self, id: Uuid, path: &Path, known: Option<ContentHash>) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let _ = self.messages.send(Message::Watch { id, path, known });
    }

    pub fn unwatch(&self, id: Uuid) {
        let _ = self.messages.send(Message::Unwatch(id));
    }

    /// Waits until the watcher has started following every file asked for so far, so a change
    /// made after this returns is not missed. Tests use it; the editor never needs to wait.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.messages.send(Message::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }

    /// The next report, if one is waiting.
    pub fn try_recv(&self) -> Option<Report> {
        self.reports.try_recv().ok()
    }

    /// The next report, waiting up to `timeout` for it.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Report> {
        self.reports.recv_timeout(timeout).ok()
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        // Not joined: the thread may be loading a large file, and stops when it is done.
        let _ = self.messages.send(Message::Stop);
    }
}

fn run(
    mut watcher: notify::RecommendedWatcher,
    messages: mpsc::Receiver<Message>,
    reports: mpsc::Sender<Report>,
    wake: impl Fn(),
) {
    let mut watches = Watches::default();
    loop {
        let message = match watches.next_due() {
            Some(due) => {
                match messages.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(message) => Some(message),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            None => match messages.recv() {
                Ok(message) => Some(message),
                Err(_) => return,
            },
        };
        match message {
            Some(Message::Stop) => return,
            Some(Message::Watch { id, path, known }) => {
                let folder = folder_of(&path);
                let real_folder = fs::canonicalize(&folder).unwrap_or_else(|_| folder.clone());
                let known = known.or_else(|| File::open(&path).and_then(ContentHash::read).ok());
                let changes = watches.watch(id, path.clone(), real_folder, known);
                if let Err(error) = apply(&mut watcher, changes) {
                    let outcome = Outcome::Failed(error.to_string());
                    if reports.send(Report { id, path, outcome }).is_err() {
                        return;
                    }
                }
            }
            Some(Message::Unwatch(id)) => {
                let _ = apply(&mut watcher, watches.unwatch(id));
            }
            Some(Message::Event(Ok(event))) => {
                if event.need_rescan() {
                    watches.rescan(Instant::now());
                } else if may_change_content(&event.kind) {
                    watches.changed(&event.paths, Instant::now());
                }
            }
            Some(Message::Event(Err(_))) => watches.rescan(Instant::now()),
            Some(Message::Flush(done)) => {
                let _ = done.send(());
            }
            None => {}
        }
        for (id, path) in watches.take_due(Instant::now()) {
            let outcome = inspect(&mut watches, id, &path);
            let changed = matches!(outcome, Outcome::Changed(_));
            if reports.send(Report { id, path, outcome }).is_err() {
                return;
            }
            if changed {
                wake();
            }
        }
    }
}

fn apply(watcher: &mut notify::RecommendedWatcher, changes: FolderChanges) -> notify::Result<()> {
    if let Some(folder) = changes.remove {
        let _ = watcher.unwatch(&folder);
    }
    if let Some(folder) = changes.add {
        watcher.watch(&folder, RecursiveMode::NonRecursive)?;
    }
    Ok(())
}

/// Reads `id`'s file and loads it if its content changed.
fn inspect(watches: &mut Watches, id: Uuid, path: &Path) -> Outcome {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let hash = ContentHash::of(&bytes);
    if !watches.needs_load(id, hash) {
        return Outcome::Unchanged;
    }
    match io::load_bytes(&bytes) {
        Ok(document) => {
            watches.loaded(id, path, hash);
            Outcome::Changed(Box::new(document))
        }
        Err(error) => Outcome::Failed(format!("{error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, MetadataKind, ModifyKind, RenameMode};

    fn hash(text: &str) -> ContentHash {
        ContentHash::of(text.as_bytes())
    }

    #[test]
    fn hashes_match_whole_and_streamed_reads() {
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(
            ContentHash::of(&bytes),
            ContentHash::read(bytes.as_slice()).unwrap()
        );
        assert_ne!(hash("a"), hash("b"));
        assert_eq!(format!("{:?}", hash("")), "e3b0c44298fc…");
    }

    #[test]
    fn only_different_content_is_loaded() {
        assert!(differs(None, hash("a")), "a file never read is loaded");
        assert!(differs(Some(hash("a")), hash("b")));
        assert!(!differs(Some(hash("a")), hash("a")));
    }

    #[test]
    fn reading_the_file_is_not_a_change() {
        assert!(!may_change_content(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!may_change_content(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(may_change_content(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(may_change_content(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::WriteTime)
        )));
        assert!(may_change_content(&EventKind::Modify(ModifyKind::Name(
            RenameMode::To
        ))));
        assert!(may_change_content(&EventKind::Create(CreateKind::File)));
        assert!(may_change_content(&EventKind::Any));
    }

    #[test]
    fn changes_wait_until_the_file_is_quiet() {
        let mut watches = Watches::default();
        let id = Uuid::new_v4();
        let path = PathBuf::from("/work/a.xuan");
        watches.watch(id, path.clone(), "/work".into(), Some(hash("a")));
        let start = Instant::now();
        assert_eq!(watches.next_due(), None);

        assert!(watches.changed(std::slice::from_ref(&path), start));
        assert_eq!(watches.next_due(), Some(start + DEBOUNCE));
        // Another write in the meantime starts the wait again.
        let later = start + DEBOUNCE / 2;
        watches.changed(std::slice::from_ref(&path), later);
        assert_eq!(watches.next_due(), Some(later + DEBOUNCE));
        assert!(watches.take_due(start + DEBOUNCE).is_empty());

        assert_eq!(watches.take_due(later + DEBOUNCE), [(id, path)]);
        assert_eq!(watches.next_due(), None, "read once per change");
        assert!(watches.take_due(later + DEBOUNCE * 10).is_empty());
    }

    #[test]
    fn events_match_the_file_by_folder_or_real_folder() {
        let mut watches = Watches::default();
        let id = Uuid::new_v4();
        watches.watch(
            id,
            "/var/work/a.xuan".into(),
            "/private/var/work".into(),
            None,
        );
        let now = Instant::now();
        for other in [
            "/var/work/b.xuan",
            "/var/a.xuan",
            "/var/work",
            "/var/work/a.xuan.tmp",
        ] {
            assert!(!watches.changed(&[other.into()], now), "{other}");
        }
        // An atomic save names the temporary file and the file it replaces.
        assert!(watches.changed(&["/var/work/.tmpX1".into(), "/var/work/a.xuan".into()], now));
        watches.take_due(now + DEBOUNCE);
        assert!(watches.changed(&["/private/var/work/a.xuan".into()], now));
    }

    #[test]
    fn rescan_reads_every_file() {
        let mut watches = Watches::default();
        let [a, b] = [Uuid::new_v4(), Uuid::new_v4()];
        watches.watch(a, "/w/a.xuan".into(), "/w".into(), None);
        watches.watch(b, "/x/b.xuan".into(), "/x".into(), None);
        let now = Instant::now();
        watches.rescan(now);
        let due = watches.take_due(now + DEBOUNCE);
        assert_eq!(
            due,
            [(a, PathBuf::from("/w/a.xuan")), (b, "/x/b.xuan".into())]
        );
    }

    #[test]
    fn folders_are_watched_while_a_file_in_them_is() {
        let mut watches = Watches::default();
        let [a, b] = [Uuid::new_v4(), Uuid::new_v4()];
        let changes = watches.watch(a, "/w/a.xuan".into(), "/w".into(), None);
        assert_eq!(changes.add, Some("/w".into()));
        assert_eq!(changes.remove, None);
        // A second file in the same folder needs nothing more.
        assert_eq!(
            watches.watch(b, "/w/b.xuan".into(), "/w".into(), None),
            FolderChanges::default()
        );
        assert_eq!(watches.unwatch(a), FolderChanges::default());
        // Save As elsewhere moves the watch.
        assert_eq!(
            watches.watch(b, "/x/b.xuan".into(), "/x".into(), None),
            FolderChanges {
                add: Some("/x".into()),
                remove: Some("/w".into())
            }
        );
        // Saving under another name in the same folder changes nothing.
        assert_eq!(
            watches.watch(b, "/x/c.xuan".into(), "/x".into(), None),
            FolderChanges::default()
        );
        assert_eq!(
            watches.unwatch(b),
            FolderChanges {
                add: None,
                remove: Some("/x".into())
            }
        );
        assert_eq!(watches.unwatch(b), FolderChanges::default());
        // A file without a folder is in the current one.
        assert_eq!(
            watches.watch(a, "a.xuan".into(), ".".into(), None).add,
            Some(".".into())
        );
    }

    #[test]
    fn a_loaded_file_is_the_new_content_to_compare_with() {
        let mut watches = Watches::default();
        let id = Uuid::new_v4();
        let path = PathBuf::from("/w/a.xuan");
        watches.watch(id, path.clone(), "/w".into(), Some(hash("a")));
        assert!(!watches.needs_load(id, hash("a")));
        assert!(watches.needs_load(id, hash("b")));
        // A half-written file that failed to load is not recorded, so it is tried again.
        assert!(watches.needs_load(id, hash("b")));
        watches.loaded(id, &path, hash("b"));
        assert!(!watches.needs_load(id, hash("b")));
        assert_eq!(watches.path(id), Some(path.as_path()));

        // A load that finished after the document moved to another file is not recorded.
        watches.watch(id, "/w/c.xuan".into(), "/w".into(), Some(hash("c")));
        watches.loaded(id, &path, hash("d"));
        assert_eq!(watches.known(id), Some(hash("c")));
        // Nor after it was closed.
        watches.unwatch(id);
        watches.loaded(id, &path, hash("d"));
        assert_eq!(watches.known(id), None);
        assert!(watches.needs_load(id, hash("d")));
    }

    // The watcher itself, on real files. Reports are awaited with a deadline rather than a
    // fixed sleep, so the tests are quick when the machine is and still pass when it is busy.

    const WAIT: Duration = Duration::from_secs(10);

    fn project(width: u32) -> Document {
        Document::new(width, 8).unwrap()
    }

    fn bytes(document: &Document, directory: &Path) -> Vec<u8> {
        let path = directory.join(format!("{}.scratch", Uuid::new_v4()));
        io::save(document, &path).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(path).unwrap();
        bytes
    }

    /// A watcher following a saved 10-pixel-wide project.
    fn watched() -> (tempfile::TempDir, PathBuf, Uuid, FileWatcher) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("project.xuan");
        let mut known = None;
        io::save_hashed(&project(10), &path, |hash| known = Some(hash)).unwrap();
        let watcher = FileWatcher::spawn(|| {}).unwrap();
        let id = Uuid::new_v4();
        watcher.watch(id, &path, known);
        watcher.flush();
        (directory, path, id, watcher)
    }

    /// The next report, failing the test if none comes in time.
    #[track_caller]
    fn next(watcher: &FileWatcher) -> Report {
        watcher
            .recv_timeout(WAIT)
            .expect("the watcher reports within the deadline")
    }

    /// Skips reports of partial writes until the file loads.
    #[track_caller]
    fn next_change(watcher: &FileWatcher) -> (Uuid, Document) {
        let deadline = Instant::now() + WAIT;
        while let Some(report) =
            watcher.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if let Outcome::Changed(document) = report.outcome {
                return (report.id, *document);
            }
        }
        panic!("no change reported within the deadline");
    }

    #[test]
    fn a_script_overwriting_the_file_is_reported() {
        let (directory, path, id, watcher) = watched();
        fs::write(&path, bytes(&project(20), directory.path())).unwrap();
        let (changed, document) = next_change(&watcher);
        assert_eq!((changed, document.width), (id, 20));
    }

    #[test]
    fn an_atomic_replace_is_reported() {
        let (directory, path, id, watcher) = watched();
        let temporary = directory.path().join(".project.xuan.part");
        fs::write(&temporary, bytes(&project(30), directory.path())).unwrap();
        fs::rename(&temporary, &path).unwrap();
        let (changed, document) = next_change(&watcher);
        assert_eq!((changed, document.width), (id, 30));
        // The new file is followed too: the watch was on its folder, not the old file.
        fs::write(&path, bytes(&project(31), directory.path())).unwrap();
        assert_eq!(next_change(&watcher).1.width, 31);
    }

    #[test]
    fn touching_the_file_changes_nothing() {
        let (_directory, path, id, watcher) = watched();
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        drop(file);
        let report = next(&watcher);
        assert_eq!(report.id, id);
        assert!(
            matches!(report.outcome, Outcome::Unchanged),
            "{:?}",
            report.outcome
        );
    }

    #[test]
    fn xuans_own_save_changes_nothing() {
        let (_directory, path, id, watcher) = watched();
        // The editor tells the watcher what it is about to write, as `save_current` does.
        io::save_hashed(&project(40), &path, |hash| {
            watcher.watch(id, &path, Some(hash))
        })
        .unwrap();
        let report = next(&watcher);
        assert!(
            matches!(report.outcome, Outcome::Unchanged),
            "{:?}",
            report.outcome
        );
    }

    #[test]
    fn a_half_written_file_waits_for_the_rest() {
        let (directory, path, id, watcher) = watched();
        let complete = bytes(&project(50), directory.path());
        let mut file = File::create(&path).unwrap();
        std::io::Write::write_all(&mut file, &complete[..complete.len() / 2]).unwrap();
        file.sync_all().unwrap();
        let report = next(&watcher);
        assert_eq!(report.id, id);
        assert!(
            matches!(report.outcome, Outcome::Failed(_)),
            "{:?}",
            report.outcome
        );
        std::io::Write::write_all(&mut file, &complete[complete.len() / 2..]).unwrap();
        drop(file);
        let (_, document) = next_change(&watcher);
        assert_eq!(document.width, 50);
    }

    #[test]
    fn unwatched_files_are_not_reported() {
        let (directory, path, id, watcher) = watched();
        let other = directory.path().join("other.xuan");
        io::save(&project(10), &other).unwrap();
        let second = Uuid::new_v4();
        watcher.watch(second, &other, None);
        watcher.unwatch(id);
        watcher.flush();
        // Changed first, so a report for it, were it still followed, would come first.
        fs::write(&path, bytes(&project(60), directory.path())).unwrap();
        fs::write(&other, bytes(&project(61), directory.path())).unwrap();
        let (changed, document) = next_change(&watcher);
        assert_eq!((changed, document.width), (second, 61));
        assert!(watcher.try_recv().is_none());
    }
}

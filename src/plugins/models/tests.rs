//! The downloader against a fake transport, which covers every rule without
//! a network, and against a real local HTTPS server (a test certificate
//! authority made with rcgen, served by rustls), which checks that [`Https`]
//! speaks TLS, refuses unknown certificates and leaves redirects to
//! [`download`].
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    net::TcpListener,
    sync::Mutex,
};

use super::*;

const DATA: &[u8] = b"weights of a very small model\n";

fn sha256(data: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

fn model(url: &str, data: &[u8]) -> Model {
    Model {
        id: "net".into(),
        url: url.into(),
        sha256: sha256(data).to_ascii_uppercase(),
        size: data.len() as u64,
        file: Some("net.onnx".into()),
        license: "Apache-2.0".into(),
        source: "Test".into(),
    }
}

/// What the fake serves at one URL.
#[derive(Clone)]
enum Page {
    Body(Vec<u8>),
    /// A body with a `Content-Length` that does not match it.
    Lying(Vec<u8>, u64),
    Redirect(u16, String),
    Status(u16),
    /// A body that never ends.
    Endless,
}

#[derive(Default)]
struct Fake {
    pages: HashMap<String, Page>,
    requested: Mutex<Vec<String>>,
}

impl Fake {
    fn with(mut self, url: &str, page: Page) -> Self {
        self.pages.insert(url.into(), page);
        self
    }

    fn requested(&self) -> Vec<String> {
        self.requested.lock().unwrap().clone()
    }
}

struct Endless;

impl Read for Endless {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        buffer.fill(7);
        Ok(buffer.len())
    }
}

impl Transport for Fake {
    fn get(&self, url: &Url, _size: u64) -> Result<Reply> {
        self.requested.lock().unwrap().push(url.to_string());
        let reply = |status, location, length, body: Box<dyn Read + Send>| Reply {
            status,
            location,
            length,
            body,
        };
        Ok(
            match self.pages.get(url.as_str()).context("no such page")? {
                Page::Body(data) => reply(
                    200,
                    None,
                    Some(data.len() as u64),
                    Box::new(std::io::Cursor::new(data.clone())),
                ),
                Page::Lying(data, length) => reply(
                    200,
                    None,
                    Some(*length),
                    Box::new(std::io::Cursor::new(data.clone())),
                ),
                Page::Redirect(status, to) => {
                    reply(*status, Some(to.clone()), None, Box::new(std::io::empty()))
                }
                Page::Status(status) => reply(*status, None, None, Box::new(std::io::empty())),
                Page::Endless => reply(200, None, None, Box::new(Endless)),
            },
        )
    }
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn a_download_is_verified_then_renamed_into_place() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    let url = "https://models.example/a/net.onnx";
    let fake = Fake::default().with(url, Page::Body(DATA.to_vec()));
    let model = model(url, DATA);
    assert_eq!(state(&model, &models), (State::Missing, None));
    let progress = Progress::default();
    let path = download(&fake, &model, &models, &progress).unwrap();
    assert_eq!(path, models.join("net.onnx"));
    assert_eq!(std::fs::read(&path).unwrap(), DATA);
    assert_eq!(progress.done(), DATA.len() as u64);
    assert_eq!(entries(&models), [".verified.json", "net.onnx"]);
    assert_eq!(
        state(&model, &models),
        (State::Ready, Some(DATA.len() as u64))
    );
    assert_eq!(
        ready_paths(std::slice::from_ref(&model), &models),
        BTreeMap::from([("net".to_owned(), path.clone())])
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&models), 0o700);
        assert_eq!(mode(&path), 0o600);
    }
    // Without a `file`, the name comes from the URL, or else the id.
    let mut unnamed = model.clone();
    unnamed.file = None;
    assert_eq!(unnamed.file_name(), "net.onnx");
    unnamed.url = "https://models.example/get?id=1".into();
    assert_eq!(unnamed.file_name(), "get");
    unnamed.url = "https://models.example/".into();
    assert_eq!(unnamed.file_name(), "net");
}

#[test]
fn wrong_hashes_and_sizes_leave_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://models.example/net.onnx";
    let progress = Progress::default();
    let mut tampered = DATA.to_vec();
    tampered[0] ^= 1;
    for (page, expected) in [
        (Page::Body(tampered), "does not match the SHA-256"),
        (Page::Body(DATA[..4].to_vec()), "is 4 bytes"),
        (
            Page::Lying(DATA[..4].to_vec(), DATA.len() as u64),
            "sent 4 bytes",
        ),
        (
            Page::Lying([DATA, DATA].concat(), DATA.len() as u64),
            "sent more than",
        ),
        (Page::Endless, "sent more than"),
        (Page::Status(404), "HTTP status 404"),
    ] {
        let fake = Fake::default().with(url, page);
        let error = download(&fake, &model(url, DATA), dir.path(), &progress).unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
        assert!(
            !dir.path().join("net.part").exists() && !dir.path().join("net.onnx").exists(),
            "{expected}: {:?}",
            entries(dir.path())
        );
    }
}

#[test]
fn a_failed_download_keeps_the_verified_file() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://models.example/net.onnx";
    let good = Fake::default().with(url, Page::Body(DATA.to_vec()));
    let model = model(url, DATA);
    download(&good, &model, dir.path(), &Progress::default()).unwrap();
    let broken = Fake::default().with(url, Page::Status(500));
    assert!(download(&broken, &model, dir.path(), &Progress::default()).is_err());
    assert_eq!(std::fs::read(dir.path().join("net.onnx")).unwrap(), DATA);
    assert_eq!(state(&model, dir.path()).0, State::Ready);
}

#[test]
fn redirects_are_followed_over_https_only_and_at_most_five_times() {
    let dir = tempfile::tempdir().unwrap();
    let start = "https://models.example/net.onnx";
    // A relative redirect, then one to another https host.
    let fake = Fake::default()
        .with(start, Page::Redirect(302, "/v2/net.onnx".into()))
        .with(
            "https://models.example/v2/net.onnx",
            Page::Redirect(307, "https://cdn.example/net.onnx".into()),
        )
        .with("https://cdn.example/net.onnx", Page::Body(DATA.to_vec()));
    download(&fake, &model(start, DATA), dir.path(), &Progress::default()).unwrap();
    assert_eq!(
        fake.requested(),
        [
            start,
            "https://models.example/v2/net.onnx",
            "https://cdn.example/net.onnx"
        ]
    );

    // A redirect to http is refused before anything is fetched from it.
    let fake = Fake::default()
        .with(
            start,
            Page::Redirect(301, "http://cdn.example/net.onnx".into()),
        )
        .with("http://cdn.example/net.onnx", Page::Body(DATA.to_vec()));
    let error = download(&fake, &model(start, DATA), dir.path(), &Progress::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("which is not https"), "{error}");
    assert_eq!(fake.requested(), [start]);

    // Five redirects are fine, six are not.
    let hop = |n: usize| format!("https://models.example/{n}");
    let mut fake = Fake::default();
    for n in 0..6 {
        fake = fake.with(&hop(n), Page::Redirect(308, hop(n + 1)));
    }
    let fake = fake.with(&hop(6), Page::Body(DATA.to_vec()));
    let error = download(
        &fake,
        &model(&hop(0), DATA),
        dir.path(),
        &Progress::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("more than 5 times"), "{error}");
    assert_eq!(fake.requested().len(), 6);
    download(
        &fake,
        &model(&hop(1), DATA),
        dir.path(),
        &Progress::default(),
    )
    .unwrap();

    // A model URL itself must be https, and a redirect needs a location.
    let error = download(
        &fake,
        &model("http://models.example/net.onnx", DATA),
        dir.path(),
        &Progress::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("not an https URL"), "{error}");
    let fake = Fake::default().with(start, Page::Status(302));
    assert!(download(&fake, &model(start, DATA), dir.path(), &Progress::default()).is_err());
    assert!(
        entries(dir.path())
            .iter()
            .all(|name| !name.ends_with(".part"))
    );
}

/// Serves an endless body and cancels the download once it has started.
#[test]
fn cancelling_stops_the_download_and_removes_the_part_file() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://models.example/net.onnx";
    let fake = Fake::default().with(url, Page::Endless);
    let mut model = model(url, DATA);
    model.size = crate::plugins::manifest::MAX_MODEL_SIZE;
    let progress = Arc::new(Progress::default());
    let worker = {
        let progress = progress.clone();
        let dir = dir.path().to_path_buf();
        std::thread::spawn(move || download(&fake, &model, &dir, &progress))
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while progress.done() < 1024 * 1024 {
        assert!(
            std::time::Instant::now() < deadline,
            "the download never started"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(dir.path().join("net.part").exists());
    progress.cancel();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.is::<Cancelled>(), "{error:#}");
    assert_eq!(entries(dir.path()), Vec::<String>::new());
}

/// Write `data` to `path` and give it a modification time of its own, so
/// the test does not depend on the file system's timestamp resolution.
fn rewrite(path: &Path, data: &[u8]) {
    static SECONDS: AtomicU64 = AtomicU64::new(1_000_000_000);
    std::fs::write(path, data).unwrap();
    let time = std::time::UNIX_EPOCH + Duration::from_secs(SECONDS.fetch_add(1, Ordering::Relaxed));
    let file = OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(time).unwrap();
}

#[test]
fn a_changed_file_is_hashed_again_and_found_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://models.example/net.onnx";
    let fake = Fake::default().with(url, Page::Body(DATA.to_vec()));
    let model = model(url, DATA);
    let path = download(&fake, &model, dir.path(), &Progress::default()).unwrap();
    // The same size with other bytes: no longer trusted without a hash.
    let mut changed = DATA.to_vec();
    changed[3] ^= 0x20;
    rewrite(&path, &changed);
    assert_eq!(state(&model, dir.path()).0, State::Unverified);
    assert!(ready_paths(std::slice::from_ref(&model), dir.path()).is_empty());
    assert!(!verify(&model, dir.path(), &Progress::default()).unwrap());
    // The result is remembered until the file changes again.
    assert_eq!(state(&model, dir.path()).0, State::Corrupt);
    rewrite(&path, DATA);
    assert_eq!(state(&model, dir.path()).0, State::Unverified);
    assert!(verify(&model, dir.path(), &Progress::default()).unwrap());
    assert_eq!(state(&model, dir.path()).0, State::Ready);
    // A file of the wrong size is corrupt without reading it.
    rewrite(&path, b"short");
    assert_eq!(state(&model, dir.path()), (State::Corrupt, Some(5)));
    assert!(!verify(&model, dir.path(), &Progress::default()).unwrap());
    // A plugin update that declares another hash makes the file corrupt.
    rewrite(&path, DATA);
    assert!(verify(&model, dir.path(), &Progress::default()).unwrap());
    let mut updated = model.clone();
    updated.sha256 = sha256(b"other");
    assert_eq!(state(&updated, dir.path()).0, State::Corrupt);
}

#[test]
fn delete_removes_the_file_its_part_and_its_record() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    let url = "https://models.example/net.onnx";
    let fake = Fake::default().with(url, Page::Body(DATA.to_vec()));
    let model = model(url, DATA);
    download(&fake, &model, &models, &Progress::default()).unwrap();
    std::fs::write(models.join("net.part"), b"left over").unwrap();
    delete(&model, &models).unwrap();
    assert_eq!(state(&model, &models), (State::Missing, None));
    assert_eq!(entries(&models), [".verified.json"]);
    assert!(
        !std::fs::read_to_string(models.join(CACHE_FILE))
            .unwrap()
            .contains("net.onnx")
    );
    // Deleting again, or everything, is fine.
    delete(&model, &models).unwrap();
    download(&fake, &model, &models, &Progress::default()).unwrap();
    delete_all(&models).unwrap();
    assert!(!models.exists());
    delete_all(&models).unwrap();
}

/// A tiny HTTPS server on localhost with its own certificate authority.
struct Server {
    port: u16,
    ca: Vec<u8>,
}

impl Server {
    /// Serve `routes` (path → raw HTTP response) to every connection.
    fn start(routes: HashMap<String, Vec<u8>>) -> Self {
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let leaf = rcgen::CertificateParams::new(vec!["localhost".to_owned()])
            .unwrap()
            .signed_by(&key, &ca, &ca_key)
            .unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
        )
        .unwrap();
        let config = Arc::new(config);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let config = config.clone();
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let Ok(connection) = rustls::ServerConnection::new(config) else {
                        return;
                    };
                    let mut tls = rustls::StreamOwned::new(connection, stream);
                    let mut reader = BufReader::new(&mut tls);
                    let mut request = String::new();
                    if reader.read_line(&mut request).is_err() {
                        return;
                    }
                    loop {
                        let mut header = String::new();
                        match reader.read_line(&mut header) {
                            Ok(0) | Err(_) => return,
                            Ok(_) if header.trim().is_empty() => break,
                            Ok(_) => {}
                        }
                    }
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let response = routes.get(&path).cloned().unwrap_or_else(|| {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    });
                    let _ = tls.write_all(&response);
                    let _ = tls.flush();
                    tls.conn.send_close_notify();
                    let _ = tls.flush();
                });
            }
        });
        Self {
            port,
            ca: ca.der().to_vec(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.port)
    }
}

fn ok(body: &[u8]) -> Vec<u8> {
    [
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
        body,
    ]
    .concat()
}

fn redirect(to: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

#[test]
fn downloads_over_real_https_and_checks_the_certificate() {
    let routes = HashMap::from([
        ("/net.onnx".to_owned(), ok(DATA)),
        ("/moved".to_owned(), redirect("/net.onnx")),
        (
            "/insecure".to_owned(),
            redirect("http://localhost:1/net.onnx"),
        ),
        ("/bad".to_owned(), ok(b"not the declared model weights!")),
    ]);
    let server = Server::start(routes);
    let https = Https::with_certificates(std::slice::from_ref(&server.ca));
    let dir = tempfile::tempdir().unwrap();

    let model_at = |path: &str| model(&server.url(path), DATA);
    let path = download(
        &https,
        &model_at("/moved"),
        dir.path(),
        &Progress::default(),
    )
    .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), DATA);

    let error = download(
        &https,
        &model_at("/insecure"),
        dir.path(),
        &Progress::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("which is not https"), "{error}");
    let error = download(&https, &model_at("/bad"), dir.path(), &Progress::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("is 31 bytes"), "{error}");

    // A client that does not trust the test authority refuses the server.
    let stranger = Https::with_certificates(&[]);
    let error = download(
        &stranger,
        &model_at("/net.onnx"),
        dir.path(),
        &Progress::default(),
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("Cannot download"),
        "{error:#}"
    );
    // And the client never speaks plain http, even when asked to directly.
    let plain = Url::parse(&format!("http://localhost:{}/net.onnx", server.port)).unwrap();
    assert!(https.get(&plain, 1).is_err());
    assert!(
        entries(dir.path())
            .iter()
            .all(|name| !name.ends_with(".part"))
    );
}

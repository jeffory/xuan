//! The MCP server: Streamable HTTP on 127.0.0.1, every request checked for
//! the Host, the Origin and the token before rmcp sees it.
use std::{
    collections::VecDeque,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use axum::{
    Router,
    extract::{Request, State},
    http::header,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, ListResourcesResult,
        ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::{auth, editor::Editor, tools};

/// The port tried first unless the settings name another.
pub const DEFAULT_PORT: u16 = 8765;
/// How often a taken port is tried again.
const RETRY: std::time::Duration = std::time::Duration::from_secs(10);
/// The MCP endpoint's path.
pub const PATH: &str = "/mcp";
/// Tool calls the pane lists.
const ACTIVITY: usize = 8;

/// What the server is doing, for the pane.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    Listening {
        port: u16,
    },
    /// Another program listens on the configured port. Clients set up for
    /// it would send it their requests, token included, so the server does
    /// not move to another port until the user says so.
    PortInUse {
        port: u16,
    },
    Failed(String),
}

/// State shared by the server, the pane and the settings.
pub struct Shared {
    pub token: RwLock<String>,
    pub status: Mutex<Status>,
    /// The port the settings ask for (0: let the system choose).
    pub port: Mutex<u16>,
    /// The user chose, in the pane, to listen on any free port because the
    /// configured one is taken. Cleared when the port setting changes.
    pub any_port: std::sync::atomic::AtomicBool,
    pub data_dir: Mutex<PathBuf>,
    /// Recent tool calls, newest last.
    pub activity: Mutex<VecDeque<String>>,
    /// Woken to listen again, after the port changed.
    pub restart: tokio::sync::Notify,
    /// Called when the pane should be drawn again.
    pub changed: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// MCP session ids this server issued, with the label Xuan shows.
    sessions: Mutex<Sessions>,
}

/// Session ids rmcp minted in answers to `initialize`, as this server saw
/// them go out. Only these name an edit session; anything else a client
/// sends in `Mcp-Session-Id` is ignored.
#[derive(Default)]
struct Sessions {
    known: VecDeque<(String, u32)>,
    next: u32,
}

/// Session ids remembered; the oldest are forgotten first.
const SESSIONS: usize = 256;

/// The edit session of every request that has no session this server
/// issued: clients on the stateless protocol (2026-07-28 and later) or
/// without a session header. They all share one answer, and Xuan's prompt
/// says so.
pub const STATELESS_SESSION: &str = "every MCP client without a session (they share this answer)";

impl Shared {
    pub fn new(token: String, data_dir: PathBuf) -> Self {
        Self {
            token: RwLock::new(token),
            status: Mutex::new(Status::Starting),
            port: Mutex::new(DEFAULT_PORT),
            any_port: std::sync::atomic::AtomicBool::new(false),
            data_dir: Mutex::new(data_dir),
            activity: Mutex::new(VecDeque::new()),
            restart: tokio::sync::Notify::new(),
            changed: Mutex::new(None),
            sessions: Mutex::default(),
        }
    }

    /// Remember a session id the server just issued.
    fn remember(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            if sessions.known.iter().any(|(known, _)| known == id) {
                return;
            }
            sessions.next += 1;
            let label = sessions.next;
            sessions.known.push_back((id.to_owned(), label));
            while sessions.known.len() > SESSIONS {
                sessions.known.pop_front();
            }
        }
    }

    /// The edit session for a client's `Mcp-Session-Id`: a label the server
    /// made, never the client's own text.
    pub fn edit_session(&self, header: Option<&str>) -> String {
        let label = header.and_then(|id| {
            let sessions = self.sessions.lock().ok()?;
            sessions
                .known
                .iter()
                .find(|(known, _)| known == id)
                .map(|(_, label)| format!("MCP client {label}"))
        });
        label.unwrap_or_else(|| STATELESS_SESSION.to_owned())
    }

    pub fn token(&self) -> String {
        self.token.read().map(|t| t.clone()).unwrap_or_default()
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(Status::Starting)
    }

    fn set_status(&self, status: Status) {
        if let Ok(mut current) = self.status.lock() {
            *current = status;
        }
        self.notify();
    }

    pub fn notify(&self) {
        if let Ok(changed) = self.changed.lock()
            && let Some(changed) = changed.as_ref()
        {
            changed();
        }
    }

    fn record(&self, line: String) {
        if let Ok(mut activity) = self.activity.lock() {
            activity.push_back(line);
            while activity.len() > ACTIVITY {
                activity.pop_front();
            }
        }
        self.notify();
    }

    pub fn url(port: u16) -> String {
        format!("http://127.0.0.1:{port}{PATH}")
    }
}

/// How the server explains itself to clients.
const INSTRUCTIONS: &str = "Xuan is an image editor running on the user's computer. Start with get_document to see the layers and get_preview to see the image. Edits apply live as one undo step each; the first edit of a session asks the user in Xuan, who may refuse. Layer ids, coordinates and sizes are in document pixels with the origin at the top-left. Saving, exporting and opening files show a dialog to the user.";

/// The MCP side: tools and resources over the editor.
#[derive(Clone)]
pub struct Xuan {
    pub editor: Arc<dyn Editor>,
    pub shared: Arc<Shared>,
}

/// The edit session of a request: the label of the MCP session this server
/// issued to the client, or the one session every other client shares.
fn session_of(context: &RequestContext<RoleServer>, shared: &Shared) -> Option<String> {
    let header = context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.headers.get(SESSION_HEADER))
        .and_then(|value| value.to_str().ok());
    Some(shared.edit_session(header))
}

const SESSION_HEADER: &str = "mcp-session-id";

impl Xuan {
    fn incoming(&self) -> PathBuf {
        self.shared
            .data_dir
            .lock()
            .map(|dir| dir.join("incoming"))
            .unwrap_or_default()
    }

    /// Run blocking editor requests off the async workers.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Self) -> T + Send + 'static,
    ) -> Result<T, ErrorData> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || work(&this))
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }
}

impl ServerHandler for Xuan {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(
            Implementation::new("xuan", env!("CARGO_PKG_VERSION")).with_title("Xuan image editor"),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools::list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let session = session_of(&context, &self.shared);
        let name = request.name.to_string();
        let arguments = request.arguments.unwrap_or_default();
        let result = self
            .blocking(move |this| {
                let incoming = this.incoming();
                let cx = tools::Context {
                    editor: this.editor.as_ref(),
                    session: session.as_deref(),
                    incoming: &incoming,
                };
                let result = tools::call(&cx, &name, arguments);
                let outcome = if result.is_error == Some(true) {
                    "failed"
                } else {
                    "done"
                };
                this.shared.record(format!("{name}: {outcome}"));
                result
            })
            .await?;
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let session = None::<String>;
        let resources = self
            .blocking(move |this| resources(this.editor.as_ref(), session.as_deref()))
            .await?;
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let session = session_of(&context, &self.shared);
        let uri = request.uri;
        let contents = self
            .blocking(move |this| read(this.editor.as_ref(), session.as_deref(), &uri))
            .await??;
        Ok(ReadResourceResult::new(contents).into())
    }
}

/// The resources: the document manifest, the flattened preview, the
/// selection mask and a thumbnail for each layer of the current document.
pub fn resources(editor: &dyn Editor, session: Option<&str>) -> Vec<Resource> {
    let mut list = vec![
        Resource::new("xuan://document", "document")
            .with_title("Current document")
            .with_description("Size, selection and layers of the current document, as JSON")
            .with_mime_type("application/json"),
        Resource::new("xuan://document/preview.png", "preview")
            .with_title("Flattened preview")
            .with_description("The current document flattened, longest side 1024 pixels")
            .with_mime_type("image/png"),
        Resource::new("xuan://document/selection.png", "selection")
            .with_title("Selection mask")
            .with_description("The selection cropped to its bounds; white is selected")
            .with_mime_type("image/png"),
    ];
    if let Ok(document) = editor.request(session, "document/get", json!({})) {
        for layer in document["layers"].as_array().into_iter().flatten() {
            let (Some(id), Some(name)) = (layer["id"].as_str(), layer["name"].as_str()) else {
                continue;
            };
            list.push(
                Resource::new(
                    format!("xuan://layers/{id}/thumbnail.png"),
                    format!("layer {id}"),
                )
                .with_title(format!("Layer “{name}”"))
                .with_description("The layer's pixels, longest side 256 pixels")
                .with_mime_type("image/png"),
            );
        }
    }
    list
}

/// Read one resource.
pub fn read(
    editor: &dyn Editor,
    session: Option<&str>,
    uri: &str,
) -> Result<Vec<ResourceContents>, ErrorData> {
    let failed = |message: String| ErrorData::internal_error(message, None);
    let request = |method: &str, params: Value| {
        editor
            .request(session, method, params)
            .map_err(|error| failed(error.message))
    };
    let png = |export: Value| -> Result<Vec<ResourceContents>, ErrorData> {
        let (data, _) = tools::read_export(&export).map_err(failed)?;
        Ok(vec![
            ResourceContents::blob(data, uri).with_mime_type("image/png"),
        ])
    };
    match uri {
        "xuan://document" => {
            let document = request("document/get", json!({}))?;
            Ok(vec![
                ResourceContents::text(
                    serde_json::to_string_pretty(&document).unwrap_or_default(),
                    uri,
                )
                .with_mime_type("application/json"),
            ])
        }
        "xuan://document/preview.png" => png(request(
            "document/export",
            json!({"max_side": tools::DEFAULT_PREVIEW}),
        )?),
        "xuan://document/selection.png" => {
            let export = request("selection/export", json!({}))?;
            if export.is_null() {
                return Err(ErrorData::resource_not_found("Nothing is selected", None));
            }
            png(export)
        }
        _ => {
            let layer = uri
                .strip_prefix("xuan://layers/")
                .and_then(|rest| rest.strip_suffix("/thumbnail.png"))
                .ok_or_else(|| ErrorData::resource_not_found(format!("No resource {uri}"), None))?;
            png(request(
                "layer/export",
                json!({"layer": layer, "what": "pixels", "max_side": 256}),
            )?)
        }
    }
}

/// The HTTP application: `/mcp`, behind the checks.
pub fn router(xuan: Xuan, port: u16) -> Router {
    let shared = xuan.shared.clone();
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(["127.0.0.1", "localhost"])
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || Ok(xuan.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new()
        .nest_service(PATH, service)
        .layer(middleware::from_fn_with_state((shared, port), guard))
}

async fn guard(
    State((shared, port)): State<(Arc<Shared>, u16)>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(rejection) = auth::check(request.headers(), &shared.token(), port) {
        let mut response = (rejection.status(), rejection.message()).into_response();
        if rejection == auth::Rejection::Token {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                "Bearer".parse().expect("valid header"),
            );
        }
        return response;
    }
    // A session id in the answer to a request that had none was issued by
    // rmcp just now (to `initialize`): that is the only way one is trusted.
    let issued_here = !request.headers().contains_key(SESSION_HEADER);
    let response = next.run(request).await;
    if issued_here
        && let Some(id) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
    {
        shared.remember(id);
    }
    response
}

/// Listen on 127.0.0.1 only, on exactly this port (0: any free one). A taken
/// port is an error, never a reason to move silently.
pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await
}

/// Serve until the plugin stops, listening again when the port changes.
pub fn run(xuan: Xuan) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            xuan.shared
                .set_status(Status::Failed(format!("Cannot start: {error}")));
            return;
        }
    };
    runtime.block_on(async move {
        loop {
            let configured = xuan.shared.port.lock().map(|p| *p).unwrap_or(DEFAULT_PORT);
            let any = (xuan.shared.any_port).load(std::sync::atomic::Ordering::Relaxed);
            let wanted = if any { 0 } else { configured };
            match bind(wanted).await {
                Ok(listener) => {
                    let port = listener.local_addr().map(|a| a.port()).unwrap_or(wanted);
                    write_connection(&xuan.shared, port);
                    xuan.shared.set_status(Status::Listening { port });
                    let app = router(xuan.clone(), port);
                    let shared = xuan.shared.clone();
                    let served = axum::serve(listener, app)
                        .with_graceful_shutdown(async move { shared.restart.notified().await })
                        .await;
                    if let Err(error) = served {
                        xuan.shared.set_status(Status::Failed(error.to_string()));
                        xuan.shared.restart.notified().await;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    xuan.shared.set_status(Status::PortInUse { port: wanted });
                    // Try again now and then, or when the user decides.
                    let _ = tokio::time::timeout(RETRY, xuan.shared.restart.notified()).await;
                }
                Err(error) => {
                    xuan.shared.set_status(Status::Failed(format!(
                        "Cannot listen on 127.0.0.1:{wanted}: {error}"
                    )));
                    xuan.shared.restart.notified().await;
                }
            }
        }
    });
}

/// `connection.json` in the data folder: where the server listens (the
/// token is in its own file), for scripts and tests.
fn write_connection(shared: &Shared, port: u16) {
    if let Ok(dir) = shared.data_dir.lock() {
        let text = json!({"url": Shared::url(port), "port": port}).to_string();
        let _ = auth::write_private(&dir.join("connection.json"), text.as_bytes());
    }
}

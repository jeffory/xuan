//! The HTTP layer and the MCP protocol, through an in-process client (the
//! router called as a tower service) against a fake editor.
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use rmcp::model::{
    CallToolResult, InitializeResult, ListResourcesResult, ListToolsResult, ReadResourceResult,
    ResourceContents,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use xuan_plugin::CancelToken;

use crate::{
    editor::{CANCELLED, Editor, EditorError, WITHDRAWN},
    server::{self, Shared, Waits, Xuan},
    tools,
};

/// A 1 × 1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
const PORT: u16 = 8765;

/// Answers like Xuan with one document, and records every request.
struct FakeEditor {
    dir: PathBuf,
    log: Mutex<Vec<(Option<String>, String, Value)>>,
    /// Refuse edits as a user who said Deny.
    deny: bool,
    /// Requests with this method wait, as for a user who has not answered,
    /// until `answer` is set or they are withdrawn.
    hold: Option<&'static str>,
    answer: AtomicBool,
    /// Methods of the requests that were withdrawn while they waited.
    withdrawn: Mutex<Vec<String>>,
}

impl FakeEditor {
    fn new(deny: bool) -> Arc<Self> {
        let dir = std::env::temp_dir().join(format!("xuan-mcp-test-{}", crate::auth::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Self {
            dir,
            log: Mutex::default(),
            deny,
            hold: None,
            answer: AtomicBool::new(false),
            withdrawn: Mutex::default(),
        })
    }

    /// An editor whose `method` requests wait for the user.
    fn holding(method: &'static str) -> Arc<Self> {
        let mut editor = Arc::into_inner(Self::new(false)).unwrap();
        editor.hold = Some(method);
        Arc::new(editor)
    }

    fn withdrawn(&self) -> Vec<String> {
        self.withdrawn.lock().unwrap().clone()
    }

    /// Wait (in a test thread) until a request with this method arrived.
    async fn wait_for(&self, method: &str) {
        for _ in 0..500 {
            if self.requests().iter().any(|(_, m, _)| m == method) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{method} never reached the editor");
    }

    fn requests(&self) -> Vec<(Option<String>, String, Value)> {
        self.log.lock().unwrap().clone()
    }

    fn export(&self) -> Value {
        let path = self.dir.join(format!("{}.png", crate::auth::generate()));
        std::fs::write(&path, STANDARD.decode(PNG).unwrap()).unwrap();
        json!({"path": path, "width": 1, "height": 1, "x": 0.0, "y": 0.0, "scale": 1.0})
    }
}

impl Editor for FakeEditor {
    fn request(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        cancel: &CancelToken,
    ) -> Result<Value, EditorError> {
        if cancel.is_cancelled() {
            return Err(EditorError {
                code: WITHDRAWN,
                message: format!("{method} was withdrawn"),
            });
        }
        self.log.lock().unwrap().push((
            session.map(str::to_owned),
            method.to_owned(),
            params.clone(),
        ));
        if self.hold == Some(method) {
            // Waiting for the user, as Xuan holds the request.
            while !self.answer.load(Ordering::SeqCst) {
                if cancel.is_cancelled() {
                    self.withdrawn.lock().unwrap().push(method.to_owned());
                    return Err(EditorError {
                        code: WITHDRAWN,
                        message: format!("{method} was withdrawn"),
                    });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        match method {
            "document/get" => Ok(json!({
                "id": "doc", "width": 64, "height": 48, "selection": null,
                "layers": [{"id": "base", "name": "Background", "kind": "image", "opacity": 1.0}],
            })),
            "document/list" => {
                Ok(json!({"documents": [{"id": "doc", "title": "Untitled", "current": true}]}))
            }
            "document/export" | "layer/export" => Ok(self.export()),
            "selection/export" => Ok(Value::Null),
            "session/status" => {
                Ok(json!({"edit_prompt": "session", "edits": "ask", "auto": false}))
            }
            "document/edit" | "host/run" if self.deny => Err(EditorError {
                code: CANCELLED,
                message: "The user did not allow this plugin to edit documents in this session"
                    .into(),
            }),
            "document/edit" => Ok(json!({"ok": true, "layers": ["new-layer"]})),
            "host/run" | "document/activate" => Ok(json!({"ok": true})),
            // As Xuan words a file that fails to load.
            "file/open" => Err(EditorError {
                code: -32603,
                message: "Could not open /home/someone/Secret Plans/broken.png\n\nformat error at \"/home/someone/Secret Plans/broken.png\"".into(),
            }),
            "file/save_as" => Err(EditorError {
                code: CANCELLED,
                message: "The user cancelled the save dialog".into(),
            }),
            other => Err(EditorError {
                code: -32601,
                message: format!("Unknown method `{other}`"),
            }),
        }
    }
}

fn app(editor: Arc<FakeEditor>) -> (axum::Router, Arc<Shared>) {
    app_with(editor, Waits::default())
}

fn app_with(editor: Arc<FakeEditor>, waits: Waits) -> (axum::Router, Arc<Shared>) {
    let mut shared = Shared::new("k".repeat(64), editor.dir.clone());
    shared.waits = waits;
    let shared = Arc::new(shared);
    let xuan = Xuan {
        editor,
        shared: shared.clone(),
    };
    (server::router(xuan, PORT), shared)
}

/// A client of the router, in process.
struct Client {
    app: axum::Router,
    session: Option<String>,
    next: i64,
}

impl Client {
    fn new(app: axum::Router) -> Self {
        Self {
            app,
            session: None,
            next: 1,
        }
    }

    fn request(&self, body: &Value) -> Request<Body> {
        let mut request = Request::post("/mcp")
            .header(header::HOST, format!("127.0.0.1:{PORT}"))
            .header(header::AUTHORIZATION, format!("Bearer {}", "k".repeat(64)))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        request.body(Body::from(body.to_string())).unwrap()
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, String) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = tokio::time::timeout(Duration::from_secs(10), response.into_body().collect())
            .await
            .expect("the response ends")
            .unwrap()
            .to_bytes();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    /// A JSON-RPC request; returns the response message.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let (status, headers, text) = self.send(self.request(&body)).await;
        assert_eq!(status, StatusCode::OK, "{method}: {text}");
        if let Some(session) = headers.get("mcp-session-id") {
            self.session = Some(session.to_str().unwrap().to_owned());
        }
        // Server-sent events or plain JSON: find the response to this id.
        let messages: Vec<Value> = if text.trim_start().starts_with('{') {
            vec![serde_json::from_str(&text).unwrap()]
        } else {
            text.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .filter_map(|data| serde_json::from_str(data.trim()).ok())
                .collect()
        };
        messages
            .into_iter()
            .find(|message| message["id"] == id)
            .unwrap_or_else(|| panic!("no response to {method}: {text}"))
    }

    async fn notify(&self, method: &str) -> StatusCode {
        let body = json!({"jsonrpc": "2.0", "method": method});
        self.send(self.request(&body)).await.0
    }

    async fn initialize(&mut self) -> InitializeResult {
        let response = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1"},
                }),
            )
            .await;
        let result = serde_json::from_value(response["result"].clone()).unwrap();
        assert_eq!(
            self.notify("notifications/initialized").await,
            StatusCode::ACCEPTED
        );
        result
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> CallToolResult {
        let response = self
            .call("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        serde_json::from_value(response["result"].clone())
            .unwrap_or_else(|error| panic!("{name}: {error}: {response}"))
    }
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn requests_without_the_token_or_from_browsers_are_refused() {
    let editor = FakeEditor::new(false);
    let (app, shared) = app(editor.clone());
    let client = Client::new(app);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let refused = |request: Request<Body>| async { client.send(request).await };

    let mut request = client.request(&body);
    request.headers_mut().remove(header::AUTHORIZATION);
    let (status, headers, _) = refused(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");

    let mut request = client.request(&body);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", "x".repeat(64)).parse().unwrap(),
    );
    assert_eq!(refused(request).await.0, StatusCode::UNAUTHORIZED);

    // DNS rebinding: the right token from a page served by evil.example.
    let mut request = client.request(&body);
    request.headers_mut().insert(
        header::HOST,
        format!("evil.example:{PORT}").parse().unwrap(),
    );
    assert_eq!(refused(request).await.0, StatusCode::FORBIDDEN);
    let mut request = client.request(&body);
    request
        .headers_mut()
        .insert(header::ORIGIN, "http://localhost:3000".parse().unwrap());
    assert_eq!(refused(request).await.0, StatusCode::FORBIDDEN);
    // Nothing reached the editor.
    assert!(editor.requests().is_empty());

    // A new token takes effect at once.
    let mut client = client;
    client.initialize().await;
    *shared.token.write().unwrap() = "n".repeat(64);
    let (status, ..) = client.send(client.request(&body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_server_speaks_mcp_initialize_tools_and_resources() {
    let editor = FakeEditor::new(false);
    let (app, shared) = app(editor.clone());
    let mut client = Client::new(app);
    let info = client.initialize().await;
    assert!(info.capabilities.tools.is_some());
    assert!(info.capabilities.resources.is_some());
    assert_eq!(info.server_info.name, "xuan");
    assert!(info.instructions.unwrap().contains("undo step"));
    assert!(client.session.is_some(), "a session id");

    let tools: ListToolsResult =
        serde_json::from_value(client.call("tools/list", json!({})).await["result"].clone())
            .unwrap();
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in [
        "list_documents",
        "get_document",
        "get_preview",
        "set_layer",
        "create_text_layer",
        "create_shape_layer",
        "create_image_layer",
        "select_shape",
        "select_color",
        "paint_stroke",
        "fill_gradient",
        "apply_filter",
        "apply_adjustment",
        "merge_layers",
        "group_layers",
        "move_layer",
        "crop_canvas",
        "resize_canvas",
        "undo",
        "redo",
        "save_document",
        "export_document",
        "open_document",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    for tool in &tools.tools {
        assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
        assert!(tool.description.is_some(), "{}", tool.name);
    }
    let preview = tools
        .tools
        .iter()
        .find(|t| t.name == "get_preview")
        .unwrap();
    assert_eq!(
        preview.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
    let set = tools.tools.iter().find(|t| t.name == "set_layer").unwrap();
    assert_eq!(
        set.annotations.as_ref().unwrap().read_only_hint,
        Some(false)
    );

    // A read, an edit and an image, each mapped onto the plugin protocol.
    let document = client.tool("get_document", json!({})).await;
    assert_ne!(document.is_error, Some(true));
    assert!(text_of(&document).contains("Background"));
    let set = client
        .tool(
            "set_layer",
            json!({"layer": "base", "opacity": 0.5, "x": 4}),
        )
        .await;
    assert_ne!(set.is_error, Some(true), "{}", text_of(&set));
    let preview = client
        .tool("get_preview", json!({"max_side": 100000}))
        .await;
    let image = preview.content[0].as_image().expect("an image");
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(
        STANDARD.decode(&image.data).unwrap(),
        STANDARD.decode(PNG).unwrap()
    );

    let requests = editor.requests();
    let (edit_session, _, edit) = requests
        .iter()
        .find(|(_, method, _)| method == "document/edit")
        .unwrap();
    // The edit names a label for the session the server issued, so Xuan
    // asks once per client and never shows the client's own text.
    assert_eq!(edit_session.as_deref(), Some("MCP client 1"));
    assert_eq!(edit["name"], "Set Layer");
    assert_eq!(
        edit["edits"],
        json!([{"op": "set", "layer": "base", "opacity": 0.5}, {"op": "transform", "layer": "base", "x": 4}])
    );
    let export = requests
        .iter()
        .find(|(_, m, _)| m == "document/export")
        .unwrap();
    assert_eq!(export.2["max_side"], 4096, "clamped");
    // The exported file is removed once read.
    assert_eq!(std::fs::read_dir(&editor.dir).unwrap().count(), 0);

    // Resources.
    let resources: ListResourcesResult =
        serde_json::from_value(client.call("resources/list", json!({})).await["result"].clone())
            .unwrap();
    let uris: Vec<&str> = resources.resources.iter().map(|r| r.uri.as_str()).collect();
    assert_eq!(
        uris,
        [
            "xuan://document",
            "xuan://document/preview.png",
            "xuan://document/selection.png",
            "xuan://layers/base/thumbnail.png"
        ]
    );
    let read = |uri: &str| json!({"uri": uri});
    let manifest: ReadResourceResult = serde_json::from_value(
        client.call("resources/read", read("xuan://document")).await["result"].clone(),
    )
    .unwrap();
    assert!(
        matches!(&manifest.contents[0], ResourceContents::TextResourceContents { text, .. } if text.contains("Background"))
    );
    let thumbnail: ReadResourceResult = serde_json::from_value(
        client
            .call("resources/read", read("xuan://layers/base/thumbnail.png"))
            .await["result"]
            .clone(),
    )
    .unwrap();
    assert!(
        matches!(&thumbnail.contents[0], ResourceContents::BlobResourceContents { mime_type: Some(m), .. } if m == "image/png")
    );
    let missing = client
        .call("resources/read", read("xuan://document/selection.png"))
        .await;
    assert!(missing["error"].is_object(), "{missing}");
    assert!(shared.activity.lock().unwrap().len() >= 3);
}

#[tokio::test]
async fn fill_gradient_sends_a_gradient_edit_with_its_stops() {
    let editor = FakeEditor::new(false);
    let (app, _shared) = app(editor.clone());
    let mut client = Client::new(app);
    client.initialize().await;
    let stops = json!([
        {"position": 0, "color": "#ff8800"},
        {"position": 0.5, "color": "#ffcc0080"},
        {"position": 1, "color": "#001133"},
    ]);
    let result = client
        .tool(
            "fill_gradient",
            json!({
                "layer": "base", "start": [0, 0], "end": [0, 600], "stops": stops,
                "radial": true, "opacity": 0.8, "mask": false,
            }),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    let requests = editor.requests();
    let (_, method, params) = requests.last().unwrap();
    assert_eq!(method, "document/edit");
    assert_eq!(params["name"], "Gradient");
    assert_eq!(
        params["edits"],
        json!([{
            "op": "gradient", "layer": "base", "start": [0, 0], "end": [0, 600],
            "stops": stops, "radial": true, "opacity": 0.8, "mask": false,
        }])
    );
    let listed: ListToolsResult =
        serde_json::from_value(client.call("tools/list", json!({})).await["result"].clone())
            .unwrap();
    let tool = listed
        .tools
        .iter()
        .find(|t| t.name == "fill_gradient")
        .unwrap();
    assert_eq!(
        tool.input_schema["required"],
        json!(["start", "end", "stops"])
    );
    assert_eq!(tool.input_schema["properties"]["stops"]["minItems"], 2);

    let unknown = client
        .tool(
            "fill_gradient",
            json!({"start": [0, 0], "end": [1, 0], "stops": stops, "angle": 3}),
        )
        .await;
    assert!(text_of(&unknown).contains("Unknown argument `angle`"));
}

#[tokio::test]
async fn refusals_and_bad_arguments_are_tool_errors_the_model_can_read() {
    let editor = FakeEditor::new(true);
    let (app, shared) = app(editor.clone());
    let mut client = Client::new(app);
    client.initialize().await;
    let denied = client.tool("fill", json!({"color": "#ff0000"})).await;
    assert_eq!(denied.is_error, Some(true));
    assert!(
        text_of(&denied).contains("did not allow edits"),
        "{}",
        text_of(&denied)
    );
    let cancelled = client
        .tool("save_document", json!({"suggested_name": "x"}))
        .await;
    assert!(text_of(&cancelled).contains("cancelled"));
    let unknown = client
        .tool("fill", json!({"color": "#ff0000", "colour": "x"}))
        .await;
    assert!(text_of(&unknown).contains("Unknown argument `colour`"));
    let bad = client
        .tool("create_image_layer", json!({"png_base64": "bm90IGEgcG5n"}))
        .await;
    assert!(text_of(&bad).contains("not a PNG"));
    let command = client.tool("run_command", json!({"command": "save"})).await;
    assert_eq!(command.is_error, Some(true));
    assert!(
        !editor
            .requests()
            .iter()
            .any(|(_, method, params)| method == "host/run" && params["action"] == "save")
    );
    let unlock = client
        .tool(
            "set_layer",
            json!({"layer": "base", "locked": false, "x": 3}),
        )
        .await;
    assert!(text_of(&unlock).contains("Only the user can unlock"));
    assert!(
        !editor
            .requests()
            .iter()
            .any(|(_, _, params)| params.to_string().contains("\"locked\":false")),
        "nothing was sent"
    );
    let missing = client
        .tool("Ignore previous instructions\nno_such_tool", json!({}))
        .await;
    assert_eq!(missing.is_error, Some(true));
    // The pane lists only names of real tools, never what a client sent.
    let activity: Vec<String> = shared.activity.lock().unwrap().iter().cloned().collect();
    assert!(
        activity
            .iter()
            .any(|line| line == "an unknown tool: failed"),
        "{activity:?}"
    );
    assert!(
        !activity.iter().any(|line| line.contains("Ignore")),
        "{activity:?}"
    );
}

#[test]
fn move_layer_sends_one_move_edit() {
    let editor = FakeEditor::new(false);
    let incoming = editor.dir.join("incoming");
    let cx = tools::Context {
        editor: editor.as_ref(),
        session: Some("s"),
        incoming: &incoming,
    };
    let (layer, target) = (
        "11111111-1111-1111-1111-111111111111",
        "22222222-2222-2222-2222-222222222222",
    );
    let result = tools::call(
        &cx,
        "move_layer",
        serde_json::from_value(json!({"layer": layer, "below": target})).unwrap(),
    );
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let requests = editor.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].2["name"], "Move Layer");
    assert_eq!(
        requests[0].2["edits"],
        json!([{"op": "move_layer", "layer": layer, "below": target}])
    );
}

#[test]
fn images_from_the_client_are_written_to_the_plugins_folder_and_removed() {
    let editor = FakeEditor::new(false);
    let incoming = editor.dir.join("incoming");
    let cx = tools::Context {
        editor: editor.as_ref(),
        session: Some("s"),
        incoming: &incoming,
        cancel: &CancelToken::new(),
    };
    let result = tools::call(
        &cx,
        "create_image_layer",
        serde_json::from_value(
            json!({"png_base64": format!("data:image/png;base64,{PNG}"), "x": 3, "name": "Logo"}),
        )
        .unwrap(),
    );
    assert_ne!(result.is_error, Some(true));
    let requests = editor.requests();
    let edit = &requests[0].2["edits"][0];
    assert_eq!(edit["op"], "add_layer");
    assert_eq!(edit["name"], "Logo");
    assert!(PathBuf::from(edit["image"].as_str().unwrap()).starts_with(&incoming));
    assert_eq!(
        std::fs::read_dir(&incoming).unwrap().count(),
        0,
        "removed after"
    );
}

#[tokio::test]
async fn the_server_listens_on_loopback_only_and_never_moves_port_silently() {
    let first = server::bind(0).await.unwrap();
    let address = first.local_addr().unwrap();
    assert!(address.ip().is_loopback(), "{address}");
    assert_eq!(address.ip().to_string(), "127.0.0.1");
    // A taken port is not swapped for another.
    let taken = server::bind(address.port()).await.unwrap_err();
    assert_eq!(taken.kind(), std::io::ErrorKind::AddrInUse);
    // The server says so and waits for the user's choice.
    let editor = FakeEditor::new(false);
    let shared = Arc::new(Shared::new("k".repeat(64), editor.dir.clone()));
    *shared.port.lock().unwrap() = address.port();
    let xuan = Xuan {
        editor: editor.clone(),
        shared: shared.clone(),
    };
    std::thread::spawn(move || server::run(xuan));
    let wait_for = |wanted: &dyn Fn(&server::Status) -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !wanted(&shared.status()) {
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                shared.status()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait_for(&|status| {
        *status
            == server::Status::PortInUse {
                port: address.port(),
            }
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        shared.status(),
        server::Status::PortInUse {
            port: address.port()
        }
    );
    let pane = crate::pane::tree(&shared, None).to_string();
    assert!(pane.contains("in use by another program"), "{pane}");
    // Only once the user chooses does it listen elsewhere.
    shared
        .any_port
        .store(true, std::sync::atomic::Ordering::Relaxed);
    shared.restart.notify_one();
    wait_for(
        &|status| matches!(status, server::Status::Listening { port } if *port != address.port()),
    );

    let second = server::bind(0).await.unwrap();
    let port = second.local_addr().unwrap().port();
    assert!(second.local_addr().unwrap().ip().is_loopback());

    // A real connection over TCP gets the same checks.
    let editor = FakeEditor::new(false);
    let shared = Arc::new(Shared::new("k".repeat(64), editor.dir.clone()));
    let app = server::router(Xuan { editor, shared }, port);
    tokio::spawn(server::serve(
        second,
        app,
        server::Limits::default(),
        std::future::pending(),
    ));
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    drop(first);
}

#[tokio::test]
async fn edit_sessions_are_only_the_ones_the_server_issued() {
    let editor = FakeEditor::new(false);
    let (app, _) = app(editor.clone());
    // The stateless protocol (2026-07-28) has no initialize and carries its
    // version in every request.
    let stateless = json!({
        "name": "fill", "arguments": {"color": "#ff0000"},
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
        },
    });
    for session in [Some("forged-session-123"), None] {
        let mut client = Client::new(app.clone());
        client.session = session.map(str::to_owned);
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": stateless});
        let mut request = client.request(&body);
        request
            .headers_mut()
            .insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
        request
            .headers_mut()
            .insert("mcp-method", "tools/call".parse().unwrap());
        request
            .headers_mut()
            .insert("mcp-name", "fill".parse().unwrap());
        let (status, _, text) = client.send(request).await;
        assert_eq!(status, StatusCode::OK, "{text}");
    }
    // On the legacy protocol rmcp itself refuses a session it did not issue.
    let mut forged = Client::new(app.clone());
    forged.session = Some("forged-session-123".into());
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": "fill", "arguments": {"color": "#ff0000"}}});
    assert_eq!(
        forged.send(forged.request(&body)).await.0,
        StatusCode::NOT_FOUND
    );
    // Two clients that did initialize get their own sessions.
    let mut first = Client::new(app.clone());
    first.initialize().await;
    first.tool("fill", json!({"color": "#0000ff"})).await;
    let mut second = Client::new(app.clone());
    second.initialize().await;
    second.tool("fill", json!({"color": "#0000ff"})).await;

    let sessions: Vec<String> = editor
        .requests()
        .into_iter()
        .filter(|(_, method, _)| method == "document/edit")
        .map(|(session, ..)| session.unwrap())
        .collect();
    assert!(
        !sessions.iter().any(|s| s.contains("forged")),
        "{sessions:?}"
    );
    let (issued, shared): (Vec<&String>, Vec<&String>) =
        sessions.iter().partition(|s| s.starts_with("MCP client "));
    assert_eq!(issued, ["MCP client 1", "MCP client 2"], "{sessions:?}");
    assert!(!shared.is_empty());
    assert!(shared.iter().all(|s| *s == server::STATELESS_SESSION));
}

#[tokio::test]
async fn idle_and_excess_connections_are_closed() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let editor = FakeEditor::new(false);
    let listener = server::bind(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shared = Arc::new(Shared::new("k".repeat(64), editor.dir.clone()));
    let app = server::router(Xuan { editor, shared }, port);
    let limits = server::Limits {
        connections: 2,
        header_timeout: Duration::from_millis(300),
    };
    tokio::spawn(server::serve(listener, app, limits, std::future::pending()));
    let connect = || tokio::net::TcpStream::connect(("127.0.0.1", port));
    // Two idle connections take the places; a third is closed at once.
    let mut idle = [connect().await.unwrap(), connect().await.unwrap()];
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut third = connect().await.unwrap();
    let mut buffer = Vec::new();
    let closed = tokio::time::timeout(Duration::from_millis(250), third.read_to_end(&mut buffer));
    assert!(
        matches!(closed.await, Ok(Ok(0))),
        "the third is closed before any timeout"
    );
    // An idle connection that sends no headers is closed after the timeout.
    let started = std::time::Instant::now();
    let read = tokio::time::timeout(Duration::from_secs(5), idle[0].read_to_end(&mut buffer));
    assert!(read.await.is_ok(), "closed");
    assert!(started.elapsed() < Duration::from_secs(2));
    // Then there is room again.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let mut stream = connect().await.unwrap();
    stream
        .write_all(
            format!("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    let _ = idle[1].write_all(b"").await;
}

#[test]
fn absolute_paths_in_messages_become_file_names() {
    let cases = [
        (
            "Could not open /home/me/Secret Plans/broken.png",
            "Could not open broken.png",
        ),
        (
            "error at path \"/home/me/My Pictures/.tmpAb1\" (os error 2)",
            "error at path \".tmpAb1\" (os error 2)",
        ),
        (
            "Cannot read /tmp/x/y.png: denied.",
            "Cannot read y.png: denied.",
        ),
        (
            "at \"C:\\\\Users\\\\me\\\\Pictures\\\\a b.png\"",
            "at \"a b.png\"",
        ),
        ("C:\\Users\\me\\a.png is locked", "a.png is locked"),
        ("on \\\\server\\share\\a.png", "on a.png"),
        ("(/srv/data/b.jpg)", "(b.jpg)"),
        ("/home/me/My File.png", "My File.png"),
        // Left alone: no absolute path.
        ("http://127.0.0.1:8765/mcp", "http://127.0.0.1:8765/mcp"),
        ("xuan://document/preview.png", "xuan://document/preview.png"),
        (
            "relative/a.png and 1/2 of `/`",
            "relative/a.png and 1/2 of `/`",
        ),
        ("Unknown argument `colour`", "Unknown argument `colour`"),
    ];
    for (message, redacted) in cases {
        assert_eq!(tools::redact_paths(message), redacted, "{message}");
    }
}

#[tokio::test]
async fn tool_and_resource_errors_name_files_never_folders() {
    let editor = FakeEditor::new(false);
    let (app, _) = app(editor.clone());
    let mut client = Client::new(app);
    client.initialize().await;
    let failed = client
        .tool("open_document", json!({"path": "/home/someone/link.png"}))
        .await;
    assert_eq!(failed.is_error, Some(true));
    let text = text_of(&failed);
    assert!(text.contains("broken.png"), "{text}");
    assert!(!text.contains("someone"), "{text}");
    assert!(!text.contains("Secret"), "{text}");
    // The plugin's own messages too.
    let missing = editor.dir.join("private").join("gone.png");
    let error = tools::read_export(&json!({"path": missing})).unwrap_err();
    assert!(error.contains("gone.png"), "{error}");
    assert!(!error.contains("private"), "{error}");
    let blocked = editor.dir.join("a file");
    std::fs::write(&blocked, b"").unwrap();
    let incoming = blocked.join("incoming");
    let cx = tools::Context {
        editor: editor.as_ref(),
        session: None,
        incoming: &incoming,
        cancel: &CancelToken::new(),
    };
    let args = serde_json::from_value(json!({"png_base64": PNG})).unwrap();
    let result = tools::call(&cx, "create_image_layer", args);
    let text = text_of(&result);
    assert_eq!(result.is_error, Some(true));
    assert!(!text.contains(editor.dir.to_str().unwrap()), "{text}");
}

#[tokio::test]
async fn large_images_fit_and_larger_requests_get_a_clear_answer() {
    let editor = FakeEditor::new(false);
    let (app, _) = app(editor.clone());
    let mut client = Client::new(app);
    client.initialize().await;
    // Over rmcp's default 4 MiB body: an image well inside the 64 MiB limit.
    let mut png = STANDARD.decode(PNG).unwrap();
    png.resize(6 * 1024 * 1024, 0);
    let added = client
        .tool(
            "create_image_layer",
            json!({"png_base64": STANDARD.encode(png)}),
        )
        .await;
    assert_ne!(added.is_error, Some(true), "{}", text_of(&added));
    // A body over the limit is refused before it is read, saying why.
    let body = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"});
    let mut request = client.request(&body);
    request.headers_mut().insert(
        header::CONTENT_LENGTH,
        (server::MAX_BODY_BYTES + 1).to_string().parse().unwrap(),
    );
    let (status, _, text) = client.send(request).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(text, server::TOO_LARGE);
    assert!(text.contains("64 MiB") && text.contains("96 MiB"), "{text}");
    // What Xuan would refuse as a message is refused here, not sent (Xuan
    // would stop the plugin).
    let before = editor.requests().len();
    let cx = tools::Context {
        editor: editor.as_ref(),
        session: None,
        incoming: &editor.dir,
        cancel: &CancelToken::new(),
    };
    let text = "x".repeat(tools::MAX_REQUEST_BYTES + 1);
    let args = serde_json::from_value(json!({"text": text})).unwrap();
    let result = tools::call(&cx, "create_text_layer", args);
    assert_eq!(result.is_error, Some(true));
    assert!(
        text_of(&result).contains("too large"),
        "{}",
        text_of(&result)
    );
    assert_eq!(editor.requests().len(), before, "nothing sent");
}

#[tokio::test]
async fn a_new_token_forgets_the_sessions_issued_before() {
    let editor = FakeEditor::new(false);
    let (app, shared) = app(editor.clone());
    let mut first = Client::new(app.clone());
    first.initialize().await;
    first.tool("fill", json!({"color": "#0000ff"})).await;
    // New Token (the same text here, so this client still gets in): its
    // session id no longer names the session the user answered for.
    shared.replace_token("k".repeat(64));
    first.tool("fill", json!({"color": "#0000ff"})).await;
    let mut second = Client::new(app);
    second.initialize().await;
    second.tool("fill", json!({"color": "#0000ff"})).await;
    let sessions: Vec<String> = editor
        .requests()
        .into_iter()
        .filter(|(_, method, _)| method == "document/edit")
        .map(|(session, ..)| session.unwrap())
        .collect();
    assert_eq!(
        sessions,
        [
            "MCP client 1",
            server::STATELESS_SESSION,
            // Labels are never reused.
            "MCP client 2",
        ]
    );
}

/// The messages of a server-sent event stream, read as they arrive.
struct Events {
    body: Body,
    buffer: String,
    seen: usize,
}

impl Events {
    fn new(body: Body) -> Self {
        Self {
            body,
            buffer: String::new(),
            seen: 0,
        }
    }

    /// The next message that `wanted` accepts; others are skipped.
    async fn next(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            let messages: Vec<Value> = (self.buffer.lines())
                .filter_map(|line| line.strip_prefix("data:"))
                .filter_map(|data| serde_json::from_str(data.trim()).ok())
                .collect();
            for message in messages.into_iter().skip(self.seen) {
                self.seen += 1;
                if wanted(&message) {
                    return message;
                }
            }
            let frame = tokio::time::timeout(Duration::from_secs(10), self.body.frame())
                .await
                .expect("the server sends more")
                .expect("the stream has not ended")
                .unwrap();
            if let Ok(data) = frame.into_data() {
                self.buffer.push_str(&String::from_utf8_lossy(&data));
            }
        }
    }
}

fn is_progress(message: &Value) -> bool {
    message["method"] == "notifications/progress"
}

/// Waits short enough for a test.
fn quick() -> Waits {
    Waits {
        progress_every: Duration::from_millis(50),
        with_progress: Duration::from_secs(30),
        without_progress: Duration::from_secs(30),
    }
}

async fn wait_until(done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out");
}

#[tokio::test]
async fn a_call_waiting_for_the_user_reports_progress_and_is_withdrawn_when_cancelled() {
    let editor = FakeEditor::holding("document/edit");
    let (app, _) = app_with(editor.clone(), quick());
    let mut client = Client::new(app.clone());
    client.initialize().await;
    let call = |id: i64| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
            "name": "create_layer", "arguments": {}, "_meta": {"progressToken": format!("p{id}")},
        }})
    };

    // While the user decides, the client hears that the call is waiting.
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        app.clone().oneshot(client.request(&call(7))),
    )
    .await
    .expect("the answer starts before the user answers")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut events = Events::new(response.into_body());
    let first = events.next(is_progress).await;
    assert_eq!(first["params"]["progressToken"], "p7");
    assert_eq!(
        first["params"]["message"],
        "Waiting for the user to allow edits in Xuan"
    );
    let second = events.next(is_progress).await;
    let progress = |m: &Value| m["params"]["progress"].as_f64().unwrap();
    assert!(progress(&second) > progress(&first));
    // The user answers: the call completes.
    editor.answer.store(true, Ordering::SeqCst);
    let done = events.next(|m| m["id"] == 7).await;
    assert_eq!(done["result"]["isError"], json!(false), "{done}");

    // The client cancels (notifications/cancelled): the request is withdrawn
    // in Xuan, so its prompt closes and a late answer does nothing.
    editor.answer.store(false, Ordering::SeqCst);
    let response = app.clone().oneshot(client.request(&call(8))).await.unwrap();
    let mut events = Events::new(response.into_body());
    events.next(is_progress).await;
    assert!(editor.withdrawn().is_empty());
    let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                        "params": {"requestId": 8, "reason": "The operation timed out"}});
    assert_eq!(
        client.send(client.request(&cancel)).await.0,
        StatusCode::ACCEPTED
    );
    wait_until(|| editor.withdrawn() == ["document/edit"]).await;
}

#[tokio::test]
async fn a_stateless_call_starts_its_answer_with_progress_and_ends_with_the_connection() {
    let editor = FakeEditor::holding("file/open");
    let (app, _) = app_with(editor.clone(), quick());
    let client = Client::new(app.clone());
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "open_document", "arguments": {"path": "/tmp/a.png"},
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "progressToken": 5,
        },
    }});
    let mut request = client.request(&body);
    for (name, value) in [
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", "tools/call"),
        ("mcp-name", "open_document"),
    ] {
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    // rmcp sends no headers on this protocol before the first message, and
    // clients give up on a request without headers (Claude Code after 60 s):
    // progress opens the answer while the user decides.
    let response = tokio::time::timeout(Duration::from_secs(5), app.clone().oneshot(request))
        .await
        .expect("the answer starts before the user answers")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut events = Events::new(response.into_body());
    let progress = events.next(is_progress).await;
    assert_eq!(progress["params"]["progressToken"], 5);
    assert_eq!(
        progress["params"]["message"],
        "Waiting for the user to answer in Xuan"
    );
    // The client closes the request: the open prompt is withdrawn.
    drop(events);
    wait_until(|| editor.withdrawn() == ["file/open"]).await;
}

#[tokio::test]
async fn without_progress_the_server_gives_up_and_says_the_user_has_not_answered() {
    let editor = FakeEditor::holding("document/edit");
    let waits = Waits {
        without_progress: Duration::from_millis(200),
        ..quick()
    };
    let (app, _) = app_with(editor.clone(), waits);
    let mut client = Client::new(app);
    client.initialize().await;
    let result = client.tool("create_layer", json!({})).await;
    assert_eq!(result.is_error, Some(true));
    let text = text_of(&result);
    assert!(
        text.contains("has not yet answered Xuan's prompt to allow edits")
            && text.contains("nothing was changed")
            && text.contains("ask the user"),
        "{text}"
    );
    assert_eq!(editor.withdrawn(), ["document/edit"]);

    // The same for a file dialog.
    let editor = FakeEditor::holding("file/save_as");
    let (app, _) = app_with(editor.clone(), waits);
    let mut client = Client::new(app);
    client.initialize().await;
    let text = text_of(&client.tool("save_document", json!({})).await);
    assert!(text.contains("Xuan's dialog for this request"), "{text}");
    assert_eq!(editor.withdrawn(), ["file/save_as"]);
}

#[tokio::test]
async fn tools_that_wait_for_the_user_say_so() {
    let (app, _) = app(FakeEditor::new(false));
    let mut client = Client::new(app);
    client.initialize().await;
    let list: ListToolsResult =
        serde_json::from_value(client.call("tools/list", json!({})).await["result"].clone())
            .unwrap();
    let description = |name: &str| {
        let tool = list.tools.iter().find(|tool| tool.name == name).unwrap();
        tool.description.clone().unwrap_or_default().into_owned()
    };
    for name in ["create_layer", "fill", "run_command", "undo"] {
        let text = description(name);
        assert!(
            text.contains("waits until the user answers Xuan's prompt")
                && text.contains("instead of retrying"),
            "{name}: {text}"
        );
    }
    for name in ["open_document", "save_document", "export_document"] {
        let text = description(name);
        assert!(
            text.contains("waits until the user answers in Xuan")
                && text.contains("instead of retrying"),
            "{name}: {text}"
        );
    }
    assert!(!description("get_document").contains("waits"));
}

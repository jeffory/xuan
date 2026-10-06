//! The HTTP layer and the MCP protocol, through an in-process client (the
//! router called as a tower service) against a fake editor.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
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

use crate::{
    editor::{CANCELLED, Editor, EditorError},
    server::{self, Shared, Xuan},
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
}

impl FakeEditor {
    fn new(deny: bool) -> Arc<Self> {
        let dir = std::env::temp_dir().join(format!("xuan-mcp-test-{}", crate::auth::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Self {
            dir,
            log: Mutex::default(),
            deny,
        })
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
    ) -> Result<Value, EditorError> {
        self.log.lock().unwrap().push((
            session.map(str::to_owned),
            method.to_owned(),
            params.clone(),
        ));
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
    let shared = Arc::new(Shared::new("k".repeat(64), editor.dir.clone()));
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
        "apply_filter",
        "apply_adjustment",
        "merge_layers",
        "group_layers",
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
async fn refusals_and_bad_arguments_are_tool_errors_the_model_can_read() {
    let editor = FakeEditor::new(true);
    let (app, _) = app(editor.clone());
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
    let missing = client.tool("no_such_tool", json!({})).await;
    assert_eq!(missing.is_error, Some(true));
}

#[test]
fn images_from_the_client_are_written_to_the_plugins_folder_and_removed() {
    let editor = FakeEditor::new(false);
    let incoming = editor.dir.join("incoming");
    let cx = tools::Context {
        editor: editor.as_ref(),
        session: Some("s"),
        incoming: &incoming,
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
async fn the_server_listens_on_loopback_only_and_falls_back_to_another_port() {
    let first = server::bind(0).await.unwrap();
    let address = first.local_addr().unwrap();
    assert!(address.ip().is_loopback(), "{address}");
    assert_eq!(address.ip().to_string(), "127.0.0.1");
    // The port is taken: the next one is used.
    let second = server::bind(address.port()).await.unwrap();
    let port = second.local_addr().unwrap().port();
    assert_ne!(port, address.port());
    assert!(second.local_addr().unwrap().ip().is_loopback());

    // A real connection over TCP gets the same checks.
    let editor = FakeEditor::new(false);
    let shared = Arc::new(Shared::new("k".repeat(64), editor.dir.clone()));
    let app = server::router(Xuan { editor, shared }, port);
    tokio::spawn(async move { axum::serve(second, app).await });
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

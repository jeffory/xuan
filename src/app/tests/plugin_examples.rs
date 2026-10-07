//! The example plugins shipped in `plugins/`, run through their SDKs.
use super::*;
use xuan::plugins::Manifest;

fn example(name: &str) -> Manifest {
    Manifest::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join(name),
    )
    .unwrap()
}

#[test]
fn bundled_plugins_load_with_their_shortcuts() {
    let (_context, mut app) = app();
    let manifests = [
        "comfy-cloud",
        "extend-edges",
        "histogram",
        "invert-regions",
        "local-upscale",
        "mcp-server",
        "select-bright",
    ]
    .map(example);
    app.install_plugins(manifests.to_vec(), vec![]);
    assert!(app.plugins.errors.is_empty(), "{:?}", app.plugins.errors);
    assert!(
        app.keymap
            .entries()
            .iter()
            .any(|entry| entry.id.starts_with("invert-regions/") && !entry.keys.is_empty())
    );
}

/// Tests that run the example plugins' processes.
#[cfg(unix)]
mod unix {
    use super::*;
    use std::time::{Duration, Instant};
    use xuan::plugins::ui::Node;

    fn python() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn run_until(
        context: &egui::Context,
        app: &mut EditorApp,
        mut done: impl FnMut(&EditorApp) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done(app) {
            assert!(
                Instant::now() < deadline,
                "timed out; error: {:?}",
                app.error
            );
            frame(context, app);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn find<'a>(node: &'a Node, wanted: &str) -> Option<&'a Node> {
        match node {
            Node::Column { children, .. } | Node::Row { children, .. } => {
                children.iter().find_map(|child| find(child, wanted))
            }
            Node::Image { .. } if wanted == "image" => Some(node),
            Node::Select { .. } if wanted == "select" => Some(node),
            Node::Label { .. } if wanted == "label" => Some(node),
            _ => None,
        }
    }

    #[test]
    fn histogram_pane_renders_through_the_python_sdk() {
        if !python() {
            eprintln!("python3 not available; skipping");
            return;
        }
        let (context, mut app) = app();
        app.install_plugins(vec![example("histogram")], vec![]);
        app.grant_plugin("histogram", true);
        app.dimensions = [32, 24];
        app.new_document();
        app.command("fill_fg");
        let key = "plugin:histogram/histogram";
        run_until(&context, &mut app, |app| {
            app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        assert!(app.error.is_none(), "{:?}", app.error);
        let tree = app.plugins.panes[key].tree.clone().unwrap();
        let Some(Node::Image { src, .. }) = find(&tree, "image") else {
            panic!("{tree:?}");
        };
        assert!(src.starts_with("data:image/png;base64,"));
        assert!(find(&tree, "select").is_some());
        let decoded =
            xuan::plugins::ui::decode_base64(src.split_once(";base64,").unwrap().1).unwrap();
        let image = image::load_from_memory(&decoded).unwrap();
        assert_eq!((image.width(), image.height()), (256, 96));
        // The pane is drawn with the data URL loaded as a texture.
        frame(&context, &mut app);
        assert!(app.plugins.panes[key].images.contains_key(src.as_str()));

        // Changing a widget re-renders through `pane/render` with an event.
        app.render_pane(
            key,
            "event",
            Some(xuan::plugins::ui::Event {
                widget: "channel".into(),
                value: serde_json::Value::String("r".into()),
            }),
        );
        run_until(&context, &mut app, |app| {
            !app.plugins.panes[key].pending
                && matches!(
                    find(app.plugins.panes[key].tree.as_ref().unwrap(), "select"),
                    Some(Node::Select { value, .. }) if value == "r"
                )
        });
        // Settings reach the plugin while it runs.
        app.set_plugin_setting("histogram", "log_scale", serde_json::Value::Bool(true));
        assert_eq!(
            app.config.plugins["histogram"].settings["log_scale"],
            toml::Value::Boolean(true)
        );
        app.stop_plugin("histogram");
        assert!(app.error.is_none(), "{:?}", app.error);
    }

    /// Issue 73: a Python example copied out of the repository, as a user
    /// installs it, finds the SDK installed with Xuan. Here Xuan's prefix is
    /// a temporary folder with the SDK in `lib/xuan/sdk/python`, and the
    /// plugin's repository-relative path to the SDK leads nowhere.
    #[test]
    fn an_installed_python_plugin_imports_the_sdk_that_ships_with_xuan() {
        if !python() {
            eprintln!("python3 not available; skipping");
            return;
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let prefix = tempfile::tempdir().unwrap();
        let lib = prefix.path().join("lib").join("xuan");
        let sdk = lib.join("sdk").join("python");
        std::fs::create_dir_all(&sdk).unwrap();
        std::fs::create_dir_all(lib.join("plugins")).unwrap();
        std::fs::copy(
            repo.join("sdk/python/xuan_plugin.py"),
            sdk.join("xuan_plugin.py"),
        )
        .unwrap();
        let user = tempfile::tempdir().unwrap();
        let plugin = user.path().join("plugins").join("histogram");
        std::fs::create_dir_all(&plugin).unwrap();
        for name in ["plugin.toml", "main.py"] {
            std::fs::copy(repo.join("plugins/histogram").join(name), plugin.join(name)).unwrap();
        }
        assert!(!user.path().join("sdk/python/xuan_plugin.py").exists());

        let (context, mut app) = app();
        app.plugins.sdk_dir = xuan::plugins::sdk_dir_from(None, Some(&lib.join("plugins")), None);
        assert_eq!(app.plugins.sdk_dir.as_deref(), Some(sdk.as_path()));
        app.install_plugins(vec![Manifest::load(&plugin).unwrap()], vec![]);
        app.grant_plugin("histogram", true);
        app.dimensions = [16, 16];
        app.new_document();
        let key = "plugin:histogram/histogram";
        run_until(&context, &mut app, |app| {
            app.plugins.panes.get(key).is_some_and(|p| p.tree.is_some())
        });
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(app.plugins.running("histogram"));
        app.stop_plugin("histogram");
    }

    #[test]
    fn select_bright_areas_proposes_a_selection_through_the_python_sdk() {
        if !python() {
            eprintln!("python3 not available; skipping");
            return;
        }
        let (context, mut app) = app();
        app.install_plugins(vec![example("select-bright")], vec![]);
        app.grant_plugin("select-bright", true);
        app.dimensions = [40, 30];
        app.new_document();
        // White on the left, black on the right.
        app.brush.color = [255, 255, 255, 255];
        app.command("fill_fg");
        app.edit_selection("Right", |document| {
            document.selection = Some(std::sync::Arc::new(image::GrayImage::from_fn(
                40,
                30,
                |x, _| image::Luma([if x >= 20 { 255 } else { 0 }]),
            )));
        });
        app.brush.color = [0, 0, 0, 255];
        app.command("fill_fg");
        let pixels = app.session().unwrap().document.layers[0].pixels.clone();
        app.command("deselect");
        app.start_plugin_action("select-bright", "select-bright");
        app.run_plugin_action();
        assert!(app.error.is_none(), "{:?}", app.error);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let selection = app.session().unwrap().document.selection.clone().unwrap();
        assert_eq!(selection.get_pixel(5, 15)[0], 255);
        assert_eq!(selection.get_pixel(35, 15)[0], 0);
        let message = app.plugins.proposal.as_ref().unwrap().message.clone();
        assert_eq!(message.as_deref(), Some("Selected about 50% of the image"));
        app.resolve_proposal(true);
        let session = app.session().unwrap();
        assert_eq!(session.history.undo_name(), Some("Select Bright Areas"));
        assert_eq!(session.document.layers.len(), 1);
        assert_eq!(session.document.layers[0].pixels, pixels);

        // As the Select Subject provider it runs at once, without its dialog.
        app.config.providers.set(
            xuan::plugins::manifest::Capability::SelectSubject,
            Some("select-bright".into()),
        );
        app.command("deselect");
        app.command("select_subject");
        assert!(app.job.is_none() && app.plugins.action.is_none());
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        app.resolve_proposal(true);
        let session = app.session().unwrap();
        assert_eq!(session.history.undo_name(), Some("Select Subject"));
        let selection = session.document.selection.clone().unwrap();
        assert_eq!(selection.get_pixel(5, 15)[0], 255);
        assert_eq!(selection.get_pixel(35, 15)[0], 0);
        app.stop_plugin("select-bright");
    }

    #[test]
    fn extend_edges_outpaints_a_larger_canvas_through_the_python_sdk() {
        if !python() {
            eprintln!("python3 not available; skipping");
            return;
        }
        let (context, mut app) = app();
        app.install_plugins(vec![example("extend-edges")], vec![]);
        app.grant_plugin("extend-edges", true);
        app.dimensions = [40, 30];
        app.new_document();
        // White on the left, black on the right.
        app.brush.color = [255, 255, 255, 255];
        app.command("fill_fg");
        app.edit_selection("Right", |document| {
            document.selection = Some(std::sync::Arc::new(image::GrayImage::from_fn(
                40,
                30,
                |x, _| image::Luma([if x >= 20 { 255 } else { 0 }]),
            )));
        });
        app.brush.color = [0, 0, 0, 255];
        app.command("fill_fg");
        app.command("deselect");
        let source = app.session().unwrap().document.layers[0].id;
        let steps = app.session().unwrap().history.names().count();
        let inputs = serde_json::json!({"amount": 5, "fill": "repeat"});
        app.start_plugin_action_with("extend-edges", "outpaint", Some(&inputs));
        app.run_plugin_action();
        assert!(app.error.is_none(), "{:?}", app.error);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let document = &app.session().unwrap().document;
        assert_eq!((document.width, document.height), (50, 40));
        let base = document.layers.iter().find(|l| l.id == source).unwrap();
        assert_eq!((base.transform.x, base.transform.y), (5.0, 5.0));
        let layer = (document.layers.iter())
            .find(|l| l.name == "Outpainted edges")
            .unwrap();
        let t = layer.transform;
        assert_eq!((t.x, t.y, t.width, t.height), (0.0, 0.0, 50.0, 40.0));
        // The edges repeat outwards; the mask shows only the new canvas.
        let pixels = layer.pixels.as_ref().unwrap();
        assert_eq!(pixels.dimensions(), (50, 40));
        assert_eq!(pixels.get_pixel(1, 20).0, [255, 255, 255, 255]);
        assert_eq!(pixels.get_pixel(48, 2).0, [0, 0, 0, 255]);
        let mask = &layer.mask.as_ref().unwrap().pixels;
        assert_eq!(
            (mask.get_pixel(1, 20)[0], mask.get_pixel(25, 20)[0]),
            (255, 0)
        );
        let message = app.plugins.proposal.as_ref().unwrap().message.clone();
        assert_eq!(message.as_deref(), Some("Extended the canvas to 50x40"));
        app.resolve_proposal(true);
        let session = app.session().unwrap();
        assert_eq!(session.history.names().count(), steps + 1);
        assert_eq!(session.history.undo_name(), Some("Outpaint (Fill Edges)"));
        app.command("undo");
        let document = &app.session().unwrap().document;
        assert_eq!((document.width, document.height), (40, 30));
        app.stop_plugin("extend-edges");
    }

    /// The MCP server plugin's binary: `$XUAN_MCP_SERVER`, or a release
    /// build in its folder.
    fn mcp_server_binary() -> Option<std::path::PathBuf> {
        std::env::var_os("XUAN_MCP_SERVER")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                Some(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("plugins/mcp-server/target/release/xuan-mcp-server"),
                )
            })
            .filter(|path| path.is_file())
    }

    /// One HTTP/1.1 POST to the MCP endpoint; returns the status, the
    /// `mcp-session-id` header and the JSON-RPC messages in the body.
    fn mcp_post(
        port: u16,
        token: &str,
        session: Option<&str>,
        body: &serde_json::Value,
    ) -> (u16, Option<String>, Vec<serde_json::Value>) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .unwrap();
        let body = body.to_string();
        let session = session
            .map(|id| format!("mcp-session-id: {id}\r\n"))
            .unwrap_or_default();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             {session}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let response = String::from_utf8_lossy(&response).into_owned();
        let (head, mut rest) = response.split_once("\r\n\r\n").unwrap();
        let status = head[9..12].parse().unwrap();
        let header = |name: &str| {
            head.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_owned())
            })
        };
        let mut body = String::new();
        if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
            while let Some((size, after)) = rest.split_once("\r\n") {
                let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
                if size == 0 {
                    break;
                }
                body.push_str(&after[..size]);
                rest = after[size..].trim_start_matches("\r\n");
            }
        } else {
            body = rest.to_owned();
        }
        let messages = if body.trim_start().starts_with('{') {
            vec![serde_json::from_str(&body).unwrap()]
        } else {
            body.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .filter_map(|data| serde_json::from_str(data.trim()).ok())
                .collect()
        };
        (status, header("mcp-session-id"), messages)
    }

    /// An MCP client session over real HTTP, for the thread that plays the
    /// LLM client.
    struct McpClient {
        port: u16,
        token: String,
        session: Option<String>,
        next: i64,
    }

    impl McpClient {
        fn connect(port: u16, token: &str) -> Self {
            let mut client = Self {
                port,
                token: token.to_owned(),
                session: None,
                next: 1,
            };
            let info = client.call(
                "initialize",
                serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {},
                                   "clientInfo": {"name": "e2e", "version": "1"}}),
            );
            assert_eq!(info["serverInfo"]["name"], "xuan", "{info}");
            let note = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
            let (status, ..) = mcp_post(port, token, client.session.as_deref(), &note);
            assert_eq!(status, 202);
            client
        }

        fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
            let id = self.next;
            self.next += 1;
            let body =
                serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            let (status, session, messages) =
                mcp_post(self.port, &self.token, self.session.as_deref(), &body);
            assert_eq!(status, 200, "{method}: {messages:?}");
            if session.is_some() {
                self.session = session;
            }
            let message = (messages.into_iter())
                .find(|m| m["id"] == id)
                .unwrap_or_else(|| panic!("no answer to {method}"));
            message["result"].clone()
        }

        /// A tool's text, or panics when it failed.
        fn tool(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
            let result = self.call(
                "tools/call",
                serde_json::json!({"name": name, "arguments": arguments}),
            );
            assert_ne!(result["isError"], true, "{name}: {result}");
            result
        }

        fn layer(&mut self, id: &str) -> serde_json::Value {
            let result = self.tool("get_layer", serde_json::json!({"layer": id}));
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
        }
    }

    /// The mcp-server plugin against a real headless editor: a client lists
    /// the layers, sets an opacity (one undo step, after the edit prompt),
    /// reads the preview resource (after the send prompt), and a later
    /// session runs in auto mode. Needs `cargo build --release` in
    /// `plugins/mcp-server` first, or `XUAN_MCP_SERVER` naming the binary;
    /// skipped otherwise.
    #[test]
    fn the_mcp_server_plugin_drives_the_editor_over_http() {
        use crate::app::plugin_sessions::EditAnswer;
        let Some(binary) = mcp_server_binary() else {
            eprintln!("the mcp-server plugin is not built; skipping");
            return;
        };
        let (context, mut app) = app();
        let mut manifest = example("mcp-server");
        manifest.plugin.command = vec![binary.display().to_string()];
        // Let the system choose the port, so tests never collide.
        (app.config.plugins.entry("mcp-server".into()).or_default())
            .settings
            .insert("port".into(), toml::Value::Integer(0));
        app.install_plugins(vec![manifest], vec![]);
        app.grant_plugin("mcp-server", true);
        app.dimensions = [32, 24];
        app.new_document();
        app.command("fill_fg");
        let base = app.session().unwrap().document.layers[0].id.to_string();
        app.render_pane("plugin:mcp-server/status", "open", None);
        let data = app.plugins.data_dir("mcp-server").unwrap();
        run_until(&context, &mut app, |_| {
            data.join("connection.json").is_file()
        });
        let connection: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(data.join("connection.json")).unwrap())
                .unwrap();
        let port = connection["port"].as_u64().unwrap() as u16;
        let token = std::fs::read_to_string(data.join("token")).unwrap();
        let steps = app.session().unwrap().history.names().count();

        let client = std::thread::spawn({
            let (base, token) = (base.clone(), token.trim().to_owned());
            move || {
                let mut first = McpClient::connect(port, &token);
                let layers = first.tool("get_document", serde_json::json!({}));
                assert!(layers.to_string().contains(&base), "{layers}");
                // The first edit waits for the prompt.
                first.tool(
                    "set_layer",
                    serde_json::json!({"layer": base, "opacity": 0.5}),
                );
                let opacity = first.layer(&base)["opacity"].as_f64().unwrap();
                // The preview waits for the send prompt.
                let preview = first.call(
                    "resources/read",
                    serde_json::json!({"uri": "xuan://document/preview.png"}),
                );
                let blob = preview["contents"][0]["blob"].as_str().unwrap().to_owned();
                // One undo step brings the opacity back.
                first.tool("undo", serde_json::json!({}));
                let undone = first.layer(&base)["opacity"].as_f64().unwrap();
                // A new session asks again: Always Allow turns on auto mode,
                // and then a third session edits without asking.
                let mut second = McpClient::connect(port, &token);
                second.tool(
                    "set_layer",
                    serde_json::json!({"layer": base, "name": "Renamed"}),
                );
                let mut third = McpClient::connect(port, &token);
                third.tool(
                    "set_layer",
                    serde_json::json!({"layer": base, "visible": false}),
                );
                let status = third.tool("get_edit_permission", serde_json::json!({}));
                (opacity, blob, undone, status.to_string())
            }
        });
        let mut prompts = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while !client.is_finished() {
            assert!(std::time::Instant::now() < deadline, "timed out");
            frame(&context, &mut app);
            match app.dialog {
                Some(Dialog::PluginEditSession) => {
                    let prompt = app.plugins.edit_prompt.clone().unwrap();
                    prompts.push(prompt.clone());
                    app.answer_edit_session(if prompts.len() == 1 {
                        EditAnswer::Allow
                    } else {
                        EditAnswer::Always
                    });
                }
                Some(Dialog::PluginConsent) => app.answer_consent(true),
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let (opacity, blob, undone, status) = client.join().expect("the client's checks pass");
        assert_eq!(opacity, 0.5);
        assert_eq!(undone, 1.0);
        let png = xuan::plugins::ui::decode_base64(&blob).unwrap();
        use image::GenericImageView;
        assert_eq!(
            image::load_from_memory(&png).unwrap().dimensions(),
            (32, 24)
        );
        // Two sessions asked, each naming its first edit; the third did not.
        assert_eq!(prompts.len(), 2, "{prompts:?}");
        assert_eq!(prompts[0].edit, "Set Layer");
        assert!(!prompts[0].session.is_empty());
        assert_ne!(prompts[0].session, prompts[1].session);
        assert!(app.edits_without_asking("mcp-server"));
        assert!(status.contains("\\\"auto\\\": true"), "{status}");
        let session = app.session().unwrap();
        let layer = &session.document.layers[0];
        assert_eq!((layer.name.as_str(), layer.visible), ("Renamed", false));
        // set, undone; rename; hide.
        assert_eq!(session.history.names().count(), steps + 2);
        app.stop_plugin("mcp-server");
        assert!(app.error.is_none(), "{:?}", app.error);
    }

    /// Needs `cargo build --release` in `plugins/invert-regions` first.
    #[test]
    #[ignore]
    fn invert_regions_action_runs_through_the_rust_sdk() {
        let (context, mut app) = app();
        app.install_plugins(vec![example("invert-regions")], vec![]);
        app.grant_plugin("invert-regions", true);
        app.dimensions = [40, 30];
        app.new_document();
        app.brush.color = [200, 100, 50, 255];
        app.command("fill_fg");
        app.start_plugin_action("invert-regions", "invert");
        app.add_region(Point::new(10.0, 10.0), Point::new(30.0, 20.0));
        app.run_plugin_action();
        assert!(app.error.is_none(), "{:?}", app.error);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let document = &app.session().unwrap().document;
        let layer = document
            .layers
            .iter()
            .find(|l| l.name == "Inverted")
            .unwrap();
        let pixels = layer.pixels.as_ref().unwrap();
        let inside = pixels.get_pixel(pixels.width() / 2, pixels.height() / 2);
        assert_eq!(inside.0[..3], [55, 155, 205]);
        assert!(layer.mask.is_some());
        assert_eq!(
            app.status,
            "Invert Regions (plugin invert-regions): Inverted 1 region(s)"
        );
        app.resolve_proposal(true);
    }
}

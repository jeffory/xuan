//! The plugin's pane: where the server listens, the token and ready-made
//! client settings with Copy buttons, how edits are allowed, and recent
//! activity.
use serde_json::{Value, json};
use xuan_plugin::ui;

use crate::server::{Shared, Status};

/// The Claude Code command that adds this server.
pub fn claude_command(url: &str, token: &str) -> String {
    format!("claude mcp add --transport http xuan {url} --header \"Authorization: Bearer {token}\"")
}

/// The `mcpServers` entry most other clients read.
pub fn client_config(url: &str, token: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "mcpServers": {"xuan": {
            "type": "http",
            "url": url,
            "headers": {"Authorization": format!("Bearer {token}")},
        }}
    }))
    .unwrap_or_default()
}

/// The pane, given `session/status` when Xuan answered it.
pub fn tree(shared: &Shared, permissions: Option<&Value>) -> Value {
    let mut children = vec![ui::heading("MCP server")];
    let token = shared.token();
    match shared.status() {
        Status::Starting => children.push(ui::progress(None, "Starting…")),
        Status::Failed(error) => {
            children.push(ui::label(&format!("Not running: {error}")));
            children.push(ui::muted(
                "Choose another port in Plugins → Manage Plugins… → MCP Server.",
            ));
        }
        Status::Listening { port } => {
            let url = Shared::url(port);
            children.push(ui::label(&format!("Listening on {url}")));
            children.push(ui::muted(
                "Only programs on this computer that have the token can connect.",
            ));
            children.push(ui::row(vec![
                ui::copy_button("copy_url", "Copy URL", &url),
                ui::copy_button("copy_token", "Copy Token", &token),
            ]));
            children.push(ui::separator());
            children.push(ui::label("Claude Code: run this in a terminal"));
            children.push(ui::copy_button(
                "copy_claude",
                "Copy Command",
                &claude_command(&url, &token),
            ));
            children.push(ui::label("Other clients: an mcpServers entry"));
            children.push(ui::copy_button(
                "copy_config",
                "Copy JSON",
                &client_config(&url, &token),
            ));
        }
    }
    children.push(ui::separator());
    let edits = match permissions {
        Some(status) if status["auto"] == true => {
            "Auto mode: clients edit without asking. Turn it off in Plugins → Manage Plugins…."
        }
        Some(status) if status["edit_prompt"] == "session" => {
            "Xuan asks before the first edit of each client session."
        }
        _ => "Edits are applied as clients send them.",
    };
    children.push(ui::muted(edits));
    children.push(ui::button("new_token", "New Token"));
    children.push(ui::muted(
        "A new token disconnects every client that uses the old one.",
    ));
    let activity: Vec<String> = shared
        .activity
        .lock()
        .map(|a| a.iter().rev().cloned().collect())
        .unwrap_or_default();
    if !activity.is_empty() {
        children.push(ui::separator());
        children.push(ui::label("Recent tool calls"));
        for line in activity {
            children.push(json!({"type": "label", "text": line, "small": true, "muted": true}));
        }
    }
    ui::column(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_copy(tree: &Value, id: &str) -> Option<String> {
        if tree["id"] == id {
            return tree["copy"].as_str().map(str::to_owned);
        }
        tree["children"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|child| find_copy(child, id))
    }

    #[test]
    fn the_pane_offers_the_connection_details_to_copy() {
        let shared = Shared::new("t".repeat(64), std::env::temp_dir());
        let starting = tree(&shared, None);
        assert!(find_copy(&starting, "copy_url").is_none());
        *shared.status.lock().unwrap() = Status::Listening { port: 8766 };
        let pane = tree(
            &shared,
            Some(&json!({"edit_prompt": "session", "auto": false})),
        );
        assert_eq!(
            find_copy(&pane, "copy_url").as_deref(),
            Some("http://127.0.0.1:8766/mcp")
        );
        assert_eq!(find_copy(&pane, "copy_token").unwrap(), "t".repeat(64));
        let command = find_copy(&pane, "copy_claude").unwrap();
        assert_eq!(
            command,
            format!(
                "claude mcp add --transport http xuan http://127.0.0.1:8766/mcp --header \"Authorization: Bearer {}\"",
                "t".repeat(64)
            )
        );
        let config: Value =
            serde_json::from_str(&find_copy(&pane, "copy_config").unwrap()).unwrap();
        assert_eq!(
            config["mcpServers"]["xuan"]["url"],
            "http://127.0.0.1:8766/mcp"
        );
        let text = pane.to_string();
        assert!(text.contains("asks before the first edit"), "{text}");
        let auto = tree(
            &shared,
            Some(&json!({"edit_prompt": "session", "auto": true})),
        );
        assert!(auto.to_string().contains("Auto mode"));
    }
}

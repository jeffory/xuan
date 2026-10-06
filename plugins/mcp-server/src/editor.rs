//! The editor as the tools see it: plugin protocol requests to Xuan. The
//! real one goes through the SDK's [`Host`]; tests use a fake.
use std::time::Duration;

use serde_json::Value;
use xuan_plugin::Host;

/// Xuan's answer to a refused export or edit: the user said no.
pub const CANCELLED: i64 = -32800;

/// An error Xuan answered a request with.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Sends plugin protocol requests to the editor.
pub trait Editor: Send + Sync + 'static {
    /// One request, in the client's edit session when it has one.
    fn request(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, EditorError>;
}

/// How long a request may wait: edits and file requests wait for the user.
const TIMEOUT: Duration = Duration::from_secs(600);

/// The running editor, through the plugin SDK.
pub struct HostEditor(pub Host);

impl Editor for HostEditor {
    fn request(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, EditorError> {
        let host = self.0.with_timeout(TIMEOUT);
        let host = match session {
            Some(session) => host.with_session(session),
            None => host,
        };
        host.request(method, params).map_err(|error| EditorError {
            code: error.code,
            message: error.message,
        })
    }
}

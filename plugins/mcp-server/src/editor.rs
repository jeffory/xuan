//! The editor as the tools see it: plugin protocol requests to Xuan. The
//! real one goes through the SDK's [`Host`]; tests use a fake.
use std::time::Duration;

use serde_json::Value;
use xuan_plugin::{CancelToken, Host};

/// Xuan's answer to a refused export or edit: the user said no.
pub const CANCELLED: i64 = -32800;
/// The SDK gave up waiting for Xuan and withdrew the request.
pub const TIMED_OUT: i64 = xuan_plugin::codes::TIMED_OUT;
/// The request was withdrawn with its [`CancelToken`]: the client cancelled
/// or went away, or the server stopped waiting for the user.
pub const WITHDRAWN: i64 = xuan_plugin::codes::WITHDRAWN;

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
    /// One request, in the client's edit session when it has one. `cancel`
    /// withdraws it in Xuan, closing the prompt it waits on, and makes it
    /// fail with [`WITHDRAWN`].
    fn request(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        cancel: &CancelToken,
    ) -> Result<Value, EditorError>;
}

/// The longest a request may wait in the SDK, behind the server's own
/// limits ([`crate::server::Waits`]), which withdraw it sooner.
const TIMEOUT: Duration = Duration::from_secs(660);

/// The running editor, through the plugin SDK.
pub struct HostEditor(pub Host);

impl Editor for HostEditor {
    fn request(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        cancel: &CancelToken,
    ) -> Result<Value, EditorError> {
        let host = self.0.with_timeout(TIMEOUT).with_cancel(cancel);
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

//! JSON-RPC 2.0 messages, one per line.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// The job was cancelled by the user.
pub const CANCELLED: i64 = -32800;
/// The plugin needs configuring; the host offers to open its settings.
pub const NEEDS_SETUP: i64 = -32001;
pub const INSUFFICIENT_CREDITS: i64 = -32002;
pub const RATE_LIMITED: i64 = -32003;
/// Longest line either side accepts.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Id,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: Value::Null,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("Unknown method `{method}`"))
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

impl Message {
    pub fn request(id: Id, method: &str, params: Value) -> Self {
        Self::Request(Request {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        })
    }

    pub fn notification(method: &str, params: Value) -> Self {
        Self::Notification(Notification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        })
    }

    pub fn response(id: Id, result: Result<Value, RpcError>) -> Self {
        let (result, error) = match result {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        };
        Self::Response(Response {
            jsonrpc: "2.0".into(),
            id,
            result,
            error,
        })
    }

    pub fn parse(line: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            jsonrpc: Option<String>,
            #[serde(default)]
            id: Option<Id>,
            #[serde(default)]
            method: Option<String>,
            #[serde(default)]
            params: Value,
            #[serde(default, deserialize_with = "present")]
            result: Option<Value>,
            #[serde(default)]
            error: Option<RpcError>,
        }
        // A `"result": null` is a successful answer, unlike a missing key.
        fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
            Value::deserialize(d).map(Some)
        }
        let raw: Raw = serde_json::from_str(line)?;
        if raw.jsonrpc.as_deref() != Some("2.0") {
            bail!("missing \"jsonrpc\": \"2.0\"");
        }
        Ok(match (raw.id, raw.method) {
            (Some(id), Some(method)) => Self::Request(Request {
                jsonrpc: "2.0".into(),
                id,
                method,
                params: raw.params,
            }),
            (None, Some(method)) => Self::Notification(Notification {
                jsonrpc: "2.0".into(),
                method,
                params: raw.params,
            }),
            (Some(id), None) if raw.result.is_some() != raw.error.is_some() => {
                Self::Response(Response {
                    jsonrpc: "2.0".into(),
                    id,
                    result: raw.result,
                    error: raw.error,
                })
            }
            _ => bail!("message is neither a request, a notification nor a response"),
        })
    }

    /// One line of JSON without a trailing newline.
    pub fn to_line(&self) -> String {
        match self {
            Self::Request(m) => serde_json::to_string(m),
            Self::Notification(m) => serde_json::to_string(m),
            Self::Response(m) => serde_json::to_string(m),
        }
        .expect("messages serialize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_roundtrip_and_malformed_lines_are_rejected() {
        let request = Message::request(Id::Number(1), "initialize", json!({"protocol": 1}));
        let line = request.to_line();
        assert_eq!(
            line,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":1}}"#
        );
        assert_eq!(Message::parse(&line).unwrap(), request);
        let notification = Message::notification("job/progress", json!({"fraction": 0.5}));
        assert_eq!(
            Message::parse(&notification.to_line()).unwrap(),
            notification
        );
        let ok = Message::response(Id::Text("a".into()), Ok(json!(null)));
        assert_eq!(ok.to_line(), r#"{"jsonrpc":"2.0","id":"a","result":null}"#);
        assert_eq!(Message::parse(&ok.to_line()).unwrap(), ok);
        let failed = Message::response(Id::Number(2), Err(RpcError::new(CANCELLED, "stopped")));
        assert_eq!(Message::parse(&failed.to_line()).unwrap(), failed);
        assert!(Message::parse(r#"{"id":1,"method":"x"}"#).is_err());
        assert!(Message::parse(r#"{"jsonrpc":"2.0","id":1}"#).is_err());
        assert!(
            Message::parse(
                r#"{"jsonrpc":"2.0","id":1,"result":1,"error":{"code":1,"message":""}}"#
            )
            .is_err()
        );
        assert!(Message::parse("not json").is_err());
        assert_eq!(
            Message::parse(r#"{"jsonrpc":"2.0","method":"ping"}"#).unwrap(),
            Message::notification("ping", Value::Null)
        );
    }
}

//! JSON-RPC 2.0 framing for MCP, hand-rolled to match the pattern the
//! repository already uses for its own server (ADR-012). No client crate is
//! involved, so the wire shape is visible here and nowhere else.

use gritt_core::{Error, Result};
use serde_json::{json, Value};

/// MCP method names Gritt sends or recognizes.
pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "notifications/initialized";
    pub const TOOLS_LIST: &str = "tools/list";
    pub const TOOLS_CALL: &str = "tools/call";
    pub const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";
    pub const CANCELLED: &str = "notifications/cancelled";
    pub const PING: &str = "ping";
}

/// `-32602`, the code the specification cites for an unsupported protocol
/// version and for an unknown tool.
pub const INVALID_PARAMS: i64 = -32602;
/// `-32601`, the answer Gritt gives to a server request it does not
/// implement.
pub const METHOD_NOT_FOUND: i64 = -32601;

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    /// A one-line, safe rendering. The server's message is kept because it
    /// explains the failure; no local value is added to it.
    pub fn summary(&self) -> String {
        format!("server error {}: {}", self.code, self.message)
    }
}

/// One decoded message from a server.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: u64,
        result: std::result::Result<Value, RpcError>,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A server-initiated request. Gritt answers `ping` and refuses the rest.
    Request {
        id: Value,
        method: String,
    },
    /// Well-formed JSON that is not a message Gritt can route, such as a
    /// response whose id it never issued. Recorded, never fatal.
    Unroutable,
}

/// The request frame for `id`.
pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A notification frame. Notifications carry no id and get no response.
pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// The `notifications/cancelled` params for an in-flight request.
pub fn cancellation_params(id: u64, reason: &str) -> Value {
    json!({"requestId": id, "reason": reason})
}

/// A response to a server-initiated request.
pub fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// An error response to a server-initiated request.
pub fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Classifies one decoded JSON value. Unknown shapes are `Unroutable`
/// rather than errors: a malformed line must not take the connection down.
pub fn classify(value: &Value) -> Incoming {
    let Some(object) = value.as_object() else {
        return Incoming::Unroutable;
    };
    let id = object.get("id");
    let method = object.get("method").and_then(Value::as_str);
    match (id, method) {
        (Some(id), None) => {
            // Only ids Gritt issued are routable, and it issues integers.
            let Some(id) = id.as_u64() else {
                return Incoming::Unroutable;
            };
            if let Some(error) = object.get("error") {
                return Incoming::Response {
                    id,
                    result: Err(parse_error(error)),
                };
            }
            Incoming::Response {
                id,
                result: Ok(object.get("result").cloned().unwrap_or(Value::Null)),
            }
        }
        (Some(id), Some(method)) => Incoming::Request {
            id: id.clone(),
            method: method.to_owned(),
        },
        (None, Some(method)) => Incoming::Notification {
            method: method.to_owned(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        },
        (None, None) => Incoming::Unroutable,
    }
}

fn parse_error(value: &Value) -> RpcError {
    RpcError {
        code: value.get("code").and_then(Value::as_i64).unwrap_or(0),
        message: value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_owned(),
        data: value.get("data").cloned(),
    }
}

/// Serializes one message as a single line. The stdio transport forbids
/// embedded newlines, and compact JSON never produces one outside a string,
/// where `serde_json` escapes it.
pub fn encode_line(value: &Value) -> Result<String> {
    let mut text = serde_json::to_string(value)
        .map_err(|error| Error::config(format!("cannot encode an MCP message: {error}")))?;
    debug_assert!(!text.contains('\n'));
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_notifications_and_server_requests_are_told_apart() {
        let response = serde_json::json!({"jsonrpc": "2.0", "id": 3, "result": {"ok": true}});
        assert_eq!(
            classify(&response),
            Incoming::Response {
                id: 3,
                result: Ok(serde_json::json!({"ok": true}))
            }
        );
        let failure = serde_json::json!({"jsonrpc": "2.0", "id": 4,
            "error": {"code": -32602, "message": "Unsupported protocol version"}});
        let Incoming::Response {
            result: Err(error), ..
        } = classify(&failure)
        else {
            panic!("expected an error response");
        };
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.summary().contains("Unsupported protocol version"));
        assert_eq!(
            classify(&serde_json::json!({"jsonrpc": "2.0",
                "method": "notifications/tools/list_changed"})),
            Incoming::Notification {
                method: method::TOOLS_LIST_CHANGED.into(),
                params: Value::Null
            }
        );
        assert!(matches!(
            classify(&serde_json::json!({"jsonrpc": "2.0", "id": "s1", "method": "ping"})),
            Incoming::Request { .. }
        ));
        assert_eq!(classify(&serde_json::json!([1, 2])), Incoming::Unroutable);
        // A string id is never one Gritt issued.
        assert_eq!(
            classify(&serde_json::json!({"jsonrpc": "2.0", "id": "x", "result": {}})),
            Incoming::Unroutable
        );
    }

    #[test]
    fn encoded_frames_are_one_line_each() {
        let line = encode_line(&request(
            1,
            method::TOOLS_CALL,
            serde_json::json!({"name": "t", "arguments": {"text": "a\nb"}}),
        ))
        .unwrap();
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.ends_with('\n'));
        assert!(line.contains("a\\nb"));
    }
}

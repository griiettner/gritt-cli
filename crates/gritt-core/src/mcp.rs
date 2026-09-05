//! Model Context Protocol contracts: the `.mcp.json` configuration model,
//! server lifecycle state, the trust record, and the runtime deadlines.
//!
//! Nothing here performs I/O. Parsing takes the file text and a snapshot of
//! the launch environment and returns typed entries; reading and launching
//! live in the harness (ADR-006). Reading a workspace file never authorizes
//! executing its commands, so a parsed entry carries no permission of its
//! own: the runtime pairs it with a [`TrustRecord`] first.
//!
//! Gritt talks the versioned MCP protocol. It offers
//! [`LATEST_PROTOCOL_VERSION`] and accepts any revision in
//! [`SUPPORTED_PROTOCOL_VERSIONS`]; a server that answers with anything else
//! is disconnected, as the lifecycle specification requires.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::secret::is_secret_env_name;
use crate::{Error, Result};

/// The revision Gritt sends in `initialize`.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";

/// Revisions Gritt can operate against, newest first. The tool surface
/// (`tools/list` pagination, `tools/call`, `notifications/tools/list_changed`)
/// is unchanged across these three, so an older server that answers with one
/// of them is usable. Anything else ends the connection.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Policy rules match MCP tools with this pattern. Dispatch names are
/// `mcp__<server>__<tool>`, so one rule covers every server and a narrower
/// rule such as `mcp__docs__*` can override it.
pub const DISPATCH_TOOL_PATTERN: &str = "mcp__*";

/// True when Gritt can operate against `version`.
pub fn protocol_version_supported(version: &str) -> bool {
    SUPPORTED_PROTOCOL_VERSIONS.contains(&version)
}

/// Which transport an entry declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    Http,
}

impl std::fmt::Display for McpTransportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            McpTransportKind::Stdio => "stdio",
            McpTransportKind::Http => "http",
        })
    }
}

/// A resolved transport. Environment and header values are already
/// interpolated, so this type may hold a credential: it does not implement
/// `Serialize`, and `Debug` prints names without values.
#[derive(Clone, PartialEq, Eq)]
pub enum McpTransport {
    /// Launched as a child process. `command` is resolved against the
    /// workspace when it is relative and through `PATH` when it is a bare
    /// name; `args` are passed verbatim, never through a shell.
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    /// Streamable HTTP against one MCP endpoint.
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl McpTransport {
    pub fn kind(&self) -> McpTransportKind {
        match self {
            McpTransport::Stdio { .. } => McpTransportKind::Stdio,
            McpTransport::Http { .. } => McpTransportKind::Http,
        }
    }
}

impl std::fmt::Debug for McpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpTransport::Stdio { command, args, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", &env.keys().collect::<Vec<_>>())
                .finish(),
            McpTransport::Http { url, headers } => f
                .debug_struct("Http")
                .field("url", url)
                .field("headers", &headers.keys().collect::<Vec<_>>())
                .finish(),
        }
    }
}

/// One usable server definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    /// Stable digest of the raw definition, before interpolation. Trust is
    /// keyed on it, so editing the entry invalidates the approval and no
    /// secret value can reach the record.
    pub fingerprint: String,
}

/// Why an entry cannot run. Every variant names fields and variables only;
/// no value is ever echoed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum McpConfigError {
    /// A transport Gritt does not implement, including legacy standalone SSE.
    UnsupportedTransport {
        transport: String,
    },
    MissingField {
        field: String,
    },
    InvalidField {
        field: String,
        detail: String,
    },
    /// A `${VAR}` reference the launch environment does not define, with no
    /// `:-` default.
    MissingVariable {
        variable: String,
        field: String,
    },
    /// A credential-looking field holding a literal instead of a reference.
    EmbeddedCredential {
        field: String,
    },
}

impl McpConfigError {
    /// A one-line message safe to log, show, or store.
    pub fn message(&self) -> String {
        match self {
            McpConfigError::UnsupportedTransport { transport } => {
                format!("transport `{transport}` is not supported")
            }
            McpConfigError::MissingField { field } => format!("missing `{field}`"),
            McpConfigError::InvalidField { field, detail } => format!("`{field}` {detail}"),
            McpConfigError::MissingVariable { variable, field } => {
                format!("`{field}` needs `{variable}`, which is not set")
            }
            McpConfigError::EmbeddedCredential { field } => format!(
                "`{field}` looks like a credential; reference an environment variable such as ${{VAR}} instead of a literal value"
            ),
        }
    }

    /// True when the entry named a transport Gritt does not implement, which
    /// gets its own lifecycle state.
    pub fn is_unsupported_transport(&self) -> bool {
        matches!(self, McpConfigError::UnsupportedTransport { .. })
    }
}

/// What one `mcpServers` key resolved to. Every configured key produces one
/// entry, so nothing is ever silently omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpEntry {
    Server(McpServerConfig),
    Rejected {
        name: String,
        fingerprint: String,
        error: McpConfigError,
    },
}

impl McpEntry {
    pub fn name(&self) -> &str {
        match self {
            McpEntry::Server(config) => &config.name,
            McpEntry::Rejected { name, .. } => name,
        }
    }

    pub fn fingerprint(&self) -> &str {
        match self {
            McpEntry::Server(config) => &config.fingerprint,
            McpEntry::Rejected { fingerprint, .. } => fingerprint,
        }
    }
}

/// Every entry of one `.mcp.json`, in name order. A missing file is an empty
/// set, not an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfig {
    pub entries: Vec<McpEntry>,
}

impl McpConfig {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, name: &str) -> Option<&McpEntry> {
        self.entries.iter().find(|entry| entry.name() == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.name().to_owned())
            .collect()
    }
}

/// Header names that always carry a credential regardless of the shared
/// environment-variable name rule.
const CREDENTIAL_HEADERS: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "x-auth-token",
];

fn header_is_credential(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    CREDENTIAL_HEADERS.contains(&lower.as_str()) || is_secret_env_name(name, &[])
}

/// Parses `.mcp.json` text. `env` is a snapshot of the launch environment
/// used for `${VAR}` and `${VAR:-default}` references; no shell is executed.
///
/// Invalid JSON, or a `mcpServers` value that is not an object, is a visible
/// configuration error. A bad individual entry is isolated as
/// [`McpEntry::Rejected`] so healthy servers stay usable. `hint` names the
/// file in the error and is never the file content.
pub fn parse_mcp_config(
    text: &str,
    env: &BTreeMap<String, String>,
    hint: &str,
) -> Result<McpConfig> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| {
        Error::config(format!(
            "invalid JSON in {hint} at line {}, column {}: {}",
            error.line(),
            error.column(),
            json_error_kind(&error)
        ))
    })?;
    parse_mcp_value(&value, env, hint)
}

/// The parser without the JSON decoding step, for callers that already hold
/// the document.
pub fn parse_mcp_value(
    value: &serde_json::Value,
    env: &BTreeMap<String, String>,
    hint: &str,
) -> Result<McpConfig> {
    let root = value
        .as_object()
        .ok_or_else(|| Error::config(format!("{hint} must be a JSON object")))?;
    let servers = match root.get("mcpServers") {
        None | Some(serde_json::Value::Null) => return Ok(McpConfig::default()),
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => {
            return Err(Error::config(format!(
                "{hint} field `mcpServers` must be an object"
            )))
        }
    };
    let mut entries = Vec::with_capacity(servers.len());
    for (name, definition) in servers {
        let fingerprint = fingerprint(name, definition);
        match parse_entry(name, definition, env) {
            Ok(transport) => entries.push(McpEntry::Server(McpServerConfig {
                name: name.clone(),
                transport,
                fingerprint,
            })),
            Err(error) => entries.push(McpEntry::Rejected {
                name: name.clone(),
                fingerprint,
                error,
            }),
        }
    }
    entries.sort_by(|a, b| a.name().cmp(b.name()));
    Ok(McpConfig { entries })
}

/// `serde_json` messages can quote the offending input. Only the failure
/// class is reported, because a malformed file may hold a credential.
fn json_error_kind(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Io => "read failed",
        serde_json::error::Category::Syntax => "syntax error",
        serde_json::error::Category::Data => "unexpected value",
        serde_json::error::Category::Eof => "unexpected end of input",
    }
}

fn parse_entry(
    name: &str,
    definition: &serde_json::Value,
    env: &BTreeMap<String, String>,
) -> std::result::Result<McpTransport, McpConfigError> {
    let object = definition
        .as_object()
        .ok_or_else(|| McpConfigError::InvalidField {
            field: name.to_owned(),
            detail: "must be an object".into(),
        })?;
    let declared = match object.get("type").or_else(|| object.get("transport")) {
        Some(serde_json::Value::String(kind)) => Some(kind.to_ascii_lowercase()),
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err(McpConfigError::InvalidField {
                field: "type".into(),
                detail: "must be a string".into(),
            })
        }
    };
    let kind = match declared.as_deref() {
        Some("stdio") => McpTransportKind::Stdio,
        // `http` is the plan's name for Streamable HTTP; the protocol's own
        // name is accepted too.
        Some("http") | Some("streamable-http") | Some("streamable_http") => McpTransportKind::Http,
        Some(other) => {
            return Err(McpConfigError::UnsupportedTransport {
                transport: other.to_owned(),
            })
        }
        // No explicit type: infer from the fields, the shape every existing
        // `.mcp.json` uses.
        None if object.contains_key("url") => McpTransportKind::Http,
        None if object.contains_key("command") => McpTransportKind::Stdio,
        None => {
            return Err(McpConfigError::MissingField {
                field: "command".into(),
            })
        }
    };
    match kind {
        McpTransportKind::Stdio => parse_stdio(object, env),
        McpTransportKind::Http => parse_http(object, env),
    }
}

type Object = serde_json::Map<String, serde_json::Value>;

fn parse_stdio(
    object: &Object,
    env: &BTreeMap<String, String>,
) -> std::result::Result<McpTransport, McpConfigError> {
    let command = require_string(object, "command")?;
    if command.trim().is_empty() {
        return Err(McpConfigError::InvalidField {
            field: "command".into(),
            detail: "must not be empty".into(),
        });
    }
    let args = match object.get("args") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => {
            let mut args = Vec::with_capacity(items.len());
            for item in items {
                // Arguments are preserved verbatim. No interpolation and no
                // shell, so an argument cannot smuggle in an expansion.
                let value = item.as_str().ok_or_else(|| McpConfigError::InvalidField {
                    field: "args".into(),
                    detail: "must contain only strings".into(),
                })?;
                args.push(value.to_owned());
            }
            args
        }
        Some(_) => {
            return Err(McpConfigError::InvalidField {
                field: "args".into(),
                detail: "must be an array".into(),
            })
        }
    };
    let env = resolve_map(object, "env", env, |name| is_secret_env_name(name, &[]))?;
    Ok(McpTransport::Stdio { command, args, env })
}

fn parse_http(
    object: &Object,
    env: &BTreeMap<String, String>,
) -> std::result::Result<McpTransport, McpConfigError> {
    let url = require_string(object, "url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(McpConfigError::InvalidField {
            field: "url".into(),
            detail: "must be an http or https URL".into(),
        });
    }
    let headers = resolve_map(object, "headers", env, header_is_credential)?;
    Ok(McpTransport::Http { url, headers })
}

fn require_string(object: &Object, field: &str) -> std::result::Result<String, McpConfigError> {
    match object.get(field) {
        Some(serde_json::Value::String(value)) => Ok(value.clone()),
        None | Some(serde_json::Value::Null) => Err(McpConfigError::MissingField {
            field: field.to_owned(),
        }),
        Some(_) => Err(McpConfigError::InvalidField {
            field: field.to_owned(),
            detail: "must be a string".into(),
        }),
    }
}

/// Reads an object of string values, refusing a literal in a
/// credential-looking key and interpolating the rest.
fn resolve_map(
    object: &Object,
    field: &str,
    env: &BTreeMap<String, String>,
    is_credential: impl Fn(&str) -> bool,
) -> std::result::Result<BTreeMap<String, String>, McpConfigError> {
    let map = match object.get(field) {
        None | Some(serde_json::Value::Null) => return Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => {
            return Err(McpConfigError::InvalidField {
                field: field.to_owned(),
                detail: "must be an object".into(),
            })
        }
    };
    let mut resolved = BTreeMap::new();
    for (key, value) in map {
        let raw = value.as_str().ok_or_else(|| McpConfigError::InvalidField {
            field: format!("{field}.{key}"),
            detail: "must be a string".into(),
        })?;
        let label = format!("{field}.{key}");
        if is_credential(key) && !is_pure_reference(raw) {
            // ADR-008: a credential is named, never written down. The value
            // is not echoed.
            return Err(McpConfigError::EmbeddedCredential { field: label });
        }
        resolved.insert(key.clone(), interpolate(raw, env, &label)?);
    }
    Ok(resolved)
}

/// True when the whole value is one `${VAR}` reference with no default and
/// no surrounding literal text, the only form a credential field may take.
fn is_pure_reference(value: &str) -> bool {
    let Some(inner) = value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) else {
        return false;
    };
    !inner.is_empty()
        && !inner.contains(":-")
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Expands `${VAR}` and `${VAR:-default}` from `env`. A `$` that does not
/// open a reference stays literal, and no shell is involved: the default is
/// taken as plain text, so `${VAR:-$(id)}` yields the characters `$(id)`.
pub fn interpolate(
    value: &str,
    env: &BTreeMap<String, String>,
    field: &str,
) -> std::result::Result<String, McpConfigError> {
    let bytes: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != '$' || index + 1 >= bytes.len() || bytes[index + 1] != '{' {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        let Some(end) = (index + 2..bytes.len()).find(|&i| bytes[i] == '}') else {
            return Err(McpConfigError::InvalidField {
                field: field.to_owned(),
                detail: "has an unterminated ${...} reference".into(),
            });
        };
        let inner: String = bytes[index + 2..end].iter().collect();
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default.to_owned())),
            None => (inner.as_str(), None),
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(McpConfigError::InvalidField {
                field: field.to_owned(),
                detail: "has an invalid ${...} variable name".into(),
            });
        }
        match env.get(name).cloned().or(default) {
            Some(resolved) => out.push_str(&resolved),
            None => {
                return Err(McpConfigError::MissingVariable {
                    variable: name.to_owned(),
                    field: field.to_owned(),
                })
            }
        }
        index = end + 1;
    }
    Ok(out)
}

/// A stable digest of one raw entry, used as the trust key. FNV-1a over the
/// canonical JSON keeps the crate dependency-free; this is a change detector,
/// not a security primitive, and it never sees an interpolated value.
pub fn fingerprint(name: &str, definition: &serde_json::Value) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut write = |text: &str| {
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    write(name);
    write("\u{0}");
    write(&canonical(definition));
    format!("{hash:016x}")
}

/// JSON with object keys in sorted order, so a reordered but identical
/// definition keeps its approval while any real edit invalidates it.
fn canonical(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String(key.clone()),
                        canonical(&map[key])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// Lifecycle state of one configured entry. Every configured key always has
/// one, so `/mcp` can account for the whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum McpServerState {
    /// Parsed and runnable, but not yet approved for this workspace.
    AwaitingApproval,
    /// Approval was declined; nothing was launched.
    Denied,
    /// Approved, connecting or handshaking. Queued servers wait here.
    Starting,
    /// Initialized; its tools are in the registry.
    Ready,
    /// Launch, handshake, or the connection failed. The reason is safe.
    Failed { reason: String },
    /// Shut down cleanly, by exit or by an explicit stop.
    Stopped,
    /// The entry could not be parsed.
    Invalid { reason: String },
    /// The entry named a transport Gritt does not implement.
    UnsupportedTransport { reason: String },
}

impl McpServerState {
    /// The one-line reason shown beside the state, empty when there is none.
    pub fn reason(&self) -> &str {
        match self {
            McpServerState::Failed { reason }
            | McpServerState::Invalid { reason }
            | McpServerState::UnsupportedTransport { reason } => reason,
            _ => "",
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, McpServerState::Ready)
    }

    /// A one-line explanation for any state, never empty.
    ///
    /// [`McpServerState::reason`] carries failure detail and is empty for the
    /// states that have none. An interface still has to tell the user why a
    /// server is not running, so every state gets a sentence here.
    pub fn explain(&self) -> String {
        match self {
            McpServerState::AwaitingApproval => {
                "waiting for approval to run this definition in this workspace".to_owned()
            }
            McpServerState::Denied => {
                "approval was declined for this definition; it will not be launched".to_owned()
            }
            McpServerState::Starting => "connecting and negotiating the protocol".to_owned(),
            McpServerState::Ready => "connected; its tools are available".to_owned(),
            McpServerState::Stopped => "stopped; restart it to reconnect".to_owned(),
            McpServerState::Failed { reason } => format!("failed: {reason}"),
            McpServerState::Invalid { reason } => format!("the entry is not usable: {reason}"),
            McpServerState::UnsupportedTransport { reason } => {
                format!("gritt cannot connect to it: {reason}")
            }
        }
    }
}

/// What the interface shows for one entry. Safe to log and persist: it holds
/// no environment value, header value, or URL credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerSnapshot {
    pub name: String,
    pub state: McpServerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<McpTransportKind>,
    pub tool_count: usize,
    /// Dispatch names contributed to the registry, in registry order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    pub fingerprint: String,
}

/// One tool as the registry holds it: the provider-valid dispatch name and
/// the server and tool it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolRef {
    /// The name given to the provider. Unique across every server.
    pub dispatch_name: String,
    pub server: String,
    /// The name the server knows, sent verbatim in `tools/call`.
    pub tool: String,
}

/// Whether a workspace has approved launching a server definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustDecision {
    Approved,
    Denied,
}

/// A first-use decision, keyed by the exact workspace and definition. A
/// changed definition produces a new fingerprint, so the old record no
/// longer matches and the server returns to `awaiting approval`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRecord {
    pub workspace: String,
    pub server: String,
    pub fingerprint: String,
    pub decision: TrustDecision,
}

/// Deadlines and bounds for the runtime. Every value is configurable so a
/// slow server can be accommodated without editing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpRuntimeSettings {
    /// Launch plus `initialize` plus the first `tools/list`.
    pub init_timeout: Duration,
    /// One `tools/call`.
    pub call_timeout: Duration,
    /// How long a stdio child may take to exit after its stdin closes,
    /// before termination is escalated.
    pub shutdown_grace: Duration,
    /// How many servers initialize at once. The rest queue; there is no cap
    /// on how many entries may be configured.
    pub max_concurrent_init: usize,
    /// Largest tool result text handed back to the model.
    pub max_result_bytes: usize,
    /// Guard against a server that never stops paginating.
    pub max_list_pages: usize,
}

impl Default for McpRuntimeSettings {
    fn default() -> Self {
        Self {
            init_timeout: Duration::from_secs(30),
            call_timeout: Duration::from_secs(120),
            shutdown_grace: Duration::from_secs(5),
            max_concurrent_init: 4,
            max_result_bytes: 64 * 1024,
            max_list_pages: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn missing_servers_object_means_no_servers() {
        let config = parse_mcp_config("{}", &env(&[]), ".mcp.json").unwrap();
        assert!(config.is_empty());
    }

    #[test]
    fn invalid_json_is_a_visible_error_without_the_content() {
        let error = parse_mcp_config(
            "{\"mcpServers\": {\"a\": {\"env\": {\"TOKEN\": \"sk-leak",
            &env(&[]),
            ".mcp.json",
        )
        .unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::Config);
        assert!(!error.message.contains("sk-leak"), "{}", error.message);
        assert!(error.message.contains(".mcp.json"));
    }

    #[test]
    fn stdio_entry_without_a_type_is_inferred_and_args_are_verbatim() {
        let text = r#"{"mcpServers": {"anything at all": {
            "command": ".agents/gritt-agent", "args": ["mcp", "serve", "$HOME"]}}}"#;
        let config = parse_mcp_config(text, &env(&[("HOME", "/home/x")]), ".mcp.json").unwrap();
        let McpEntry::Server(server) = &config.entries[0] else {
            panic!("{config:?}");
        };
        assert_eq!(server.name, "anything at all");
        let McpTransport::Stdio { command, args, env } = &server.transport else {
            panic!("{server:?}");
        };
        assert_eq!(command, ".agents/gritt-agent");
        assert_eq!(args, &["mcp", "serve", "$HOME"]);
        assert!(env.is_empty());
    }

    #[test]
    fn a_bad_entry_is_isolated_from_healthy_ones() {
        let text = r#"{"mcpServers": {
            "good": {"command": "srv"},
            "legacy": {"type": "sse", "url": "https://example.test/sse"},
            "broken": {"args": ["x"]},
            "remote": {"type": "http", "url": "https://example.test/mcp"}}}"#;
        let config = parse_mcp_config(text, &env(&[]), ".mcp.json").unwrap();
        assert_eq!(config.names(), ["broken", "good", "legacy", "remote"]);
        let McpEntry::Rejected { error, .. } = config.get("legacy").unwrap() else {
            panic!("legacy should be rejected");
        };
        assert!(error.is_unsupported_transport());
        let McpEntry::Rejected { error, .. } = config.get("broken").unwrap() else {
            panic!("broken should be rejected");
        };
        assert_eq!(
            error,
            &McpConfigError::MissingField {
                field: "command".into()
            }
        );
        assert!(matches!(config.get("good").unwrap(), McpEntry::Server(_)));
        assert!(matches!(config.get("remote").unwrap(), McpEntry::Server(_)));
    }

    #[test]
    fn interpolation_covers_defaults_and_missing_variables() {
        let vars = env(&[("PRESENT", "here")]);
        assert_eq!(interpolate("a${PRESENT}b", &vars, "f").unwrap(), "ahereb");
        assert_eq!(
            interpolate("${ABSENT:-fallback}", &vars, "f").unwrap(),
            "fallback"
        );
        assert_eq!(interpolate("100% $ok", &vars, "f").unwrap(), "100% $ok");
        // No shell: the default is literal text.
        assert_eq!(interpolate("${X:-$(id)}", &vars, "f").unwrap(), "$(id)");
        assert_eq!(
            interpolate("${ABSENT}", &vars, "env.A").unwrap_err(),
            McpConfigError::MissingVariable {
                variable: "ABSENT".into(),
                field: "env.A".into()
            }
        );
        assert!(interpolate("${UNCLOSED", &vars, "f").is_err());
    }

    #[test]
    fn a_missing_variable_disables_only_that_server() {
        let text = r#"{"mcpServers": {
            "needs": {"command": "srv", "env": {"REGION": "${MCP_REGION}"}},
            "fine": {"command": "srv"}}}"#;
        let config = parse_mcp_config(text, &env(&[]), ".mcp.json").unwrap();
        let McpEntry::Rejected { error, .. } = config.get("needs").unwrap() else {
            panic!("needs should be rejected");
        };
        assert!(error.message().contains("MCP_REGION"));
        assert!(matches!(config.get("fine").unwrap(), McpEntry::Server(_)));
    }

    #[test]
    fn literal_credentials_are_refused_without_echoing_them() {
        let text = r#"{"mcpServers": {
            "a": {"command": "srv", "env": {"API_KEY": "sk-literal-value"}},
            "b": {"type": "http", "url": "https://e.test/mcp",
                  "headers": {"Authorization": "Bearer sk-literal-value"}},
            "c": {"type": "http", "url": "https://e.test/mcp",
                  "headers": {"X-Api-Key": "${REAL_KEY:-sk-default}"}}}}"#;
        let config = parse_mcp_config(text, &env(&[]), ".mcp.json").unwrap();
        for name in ["a", "b", "c"] {
            let McpEntry::Rejected { error, .. } = config.get(name).unwrap() else {
                panic!("{name} should be rejected");
            };
            assert!(matches!(error, McpConfigError::EmbeddedCredential { .. }));
            assert!(!error.message().contains("sk-literal-value"));
            assert!(!error.message().contains("sk-default"));
        }
    }

    #[test]
    fn credential_references_resolve_and_plain_fields_still_interpolate() {
        let text = r#"{"mcpServers": {"remote": {"type": "http",
            "url": "https://e.test/mcp",
            "headers": {"Authorization": "${MCP_TOKEN}", "X-Region": "${REGION:-us}"}}}}"#;
        let config =
            parse_mcp_config(text, &env(&[("MCP_TOKEN", "Bearer live")]), ".mcp.json").unwrap();
        let McpEntry::Server(server) = config.get("remote").unwrap() else {
            panic!("{config:?}");
        };
        let McpTransport::Http { headers, .. } = &server.transport else {
            panic!("{server:?}");
        };
        assert_eq!(headers["Authorization"], "Bearer live");
        assert_eq!(headers["X-Region"], "us");
        // The resolved header never reaches Debug output.
        assert!(!format!("{:?}", server.transport).contains("Bearer live"));
    }

    #[test]
    fn the_fingerprint_tracks_the_definition_not_its_key_order() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"command": "srv", "args": ["x"]}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"args": ["x"], "command": "srv"}"#).unwrap();
        let c: serde_json::Value =
            serde_json::from_str(r#"{"command": "srv", "args": ["y"]}"#).unwrap();
        assert_eq!(fingerprint("s", &a), fingerprint("s", &b));
        assert_ne!(fingerprint("s", &a), fingerprint("s", &c));
        assert_ne!(fingerprint("s", &a), fingerprint("other", &a));
    }

    #[test]
    fn protocol_versions_are_pinned() {
        assert!(protocol_version_supported(LATEST_PROTOCOL_VERSION));
        assert!(protocol_version_supported("2025-03-26"));
        assert!(!protocol_version_supported("2019-01-01"));
        assert_eq!(SUPPORTED_PROTOCOL_VERSIONS[0], LATEST_PROTOCOL_VERSION);
    }

    #[test]
    fn states_and_snapshots_serialize_with_a_tag() {
        let snapshot = McpServerSnapshot {
            name: "any".into(),
            state: McpServerState::Failed {
                reason: "exited before the handshake".into(),
            },
            transport: Some(McpTransportKind::Stdio),
            tool_count: 0,
            tools: Vec::new(),
            protocol_version: None,
            server_version: None,
            fingerprint: "abc".into(),
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["state"]["state"], "failed");
        assert_eq!(json["transport"], "stdio");
        let back: McpServerSnapshot = serde_json::from_value(json).unwrap();
        assert_eq!(back, snapshot);
        assert_eq!(McpServerState::AwaitingApproval.reason(), "");
    }

    #[test]
    fn every_state_explains_itself() {
        let states = [
            McpServerState::AwaitingApproval,
            McpServerState::Denied,
            McpServerState::Starting,
            McpServerState::Ready,
            McpServerState::Stopped,
            McpServerState::Failed {
                reason: "it exited".into(),
            },
            McpServerState::Invalid {
                reason: "missing `command`".into(),
            },
            McpServerState::UnsupportedTransport {
                reason: "transport `sse` is not supported".into(),
            },
        ];
        for state in states {
            assert!(!state.explain().is_empty(), "{state:?} has no explanation");
        }
        assert!(McpServerState::Denied.explain().contains("declined"));
        assert!(McpServerState::Failed {
            reason: "it exited".into()
        }
        .explain()
        .contains("it exited"));
    }
}

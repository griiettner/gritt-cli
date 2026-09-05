//! The MCP runtime: one connection per server per workspace, generic over
//! every `mcpServers` entry.
//!
//! The binary reads `<workspace>/.mcp.json` and `gritt-core` parses it; this
//! module owns approval, connections, process lifetime, discovery, and
//! dispatch. Nothing here knows a server name: entries are whatever the file
//! declares, in whatever number.
//!
//! The runtime holds one connection per server for the whole workspace
//! session, not one per model turn, so switching sessions reuses healthy
//! connections. A failed server never disables a healthy one: each entry
//! carries its own state and its own tools.

pub mod connection;
pub mod http;
pub mod jsonrpc;
pub mod registry;
pub mod stdio;
pub mod trust;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use gritt_core::mcp::{
    parse_mcp_config, McpConfig, McpEntry, McpRuntimeSettings, McpServerConfig, McpServerSnapshot,
    McpServerState, McpToolRef, McpTransport, McpTransportKind, TrustDecision, TrustRecord,
    LATEST_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};
use gritt_core::tool::ToolDefinition;
use gritt_core::{Error, Result};
use gritt_provider::transport::HttpTransport;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use self::connection::Connection;
use self::registry::{render_result, RenderedResult, ToolRegistry};
use self::trust::{MemoryTrustStore, TrustStore};
use crate::CancellationToken;

pub use self::registry::{dispatch_name, is_dispatch_name, RegisteredTool};

/// The file every workspace is read from. Gritt never writes it.
pub const CONFIG_FILE: &str = ".mcp.json";

/// The tools one turn may call. Taken once at the start of a turn so a
/// `tools/list_changed` arriving mid-turn cannot change the set the model
/// was shown.
#[derive(Debug, Clone, Default)]
pub struct McpToolSet {
    definitions: Vec<ToolDefinition>,
    index: HashMap<String, McpToolRef>,
}

impl McpToolSet {
    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    pub fn lookup(&self, dispatch_name: &str) -> Option<&McpToolRef> {
        self.index.get(dispatch_name)
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }
}

/// One configured entry as the runtime tracks it.
struct ServerRuntime {
    name: String,
    fingerprint: String,
    /// `None` for an entry that could not be parsed.
    config: Option<McpServerConfig>,
    state: McpServerState,
    connection: Option<Arc<Connection>>,
    pid: Option<u32>,
    protocol_version: Option<String>,
    server_version: Option<String>,
}

impl ServerRuntime {
    fn transport(&self) -> Option<McpTransportKind> {
        self.config.as_ref().map(|config| config.transport.kind())
    }
}

#[derive(Default)]
struct RuntimeState {
    servers: BTreeMap<String, ServerRuntime>,
    registry: ToolRegistry,
}

pub struct McpRuntime {
    workspace: PathBuf,
    settings: McpRuntimeSettings,
    http: Option<Arc<dyn HttpTransport>>,
    trust: Arc<dyn TrustStore>,
    state: Mutex<RuntimeState>,
}

impl McpRuntime {
    /// A runtime for one workspace. Without an HTTP transport, `http`
    /// entries report that plainly instead of pretending to start.
    pub fn new(workspace: impl Into<PathBuf>, settings: McpRuntimeSettings) -> Self {
        Self {
            workspace: workspace.into(),
            settings,
            http: None,
            trust: MemoryTrustStore::new(),
            state: Mutex::new(RuntimeState::default()),
        }
    }

    pub fn with_http_transport(mut self, transport: Arc<dyn HttpTransport>) -> Self {
        self.http = Some(transport);
        self
    }

    /// Injects the persistence seam for first-use decisions. The default
    /// keeps them for the run only.
    pub fn with_trust(mut self, trust: Arc<dyn TrustStore>) -> Self {
        self.trust = trust;
        self
    }

    pub fn settings(&self) -> &McpRuntimeSettings {
        &self.settings
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn workspace_key(&self) -> String {
        self.workspace.to_string_lossy().into_owned()
    }

    /// Reads and parses the workspace file. A missing file is no servers;
    /// unreadable or malformed content is a visible error and nothing is
    /// replaced.
    pub fn read_config(&self) -> Result<McpConfig> {
        let path = self.workspace.join(CONFIG_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(McpConfig::default())
            }
            Err(error) => {
                return Err(Error::config(format!(
                    "cannot read {}: {error}",
                    path.display()
                )))
            }
        };
        let env: BTreeMap<String, String> = std::env::vars().collect();
        parse_mcp_config(&text, &env, CONFIG_FILE)
    }

    /// Applies a parsed configuration.
    ///
    /// An entry whose definition is unchanged keeps its connection and its
    /// tools. A changed, renamed, or removed entry has its connection shut
    /// down first, which drains it: in-flight calls fail rather than being
    /// answered by a server the user no longer configured. Trust is consulted
    /// per entry, so a new or edited definition returns to `awaiting
    /// approval`.
    pub async fn load(&self, config: &McpConfig) -> Result<()> {
        let workspace = self.workspace_key();
        // Decisions are read before the lock so the state mutation stays
        // synchronous and cannot interleave with a turn.
        let mut decisions: HashMap<String, Option<TrustDecision>> = HashMap::new();
        for entry in &config.entries {
            if let McpEntry::Server(server) = entry {
                let decision = self
                    .trust
                    .decision(&workspace, &server.name, &server.fingerprint)
                    .await?;
                decisions.insert(server.name.clone(), decision);
            }
        }
        let mut retired = Vec::new();
        {
            let mut state = self.state.lock().await;
            let mut next: BTreeMap<String, ServerRuntime> = BTreeMap::new();
            for entry in &config.entries {
                let name = entry.name().to_owned();
                let fingerprint = entry.fingerprint().to_owned();
                let existing = state.servers.remove(&name);
                match entry {
                    McpEntry::Rejected { error, .. } => {
                        if let Some(previous) = existing {
                            if let Some(connection) = previous.connection {
                                retired.push((name.clone(), connection));
                            }
                        }
                        state.registry.remove_server(&name);
                        let reason = error.message();
                        let server_state = if error.is_unsupported_transport() {
                            McpServerState::UnsupportedTransport { reason }
                        } else {
                            McpServerState::Invalid { reason }
                        };
                        next.insert(
                            name.clone(),
                            ServerRuntime {
                                name,
                                fingerprint,
                                config: None,
                                state: server_state,
                                connection: None,
                                pid: None,
                                protocol_version: None,
                                server_version: None,
                            },
                        );
                    }
                    McpEntry::Server(server) => {
                        // An untouched definition keeps its live connection.
                        if let Some(previous) = existing {
                            if previous.fingerprint == fingerprint
                                && previous.connection.is_some()
                                && previous.state.is_ready()
                            {
                                next.insert(name.clone(), previous);
                                continue;
                            }
                            if let Some(connection) = previous.connection {
                                retired.push((name.clone(), connection));
                            }
                        }
                        state.registry.remove_server(&name);
                        let server_state = match decisions.get(&name).copied().flatten() {
                            Some(TrustDecision::Approved) => McpServerState::Starting,
                            Some(TrustDecision::Denied) => McpServerState::Denied,
                            None => McpServerState::AwaitingApproval,
                        };
                        next.insert(
                            name.clone(),
                            ServerRuntime {
                                name,
                                fingerprint,
                                config: Some(server.clone()),
                                state: server_state,
                                connection: None,
                                pid: None,
                                protocol_version: None,
                                server_version: None,
                            },
                        );
                    }
                }
            }
            // Anything left is an entry the new file no longer declares.
            for (name, previous) in std::mem::take(&mut state.servers) {
                state.registry.remove_server(&name);
                if let Some(connection) = previous.connection {
                    retired.push((name, connection));
                }
            }
            state.servers = next;
        }
        for (_, connection) in retired {
            connection.shutdown().await;
        }
        Ok(())
    }

    /// Validates a replacement configuration before it replaces the active
    /// one. A parse failure leaves every running server untouched.
    pub async fn reload(&self) -> Result<()> {
        let config = self.read_config()?;
        self.load(&config).await
    }

    /// Records a first-use decision and moves the entry out of `awaiting
    /// approval`. Approving does not connect; call [`McpRuntime::start`].
    pub async fn decide(&self, server: &str, decision: TrustDecision) -> Result<()> {
        let fingerprint = {
            let state = self.state.lock().await;
            let entry = state.servers.get(server).ok_or_else(|| {
                Error::config(format!("`{server}` is not a configured MCP server"))
            })?;
            if entry.config.is_none() {
                return Err(Error::config(format!(
                    "`{server}` cannot be approved: {}",
                    entry.state.reason()
                )));
            }
            entry.fingerprint.clone()
        };
        self.trust
            .record(TrustRecord {
                workspace: self.workspace_key(),
                server: server.to_owned(),
                fingerprint: fingerprint.clone(),
                decision,
            })
            .await?;
        let mut state = self.state.lock().await;
        if let Some(entry) = state.servers.get_mut(server) {
            entry.state = match decision {
                TrustDecision::Approved => McpServerState::Starting,
                TrustDecision::Denied => McpServerState::Denied,
            };
        }
        Ok(())
    }

    /// Initializes every approved server that is not connected yet.
    ///
    /// At most `max_concurrent_init` handshakes run at once; the rest queue,
    /// so a file may declare any number of servers. One server's failure is
    /// recorded against that server only.
    pub async fn start(&self, cancel: &CancellationToken) -> Vec<McpServerSnapshot> {
        let pending: Vec<McpServerConfig> = {
            let state = self.state.lock().await;
            state
                .servers
                .values()
                .filter(|entry| {
                    entry.connection.is_none() && matches!(entry.state, McpServerState::Starting)
                })
                .filter_map(|entry| entry.config.clone())
                .collect()
        };
        if pending.is_empty() {
            return self.snapshots().await;
        }
        let limit = self.settings.max_concurrent_init.max(1);
        let outcomes: Vec<(String, std::result::Result<Established, String>)> =
            futures::stream::iter(pending.into_iter().map(|config| async move {
                let name = config.name.clone();
                let outcome = self.connect(&config, cancel).await;
                (name, outcome)
            }))
            .buffer_unordered(limit)
            .collect()
            .await;
        let mut state = self.state.lock().await;
        for (name, outcome) in outcomes {
            let Some(entry) = state.servers.get_mut(&name) else {
                // The entry was reloaded away while it was connecting.
                if let Ok(established) = outcome {
                    let connection = established.connection;
                    tokio::spawn(async move { connection.shutdown().await });
                }
                continue;
            };
            match outcome {
                Ok(established) => {
                    entry.state = McpServerState::Ready;
                    entry.connection = Some(established.connection);
                    entry.pid = established.pid;
                    entry.protocol_version = Some(established.protocol_version);
                    entry.server_version = established.server_version;
                    state.registry.remove_server(&name);
                    for tool in &established.tools {
                        state.registry.insert(&name, tool);
                    }
                }
                Err(reason) => {
                    entry.state = McpServerState::Failed { reason };
                    entry.connection = None;
                    entry.pid = None;
                }
            }
        }
        drop(state);
        self.snapshots().await
    }

    /// Loads the workspace file and starts every already-approved server.
    pub async fn open(&self, cancel: &CancellationToken) -> Result<Vec<McpServerSnapshot>> {
        let config = self.read_config()?;
        self.load(&config).await?;
        Ok(self.start(cancel).await)
    }

    /// Stops one server and connects it again. A failed server restarts only
    /// through this explicit action.
    pub async fn restart(&self, server: &str, cancel: &CancellationToken) -> Result<()> {
        let connection = {
            let mut state = self.state.lock().await;
            let entry = state.servers.get_mut(server).ok_or_else(|| {
                Error::config(format!("`{server}` is not a configured MCP server"))
            })?;
            if entry.config.is_none() {
                return Err(Error::config(format!(
                    "`{server}` cannot run: {}",
                    entry.state.reason()
                )));
            }
            let connection = entry.connection.take();
            entry.pid = None;
            entry.protocol_version = None;
            entry.server_version = None;
            entry.state = McpServerState::Starting;
            state.registry.remove_server(server);
            connection
        };
        if let Some(connection) = connection {
            connection.shutdown().await;
        }
        self.start(cancel).await;
        Ok(())
    }

    /// Stops one server without forgetting it. Its entry stays visible as
    /// `stopped`.
    pub async fn stop(&self, server: &str) -> Result<()> {
        let connection = {
            let mut state = self.state.lock().await;
            let entry = state.servers.get_mut(server).ok_or_else(|| {
                Error::config(format!("`{server}` is not a configured MCP server"))
            })?;
            let connection = entry.connection.take();
            entry.pid = None;
            if entry.config.is_some() {
                entry.state = McpServerState::Stopped;
            }
            state.registry.remove_server(server);
            connection
        };
        if let Some(connection) = connection {
            connection.shutdown().await;
        }
        Ok(())
    }

    /// Ends every connection. Stdio children get the specified sequence:
    /// stdin closed, a bounded grace period, then termination and a kill.
    /// Gritt owns every process it launched, so none may outlive this.
    pub async fn shutdown(&self) {
        let connections: Vec<Arc<Connection>> = {
            let mut state = self.state.lock().await;
            let mut connections = Vec::new();
            for entry in state.servers.values_mut() {
                if let Some(connection) = entry.connection.take() {
                    connections.push(connection);
                }
                entry.pid = None;
                if entry.config.is_some() && entry.state.is_ready() {
                    entry.state = McpServerState::Stopped;
                }
            }
            state.registry = ToolRegistry::new();
            connections
        };
        futures::future::join_all(connections.iter().map(|connection| connection.shutdown())).await;
    }

    /// Applies pending `tools/list_changed` notifications and notices
    /// connections that died. Called between turns, never during one, so the
    /// tool set a model was shown stays stable for that turn.
    pub async fn refresh(&self, cancel: &CancellationToken) -> Vec<McpServerSnapshot> {
        let candidates: Vec<(String, Arc<Connection>)> = {
            let state = self.state.lock().await;
            state
                .servers
                .values()
                .filter(|entry| entry.state.is_ready())
                .filter_map(|entry| {
                    entry
                        .connection
                        .clone()
                        .map(|connection| (entry.name.clone(), connection))
                })
                .collect()
        };
        for (name, connection) in candidates {
            if connection.is_closed() {
                self.mark_lost(&name, "the server closed the connection")
                    .await;
                continue;
            }
            if !connection.take_tools_changed() {
                continue;
            }
            match self.discover(&connection, None, cancel).await {
                Ok(tools) => {
                    let mut state = self.state.lock().await;
                    state.registry.remove_server(&name);
                    for tool in &tools {
                        state.registry.insert(&name, tool);
                    }
                }
                Err(error) => self.mark_lost(&name, &error.message).await,
            }
        }
        self.snapshots().await
    }

    async fn mark_lost(&self, server: &str, reason: &str) {
        let mut state = self.state.lock().await;
        if let Some(entry) = state.servers.get_mut(server) {
            entry.connection = None;
            entry.pid = None;
            entry.state = McpServerState::Failed {
                reason: reason.to_owned(),
            };
        }
        state.registry.remove_server(server);
    }

    /// The tools available right now, as one immutable per-turn snapshot.
    pub async fn tool_set(&self) -> McpToolSet {
        let state = self.state.lock().await;
        McpToolSet {
            definitions: state.registry.definitions(),
            index: state
                .registry
                .tools()
                .iter()
                .map(|tool| (tool.reference.dispatch_name.clone(), tool.reference.clone()))
                .collect(),
        }
    }

    /// Every configured entry with its state, tool count, and safe reason.
    /// Nothing here holds an environment value, a header value, or a URL.
    pub async fn snapshots(&self) -> Vec<McpServerSnapshot> {
        let state = self.state.lock().await;
        state
            .servers
            .values()
            .map(|entry| McpServerSnapshot {
                name: entry.name.clone(),
                state: entry.state.clone(),
                transport: entry.transport(),
                tool_count: state.registry.names_for(&entry.name).len(),
                tools: state.registry.names_for(&entry.name),
                protocol_version: entry.protocol_version.clone(),
                server_version: entry.server_version.clone(),
                fingerprint: entry.fingerprint.clone(),
            })
            .collect()
    }

    /// The process ids of the stdio children this runtime owns, for tests
    /// and diagnostics.
    pub async fn child_pids(&self) -> Vec<u32> {
        let state = self.state.lock().await;
        state
            .servers
            .values()
            .filter_map(|entry| entry.pid)
            .collect()
    }

    /// Calls one discovered tool. The caller has already run the permission
    /// engine; nothing in this module can be reached without it.
    ///
    /// A disconnect fails the call. It is never replayed: a remote server may
    /// already have completed the side effect, and the model is told the
    /// outcome is unknown rather than being handed a second attempt.
    pub async fn call(
        &self,
        dispatch_name: &str,
        arguments: &Value,
        cancel: &CancellationToken,
    ) -> Result<RenderedResult> {
        let (reference, connection) = {
            let state = self.state.lock().await;
            let tool = state
                .registry
                .lookup(dispatch_name)
                .ok_or_else(|| Error::config(format!("unknown tool `{dispatch_name}`")))?;
            let reference = tool.reference.clone();
            let connection = state
                .servers
                .get(&reference.server)
                .and_then(|entry| entry.connection.clone())
                .ok_or_else(|| {
                    Error::config(format!(
                        "the `{}` MCP server is not connected",
                        reference.server
                    ))
                })?;
            (reference, connection)
        };
        let params = json!({
            "name": reference.tool,
            "arguments": if arguments.is_null() { json!({}) } else { arguments.clone() },
        });
        let outcome = connection
            .request(
                jsonrpc::method::TOOLS_CALL,
                params,
                self.settings.call_timeout,
                cancel,
            )
            .await;
        match outcome {
            Ok(value) => Ok(render_result(&value, self.settings.max_result_bytes)),
            Err(error) => {
                if connection.is_closed() {
                    self.mark_lost(&reference.server, "the server closed the connection")
                        .await;
                }
                Err(error)
            }
        }
    }

    /// Opens a transport, negotiates the protocol, and discovers tools,
    /// all inside the initialization deadline.
    async fn connect(
        &self,
        config: &McpServerConfig,
        cancel: &CancellationToken,
    ) -> std::result::Result<Established, String> {
        match tokio::time::timeout(
            self.settings.init_timeout,
            self.connect_inner(config, cancel),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(format!(
                "the server was not ready within {}s",
                self.settings.init_timeout.as_secs()
            )),
        }
    }

    async fn connect_inner(
        &self,
        config: &McpServerConfig,
        cancel: &CancellationToken,
    ) -> std::result::Result<Established, String> {
        let (connection, pid, stderr) = match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                let launch = stdio::launch(
                    &self.workspace,
                    command,
                    args,
                    env,
                    self.settings.shutdown_grace,
                )
                .map_err(|error| error.message)?;
                let stderr = Arc::clone(&launch.stderr);
                (Arc::new(launch.connection), launch.pid, Some(stderr))
            }
            McpTransport::Http { url, headers } => {
                let Some(transport) = self.http.clone() else {
                    return Err(
                        "this Gritt build has no HTTP transport for MCP endpoints".to_owned()
                    );
                };
                (Arc::new(http::connect(transport, url, headers)), None, None)
            }
        };
        let tail = |reason: String| match &stderr {
            Some(stderr) => {
                let tail = stderr.lock().expect("mcp stderr");
                let last = tail.trim().lines().next_back().unwrap_or_default().trim();
                if last.is_empty() {
                    reason
                } else {
                    format!("{reason} ({last})")
                }
            }
            None => reason,
        };
        let handshake = self.handshake(&connection).await;
        let handshake = match handshake {
            Ok(handshake) => handshake,
            Err(error) => {
                connection.shutdown().await;
                return Err(tail(error.message));
            }
        };
        let tools = match self
            .discover(&connection, Some(&handshake.capabilities), cancel)
            .await
        {
            Ok(tools) => tools,
            Err(error) => {
                connection.shutdown().await;
                return Err(tail(error.message));
            }
        };
        Ok(Established {
            connection,
            pid,
            protocol_version: handshake.protocol_version,
            server_version: handshake.server_version,
            tools,
        })
    }

    /// `initialize`, version and capability negotiation, then
    /// `notifications/initialized`.
    ///
    /// Gritt advertises no client capabilities: roots, sampling, and
    /// elicitation are not implemented, and the specification forbids using a
    /// capability that was not negotiated.
    async fn handshake(&self, connection: &Connection) -> Result<Handshake> {
        let params = json!({
            "protocolVersion": LATEST_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "gritt",
                "title": "Gritt",
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        // The specification forbids cancelling `initialize`, so this waits
        // for its own deadline instead of sending a cancellation.
        let result = connection
            .request_uncancellable(
                jsonrpc::method::INITIALIZE,
                params,
                self.settings.init_timeout,
            )
            .await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::config("the server did not report a protocol version"))?;
        if !gritt_core::mcp::protocol_version_supported(version) {
            return Err(Error::config(format!(
                "the server answered with MCP revision `{version}`; gritt speaks {}",
                SUPPORTED_PROTOCOL_VERSIONS.join(", ")
            )));
        }
        connection.set_protocol_version(version);
        connection
            .notify(jsonrpc::method::INITIALIZED, json!({}))
            .await?;
        Ok(Handshake {
            protocol_version: version.to_owned(),
            server_version: result
                .get("serverInfo")
                .and_then(|info| info.get("version"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            capabilities: result
                .get("capabilities")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
    }

    /// Paginated `tools/list`. A server that reports no `tools` capability
    /// is simply a server with no tools, not a failure.
    async fn discover(
        &self,
        connection: &Connection,
        capabilities: Option<&Value>,
        cancel: &CancellationToken,
    ) -> Result<Vec<Value>> {
        if let Some(capabilities) = capabilities {
            if capabilities.get("tools").is_none() {
                return Ok(Vec::new());
            }
        }
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..self.settings.max_list_pages {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let page = connection
                .request(
                    jsonrpc::method::TOOLS_LIST,
                    params,
                    self.settings.init_timeout,
                    cancel,
                )
                .await?;
            if let Some(items) = page.get("tools").and_then(Value::as_array) {
                tools.extend(items.iter().cloned());
            }
            let next = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match next {
                None => return Ok(tools),
                Some(next) => {
                    // A cursor that repeats would page forever.
                    if seen.contains(&next) {
                        return Err(Error::config("the server repeated a tools/list cursor"));
                    }
                    seen.push(next.clone());
                    cursor = Some(next);
                }
            }
        }
        Err(Error::config(format!(
            "the server sent more than {} pages of tools",
            self.settings.max_list_pages
        )))
    }
}

struct Handshake {
    protocol_version: String,
    server_version: Option<String>,
    capabilities: Value,
}

struct Established {
    connection: Arc<Connection>,
    pid: Option<u32>,
    protocol_version: String,
    server_version: Option<String>,
    tools: Vec<Value>,
}

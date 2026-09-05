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
//!
//! Two rules run through everything here. Every entry carries a
//! **generation** that changes whenever its definition or its lifecycle
//! changes, so work that started under an older generation is refused rather
//! than published over a newer decision. And every string or value a server
//! produces is **redacted** against the credentials that server was given,
//! because a server can echo its own token back in an error, its metadata, a
//! schema, or a tool result.

pub mod connection;
pub mod http;
pub mod jsonrpc;
pub mod registry;
pub mod stdio;
pub mod trust;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use gritt_core::mcp::{
    parse_mcp_config, McpConfig, McpEntry, McpRuntimeSettings, McpServerConfig, McpServerSnapshot,
    McpServerState, McpToolRef, McpTransport, McpTransportKind, TrustDecision, TrustRecord,
    LATEST_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};
use gritt_core::secret::{is_secret_env_name, Secret};
use gritt_core::tool::ToolDefinition;
use gritt_core::{Error, Result};
use gritt_provider::adapter::{redact_text, redact_value};
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

/// One tool exactly as a turn was authorized to call it.
///
/// The dispatch name alone is not an identity: a reload can hand the same
/// collision-suffixed name to a different original tool. Carrying the server,
/// the original tool name, and the generation means the call that runs is the
/// call the permission engine approved, or it does not run at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenTool {
    pub reference: McpToolRef,
    pub generation: u64,
}

/// The tools one turn may call. Taken once at the start of a turn so a
/// `tools/list_changed` arriving mid-turn cannot change the set the model
/// was shown.
#[derive(Debug, Clone, Default)]
pub struct McpToolSet {
    definitions: Vec<ToolDefinition>,
    index: HashMap<String, FrozenTool>,
}

impl McpToolSet {
    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    pub fn lookup(&self, dispatch_name: &str) -> Option<&FrozenTool> {
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
    /// Changes on every definition or lifecycle change. Asynchronous work
    /// carries the value it started with and is refused if it no longer
    /// matches.
    generation: u64,
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
    /// Every credential value the configured servers were given, redacted
    /// out of anything a server produces.
    secrets: Vec<Secret>,
}

pub struct McpRuntime {
    workspace: PathBuf,
    settings: McpRuntimeSettings,
    http: Option<Arc<dyn HttpTransport>>,
    trust: Arc<dyn TrustStore>,
    state: Mutex<RuntimeState>,
    generations: AtomicU64,
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
            generations: AtomicU64::new(1),
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

    fn next_generation(&self) -> u64 {
        self.generations.fetch_add(1, Ordering::SeqCst) + 1
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
            // Every credential the current file hands to any server, so a
            // server echoing its own token is redacted wherever it lands.
            state.secrets = configured_secrets(config);
            let mut next: BTreeMap<String, ServerRuntime> = BTreeMap::new();
            for entry in &config.entries {
                let name = entry.name().to_owned();
                let fingerprint = entry.fingerprint().to_owned();
                let existing = state.servers.remove(&name);
                match entry {
                    McpEntry::Rejected { error, .. } => {
                        if let Some(previous) = existing {
                            if let Some(connection) = previous.connection {
                                retired.push(connection);
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
                                generation: self.next_generation(),
                            },
                        );
                    }
                    McpEntry::Server(server) => {
                        // An untouched definition keeps its live connection,
                        // and its generation, so a turn frozen against it
                        // stays valid across an unrelated reload.
                        if let Some(previous) = existing {
                            if previous.fingerprint == fingerprint
                                && previous.connection.is_some()
                                && previous.state.is_ready()
                            {
                                next.insert(name.clone(), previous);
                                continue;
                            }
                            if let Some(connection) = previous.connection {
                                retired.push(connection);
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
                                generation: self.next_generation(),
                            },
                        );
                    }
                }
            }
            // Anything left is an entry the new file no longer declares.
            for (name, previous) in std::mem::take(&mut state.servers) {
                state.registry.remove_server(&name);
                if let Some(connection) = previous.connection {
                    retired.push(connection);
                }
            }
            state.servers = next;
        }
        close_all(retired).await;
        Ok(())
    }

    /// Validates a replacement configuration before it replaces the active
    /// one. A parse failure leaves every running server untouched.
    pub async fn reload(&self) -> Result<()> {
        let config = self.read_config()?;
        self.load(&config).await
    }

    /// Records a first-use decision and applies it at once.
    ///
    /// Approving does not connect; call [`McpRuntime::start`]. Denying takes
    /// effect immediately: the server's tools leave the registry and its
    /// connection is closed, so a denial revokes live access rather than only
    /// preventing the next launch.
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
        let retired = {
            let mut state = self.state.lock().await;
            let generation = self.next_generation();
            let Some(entry) = state.servers.get_mut(server) else {
                return Ok(());
            };
            // The definition may have been replaced while the decision was
            // being persisted. Applying it to a different definition would
            // approve something the user never saw.
            if entry.fingerprint != fingerprint {
                return Err(Error::config(format!(
                    "`{server}` changed while the decision was recorded; review it again"
                )));
            }
            entry.generation = generation;
            let retired = entry.connection.take();
            entry.pid = None;
            entry.state = match decision {
                TrustDecision::Approved => McpServerState::Starting,
                TrustDecision::Denied => McpServerState::Denied,
            };
            if retired.is_some() {
                entry.protocol_version = None;
                entry.server_version = None;
            }
            state.registry.remove_server(server);
            retired
        };
        if let Some(connection) = retired {
            connection.shutdown().await;
        }
        Ok(())
    }

    /// Initializes every approved server that is not connected yet.
    ///
    /// At most `max_concurrent_init` handshakes run at once; the rest queue,
    /// so a file may declare any number of servers. One server's failure is
    /// recorded against that server only, and a result whose generation no
    /// longer matches is discarded instead of published.
    pub async fn start(&self, cancel: &CancellationToken) -> Vec<McpServerSnapshot> {
        let pending: Vec<(McpServerConfig, u64)> = {
            let state = self.state.lock().await;
            state
                .servers
                .values()
                .filter(|entry| {
                    entry.connection.is_none() && matches!(entry.state, McpServerState::Starting)
                })
                .filter_map(|entry| {
                    entry
                        .config
                        .clone()
                        .map(|config| (config, entry.generation))
                })
                .collect()
        };
        if pending.is_empty() {
            return self.snapshots().await;
        }
        let limit = self.settings.max_concurrent_init.max(1);
        let outcomes: Vec<(String, u64, std::result::Result<Established, String>)> =
            futures::stream::iter(pending.into_iter().map(|(config, generation)| async move {
                let name = config.name.clone();
                let outcome = self.connect(&config, cancel).await;
                (name, generation, outcome)
            }))
            .buffer_unordered(limit)
            .collect()
            .await;
        let mut stale = Vec::new();
        {
            let mut state = self.state.lock().await;
            for (name, generation, outcome) in outcomes {
                let current = state.servers.get(&name).map(|entry| entry.generation);
                if current != Some(generation) {
                    // Reloaded, denied, stopped, or shut down while this was
                    // connecting. The result belongs to a definition that is
                    // no longer configured under this name.
                    if let Ok(established) = outcome {
                        stale.push(established.connection);
                    }
                    continue;
                }
                let secrets = state.secrets.clone();
                let Some(entry) = state.servers.get_mut(&name) else {
                    continue;
                };
                match outcome {
                    Ok(established) => {
                        entry.state = McpServerState::Ready;
                        entry.connection = Some(established.connection);
                        entry.pid = established.pid;
                        entry.protocol_version = Some(established.protocol_version);
                        entry.server_version = established
                            .server_version
                            .map(|version| redact_text(&version, &secrets));
                        state.registry.remove_server(&name);
                        for tool in &established.tools {
                            state.registry.insert(&name, tool);
                        }
                    }
                    Err(reason) => {
                        entry.state = McpServerState::Failed {
                            reason: redact_text(&reason, &secrets),
                        };
                        entry.connection = None;
                        entry.pid = None;
                    }
                }
            }
        }
        close_all(stale).await;
        self.snapshots().await
    }

    /// Loads the workspace file and starts every already-approved server.
    pub async fn open(&self, cancel: &CancellationToken) -> Result<Vec<McpServerSnapshot>> {
        let config = self.read_config()?;
        self.load(&config).await?;
        Ok(self.start(cancel).await)
    }

    /// Stops one server and connects it again.
    ///
    /// Restarting is not a way around approval: the current definition must
    /// be trusted, so an entry that is awaiting approval or denied is put back
    /// into that state instead of being launched.
    pub async fn restart(&self, server: &str, cancel: &CancellationToken) -> Result<()> {
        let fingerprint = {
            let state = self.state.lock().await;
            let entry = state.servers.get(server).ok_or_else(|| {
                Error::config(format!("`{server}` is not a configured MCP server"))
            })?;
            if entry.config.is_none() {
                return Err(Error::config(format!(
                    "`{server}` cannot run: {}",
                    entry.state.reason()
                )));
            }
            entry.fingerprint.clone()
        };
        let decision = self
            .trust
            .decision(&self.workspace_key(), server, &fingerprint)
            .await?;
        let retired = {
            let mut state = self.state.lock().await;
            let generation = self.next_generation();
            let Some(entry) = state.servers.get_mut(server) else {
                return Ok(());
            };
            if entry.fingerprint != fingerprint {
                return Err(Error::config(format!(
                    "`{server}` changed while it was being restarted; review it again"
                )));
            }
            entry.generation = generation;
            let retired = entry.connection.take();
            entry.pid = None;
            entry.protocol_version = None;
            entry.server_version = None;
            entry.state = match decision {
                Some(TrustDecision::Approved) => McpServerState::Starting,
                Some(TrustDecision::Denied) => McpServerState::Denied,
                None => McpServerState::AwaitingApproval,
            };
            state.registry.remove_server(server);
            retired
        };
        if let Some(connection) = retired {
            connection.shutdown().await;
        }
        self.start(cancel).await;
        Ok(())
    }

    /// Stops one server without forgetting it. Its entry stays visible as
    /// `stopped`.
    pub async fn stop(&self, server: &str) -> Result<()> {
        let retired = {
            let mut state = self.state.lock().await;
            let generation = self.next_generation();
            let entry = state.servers.get_mut(server).ok_or_else(|| {
                Error::config(format!("`{server}` is not a configured MCP server"))
            })?;
            entry.generation = generation;
            let retired = entry.connection.take();
            entry.pid = None;
            if entry.config.is_some() {
                entry.state = McpServerState::Stopped;
            }
            state.registry.remove_server(server);
            retired
        };
        if let Some(connection) = retired {
            connection.shutdown().await;
        }
        Ok(())
    }

    /// Ends every connection. Stdio children get the specified sequence:
    /// stdin closed, a bounded grace period, then termination and a kill,
    /// covering the process group so a descendant cannot outlive its parent.
    /// Gritt owns every process it launched, so none may outlive this.
    pub async fn shutdown(&self) {
        let connections: Vec<Arc<Connection>> = {
            let mut state = self.state.lock().await;
            let mut connections = Vec::new();
            let mut generations = Vec::new();
            for entry in state.servers.values_mut() {
                if let Some(connection) = entry.connection.take() {
                    connections.push(connection);
                }
                entry.pid = None;
                if entry.config.is_some() && entry.state.is_ready() {
                    entry.state = McpServerState::Stopped;
                }
                generations.push(entry.name.clone());
            }
            // A handshake still running belongs to a runtime that is going
            // away; bumping every generation makes its result stale.
            for name in generations {
                let generation = self.next_generation();
                if let Some(entry) = state.servers.get_mut(&name) {
                    entry.generation = generation;
                }
            }
            state.registry = ToolRegistry::new();
            connections
        };
        close_all(connections).await;
    }

    /// Applies pending `tools/list_changed` notifications and notices
    /// connections that died. Called between turns, never during one, so the
    /// tool set a model was shown stays stable for that turn.
    pub async fn refresh(&self, cancel: &CancellationToken) -> Vec<McpServerSnapshot> {
        let candidates: Vec<(String, u64, Arc<Connection>)> = {
            let state = self.state.lock().await;
            state
                .servers
                .values()
                .filter(|entry| entry.state.is_ready())
                .filter_map(|entry| {
                    entry
                        .connection
                        .clone()
                        .map(|connection| (entry.name.clone(), entry.generation, connection))
                })
                .collect()
        };
        for (name, generation, connection) in candidates {
            if connection.is_closed() {
                self.mark_lost(&name, generation, "the server closed the connection")
                    .await;
                continue;
            }
            if !connection.take_tools_changed() {
                continue;
            }
            match self.discover(&connection, None, cancel).await {
                Ok(tools) => {
                    let mut state = self.state.lock().await;
                    if state.servers.get(&name).map(|entry| entry.generation) != Some(generation) {
                        continue;
                    }
                    state.registry.remove_server(&name);
                    for tool in &tools {
                        state.registry.insert(&name, tool);
                    }
                }
                Err(error) => {
                    self.mark_lost(&name, generation, &error.message).await;
                }
            }
        }
        self.snapshots().await
    }

    /// Records that a connection is gone, unless the entry has moved on.
    async fn mark_lost(&self, server: &str, generation: u64, reason: &str) {
        let mut state = self.state.lock().await;
        if state.servers.get(server).map(|entry| entry.generation) != Some(generation) {
            return;
        }
        let secrets = state.secrets.clone();
        if let Some(entry) = state.servers.get_mut(server) {
            entry.connection = None;
            entry.pid = None;
            entry.state = McpServerState::Failed {
                reason: redact_text(reason, &secrets),
            };
        }
        state.registry.remove_server(server);
    }

    /// The tools available right now, as one immutable per-turn snapshot.
    /// Each entry carries the generation it was taken under, so a call is
    /// only executed against the definition that was authorized.
    pub async fn tool_set(&self) -> McpToolSet {
        let state = self.state.lock().await;
        let generations: HashMap<&str, u64> = state
            .servers
            .values()
            .map(|entry| (entry.name.as_str(), entry.generation))
            .collect();
        McpToolSet {
            definitions: state.registry.definitions(),
            index: state
                .registry
                .tools()
                .iter()
                .filter_map(|tool| {
                    generations
                        .get(tool.reference.server.as_str())
                        .map(|generation| {
                            (
                                tool.reference.dispatch_name.clone(),
                                FrozenTool {
                                    reference: tool.reference.clone(),
                                    generation: *generation,
                                },
                            )
                        })
                })
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

    /// Calls the exact tool a turn was authorized to call.
    ///
    /// The caller has already run the permission engine; nothing in this
    /// module can be reached without it. The frozen reference is checked
    /// against the live registry, so a reload that reassigned a
    /// collision-suffixed name cannot make an approved call reach a different
    /// tool. A server that is no longer ready, including one whose trust was
    /// revoked, refuses the call.
    ///
    /// A disconnect fails the call. It is never replayed: a remote server may
    /// already have completed the side effect, and the model is told the
    /// outcome is unknown rather than being handed a second attempt.
    pub async fn call(
        &self,
        tool: &FrozenTool,
        arguments: &Value,
        cancel: &CancellationToken,
    ) -> Result<RenderedResult> {
        let (connection, generation, secrets) = {
            let state = self.state.lock().await;
            let entry = state.servers.get(&tool.reference.server).ok_or_else(|| {
                Error::config(format!(
                    "the `{}` MCP server is no longer configured",
                    tool.reference.server
                ))
            })?;
            if entry.generation != tool.generation {
                return Err(Error::config(format!(
                    "the `{}` MCP server changed since this tool was offered; \
                     the call was not made",
                    tool.reference.server
                )));
            }
            if !entry.state.is_ready() {
                return Err(Error::config(format!(
                    "the `{}` MCP server is not available",
                    tool.reference.server
                )));
            }
            // The live registry must still map this name to this exact tool.
            let registered = state
                .registry
                .lookup(&tool.reference.dispatch_name)
                .ok_or_else(|| {
                    Error::config(format!("unknown tool `{}`", tool.reference.dispatch_name))
                })?;
            if registered.reference != tool.reference {
                return Err(Error::config(format!(
                    "`{}` no longer refers to the tool that was approved; \
                     the call was not made",
                    tool.reference.dispatch_name
                )));
            }
            let connection = entry.connection.clone().ok_or_else(|| {
                Error::config(format!(
                    "the `{}` MCP server is not connected",
                    tool.reference.server
                ))
            })?;
            (connection, entry.generation, state.secrets.clone())
        };
        let params = json!({
            "name": tool.reference.tool,
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
            Ok(value) => {
                // Everything below this line came from the server.
                let value = redact_value(value, &secrets);
                Ok(render_result(&value, self.settings.max_result_bytes))
            }
            Err(error) => {
                if connection.is_closed() {
                    self.mark_lost(
                        &tool.reference.server,
                        generation,
                        "the server closed the connection",
                    )
                    .await;
                }
                Err(redact_error(error, &secrets))
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
        let handshake = match self.handshake(&connection, cancel).await {
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
    ///
    /// `initialize` may not be cancelled on the wire, so cancellation stops
    /// the local wait and the connection is dropped by the caller instead.
    /// The `initialized` notification is awaited to delivery: the next
    /// request may not overtake it.
    async fn handshake(
        &self,
        connection: &Connection,
        cancel: &CancellationToken,
    ) -> Result<Handshake> {
        let params = json!({
            "protocolVersion": LATEST_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "gritt",
                "title": "Gritt",
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        let result = connection
            .request_uncancellable(
                jsonrpc::method::INITIALIZE,
                params,
                self.settings.init_timeout,
                cancel,
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
            .notify_delivered(
                jsonrpc::method::INITIALIZED,
                json!({}),
                self.settings.init_timeout,
            )
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
    /// is simply a server with no tools, not a failure. Every discovered
    /// definition is redacted before it can reach a schema or a prompt.
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
        let secrets = { self.state.lock().await.secrets.clone() };
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
                .await
                .map_err(|error| redact_error(error, &secrets))?;
            if let Some(items) = page.get("tools").and_then(Value::as_array) {
                tools.extend(
                    items
                        .iter()
                        .map(|tool| redact_value(tool.clone(), &secrets)),
                );
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

/// Every credential value the configured servers are given.
///
/// A server can echo its own token back in an error, its metadata, a schema,
/// or a tool result, so the runtime has to recognize those values wherever
/// they surface. The set covers the whole workspace rather than one server:
/// the cost is a string scan, and the alternative is deciding, per value,
/// which server it came from.
fn configured_secrets(config: &McpConfig) -> Vec<Secret> {
    let mut secrets = Vec::new();
    for entry in &config.entries {
        let McpEntry::Server(server) = entry else {
            continue;
        };
        let values: Vec<(&String, &String)> = match &server.transport {
            McpTransport::Stdio { env, .. } => env.iter().collect(),
            McpTransport::Http { headers, .. } => headers.iter().collect(),
        };
        for (name, value) in values {
            if value.is_empty() {
                continue;
            }
            let credential = is_secret_env_name(name, &[])
                || matches!(
                    name.to_ascii_lowercase().as_str(),
                    "authorization" | "proxy-authorization" | "cookie" | "x-auth-token"
                );
            if credential {
                secrets.push(Secret::new(value.clone()));
            }
        }
    }
    secrets
}

/// Redacts an error's message and diagnostic. Server text reaches errors
/// through RPC error bodies, which a server controls completely.
fn redact_error(error: Error, secrets: &[Secret]) -> Error {
    if secrets.is_empty() {
        return error;
    }
    let mut redacted = Error::new(error.kind, redact_text(&error.message, secrets));
    if let Some(diagnostic) = error.diagnostic {
        redacted = redacted.with_diagnostic(redact_value(diagnostic, secrets));
    }
    redacted
}

/// Closes connections together rather than one after another, so shutting a
/// workspace down costs one grace period rather than one per server.
async fn close_all(connections: Vec<Arc<Connection>>) {
    futures::future::join_all(connections.iter().map(|connection| connection.shutdown())).await;
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

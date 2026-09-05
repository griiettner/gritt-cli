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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
use tokio::sync::{broadcast, Mutex};

use self::connection::Connection;
use self::registry::{render_result, RenderedResult, ToolRegistry};
use self::trust::{MemoryTrustStore, TrustStore};
use crate::CancellationToken;

pub use self::registry::{dispatch_name, is_dispatch_name, RegisteredTool};

/// The file every workspace is read from. Gritt never writes it.
pub const CONFIG_FILE: &str = ".mcp.json";

/// How many lifecycle messages a slow subscriber may fall behind before the
/// oldest are dropped. Every message carries the full state, so a lag costs
/// intermediate frames and never correctness.
const LIFECYCLE_QUEUE: usize = 32;

/// How long shutdown waits for in-flight initializations to release their
/// connections after they have been closed. Only a future that never returns
/// should reach it.
const LAUNCH_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// The credentials this entry's *running* connection was launched with.
    ///
    /// Not the ones the current file names. A reload that only rotates
    /// `${TOKEN}` leaves the raw definition, and therefore the fingerprint,
    /// unchanged, so the process keeps running with the old value; redacting
    /// against the new one would let the old one through. These retire with
    /// the connection.
    secrets: Vec<Secret>,
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
    /// Launches that are under way but not installed against an entry yet.
    ///
    /// A slot is reserved before anything is spawned and released only once
    /// the connection is installed or its cleanup has finished, so there is
    /// no instant in which a live child belongs to nobody. The value is
    /// `None` between reserving the slot and the process existing.
    launching: HashMap<u64, Option<Arc<Connection>>>,
    /// Set once shutdown has begun, so a launch that starts afterwards is
    /// closed instead of installed.
    closing: bool,
}

pub struct McpRuntime {
    workspace: PathBuf,
    settings: McpRuntimeSettings,
    http: Option<Arc<dyn HttpTransport>>,
    trust: Arc<dyn TrustStore>,
    state: Mutex<RuntimeState>,
    generations: AtomicU64,
    launches: AtomicU64,
    /// Serializes first-use decisions. Two decisions racing each other could
    /// otherwise persist in one order and apply in the other, leaving the
    /// stored decision disagreeing with the live one.
    decisions: Mutex<()>,
    /// Held between creating a server process and registering it against its
    /// reserved slot.
    ///
    /// Unset in production, where that window is a few microseconds and
    /// cannot be observed reliably. A test sets it to prove shutdown waits
    /// for a launch that has a child but has not attached it yet.
    launch_gate: Option<Arc<tokio::sync::Semaphore>>,
    /// Whether the workspace file has been read and its approved servers
    /// started. A session that begins on a connector never opens the
    /// runtime; entering a native session later does.
    opened: AtomicBool,
    /// Lifecycle delivery. Every state change publishes the whole snapshot
    /// list, so an interface never polls and a subscriber that missed a
    /// message still converges on the current truth (TKT-0019).
    lifecycle: broadcast::Sender<Vec<McpServerSnapshot>>,
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
            launches: AtomicU64::new(1),
            decisions: Mutex::new(()),
            launch_gate: None,
            opened: AtomicBool::new(false),
            lifecycle: broadcast::channel(LIFECYCLE_QUEUE).0,
        }
    }

    /// Subscribes to lifecycle changes. Each message is the complete
    /// snapshot list, which is why a lagging receiver can be resynchronised
    /// by the next message instead of replaying a history.
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<McpServerSnapshot>> {
        self.lifecycle.subscribe()
    }

    /// Publishes the current snapshots to every subscriber. A send with no
    /// subscribers is not an error.
    async fn publish(&self) {
        let snapshots = self.snapshots().await;
        let _ = self.lifecycle.send(snapshots);
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

    /// Holds every launch between spawning its process and registering it.
    ///
    /// Only a test sets this, to keep that window open long enough to observe.
    /// Each launch acquires one permit, so a semaphore with no permits freezes
    /// them there until the test adds some.
    pub fn with_launch_gate(mut self, gate: Arc<tokio::sync::Semaphore>) -> Self {
        self.launch_gate = Some(gate);
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
                                secrets: Vec::new(),
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
                                // A connection is only installed with the
                                // credentials it was launched with; this is
                                // the value for the launch that has not
                                // happened yet.
                                secrets: entry_secrets(server),
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
        self.publish().await;
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
        // One decision at a time. Without this, two decisions can persist in
        // one order and apply in the other, and the store ends up disagreeing
        // with the state the user sees.
        let _decisions = self.decisions.lock().await;
        let (fingerprint, observed) = {
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
            (entry.fingerprint.clone(), entry.generation)
        };
        // Checked before the write, not after it. A decision that is already
        // stale must not reach the store at all: returning an error after
        // persisting would leave an approval recorded for a definition the
        // user has since denied.
        {
            let state = self.state.lock().await;
            let current = state.servers.get(server);
            if current.map(|entry| (&entry.fingerprint, entry.generation))
                != Some((&fingerprint, observed))
            {
                return Err(Error::config(format!(
                    "`{server}` changed while the decision was being made; review it again"
                )));
            }
        }
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
            // approve something the user never saw. The generation covers
            // what the fingerprint cannot: a concurrent denial, stop, or
            // restart keeps the same raw definition.
            if entry.fingerprint != fingerprint || entry.generation != observed {
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
        self.publish().await;
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
                let outcome = self.connect_inner(&config, cancel).await;
                (name, generation, outcome)
            }))
            .buffer_unordered(limit)
            .collect()
            .await;
        let mut stale: Vec<(u64, Arc<Connection>)> = Vec::new();
        {
            let mut state = self.state.lock().await;
            for (name, generation, outcome) in outcomes {
                let current = state.servers.get(&name).map(|entry| entry.generation);
                if current != Some(generation) || state.closing {
                    // Reloaded, denied, stopped, or shut down while this was
                    // connecting. The result belongs to a definition that is
                    // no longer configured under this name, or to a runtime
                    // that is going away. It keeps its launch slot until the
                    // close below finishes, so shutdown cannot see an empty
                    // map while the child is still going away.
                    if let Ok(established) = outcome {
                        stale.push((established.launch, established.connection));
                    }
                    continue;
                }
                let Some(entry) = state.servers.get_mut(&name) else {
                    continue;
                };
                // The credentials this launch actually used, which stay with
                // the connection until it retires.
                let secrets = established_secrets(entry);
                match outcome {
                    Ok(established) => {
                        entry.secrets = secrets.clone();
                        entry.state = McpServerState::Ready;
                        entry.connection = Some(established.connection);
                        entry.pid = established.pid;
                        entry.protocol_version = Some(established.protocol_version);
                        entry.server_version = established
                            .server_version
                            .map(|version| redact_text(&version, &secrets));
                        // Installed: the entry owns the connection from here,
                        // so the launch slot is released in the same critical
                        // section that took ownership of it.
                        state.launching.remove(&established.launch);
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
        // Closed first, released second: the slot is what keeps shutdown
        // waiting for these.
        let (launches, connections): (Vec<u64>, Vec<Arc<Connection>>) = stale.into_iter().unzip();
        close_all(connections).await;
        for launch in launches {
            self.finish_launch(launch).await;
        }
        self.publish().await;
        self.snapshots().await
    }

    /// Loads the workspace file and starts every already-approved server.
    pub async fn open(&self, cancel: &CancellationToken) -> Result<Vec<McpServerSnapshot>> {
        self.opened.store(true, Ordering::SeqCst);
        let config = self.read_config()?;
        self.load(&config).await?;
        Ok(self.start(cancel).await)
    }

    /// Opens the runtime the first time it is needed and does nothing after.
    ///
    /// A run that starts on an external agent never opens it: that agent owns
    /// its own MCP clients. Switching to a native session later, through a
    /// resume, is the first moment Gritt needs its own servers, and this is
    /// what starts them then rather than leaving the session without tools.
    pub async fn ensure_open(&self, cancel: &CancellationToken) -> Result<Vec<McpServerSnapshot>> {
        if self.opened.swap(true, Ordering::SeqCst) {
            return Ok(self.snapshots().await);
        }
        let config = self.read_config()?;
        self.load(&config).await?;
        Ok(self.start(cancel).await)
    }

    /// True once the workspace file has been read and its servers started.
    pub fn is_open(&self) -> bool {
        self.opened.load(Ordering::SeqCst)
    }

    /// Stops one server and connects it again.
    ///
    /// Restarting is not a way around approval: the current definition must
    /// be trusted, so an entry that is awaiting approval or denied is put back
    /// into that state instead of being launched.
    pub async fn restart(&self, server: &str, cancel: &CancellationToken) -> Result<()> {
        let (fingerprint, observed) = {
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
            (entry.fingerprint.clone(), entry.generation)
        };
        // Reading the decision can take as long as the store needs. A denial
        // or a stop that lands while it is in flight keeps the fingerprint,
        // so only the generation can tell that this answer is stale.
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
            if entry.fingerprint != fingerprint || entry.generation != observed {
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
        self.publish().await;
        Ok(())
    }

    /// Ends every connection. Stdio children get the specified sequence:
    /// stdin closed, a bounded grace period, then termination and a kill,
    /// covering the process group so a descendant cannot outlive its parent.
    /// Gritt owns every process it launched, so none may outlive this.
    pub async fn shutdown(&self) {
        let connections: Vec<Arc<Connection>> = {
            let mut state = self.state.lock().await;
            // No launch started after this point is installed; it is closed
            // by whoever started it.
            state.closing = true;
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
            // Children whose handshake never finished. Closing these is what
            // stops a stalled initialization from outliving the runtime, and
            // it also unblocks the future waiting on that handshake.
            connections.extend(state.launching.values().flatten().cloned());
            state.registry = ToolRegistry::new();
            connections
        };
        close_all(connections).await;
        // The launches release themselves once their handshake fails, which
        // closing the connection above guarantees. Waiting for that is what
        // makes shutdown mean "every child is gone", not "every installed
        // child is gone".
        let deadline = std::time::Instant::now() + LAUNCH_DRAIN_TIMEOUT;
        loop {
            let (reserved, attached) = {
                let state = self.state.lock().await;
                (
                    state.launching.len(),
                    state
                        .launching
                        .values()
                        .flatten()
                        .cloned()
                        .collect::<Vec<Arc<Connection>>>(),
                )
            };
            // The count of *slots* is the condition, not the count of
            // connections. A slot still holding `None` is a launch that has
            // reserved its place and may be inside spawning right now: its
            // child exists, or is about to, and waiting for the slot to
            // clear is the only way to know which. Leaving on an empty
            // connection list would let that child outlive shutdown, and the
            // signal handler exits the process immediately afterwards.
            if reserved == 0 || std::time::Instant::now() >= deadline {
                // The bound only guards against a future that never returns.
                break;
            }
            close_all(attached).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.publish().await;
    }

    /// Lets a runtime be used again after a shutdown, which the tests need
    /// and a future reconnect action would too.
    pub async fn reopen(&self) {
        self.state.lock().await.closing = false;
    }

    /// Applies pending `tools/list_changed` notifications and notices
    /// connections that died. Called between turns, never during one, so the
    /// tool set a model was shown stays stable for that turn.
    pub async fn refresh(&self, cancel: &CancellationToken) -> Vec<McpServerSnapshot> {
        let candidates: Vec<(String, u64, Arc<Connection>, Vec<Secret>)> = {
            let state = self.state.lock().await;
            state
                .servers
                .values()
                .filter(|entry| entry.state.is_ready())
                .filter_map(|entry| {
                    entry.connection.clone().map(|connection| {
                        (
                            entry.name.clone(),
                            entry.generation,
                            connection,
                            entry.secrets.clone(),
                        )
                    })
                })
                .collect()
        };
        for (name, generation, connection, secrets) in candidates {
            if connection.is_closed() {
                self.mark_lost(&name, generation, "the server closed the connection")
                    .await;
                continue;
            }
            if !connection.take_tools_changed() {
                continue;
            }
            match self.discover(&connection, None, &secrets, cancel).await {
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
        self.publish().await;
        self.snapshots().await
    }

    /// Records that a connection is gone, unless the entry has moved on.
    async fn mark_lost(&self, server: &str, generation: u64, reason: &str) {
        let mut state = self.state.lock().await;
        if state.servers.get(server).map(|entry| entry.generation) != Some(generation) {
            return;
        }
        if let Some(entry) = state.servers.get_mut(server) {
            let secrets = entry.secrets.clone();
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
            (connection, entry.generation, entry.secrets.clone())
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

    async fn connect_inner(
        &self,
        config: &McpServerConfig,
        cancel: &CancellationToken,
    ) -> std::result::Result<Established, String> {
        // The slot is claimed before anything is spawned. A launch that
        // starts during shutdown is refused here, so it never becomes a child
        // that nobody owns, and one that starts just before it is already
        // registered by the time the process exists.
        let Some(launch) = self.reserve_launch().await else {
            return Err("the runtime is shutting down".to_owned());
        };
        let spawned = match &config.transport {
            McpTransport::Stdio { command, args, env } => stdio::launch(
                &self.workspace,
                command,
                args,
                env,
                self.settings.shutdown_grace,
            )
            .map(|started| {
                let stderr = Arc::clone(&started.stderr);
                (Arc::new(started.connection), started.pid, Some(stderr))
            })
            .map_err(|error| error.message),
            McpTransport::Http { url, headers } => match self.http.clone() {
                Some(transport) => {
                    Ok((Arc::new(http::connect(transport, url, headers)), None, None))
                }
                None => Err("this Gritt build has no HTTP transport for MCP endpoints".to_owned()),
            },
        };
        let (connection, pid, stderr) = match spawned {
            Ok(spawned) => spawned,
            Err(reason) => {
                // Nothing was started, so the slot is released at once.
                self.finish_launch(launch).await;
                return Err(reason);
            }
        };
        if let Some(gate) = &self.launch_gate {
            // The child exists but its slot still holds `None`. Shutdown has
            // to treat that as work in progress.
            if let Ok(permit) = gate.acquire().await {
                permit.forget();
            }
        }
        self.attach_launch(launch, &connection).await;
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
        // The overall deadline wraps the negotiation only. Wrapping the whole
        // function would drop it while it still owned the connection, and
        // nothing would then close that child or release its registration.
        let outcome = match tokio::time::timeout(
            self.settings.init_timeout,
            self.negotiate(&connection, config, cancel),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(Error::config(format!(
                "the server was not ready within {}",
                connection::human(self.settings.init_timeout)
            ))),
        };
        let handshake = match outcome {
            Ok(handshake) => handshake,
            Err(error) => {
                // Closed and released together, so a failed launch leaves
                // nothing behind for shutdown to find.
                connection.shutdown().await;
                self.finish_launch(launch).await;
                return Err(tail(error.message));
            }
        };
        // The launch registration is deliberately not released here. It
        // travels with the result and is dropped by `start`, under the lock
        // that installs the connection.
        Ok(Established {
            launch,
            connection,
            pid,
            protocol_version: handshake.protocol_version,
            server_version: handshake.server_version,
            tools: handshake.tools,
        })
    }

    /// The handshake and the first discovery, as one unit, so the caller can
    /// bracket them with the launch registration.
    async fn negotiate(
        &self,
        connection: &Connection,
        config: &McpServerConfig,
        cancel: &CancellationToken,
    ) -> Result<Negotiated> {
        let handshake = self.handshake(connection, cancel).await?;
        let tools = self
            .discover(
                connection,
                Some(&handshake.capabilities),
                &entry_secrets(config),
                cancel,
            )
            .await?;
        Ok(Negotiated {
            protocol_version: handshake.protocol_version,
            server_version: handshake.server_version,
            tools,
        })
    }

    /// Records a connection that exists but is not installed yet. Returns
    /// false when shutdown has already begun, in which case the caller closes
    /// it instead.
    /// Claims a launch slot before anything is spawned.
    ///
    /// `None` means shutdown has begun, so nothing should be started at all.
    /// Reserving first is what keeps a rejected launch from spawning a child
    /// that shutdown cannot see.
    async fn reserve_launch(&self) -> Option<u64> {
        let mut state = self.state.lock().await;
        if state.closing {
            return None;
        }
        let launch = self.launches.fetch_add(1, Ordering::SeqCst);
        state.launching.insert(launch, None);
        Some(launch)
    }

    /// Puts the connection into its reserved slot, so shutdown can close it.
    async fn attach_launch(&self, launch: u64, connection: &Arc<Connection>) {
        let mut state = self.state.lock().await;
        if let Some(slot) = state.launching.get_mut(&launch) {
            *slot = Some(Arc::clone(connection));
        }
    }

    /// Releases a launch slot.
    ///
    /// Only called once the connection is installed against an entry or its
    /// cleanup has completed; see [`Established::launch`].
    async fn finish_launch(&self, launch: u64) {
        self.state.lock().await.launching.remove(&launch);
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
        secrets: &[Secret],
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
                .await
                .map_err(|error| redact_error(error, secrets))?;
            if let Some(items) = page.get("tools").and_then(Value::as_array) {
                tools.extend(items.iter().map(|tool| redact_value(tool.clone(), secrets)));
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

/// The credential values one server definition is given.
///
/// A server can echo its own token back in an error, its metadata, a schema,
/// or a tool result, so the runtime has to recognize the values it handed
/// that server. Keeping this per definition rather than per file is what
/// makes a rotation safe: the running process still holds the old value.
fn entry_secrets(server: &McpServerConfig) -> Vec<Secret> {
    let values: Vec<(&String, &String)> = match &server.transport {
        McpTransport::Stdio { env, .. } => env.iter().collect(),
        McpTransport::Http { headers, .. } => headers.iter().collect(),
    };
    values
        .into_iter()
        .filter(|(name, value)| {
            !value.is_empty()
                && (is_secret_env_name(name, &[])
                    || matches!(
                        name.to_ascii_lowercase().as_str(),
                        "authorization" | "proxy-authorization" | "cookie" | "x-auth-token"
                    ))
        })
        .flat_map(|(_, value)| credential_variants(value))
        .collect()
}

/// The forms of one credential a server could echo back.
///
/// Redaction is exact-string replacement, so registering only the configured
/// value misses the part that actually is the secret. `Authorization` is
/// commonly set to a whole `Bearer <token>` string, and a server that logs
/// just the token would otherwise leak it. Both the complete value and the
/// part after an authentication scheme are registered.
fn credential_variants(value: &str) -> Vec<Secret> {
    let mut variants = vec![Secret::new(value.to_owned())];
    if let Some((scheme, rest)) = value.split_once(char::is_whitespace) {
        let rest = rest.trim();
        // A scheme is a short word; the remainder is the credential. The
        // length floor keeps a stray short word from being redacted out of
        // every message.
        let looks_like_scheme = !scheme.is_empty()
            && scheme.len() <= 16
            && scheme.chars().all(|c| c.is_ascii_alphabetic());
        if looks_like_scheme && rest.len() >= 4 && !rest.contains(char::is_whitespace) {
            variants.push(Secret::new(rest.to_owned()));
        }
    }
    variants
}

/// The credentials a freshly installed connection should carry.
///
/// `load` already put the current definition's values on the entry, and the
/// launch used exactly that definition, so this is that value. It exists as
/// its own function so the rule has one place to live.
fn established_secrets(entry: &ServerRuntime) -> Vec<Secret> {
    entry.config.as_ref().map(entry_secrets).unwrap_or_default()
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

/// A completed handshake plus the first tool listing.
struct Negotiated {
    protocol_version: String,
    server_version: Option<String>,
    tools: Vec<Value>,
}

struct Established {
    /// The registration this connection still holds. It is released only
    /// under the same lock that installs or closes the connection, so there
    /// is no window in which shutdown can see neither.
    launch: u64,
    connection: Arc<Connection>,
    pid: Option<u32>,
    protocol_version: String,
    server_version: Option<String>,
    tools: Vec<Value>,
}

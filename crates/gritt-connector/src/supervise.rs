//! The generic external connector: one [`Protocol`] describes how an agent
//! is launched and how its output maps to events; this module owns the
//! process lifecycle, timeouts, cancellation, session state, and the
//! `Connector` contract on top of it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::Utc;
use futures::Stream;
use gritt_core::config::ConnectorSettings;
use gritt_core::connector::{
    AuthState, Connector, ConnectorCapabilities, ConnectorId, ConnectorInfo, ConnectorInspection,
    TaskRequest, TaskState, Transport,
};
use gritt_core::event::{
    ApprovalDecision, ApprovalId, Event, EventKind, EventSource, SessionStatus, StopReason,
};
use gritt_core::provider::EventStream;
use gritt_core::secret::Secret;
use gritt_core::session::{BoxFuture, ContinuationState, SessionId};
use gritt_core::{Error, ErrorKind, Result};
use tokio::sync::{mpsc, Notify};

use crate::health::{find_executable, probe, version_at_least, version_token, ProbeOutput};
use crate::process::{self, Launch, Line, Supervised};
use crate::redact::{cap, redact_text, redact_value};

/// Longest raw line kept in a diagnostic.
pub const MAX_RAW_BYTES: usize = 2048;
/// Stderr lines kept for the exit diagnostic.
const STDERR_TAIL: usize = 20;
/// Time an agent gets to flush trailing output and exit after its
/// terminal event, capped by the idle timeout.
const WRAP_UP: Duration = Duration::from_secs(10);

/// The launch arguments as a diagnostic: the prompt is user content and
/// any `name=value` argument may carry a value, so neither is recorded
/// raw. The connector's own secrets are redacted on top of this.
pub fn diagnostic_args(args: &[String], prompt: &str) -> Vec<String> {
    args.iter()
        .map(|arg| {
            if arg == prompt {
                "[prompt]".to_owned()
            } else if let Some((name, _)) = arg.split_once('=') {
                format!("{name}=[redacted]")
            } else {
                arg.clone()
            }
        })
        .collect()
}

/// What the agent's own terminal event said, kept so the process exit can
/// be judged against it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalKind {
    Completed,
    Error(String),
    Cancelled,
}

/// One normalized event with its raw diagnostic detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub kind: EventKind,
    pub diagnostic: Option<serde_json::Value>,
}

impl Normalized {
    pub fn new(kind: EventKind) -> Self {
        Self {
            kind,
            diagnostic: None,
        }
    }

    pub fn with(kind: EventKind, diagnostic: serde_json::Value) -> Self {
        Self {
            kind,
            diagnostic: Some(diagnostic),
        }
    }

    /// The event an unknown wire message becomes: a streaming status with
    /// the raw message in the diagnostic. Never fatal.
    pub fn unknown(raw: &serde_json::Value) -> Self {
        Self::with(
            EventKind::StatusChanged {
                status: SessionStatus::Streaming,
            },
            serde_json::json!({ "unknown_event": raw }),
        )
    }
}

/// Maps one agent's wire messages to events. Stateful: tool calls need
/// their names when the result arrives, and the external id shows up on
/// the first message.
pub trait Normalizer: Send {
    /// One parsed JSON line.
    fn message(&mut self, value: serde_json::Value) -> Vec<Normalized>;
    /// The agent's own session or thread identifier, once seen.
    fn external_id(&self) -> Option<String>;
    /// True once a completion, error, or cancellation was produced.
    fn terminal_seen(&self) -> bool;
}

/// How one installed agent is launched and read.
pub trait Protocol: Send + Sync + 'static {
    fn id(&self) -> ConnectorId;
    /// The executable name looked up on `PATH` unless configured.
    fn executable(&self) -> &'static str;
    fn capabilities(&self) -> ConnectorCapabilities;
    fn minimum_version(&self) -> Option<&'static str> {
        None
    }
    fn version_args(&self) -> Vec<String> {
        vec!["--version".into()]
    }
    /// Arguments of the auth probe, or `None` when the agent has no
    /// documented way to ask.
    fn auth_probe_args(&self) -> Option<Vec<String>>;
    fn auth_state(&self, probe: &ProbeOutput) -> AuthState;
    /// Arguments for one task. `external_id` resumes the agent's own
    /// thread when it is known and the agent supports it.
    fn task_args(&self, request: &TaskRequest, external_id: Option<&str>) -> Vec<String>;
    fn normalizer(&self) -> Box<dyn Normalizer>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub health: Duration,
    /// Time allowed before the first output line.
    pub startup: Duration,
    /// Time allowed between output lines.
    pub idle: Duration,
}

impl Timeouts {
    pub fn from_settings(settings: &ConnectorSettings) -> Self {
        Self {
            health: Duration::from_secs(settings.health_check_timeout_secs.unwrap_or(15)),
            startup: Duration::from_secs(settings.task_timeout_secs.unwrap_or(120)),
            idle: Duration::from_secs(settings.task_timeout_secs.unwrap_or(600)),
        }
    }
}

/// A cancellation flag other tasks can await.
#[derive(Default)]
pub struct CancelFlag {
    set: AtomicBool,
    notify: Notify,
}

impl CancelFlag {
    pub fn cancel(&self) {
        self.set.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.set.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
struct SessionState {
    external_id: Option<String>,
    state: TaskState,
    cancel: Arc<CancelFlag>,
    pending_input: Option<String>,
    last_error: Option<String>,
    pid: Option<u32>,
    workspace: PathBuf,
}

type Sessions = Arc<Mutex<HashMap<SessionId, SessionState>>>;

/// Events from a driver task. Dropping it kills the agent.
pub struct EventReceiver {
    rx: mpsc::Receiver<Result<Event>>,
}

impl Stream for EventReceiver {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

/// An installed agent behind the `Connector` contract.
pub struct ExternalConnector<P: Protocol> {
    protocol: P,
    program: Option<PathBuf>,
    transport: Transport,
    extra_args: Vec<String>,
    timeouts: Timeouts,
    secrets: Vec<Secret>,
    sessions: Sessions,
}

impl<P: Protocol> ExternalConnector<P> {
    pub fn new(protocol: P, settings: &ConnectorSettings) -> Self {
        let id = protocol.id();
        // Settings are keyed by any spelling `parse_connector_id` accepts.
        let matches = |key: &String| crate::parse_connector_id(key) == Some(id);
        let configured = settings
            .executables
            .iter()
            .find(|(key, _)| matches(key))
            .map(|(_, path)| path.clone());
        let program = match configured {
            Some(path) => find_executable(&path),
            None => find_executable(protocol.executable()),
        };
        let transport = if settings.pty.iter().any(matches) {
            Transport::Pty
        } else {
            Transport::MachineReadable
        };
        let extra_args = settings
            .extra_args
            .iter()
            .find(|(key, _)| matches(key))
            .map(|(_, args)| args.clone())
            .unwrap_or_default();
        Self {
            protocol,
            program,
            transport,
            extra_args,
            timeouts: Timeouts::from_settings(settings),
            secrets: Vec::new(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers key values to redact out of every event and diagnostic.
    pub fn with_secrets(mut self, secrets: Vec<Secret>) -> Self {
        self.secrets = secrets;
        self
    }

    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    pub fn program(&self) -> Option<&PathBuf> {
        self.program.as_ref()
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    fn owner(&self) -> String {
        format!("connector:{}", self.protocol.id().as_str())
    }

    fn continuation_external_id(&self, state: Option<&ContinuationState>) -> Option<String> {
        let state = state?;
        if state.owner != self.owner() {
            return None;
        }
        state
            .state
            .get("external_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    /// The continuation state a session should store after a turn, when
    /// the agent reported an identifier that can resume it.
    pub fn continuation_for(&self, session_id: &SessionId) -> Option<ContinuationState> {
        let sessions = self.sessions.lock().expect("sessions");
        let external_id = sessions.get(session_id)?.external_id.clone()?;
        Some(ContinuationState {
            owner: self.owner(),
            state: serde_json::json!({ "external_id": external_id }),
        })
    }

    fn not_installed(&self) -> Error {
        Error::connector(format!(
            "{} is not installed: `{}` was not found on PATH and no executable is configured",
            self.protocol.id().as_str(),
            self.protocol.executable()
        ))
    }

    fn launch(&self, request: &TaskRequest, external_id: Option<&str>) -> Result<Launch> {
        let program = self.program.clone().ok_or_else(|| self.not_installed())?;
        let mut args = self.protocol.task_args(request, external_id);
        args.extend(self.extra_args.iter().cloned());
        Ok(Launch {
            program,
            args,
            cwd: request.workspace.clone(),
            // The agent keeps its own credentials (ADR-010); nothing is
            // removed from its environment here.
            env_remove: Vec::new(),
            transport: self.transport,
        })
    }

    fn session_state(&self, session_id: &SessionId) -> Option<SessionState> {
        self.sessions
            .lock()
            .expect("sessions")
            .get(session_id)
            .cloned()
    }

    async fn run(
        &self,
        request: TaskRequest,
        external_id: Option<String>,
    ) -> Result<EventStream<'_>> {
        let launch = self.launch(&request, external_id.as_deref())?;
        let cancel = Arc::new(CancelFlag::default());
        {
            let mut sessions = self.sessions.lock().expect("sessions");
            let state = sessions
                .entry(request.session_id.clone())
                .or_insert_with(|| SessionState {
                    external_id: None,
                    state: TaskState::Idle,
                    cancel: Arc::clone(&cancel),
                    pending_input: None,
                    last_error: None,
                    pid: None,
                    workspace: request.workspace.clone(),
                });
            state.cancel = Arc::clone(&cancel);
            state.workspace = request.workspace.clone();
            state.state = TaskState::Running;
            state.last_error = None;
            if external_id.is_some() {
                state.external_id = external_id.clone();
            }
        }
        let supervised = match process::spawn(&launch).await {
            Ok(supervised) => supervised,
            Err(error) => {
                update(
                    &self.sessions,
                    &request.session_id,
                    TaskState::Failed,
                    Some(redact_text(&error.message, &self.secrets)),
                    None,
                    None,
                );
                return Err(error);
            }
        };
        let pid = supervised.control.pid();
        update(
            &self.sessions,
            &request.session_id,
            TaskState::Running,
            None,
            None,
            pid,
        );
        let (tx, rx) = mpsc::channel(256);
        let driver = Driver {
            id: self.protocol.id(),
            session_id: request.session_id.clone(),
            normalizer: self.protocol.normalizer(),
            secrets: self.secrets.clone(),
            timeouts: self.timeouts,
            cancel,
            tx,
            sequence: 0,
            terminal: None,
            launch_diagnostic: serde_json::json!({
                "program": launch.program.display().to_string(),
                "args": diagnostic_args(&launch.args, &request.prompt),
                "transport": supervised.transport,
                "external_id": external_id,
                "pid": pid,
            }),
        };
        let sessions = Arc::clone(&self.sessions);
        let session_id = request.session_id.clone();
        tokio::spawn(async move {
            let outcome = driver.drive(supervised).await;
            update(
                &sessions,
                &session_id,
                outcome.state,
                outcome.error,
                outcome.external_id,
                None,
            );
        });
        Ok(Box::pin(EventReceiver { rx }))
    }
}

fn update(
    sessions: &Sessions,
    session_id: &SessionId,
    state: TaskState,
    error: Option<String>,
    external_id: Option<String>,
    pid: Option<u32>,
) {
    let mut sessions = sessions.lock().expect("sessions");
    if let Some(session) = sessions.get_mut(session_id) {
        session.state = state;
        if error.is_some() {
            session.last_error = error;
        }
        if external_id.is_some() {
            session.external_id = external_id;
        }
        if pid.is_some() {
            session.pid = pid;
        }
    }
}

struct DriverOutcome {
    state: TaskState,
    error: Option<String>,
    external_id: Option<String>,
}

/// Runs one agent process to its end, translating lines into events.
struct Driver {
    id: ConnectorId,
    session_id: SessionId,
    normalizer: Box<dyn Normalizer>,
    secrets: Vec<Secret>,
    timeouts: Timeouts,
    cancel: Arc<CancelFlag>,
    tx: mpsc::Sender<Result<Event>>,
    sequence: u64,
    /// The agent's own terminal event, once emitted.
    terminal: Option<TerminalKind>,
    launch_diagnostic: serde_json::Value,
}

impl Driver {
    fn event(&mut self, normalized: Normalized) -> Event {
        let diagnostic = normalized
            .diagnostic
            .map(|value| redact_value(value, &self.secrets));
        let kind = match normalized.kind {
            EventKind::TextDelta { text } => EventKind::TextDelta {
                text: crate::redact::redact_text(&text, &self.secrets),
            },
            EventKind::ReasoningSummary { text } => EventKind::ReasoningSummary {
                text: crate::redact::redact_text(&text, &self.secrets),
            },
            EventKind::ToolCall { mut call } => {
                call.arguments = redact_value(call.arguments, &self.secrets);
                EventKind::ToolCall { call }
            }
            EventKind::ToolResult { mut result } => {
                result.output = crate::redact::redact_text(&result.output, &self.secrets);
                EventKind::ToolResult { result }
            }
            EventKind::Error {
                error_kind,
                message,
            } => EventKind::Error {
                error_kind,
                message: crate::redact::redact_text(&message, &self.secrets),
            },
            other => other,
        };
        match &kind {
            EventKind::Completed { .. } => self.terminal = Some(TerminalKind::Completed),
            EventKind::Error { message, .. } => {
                self.terminal = Some(TerminalKind::Error(message.clone()));
            }
            EventKind::Cancelled => self.terminal = Some(TerminalKind::Cancelled),
            _ => {}
        }
        let event = Event {
            session_id: self.session_id.clone(),
            sequence: self.sequence,
            source: EventSource::Connector { id: self.id },
            timestamp: Utc::now(),
            kind,
            diagnostic,
        };
        self.sequence += 1;
        event
    }

    /// Sends one event; `false` when the consumer dropped the stream.
    async fn emit(&mut self, normalized: Normalized) -> bool {
        let event = self.event(normalized);
        self.tx.send(Ok(event)).await.is_ok()
    }

    async fn drive(mut self, mut supervised: Supervised) -> DriverOutcome {
        let mut outcome = DriverOutcome {
            state: TaskState::Running,
            error: None,
            external_id: None,
        };
        let launch_diagnostic = self.launch_diagnostic.clone();
        if !self
            .emit(Normalized::with(
                EventKind::StatusChanged {
                    status: SessionStatus::Connecting,
                },
                launch_diagnostic,
            ))
            .await
        {
            supervised.control.kill().await;
            outcome.state = TaskState::Cancelled;
            return outcome;
        }
        let mut stderr_tail: Vec<String> = Vec::new();
        let mut malformed = 0u64;
        let mut first_line = true;
        let mut consumer_gone = false;
        loop {
            let limit = if self.normalizer.terminal_seen() {
                WRAP_UP.min(self.timeouts.idle)
            } else if first_line {
                self.timeouts.startup
            } else {
                self.timeouts.idle
            };
            let next = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => None,
                line = tokio::time::timeout(limit, supervised.lines.recv()) => Some(line),
            };
            let Some(next) = next else {
                supervised.control.kill().await;
                self.emit(Normalized::new(EventKind::Cancelled)).await;
                outcome.state = TaskState::Cancelled;
                break;
            };
            match next {
                Err(_) if self.normalizer.terminal_seen() => {
                    // The agent finished its turn but neither exited nor
                    // closed its output in time: its own verdict stands,
                    // and it is made to go away.
                    supervised.control.kill().await;
                    self.finish_after_terminal(&mut outcome, None, &stderr_tail, malformed, true)
                        .await;
                    break;
                }
                Err(_) => {
                    supervised.control.kill().await;
                    let message = if first_line {
                        format!(
                            "the agent produced no output within {}s of starting",
                            limit.as_secs()
                        )
                    } else {
                        format!("the agent produced no output for {}s", limit.as_secs())
                    };
                    self.emit(Normalized::with(
                        EventKind::Error {
                            error_kind: ErrorKind::Connector,
                            message: message.clone(),
                        },
                        serde_json::json!({ "timeout_secs": limit.as_secs(), "stderr": stderr_tail }),
                    ))
                    .await;
                    outcome.state = TaskState::Failed;
                    outcome.error = Some(message);
                    break;
                }
                Ok(None) => {
                    // Output closed: the process is gone or going.
                    let exit = supervised.control.wait(Duration::from_secs(10)).await;
                    if exit.is_none() {
                        supervised.control.kill().await;
                    }
                    if self.normalizer.terminal_seen() {
                        self.finish_after_terminal(
                            &mut outcome,
                            exit,
                            &stderr_tail,
                            malformed,
                            false,
                        )
                        .await;
                        break;
                    }
                    let diagnostic = serde_json::json!({
                        "exit": exit.map(|e| e.code),
                        "stderr": stderr_tail,
                        "malformed_lines": malformed,
                    });
                    match exit {
                        Some(exit) if exit.success => {
                            self.emit(Normalized::with(
                                EventKind::Completed {
                                    stop_reason: StopReason::Other,
                                },
                                diagnostic,
                            ))
                            .await;
                            outcome.state = TaskState::Completed;
                        }
                        Some(exit) => {
                            let message = format!(
                                "the agent exited with status {} before finishing{}",
                                exit.code
                                    .map(|c| c.to_string())
                                    .unwrap_or_else(|| "unknown".into()),
                                stderr_tail
                                    .last()
                                    .map(|line| format!(": {line}"))
                                    .unwrap_or_default()
                            );
                            self.emit(Normalized::with(
                                EventKind::Error {
                                    error_kind: ErrorKind::Connector,
                                    message: message.clone(),
                                },
                                diagnostic,
                            ))
                            .await;
                            outcome.state = TaskState::Failed;
                            outcome.error = Some(self.redact(&message));
                        }
                        None => {
                            let message = "the agent closed its output but did not exit".to_owned();
                            self.emit(Normalized::with(
                                EventKind::Error {
                                    error_kind: ErrorKind::Connector,
                                    message: message.clone(),
                                },
                                diagnostic,
                            ))
                            .await;
                            outcome.state = TaskState::Failed;
                            outcome.error = Some(message);
                        }
                    }
                    break;
                }
                Ok(Some(Line::Err(text))) => {
                    first_line = false;
                    if stderr_tail.len() == STDERR_TAIL {
                        stderr_tail.remove(0);
                    }
                    stderr_tail.push(cap(&text, MAX_RAW_BYTES));
                }
                Ok(Some(Line::Out(text))) => {
                    first_line = false;
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let value: serde_json::Value = match serde_json::from_str(trimmed) {
                        Ok(value) => value,
                        Err(error) => {
                            malformed += 1;
                            if !self
                                .emit(Normalized::with(
                                    EventKind::StatusChanged {
                                        status: SessionStatus::Streaming,
                                    },
                                    serde_json::json!({
                                        "malformed_line": cap(trimmed, MAX_RAW_BYTES),
                                        "parse_error": error.to_string(),
                                    }),
                                ))
                                .await
                            {
                                consumer_gone = true;
                                break;
                            }
                            continue;
                        }
                    };
                    // Every line is normalized, including anything after
                    // the terminal event: a trailing error must not be
                    // lost behind an earlier completion.
                    let events = self.normalizer.message(value);
                    for normalized in events {
                        if !self.emit(normalized).await {
                            consumer_gone = true;
                            break;
                        }
                    }
                    if consumer_gone {
                        break;
                    }
                }
            }
        }
        if consumer_gone {
            supervised.control.kill().await;
            outcome.state = TaskState::Cancelled;
        }
        outcome.external_id = self.normalizer.external_id();
        outcome
    }

    fn redact(&self, text: &str) -> String {
        redact_text(text, &self.secrets)
    }

    /// Settles the outcome once the agent has spoken its terminal event
    /// and its process has ended (or been ended). An error terminal stays
    /// an error, a cancellation stays a cancellation, and a completion is
    /// downgraded to an error when the process then exited non-zero.
    async fn finish_after_terminal(
        &mut self,
        outcome: &mut DriverOutcome,
        exit: Option<process::ExitOutcome>,
        stderr_tail: &[String],
        malformed: u64,
        killed: bool,
    ) {
        match self.terminal.clone() {
            Some(TerminalKind::Error(message)) => {
                outcome.state = TaskState::Failed;
                outcome.error = Some(message);
            }
            Some(TerminalKind::Cancelled) => {
                outcome.state = TaskState::Cancelled;
            }
            Some(TerminalKind::Completed) | None => match exit {
                Some(exit) if !exit.success => {
                    let message = format!(
                        "the agent exited with status {} after finishing{}",
                        exit.code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "unknown".into()),
                        stderr_tail
                            .last()
                            .map(|line| format!(": {line}"))
                            .unwrap_or_default()
                    );
                    self.emit(Normalized::with(
                        EventKind::Error {
                            error_kind: ErrorKind::Connector,
                            message: message.clone(),
                        },
                        serde_json::json!({
                            "exit": exit.code,
                            "stderr": stderr_tail,
                            "malformed_lines": malformed,
                        }),
                    ))
                    .await;
                    outcome.state = TaskState::Failed;
                    outcome.error = Some(self.redact(&message));
                }
                _ => {
                    if killed {
                        self.emit(Normalized::with(
                            EventKind::StatusChanged {
                                status: SessionStatus::Finished,
                            },
                            serde_json::json!({
                                "note": "the agent did not exit after finishing and was stopped",
                                "stderr": stderr_tail,
                            }),
                        ))
                        .await;
                    }
                    outcome.state = TaskState::Completed;
                }
            },
        }
    }
}

impl<P: Protocol> Connector for ExternalConnector<P> {
    fn id(&self) -> ConnectorId {
        self.protocol.id()
    }

    fn info(&self) -> BoxFuture<'_, Result<ConnectorInfo>> {
        Box::pin(async move {
            let capabilities = self.protocol.capabilities();
            let Some(program) = &self.program else {
                return Ok(ConnectorInfo {
                    id: self.protocol.id(),
                    version: None,
                    transport: self.transport,
                    capabilities,
                    auth: AuthState::NotInstalled,
                });
            };
            let version =
                match probe(program, &self.protocol.version_args(), self.timeouts.health).await {
                    Ok(output) if output.success => {
                        version_token(&output.stdout).or_else(|| version_token(&output.stderr))
                    }
                    _ => None,
                };
            if let (Some(found), Some(minimum)) = (&version, self.protocol.minimum_version()) {
                if version_at_least(found, minimum) == Some(false) {
                    return Err(Error::connector(format!(
                        "{} {found} is older than the supported minimum {minimum}",
                        self.protocol.id().as_str()
                    )));
                }
            }
            let auth = match self.protocol.auth_probe_args() {
                None => AuthState::Unknown,
                Some(args) => match probe(program, &args, self.timeouts.health).await {
                    Ok(output) => self.protocol.auth_state(&output),
                    Err(_) => AuthState::Unknown,
                },
            };
            Ok(ConnectorInfo {
                id: self.protocol.id(),
                version,
                transport: self.transport,
                capabilities,
                auth,
            })
        })
    }

    fn start(&self, request: TaskRequest) -> BoxFuture<'_, Result<EventStream<'_>>> {
        Box::pin(async move {
            let external_id = self
                .continuation_external_id(request.continuation.as_ref())
                .or_else(|| self.session_state(&request.session_id)?.external_id);
            let external_id = if self.protocol.capabilities().resume {
                external_id
            } else {
                None
            };
            self.run(request, external_id).await
        })
    }

    fn send_input(&self, session_id: &SessionId, input: String) -> BoxFuture<'_, Result<()>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let mut sessions = self.sessions.lock().expect("sessions");
            let session = sessions
                .get_mut(&session_id)
                .ok_or_else(|| Error::connector("unknown connector session"))?;
            session.pending_input = Some(input);
            session.state = TaskState::AwaitingInput;
            Ok(())
        })
    }

    fn answer_approval(
        &self,
        _session_id: &SessionId,
        _request_id: ApprovalId,
        _decision: ApprovalDecision,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            Err(Error::connector(format!(
                "{} runs with its own approval policy in headless mode; Gritt cannot answer its prompts",
                self.protocol.id().as_str()
            )))
        })
    }

    fn cancel(&self, session_id: &SessionId) -> BoxFuture<'_, Result<()>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let state = self
                .session_state(&session_id)
                .ok_or_else(|| Error::connector("unknown connector session"))?;
            state.cancel.cancel();
            Ok(())
        })
    }

    fn resume(&self, session_id: &SessionId) -> BoxFuture<'_, Result<EventStream<'_>>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            if !self.protocol.capabilities().resume {
                return Err(Error::connector(format!(
                    "{} does not support resuming a session",
                    self.protocol.id().as_str()
                )));
            }
            let (external_id, input, workspace) = {
                let mut sessions = self.sessions.lock().expect("sessions");
                let session = sessions
                    .get_mut(&session_id)
                    .ok_or_else(|| Error::connector("unknown connector session"))?;
                let input = session.pending_input.take().ok_or_else(|| {
                    Error::connector("no follow-up input queued; call send_input first")
                })?;
                (
                    session.external_id.clone(),
                    input,
                    session.workspace.clone(),
                )
            };
            let request = TaskRequest {
                session_id: session_id.clone(),
                prompt: input,
                workspace,
                continuation: None,
            };
            self.run(request, external_id).await
        })
    }

    fn inspect(&self, session_id: &SessionId) -> BoxFuture<'_, Result<ConnectorInspection>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let state = self
                .session_state(&session_id)
                .ok_or_else(|| Error::connector("unknown connector session"))?;
            let info = self.info().await?;
            Ok(ConnectorInspection {
                session_id,
                external_id: state.external_id.clone(),
                state: state.state,
                version: info.version,
                auth: info.auth,
                capabilities: info.capabilities,
                diagnostic: Some(serde_json::json!({
                    "last_error": state.last_error,
                    "pid": state.pid,
                    "pending_input": state.pending_input.is_some(),
                })),
            })
        })
    }
}

/// Helper for protocols: the usage block most agents report.
pub fn usage(
    input: Option<u64>,
    output: Option<u64>,
    reasoning: Option<u64>,
    cached: Option<u64>,
) -> EventKind {
    EventKind::Usage {
        usage: gritt_core::event::Usage {
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: reasoning,
            cached_input_tokens: cached,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_flag_wakes_waiters() {
        let flag = Arc::new(CancelFlag::default());
        let waiter = Arc::clone(&flag);
        let handle = tokio::spawn(async move { waiter.cancelled().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        flag.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("woken")
            .unwrap();
        assert!(flag.is_cancelled());
    }
}

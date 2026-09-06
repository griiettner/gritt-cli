//! The native agent loop (ADR-007, ADR-009). Drives one provider adapter
//! through a session: stream events, persist them, gate every tool call
//! through the policy engine, ask the interface when the policy says so,
//! execute, submit results, and continue until the turn completes, fails,
//! or is cancelled. Planning turns carry file_read; coding turns carry the
//! native tools.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use futures::StreamExt;
use gritt_core::config::Config;
use gritt_core::event::{
    ApprovalDecision, ApprovalId, ApprovalRequest, Event, EventKind, EventSource, SessionStatus,
    StopReason, Usage,
};
use gritt_core::policy::PolicyOutcome;
use gritt_core::provider::{Message, PromptRequest, ReasoningEffort, RequestOptions, Role};
use gritt_core::secret::Secret;
use gritt_core::session::{
    BoxFuture, ExecutionMode, LastUsedNative, Phase, Session, SessionId, SessionKind, SessionStore,
};
use gritt_core::tool::{ToolCall, ToolResult};
use gritt_core::{Error, ErrorKind, Result};
use gritt_provider::adapter::{redact_text, redact_value, CapabilitySource, KeyProvider};
use gritt_provider::effort::{effort_support, EffortSupport};
use gritt_provider::models::{load_models, ModelCache, ModelCatalog};
use gritt_provider::transport::HttpTransport;
use gritt_provider::{adapter_for, AdapterContext, CancellationToken};

use crate::draft::{DraftError, DraftWarning};
use crate::driver::EffortOutcome;
use crate::mcp::{is_dispatch_name, McpRuntime, McpToolSet};
use crate::policy::{Decision, PolicyEngine};
use crate::startup::{StartupOutcome, StartupRequest};
use crate::store::Store;
use crate::telemetry::Telemetry;
use crate::tools::{NativeTools, ProcessRegistry, Workspace};

/// How approvals are answered when the policy says `ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Ask the interface.
    Ask,
    /// Approve without asking. For scripts that accept the risk.
    ApproveAll,
    /// Explicit unrestricted native tool authority for this run.
    FullAccess,
    /// Deny without asking. The default when no terminal can answer.
    DenyAll,
}

/// What an interface must provide to the loop. Print, REPL, and the
/// full-screen mode all implement it.
pub trait Ui: Send {
    /// Every persisted event, in order.
    fn event(&mut self, event: &Event);
    /// Answer an `ask` outcome. `preview` is the unified diff for a write.
    /// The loop races this against cancellation, so the future must not
    /// block the thread.
    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        decision: &'a Decision,
        preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision>;
    /// The first output failure, when the interface can no longer deliver
    /// events (a closed pipe, for example). The loop stops the turn on it.
    fn output_error(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub status: TurnStatus,
    pub text: String,
    pub usage: Usage,
    pub tool_calls: u64,
    pub error: Option<String>,
}

/// Lets another task stop a running turn: the request, the stream, and any
/// child process.
#[derive(Clone)]
pub struct CancelHandle {
    token: CancellationToken,
    registry: Arc<ProcessRegistry>,
}

impl CancelHandle {
    pub fn new(token: CancellationToken, registry: Arc<ProcessRegistry>) -> Self {
        Self { token, registry }
    }

    pub fn cancel(&self) {
        self.token.cancel();
        let registry = Arc::clone(&self.registry);
        tokio::spawn(async move { registry.kill_all().await });
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

pub struct NativeAgent {
    session: Session,
    adapter: Arc<dyn gritt_core::provider::ProviderAdapter>,
    store: Arc<Store>,
    policy: PolicyEngine,
    tools: NativeTools,
    /// The workspace MCP runtime, when one is configured. Shared with every
    /// other session in the workspace: connections outlive turns.
    mcp: Option<Arc<McpRuntime>>,
    /// The MCP tools this turn may call, taken once when the turn starts so
    /// a `tools/list_changed` arriving mid-turn cannot change the set the
    /// model was shown.
    mcp_tools: McpToolSet,
    telemetry: Arc<Telemetry>,
    cancel: CancellationToken,
    approval: ApprovalMode,
    next_sequence: u64,
    started: bool,
    /// The phase the model was last told about, so a phase change sends a
    /// transition note on the next turn.
    sent_phase: Option<Phase>,
    /// Active credentials, redacted out of every event, tool result, and
    /// content-log row the harness produces.
    secrets: Vec<Secret>,
    labels: BTreeMap<String, String>,
    /// Whether this session was created in this run. Only such a session
    /// records its profile, model, and effort as the workspace's last-used
    /// choices; a resumed one leaves the record alone.
    new_session: bool,
    /// Whether the last-used record is behind the session's choices: set
    /// at creation and again when the effort changes, cleared once a
    /// completed turn has written the record.
    remember_pending: bool,
}

/// Redacts every registered secret out of an event's text, arguments,
/// results, approval fields, error message, and diagnostic.
fn redact_event(mut event: Event, secrets: &[Secret]) -> Event {
    if secrets.is_empty() {
        return event;
    }
    match &mut event.kind {
        EventKind::TextDelta { text } | EventKind::ReasoningSummary { text } => {
            *text = redact_text(text, secrets);
        }
        EventKind::ToolCall { call } => {
            call.arguments = redact_value(call.arguments.take(), secrets);
        }
        EventKind::ToolResult { result } => {
            result.output = redact_text(&result.output, secrets);
        }
        EventKind::ApprovalRequested { request } => {
            request.resource = redact_text(&request.resource, secrets);
            request.reason = redact_text(&request.reason, secrets);
        }
        EventKind::Error { message, .. } => {
            *message = redact_text(message, secrets);
        }
        EventKind::ApprovalDecided { .. }
        | EventKind::Usage { .. }
        | EventKind::StatusChanged { .. }
        | EventKind::Completed { .. }
        | EventKind::Cancelled => {}
    }
    if let Some(diagnostic) = event.diagnostic.take() {
        event.diagnostic = Some(redact_value(diagnostic, secrets));
    }
    event
}

/// The copy of an event that is persisted while content logging is off.
/// An approval request keeps the tool, the resource, and the call id (an
/// identifier, not content); the reason and the destructive diagnostic
/// are shown to the interface and not stored. Every other event is
/// stored as shown.
pub fn persisted_projection(mut event: Event) -> Event {
    if let EventKind::ApprovalRequested { request } = &mut event.kind {
        request.reason = String::new();
        event.diagnostic = None;
    }
    event
}

/// The note prepended to the first user message after a phase change.
pub fn phase_transition_note(phase: Phase) -> &'static str {
    match phase {
        Phase::Planning => {
            "[Phase changed to planning. Use file_read to inspect workspace files and propose a plan. Writes, shell, and MCP calls are disabled.]"
        }
        Phase::Coding => {
            "[Phase changed to coding. The tools file_read, file_write, and shell are now available. \
             Paths are relative to the workspace root. Follow the current execution mode.]"
        }
    }
}

impl NativeAgent {
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn phase(&self) -> Phase {
        self.session.phase
    }

    /// The effort the next turn sends. `Auto` for a session stored before
    /// effort existed.
    pub fn effort(&self) -> ReasoningEffort {
        self.session.kind.effort().unwrap_or_default()
    }

    /// The profile and model this session is pinned to.
    pub fn profile_and_model(&self) -> (&str, &str) {
        match &self.session.kind {
            SessionKind::Native {
                provider_profile,
                model,
                ..
            } => (provider_profile, model),
            SessionKind::Connector { .. } => ("", ""),
        }
    }

    /// Changes the effort for later turns and persists it with the
    /// session. A level the adapter cannot send for this model is refused
    /// with the typed reason and nothing is stored; the transcript records
    /// an applied change as a status event.
    pub async fn set_effort(&mut self, effort: ReasoningEffort) -> Result<EffortOutcome> {
        let (_, model) = self.profile_and_model();
        let model = model.to_owned();
        let capabilities = self.adapter.capabilities(&model).await?;
        if let EffortSupport::Unsupported(reason) =
            effort_support(self.adapter.protocol(), Some(&capabilities), effort)
        {
            return Ok(EffortOutcome::Unsupported { effort, reason });
        }
        if self.effort() == effort {
            return Ok(EffortOutcome::Applied { effort });
        }
        self.store
            .set_native_effort(&self.session.id, effort)
            .await?;
        if let SessionKind::Native {
            effort: current, ..
        } = &mut self.session.kind
        {
            *current = effort;
        }
        self.remember_pending = self.new_session;
        let event = self.harness_event(
            EventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
            Some(serde_json::json!({ "effort": effort })),
        );
        self.store.append_events(vec![event]).await?;
        Ok(EffortOutcome::Applied { effort })
    }

    pub fn approval_mode(&self) -> ApprovalMode {
        self.approval
    }

    pub fn set_approval_mode(&mut self, mode: ApprovalMode) {
        self.approval = mode;
        self.tools
            .set_full_access(mode == ApprovalMode::FullAccess && self.phase() == Phase::Coding);
        self.sent_phase = None;
    }

    pub fn mode(&self) -> ExecutionMode {
        if self.phase() == Phase::Planning {
            return ExecutionMode::Planning;
        }
        match self.approval {
            ApprovalMode::Ask | ApprovalMode::DenyAll => ExecutionMode::Supervised,
            ApprovalMode::ApproveAll => ExecutionMode::AutoApprove,
            ApprovalMode::FullAccess => ExecutionMode::FullAccess,
        }
    }

    pub async fn set_mode(&mut self, mode: ExecutionMode) -> Result<()> {
        self.set_phase(mode.phase()).await?;
        self.set_approval_mode(match mode {
            ExecutionMode::Planning | ExecutionMode::Supervised => ApprovalMode::Ask,
            ExecutionMode::AutoApprove => ApprovalMode::ApproveAll,
            ExecutionMode::FullAccess => ApprovalMode::FullAccess,
        });
        let event = self.harness_event(
            EventKind::StatusChanged {
                status: SessionStatus::Idle,
            },
            Some(serde_json::json!({ "mode": mode })),
        );
        self.store.append_events(vec![event]).await
    }

    pub fn handle(&self) -> CancelHandle {
        CancelHandle {
            token: self.cancel.clone(),
            registry: Arc::clone(self.tools.registry()),
        }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Switches phase and records the transition as a status event.
    pub async fn set_phase(&mut self, phase: Phase) -> Result<()> {
        if self.session.phase == phase {
            return Ok(());
        }
        self.store.set_phase(&self.session.id, phase).await?;
        self.session.phase = phase;
        self.tools
            .set_full_access(phase == Phase::Coding && self.approval == ApprovalMode::FullAccess);
        let status = match phase {
            Phase::Planning => SessionStatus::Idle,
            Phase::Coding => SessionStatus::Idle,
        };
        let event = self.harness_event(
            EventKind::StatusChanged { status },
            Some(serde_json::json!({ "phase": phase })),
        );
        self.store.append_events(vec![event]).await
    }

    fn harness_event(&mut self, kind: EventKind, diagnostic: Option<serde_json::Value>) -> Event {
        let event = Event {
            session_id: self.session.id.clone(),
            sequence: self.next_sequence,
            source: EventSource::Native,
            timestamp: Utc::now(),
            kind,
            diagnostic,
        };
        self.next_sequence += 1;
        event
    }

    /// Renumbers an adapter event into the session sequence, keeping the
    /// adapter's own number in the diagnostic.
    fn renumber(&mut self, mut event: Event) -> Event {
        let adapter_sequence = event.sequence;
        event.session_id = self.session.id.clone();
        event.sequence = self.next_sequence;
        self.next_sequence += 1;
        let extra = serde_json::json!({ "adapter_sequence": adapter_sequence });
        event.diagnostic = Some(match event.diagnostic.take() {
            Some(serde_json::Value::Object(mut map)) => {
                map.insert("adapter_sequence".into(), extra["adapter_sequence"].clone());
                serde_json::Value::Object(map)
            }
            Some(other) => {
                serde_json::json!({ "raw": other, "adapter_sequence": adapter_sequence })
            }
            None => extra,
        });
        event
    }

    /// Redacts, shows, and persists one event. An interface that can no
    /// longer deliver output ends the turn: the request is cancelled and
    /// every child process stopped before the error is returned.
    async fn emit(&mut self, ui: &mut dyn Ui, event: Event) -> Result<()> {
        let event = redact_event(event, &self.secrets);
        ui.event(&event);
        if let Some(message) = ui.output_error() {
            self.cancel.cancel();
            self.tools.registry().kill_all().await;
            return Err(Error::config(format!("output failed: {message}")));
        }
        let stored = if self.telemetry.content_logging() {
            event
        } else {
            persisted_projection(event)
        };
        self.store.append_events(vec![stored]).await
    }

    fn system_prompt(&self) -> String {
        match self.session.phase {
            Phase::Planning => format!(
                "You are Gritt, a coding agent working in {}. This is the planning phase: \
                 inspect workspace files with file_read, discuss the task, ask questions, and propose a plan. \
                 Writes, shell commands, and MCP tools are disabled.",
                self.tools.workspace().root().display()
            ),
            Phase::Coding => format!(
                "You are Gritt, a coding agent working in {}. This is the coding phase. \
                 Use file_read, file_write, and shell to do the work. Paths are relative to the \
                 workspace root. Mode: {}. {}",
                self.tools.workspace().root().display(), self.mode().label(), self.mode().description()
            ),
        }
    }

    fn request(&self, prompt: &str) -> PromptRequest {
        let (model, effort) = match &self.session.kind {
            SessionKind::Native { model, effort, .. } => (model.clone(), *effort),
            SessionKind::Connector { .. } => (String::new(), ReasoningEffort::Auto),
        };
        let mut messages = Vec::new();
        let mut content = prompt.to_owned();
        if !self.started {
            messages.push(Message {
                role: Role::System,
                content: self.system_prompt(),
            });
        } else if self.sent_phase != Some(self.session.phase) {
            // The system prompt went out under the old phase; tell the
            // model what changed rather than leaving it believing tools
            // are absent (or present).
            content = format!(
                "{} Mode: {}. {}\n\n{}",
                phase_transition_note(self.session.phase),
                self.mode().label(),
                self.mode().description(),
                prompt
            );
        }
        messages.push(Message {
            role: Role::User,
            content,
        });
        PromptRequest {
            model,
            messages,
            tools: match self.session.phase {
                Phase::Planning => NativeTools::definitions()
                    .into_iter()
                    .filter(|tool| tool.name == gritt_core::tool::native::FILE_READ)
                    .collect(),
                Phase::Coding => {
                    let mut tools = NativeTools::definitions();
                    tools.extend(self.mcp_tools.definitions().iter().cloned());
                    tools
                }
            },
            options: RequestOptions {
                effort,
                ..RequestOptions::default()
            },
        }
    }

    /// Applies any tool-list change between turns and takes this turn's
    /// snapshot. Doing it here, before the request is built, is what keeps
    /// the advertised tools and the callable tools identical for the turn.
    pub async fn refresh_mcp_tools(&mut self) {
        let Some(mcp) = self.mcp.clone() else {
            return;
        };
        // A run that began on an external agent never opened the runtime,
        // because that agent owns its own MCP clients. This turn is native,
        // so this is the moment Gritt needs its own servers. Opening is
        // idempotent; a configuration error leaves the entries visible in the
        // snapshots rather than failing the turn.
        let _ = mcp.ensure_open(&self.cancel).await;
        mcp.refresh(&self.cancel).await;
        self.mcp_tools = mcp.tool_set().await;
    }

    /// The MCP tools the current turn may call.
    pub fn mcp_tools(&self) -> &McpToolSet {
        &self.mcp_tools
    }

    /// The request the next turn would send for `prompt`, without sending
    /// it. Tests use it to inspect the phase note and tool list.
    pub fn request_preview(&self, prompt: &str) -> PromptRequest {
        self.request(prompt)
    }

    /// Runs one user turn to completion.
    pub async fn run_turn(&mut self, prompt: &str, ui: &mut dyn Ui) -> Result<TurnOutcome> {
        let started_at = Utc::now();
        self.cancel.reset();
        self.refresh_mcp_tools().await;
        self.telemetry
            .content(&self.session.id, "user", prompt, &self.secrets)
            .await?;
        let request = self.request(prompt);
        let adapter = Arc::clone(&self.adapter);
        let status = self.harness_event(
            EventKind::StatusChanged {
                status: SessionStatus::Connecting,
            },
            None,
        );
        self.emit(ui, status).await?;
        let mut outcome = TurnOutcome {
            status: TurnStatus::Completed,
            text: String::new(),
            usage: Usage::default(),
            tool_calls: 0,
            error: None,
        };
        let mut stream = match adapter.send(request).await {
            Ok(stream) => stream,
            Err(error) => {
                return self.fail(ui, error, started_at, outcome).await;
            }
        };
        self.started = true;
        if self.sent_phase != Some(self.session.phase) {
            self.sent_phase = Some(self.session.phase);
            self.store
                .set_told_phase(&self.session.id, self.sent_phase)
                .await?;
        }
        loop {
            let mut pending_calls: Vec<ToolCall> = Vec::new();
            let mut wants_tools = false;
            let mut terminal = false;
            while let Some(item) = stream.next().await {
                let event = match item {
                    Ok(event) => event,
                    Err(error) => {
                        let event = self.harness_event(
                            EventKind::Error {
                                error_kind: error.kind,
                                message: error.message.clone(),
                            },
                            error.diagnostic.clone(),
                        );
                        self.emit(ui, event).await?;
                        outcome.status = TurnStatus::Failed;
                        outcome.error = Some(error.message);
                        break;
                    }
                };
                let event = self.renumber(event);
                match &event.kind {
                    EventKind::TextDelta { text } => outcome.text.push_str(text),
                    EventKind::ToolCall { call } => pending_calls.push(call.clone()),
                    EventKind::Usage { usage } => add_usage(&mut outcome.usage, usage),
                    EventKind::Completed { stop_reason } => {
                        wants_tools = *stop_reason == StopReason::ToolUse;
                        terminal = true;
                    }
                    EventKind::Error { message, .. } => {
                        outcome.status = TurnStatus::Failed;
                        outcome.error = Some(message.clone());
                        terminal = true;
                    }
                    EventKind::Cancelled => {
                        outcome.status = TurnStatus::Cancelled;
                        terminal = true;
                    }
                    _ => {}
                }
                self.emit(ui, event).await?;
                if terminal {
                    break;
                }
            }
            drop(stream);
            if outcome.status != TurnStatus::Completed || (!wants_tools && pending_calls.is_empty())
            {
                break;
            }
            if pending_calls.is_empty() {
                break;
            }
            // Tool phase: gate, ask, execute, then continue the conversation.
            let mut results = Vec::with_capacity(pending_calls.len());
            for call in pending_calls {
                outcome.tool_calls += 1;
                let result = self.handle_call(ui, &call).await?;
                results.push(result);
                if self.cancel.is_cancelled() {
                    outcome.status = TurnStatus::Cancelled;
                    break;
                }
            }
            if outcome.status == TurnStatus::Cancelled {
                let event = self.harness_event(EventKind::Cancelled, None);
                self.emit(ui, event).await?;
                break;
            }
            stream = match adapter.submit_tool_results(results).await {
                Ok(stream) => stream,
                Err(error) => {
                    return self.fail(ui, error, started_at, outcome).await;
                }
            };
        }
        if outcome.status == TurnStatus::Cancelled {
            self.tools.registry().kill_all().await;
        }
        if let Some(mut state) = adapter.continuation().await? {
            // The adapter's state carries the conversation, tool results
            // included; it is stored redacted like every other row.
            state.state = redact_value(state.state, &self.secrets);
            self.store
                .save_continuation(&self.session.id, state)
                .await?;
        }
        if !outcome.text.is_empty() {
            self.telemetry
                .content(&self.session.id, "assistant", &outcome.text, &self.secrets)
                .await?;
        }
        let final_status = match outcome.status {
            TurnStatus::Completed => SessionStatus::Finished,
            TurnStatus::Cancelled => SessionStatus::Idle,
            TurnStatus::Failed => SessionStatus::Failed,
        };
        let event = self.harness_event(
            EventKind::StatusChanged {
                status: final_status,
            },
            None,
        );
        self.emit(ui, event).await?;
        self.record_turn(started_at, &outcome).await?;
        if self.remember_pending && outcome.status == TurnStatus::Completed {
            self.remember_choices().await;
        }
        Ok(outcome)
    }

    /// Records the session's native choices as the workspace's last-used
    /// preferences. A profile name, a model id, and an effort level: no
    /// credential is involved. Best effort: the turn has already
    /// completed and been shown, so a failed write is retried on the next
    /// completed turn rather than reported as a failed turn.
    async fn remember_choices(&mut self) {
        let SessionKind::Native {
            provider_profile,
            model,
            effort,
        } = &self.session.kind
        else {
            return;
        };
        let written = self
            .store
            .set_last_used(
                self.tools.workspace().root(),
                &LastUsedNative {
                    provider_profile: provider_profile.clone(),
                    model: model.clone(),
                    effort: *effort,
                },
            )
            .await;
        self.remember_pending = written.is_err();
    }

    async fn fail(
        &mut self,
        ui: &mut dyn Ui,
        error: Error,
        started_at: chrono::DateTime<Utc>,
        mut outcome: TurnOutcome,
    ) -> Result<TurnOutcome> {
        let event = self.harness_event(
            EventKind::Error {
                error_kind: error.kind,
                message: error.message.clone(),
            },
            error.diagnostic.clone(),
        );
        self.emit(ui, event).await?;
        outcome.status = TurnStatus::Failed;
        outcome.error = Some(error.message.clone());
        self.record_turn(started_at, &outcome).await?;
        Err(error)
    }

    async fn record_turn(
        &self,
        started_at: chrono::DateTime<Utc>,
        outcome: &TurnOutcome,
    ) -> Result<()> {
        let mut counters = BTreeMap::new();
        counters.insert("tool_calls".to_string(), outcome.tool_calls);
        if let Some(value) = outcome.usage.input_tokens {
            counters.insert("input_tokens".to_string(), value);
        }
        if let Some(value) = outcome.usage.output_tokens {
            counters.insert("output_tokens".to_string(), value);
        }
        let status = match outcome.status {
            TurnStatus::Completed => "completed",
            TurnStatus::Cancelled => "cancelled",
            TurnStatus::Failed => "failed",
        };
        self.telemetry
            .turn(
                &self.session.id,
                started_at,
                status,
                counters,
                self.labels.clone(),
            )
            .await
    }

    /// Records a call that never ran: a bad argument, a path outside the
    /// workspace, or a preview that could not be built.
    async fn refuse_call(
        &mut self,
        ui: &mut dyn Ui,
        call: &ToolCall,
        message: String,
    ) -> Result<ToolResult> {
        let result = ToolResult {
            call_id: call.id.clone(),
            name: call.name.clone(),
            is_error: true,
            output: redact_text(&message, &self.secrets),
        };
        let event = self.harness_event(
            EventKind::ToolResult {
                result: result.clone(),
            },
            None,
        );
        self.emit(ui, event).await?;
        Ok(result)
    }

    /// The resource a call touches. An MCP dispatch name resolves through
    /// this turn's snapshot to `mcp:<server>/<tool>`, so a policy rule can
    /// name one server or one tool. A name the snapshot does not hold never
    /// reaches a server.
    fn resource_for(&self, call: &ToolCall) -> Result<crate::policy::Resource> {
        if !is_dispatch_name(&call.name) {
            return self.tools.resource_for(call);
        }
        match self.mcp_tools.lookup(&call.name) {
            Some(frozen) => Ok(crate::policy::Resource::Other(format!(
                "mcp:{}/{}",
                frozen.reference.server, frozen.reference.tool
            ))),
            None => Err(Error::config(format!(
                "unknown tool `{}`; it is not offered by any connected MCP server",
                call.name
            ))),
        }
    }

    /// Executes an approved call on the tool's owner. The policy engine has
    /// already run; there is no path here that skips it.
    async fn execute_call(&mut self, call: &ToolCall) -> ToolResult {
        if !is_dispatch_name(&call.name) {
            return self.tools.execute(call, &self.cancel).await;
        }
        let Some(mcp) = self.mcp.clone() else {
            return ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                is_error: true,
                output: "no MCP runtime is available for this session".into(),
            };
        };
        // The turn's frozen entry, not the live registry: the call that runs
        // is the one the permission engine approved, or none at all.
        let Some(frozen) = self.mcp_tools.lookup(&call.name).cloned() else {
            return ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                is_error: true,
                output: format!("`{}` is not offered by any connected MCP server", call.name),
            };
        };
        match mcp.call(&frozen, &call.arguments, &self.cancel).await {
            Ok(rendered) => ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                is_error: rendered.is_error,
                output: rendered.output,
            },
            // A transport failure is reported as a tool error, never
            // retried: the server may already have done the work.
            Err(error) => ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                is_error: true,
                output: error.message,
            },
        }
    }

    /// Policy, approval, execution, and the events for one call.
    async fn handle_call(&mut self, ui: &mut dyn Ui, call: &ToolCall) -> Result<ToolResult> {
        if self.phase() == Phase::Planning && call.name != gritt_core::tool::native::FILE_READ {
            return self
                .refuse_call(ui, call, "planning mode only allows file_read".into())
                .await;
        }
        let resource = match self.resource_for(call) {
            Ok(resource) => resource,
            Err(error) => {
                // A bad argument or a path outside the workspace never
                // reaches the policy; the refusal is still recorded.
                return self.refuse_call(ui, call, error.message).await;
            }
        };
        let decision = self
            .policy
            .evaluate_mode(&call.name, &resource, self.mode());
        let approved = match decision.outcome {
            PolicyOutcome::Allow => true,
            PolicyOutcome::Deny => {
                let event = self.harness_event(
                    EventKind::StatusChanged {
                        status: SessionStatus::Idle,
                    },
                    Some(serde_json::json!({
                        "policy": "deny", "tool": call.name, "reason": decision.reason
                    })),
                );
                self.emit(ui, event).await?;
                false
            }
            PolicyOutcome::Ask => {
                // The preview is built before anything is recorded: a
                // preview that cannot be built (unreadable or non-UTF-8
                // target) refuses the call, leaving no unmatched request.
                let preview = if self.approval == ApprovalMode::Ask {
                    match self.tools.preview(call) {
                        Ok(preview) => preview.map(|text| redact_text(&text, &self.secrets)),
                        Err(error) => {
                            return self.refuse_call(ui, call, error.message).await;
                        }
                    }
                } else {
                    None
                };
                // One redacted request serves the interface, the events,
                // and the transcript; the raw resource never leaves here.
                let request = ApprovalRequest {
                    id: ApprovalId(uuid::Uuid::new_v4().to_string()),
                    tool: call.name.clone(),
                    resource: redact_text(&resource.display(), &self.secrets),
                    reason: redact_text(&decision.reason, &self.secrets),
                    call_id: Some(call.id.clone()),
                };
                let decision = Decision {
                    reason: request.reason.clone(),
                    ..decision.clone()
                };
                let event = self.harness_event(
                    EventKind::StatusChanged {
                        status: SessionStatus::WaitingForApproval,
                    },
                    None,
                );
                self.emit(ui, event).await?;
                let event = self.harness_event(
                    EventKind::ApprovalRequested {
                        request: request.clone(),
                    },
                    Some(serde_json::json!({ "destructive": decision.destructive })),
                );
                self.emit(ui, event).await?;
                let answer = match self.approval {
                    ApprovalMode::ApproveAll | ApprovalMode::FullAccess => {
                        ApprovalDecision::Approved
                    }
                    ApprovalMode::DenyAll => ApprovalDecision::Denied,
                    ApprovalMode::Ask => {
                        // The wait races cancellation so Ctrl-C or Esc
                        // during a pending approval ends the turn.
                        let cancel = self.cancel.clone();
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => ApprovalDecision::Denied,
                            answer = ui.approve(&request, &decision, preview.as_deref()) => answer,
                        }
                    }
                };
                let event = self.harness_event(
                    EventKind::ApprovalDecided {
                        request_id: request.id.clone(),
                        decision: answer,
                    },
                    None,
                );
                self.emit(ui, event).await?;
                answer == ApprovalDecision::Approved
            }
        };
        let result = if approved {
            let event = self.harness_event(
                EventKind::StatusChanged {
                    status: SessionStatus::RunningTool,
                },
                Some(serde_json::json!({ "tool": call.name })),
            );
            self.emit(ui, event).await?;
            let mut result = self.execute_call(call).await;
            // Redacted before it reaches the model, the store, or the
            // interface; the emitter above only covers adapter events.
            result.output = redact_text(&result.output, &self.secrets);
            result
        } else {
            ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                is_error: true,
                output: format!(
                    "{} was not permitted: {}",
                    call.name,
                    if decision.outcome == PolicyOutcome::Deny {
                        "denied by policy"
                    } else {
                        "the user declined"
                    }
                ),
            }
        };
        let event = self.harness_event(
            EventKind::ToolResult {
                result: result.clone(),
            },
            None,
        );
        self.emit(ui, event).await?;
        Ok(result)
    }
}

fn add_usage(total: &mut Usage, usage: &Usage) {
    fn add(slot: &mut Option<u64>, value: Option<u64>) {
        if let Some(value) = value {
            *slot = Some(slot.unwrap_or(0) + value);
        }
    }
    add(&mut total.input_tokens, usage.input_tokens);
    add(&mut total.output_tokens, usage.output_tokens);
    add(&mut total.reasoning_tokens, usage.reasoning_tokens);
    add(&mut total.cached_input_tokens, usage.cached_input_tokens);
}

/// A native session opened by the builder with the startup notes a new
/// session produced. `catalog` is the model-list state the new session's
/// choices were resolved against, `None` for a resumed session.
pub struct NativeOpen {
    pub agent: NativeAgent,
    pub warnings: Vec<DraftWarning>,
    pub catalog: Option<crate::draft::CatalogState>,
}

/// Which session a turn runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSelector {
    /// Create a new session, optionally named.
    New { name: Option<String> },
    /// Resume the named session, or create it when it does not exist.
    Named(String),
    /// Resume by id.
    Id(SessionId),
}

/// Everything needed to build agents: config, store, keys, transport, and
/// the model catalog. The binary owns the concrete key source.
/// Cloning shares every handle and copies only the configuration, which is
/// what [`crate::control::ControlPlane::reload_config`] needs: a rebuilt
/// plane must not open a second store, catalog, or MCP runtime.
#[derive(Clone)]
pub struct AgentBuilder {
    pub config: Config,
    pub store: Arc<Store>,
    pub telemetry: Arc<Telemetry>,
    pub keys: Arc<dyn KeyProvider>,
    pub transport: Arc<dyn HttpTransport>,
    pub catalog: Arc<ModelCatalog>,
    pub cache: Option<ModelCache>,
    pub workspace: Workspace,
    pub approval: ApprovalMode,
    /// The workspace MCP runtime. `None` means no `.mcp.json` support for
    /// sessions built here, which is what a caller that never opened one
    /// gets.
    pub mcp: Option<Arc<McpRuntime>>,
}

impl AgentBuilder {
    /// Shares one MCP runtime with every session this builder opens. One
    /// connection per server per workspace, not one per turn.
    pub fn with_mcp(mut self, mcp: Arc<McpRuntime>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    pub fn mcp(&self) -> Option<&Arc<McpRuntime>> {
        self.mcp.as_ref()
    }

    /// Loads the profile's model list into the catalog, refreshing at most
    /// daily. A missing list is reported, not fatal: capabilities then
    /// come back unreported.
    pub async fn load_catalog(&self, profile: &str) -> Result<Option<Error>> {
        let Some(cache) = &self.cache else {
            return Ok(None);
        };
        let Some(definition) = self.config.profiles.get(profile) else {
            return Err(Error::config(format!("unknown profile `{profile}`")));
        };
        match load_models(
            cache,
            self.transport.as_ref(),
            self.keys.as_ref(),
            definition,
            &self.config.model_list,
            Utc::now(),
            false,
        )
        .await
        {
            Ok(list) => {
                self.catalog.insert(list);
                Ok(None)
            }
            Err(error) => Ok(Some(error)),
        }
    }

    /// The provider profile a session will actually run on: the resumed
    /// session's own profile when the selector names an existing native
    /// session, else the profile a new session starts on before any
    /// failover (an explicit profile, the one a qualified model name or
    /// global alias spells out, the configured default, or the last
    /// successful session's). Callers warm the catalog for this profile
    /// before opening, so capability and deprecation data belong to the
    /// right provider. No side effects: the phase is left alone.
    pub async fn session_profile(
        &self,
        selector: &SessionSelector,
        profile_hint: Option<&str>,
        model: Option<&str>,
    ) -> Result<String> {
        if let Some(session) = self.find_session(selector, None).await? {
            if let SessionKind::Native {
                provider_profile, ..
            } = session.kind
            {
                return Ok(provider_profile);
            }
        }
        let request = StartupRequest::from_flags(profile_hint, model, None);
        let last_used = self.last_used().await?;
        match self.primary_profile(&request, last_used.as_ref()) {
            Some((profile, _)) if self.config.profiles.contains_key(&profile) => Ok(profile),
            Some((profile, _)) => Err(DraftError::UnknownProfile { profile }.into_error()),
            None => Err(DraftError::MissingProfile.into_error()),
        }
    }

    /// Finds the session a selector names, checking that it belongs to
    /// the current workspace and applying a requested phase. `None` when
    /// the selector names nothing yet.
    pub async fn find_session(
        &self,
        selector: &SessionSelector,
        phase: Option<Phase>,
    ) -> Result<Option<Session>> {
        let existing = match selector {
            SessionSelector::New { .. } => None,
            SessionSelector::Named(name) => self.store.find_by_name(name).await?,
            SessionSelector::Id(id) => self.store.get(id).await?,
        };
        let Some(mut session) = existing else {
            return Ok(None);
        };
        // Policy and tools use the current workspace, so a session
        // recorded elsewhere must not silently run here.
        let recorded = session
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| session.workspace.clone());
        if recorded != self.workspace.root() {
            return Err(Error::config(format!(
                "session `{}` belongs to workspace {} but the current workspace is {}; \
                 run with --workspace {} to resume it",
                session.name,
                session.workspace.display(),
                self.workspace.root().display(),
                session.workspace.display()
            )));
        }
        if let Some(phase) = phase {
            if session.phase != phase {
                self.store.set_phase(&session.id, phase).await?;
                session.phase = phase;
            }
        }
        Ok(Some(session))
    }

    /// The name a selector gives a new session.
    pub fn session_name(selector: &SessionSelector, id: &SessionId) -> String {
        match selector {
            SessionSelector::Named(name) => name.clone(),
            SessionSelector::New { name: Some(name) } => name.clone(),
            _ => format!("session-{}", &id.0[..8]),
        }
    }

    /// Opens or creates the session and builds its agent. A new session
    /// goes through the startup resolver with the flags as given, so it
    /// gets the same failover and remembered choices as every mode; the
    /// resolver's notes are dropped here, see [`AgentBuilder::open_with`].
    pub async fn open(
        &self,
        selector: SessionSelector,
        profile: Option<&str>,
        model: Option<&str>,
        phase: Option<Phase>,
    ) -> Result<NativeAgent> {
        let opened = self
            .open_with(
                selector,
                StartupRequest::from_flags(profile, model, None),
                phase,
            )
            .await?;
        Ok(opened.agent)
    }

    /// Opens or creates the session and builds its agent, with the
    /// startup notes for a new session: skipped profiles, remembered
    /// choices, model warnings, and the state of the model list it was
    /// resolved against. A resumed session keeps its stored profile,
    /// model, and effort and produces no notes.
    pub async fn open_with(
        &self,
        selector: SessionSelector,
        request: StartupRequest,
        phase: Option<Phase>,
    ) -> Result<NativeOpen> {
        if let Some(session) = self.find_session(&selector, phase).await? {
            return Ok(NativeOpen {
                agent: self.agent_for(session).await?,
                warnings: Vec::new(),
                catalog: None,
            });
        }
        self.start_native(&selector, request, phase).await
    }

    /// Creates a new session from a startup request, after the caller has
    /// established that the selector names nothing yet. A rejected
    /// request is the typed draft error as an [`Error`].
    pub async fn start_native(
        &self,
        selector: &SessionSelector,
        request: StartupRequest,
        phase: Option<Phase>,
    ) -> Result<NativeOpen> {
        match self.resolve_startup(&request).await? {
            StartupOutcome::Ready(selection) => {
                let agent = self
                    .create_native(
                        selector,
                        selection.profile,
                        selection.model,
                        selection.effort,
                        phase,
                    )
                    .await?;
                Ok(NativeOpen {
                    agent,
                    warnings: selection.warnings,
                    catalog: Some(selection.catalog),
                })
            }
            StartupOutcome::Rejected { mut errors, .. } => Err(errors
                .drain(..)
                .next()
                .unwrap_or(DraftError::MissingProfile)
                .into_error()),
        }
    }

    /// Creates a native session from already resolved choices and builds
    /// its agent. The profile and model are stored as given; callers
    /// resolve aliases first.
    pub async fn create_native(
        &self,
        selector: &SessionSelector,
        profile: String,
        model: String,
        effort: ReasoningEffort,
        phase: Option<Phase>,
    ) -> Result<NativeAgent> {
        let now = Utc::now();
        let id = SessionId(uuid::Uuid::new_v4().to_string());
        let name = Self::session_name(selector, &id);
        let session = Session {
            id,
            name,
            kind: SessionKind::Native {
                provider_profile: profile,
                model,
                effort,
            },
            phase: phase.unwrap_or(Phase::Planning),
            workspace: self.workspace.root().to_path_buf(),
            created_at: now,
            updated_at: now,
            parent_id: None,
        };
        self.store.create(session.clone()).await?;
        let mut agent = self.agent_for(session).await?;
        agent.new_session = true;
        agent.remember_pending = true;
        Ok(agent)
    }

    /// Builds the agent for a stored session. The phase the model was last
    /// told about is read from the store; when it differs from the
    /// session's phase, or is unknown, the next turn sends the transition
    /// note rather than assuming the model already heard it.
    pub async fn agent_for(&self, session: Session) -> Result<NativeAgent> {
        let SessionKind::Native {
            provider_profile,
            model,
            ..
        } = &session.kind
        else {
            return Err(Error::config(
                "connector sessions are driven by their connector, not the native loop",
            ));
        };
        let profile = self
            .config
            .profiles
            .get(provider_profile)
            .cloned()
            .ok_or_else(|| Error::config(format!("unknown profile `{provider_profile}`")))?;
        let cancel = CancellationToken::new();
        let capabilities: Arc<dyn CapabilitySource> = self.catalog.clone();
        // The active key is the one secret the harness can know about; a
        // missing key is reported by the adapter on the first request.
        let secrets: Vec<Secret> = self
            .keys
            .key(provider_profile, &profile.key)
            .ok()
            .into_iter()
            .collect();
        let adapter = adapter_for(AdapterContext {
            profile,
            session_id: session.id.clone(),
            transport: Arc::clone(&self.transport),
            keys: Arc::clone(&self.keys),
            capabilities,
            cancel: cancel.clone(),
        });
        let mut started = false;
        if let Some(state) = self.store.load_continuation(&session.id).await? {
            adapter.restore(state).await?;
            started = true;
        }
        let next_sequence = self.store.next_sequence(&session.id).await?;
        let policy = PolicyEngine::new(self.config.policy.clone(), self.workspace.root());
        let blocked_env: Vec<String> = self
            .config
            .profiles
            .values()
            .map(|profile| profile.key.env_var_name.clone())
            .collect();
        let mut tools = NativeTools::new(self.workspace.clone(), ProcessRegistry::new())
            .with_blocked_env(blocked_env);
        tools.set_full_access(
            self.approval == ApprovalMode::FullAccess && session.phase == Phase::Coding,
        );
        // Authority belongs to this run, so resumed provider history must
        // hear the current mode even if the persisted phase is unchanged.
        let sent_phase = None;
        let labels = BTreeMap::from([
            ("profile".to_string(), provider_profile.clone()),
            ("model".to_string(), model.clone()),
        ]);
        let mcp_tools = match &self.mcp {
            Some(mcp) => mcp.tool_set().await,
            None => McpToolSet::default(),
        };
        Ok(NativeAgent {
            session,
            adapter,
            store: Arc::clone(&self.store),
            policy,
            tools,
            mcp: self.mcp.clone(),
            mcp_tools,
            telemetry: Arc::clone(&self.telemetry),
            cancel,
            approval: self.approval,
            next_sequence,
            started,
            sent_phase,
            secrets,
            labels,
            new_session: false,
            remember_pending: false,
        })
    }

    /// Reads the workspace root back for callers that need it.
    pub fn workspace_root(&self) -> &Path {
        self.workspace.root()
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
}

/// True when the error means the turn was cancelled rather than failed.
pub fn is_cancelled(error: &Error) -> bool {
    error.kind == ErrorKind::Cancelled
}

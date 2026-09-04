//! The native agent loop (ADR-007, ADR-009). Drives one provider adapter
//! through a session: stream events, persist them, gate every tool call
//! through the policy engine, ask the interface when the policy says so,
//! execute, submit results, and continue until the turn completes, fails,
//! or is cancelled. Planning turns carry no tools; coding turns carry the
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
use gritt_core::provider::{Message, PromptRequest, RequestOptions, Role};
use gritt_core::session::{BoxFuture, Phase, Session, SessionId, SessionKind, SessionStore};
use gritt_core::tool::{ToolCall, ToolResult};
use gritt_core::{Error, ErrorKind, Result};
use gritt_provider::adapter::{CapabilitySource, KeyProvider};
use gritt_provider::alias;
use gritt_provider::models::{load_models, ModelCache, ModelCatalog};
use gritt_provider::transport::HttpTransport;
use gritt_provider::{adapter_for, AdapterContext, CancellationToken};

use crate::policy::{Decision, PolicyEngine};
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
    /// Deny without asking. The default when no terminal can answer.
    DenyAll,
}

/// What an interface must provide to the loop. Print, REPL, and the
/// full-screen mode all implement it.
pub trait Ui: Send {
    /// Every persisted event, in order.
    fn event(&mut self, event: &Event);
    /// Answer an `ask` outcome. `preview` is the unified diff for a write.
    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        decision: &'a Decision,
        preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision>;
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
    telemetry: Arc<Telemetry>,
    cancel: CancellationToken,
    approval: ApprovalMode,
    next_sequence: u64,
    started: bool,
    labels: BTreeMap<String, String>,
}

impl NativeAgent {
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn phase(&self) -> Phase {
        self.session.phase
    }

    pub fn approval_mode(&self) -> ApprovalMode {
        self.approval
    }

    pub fn set_approval_mode(&mut self, mode: ApprovalMode) {
        self.approval = mode;
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
        self.session.phase = phase;
        self.store.set_phase(&self.session.id, phase).await?;
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

    async fn emit(&mut self, ui: &mut dyn Ui, event: Event) -> Result<()> {
        ui.event(&event);
        self.store.append_events(vec![event]).await
    }

    fn system_prompt(&self) -> String {
        match self.session.phase {
            Phase::Planning => format!(
                "You are Gritt, a coding agent working in {}. This is the planning phase: \
                 discuss the task, ask questions, and propose a plan. No tools are available.",
                self.tools.workspace().root().display()
            ),
            Phase::Coding => format!(
                "You are Gritt, a coding agent working in {}. This is the coding phase. \
                 Use file_read, file_write, and shell to do the work. Paths are relative to the \
                 workspace root. The user approves writes and commands.",
                self.tools.workspace().root().display()
            ),
        }
    }

    fn request(&self, prompt: &str) -> PromptRequest {
        let model = match &self.session.kind {
            SessionKind::Native { model, .. } => model.clone(),
            SessionKind::Connector { .. } => String::new(),
        };
        let mut messages = Vec::new();
        if !self.started {
            messages.push(Message {
                role: Role::System,
                content: self.system_prompt(),
            });
        }
        messages.push(Message {
            role: Role::User,
            content: prompt.to_owned(),
        });
        PromptRequest {
            model,
            messages,
            tools: match self.session.phase {
                Phase::Planning => Vec::new(),
                Phase::Coding => NativeTools::definitions(),
            },
            options: RequestOptions::default(),
        }
    }

    /// Runs one user turn to completion.
    pub async fn run_turn(&mut self, prompt: &str, ui: &mut dyn Ui) -> Result<TurnOutcome> {
        let started_at = Utc::now();
        self.cancel.reset();
        self.telemetry
            .content(&self.session.id, "user", prompt)
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
        if let Some(state) = adapter.continuation().await? {
            self.store
                .save_continuation(&self.session.id, state)
                .await?;
        }
        if !outcome.text.is_empty() {
            self.telemetry
                .content(&self.session.id, "assistant", &outcome.text)
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
        Ok(outcome)
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

    /// Policy, approval, execution, and the events for one call.
    async fn handle_call(&mut self, ui: &mut dyn Ui, call: &ToolCall) -> Result<ToolResult> {
        let resource = match self.tools.resource_for(call) {
            Ok(resource) => resource,
            Err(error) => {
                // A bad argument or a path outside the workspace never
                // reaches the policy; the refusal is still recorded.
                let result = ToolResult {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    is_error: true,
                    output: error.message,
                };
                let event = self.harness_event(
                    EventKind::ToolResult {
                        result: result.clone(),
                    },
                    None,
                );
                self.emit(ui, event).await?;
                return Ok(result);
            }
        };
        let decision = self.policy.evaluate(&call.name, &resource);
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
                let request = ApprovalRequest {
                    id: ApprovalId(uuid::Uuid::new_v4().to_string()),
                    tool: call.name.clone(),
                    resource: resource.display(),
                    reason: decision.reason.clone(),
                    call_id: Some(call.id.clone()),
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
                    ApprovalMode::ApproveAll => ApprovalDecision::Approved,
                    ApprovalMode::DenyAll => ApprovalDecision::Denied,
                    ApprovalMode::Ask => {
                        let preview = self.tools.preview(call).ok().flatten();
                        ui.approve(&request, &decision, preview.as_deref()).await
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
            self.tools.execute(call, &self.cancel).await
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
}

impl AgentBuilder {
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

    /// Resolves `model` (alias, qualified, or bare) against the config and
    /// catalog. Bare names use `profile_hint`, then the default profile.
    pub fn resolve_model(
        &self,
        model: Option<&str>,
        profile_hint: Option<&str>,
    ) -> Result<(String, String)> {
        let name = model
            .map(str::to_owned)
            .or_else(|| self.config.default_model.clone())
            .ok_or_else(|| Error::config("no model given and no default_model configured"))?;
        let resolved = alias::resolve(&self.config, &self.catalog, &name, profile_hint)?;
        Ok((resolved.profile, resolved.model))
    }

    /// Opens or creates the session and builds its agent.
    pub async fn open(
        &self,
        selector: SessionSelector,
        profile: Option<&str>,
        model: Option<&str>,
        phase: Option<Phase>,
    ) -> Result<NativeAgent> {
        let existing = match &selector {
            SessionSelector::New { .. } => None,
            SessionSelector::Named(name) => self.store.find_by_name(name).await?,
            SessionSelector::Id(id) => self.store.get(id).await?,
        };
        let session = match existing {
            Some(mut session) => {
                if let Some(phase) = phase {
                    if session.phase != phase {
                        self.store.set_phase(&session.id, phase).await?;
                        session.phase = phase;
                    }
                }
                session
            }
            None => {
                let (profile_name, model_id) = self.resolve_model(model, profile)?;
                let now = Utc::now();
                let id = SessionId(uuid::Uuid::new_v4().to_string());
                let name = match &selector {
                    SessionSelector::Named(name) => name.clone(),
                    SessionSelector::New { name: Some(name) } => name.clone(),
                    _ => format!("session-{}", &id.0[..8]),
                };
                let session = Session {
                    id,
                    name,
                    kind: SessionKind::Native {
                        provider_profile: profile_name,
                        model: model_id,
                    },
                    phase: phase.unwrap_or(Phase::Planning),
                    workspace: self.workspace.root().to_path_buf(),
                    created_at: now,
                    updated_at: now,
                    parent_id: None,
                };
                self.store.create(session.clone()).await?;
                session
            }
        };
        self.agent_for(session).await
    }

    pub async fn agent_for(&self, session: Session) -> Result<NativeAgent> {
        let SessionKind::Native {
            provider_profile,
            model,
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
        let tools = NativeTools::new(self.workspace.clone(), ProcessRegistry::new());
        let labels = BTreeMap::from([
            ("profile".to_string(), provider_profile.clone()),
            ("model".to_string(), model.clone()),
        ]);
        Ok(NativeAgent {
            session,
            adapter,
            store: Arc::clone(&self.store),
            policy,
            tools,
            telemetry: Arc::clone(&self.telemetry),
            cancel,
            approval: self.approval,
            next_sequence,
            started,
            labels,
        })
    }

    /// Reads the workspace root back for callers that need it.
    pub fn workspace_root(&self) -> &Path {
        self.workspace.root()
    }
}

/// True when the error means the turn was cancelled rather than failed.
pub fn is_cancelled(error: &Error) -> bool {
    error.kind == ErrorKind::Cancelled
}

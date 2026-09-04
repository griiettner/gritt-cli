//! Runs a session on an external connector through the same store,
//! interface, telemetry, and event model as the native loop. The
//! external agent keeps its own authority (ADR-010): this runner shows
//! what it does, relays approvals when the connector can take an answer,
//! records decisions, and stores the transcript beside native sessions.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use futures::StreamExt;
use gritt_core::connector::{Connector, ConnectorInfo, TaskRequest};
use gritt_core::event::{ApprovalDecision, Event, EventKind, EventSource, SessionStatus, Usage};
use gritt_core::policy::PolicyOutcome;
use gritt_core::secret::Secret;
use gritt_core::session::{BoxFuture, Phase, Session, SessionStore};
use gritt_core::{Error, ErrorKind, Result};
use gritt_provider::adapter::{redact_text, redact_value};
use gritt_provider::CancellationToken;

use crate::agent::{persisted_projection, ApprovalMode, CancelHandle, TurnOutcome, TurnStatus, Ui};
use crate::driver::{Driver, DriverInfo};
use crate::policy::Decision;
use crate::store::Store;
use crate::telemetry::Telemetry;
use crate::tools::ProcessRegistry;

/// The request prefix for a planning turn. The agent keeps its authority,
/// so this is a request, not a guard.
pub const PLANNING_NOTE: &str =
    "[Planning phase: discuss the task and propose a plan. Do not modify files or run commands.]";

pub struct ConnectorSession {
    session: Session,
    connector: Arc<dyn Connector>,
    info: Option<ConnectorInfo>,
    store: Arc<Store>,
    telemetry: Arc<Telemetry>,
    cancel: CancellationToken,
    registry: Arc<ProcessRegistry>,
    approval: ApprovalMode,
    next_sequence: u64,
    secrets: Vec<Secret>,
}

impl ConnectorSession {
    pub async fn open(
        session: Session,
        connector: Arc<dyn Connector>,
        store: Arc<Store>,
        telemetry: Arc<Telemetry>,
        approval: ApprovalMode,
        secrets: Vec<Secret>,
    ) -> Result<Self> {
        let next_sequence = store.next_sequence(&session.id).await?;
        // A missing or broken agent is reported here, once, and never
        // touches the native path.
        let info = connector.info().await.ok();
        Ok(Self {
            session,
            connector,
            info,
            store,
            telemetry,
            cancel: CancellationToken::new(),
            registry: ProcessRegistry::new(),
            approval,
            next_sequence,
            secrets,
        })
    }

    pub fn connector_info(&self) -> Option<&ConnectorInfo> {
        self.info.as_ref()
    }

    fn harness_event(&mut self, kind: EventKind, diagnostic: Option<serde_json::Value>) -> Event {
        let event = Event {
            session_id: self.session.id.clone(),
            sequence: self.next_sequence,
            source: EventSource::Connector {
                id: self.connector.id(),
            },
            timestamp: Utc::now(),
            kind,
            diagnostic,
        };
        self.next_sequence += 1;
        event
    }

    /// Renumbers a connector event into the session sequence; the
    /// connector's own number stays in the diagnostic.
    fn renumber(&mut self, mut event: Event) -> Event {
        let connector_sequence = event.sequence;
        event.session_id = self.session.id.clone();
        event.sequence = self.next_sequence;
        self.next_sequence += 1;
        let extra = serde_json::json!({ "connector_sequence": connector_sequence });
        event.diagnostic = Some(match event.diagnostic.take() {
            Some(serde_json::Value::Object(mut map)) => {
                map.insert(
                    "connector_sequence".into(),
                    extra["connector_sequence"].clone(),
                );
                serde_json::Value::Object(map)
            }
            Some(other) => {
                serde_json::json!({ "raw": other, "connector_sequence": connector_sequence })
            }
            None => extra,
        });
        event
    }

    fn redact(&self, mut event: Event) -> Event {
        if self.secrets.is_empty() {
            return event;
        }
        match &mut event.kind {
            EventKind::TextDelta { text } | EventKind::ReasoningSummary { text } => {
                *text = redact_text(text, &self.secrets);
            }
            EventKind::ToolCall { call } => {
                call.arguments = redact_value(call.arguments.take(), &self.secrets);
            }
            EventKind::ToolResult { result } => {
                result.output = redact_text(&result.output, &self.secrets);
            }
            EventKind::ApprovalRequested { request } => {
                request.resource = redact_text(&request.resource, &self.secrets);
                request.reason = redact_text(&request.reason, &self.secrets);
            }
            EventKind::Error { message, .. } => {
                *message = redact_text(message, &self.secrets);
            }
            _ => {}
        }
        if let Some(diagnostic) = event.diagnostic.take() {
            event.diagnostic = Some(redact_value(diagnostic, &self.secrets));
        }
        event
    }

    async fn emit(&mut self, ui: &mut dyn Ui, event: Event) -> Result<()> {
        let event = self.redact(event);
        ui.event(&event);
        if let Some(message) = ui.output_error() {
            self.cancel.cancel();
            let _ = self.connector.cancel(&self.session.id).await;
            return Err(Error::config(format!("output failed: {message}")));
        }
        let stored = if self.telemetry.content_logging() {
            event
        } else {
            persisted_projection(event)
        };
        self.store.append_events(vec![stored]).await
    }

    fn prompt_for(&self, prompt: &str) -> String {
        match self.session.phase {
            Phase::Planning => format!("{PLANNING_NOTE}\n\n{prompt}"),
            Phase::Coding => prompt.to_owned(),
        }
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
        let labels = BTreeMap::from([(
            "connector".to_string(),
            self.connector.id().as_str().to_owned(),
        )]);
        self.telemetry
            .turn(&self.session.id, started_at, status, counters, labels)
            .await
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
        let status = self.harness_event(
            EventKind::StatusChanged {
                status: SessionStatus::Failed,
            },
            None,
        );
        self.emit(ui, status).await?;
        outcome.status = TurnStatus::Failed;
        outcome.error = Some(error.message.clone());
        self.record_turn(started_at, &outcome).await?;
        Err(error)
    }

    /// Relays one approval the connector exposed: asks the interface
    /// (racing cancellation), answers the connector, records the decision.
    async fn relay_approval(&mut self, ui: &mut dyn Ui, event: &Event) -> Result<()> {
        let EventKind::ApprovalRequested { request } = &event.kind else {
            return Ok(());
        };
        let request = request.clone();
        let supports = self
            .info
            .as_ref()
            .is_some_and(|info| info.capabilities.approvals);
        if !supports {
            return Ok(());
        }
        let decision = Decision {
            outcome: PolicyOutcome::Ask,
            reason: format!(
                "{} asked to run {} on {}",
                self.connector.id().as_str(),
                request.tool,
                request.resource
            ),
            destructive: false,
            rule: None,
        };
        let answer = match self.approval {
            ApprovalMode::ApproveAll => ApprovalDecision::Approved,
            ApprovalMode::DenyAll => ApprovalDecision::Denied,
            ApprovalMode::Ask => {
                let cancel = self.cancel.clone();
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => ApprovalDecision::Denied,
                    answer = ui.approve(&request, &decision, None) => answer,
                }
            }
        };
        self.connector
            .answer_approval(&self.session.id, request.id.clone(), answer)
            .await?;
        let event = self.harness_event(
            EventKind::ApprovalDecided {
                request_id: request.id,
                decision: answer,
            },
            None,
        );
        self.emit(ui, event).await
    }

    pub async fn run_turn(&mut self, prompt: &str, ui: &mut dyn Ui) -> Result<TurnOutcome> {
        let started_at = Utc::now();
        self.cancel.reset();
        self.telemetry
            .content(&self.session.id, "user", prompt, &self.secrets)
            .await?;
        let mut outcome = TurnOutcome {
            status: TurnStatus::Completed,
            text: String::new(),
            usage: Usage::default(),
            tool_calls: 0,
            error: None,
        };
        let status = self.harness_event(
            EventKind::StatusChanged {
                status: SessionStatus::Connecting,
            },
            Some(serde_json::json!({
                "connector": self.connector.id(),
                "version": self.info.as_ref().and_then(|i| i.version.clone()),
                "auth": self.info.as_ref().map(|i| i.auth.clone()),
            })),
        );
        self.emit(ui, status).await?;
        let request = TaskRequest {
            session_id: self.session.id.clone(),
            prompt: self.prompt_for(prompt),
            workspace: self.session.workspace.clone(),
            continuation: self.store.load_continuation(&self.session.id).await?,
        };
        let connector = Arc::clone(&self.connector);
        let mut stream = match connector.start(request).await {
            Ok(stream) => stream,
            Err(error) => return self.fail(ui, error, started_at, outcome).await,
        };
        let mut cancel_sent = false;
        loop {
            let cancel = self.cancel.clone();
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled(), if !cancel_sent => {
                    cancel_sent = true;
                    let _ = connector.cancel(&self.session.id).await;
                    continue;
                }
                item = stream.next() => item,
            };
            let Some(item) = next else {
                break;
            };
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
            // The connector closes its stream when the turn is over, so
            // the loop reads to the end: a native session streams an
            // intermediate completion before its tool phase, and an
            // external agent may report an error after a completion.
            match &event.kind {
                EventKind::TextDelta { text } => outcome.text.push_str(text),
                EventKind::ToolCall { .. } => outcome.tool_calls += 1,
                EventKind::Usage { usage } => add_usage(&mut outcome.usage, usage),
                EventKind::Error { message, .. } => {
                    outcome.status = TurnStatus::Failed;
                    outcome.error = Some(message.clone());
                }
                EventKind::Cancelled => {
                    outcome.status = TurnStatus::Cancelled;
                }
                _ => {}
            }
            let is_approval = matches!(event.kind, EventKind::ApprovalRequested { .. });
            self.emit(ui, event.clone()).await?;
            if is_approval {
                self.relay_approval(ui, &event).await?;
            }
        }
        drop(stream);
        if self.cancel.is_cancelled() && outcome.status == TurnStatus::Completed {
            outcome.status = TurnStatus::Cancelled;
            let event = self.harness_event(EventKind::Cancelled, None);
            self.emit(ui, event).await?;
        }
        if let Ok(inspection) = connector.inspect(&self.session.id).await {
            if let Some(external_id) = inspection.external_id {
                self.store
                    .save_continuation(
                        &self.session.id,
                        gritt_core::session::ContinuationState {
                            owner: format!("connector:{}", self.connector.id().as_str()),
                            state: serde_json::json!({ "external_id": external_id }),
                        },
                    )
                    .await?;
            }
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
        if outcome.status == TurnStatus::Completed
            && outcome.error.is_none()
            && !matches!(final_status, SessionStatus::Finished)
        {
            return Err(Error::new(
                ErrorKind::Connector,
                "connector turn ended abnormally",
            ));
        }
        Ok(outcome)
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

impl Driver for ConnectorSession {
    fn session(&self) -> &Session {
        &self.session
    }

    fn phase(&self) -> Phase {
        self.session.phase
    }

    fn set_phase(&mut self, phase: Phase) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.session.phase == phase {
                return Ok(());
            }
            self.session.phase = phase;
            self.store.set_phase(&self.session.id, phase).await?;
            let event = self.harness_event(
                EventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
                Some(serde_json::json!({ "phase": phase })),
            );
            self.store.append_events(vec![event]).await
        })
    }

    fn handle(&self) -> CancelHandle {
        CancelHandle::new(self.cancel.clone(), Arc::clone(&self.registry))
    }

    fn run_turn<'a>(
        &'a mut self,
        prompt: &'a str,
        ui: &'a mut dyn Ui,
    ) -> BoxFuture<'a, Result<TurnOutcome>> {
        Box::pin(ConnectorSession::run_turn(self, prompt, ui))
    }

    fn info(&self) -> DriverInfo {
        DriverInfo {
            backend: self.connector.id().as_str().to_owned(),
            detail: self
                .info
                .as_ref()
                .and_then(|info| info.version.clone())
                .unwrap_or_default(),
        }
    }
}

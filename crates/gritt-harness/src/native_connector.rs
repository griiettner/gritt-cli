//! The native path behind the `Connector` contract, so the control plane
//! and any in-process client (ADR-011) can run it exactly like an
//! installed agent. Approvals are relayed through `answer_approval`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gritt_core::connector::{
    AuthState, Connector, ConnectorCapabilities, ConnectorId, ConnectorInfo, ConnectorInspection,
    TaskRequest, TaskState, Transport,
};
use gritt_core::event::{ApprovalDecision, ApprovalId, ApprovalRequest, Event};
use gritt_core::provider::EventStream;
use gritt_core::session::{BoxFuture, SessionId, SessionKind, SessionStore};
use gritt_core::{Error, Result};
use tokio::sync::{mpsc, oneshot};

use crate::agent::{AgentBuilder, CancelHandle, NativeAgent, SessionSelector, TurnStatus, Ui};
use crate::policy::Decision;

struct Slot {
    agent: Option<NativeAgent>,
    handle: Option<CancelHandle>,
    state: TaskState,
    pending: Option<String>,
    approvals: HashMap<ApprovalId, oneshot::Sender<ApprovalDecision>>,
    last_error: Option<String>,
}

type Slots = Arc<Mutex<HashMap<SessionId, Slot>>>;

pub struct NativeConnector {
    builder: Arc<AgentBuilder>,
    slots: Slots,
}

struct ChannelUi {
    tx: mpsc::UnboundedSender<Result<Event>>,
    session_id: SessionId,
    slots: Slots,
    closed: bool,
}

impl Ui for ChannelUi {
    fn event(&mut self, event: &Event) {
        // Delivery is lossless: the channel is unbounded, so a slow
        // consumer never costs an event (an approval request in
        // particular). A send only fails once the receiver is gone.
        if self.tx.send(Ok(event.clone())).is_err() {
            self.closed = true;
        }
    }

    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        _decision: &'a Decision,
        _preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        let (sender, receiver) = oneshot::channel();
        {
            let mut slots = self.slots.lock().expect("slots");
            if let Some(slot) = slots.get_mut(&self.session_id) {
                slot.state = TaskState::AwaitingApproval;
                slot.approvals.insert(request.id.clone(), sender);
            }
        }
        Box::pin(async move { receiver.await.unwrap_or(ApprovalDecision::Denied) })
    }

    fn output_error(&self) -> Option<String> {
        self.closed
            .then(|| "the event stream was dropped".to_owned())
    }
}

struct Receiver(mpsc::UnboundedReceiver<Result<Event>>);

impl futures::Stream for Receiver {
    type Item = Result<Event>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

impl NativeConnector {
    pub fn new(builder: Arc<AgentBuilder>) -> Self {
        Self {
            builder,
            slots: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn run(
        &self,
        session_id: SessionId,
        prompt: String,
        workspace_check: Option<std::path::PathBuf>,
    ) -> Result<EventStream<'_>> {
        if let Some(workspace) = workspace_check {
            let recorded = workspace.canonicalize().unwrap_or(workspace);
            if recorded != self.builder.workspace_root() {
                return Err(Error::config(format!(
                    "the native connector runs in {}, not {}",
                    self.builder.workspace_root().display(),
                    recorded.display()
                )));
            }
        }
        let taken = {
            let mut slots = self.slots.lock().expect("slots");
            slots
                .get_mut(&session_id)
                .and_then(|slot| slot.agent.take())
        };
        let mut agent = match taken {
            Some(agent) => agent,
            None => {
                // The caller's id names the session; it is created on first
                // use with the configured defaults.
                let existing = self.builder.store().get(&session_id).await?;
                match existing {
                    Some(session) if matches!(session.kind, SessionKind::Native { .. }) => {
                        self.builder.agent_for(session).await?
                    }
                    // A caller that tracks the session under its own row
                    // (the connector runner does) gets a native session of
                    // the same name behind it.
                    Some(session) => {
                        self.builder
                            .open(
                                SessionSelector::New {
                                    name: Some(session.name.clone()),
                                },
                                None,
                                None,
                                Some(session.phase),
                            )
                            .await?
                    }
                    None => {
                        self.builder
                            .open(
                                SessionSelector::Named(session_id.0.clone()),
                                None,
                                None,
                                None,
                            )
                            .await?
                    }
                }
            }
        };
        let handle = agent.handle();
        let native_id = agent.session().id.clone();
        {
            let mut slots = self.slots.lock().expect("slots");
            let slot = slots.entry(session_id.clone()).or_insert_with(|| Slot {
                agent: None,
                handle: None,
                state: TaskState::Idle,
                pending: None,
                approvals: HashMap::new(),
                last_error: None,
            });
            slot.handle = Some(handle);
            slot.state = TaskState::Running;
            slot.last_error = None;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let slots = Arc::clone(&self.slots);
        let mut ui = ChannelUi {
            tx,
            session_id: session_id.clone(),
            slots: Arc::clone(&self.slots),
            closed: false,
        };
        let _ = native_id;
        tokio::spawn(async move {
            let result = agent.run_turn(&prompt, &mut ui).await;
            let mut slots = slots.lock().expect("slots");
            if let Some(slot) = slots.get_mut(&session_id) {
                slot.state = match &result {
                    Ok(outcome) => match outcome.status {
                        TurnStatus::Completed => TaskState::Completed,
                        TurnStatus::Cancelled => TaskState::Cancelled,
                        TurnStatus::Failed => TaskState::Failed,
                    },
                    Err(_) => TaskState::Failed,
                };
                if let Err(error) = &result {
                    slot.last_error = Some(error.message.clone());
                }
                slot.handle = None;
                slot.approvals.clear();
                slot.agent = Some(agent);
            }
        });
        Ok(Box::pin(Receiver(rx)))
    }
}

impl Connector for NativeConnector {
    fn id(&self) -> ConnectorId {
        ConnectorId::Native
    }

    fn info(&self) -> BoxFuture<'_, Result<ConnectorInfo>> {
        Box::pin(async move {
            let auth = match self.builder.config.default_profile.as_deref() {
                Some(profile) => match self.builder.config.profiles.get(profile) {
                    Some(definition) => match self.builder.keys.key(profile, &definition.key) {
                        Ok(_) => AuthState::Authenticated,
                        Err(_) => AuthState::Unauthenticated,
                    },
                    None => AuthState::Unknown,
                },
                None => AuthState::Unauthenticated,
            };
            Ok(ConnectorInfo {
                id: ConnectorId::Native,
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                transport: Transport::InProcess,
                capabilities: ConnectorCapabilities {
                    follow_up_input: true,
                    approvals: true,
                    cancel: true,
                    resume: true,
                    inspect: true,
                    structured_events: true,
                },
                auth,
            })
        })
    }

    fn start(&self, request: TaskRequest) -> BoxFuture<'_, Result<EventStream<'_>>> {
        Box::pin(async move {
            self.run(request.session_id, request.prompt, Some(request.workspace))
                .await
        })
    }

    fn send_input(&self, session_id: &SessionId, input: String) -> BoxFuture<'_, Result<()>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let mut slots = self.slots.lock().expect("slots");
            let slot = slots
                .get_mut(&session_id)
                .ok_or_else(|| Error::connector("unknown native session"))?;
            slot.pending = Some(input);
            slot.state = TaskState::AwaitingInput;
            Ok(())
        })
    }

    fn answer_approval(
        &self,
        session_id: &SessionId,
        request_id: ApprovalId,
        decision: ApprovalDecision,
    ) -> BoxFuture<'_, Result<()>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let sender = {
                let mut slots = self.slots.lock().expect("slots");
                let slot = slots
                    .get_mut(&session_id)
                    .ok_or_else(|| Error::connector("unknown native session"))?;
                slot.state = TaskState::Running;
                slot.approvals.remove(&request_id)
            };
            match sender {
                Some(sender) => {
                    let _ = sender.send(decision);
                    Ok(())
                }
                None => Err(Error::connector(format!(
                    "no pending approval {} on this session",
                    request_id.0
                ))),
            }
        })
    }

    fn cancel(&self, session_id: &SessionId) -> BoxFuture<'_, Result<()>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let handle = {
                let slots = self.slots.lock().expect("slots");
                slots
                    .get(&session_id)
                    .ok_or_else(|| Error::connector("unknown native session"))?
                    .handle
                    .clone()
            };
            if let Some(handle) = handle {
                handle.cancel();
            }
            Ok(())
        })
    }

    fn resume(&self, session_id: &SessionId) -> BoxFuture<'_, Result<EventStream<'_>>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let input = {
                let mut slots = self.slots.lock().expect("slots");
                slots
                    .get_mut(&session_id)
                    .ok_or_else(|| Error::connector("unknown native session"))?
                    .pending
                    .take()
                    .ok_or_else(|| {
                        Error::connector("no follow-up input queued; call send_input first")
                    })?
            };
            self.run(session_id, input, None).await
        })
    }

    fn inspect(&self, session_id: &SessionId) -> BoxFuture<'_, Result<ConnectorInspection>> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let info = self.info().await?;
            let slots = self.slots.lock().expect("slots");
            let slot = slots
                .get(&session_id)
                .ok_or_else(|| Error::connector("unknown native session"))?;
            Ok(ConnectorInspection {
                session_id: session_id.clone(),
                external_id: None,
                state: slot.state,
                version: info.version,
                auth: info.auth,
                capabilities: info.capabilities,
                diagnostic: Some(serde_json::json!({
                    "last_error": slot.last_error,
                    "pending_approvals": slot.approvals.len(),
                })),
            })
        })
    }
}

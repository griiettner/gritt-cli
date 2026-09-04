//! `SessionStore` on the embedded Turso database. Sessions, their events,
//! and adapter continuation state live in the `gritt_` namespace. Event
//! payloads are the full serialized event; the store never derives
//! telemetry from them.

use chrono::{DateTime, Utc};
use gritt_core::event::Event;
use gritt_core::session::{
    BoxFuture, ContinuationState, Phase, Session, SessionId, SessionKind, SessionStore,
};
use gritt_core::{Error, Result};

use super::{storage_error, Store};

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Planning => "planning",
        Phase::Coding => "coding",
    }
}

fn parse_phase(text: &str) -> Result<Phase> {
    match text {
        "planning" => Ok(Phase::Planning),
        "coding" => Ok(Phase::Coding),
        other => Err(Error::storage(format!("unknown session phase `{other}`"))),
    }
}

fn parse_time(text: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| Error::storage(format!("invalid timestamp `{text}`: {error}")))
}

fn row_to_session(row: &turso::Row) -> Result<Session> {
    let kind: String = row.get(2).map_err(storage_error)?;
    let phase: String = row.get(3).map_err(storage_error)?;
    let workspace: String = row.get(4).map_err(storage_error)?;
    let parent: Option<String> = row.get(5).map_err(storage_error)?;
    let created: String = row.get(6).map_err(storage_error)?;
    let updated: String = row.get(7).map_err(storage_error)?;
    Ok(Session {
        id: SessionId(row.get(0).map_err(storage_error)?),
        name: row.get(1).map_err(storage_error)?,
        kind: serde_json::from_str::<SessionKind>(&kind)
            .map_err(|error| Error::storage(format!("invalid session kind: {error}")))?,
        phase: parse_phase(&phase)?,
        workspace: workspace.into(),
        created_at: parse_time(&created)?,
        updated_at: parse_time(&updated)?,
        parent_id: parent.map(SessionId),
    })
}

const SESSION_COLUMNS: &str = "id, name, kind, phase, workspace, parent_id, created_at, updated_at";

impl Store {
    /// Updates the phase and bumps `updated_at`.
    pub async fn set_phase(&self, id: &SessionId, phase: Phase) -> Result<()> {
        self.connection()
            .execute(
                "UPDATE gritt_sessions SET phase = ?1, updated_at = ?2 WHERE id = ?3",
                turso::params![phase_name(phase), Utc::now().to_rfc3339(), id.0.clone()],
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Records the phase the model was last told about, or clears it.
    pub async fn set_told_phase(&self, id: &SessionId, phase: Option<Phase>) -> Result<()> {
        self.connection()
            .execute(
                "UPDATE gritt_sessions SET told_phase = ?1 WHERE id = ?2",
                turso::params![phase.map(phase_name), id.0.clone()],
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// The phase the model was last told about, `None` when unknown.
    pub async fn told_phase(&self, id: &SessionId) -> Result<Option<Phase>> {
        let mut rows = self
            .connection()
            .query(
                "SELECT told_phase FROM gritt_sessions WHERE id = ?1",
                turso::params![id.0.clone()],
            )
            .await
            .map_err(storage_error)?;
        match rows.next().await.map_err(storage_error)? {
            Some(row) => {
                let text: Option<String> = row.get(0).map_err(storage_error)?;
                text.as_deref().map(parse_phase).transpose()
            }
            None => Ok(None),
        }
    }

    /// Finds a session by name, for `--session NAME` and `/resume NAME`.
    pub async fn find_by_name(&self, name: &str) -> Result<Option<Session>> {
        let mut rows = self
            .connection()
            .query(
                &format!("SELECT {SESSION_COLUMNS} FROM gritt_sessions WHERE name = ?1 ORDER BY updated_at DESC LIMIT 1"),
                turso::params![name],
            )
            .await
            .map_err(storage_error)?;
        match rows.next().await.map_err(storage_error)? {
            Some(row) => Ok(Some(row_to_session(&row)?)),
            None => Ok(None),
        }
    }

    /// Next free event sequence for a session.
    pub async fn next_sequence(&self, id: &SessionId) -> Result<u64> {
        let mut rows = self
            .connection()
            .query(
                "SELECT COALESCE(MAX(sequence), -1) FROM gritt_session_events WHERE session_id = ?1",
                turso::params![id.0.clone()],
            )
            .await
            .map_err(storage_error)?;
        let row = rows
            .next()
            .await
            .map_err(storage_error)?
            .ok_or_else(|| Error::storage("sequence query returned no row"))?;
        let max: i64 = row.get(0).map_err(storage_error)?;
        Ok(u64::try_from(max + 1).unwrap_or(0))
    }
}

impl SessionStore for Store {
    fn create(&self, session: Session) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let kind = serde_json::to_string(&session.kind)
                .map_err(|error| Error::storage(error.to_string()))?;
            self.connection()
                .execute(
                    &format!("INSERT INTO gritt_sessions ({SESSION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
                    turso::params![
                        session.id.0,
                        session.name,
                        kind,
                        phase_name(session.phase),
                        session.workspace.to_string_lossy().into_owned(),
                        session.parent_id.map(|id| id.0),
                        session.created_at.to_rfc3339(),
                        session.updated_at.to_rfc3339()
                    ],
                )
                .await
                .map_err(storage_error)?;
            Ok(())
        })
    }

    fn get(&self, id: &SessionId) -> BoxFuture<'_, Result<Option<Session>>> {
        let id = id.0.clone();
        Box::pin(async move {
            let mut rows = self
                .connection()
                .query(
                    &format!("SELECT {SESSION_COLUMNS} FROM gritt_sessions WHERE id = ?1"),
                    turso::params![id],
                )
                .await
                .map_err(storage_error)?;
            match rows.next().await.map_err(storage_error)? {
                Some(row) => Ok(Some(row_to_session(&row)?)),
                None => Ok(None),
            }
        })
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<Session>>> {
        Box::pin(async move {
            let mut rows = self
                .connection()
                .query(
                    &format!(
                        "SELECT {SESSION_COLUMNS} FROM gritt_sessions ORDER BY updated_at DESC, id"
                    ),
                    (),
                )
                .await
                .map_err(storage_error)?;
            let mut sessions = Vec::new();
            while let Some(row) = rows.next().await.map_err(storage_error)? {
                sessions.push(row_to_session(&row)?);
            }
            Ok(sessions)
        })
    }

    fn rename(&self, id: &SessionId, name: String) -> BoxFuture<'_, Result<()>> {
        let id = id.0.clone();
        Box::pin(async move {
            let changed = self
                .connection()
                .execute(
                    "UPDATE gritt_sessions SET name = ?1, updated_at = ?2 WHERE id = ?3",
                    turso::params![name, Utc::now().to_rfc3339(), id.clone()],
                )
                .await
                .map_err(storage_error)?;
            if changed == 0 {
                return Err(Error::storage(format!("no session with id `{id}`")));
            }
            Ok(())
        })
    }

    fn remove(&self, id: &SessionId) -> BoxFuture<'_, Result<()>> {
        let id = id.0.clone();
        Box::pin(async move {
            for sql in [
                "DELETE FROM gritt_session_events WHERE session_id = ?1",
                "DELETE FROM gritt_session_continuations WHERE session_id = ?1",
                "DELETE FROM gritt_sessions WHERE id = ?1",
            ] {
                self.connection()
                    .execute(sql, turso::params![id.clone()])
                    .await
                    .map_err(storage_error)?;
            }
            Ok(())
        })
    }

    fn append_events(&self, events: Vec<Event>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            for event in events {
                let payload = serde_json::to_string(&event)
                    .map_err(|error| Error::storage(error.to_string()))?;
                let source = serde_json::to_string(&event.source)
                    .map_err(|error| Error::storage(error.to_string()))?;
                let kind = serde_json::to_value(&event.kind)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("kind")
                            .and_then(|k| k.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "unknown".into());
                self.connection()
                    .execute(
                        "INSERT INTO gritt_session_events (session_id, sequence, source, kind, timestamp, payload)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        turso::params![
                            event.session_id.0.clone(),
                            i64::try_from(event.sequence).unwrap_or(i64::MAX),
                            source,
                            kind,
                            event.timestamp.to_rfc3339(),
                            payload
                        ],
                    )
                    .await
                    .map_err(storage_error)?;
                self.connection()
                    .execute(
                        "UPDATE gritt_sessions SET updated_at = ?1 WHERE id = ?2",
                        turso::params![event.timestamp.to_rfc3339(), event.session_id.0.clone()],
                    )
                    .await
                    .map_err(storage_error)?;
            }
            Ok(())
        })
    }

    fn read_events(&self, id: &SessionId) -> BoxFuture<'_, Result<Vec<Event>>> {
        let id = id.0.clone();
        Box::pin(async move {
            let mut rows = self
                .connection()
                .query(
                    "SELECT payload FROM gritt_session_events WHERE session_id = ?1 ORDER BY sequence",
                    turso::params![id],
                )
                .await
                .map_err(storage_error)?;
            let mut events = Vec::new();
            while let Some(row) = rows.next().await.map_err(storage_error)? {
                let payload: String = row.get(0).map_err(storage_error)?;
                events.push(
                    serde_json::from_str(&payload).map_err(|error| {
                        Error::storage(format!("invalid event payload: {error}"))
                    })?,
                );
            }
            Ok(events)
        })
    }

    fn save_continuation(
        &self,
        id: &SessionId,
        state: ContinuationState,
    ) -> BoxFuture<'_, Result<()>> {
        let id = id.0.clone();
        Box::pin(async move {
            let payload = serde_json::to_string(&state.state)
                .map_err(|error| Error::storage(error.to_string()))?;
            self.connection()
                .execute(
                    "INSERT INTO gritt_session_continuations (session_id, owner, state, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(session_id) DO UPDATE SET owner = excluded.owner,
                       state = excluded.state, updated_at = excluded.updated_at",
                    turso::params![id, state.owner, payload, Utc::now().to_rfc3339()],
                )
                .await
                .map_err(storage_error)?;
            Ok(())
        })
    }

    fn load_continuation(
        &self,
        id: &SessionId,
    ) -> BoxFuture<'_, Result<Option<ContinuationState>>> {
        let id = id.0.clone();
        Box::pin(async move {
            let mut rows = self
                .connection()
                .query(
                    "SELECT owner, state FROM gritt_session_continuations WHERE session_id = ?1",
                    turso::params![id],
                )
                .await
                .map_err(storage_error)?;
            match rows.next().await.map_err(storage_error)? {
                Some(row) => {
                    let owner: String = row.get(0).map_err(storage_error)?;
                    let state: String = row.get(1).map_err(storage_error)?;
                    Ok(Some(ContinuationState {
                        owner,
                        state: serde_json::from_str(&state).map_err(|error| {
                            Error::storage(format!("invalid continuation state: {error}"))
                        })?,
                    }))
                }
                None => Ok(None),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::DatabaseLocation;
    use gritt_core::connector::ConnectorId;
    use gritt_core::event::{EventKind, EventSource};

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(DatabaseLocation::Explicit(dir.path().join("t.db")))
            .await
            .unwrap();
        (dir, store)
    }

    fn session(id: &str, kind: SessionKind) -> Session {
        let now = Utc::now();
        Session {
            id: SessionId(id.into()),
            name: format!("name-{id}"),
            kind,
            phase: Phase::Planning,
            workspace: "/tmp/ws".into(),
            created_at: now,
            updated_at: now,
            parent_id: None,
        }
    }

    fn event(id: &str, sequence: u64, source: EventSource, kind: EventKind) -> Event {
        Event {
            session_id: SessionId(id.into()),
            sequence,
            source,
            timestamp: Utc::now(),
            kind,
            diagnostic: None,
        }
    }

    #[tokio::test]
    async fn native_and_connector_sessions_round_trip() {
        let (_dir, store) = store().await;
        let native = session(
            "n1",
            SessionKind::Native {
                provider_profile: "openrouter".into(),
                model: "openai/gpt-5-nano".into(),
            },
        );
        let connector = session(
            "c1",
            SessionKind::Connector {
                id: ConnectorId::Codex,
            },
        );
        store.create(native.clone()).await.unwrap();
        store.create(connector.clone()).await.unwrap();
        let listed = store.list().await.unwrap();
        assert_eq!(listed.len(), 2);
        let back = store.get(&native.id).await.unwrap().unwrap();
        assert_eq!(back.kind, native.kind);
        assert_eq!(back.phase, Phase::Planning);
        let back = store.get(&connector.id).await.unwrap().unwrap();
        assert_eq!(
            back.kind,
            SessionKind::Connector {
                id: ConnectorId::Codex
            }
        );

        store
            .append_events(vec![
                event(
                    "n1",
                    0,
                    EventSource::Native,
                    EventKind::TextDelta { text: "hi".into() },
                ),
                event(
                    "c1",
                    0,
                    EventSource::Connector {
                        id: ConnectorId::Codex,
                    },
                    EventKind::Cancelled,
                ),
            ])
            .await
            .unwrap();
        assert_eq!(store.next_sequence(&native.id).await.unwrap(), 1);
        let events = store.read_events(&connector.id).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Cancelled);

        store
            .save_continuation(
                &native.id,
                ContinuationState {
                    owner: "chat_completions".into(),
                    state: serde_json::json!({"messages": []}),
                },
            )
            .await
            .unwrap();
        store
            .save_continuation(
                &native.id,
                ContinuationState {
                    owner: "chat_completions".into(),
                    state: serde_json::json!({"messages": [1]}),
                },
            )
            .await
            .unwrap();
        let state = store.load_continuation(&native.id).await.unwrap().unwrap();
        assert_eq!(state.state["messages"][0], 1);

        store.set_phase(&native.id, Phase::Coding).await.unwrap();
        assert_eq!(
            store.get(&native.id).await.unwrap().unwrap().phase,
            Phase::Coding
        );
        assert_eq!(store.told_phase(&native.id).await.unwrap(), None);
        store
            .set_told_phase(&native.id, Some(Phase::Planning))
            .await
            .unwrap();
        assert_eq!(
            store.told_phase(&native.id).await.unwrap(),
            Some(Phase::Planning)
        );
        store.set_told_phase(&native.id, None).await.unwrap();
        assert_eq!(store.told_phase(&native.id).await.unwrap(), None);
        store.rename(&native.id, "renamed".into()).await.unwrap();
        assert_eq!(
            store.find_by_name("renamed").await.unwrap().unwrap().id,
            native.id
        );
        store.remove(&native.id).await.unwrap();
        assert!(store.get(&native.id).await.unwrap().is_none());
        assert!(store.read_events(&native.id).await.unwrap().is_empty());
        assert!(store.load_continuation(&native.id).await.unwrap().is_none());
        assert!(store.get(&connector.id).await.unwrap().is_some());
    }
}

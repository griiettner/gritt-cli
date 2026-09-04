//! Embedded Turso/libSQL database shared with `gritt-agent` memory.
//!
//! The memory tables belong to the tooling crate. This module only creates
//! `gritt_` prefixed tables and records each migration in
//! `gritt_schema_migrations`, so the two namespaces evolve independently in
//! one file (ADR-005, TKT-0008 plan).

use std::path::{Path, PathBuf};

use gritt_core::{Error, Result};
use turso::{Builder, Connection};

/// Ordered product migrations. Append; never edit an applied entry.
pub const MIGRATIONS: [(&str, &str); 1] =
    [("0001_product_tables", include_str!("product_schema.sql"))];

/// Tables and indexes owned by `gritt-agent`. The store must never touch
/// them.
pub const MEMORY_OBJECTS: [&str; 5] = [
    "documents",
    "document_chunks",
    "index_runs",
    "documents_turso_fts",
    "document_chunks_turso_fts",
];

const MIGRATIONS_TABLE: &str = "CREATE TABLE IF NOT EXISTS gritt_schema_migrations (
  name TEXT PRIMARY KEY,
  applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
)";

/// Where the database lives. A workspace with `.agents/` shares the
/// `gritt-agent` file; anything else uses the user data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseLocation {
    Explicit(PathBuf),
    Workspace(PathBuf),
    UserData(PathBuf),
}

impl DatabaseLocation {
    pub fn path(&self) -> &Path {
        match self {
            DatabaseLocation::Explicit(path)
            | DatabaseLocation::Workspace(path)
            | DatabaseLocation::UserData(path) => path,
        }
    }
}

pub fn resolve_location(workspace: &Path, explicit: Option<&Path>) -> Result<DatabaseLocation> {
    if let Some(path) = explicit {
        return Ok(DatabaseLocation::Explicit(path.to_path_buf()));
    }
    if workspace.join(".agents").is_dir() {
        return Ok(DatabaseLocation::Workspace(
            workspace
                .join(".agents")
                .join("brain")
                .join("data")
                .join("agent-memory.db"),
        ));
    }
    let base = dirs::data_dir()
        .ok_or_else(|| Error::storage("no user data directory is available on this platform"))?;
    Ok(DatabaseLocation::UserData(
        base.join("gritt").join("gritt.db"),
    ))
}

pub struct Store {
    connection: Connection,
    location: DatabaseLocation,
}

impl Store {
    /// Opens the database at `location`, creating the parent directory and
    /// applying pending product migrations.
    pub async fn open(location: DatabaseLocation) -> Result<Self> {
        if let Some(parent) = location.path().parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::storage(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
        let database = Builder::new_local(&location.path().to_string_lossy())
            .experimental_index_method(true)
            .build()
            .await
            .map_err(storage_error)?;
        let connection = database.connect().map_err(storage_error)?;
        let store = Self {
            connection,
            location,
        };
        store.migrate().await?;
        Ok(store)
    }

    pub fn location(&self) -> &DatabaseLocation {
        &self.location
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    async fn migrate(&self) -> Result<()> {
        self.connection
            .execute_batch(MIGRATIONS_TABLE)
            .await
            .map_err(storage_error)?;
        let applied = self.applied_migrations().await?;
        for (name, sql) in MIGRATIONS {
            if applied.iter().any(|done| done == name) {
                continue;
            }
            self.connection
                .execute_batch(sql)
                .await
                .map_err(storage_error)?;
            self.connection
                .execute(
                    "INSERT INTO gritt_schema_migrations (name) VALUES (?1)",
                    turso::params![name],
                )
                .await
                .map_err(storage_error)?;
        }
        Ok(())
    }

    pub async fn applied_migrations(&self) -> Result<Vec<String>> {
        let mut rows = self
            .connection
            .query("SELECT name FROM gritt_schema_migrations ORDER BY name", ())
            .await
            .map_err(storage_error)?;
        let mut names = Vec::new();
        while let Some(row) = rows.next().await.map_err(storage_error)? {
            names.push(row.get::<String>(0).map_err(storage_error)?);
        }
        Ok(names)
    }

    /// Names of every table and index in the file, for namespace checks.
    pub async fn object_names(&self) -> Result<Vec<String>> {
        let mut rows = self
            .connection
            .query(
                "SELECT name FROM sqlite_schema WHERE type IN ('table', 'index') ORDER BY name",
                (),
            )
            .await
            .map_err(storage_error)?;
        let mut names = Vec::new();
        while let Some(row) = rows.next().await.map_err(storage_error)? {
            names.push(row.get::<String>(0).map_err(storage_error)?);
        }
        Ok(names)
    }
}

pub fn storage_error(error: impl std::fmt::Display) -> Error {
    Error::storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gritt-agent memory schema, read from the tooling crate so the
    /// test proves both namespaces coexist in one file.
    const MEMORY_SCHEMA: &str = include_str!("../../../../.agents/cli/src/memory/schema.sql");

    fn temp_location(dir: &tempfile::TempDir) -> DatabaseLocation {
        DatabaseLocation::Explicit(dir.path().join("test.db"))
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let first = Store::open(temp_location(&dir)).await.unwrap();
        let applied = first.applied_migrations().await.unwrap();
        assert_eq!(applied, vec!["0001_product_tables".to_string()]);
        drop(first);
        let second = Store::open(temp_location(&dir)).await.unwrap();
        assert_eq!(second.applied_migrations().await.unwrap(), applied);
    }

    #[tokio::test]
    async fn product_tables_coexist_with_memory_schema() {
        let dir = tempfile::tempdir().unwrap();
        let location = temp_location(&dir);
        // Apply the memory schema first, the way gritt-agent would.
        {
            let database = Builder::new_local(&location.path().to_string_lossy())
                .experimental_index_method(true)
                .build()
                .await
                .unwrap();
            let connection = database.connect().unwrap();
            connection.execute_batch(MEMORY_SCHEMA).await.unwrap();
        }
        let store = Store::open(location).await.unwrap();
        let names = store.object_names().await.unwrap();
        for object in MEMORY_OBJECTS {
            assert!(
                names.iter().any(|n| n == object),
                "missing memory object {object}: {names:?}"
            );
        }
        for table in [
            "gritt_schema_migrations",
            "gritt_sessions",
            "gritt_session_events",
            "gritt_session_continuations",
            "gritt_telemetry_events",
            "gritt_analytics_records",
        ] {
            assert!(
                names.iter().any(|n| n == table),
                "missing product table {table}"
            );
        }
        // The memory schema still applies cleanly on top of the product tables.
        store
            .connection()
            .execute_batch(MEMORY_SCHEMA)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn product_namespace_round_trips_a_row() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(temp_location(&dir)).await.unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO gritt_sessions (id, name, kind, phase, workspace, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                turso::params!["s1", "first", "native", "planning", "/tmp/ws", "2026-09-04T00:00:00Z", "2026-09-04T00:00:00Z"],
            )
            .await
            .unwrap();
        let mut rows = store
            .connection()
            .query(
                "SELECT name, phase FROM gritt_sessions WHERE id = ?1",
                turso::params!["s1"],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "first");
        assert_eq!(row.get::<String>(1).unwrap(), "planning");
    }

    #[test]
    fn location_prefers_workspace_agents_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".agents")).unwrap();
        let location = resolve_location(dir.path(), None).unwrap();
        assert_eq!(
            location,
            DatabaseLocation::Workspace(dir.path().join(".agents/brain/data/agent-memory.db"))
        );
        let explicit = resolve_location(dir.path(), Some(Path::new("/x/y.db"))).unwrap();
        assert_eq!(
            explicit,
            DatabaseLocation::Explicit(PathBuf::from("/x/y.db"))
        );
    }
}

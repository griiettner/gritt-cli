//! Embedded Turso database for Gritt product state.
//!
//! The memory tables belong to the tooling crate. This module only creates
//! `gritt_` prefixed tables and records each migration in
//! `gritt_schema_migrations`. Product state uses a separate file by default
//! so a running memory service cannot lock Gritt out (ADR-005).

mod session_store;

use std::path::{Path, PathBuf};

use gritt_core::{Error, Result};
use turso::{Builder, Connection};

/// Ordered product migrations. Append; never edit an applied entry.
pub const MIGRATIONS: [(&str, &str); 3] = [
    ("0001_product_tables", include_str!("product_schema.sql")),
    ("0002_content_log", include_str!("content_log.sql")),
    (
        "0003_session_told_phase",
        include_str!("session_told_phase.sql"),
    ),
];

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

/// Where the database lives. Agent workspaces use their own product file;
/// anything else uses the user data directory.
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
                .join("gritt.db"),
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

    /// Applies every pending migration. Each migration's statements and its
    /// ledger row commit in one transaction, so an interruption leaves the
    /// file either fully before or fully after that migration. A column
    /// that an earlier interrupted run already added is detected and the
    /// `ALTER TABLE` skipped, so the ledger can still catch up.
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
            self.apply_migration(name, sql).await?;
        }
        Ok(())
    }

    async fn apply_migration(&self, name: &str, sql: &str) -> Result<()> {
        let statements = self.recoverable_statements(sql).await?;
        self.connection
            .execute_batch("BEGIN")
            .await
            .map_err(storage_error)?;
        let result = async {
            for statement in &statements {
                self.connection
                    .execute_batch(statement)
                    .await
                    .map_err(storage_error)?;
            }
            self.connection
                .execute(
                    "INSERT INTO gritt_schema_migrations (name) VALUES (?1)",
                    turso::params![name],
                )
                .await
                .map_err(storage_error)?;
            Ok::<(), Error>(())
        }
        .await;
        match result {
            Ok(()) => self
                .connection
                .execute_batch("COMMIT")
                .await
                .map_err(storage_error),
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK").await;
                Err(Error::storage(format!(
                    "migration {name} failed and was rolled back: {}",
                    error.message
                )))
            }
        }
    }

    /// Splits a migration into statements, dropping any `ALTER TABLE ...
    /// ADD COLUMN` whose column already exists. Comment lines are removed
    /// before the split so a semicolon inside a comment is not a boundary.
    async fn recoverable_statements(&self, sql: &str) -> Result<Vec<String>> {
        let without_comments: String = sql
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut statements = Vec::new();
        for raw in without_comments.split(';') {
            let statement = raw.trim();
            if statement.is_empty() {
                continue;
            }
            if let Some((table, column)) = added_column(statement) {
                if self.column_exists(&table, &column).await? {
                    continue;
                }
            }
            statements.push(statement.to_owned());
        }
        Ok(statements)
    }

    async fn column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let mut rows = self
            .connection
            .query(&format!("PRAGMA table_info({table})"), ())
            .await
            .map_err(storage_error)?;
        while let Some(row) = rows.next().await.map_err(storage_error)? {
            if row.get::<String>(1).map_err(storage_error)? == column {
                return Ok(true);
            }
        }
        Ok(false)
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

/// `(table, column)` when `statement` is `ALTER TABLE <t> ADD COLUMN <c> ...`.
fn added_column(statement: &str) -> Option<(String, String)> {
    let words: Vec<&str> = statement.split_whitespace().collect();
    match words.as_slice() {
        [alter, table_kw, table, add, column_kw, column, ..]
            if alter.eq_ignore_ascii_case("ALTER")
                && table_kw.eq_ignore_ascii_case("TABLE")
                && add.eq_ignore_ascii_case("ADD")
                && column_kw.eq_ignore_ascii_case("COLUMN") =>
        {
            Some((table.to_string(), column.to_string()))
        }
        _ => None,
    }
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
        assert_eq!(
            applied,
            vec![
                "0001_product_tables".to_string(),
                "0002_content_log".to_string(),
                "0003_session_told_phase".to_string()
            ]
        );
        drop(first);
        let second = Store::open(temp_location(&dir)).await.unwrap();
        assert_eq!(second.applied_migrations().await.unwrap(), applied);
    }

    #[tokio::test]
    async fn a_half_applied_column_migration_is_recovered_on_the_next_open() {
        // Simulate a crash after `ALTER TABLE` ran but before the ledger row
        // was written: the column exists, the ledger does not know it.
        let dir = tempfile::tempdir().unwrap();
        let location = temp_location(&dir);
        {
            let store = Store::open(location.clone()).await.unwrap();
            store
                .connection()
                .execute(
                    "DELETE FROM gritt_schema_migrations WHERE name = ?1",
                    turso::params!["0003_session_told_phase"],
                )
                .await
                .unwrap();
            assert!(store
                .column_exists("gritt_sessions", "told_phase")
                .await
                .unwrap());
        }
        let store = Store::open(location).await.unwrap();
        assert_eq!(
            store.applied_migrations().await.unwrap(),
            vec![
                "0001_product_tables".to_string(),
                "0002_content_log".to_string(),
                "0003_session_told_phase".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn a_failing_migration_leaves_no_ledger_row() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(temp_location(&dir)).await.unwrap();
        let error = store
            .apply_migration(
                "9999_broken",
                "CREATE TABLE gritt_x (id TEXT); SELECT * FROM no_such_table;",
            )
            .await
            .unwrap_err();
        assert!(error.message.contains("9999_broken"));
        assert!(!store
            .applied_migrations()
            .await
            .unwrap()
            .iter()
            .any(|name| name == "9999_broken"));
    }

    #[test]
    fn added_column_parses_alter_statements_only() {
        assert_eq!(
            added_column("ALTER TABLE gritt_sessions ADD COLUMN told_phase TEXT"),
            Some(("gritt_sessions".into(), "told_phase".into()))
        );
        assert_eq!(added_column("CREATE TABLE x (id TEXT)"), None);
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
            DatabaseLocation::Workspace(dir.path().join(".agents/brain/data/gritt.db"))
        );
        let explicit = resolve_location(dir.path(), Some(Path::new("/x/y.db"))).unwrap();
        assert_eq!(
            explicit,
            DatabaseLocation::Explicit(PathBuf::from("/x/y.db"))
        );
    }
}

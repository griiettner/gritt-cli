//! Turso connection and schema management for the local memory database.

use std::fs;
use std::path::{Path, PathBuf};

use turso::{Builder, Connection};

use crate::Result;

pub const SCHEMA: &str = include_str!("schema.sql");

pub fn database_path(repo: &Path) -> PathBuf {
    repo.join(".agents")
        .join("brain")
        .join("data")
        .join("agent-memory.db")
}

/// Opens the database, creating the data directory and schema when needed.
pub async fn open(repo: &Path) -> Result<Connection> {
    let path = database_path(repo);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let database = Builder::new_local(&path.to_string_lossy())
        .experimental_index_method(true)
        .build()
        .await?;
    let connection = database.connect()?;
    connection.execute_batch(SCHEMA).await?;
    ensure_migrations(&connection).await?;
    Ok(connection)
}

/// Opens an in-memory database with the schema applied. Used by tests.
pub async fn open_in_memory() -> Result<Connection> {
    let database = Builder::new_local(":memory:")
        .experimental_index_method(true)
        .build()
        .await?;
    let connection = database.connect()?;
    connection.execute_batch(SCHEMA).await?;
    Ok(connection)
}

async fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut rows = connection
        .query(format!("PRAGMA table_info({table})"), ())
        .await?;
    while let Some(row) = rows.next().await? {
        if row.get::<String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn ensure_migrations(connection: &Connection) -> Result<()> {
    if !table_has_column(connection, "document_chunks", "embedding").await? {
        connection
            .execute_batch("ALTER TABLE document_chunks ADD COLUMN embedding F32_BLOB(1536)")
            .await?;
    }
    Ok(())
}

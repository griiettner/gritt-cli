//! SQLite connection and schema management for the local memory database.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::Result;

pub const SCHEMA: &str = include_str!("schema.sql");

pub fn database_path(repo: &Path) -> PathBuf {
    repo.join(".agents")
        .join("brain")
        .join("data")
        .join("agent-memory.db")
}

/// Opens the database, creating the data directory and schema when needed.
pub fn open(repo: &Path) -> Result<Connection> {
    let path = database_path(repo);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.execute_batch(SCHEMA)?;
    ensure_migrations(&connection)?;
    Ok(connection)
}

/// Opens an in-memory database with the schema applied. Used by tests.
pub fn open_in_memory() -> Result<Connection> {
    let connection = Connection::open_in_memory()?;
    connection.execute_batch(SCHEMA)?;
    Ok(connection)
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_migrations(connection: &Connection) -> Result<()> {
    if !table_has_column(connection, "document_chunks", "embedding")? {
        connection
            .execute_batch("ALTER TABLE document_chunks ADD COLUMN embedding F32_BLOB(1536)")?;
    }
    Ok(())
}

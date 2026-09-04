//! Workspace indexer: walks supported documents into the Turso database.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use turso::{params, Connection};

use super::chunk::chunk_document;
use super::db;
use crate::fsx::{self, relative_posix};
use crate::{CliError, Result};

const ALLOWED_EXTENSIONS: [&str; 5] = ["md", "mdx", "yaml", "yml", "json"];
const SKIPPED_DIRS: [&str; 8] = [
    "node_modules",
    ".git",
    ".nx",
    ".playwright-mcp",
    "dist",
    "coverage",
    ".output",
    "target",
];

#[derive(Debug, Clone)]
pub struct IndexSummary {
    pub files: usize,
    pub database: PathBuf,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest.iter() {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Collects indexable files in deterministic order.
pub fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let brain_data = root.join(".agents").join("brain").join("data");
    let mut files = Vec::new();
    collect_into(root, &brain_data, &mut files)?;
    Ok(files)
}

fn collect_into(dir: &Path, brain_data: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fsx::list_entries(dir)? {
        if SKIPPED_DIRS.contains(&entry.name.as_str()) {
            continue;
        }
        // Following either kind can escape the repository and expose local
        // files through the memory database.
        if entry.is_symlink {
            continue;
        }
        if entry.is_dir {
            if entry.path == brain_data {
                continue;
            }
            collect_into(&entry.path, brain_data, files)?;
        } else if entry.is_file {
            let extension = entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if ALLOWED_EXTENSIONS.contains(&extension) {
                files.push(entry.path);
            }
        }
    }
    Ok(())
}

fn source_mtime_ms(path: &Path) -> i64 {
    let modified = fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now());
    modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn index_file(connection: &Connection, root: &Path, file: &Path) -> Result<()> {
    let bytes = fs::read(file)?;
    let content = String::from_utf8_lossy(&bytes).into_owned();
    let relative = relative_posix(root, file);
    let title = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let content_hash = sha256_hex(content.as_bytes());
    let source_mtime = source_mtime_ms(file);

    let mut existing_rows = connection
        .query(
            "SELECT content_hash FROM documents WHERE path = ?1",
            params![relative.clone()],
        )
        .await?;
    let existing_hash = match existing_rows.next().await? {
        Some(row) => Some(row.get::<String>(0)?),
        None => None,
    };
    let unchanged = existing_hash.as_deref() == Some(content_hash.as_str());

    connection
        .execute(
            "INSERT INTO documents(path, title, content, content_hash, source_mtime)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(path) DO UPDATE SET
           title = excluded.title,
           content = excluded.content,
           content_hash = excluded.content_hash,
           source_mtime = excluded.source_mtime,
           updated_at = CURRENT_TIMESTAMP
         WHERE documents.content_hash <> excluded.content_hash",
            params![
                relative.clone(),
                title,
                content.clone(),
                content_hash.clone(),
                source_mtime
            ],
        )
        .await?;
    if unchanged {
        return Ok(());
    }

    let mut rows = connection
        .query(
            "SELECT id FROM documents WHERE path = ?1",
            params![relative],
        )
        .await?;
    let document_id = rows
        .next()
        .await?
        .ok_or_else(|| CliError::new("indexed document row is missing"))?
        .get::<i64>(0)?;
    let chunks = chunk_document(&content);
    connection
        .execute(
            "DELETE FROM document_chunks WHERE document_id = ?1",
            params![document_id],
        )
        .await?;
    let mut upsert = connection
        .prepare_cached(
            "INSERT INTO document_chunks(
           document_id, chunk_index, heading, start_line, end_line, content, content_hash
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .await?;
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        upsert
            .execute(params![
                document_id,
                chunk_index as i64,
                chunk.heading.clone(),
                chunk.start_line as i64,
                chunk.end_line as i64,
                chunk.content.clone(),
                sha256_hex(chunk.content.as_bytes()),
            ])
            .await?;
    }
    Ok(())
}

/// Indexes the workspace incrementally and records the run.
pub async fn index_workspace(repo: &Path) -> Result<IndexSummary> {
    let connection = db::open(repo).await?;
    connection
        .execute("INSERT INTO index_runs(status) VALUES ('running')", ())
        .await?;
    let run_id = connection.last_insert_rowid();
    let files = collect_files(repo)?;

    match index_files(&connection, repo, &files).await {
        Ok(()) => {
            connection
                .execute(
                    "UPDATE index_runs
                 SET completed_at = CURRENT_TIMESTAMP, files_seen = ?1, status = 'completed'
                 WHERE id = ?2",
                    params![files.len() as i64, run_id],
                )
                .await?;
            Ok(IndexSummary {
                files: files.len(),
                database: db::database_path(repo),
            })
        }
        Err(error) => {
            connection.execute(
                "UPDATE index_runs
                 SET completed_at = CURRENT_TIMESTAMP, files_seen = ?1, status = 'failed', error = ?2
                 WHERE id = ?3",
                params![files.len() as i64, error.message.clone(), run_id],
            ).await?;
            Err(CliError::new(format!("indexing failed: {}", error.message)))
        }
    }
}

async fn index_files(connection: &Connection, repo: &Path, files: &[PathBuf]) -> Result<()> {
    connection.execute_batch("BEGIN").await?;
    let result =
        async {
            connection
                .execute_batch(
                    "CREATE TEMP TABLE IF NOT EXISTS seen_paths(path TEXT PRIMARY KEY);
             DELETE FROM seen_paths;",
                )
                .await?;
            {
                let mut insert = connection
                    .prepare_cached("INSERT OR IGNORE INTO seen_paths(path) VALUES (?1)")
                    .await?;
                for file in files {
                    insert.execute(params![relative_posix(repo, file)]).await?;
                }
            }
            connection
                .execute(
                    "DELETE FROM documents WHERE path NOT IN (SELECT path FROM seen_paths)",
                    (),
                )
                .await?;
            connection.execute(
            "DELETE FROM document_chunks WHERE document_id NOT IN (SELECT id FROM documents)",
            (),
        ).await?;
            for file in files {
                index_file(connection, repo, file).await?;
            }
            Ok::<(), CliError>(())
        }
        .await;
    match result {
        Ok(()) => {
            connection.execute_batch("COMMIT").await?;
            Ok(())
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK").await;
            Err(error)
        }
    }
}

/// Prints the index summary the way the replaced script did.
pub fn report(summary: &IndexSummary) {
    eprintln!(
        "Indexed {} local knowledge files into {}",
        summary.files,
        summary.database.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_hex_sha256() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

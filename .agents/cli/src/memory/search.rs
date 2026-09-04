//! FTS5 retrieval over indexed chunks.

use rusqlite::{params, Connection};

use crate::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub title: String,
    pub heading: Option<String>,
    pub start_line: i64,
    pub end_line: i64,
    pub content: String,
    pub score: f64,
    pub chunk_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub path: String,
    pub title: String,
    pub content: String,
}

/// Turns free text into an FTS5 query: each term becomes a quoted phrase
/// and every term must match.
pub fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| {
            term.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect::<String>()
        })
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

pub fn search(connection: &Connection, query: &str, limit: usize) -> Result<Vec<Hit>> {
    let normalized = normalize_query(query);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(
        "SELECT d.path, d.title, c.id AS chunk_id, c.heading, c.start_line, c.end_line,
                c.content, bm25(document_chunks_fts) AS score
         FROM document_chunks_fts
         JOIN document_chunks c ON c.id = document_chunks_fts.rowid
         JOIN documents d ON d.id = c.document_id
         WHERE document_chunks_fts MATCH ?1
         ORDER BY score, c.id
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![normalized, limit as i64], |row| {
        Ok(Hit {
            path: row.get(0)?,
            title: row.get(1)?,
            chunk_id: row.get(2)?,
            heading: row.get(3)?,
            start_line: row.get(4)?,
            end_line: row.get(5)?,
            content: row.get(6)?,
            score: row.get(7)?,
        })
    })?;
    let mut hits = Vec::new();
    for row in rows {
        hits.push(row?);
    }
    Ok(hits)
}

pub fn read_document(connection: &Connection, path: &str) -> Result<Option<Document>> {
    let mut statement = connection
        .prepare_cached("SELECT path, title, content FROM documents WHERE path = ?1 LIMIT 1")?;
    let mut rows = statement.query(params![path])?;
    match rows.next()? {
        Some(row) => Ok(Some(Document {
            path: row.get(0)?,
            title: row.get(1)?,
            content: row.get(2)?,
        })),
        None => Ok(None),
    }
}

/// Renders hits as numbered citations with `path:start-end` sources.
pub fn format_hits(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return "No local knowledge matched the query.".to_owned();
    }
    hits.iter()
        .enumerate()
        .map(|(index, hit)| {
            let heading = hit
                .heading
                .as_deref()
                .map(|h| format!(" — {h}"))
                .unwrap_or_default();
            format!(
                "[{}] {}{heading}\nSource: {}:{}-{}\n\n{}",
                index + 1,
                hit.title,
                hit.path,
                hit.start_line,
                hit.end_line,
                hit.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

pub fn format_document(path: &str, document: Option<&Document>) -> String {
    match document {
        Some(doc) => format!("## {}\nPath: {}\n\n{}", doc.title, doc.path, doc.content),
        None => format!("No local document exists at {path}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    #[test]
    fn normalizes_queries() {
        assert_eq!(normalize_query("hello, world!"), "\"hello\" AND \"world\"");
        assert_eq!(normalize_query("  "), "");
        assert_eq!(
            normalize_query("ticket-id under_score"),
            "\"ticket-id\" AND \"under_score\""
        );
    }

    #[test]
    fn searches_indexed_chunks() {
        let connection = db::open_in_memory().unwrap();
        connection
            .execute(
                "INSERT INTO documents(path, title, content, content_hash, source_mtime)
                 VALUES ('docs/alpha.md', 'alpha', 'FTS fallback memory content', 'h', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO document_chunks(document_id, chunk_index, heading, start_line, end_line, content, content_hash)
                 VALUES (1, 0, 'Intro', 1, 1, 'FTS fallback memory content', 'c')",
                [],
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO document_chunks_fts(document_chunks_fts) VALUES ('rebuild')",
            )
            .unwrap();
        let hits = search(&connection, "FTS fallback", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "docs/alpha.md");
        assert!(format_hits(&hits).contains("Source: docs/alpha.md:1-1"));
        assert!(search(&connection, "missing", 5).unwrap().is_empty());
    }
}

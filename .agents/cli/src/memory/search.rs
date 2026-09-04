//! Turso FTS retrieval over indexed chunks.

use turso::{params, Connection};

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

/// Turns free text into an FTS query: each term becomes a quoted phrase
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

pub async fn search(connection: &Connection, query: &str, limit: usize) -> Result<Vec<Hit>> {
    let normalized = normalize_query(query);
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows = connection
        .query(
            "SELECT d.path, d.title, c.id AS chunk_id, c.heading, c.start_line, c.end_line,
                c.content, fts_score(c.heading, c.content, ?1) AS score
         FROM document_chunks c
         JOIN documents d ON d.id = c.document_id
         WHERE fts_match(c.heading, c.content, ?1)
         ORDER BY score, c.id",
            params![normalized],
        )
        .await?;
    let mut hits = Vec::new();
    while let Some(row) = rows.next().await? {
        hits.push(Hit {
            path: row.get(0)?,
            title: row.get(1)?,
            chunk_id: row.get(2)?,
            heading: row.get(3)?,
            start_line: row.get(4)?,
            end_line: row.get(5)?,
            content: row.get(6)?,
            score: row.get(7)?,
        });
    }
    let terms = query_terms(query);
    hits.sort_by(|left, right| {
        heading_rank(left, &terms)
            .cmp(&heading_rank(right, &terms))
            .then_with(|| left.score.total_cmp(&right.score))
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| {
            term.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|term| !term.is_empty())
        .collect()
}

fn heading_rank(hit: &Hit, terms: &[String]) -> u8 {
    match hit.heading.as_deref() {
        Some(heading)
            if terms
                .iter()
                .all(|term| heading.to_lowercase().contains(term)) =>
        {
            0
        }
        Some(_) => 1,
        None => 2,
    }
}

pub async fn read_document(connection: &Connection, path: &str) -> Result<Option<Document>> {
    let mut rows = connection
        .query(
            "SELECT path, title, content FROM documents WHERE path = ?1 LIMIT 1",
            params![path],
        )
        .await?;
    match rows.next().await? {
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

    #[tokio::test]
    async fn searches_indexed_chunks() {
        let connection = db::open_in_memory().await.unwrap();
        connection
            .execute(
                "INSERT INTO documents(path, title, content, content_hash, source_mtime)
                 VALUES ('docs/alpha.md', 'alpha', 'FTS fallback memory content', 'h', 0)",
                (),
            )
            .await
            .unwrap();
        connection
            .execute(
                "INSERT INTO document_chunks(document_id, chunk_index, heading, start_line, end_line, content, content_hash)
                 VALUES (1, 0, 'Intro', 1, 1, 'FTS fallback memory content', 'c')",
                (),
            )
            .await
            .unwrap();
        let hits = search(&connection, "FTS fallback", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "docs/alpha.md");
        assert!(format_hits(&hits).contains("Source: docs/alpha.md:1-1"));
        assert!(search(&connection, "missing", 5).await.unwrap().is_empty());
    }
}

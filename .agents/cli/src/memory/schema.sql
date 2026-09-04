CREATE TABLE IF NOT EXISTS documents (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  title TEXT NOT NULL,
  content TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  source_mtime INTEGER NOT NULL,
  embedding F32_BLOB(1536),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS document_chunks (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL,
  chunk_index INTEGER NOT NULL,
  heading TEXT,
  start_line INTEGER NOT NULL,
  end_line INTEGER NOT NULL,
  content TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  embedding F32_BLOB(1536),
  UNIQUE(document_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS documents_turso_fts ON documents USING fts(title, content)
  WITH (weights = 'title=10.0,content=1.0');

CREATE INDEX IF NOT EXISTS document_chunks_turso_fts ON document_chunks USING fts(heading, content)
  WITH (weights = 'heading=10.0,content=1.0');

CREATE TABLE IF NOT EXISTS index_runs (
  id INTEGER PRIMARY KEY,
  started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  completed_at TEXT,
  files_seen INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'running',
  error TEXT
);

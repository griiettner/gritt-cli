-- Gritt product namespace. Every table is `gritt_` prefixed so it can share
-- a database file with the gritt-agent memory tables (documents,
-- document_chunks, index_runs) without touching them.

CREATE TABLE IF NOT EXISTS gritt_sessions (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  phase TEXT NOT NULL,
  workspace TEXT NOT NULL,
  parent_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS gritt_session_events (
  session_id TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  source TEXT NOT NULL,
  kind TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  payload TEXT NOT NULL,
  PRIMARY KEY (session_id, sequence)
);

CREATE TABLE IF NOT EXISTS gritt_session_continuations (
  session_id TEXT PRIMARY KEY,
  owner TEXT NOT NULL,
  state TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS gritt_telemetry_events (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  session_id TEXT,
  timestamp TEXT NOT NULL,
  duration_ms INTEGER,
  status TEXT,
  counters TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS gritt_analytics_records (
  id INTEGER PRIMARY KEY,
  metric TEXT NOT NULL,
  session_id TEXT,
  timestamp TEXT NOT NULL,
  value INTEGER NOT NULL,
  labels TEXT NOT NULL DEFAULT '{}'
);

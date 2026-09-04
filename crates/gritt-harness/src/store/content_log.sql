-- Opt-in content log (ADR-008). Only written when `logging.content_logging`
-- is on; rows older than the retention window are purged on open.

CREATE TABLE IF NOT EXISTS gritt_content_log (
  id INTEGER PRIMARY KEY,
  session_id TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL
);

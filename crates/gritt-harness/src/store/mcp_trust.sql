-- First-use approval for MCP servers, keyed by the exact workspace, server
-- name, and definition fingerprint. A changed definition produces a new
-- fingerprint, so the old row no longer matches and the user is asked again.
CREATE TABLE IF NOT EXISTS gritt_mcp_trust (
  workspace TEXT NOT NULL,
  server TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  decision TEXT NOT NULL,
  decided_at TEXT NOT NULL,
  PRIMARY KEY (workspace, server, fingerprint)
);

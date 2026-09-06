-- The native profile, model, and effort of the last successful new session
-- per workspace, so later new sessions can start from the same choices.
-- Never a credential: profile names, model ids, and an effort label only.

CREATE TABLE IF NOT EXISTS gritt_last_used (
  workspace TEXT PRIMARY KEY,
  provider_profile TEXT NOT NULL,
  model TEXT NOT NULL,
  effort TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- The phase the model was last told about. NULL until the first request
-- of a session goes out, and after a phase change that no turn has yet
-- announced, so a resumed agent knows whether to send the transition note.

ALTER TABLE gritt_sessions ADD COLUMN told_phase TEXT;

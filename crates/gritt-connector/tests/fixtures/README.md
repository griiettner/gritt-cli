# Connector fixtures

One JSONL file per case, replayed through the fake agent in
`../fake-agent/agent.sh` so the supervisor and the normalizers run exactly as
they do against the real CLI.

Origin of each file:

- `codex/`: captured on 2026-09-04 from `codex exec --json` 0.153.2 (`text`
  and `tool`); `error` and `malformed` are hand-authored from the same shapes.
- `claude/`: captured on 2026-09-04 from `claude -p --output-format stream-json
  --verbose` 2.1.260 (`text` and `tool`, trimmed to the fields the normalizer
  reads); `error` is hand-authored from the `result` shape.
- `opencode/`: captured on 2026-09-04 from `opencode run --format json` 1.15.4
  (`text` and `tool`); `error` is hand-authored.
- `cursor/`: hand-authored from the published `cursor-agent -p --output-format
  stream-json` format. The CLI is not installed on the development machine, so
  nothing here is a live recording.

Session, thread, and request identifiers were replaced with fixed placeholder
values. No credential, account identifier, or base64 payload is present.

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

`mcp/` holds each connector's own MCP inventory listing, read by the
parsers under `src/protocols/`:

- `mcp/codex/list.json`: shape of `codex mcp list --json` 0.153.x captured
  on 2026-09-06, with paths, names, and values replaced by placeholders and
  an `env`, `http_headers`, and query-string value added so the tests can
  prove none of them is kept.
- `mcp/claude/list.txt`: shape of `claude mcp list` 2.1.x captured on
  2026-09-06 (`Connected`, `Pending approval`, `Failed to connect`), with a
  `Needs authentication` line and a credential-bearing argument added by
  hand. `empty.txt` and `malformed.txt` are hand-authored.
- `mcp/opencode/list.txt` and `empty.txt`: shape of `opencode mcp list`
  1.18.29 captured on 2026-09-06 against a scratch project config, with a
  `connected (OAuth)` line added from the CLI source. `malformed.txt` is
  hand-authored.

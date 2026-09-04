# Connectors

A connector runs an installed agent through the same sessions, transcript,
telemetry, and modes as the native path. The native path is itself the
first connector, so the control plane never special-cases it. Order and
authority follow ADR-010.

```bash
gritt connectors
gritt run --connector codex "…"
gritt repl --connector claude
gritt tui --connector opencode
```

| Connector | `--connector` | Interface | State on this machine |
| --- | --- | --- | --- |
| Native | `native` | in process | always available |
| Codex | `codex` | `codex exec --json` | live-tested |
| Claude Code | `claude` | `claude -p --output-format stream-json --verbose` | live-tested |
| OpenCode | `opencode` | `opencode run --format json` | runs, auth unknown |
| Cursor | `cursor` | `cursor-agent -p --output-format stream-json` | fixtures only, CLI not installed |

## Authority

The external agent keeps its own command and tool authority. Gritt
launches it, supervises the process, normalizes its output into the shared
event model, records the session, and handles follow-up input, resume,
cancellation, health checks, and process failure. Gritt does not re-run the
agent's tools or second-guess its permissions.

## Approvals are the agent's own

The headless interfaces of all four external agents apply the agent's own
approval policy and do not expose approvals to a supervisor. Gritt shows
this instead of faking it: `gritt connectors` lists `own-approvals`, and
the native `--approve-all`, `--deny-all`, and `--ask` flags print a warning
and have no effect with an external connector. Pass the agent's own
permission flags through the config:

```toml
[connectors.extra_args]
codex = ["--full-auto"]
claude = ["--permission-mode", "acceptEdits"]
```

An `extra_args` entry that carries a credential (an option named like a
key, token, secret, or password, or any value matching a known secret) is
refused, because arguments appear in process lists and diagnostics.

## Planning is a request

A planning session on a connector prefixes a planning request to the
prompt. It asks the agent to discuss rather than act; it does not remove
the agent's tools. Use the native path when planning must be tool-free.

## Settings

```toml
[connectors]
# Executable overrides, by connector id.
executables = { codex = "/opt/codex/bin/codex" }
# Use a pseudo-terminal instead of pipes for these connectors.
pty = ["opencode"]
health_check_timeout_secs = 10
task_timeout_secs = 1800
```

Every field has a default; a partial section is valid.

## Supervision

Startup, idle, and task timeouts (the first two are built in, the task
timeout is configurable); health checks (version and auth probes
before a task starts); cancellation that kills the process tree; process
exit, non-zero exit, missing executable, outdated version, and malformed
output are surfaced as normalized error or status events. A terminal event
followed by a non-zero exit is reported as an error. After a terminal event
the process is drained until it exits, bounded by one deadline. Raw
connector output is kept in event diagnostics for troubleshooting,
key-redacted.

A missing or broken external agent fails only its own sessions and never
creates a session row. The native path is unaffected.

## PTY fallback

Machine-readable output is preferred. A connector listed under
`[connectors] pty` runs in a pseudo-terminal instead of pipes, for agents
whose structured mode needs one. Terminal scraping is not implemented; the
PTY path still parses the agent's structured output.

## Live tests

`GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`
runs a trivial task through the real Codex and Claude Code CLIs when they
are installed and authenticated. Fixture tests cover the same behavior
without them and are never skipped.

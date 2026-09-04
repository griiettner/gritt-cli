---
id: TKT-0012
namespace: griiettner
title: Implement supervised native and external connectors with PTY fallback, live Codex and Claude Code tests, and normalized events
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0012 Report: Implement supervised native and external connectors with PTY fallback, live Codex and Claude Code tests, and normalized events

## Summary

Worker 4 of the TKT-0008 chain. Installed agents now run through the same
session store, interface, telemetry, and event model as the native loop,
while keeping their own command and tool authority (ADR-010).

Chain facts:

- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0012`
- Branch: `tkt-0012-04-connectors`
- Base: `feature/tkt-0008-gritt-cli` at `fb2d9bb` (PR #3 merge)
- Commits: `ef2a974` (implementation, fixtures, ticket artifacts) and the
  report update commit that follows it
- PR: https://github.com/griiettner/gritt-cli/pull/4 into
  `feature/tkt-0008-gritt-cli`

What landed:

- `crates/gritt-connector`: `process.rs` launches an agent through pipes
  with its own process group, reads stdout and stderr as lines, and kills
  the process tree with the platform's own `kill` or `taskkill`;
  `pty.rs` is the PTY fallback on `portable-pty`; `supervise.rs` holds the
  generic `ExternalConnector<P: Protocol>` with startup and idle timeouts,
  cancellation, malformed-line and unknown-event diagnostics, exit
  handling, key redaction, session state, `inspect`, and follow-up input
  through `send_input` plus `resume`; `health.rs` finds executables and
  runs the version and auth probes; `protocols/` maps Codex, Claude Code,
  OpenCode, and Cursor output to events.
- `crates/gritt-harness`: `driver.rs` (`Driver`, one interface over a
  native agent and a connector session), `connector_session.rs` (runs a
  connector through the store, the `Ui`, approval relay, telemetry, and
  continuation), `native_connector.rs` (the native loop behind the
  `Connector` contract, with approvals answered through
  `answer_approval`), and `control.rs` (`ControlPlane`: picks the backend,
  reports every connector, opens sessions). The REPL and the full-screen
  mode take a `Box<dyn Driver>` and resume through the control plane.
- `crates/gritt`: `--connector codex|claude|cursor|opencode` on `run`,
  `repl`, and `tui`; `gritt connectors` lists installed state, version,
  transport, and capabilities; `session list` shows `connector:<id>`.
- `gritt-core`: `TaskRequest.continuation` (optional, serde default) and
  `ConnectorSettings.pty` and `extra_args` (both serde default). Both are
  additive.

Interfaces targeted, all machine-readable:

| Connector | Version on this machine | Launch | Resume |
| --- | --- | --- | --- |
| Codex | 0.153.2 | `codex exec --json --skip-git-repo-check -C <ws> <prompt>` | `codex exec resume --json --skip-git-repo-check <thread_id> <prompt>` |
| Claude Code | 2.1.260 | `claude -p --output-format stream-json --verbose <prompt>` | `--resume <session_id>` |
| OpenCode | 1.15.4 | `opencode run --format json --dir <ws> <message>` | `--session <id>` |
| Cursor | not installed | `cursor-agent -p --output-format stream-json <prompt>` | `--resume <session_id>` |

Version probes use `--version`; auth probes use `codex login status`,
`claude auth status`, `opencode auth list`, and `cursor-agent status`.

## Key Decisions

- Approvals are a shown difference, not a faked one. All four headless
  interfaces apply the agent's own permission policy and expose no prompt,
  so `capabilities.approvals` is false, `answer_approval` returns a
  connector error naming that, and `gritt connectors` prints
  `own-approvals`. The user selects the agent's mode through
  `[connectors.extra_args]`. The native connector relays approvals.
- External agents keep their full environment. Stripping credential
  variables, as the native shell tool does, would break the agents' own
  authentication. Instead every profile key the resolver can produce and
  every value of a credential-like variable in Gritt's own environment
  (the `gritt-core` name rule the shell tool also uses) is redacted out of
  connector events and diagnostics, once in the connector and again in the
  harness runner. Launch diagnostics record `[prompt]` for the prompt and
  `name=[redacted]` for any `name=value` argument. A credential-bearing
  `[connectors.extra_args]` entry is refused at startup with a config
  error that names the flag, never the value.
- The native `--approve-all`, `--deny-all`, and `--ask` flags are accepted
  with an external connector but print a warning on stderr that the agent
  applies its own approval policy and that `[connectors.extra_args]` is
  where its permission flags go.
- The agent's terminal event is a verdict, not a stop signal. The driver
  keeps reading until the process exits (bounded by the idle timeout, at
  most ten seconds after the terminal event), so a trailing error is
  still emitted; an error terminal keeps the session failed, and a
  completion followed by a non-zero exit becomes an error naming the
  status. `inspect()` reports that real state, with `last_error`
  key-redacted.
- The PTY transport never blocks while holding the child handle: exit is
  polled with `try_wait`, so `kill` and `wait` cannot wedge each other when
  an agent lingers after finishing.
- The child's stdin is closed. Codex blocks on an open stdin pipe until it
  sees end-of-input; every supported agent takes its prompt as an
  argument, and follow-up input is a new turn on the agent's own thread.
- The `NativeConnector` lives in `gritt-harness` because the connector
  crate depends on `gritt-core` only (ADR-006). The control plane hands
  modes a `NativeAgent` for native sessions, which keeps diff previews and
  policy reasons, and a `ConnectorSession` for connector sessions. The
  native connector is exercised through the contract in tests and stays
  available for an in-process client (ADR-011).
- A connector turn ends when the connector closes its stream, not on the
  first completion event: the native path streams an intermediate
  completion before its tool phase, and an external agent may report an
  error after one.
- Cursor is looked up only as `cursor-agent`. A bare `agent` on this
  machine belongs to another tool.
- Planning turns on a connector prefix the prompt with a planning request.
  The agent's authority is its own, so this is a request, not a guard, and
  the docs must say so.
- `gritt-harness` gained a dev-dependency on `gritt-connector` so the
  control-plane tests drive the fake agent. The runtime dependency graph
  is unchanged.

## Alternatives Considered

- Answering Claude Code approvals through its SDK control protocol
  (`--input-format stream-json`). Deferred: it needs a persistent stdin
  session and a permission tool handshake, which is a larger protocol than
  this step's contract.
- One process per session held open for follow-up input. Rejected: none of
  the four CLIs takes a second prompt on stdin in headless mode; all four
  resume by identifier.
- Stripping the child environment as the native shell tool does. Rejected,
  see Key Decisions.

## Assumptions

- `codex exec resume` accepts `--json` and `--skip-git-repo-check` after
  the subcommand (its help lists both); the live test only exercised the
  first turn.
- OpenCode with zero stored credentials reports `Unknown` auth, because it
  can still run through its own config and environment (it did here).
- `Cursor` output was mapped from the published format, not a recording.
- A connector session's continuation is only the agent's thread or session
  id; the transcript itself lives in the agent.

## Edge Cases and Failures

- Codex hung in the first live run: its stdin pipe was open. Fixed by
  closing stdin (see Key Decisions), then it completed in 7.3 s.
- A `claude --bare` probe ended with `terminal_reason: api_error`; the
  connector does not pass `--bare`.
- Sessions created before a connector fails were listed as sessions that
  never ran. The control plane now probes the connector first and refuses
  one that is not installed before creating the row, and removes a row it
  created in the same call when opening the session fails.
- Unknown wire messages become a streaming status event with the raw
  message in `diagnostic.unknown_event`; malformed lines become
  `diagnostic.malformed_line`. Neither is fatal.
- A process that exits cleanly without a terminal event completes with
  `StopReason::Other`; a non-zero exit is an error naming the status and
  the last stderr line.

## Validation

All run from the worktree root on 2026-09-04:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test -p gritt-connector`: pass, 30 tests (5 unit, 23 integration,
  2 live tests that skip without the gate).
- `cargo test --workspace`: pass, 178 tests.
- `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`:
  pass. Codex 0.153.2 authenticated, completed in 7.3 s with text `PONG`;
  Claude Code 2.1.260 authenticated, completed in 7.3 s with text `PONG`.
- `cargo build --release`: pass, 1 m 40 s cold.
- Release binary in a scratch workspace: `gritt connectors` listed native,
  codex (authenticated, 0.153.2), claude_code (authenticated, 2.1.260),
  cursor (not installed), opencode (installed, 1.15.4); `gritt run
  --connector claude --session demo "Reply with the single word PONG."`
  printed `PONG` in 5.6 s and exit 0; `session list` showed
  `connector:claude_code`; `session show demo` listed the connector
  events with usage and completion; `--connector cursor` failed with the
  not-installed message; `--connector grok` failed naming the valid
  names; the native path failed only on its own missing configuration.
- `gritt-agent ticket validate --repo-root .`: ok, 0 warnings.
- `gritt-agent ticket chain-check --ticket TKT-0012 --base
  feature/tkt-0008-gritt-cli`: ok, 0 warnings, 50 changed files,
  merge-base equal to the base tip `fb2d9bb`.
- `gritt-agent ticket chain-check --ticket TKT-0012 --base main`: ok,
  7 warnings, all expected: the merged TKT-0009, TKT-0010, and TKT-0011
  ticket files and the merge-base gap against `main`.

Benchmarks: none required. Test duration for the connector crate is about
5 s; the cancellation and timeout tests dominate.

## Completion Gate

- Acceptance: yes. Native, Codex, Claude Code, Cursor, and OpenCode satisfy
  the normalized contract and keep external authority; process exit,
  cancellation, timeout, approval, missing executable, and malformed
  output are surfaced without touching the native path; live Codex and
  Claude Code smoke tests passed and fixtures cover the same paths;
  connector sessions are stored, listed, shown, and resumed beside native
  sessions.
- Scope: yes. No provider parsing, policy semantics, release packaging, or
  frontend changed. Core changes are two additive fields; harness changes
  are the driver seam the task named.
- Validation: yes, as listed above.
- Security and safety: connector output and diagnostics are key-redacted;
  the child gets no extra authority from Gritt and keeps its own; no
  network code was added beyond launching the installed agent.
- Regression risk: low to medium. The REPL and TUI entry points changed
  signature to the driver; every existing native test passes through them.
  Windows process handling is untested here.
- Follow-up: see below.
- Assumptions: recorded above.

## Follow-up

- TKT-0013 documents that connector approvals are the agent's own, that
  planning on a connector is a request, and how `[connectors]` settings
  select executables, PTY transport, and extra arguments.
- Windows: `taskkill` tree kill and the PTY path are untested on this
  machine.
- Replace the hand-authored Cursor fixtures with recordings when the CLI
  is available, and record the `codex exec resume` turn live.
- Claude Code approval relay through its control protocol is a candidate
  for a later ticket.

## Updates

- 2026-09-04 second review fix round. Two findings: a credential-like option
  in `extra_args` is now refused whether its value is attached or split
  into the next token, and launch diagnostics keep option names only, with
  every positional token and option value shown as `[value]`; the wrap-up
  after a terminal event now runs against one absolute deadline, so an
  agent that keeps printing after finishing is stopped at the bound instead
  of extending it with every line.
- 2026-09-04 review fix round. Seven findings: credential values inherited
  from Gritt's environment now join the connector redaction set,
  credential-bearing `extra_args` are refused, and launch diagnostics omit
  the prompt and argument values; the driver drains through process exit
  so a trailing error or a non-zero exit after a terminal event is
  surfaced and `inspect()` reports the real state; `last_error` is
  key-redacted; native approval flags warn on an external connector; the
  PTY waiter polls instead of blocking under the child mutex;
  `ConnectorSettings` has a struct-level serde default so a partial
  `[connectors]` section parses; and a connector that is not installed
  leaves no session row. Twelve tests added across the connector, harness,
  core, and binary crates. Validation rerun, all green.
- 2026-09-04 PR #4 opened; the report records the PR, the commits, and
  both chain-check results.

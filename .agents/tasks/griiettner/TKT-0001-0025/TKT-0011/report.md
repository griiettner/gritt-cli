---
id: TKT-0011
namespace: griiettner
title: Implement sessions, planning and coding phases, permissions, workspace-bounded tools, terminal modes, approvals, cancellation, and telemetry
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

# TKT-0011 Report: Implement sessions, planning and coding phases, permissions, workspace-bounded tools, terminal modes, approvals, cancellation, and telemetry

## Summary

Worker 3 of the TKT-0008 chain. The harness crate now holds the session
store, the policy engine, the workspace-bounded native tools, the native
agent loop, local telemetry, and the three terminal modes. The `gritt`
binary gained `run`, `repl`, `tui`, and `session` commands on top of the
`config` and `key-set` commands from TKT-0009.

Chain facts:

- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0011`
- Branch: `tkt-0011-03-harness`
- Base: `feature/tkt-0008-gritt-cli` at `aa31c47` (PR #2 merge)
- Commit: `111da5e` (implementation), plus the report update commit
- PR: https://github.com/griiettner/gritt-cli/pull/3 into
  `feature/tkt-0008-gritt-cli`

What landed, by module in `crates/gritt-harness/src`:

- `store/session_store.rs`: `SessionStore` on Turso for create, get,
  list, rename, remove, append and read events, and continuation state;
  `set_phase`, `find_by_name`, and `next_sequence` helpers. Migration
  `0002_content_log` adds the opt-in content table.
- `policy.rs`: `PolicyEngine::evaluate(tool, resource)` over the core
  rules with `*`, `**`, and `?` wildcards, `workspace:` patterns resolved
  against the canonical root, first match wins, fallback otherwise. Shell
  commands matching a destructive fragment list get a stronger prompt.
- `tools.rs`: `Workspace::resolve` rejects `..` climbing, absolute paths
  elsewhere, and symlink escapes by canonicalizing the deepest existing
  ancestor. File read, file write with a unified diff preview, and shell
  through `sh -c` in its own process group (`cmd /C` on Windows). A
  `ProcessRegistry` tracks children; cancellation kills the group with the
  platform's own `kill` or `taskkill`, so no extra dependency was added.
- `agent.rs`: `NativeAgent::run_turn` streams adapter events into the
  store and the interface, renumbers them into one session sequence
  (adapter numbers stay in the diagnostic), gates every tool call through
  the policy, asks the `Ui` when the outcome is `ask`, executes, submits
  results, and continues until the turn completes, fails, or is cancelled.
  `AgentBuilder` resolves the model through the alias layer, loads the
  catalog at most daily, and restores continuation state on resume.
  `CancelHandle` stops the request, the stream, and every child process.
- `telemetry.rs`: content-free turn and token records into the
  `gritt_telemetry_events` and `gritt_analytics_records` tables; content
  rows only when `logging.content_logging` is on, purged after the
  configured retention on every open.
- `modes/print.rs` and `modes/repl.rs`: print mode over any writer pair
  with a caller-supplied approval prompt; the REPL adds history, `/plan`,
  `/code`, `/sessions`, `/resume NAME`, `/history`, `/help`, `/quit`.
- `tui/`: Ratatui 0.30.2 with Crossterm 0.29 through ratatui's own
  re-export so one crossterm version is in the tree. `app.rs` is the state
  and key reducer, `render.rs` draws the transcript, multiline prompt,
  status bar, approval and diff views, command palette, and session list,
  and `run.rs` owns the terminal, the panic hook that restores it, the key
  reader thread, and the turn task.

`crates/gritt/src/main.rs` wires the modes: `--session`, `--profile`,
`--model`, `--plan` or `--code`, `--approve-all`, `--deny-all`, `--ask`,
`--no-models`, `--database`, and `session list|show|rename|remove`.
Approvals default to asking when stdin is a terminal and to denying when
it is not. Ctrl-C cancels a running turn and exits when nothing runs.

## Key Decisions

- The harness owns the session event sequence. Adapter events are
  renumbered on persist and keep their adapter sequence under
  `diagnostic.adapter_sequence`, so the store's primary key never collides
  between adapter and harness events.
- A bare `*` resource rule matches any resource, including commands and
  URLs with slashes. Longer patterns keep path semantics, where `*` stops
  at a separator and `**` does not.
- Added a `network` `ask` rule to the core workspace defaults (additive
  change in `crates/gritt-core/src/policy.rs`), so the default table
  covers every outcome the harness skill lists.
- The system prompt is sent once per session. A resumed session restores
  the adapter's continuation state and sends only the new user message.
- Tool calls whose arguments fail to resolve, including paths outside the
  workspace, never reach the policy; the refusal is still recorded as an
  error tool result and returned to the model.
- The TUI answers approvals itself, so a `tui` start without `--deny-all`
  never inherits the non-terminal deny default.
- Diff review uses `diffy` (MIT OR Apache-2.0) for unified diffs. The
  wildcard matcher is local code rather than a glob crate.

## Alternatives Considered

- Killing children through a `libc` or `nix` dependency. Rejected in
  favor of the platform `kill` and `taskkill` tools to keep the dependency
  set small.
- Sending the whole conversation on every turn. Rejected because the
  adapters already keep wire state and continuation, and the Responses
  adapter continues by `previous_response_id`.
- Async terminal events through crossterm's `event-stream` feature.
  Rejected for a plain reader thread with a stop flag, which needs no
  extra feature and exits with the loop.

## Assumptions

- `sh -c` is available on macOS and Linux; Windows uses `cmd /C`. Process
  group kill on Unix runs `kill -KILL -- -<pid>` followed by a direct kill.
- The destructive command list is a heuristic for the stronger prompt
  only; it never changes an outcome.
- Content logging retention purges on open, not on a timer.
- Live provider calls were not exercised because no key exists on this
  machine; every session test replays the TKT-0010 chat fixtures.

## Edge Cases and Failures

- The first run of the approve-all test showed only one of three tool
  results: the resource-error path returned before emitting its result
  event. Fixed by emitting the result on that path too.
- A bare `*` resource rule did not match `https://x` or `ls /tmp` because
  `*` stopped at a separator. Fixed as described above.
- Clippy rejected a large variant size difference in the TUI message
  enum; the returned agent is boxed.
- Print mode showed a failed turn's error twice, once from the interface
  and once from `main`. The binary now relies on the interface's event.

## Validation

All run from the worktree root on 2026-09-04:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test -p gritt-harness`: pass, 39 tests (30 unit, 9 integration).
- `cargo test --workspace`: pass, 114 tests.
- `cargo build --release`: pass, 1m 40s cold.
- Manual terminal pass with `target/release/gritt` in a scratch workspace
  with a profile whose key is absent: `config` reports the profile with
  the missing-key state; `run "hello" --no-models` prints the missing-key
  error naming the profile and variable and exits 1; `session list` and
  `session show` work and refuse an unknown name; `repl` over piped input
  handles `/help`, `/code`, `/sessions`, `/history`, an unknown command,
  and `/quit`; `tui` without a terminal exits 1 with a clean message
  instead of a panic. Resize, keyboard-only navigation, and the approval
  and diff views could not be exercised without an interactive terminal;
  the state reducer and renderer are covered by unit tests through
  ratatui's `TestBackend`, and the manual TTY pass is a follow-up for
  TKT-0013's end-to-end run.
- `gritt-agent ticket validate --repo-root .`: ok, 0 warnings.
- `gritt-agent ticket chain-check --ticket TKT-0011 --base feature/tkt-0008-gritt-cli`:
  ok, 0 warnings, 26 changed files, merge-base equals the base tip
  `aa31c47`.
- `gritt-agent ticket chain-check --ticket TKT-0011 --base main`: ok, 5
  warnings, all expected: the merged TKT-0009 and TKT-0010 ticket files
  and the merge-base gap between `main` and the chain branch.

Dependency checks with `cargo info`: ratatui 0.30.2 (MIT, MSRV 1.88),
crossterm 0.29.0 (MIT), diffy 0.5.2 (MIT OR Apache-2.0), uuid 1.26.0
(Apache-2.0 OR MIT). tokio gained the `process`, `io-util`, `io-std`,
`fs`, and `signal` features. `cargo tree` shows a single crossterm 0.29.0.

## Completion Gate

- Acceptance: yes. Print and REPL sessions plan, approve, execute, cancel,
  and resume against fixtures. Full-screen mode renders the transcript,
  approvals, tool activity, status, multiline input, and diff review on
  Ratatui 0.30.2 and Crossterm 0.29. File and shell tools cannot escape
  the workspace and every execution passes the policy first. Telemetry
  and analytics are local, content-free, and in their own tables.
- Scope: yes. No provider wire parsing, connector launcher, packaging, or
  cloud code was added. The one core change is an additive default rule.
- Validation: yes for the automated set and the non-interactive manual
  pass. Interactive TTY behavior is not verified here.
- Security and safety: workspace boundary enforced before policy, policy
  before execution, child processes tracked and killed on cancel, no key
  or prompt content in telemetry, content logging off by default.
- Regression risk: low. TKT-0009 and TKT-0010 tests still pass; the core
  default rule change adds a `network` entry before the catch-all.
- Follow-up: interactive TTY pass, Windows process kill path, and REPL
  line editing.
- Assumptions: recorded above.

## Follow-up

- TKT-0013 should run the full-screen mode in a real terminal: resize,
  keyboard-only navigation, approval and diff views, and `NO_COLOR`.
- The Windows shell path (`cmd /C` plus `taskkill /T`) is untested here.
- The REPL reads plain lines; arrow-key history editing would need a line
  editor crate.
- TKT-0012 can implement `Ui` for connector sessions and reuse the store,
  telemetry, and modes without changes to the native loop.

## Updates

- 2026-09-04 report update. Added the commit, PR #3, and chain-check
  evidence after the PR was opened. No code changed.

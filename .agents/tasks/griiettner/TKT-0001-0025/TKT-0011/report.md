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
  When the phase changes after that, the next user message is prefixed
  with a phase transition note naming the tools that are now available or
  withdrawn, so the retained system instruction cannot mislead the model.
  The phase the model was last told about is persisted in the
  `told_phase` column (migration `0003_session_told_phase`), so a phase
  change followed by an exit still sends the note on resume, and a
  session with no recorded value sends the current-phase note rather
  than assuming the model heard it.
- Recorded exception, shell confinement: ADR-009 runs shell under approval
  and bounds only the file tools to the workspace. No operating-system
  sandbox is built. Instead the shell child runs in the workspace root,
  the approval prompt states that the command runs with the user's
  authority and may reach outside the workspace, and a command that names
  an absolute path outside the workspace, a drive path, a `..` component,
  or a path the shell would expand (`~`, `~user`, `$VAR`, `${VAR}`,
  `%VAR%`) gets the stronger prompt. Such a command is never allowed
  silently: an `allow` outcome from any rule becomes `ask`; `deny` stays
  `deny`. TKT-0013 documents this boundary in the tool and privacy docs.
- Shell children never inherit credentials. Every configured profile key
  variable, `AGENT_MEMORY_API_KEY`, and any variable whose name contains
  `KEY`, `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, or `CREDENTIAL` is
  removed from the child environment, which covers the conventional
  names such as `AWS_SECRET_ACCESS_KEY` and `GITHUB_TOKEN`. `PATH`,
  `HOME`, `TERM`, the locale variables, and the other plain shell
  variables are kept.
- Session events keep tool arguments and outputs because resume and the
  transcript need them, but every event, tool result, approval resource,
  and content-log row is redacted against the active key before it is
  shown, stored, or sent back to the provider. The adapter's continuation
  state is redacted the same way before it is saved, and the diff preview
  is redacted before it is shown and never stored. The redacted approval
  request is built once and is what the interface, the events, and the
  transcript all see. While content logging is off, the stored approval
  request keeps only the tool, the resource, and the call id; the reason
  and the destructive flag are shown and not persisted.
- A session resumes only in the workspace it was recorded in. Canonical
  paths are compared and a mismatch names both paths and the
  `--workspace` flag that resolves it.
- Approval waits are raced against cancellation in every mode. Print and
  REPL read the answer on a blocking thread so Ctrl-C can end the turn;
  the full-screen mode drops the approval view on Esc. A cancel during a
  pending approval denies the tool and ends the turn as cancelled.
- One reader owns stdin. `LineInput` runs a reader thread that forwards
  lines over a channel; the REPL loop takes commands from it and the
  approval prompter takes answers from it, so neither holds the stdin
  lock while the other waits. A prompter whose turn was cancelled gives
  up its wait within 100 ms, so the next typed line reaches the loop as
  a command instead of answering a question that is gone. Print mode
  uses the same owner.
- The diff preview is built before any approval event is recorded, so a
  preview that cannot be built refuses the call without leaving an
  unmatched request. An approval prompt that cannot be written denies at
  once, and print mode reports an output failure on its closing write
  with a non-zero exit code.
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
- Content logging retention purges whenever the database opens, whether
  or not content logging is currently enabled, not on a timer.
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
  and once from `main`. The binary now relies on the interface's event,
  and the REPL does the same.
- A child that filled stderr before closing stdout deadlocked the
  sequential pipe drain. Both pipes now drain concurrently; a test floods
  stderr with 300 KB first.
- A write preview treated every read error as an empty file, so an
  unreadable or non-UTF-8 target showed a misleading new-file diff. Only
  `NotFound` means a new file now; other errors refuse the call.
- Print mode ignored write and flush failures, so a closed pipe let the
  turn keep consuming tokens and running tools. The first output error
  now cancels the request, kills child processes, and fails the turn.

## Validation

All run from the worktree root on 2026-09-04:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test -p gritt-harness`: pass, 61 tests (40 unit, 21 integration)
  after the second review fix round; 52 after the first, 39 before it.
- `cargo test --workspace`: pass, 136 tests after the second fix round;
  127 after the first, 114 before it.
- `cargo build --release`: pass, 1m 40s cold.
- Manual terminal pass with `target/release/gritt` in a scratch workspace
  with a profile whose key is absent: `config` reports the profile with
  the missing-key state; `run "hello" --no-models` prints the missing-key
  error naming the profile and variable and exits 1; `session list` and
  `session show` work and refuse an unknown name; `repl` over piped input
  handles `/help`, `/code`, `/sessions`, `/history`, an unknown command,
  and `/quit`; `tui` without a terminal exits 1 with a clean message
  instead of a panic. The scripted REPL integration test
  `repl_runs_a_scripted_session_end_to_end` drives `run_repl` through a
  planning turn, `/code`, an approved write, a cancelled shell command,
  `/sessions`, `/resume`, `/history`, and `/quit` against fixtures, with
  the two `y` answers read from the same shared input as the commands.
  After the second fix round the release binary's `repl` over piped
  input was run again: `/help`, an unknown command, a prompt that fails
  on the missing key, and `/quit` behave as before through the shared
  stdin owner. Resize, keyboard-only navigation, and the approval
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
  and resume against fixtures, including a fixture-backed REPL run and a
  cancel during a pending approval. Full-screen mode renders the transcript,
  approvals, tool activity, status, multiline input, and diff review on
  Ratatui 0.30.2 and Crossterm 0.29. File and shell tools cannot escape
  the workspace and every execution passes the policy first. Telemetry
  and analytics are local, content-free, and in their own tables.
- Scope: yes. No provider wire parsing, connector launcher, packaging, or
  cloud code was added. The one core change is an additive default rule.
- Validation: yes for the automated set and the non-interactive manual
  pass. Interactive TTY behavior is not verified here.
- Security and safety: file tools bounded before policy, policy before
  execution, shell children stripped of credential variables, every
  event and tool result and content-log row key-redacted, child processes
  tracked and killed on cancel, no key or prompt content in telemetry,
  content logging off by default with unconditional retention purge.
  Shell commands are not sandboxed; see the recorded exception above.
- Regression risk: low. TKT-0009 and TKT-0010 tests still pass; the core
  default rule change adds a `network` entry before the catch-all.
- Follow-up: interactive TTY pass, Windows process kill path, REPL line
  editing, and TKT-0013 documentation of the shell authority boundary.
- Assumptions: recorded above.

## Follow-up

- TKT-0013 should run the full-screen mode in a real terminal: resize,
  keyboard-only navigation, approval and diff views, and `NO_COLOR`.
- The Windows shell path (`cmd /C` plus `taskkill /T`) is untested here.
- The REPL reads plain lines; arrow-key history editing would need a line
  editor crate.
- TKT-0013 must document in the tool and privacy docs that shell commands
  run with the user's authority and are not confined to the workspace,
  and that the stronger prompt is the only guard for paths outside it.
- After a cancel during a print or REPL approval, the blocking stdin read
  that was waiting for the answer lingers until the next line arrives, so
  the next typed line may be consumed as that stale answer.

## Updates

- 2026-09-04 third review fix round. The re-review kept one finding: the
  stdin approval prompter polled the shared cancel slot, which the loop
  clears as soon as a cancelled turn returns, so a reader that checked
  after the clear missed the cancellation and held the input for good.
  `line_prompter` in the harness now captures the turn's cancel handle
  when the question is asked and waits on that; the binary uses it. Test
  `repl_recovers_after_cancelling_a_pending_approval` drives the REPL
  through a pipe, cancels a pending approval, then runs another command
  and approval.
- 2026-09-04 second review fix round. The re-review kept nine findings.
  Resolved: shell commands that reach outside the workspace, including
  `~` and variable expansions, are forced to `ask` with the stronger
  prompt whatever rule matched, with an authority-line test; the
  credential filter matches name fragments so `AWS_SECRET_ACCESS_KEY`
  and `GITHUB_TOKEN` are stripped; continuation state is redacted before
  it is saved; the interface receives the redacted approval request and
  preview; the last told phase is persisted so an exit after `/code`
  still sends the note; one reader owns stdin for the REPL loop and its
  approval prompter; stored approval requests drop the reason and
  diagnostic while content logging is off; the preview is built before
  the request is recorded; and a failed approval prompt or closing write
  ends the turn with a failure. Tests cover each.
- 2026-09-04 review fix round. The chain reviewer returned twelve
  findings. Resolved: shell approval wording and outside-workspace
  stronger prompt with the recorded exception; credential variables
  stripped from shell children and every tool result redacted; session
  events redacted before persistence; phase transition note after
  `/plan` and `/code`; workspace check on resume; cancellation-aware
  approval waits in all modes; concurrent stdout and stderr draining;
  redacted content-log rows; unconditional retention purge; preview read
  errors propagated; print output failures stop the turn; and a scripted
  REPL integration test. The earlier claims that content is purged only
  while logging is on and that the REPL had fixture coverage were
  corrected above.
- TKT-0012 can implement `Ui` for connector sessions and reuse the store,
  telemetry, and modes without changes to the native loop.

## Updates

- 2026-09-04 report update. Added the commit, PR #3, and chain-check
  evidence after the PR was opened. No code changed.

---
id: TKT-0017
namespace: griiettner
title: Review fixes for the MCP runtime and harness tool dispatch
artifact: update
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
---

# 2026-09-04 Review fixes

## Trigger

The reviewer returned `needs-fix` on PR #9 with 3 High and 8 Medium findings,
all confirmed against the branch. Trust enforcement, credential redaction, and
process cleanup did not meet the ticket contract, and several tests were
weaker than the report claimed. Everything below landed on
`tkt-0017-02-mcp` in the same worktree.

## Changes per finding

**1. High — redact MCP credentials before exposing server-controlled data.**
The runtime now records every credential value the current configuration
hands to any server (`configured_secrets` in `mcp/mod.rs`, built from
credential-named `env` entries and credential-named or auth headers) and
redacts against them at the boundary: failure reasons including the stderr
tail, RPC error messages and diagnostics (`redact_error`), `serverInfo`
metadata copied into snapshots, every discovered tool definition before it
becomes a schema, and every `tools/call` result before it is rendered. Two
new fixtures echo their own configured credential back through metadata, a
description, a schema default, a text result, structured content, and an RPC
error, one per transport; both assert the value never appears and that
`[redacted]` does.

**2. High — trust on restart, revocation on denial.** `restart` now reads the
trust store for the *current* fingerprint and puts an unapproved entry back
into `AwaitingApproval` or `Denied` instead of launching it. `decide(Denied)`
bumps the generation, removes the server's tools from the registry, and shuts
its connection down, so denial revokes live access rather than only
preventing the next launch. `call` refuses any server that is not `Ready`.
`restarting_does_not_bypass_approval` and
`denying_a_running_server_revokes_it_immediately` drive all of this through
the public API, the second also asserting the process is gone and that a
reference held from before the revocation never reaches the server.

**3. High — owned-process cleanup on CLI cancellation and startup errors.**
`install_ctrl_c` now takes the runtime and awaits `shutdown()` before
`process::exit(130)`, which covers the idle REPL Ctrl-C that previously
skipped destructors entirely. Each mode was split into an outer function that
owns the runtime's lifetime and an inner one that may fail (`print_turn`,
`repl_loop`, `tui_session`), so every error after MCP start — including the
fallible session open — still runs `stop_mcp`. The signal handler and its
cancel slot are installed *before* the first server is launched, so a Ctrl-C
during startup releases what has already started.

**4. Medium — reject stale lifecycle completions.** Every entry carries a
`generation` that changes on any definition or lifecycle change: load with a
new fingerprint, `decide`, `stop`, `restart`, `mark_lost`, and `shutdown`.
Asynchronous initialization captures the generation it started under, and
`start` installs a result only if it still matches; otherwise the connection
is closed as stale. `decide` and `restart` also re-check the fingerprint
after awaiting persistence and refuse to apply a decision to a definition the
user never saw. `refresh` and `mark_lost` are generation-checked the same
way. `a_late_initialization_result_is_discarded_after_a_denial` denies a
server mid-handshake and asserts no tools, no process, and the `Denied` state
survive.

**5. Medium — execute the exact tool identity approved for the turn.** The
per-turn snapshot now holds `FrozenTool { reference, generation }` rather than
a bare name, and `McpRuntime::call` takes that frozen value. It verifies the
generation, that the server is ready, and that the live registry still maps
that dispatch name to that exact server-and-tool pair before anything is
sent. `agent.rs` passes the turn's frozen entry rather than the call name.
Two tests cover it: a reference frozen before a reload is refused after it,
and a dispatch name that now means a different original tool is refused with
the mismatch message.

**6. Medium — cancellable, bounded writes and queue operations.** Writes race
the shutdown signal and a write deadline, so a server that stops reading
stdin cannot park the command task. Queue sends use `send_timeout`.
Shutdown is signalled through a flag rather than only through the queue, and
completion is reported through a second flag, so `shutdown()` still waits for
real cleanup on the paths where the queue is already gone. Initialization now
honors the cancellation token locally through
`request_uncancellable(..., cancel)`, which stops the wait without sending the
prohibited cancellation for `initialize`.
`a_server_that_stops_reading_cannot_block_shutdown` proves the connection is
`Ready` first, so it exercises a blocked writer rather than a failed
handshake.

**7. Medium — descendants outliving the direct child.** `terminate` now
continues after the child is reaped: if anything remains in the process group
it sends TERM, waits the grace period, then kills the group. Reader tasks are
joined with a bound and aborted if a wedged descendant holds a pipe open. A
new fixture behavior spawns a `sh` that traps TERM, records its pid, and
outlives its parent; the test asserts that pid is gone after shutdown.

**8. Medium — own outstanding HTTP work.** Request tasks are tracked by id
and aborted on `Command::Abort` (sent whenever a caller stops waiting), on
reload, and on shutdown. A semaphore bounds concurrent in-flight requests per
endpoint. The session `DELETE` has its own deadline, so an unresponsive
endpoint cannot hold up exit. The comment that a remote side effect may still
complete is preserved on the abort path.

**9. Medium — HTTP initialization ordering and protocol-level requests.**
`notify_delivered` gives the caller a delivery barrier: the transport awaits
the notification POST in its command loop, so no later request can overtake
`notifications/initialized`, and its failure fails the handshake. `route` now
answers server-initiated requests on both transports: `ping` gets a result,
anything else gets method-not-found. The HTTP fixture asserts the transport
contract (both accepted content types on every POST) and panics if
`tools/list` arrives before `initialized`; the stdio `strict` fixture refuses
the same ordering violation and sends a server-initiated ping.

**10. Medium — input limits enforced during accumulation.** `read_line_bounded`
replaces `read_line` for both stdout and stderr and stops at the limit
instead of after it, with unit tests for a delimiter-free flood and for
ordinary lines. The SSE reader bounds both the total stream and the bytes
accumulated between two complete events, which is what the parser can be
holding for an unterminated one, and closes the connection on either. Stdio
pending requests are capped and dropped on `Abort`.

**11. Medium — MCP startup follows the resolved backend.** `runs_on_native`
resolves the actual session backend before starting anything, delegating the
decision to the pure `native_backend`. `--connector native` now loads MCP,
and resuming an external session without a flag no longer does. Unit tested
across explicit native, implicit native, explicit external, and resumed
external.

## Optional follow-ups applied

- `McpServerState::explain()` gives every state a non-empty sentence,
  including awaiting approval, denied, starting, ready, and stopped;
  `gritt mcp list` and the startup notices use it.
- The live smoke check now fails when a server whose executable is present,
  on a supported transport, does not become ready. It previously passed when
  every available server failed.

Not applied, recorded in `report.md` instead: lifecycle event delivery for
TKT-0019 (still polling), and the ADR updates, which belong to the chain's
close rather than to this worker's branch.

## Test-timing robustness

The PM saw two failures in one suite on a loaded machine that four later runs
could not reproduce. Two genuine races were found in the new tests, both
load-sensitive and both a plausible match for that report. Neither was a
product defect; both were the tests measuring themselves badly.

- **The HTTP fixture closed each connection without saying so.** It answers
  one request per socket, but sent no `Connection: close`, so `reqwest`
  pooled the socket and the next request raced the server's close. It
  surfaced as `request failed for 127.0.0.1/mcp` in
  `a_streamable_http_server_handshakes_lists_and_calls`, roughly one run in
  eight. Every fixture response now carries the header.
- **The concurrency probe lost writes.** `writeln!` issues one syscall per
  format piece, so six servers appending `start` and `end` at once
  interleaved into corrupted lines; the log undercounted and the computed
  peak was wrong in both directions. Each line is now built first and written
  with a single `write_all`. With that fixed the measurement is stable at
  exactly the configured limit over twelve consecutive runs, which is also
  the evidence that the runtime enforces it: the earlier "peak 4" was the
  broken probe, not a breached limit.

Both were confirmed by looping the suite: ten consecutive clean runs of
`mcp_runtime` and three of the full workspace after the fixes.

Reviewing the remaining MCP tests for load sensitivity:

- The shared test `settings()` used a 10 s call deadline while a cancellation
  test asserted the wait ended in under 3 s. Under load the deadline could
  have fired first and changed the error kind. Deadlines for tests that are
  *not* about deadlines are now 30 s and 60 s, far above anything those tests
  wait for, and the elapsed-time bounds were widened to 20 s. The tests that
  do exercise a deadline still set their own short value.
- `all_gone` polled for 4 s; it now polls for 15 s. It waits for an outcome
  rather than asserting a duration, so load makes it slower, never wrong.
- The initialization-deadline test asserted completion within 5 s against a
  400 ms deadline; that bound is now 20 s and prints the measured value.

Two of the new tests were also found to be passing for the wrong reason: the
`deaf` and `descendant` fixtures each held the client waiting for the full
30 s handshake deadline, so neither reached the behavior it named. The
fixtures were fixed (`deaf` answers the real request id before going silent;
`descendant` detaches its child's stdio instead of holding the parent's
pipes) and both tests now assert the precondition they need. The suite went
from 30 s to about 1 s as a result.

## Validation

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass. The
  `nix v0.28.0` future-incompatibility note is pre-existing.
- `cargo test --workspace --no-fail-fast`: pass, 306 tests, 0 failed.
  gritt-core 34; gritt-harness 66 unit, 34 mcp_runtime, 4 mcp_native_session,
  1 mcp_live_smoke, 23 native_session, 12 session_draft, 8 connector_session;
  gritt-provider 25 unit, 26 contract, 6 models_cache, 1 sse_tcp, 3 live
  (skips); gritt 15 unit, 11 e2e, 4 tui_pty; gritt-connector 5 unit, 25
  connectors, 3 live (skips).
- `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke`:
  pass. The one configured entry, `gritt`, is ready on protocol `2025-06-18`
  with 3 tools. No tool was called.

Twelve tests were added to `mcp_runtime.rs` (34 total), two to the stdio unit
tests, one to core, and one to the binary.

## Remaining follow-up

Unchanged from `report.md`, plus: there is still no lifecycle event
subscription, so TKT-0019 polls `snapshots()`. Defining that delivery is the
cheapest remaining improvement for the TUI step.

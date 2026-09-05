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

---

# 2026-09-05 Round 2 review fixes

## Trigger

The re-review of PR #9 returned `needs-fix` again: 3 High and 6 Medium, all
confirmed. Findings 5 and 7 from round 1 were confirmed fixed; the other nine
were partial. The theme was that round 1 fixed the named symptom but left the
same class of hole one step further out: credentials were redacted but tied to
the file rather than the connection, shutdown owned installed connections but
not the ones still being built, and the CLI cleaned up two of its four
launching paths.

## Changes per finding

**1. High — credentials belong to a connection, not to the file.** `load`
replaced the whole redaction set while retaining a ready connection by raw
fingerprint. Rotating `${TOKEN}` leaves the definition, and therefore the
fingerprint, unchanged, so the process kept running with token A while only
token B was redacted. `ServerRuntime` now carries the credentials its running
connection was launched with; they retire with the connection. `call`,
`refresh`, `discover`, and `mark_lost` all redact against the entry's own set
rather than a global one, and `configured_secrets` became the per-definition
`entry_secrets`. `rotating_a_token_keeps_redacting_what_the_running_server_
still_holds` asserts the connection is retained, the process is not restarted,
and the first token is still redacted out of both a result and an error.

**2. High — shutdown owns initialization work.** A connection created inside
an in-flight handshake existed only in that future, so shutdown could return
while its child was alive and the signal handler would then call
`process::exit`. Connections are now registered in `RuntimeState.launching`
the moment they exist and released when the handshake resolves; shutdown sets
a `closing` flag, closes the launching connections alongside the installed
ones, and waits for the map to drain. A launch that starts after `closing` is
closed instead of installed. `shutdown_during_startup_takes_the_children_with_
it` runs one server that becomes ready and one that never answers, interrupts
mid-handshake, and asserts both processes are gone by pid and that nothing was
installed afterwards.

**3. High — signal cleanup on every launching path.** TUI mode started MCP
before raw mode with no handler installed, and `gritt mcp list` and `gritt mcp
trust` had none at all. Both now install the cleanup-aware handler before the
first launch. `interrupting_mcp_startup_leaves_no_server_running` in
`crates/gritt/tests/e2e.rs` runs `gritt mcp trust` against a server that never
speaks MCP, sends SIGINT during initialization, and asserts both a 130 exit
and that the server process is gone. Without the handler the process dies by
signal rather than exiting 130, so the test fails on the unfixed code.

**4. Medium — stale trust reads.** `restart` and `decide` checked only the
fingerprint after awaiting the trust store, and a concurrent denial or stop
preserves the fingerprint. Both now capture the lifecycle generation before
the await and refuse to apply an answer whose generation no longer matches.
`a_lifecycle_change_during_a_slow_trust_read_wins` holds a trust read open
with a paused store, stops the server underneath it, and asserts the stale
approval is refused and the server stays stopped.

**5. Medium — cancellable queue admission.** Admission now checks
cancellation first and races the enqueue against the token, so a cancelled
request cannot wait for capacity and then still be written. Giving up sends
the abort before the notification and both use a short deadline, so
cancellation is not delayed by its own bookkeeping. Both transports remember
abandoned ids, so a request whose abort overtook it is dropped rather than
sent. `a_blocked_writer_neither_hangs_callers_nor_shutdown` now makes the
server go deaf only after it is ready, fills the pipe with 200 concurrent
64 KiB calls, asserts they all come back as errors rather than hanging, and
asserts shutdown is still prompt and the child gone.

**6. Medium — all HTTP work is owned.** Notification POSTs and server-request
replies were detached and invisible to shutdown. Both now go through
`Endpoint::spawn_auxiliary`, which keeps the handle, and shutdown aborts and
awaits them alongside the request tasks. Auxiliary POSTs have their own small
semaphore, separate from the request budget, because a reply is produced while
a request permit is held. `a_stalled_notification_cannot_hold_up_shutdown`
cancels a call against an endpoint that accepts the cancellation notification
and never answers it, then asserts shutdown still completes promptly.

**7. Medium — compliant server-request replies.** Replies bypassed the POST
helper and so omitted `Accept: application/json, text/event-stream`, which the
transport requires on every client POST. They now go through the same helper
as every other message. `a_server_request_is_answered_with_a_compliant_post`
uses an endpoint that sends a `ping` on the discovery stream and records the
reply, asserting the answer arrived and carried both content types, the
session id, and the negotiated revision.

**8. Medium — input limits without framing bypasses.** The stdio reader
checked the cumulative length only on the branch with no newline, so a line
could exceed the limit as long as its final chunk carried the terminator; the
check now runs on both branches, with tests for a line crossing the limit in
its terminating chunk and for one exactly at the limit. The SSE accounting
moved into `SseBudget`, which checks before the parser is fed and computes
what the parser still holds from the last event terminator in the chunk rather
than resetting to zero whenever any event completed. Three unit tests cover an
oversized event after earlier events completed, a single event larger than the
bound, and a stream stopped by the stream bound rather than the event bound.

**9. Medium — backend prediction matches session resolution.** An explicit
connector flag bypassed the session lookup, so `--session existing-native
--connector codex` predicted external and disabled MCP, while
`ControlPlane::open` resolves that session as native and runs it. The session
is now resolved first and its own kind decides; the flag only decides when
there is no stored session. The unit expectation that encoded the opposite was
replaced with one that states the rule and why.

## Optional follow-up applied

The stdio strict fixture now records that it received a *response* to the ping
it sent, and the test asserts that marker. Sending a ping only showed the
server asked; this shows the client answered.

## Validation

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass. Three
  `useless_vec` findings in the new budget tests were fixed before the final
  run.
- `cargo test --workspace --no-fail-fast`: pass, 319 tests, 0 failed, over
  three consecutive runs.
- `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke`:
  pass. The one configured entry, `gritt`, is ready on protocol `2025-06-18`
  with 3 tools. No tool was called.

Thirteen tests were added or rewritten: six in `mcp_runtime.rs`, five unit
tests across `mcp/stdio.rs` and `mcp/http.rs`, one in `crates/gritt/tests/
e2e.rs`, and the strict-handshake assertion.

## Notes for the record

The live smoke check reads the workspace's own `.mcp.json`, whose single entry
holds an exclusive lock on the worktree's memory database. Only one instance
can run at a time, so the check fails while another `gritt-agent mcp serve`
holds that lock, including one started by a reviewer in the same worktree.
That is an environment collision, not a defect: rerun once the other process
has exited.

---

# 2026-09-05 Round 3 review fixes

## Trigger

The third review returned `needs-fix` with seven confirmed defects: two High
and five Medium. Round-2 fixes 3, 7, and 9 were confirmed resolved. The rest
were partial in the same way as before: the named case was closed but the
mechanism still had a gap one step out. Two of the seven were defects the
round-2 *fix* introduced (the SSE accounting and the auxiliary task spawn
order), which is worth saying plainly.

## Changes per finding

**1. High — shutdown lost ownership of startup connections.** Two gaps.
`finish_launch` released a completed connection while `start` still held it
inside `.collect()` until every handshake in the batch finished, so during
that window shutdown could see neither a pending launch nor an installed
entry. And the outer `timeout` wrapped the whole of `connect_inner`, so a
handshake that ran long had its future dropped before the cleanup, leaving a
live child that only shutdown's drain would find. Ownership is now continuous:
the launch registration travels on `Established` and is released only under
the same lock that installs or closes the connection, and the deadline wraps
the negotiation alone so every exit path runs its own cleanup. `start` also
treats `closing` as stale. The test now asserts the processes are gone *as
shutdown returns*, with no polling, because the signal handler calls
`process::exit` immediately afterwards.

**2. High — bearer tokens escaped redaction.** Only the complete header value
was registered, so `Authorization: Bearer sk-...` protected the whole string
while a server echoing just `sk-...` slipped through exact-string replacement.
`credential_variants` now registers the complete value and the credential part
after an authentication scheme, with a length floor so a stray short word
cannot be redacted out of every message. The HTTP echo regression now
configures `Bearer <token>` and the endpoint echoes the bare token into its
metadata, a tool description, and a result. Reverting the fix fails that test.

**3. Medium — a rejected stale approval stayed persisted.** `decide` wrote the
trust record and only then validated the generation, so a delayed approval
could overwrite a newer denial and return an error without undoing the write;
a later restart then read Approved and launched the denied definition.
Decisions are now serialized and the state is re-read inside that guard, so
persistence and application happen in the same order and a stale write cannot
replace a newer decision. The regression delays the write, lands a denial
behind it, and asserts the denial is what is persisted, what the runtime
shows, and what a subsequent restart reads.

**4. Medium — cancelled queued calls could still reach the server.** The abort
is queued behind the request it cancels, so a writer resuming after a block
sent the request first and only then read the cancellation. Cancellation is
now recorded on the connection itself, before anything is queued, and both
transports check it immediately before writing. The regression fills the pipe
to a server that has paused its reader, cancels a call sitting in the backlog,
lets the server drain, and asserts the marker that server writes on receipt
never appears. Reverting the check fails it.

**5. Medium — auxiliary HTTP work was still unbounded.** Tasks were spawned
and stored before acquiring a permit, so eight stalled POSTs held the permits
while every later notification and server reply piled up behind them with its
payload. Admission is now bounded by the same cap as execution: over the
limit, the message is dropped rather than queued, which is safe for
best-effort traffic. Each auxiliary POST also has its own deadline so a
stalled one releases its permit and its slot instead of holding both until
shutdown. The regression cancels sixty calls against an endpoint that accepts
notifications and never answers, and asserts only a bounded handful ever
reached it.

**6. Medium — the SSE budget failed on chunk boundaries.** The round-2
accounting looked for a delimiter within a single chunk, so a `\n\n` split
across two chunks never reset the pending count and a valid stream would
eventually be rejected at the event bound; a chunk carrying many events also
counted as one. Framing is now tracked byte by byte with the newline run
carried across chunks, the way the parser sees the stream. Tests cover a
delimiter split across chunks, in both `\n\n` and `\r\n\r\n` form, and fifty
valid events arriving both as one burst and byte by byte.

**7. Medium — switching to a native session left MCP unloaded.** Opening was
gated on the backend resolved at startup, so a run that began on an external
agent and later resumed into a native session had no servers, and the turn
refresh only touched entries that were already loaded. `McpRuntime::ensure_open`
opens once and does nothing after, and the native agent calls it at the start
of every turn. The regression builds a native session against a runtime that
was never opened and asserts the tool is discovered, gated, approved, and
called.

## Validation

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass. Three
  findings in the new code (an unused accessor, `manual_repeat_n`, and an
  unused helper left by a stale edit) were fixed before the final run.
- `cargo test --workspace --no-fail-fast`: pass, 324 tests, 0 failed, over
  three consecutive runs.
- `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke`:
  pass. The single configured entry, `gritt`, is ready on protocol
  `2025-06-18` with 3 tools. No tool was called.

Five tests were added and two rewritten: three in `mcp_runtime.rs`, one in
`mcp_native_session.rs`, and three unit tests for the SSE budget replacing the
two that described the old accounting.

Two of the new regressions were checked against the unfixed code: reverting
the bearer-token variants fails the HTTP echo test, and reverting the
pre-write cancellation check fails the queued-cancellation test. The others
assert states that the unfixed paths could not reach.

## Note on the previous session

This round was started once before and interrupted. Findings 1, 2, 3, 4, and 7
were already implemented in the worktree when work resumed; findings 5 and 6,
every regression, and the validation were completed here. One file was briefly
reverted by mistake during recovery and restored from the stash object, then
de-duplicated against the interrupted session's version.

---

# 2026-09-05 Round 4 review fixes

## Trigger

The fourth review returned `needs-fix` with three confirmed defects: one High
and two Medium. Round-3 fixes 2, 3, 5, and 7 were confirmed. The remaining
three were the last narrow edges of finding groups already worked twice: two
more windows in launch ownership, an eviction bound on the cancellation
record, and a framing case the SSE budget still disagreed with the parser on.

## Changes per finding

**1. High — launch ownership through queued starts and stale cleanup.** Two
windows remained. `connect_inner` spawned the process and only then asked
whether shutdown had begun, so a launch refused during shutdown had already
created a child and then awaited its cleanup unregistered. And `start`
unregistered a stale result before closing it, so between those two steps the
launch map could be empty while the child was still going away. Ownership is
now reserved before anything is spawned: the slot is claimed first and holds
`None` until the connection exists, a launch that starts during shutdown is
refused before any process is created, and a spawn that fails releases the
slot at once. A stale result keeps its slot until its close has finished.
Two regressions: eight servers against a concurrency limit of two, with
shutdown landing while some are queued and others mid-handshake, asserting
every started child is gone as shutdown returns and that nothing started
afterwards; and a result whose entry was stopped mid-handshake, asserting its
child is gone before `start` returns. Restoring the old ordering fails the
second.

**2. Medium — a cancellation could be forgotten before its write.** The
record was a fixed ring of 1,024 ids shared by the whole connection, and
cancelled admission waiters whose requests never reached the queue consumed
slots in it, so heavy concurrent use could evict an earlier queued call's
cancellation and let it onto the wire. The record is no longer shared: the
flag travels on the request command itself, so it cannot be evicted by
unrelated traffic and it is released when the command is. That removes the
bookkeeping rather than resizing it. The regression cancels a queued call,
then floods the connection with 1,500 later cancellations while the writer is
blocked, and asserts the first call still never reaches the server.

**3. Medium — bare-CR framing was rejected.** The budget counted a lone `\r`
as ordinary content while the provider's SSE parser treats it as a line
ending, so a CR-framed stream looked like one event that never ended and many
small valid events would fail the event bound. The budget now applies the
parser's own rule: a line ends at `\n`, `\r`, or `\r\n`, and a blank line ends
an event, with the carriage-return state carried across chunk boundaries.
Tests mirror the LF and CRLF ones: CR framing within a chunk, split across
chunks, and fifty CR-framed events arriving both as one burst and byte by
byte.

## Optional follow-up applied

`stalled_auxiliary_posts_cannot_accumulate` now also asserts that an ordinary
call still succeeds while all the stalled auxiliary work is parked, which is
what distinguishes bounded admission from work merely queued behind a
semaphore, and it polls for the endpoint to drain rather than sampling
mid-flight. The endpoint answers one connection at a time, so the burst and
its stall were shortened to keep the test quick.

## Validation

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace --no-fail-fast`: pass, 329 tests, 0 failed, over
  three consecutive runs.
- `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke`:
  pass. The single configured entry, `gritt`, is ready on protocol
  `2025-06-18` with 3 tools. No tool was called.

Five tests were added and one strengthened: three in `mcp_runtime.rs` and two
unit tests for CR framing.

Two of the new regressions were checked against the unfixed code: restoring
the unregister-before-close ordering fails the stale-cleanup test, and
neutralising the pre-write cancellation check fails the queued-cancellation
test.

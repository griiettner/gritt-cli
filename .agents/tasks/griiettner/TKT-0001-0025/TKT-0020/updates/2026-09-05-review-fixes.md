---
id: TKT-0020
namespace: griiettner
title: Review fixes for the hardening step
artifact: update
status: done
owner: griiettner
created: 2026-09-05
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
---

# TKT-0020 Update: review fixes

## Trigger

Reviewer verdict `needs-fix` on PR #12 at `a296243`: one High, four Medium,
one Low, all confirmed. Findings 1, 2, 3, 4, and 6 are resolved here. Finding
5, the human real-terminal walkthrough, cannot be performed by an agent and
stays pending with its checklist.

The High finding was correct and the most valuable thing in the review. The
benchmark measured the reducer and the renderer and then reported the results
as though they described the loop. They did not, and two of the report's
claims were wrong because of it.

## Finding 1: measure the production load path

**What was wrong.** `tui_responsiveness.rs` computed a hypothetical backlog
and drained it before every draw. The real loop takes one message per wakeup
and redraws, and `ChannelUi::event` sends every streaming text delta through
the same unbounded channel as every other completion. The report's statement
that "every producer is a discrete completion rather than a stream" was
false. Cancellation was never executed through the runtime handler, MCP
startup and the 1 MiB result ran separately on reduced transcripts, and the
launch harness always passed `--no-models`.

**What landed.**

- `crates/gritt-harness/src/tui/run.rs` gains `LoopHarness`, a `#[doc(hidden)]`
  seam over the loop's own state that forwards to the same `on_message` and
  `on_action` the product calls, and exposes the production `Ui`, the real
  queue depth, and one-message-per-wakeup stepping. It adds no behavior.
- `crates/gritt-harness/tests/tui_load.rs` runs the plan's whole scenario as
  one workload against it: a 10,000-message transcript, deltas produced at
  1,000/s through the production `Ui`, four real MCP fixture servers of which
  one never answers `initialize`, a 1 MiB tool result mid-stream, a synthetic
  user typing throughout, and a cancellation executed through the runtime
  handler that asserts the turn's cancellation token fired and the frame
  changed.
- `tui_responsiveness.rs` is relabelled throughout as microbenchmarks, with
  the module doc stating that its numbers may not be quoted as evidence about
  the loop and pointing at `tui_load.rs` instead. Its misleading
  `sustained_output_render_work_and_queue_depth` is renamed
  `render_work_per_frame_while_appending_output` and its "backlog" is
  labelled synthetic.
- `tui_bench.rs` gains `launch_to_usable_composer_with_a_pending_catalog_request`,
  which starts without `--no-models` against a provider that accepts the
  connection and never answers.

**What the honest measurement showed.** The first run on the real loop:

| Measure | Before any fix |
| --- | --- |
| Messages handled | 693 of 9,552 produced (**69/s** against 1,000/s) |
| Queue depth | peak **8,863**, final 8,862 |
| 1 MiB tool result | never reached the transcript; still queued at the end |

The loop drew a frame per message, so throughput was capped at the frame rate
while the queue grew without bound. The plan requires coalesced updates, a
30 fps render cap, and bounded queues, and none of the three existed.

**The product fix.** `event_loop` now caps drawing at 30 fps
(`FRAME_INTERVAL`, with a `sleep_until` branch so an owed frame is never
late), coalesces by handling every message already waiting before the next
draw (`MAX_COALESCED_MESSAGES`, a ceiling on work between frames rather than
on the queue), and biases its `select!` so input is taken before a queued
message. Nothing is dropped: every message reaches the same handler in the
same order, and only the intermediate frames between them are skipped.

| Measure | Before | After |
| --- | --- | --- |
| Drain rate | 69/s | **975/s** |
| Queue peak | 8,863 | **52** |
| Queue final | 8,862 | **0** |
| Frame rate under load | ~78 fps | **20 fps** (capped) |
| 1 MiB result delivered | no | yes |
| Cancel under load | not exercised | **2.664 ms**, token fired |

Input-to-frame under sustained load is p50 47.204 ms, **p95 52.488 ms**,
against the 50 ms budget: **NOT MET**, by 2.5 ms. The cycle is one coalesced
drain plus one frame, and the frame is the full-transcript rebuild that
follow-up 1 describes. This is the same root cause as the memory plateau, now
visible from a second direction.

## Finding 2: catalog readiness before eager resolution

Confirmed and fixed. `main.rs` warmed the catalog in a spawned task
regardless of path, so `tui --session fresh --model retired-id` could resolve
against an empty catalog and persist the retired identifier. The warm is now
awaited on the eager path, where `plane.open` resolves and persists a model
before the first frame, and stays in the background on the lazy path, which
opens nothing until a prompt is submitted and warms during draft validation.
Launch responsiveness is unchanged for the ordinary `gritt tui` case.

Regression: `a_new_named_session_resolves_its_model_against_the_loaded_catalog`
serves a model list after a 2.5 s delay in which `retired-model` declares
`current-model` as its replacement, then reads the persisted row back with
`gritt session list`. Verified in both directions: it fails with the fix
reverted, reporting `.../retired-model`.

One thing that made this harder to see is worth recording: the model cache is
per profile under the user cache directory, so a test using a profile named
`local` is served whatever an earlier run left there and the race never
appears. The test generates a unique profile name to guarantee a cold catalog.

## Finding 3: surface background MCP startup errors

Confirmed and fixed. `runtime.open` errors were discarded, so a malformed or
unreadable `.mcp.json` failed before any entry was published and the sidebar
said "no MCP servers configured", which is what an empty workspace looks like.

Opening moved into the interface: `run_tui` takes the runtime to start,
`open_mcp` starts it in the background, and a failure arrives as
`UiMsg::McpStartupFailed`, which pushes an error entry and a notice. The
reason is a configuration error, which names fields and variables and never
their values (ADR-008). Startup stays asynchronous.

Regression: `a_malformed_mcp_configuration_is_reported_in_the_interface`
writes invalid JSON, asserts the interface shows `MCP configuration`, and
asserts it does not claim no servers are configured.

## Finding 4: idle measurement independent of startup timing

Confirmed and fixed. The test assumed startup finished within three seconds
and then required exactly zero bytes; a connector probe has a 15 s deadline
and profile discovery can wait on the keychain, so a legitimate redraw at
second four would have failed it.

Startup dependencies are now controlled (`PATH` is pointed at a directory
with no executables, so no agent probe runs a real binary), and both idle
tests wait for actual silence with `wait_until_quiet` before measuring. The
zero-byte assertion is unchanged once quiescent, which is what catches the
periodic redraw.

Also applied from the optional list: a `ps` sample that is unavailable is now
reported as skipped instead of being converted to zero, which could have
printed a false `MET` for idle CPU.

## Finding 5: human real-terminal walkthrough

Not resolvable by an agent. It remains pending human verification, with the
seven-item checklist in `report.md`. The completion gate says so plainly.

## Finding 6: documentation corrections

- `docs/terminal-modes.md`: effort is no longer described as pinned, because
  `/effort` works on a live session and applies from the next turn. The `/new`
  description is corrected: a rejected provider or model choice is not
  carried, because the refusal happens before the draft is updated, so the
  order is `/new` first and then `/models`.
- `docs/terminal-modes.md` and `docs/getting-started.md`: both now describe
  the `Set up <name>…` and `Custom endpoint…` rows in `/connect`, and the
  `Set up <profile>…` row that heads `/models` when the selected profile has
  no key.
- `docs/tools-and-permissions.md`: no longer promises that a cold-start
  timeout succeeds on the next attempt. If the work is not resumable the next
  start begins again and hits the same deadline, which is what this
  repository's own server does.
- `ADR-013`: workspace observation is not injected by the binary; the loop
  constructs it. Corrected to two binary seams, with the MCP runtime handoff
  added as what the binary does pass in.
- Optional items applied: `docs/providers.md` states that capability records
  describe what Gritt parsed rather than what a model can do, and
  `docs/tools-and-permissions.md` states that fingerprint invalidation
  excludes formatting and key-order changes and the rotation of a named
  variable's value.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | 494 passed, 0 failed |
| `cargo test --manifest-path .agents/cli/Cargo.toml` | 107 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | 13 passed, 0 failed |
| `GRITT_LIVE_MCP_TESTS=1 ... mcp_live_smoke` | 1 passed; `gritt` ready, 3 tools, 2025-06-18 |
| `GRITT_LIVE_CONNECTOR_TESTS=1 ... live` | 3 passed; codex 0.153.1, claude 2.1.261 |
| `GRITT_LIVE_TESTS=1 -p gritt-provider --test live` | 3 skipped honestly, no keys set |
| `GRITT_BENCH=1 ... --test tui_load` (release) | 1 passed |
| `GRITT_BENCH=1 ... --test tui_responsiveness` (release) | 6 passed |
| `GRITT_BENCH=1 ... --test tui_bench` (release) | 5 passed |

Workspace tests moved from 490 to 494: the combined-load case, the
pending-catalog launch case, and the two startup regressions.

## Remaining follow-up

The report's list stands, with follow-up 2 now closed: the loop coalesces and
the queue stays bounded under the plan's load. What replaces it is narrower
and recorded in its place: the `UiMsg` channel is still an unbounded channel,
now with a drain that keeps it short in practice rather than a capacity that
makes it impossible to grow.

Follow-up 1 gains a second piece of evidence. Limiting rendering to visible
content would close the memory plateau, the render-work margin, and the
2.5 ms input-under-load overshoot, all of which are the cost of rebuilding
the whole transcript per frame.

---

# Round 2

## Trigger

Second reviewer verdict `needs-fix` on PR #12 at `eea68c2`: two High and two
Medium, all confirmed. Round 1's findings 2, 3 and 6 were accepted. The human
walkthrough remains an external dependency rather than a worker defect.

The first High is the one worth reading: the frame cap I added in round 1 to
meet the idle budget introduced a safety defect. Fixing one budget broke
something that matters more than a budget.

## Finding 1: approvals could be accepted before being displayed

**What was wrong.** `y` approves the moment it reaches the reducer. The frame
cap can suppress the frame after an approval arrives, so a keystroke landing
in that window approved a tool call or an MCP trust launch that had never
been on screen. Before the cap the loop drew on every iteration, so the
question could not arise. This is a permission-boundary defect, not a
rendering one.

**What landed.** Two guards, one of which makes the other unreachable and is
kept so that it stays unreachable:

- the frame cap yields to an approval that has not been drawn, so the draw
  at the top of a step always precedes the `select!` that reads a key;
- a decision key arriving while an approval is pending but undrawn is
  discarded rather than applied.

**Regression.** `an_approval_is_drawn_before_a_decision_key_is_accepted`
runs the real `Scheduler` against a `TestBackend` with `start_paused`, so no
time passes between the first frame and the approval and the cap is engaged
deterministically instead of by racing it. It asserts the frame that shows
`approve?` precedes the decision reaching the responder. Verified to fail
with the guards removed: the decision is sent while the prompt has never
been drawn. This added `tokio`'s `test-util` feature as a dev-dependency of
`gritt-harness`; nothing in the product depends on it.

## Finding 2: the benchmark duplicated the scheduler instead of driving it

**What was wrong.** The round 1 benchmark called shared handlers but kept its
own copy of the scheduling rules, and the copy differed from production: it
skipped the drain after input, polled instead of using the deadline branch,
started latency timing when it processed a key rather than when the key was
queued, and cancelled after aborting the producer while drawing past the cap.
Numbers from it could not establish input latency, cancellation under load,
or protect the scheduler from regressing.

**What landed.** The loop body is extracted into `Scheduler::step`, generic
over the `ratatui` backend. `event_loop` is now `while !quit { step() }`
around it, and `LoopHarness` exposes the same `step` against a `TestBackend`
with an injected input channel. There is one implementation of draw timing,
drain order, and input priority, and both callers use it.

The benchmark accordingly: input is queued onto the channel the key reader
thread writes to and timestamped **when queued**, so its scheduling wait is
inside the measurement; cancellation is a real Escape delivered through that
channel halfway through the run while the stream is still producing, with the
latency measured to the frame after the turn's token fired; and queue
evidence is the largest batch one step drained, because `step` drains inside
itself and sampling from outside always finds the channel empty.

**Revised numbers.** Driving the real scheduler moved two classifications:

| Measure | Round 1 (copied scheduler) | Round 2 (production scheduler) |
| --- | --- | --- |
| Input to frame under load, p95 | 52.488 ms, NOT MET | 48.9 to 51.1 ms over six runs, **NOT MET**, on the line |
| Cancel under load | 2.664 ms, after the producer stopped | **27.9 to 52.0 ms**, during the stream |
| Queue evidence | peak depth 52 | largest batch drained in one step 52, empty at the end |
| Drain rate | 975/s | 954 to 969/s |
| Frame rate | 20 fps | 19 fps |

The input figure straddles the budget rather than clearing or missing it
cleanly, clearing it in two runs of six, and the range is reported rather
than a single run so that is visible. It is recorded as missed.

## Finding 3: the pending-catalog test overwrote its own fixture

`Session::start` unconditionally rewrote `config.toml`, replacing the
stalling provider with `127.0.0.1:9`, so the test measured a connection
refusal rather than a request left hanging, and the shared `local` profile
allowed cache reuse. Config writing is now split out
(`start_with_existing_config`), the profile name is unique per run so the
catalog is cold, and the fixture counts request lines it has read and holds
open. The test asserts at least one request reached it and stayed pending.

## Finding 4: idle synchronization still mistook silence for completion

Two seconds of silence is not proof that startup finished, because profile
discovery reaches the operating system keychain before it looks at the
environment and that call has no bound the test controls. The idle tests now
run against a configuration with **no profiles at all**, so there is nothing
to resolve and no keychain call; combined with the empty `PATH` that already
suppressed agent probes, every asynchronous startup dependency is controlled.
The quiescence wait is kept as well, and the zero-byte assertion is
unchanged.

## Optional items

The report summary no longer says eight of nine budgets pass, and ADR-013's
section heading no longer claims workspace observation is injected; both now
match their corrected bodies.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | 495 passed, 0 failed |
| `cargo test --manifest-path .agents/cli/Cargo.toml` | 107 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | 13 passed, 0 failed |
| `GRITT_LIVE_MCP_TESTS=1 ... mcp_live_smoke` | 1 passed; `gritt` ready, 3 tools |
| `GRITT_LIVE_CONNECTOR_TESTS=1 ... live` | 3 passed |
| `GRITT_LIVE_TESTS=1 -p gritt-provider --test live` | 3 skipped honestly |
| `GRITT_BENCH=1 ... --test tui_load` (release) | 1 passed |
| `GRITT_BENCH=1 ... --test tui_responsiveness` (release) | 6 passed |
| `GRITT_BENCH=1 ... --test tui_bench` (release) | 5 passed |

## Remaining follow-up

Unchanged. Follow-up 1 now accounts for three separate results: the memory
plateau, the render-work margin, and the input-under-load figure sitting on
its budget. The human real-terminal walkthrough is still outstanding and is
still the chain's to close.

---

# Round 3

## Trigger

Third reviewer verdict `needs-fix` on PR #12 at `c2548d5`: two High and one
Medium, all confirmed. The pending-catalog fixture, the scheduler
extraction, the discard guard for a drawn approval, and the dev-only tokio
feature were all accepted.

Both High findings were about the same kind of mistake in two places:
tracking a proxy for the thing that matters instead of the thing itself.

## Finding 1: a second approval could inherit the first one's visibility

**What was wrong.** The guard added in round 2 tracked whether *an* approval
had been drawn, not *which* one. A single iteration can answer A and install
B during its coalescing drain, because the turn that receives A's decision
enqueues B from another worker. `pending` is `Some` before and after, so the
flag stayed set, the cap suppressed B's frame, and the next decision key
answered a prompt that had never been drawn.

**What landed.** `App` now stamps each installed request with a counter and
exposes `pending_install()`; both install sites go through one method so a
new request cannot be installed without a new identity. The scheduler holds
`drawn_approval: Option<u64>` and compares identities, so a request that
replaced an answered one is undrawn even though `pending` never became
`None`.

**Regressions.** `a_second_approval_does_not_inherit_the_first_one_s_visibility`
drives one step that answers A and installs B, then queues the next
decision key, on a paused clock. It asserts B is drawn on its own account
and that A's decision reached A's responder. Verified to fail with identity
replaced by presence: "B inherited A's visibility". Both approval tests now
also assert the legitimate decision **is** delivered, so the guard cannot
pass by swallowing the key instead of delaying it.

## Finding 2: latency credited to a frame that preceded the input

**What was wrong.** `step` draws before it takes input and drains messages,
so the frame a step produces shows the state as of the end of the previous
step. The benchmark credited every timestamp available when `step` returned
to that step's frame, so a key consumed after the draw, or still queued,
counted as displayed. The cancellation number had the same flaw.

**What landed.** Attribution is now ordered against that fact: a drawn frame
first completes whatever earlier steps handled, and only then is what this
step handled recorded as waiting for a later frame. Typed keys are counted
by what actually reached the composer, so the one-off Escape is never
mistaken for one. `a_step_draws_before_it_handles_input` is the deterministic
guard: it asserts a step that both draws and takes a key produces a frame
**without** that key on it, and that the key was nonetheless applied.

**Revised numbers.** The correction made the result substantially worse,
which is why it mattered:

| Measure | Round 2 (mis-attributed) | Round 3 (corrected) |
| --- | --- | --- |
| Input to frame under load, p50 | 19.8 to 29.4 ms | **40.9 to 47.7 ms** |
| Input to frame under load, p95 | 48.9 to 51.1 ms, "on the line" | **66.6 to 78.9 ms, NOT MET by 1.3 to 1.6x** |
| Cancel under load | 27.9 to 52.0 ms | **21.6 to 49.7 ms**, still MET |

The budget is now missed clearly rather than marginally. The cause is
unchanged and is follow-up 1: one cycle is a coalesced drain plus a
full-transcript frame, and a keystroke waits for the cycle in progress.

## Finding 3: idle startup still inherited the user configuration layer

**What was wrong.** An empty project `config.toml` does not produce an empty
configuration. The user layer is loaded underneath it and `merge` extends the
profile map rather than replacing it, so a developer's own profiles still
reached `profile_summaries` and its keychain calls, and user defaults could
have invalidated the unconfigured-composer expectation.

**What landed.** Every benchmark child now runs with `HOME` and the `XDG_*`
directories pointed at a fresh empty temp directory, which is what
`dirs::config_dir()` and `dirs::cache_dir()` are derived from, so the user
config layer and the model cache are both out of the picture. The idle tests
verify the result rather than assuming it: `assert_no_profiles_resolve` runs
`gritt doctor` under the same environment and asserts no profile line
appears, which is the evidence that nothing will reach the keychain.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | 497 passed, 0 failed |
| `cargo test --manifest-path .agents/cli/Cargo.toml` | 107 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | 13 passed, 0 failed |
| `GRITT_LIVE_MCP_TESTS=1 ... mcp_live_smoke` | 1 passed; `gritt` ready, 3 tools |
| `GRITT_LIVE_CONNECTOR_TESTS=1 ... live` | 3 passed |
| `GRITT_LIVE_TESTS=1 -p gritt-provider --test live` | 3 skipped honestly |
| `GRITT_BENCH=1 ... --test tui_load` (release) | 1 passed |
| `GRITT_BENCH=1 ... --test tui_responsiveness` (release) | 6 passed |
| `GRITT_BENCH=1 ... --test tui_bench` (release) | 5 passed |

Workspace tests moved from 495 to 497: the second-approval transition and the
draw-order guard.

## Remaining follow-up

Unchanged. Follow-up 1 now accounts for the memory plateau, the render-work
margin, and an input-under-load figure that misses its budget outright rather
than sitting on it. The human real-terminal walkthrough is still outstanding
and is still the chain's to close.

---

# Round 4

## Trigger

Fourth reviewer verdict `needs-fix` on PR #12 at `7636a8d`: one confirmed
Medium, plus two low optional items. The approval identity fix and the
configuration isolation were accepted.

## Finding 1: the completion timestamp was still the wrong one

**What was wrong.** Round 3 fixed *which* frame completes a keystroke. It did
not fix *when* that frame completes. The benchmark took `Instant::now()` after
`step()` returned, but the frame is written near the top of `step`, and the
step then waits in `select!`, handles an action, and drains the queue. All of
that is scheduler time with the frame already on screen, and it was being
charged to the keystroke. The round 3 draw-order test could not catch this,
because it checks ordering rather than timing.

**What landed.** `Step` carries `drew_at`, recorded inside `step` immediately
after the draw completes, and the benchmark attributes both input and
cancellation latency to that instant.

**Deterministic coverage.**
`a_step_records_when_its_frame_was_drawn_not_when_it_returned` owes a frame
and leaves both queues empty, so the step draws and then waits out the 50 ms
tick with nothing to do. It asserts the gap between `drew_at` and the moment
`step` returns is at least 20 ms, which is exactly the interval that used to
be counted against the keystroke. It runs on the real clock, because the
point is to observe the gap.

**Revised numbers.**

| Measure | Round 3 (stopped too late) | Round 4 (corrected) |
| --- | --- | --- |
| Input to frame under load, p50 | 40.9 to 47.7 ms | **38.6 to 45.8 ms** |
| Input to frame under load, p95 | 66.6 to 78.9 ms | **62.8 to 67.1 ms** |
| Input to frame under load, max | 70 to 98 ms | **67.9 to 70.0 ms** |
| Cancel under load | 21.6 to 49.7 ms | **19.7 to 52.2 ms** |

The verdict is unchanged: input to frame under a saturating 1,000 deltas a
second misses the 50 ms budget, now by about 1.3 times rather than 1.3 to
1.6. Cancellation remains comfortably inside its 100 ms budget. Drain rate
970/s, 19 fps, largest batch 52, queue empty at the end.

## Optional items, both applied

**The profile probe only checked for absent strings.** It now asserts the
diagnostic actually ran, and asserts the affirmative result
(`no profiles configured`) rather than only the absence of credential
strings. That immediately earned its keep: the exit-status assertion caught
the probe failing with a database lock error, because it shared the running
fixture's session database. It now uses its own, and the previous version
would have passed silently on a diagnostic that never produced any output.

**The report quoted an obsolete 2.5 ms overshoot** in follow-up 1. It now
carries the measured p95 range.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | 498 passed, 0 failed |
| `cargo test --manifest-path .agents/cli/Cargo.toml` | 107 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | 13 passed, 0 failed |
| `GRITT_LIVE_MCP_TESTS=1 ... mcp_live_smoke` | 1 passed; `gritt` ready, 3 tools |
| `GRITT_LIVE_CONNECTOR_TESTS=1 ... live` | 3 passed |
| `GRITT_LIVE_TESTS=1 -p gritt-provider --test live` | 3 skipped honestly |
| `GRITT_BENCH=1 ... --test tui_load` (release) | 1 passed |
| `GRITT_BENCH=1 ... --test tui_responsiveness` (release) | 6 passed |
| `GRITT_BENCH=1 ... --test tui_bench` (release) | 5 passed |

Workspace tests moved from 497 to 498: the draw-timestamp guard.

## Remaining follow-up

Unchanged. Four rounds of review moved the input-to-frame figure three
times without changing its verdict, and the cause named in follow-up 1 has
been the same throughout: the whole transcript is rebuilt for every frame.
The human real-terminal walkthrough is still outstanding and is still the
chain's to close.

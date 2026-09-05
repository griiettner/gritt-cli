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

---
id: TKT-0020
namespace: griiettner
title: Complete documentation, performance benchmarks, and integrated TUI hardening
artifact: report
status: done
owner: griiettner
created: 2026-09-05
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0019
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
  - docs
  - .agents/plans
skills:
  - tkt
  - tkt-exec-chain
  - dev-harness
  - dev-provider
  - codebase-design
  - tdd
  - write-plan
---

# TKT-0020 Report: Complete documentation, performance benchmarks, and integrated TUI hardening

## Summary

The chain's last worker step. Four documents now describe the interface as it
was built, one ADR records the durable contract changes the chain accepted, a
deterministic responsiveness harness measures every budget the feature plan
states against the production scheduler, and the defects the measurements
exposed are fixed.

Most of the plan's budgets are met. Two are not, and both are named rather
than softened: the resident-memory plateau over a five-minute soak, and
input-to-frame under a saturating 1,000 deltas a second, which sits at the
50 ms budget and crosses it about as often as not. Both have the same cause,
the renderer materializing every transcript entry on each rebuild with no
history paging, so memory tracks the transcript and a frame costs what the
whole transcript costs. The full table and the measurement method are under
[Benchmarks](#benchmarks); the numbers there come from the production
scheduler, after two rounds of review corrected how they were obtained.

Six fixes landed, all of them responsiveness, startup-correctness, or
secret-accuracy defects the plan names as acceptance requirements, three of
them found by review:

1. The full-screen loop drew on every wakeup, including a 50 ms tick with no
   input behind it. An idle session burned 2.5% of a core and wrote about 540
   bytes a second to the terminal forever. Nothing in the interface is drawn
   from a clock, so the loop now draws only after a wakeup that could have
   changed the screen. Idle CPU fell to 0.2% and idle terminal traffic to
   zero.
2. The binary awaited MCP startup and a model-list fetch before entering the
   alternate screen. The plan's launch budget is explicitly "independent of
   provider/MCP readiness", and a server that never answers `initialize` held
   a blank terminal for the whole 30 s deadline. Both are background work now.
3. A doc comment on `McpRuntime::definition_summary` claimed a guarantee about
   arguments that the code does not make. Corrected to state what is actually
   true.

Live checks: the one `.mcp.json` entry initializes and lists its tools; both
installed connectors ran; provider tests skipped honestly with no keys set.

## Key Decisions

**The redraw gate is a fix, not a follow-up.** Two of the plan's budgets (idle
CPU, no continuous full redraw) failed on one cause, the cause was understood,
and the change is about ten lines in two loops with no time-based rendering to
break. Recording it as a follow-up would have left the parent's "responsiveness
evidence meets or explains the plan's budgets" resting on an explanation where
a fix was cheaper than the explanation.

**Pre-terminal waits move to the background rather than being deleted.** The
model list is still warmed at startup, just not awaited, so `/models` is still
populated for a user who opens it a moment later. The MCP runtime is opened
through `McpRuntime::open` directly rather than `start_mcp`, because
`start_mcp` reports each server on stderr and the alternate screen owns stderr
from that point; the interface reads the same states from the lifecycle
subscription it already subscribes to.

**The 30 s MCP initialization default is unchanged.** The measurement that
would justify a change is real: this repository's own server answered
`initialize` in 43.1 s on its first start in a fresh worktree, indexing 292
files first. Raising the global default to accommodate one server's one-time
cold start would slow failure detection for every server in every workspace,
which is the thing the deadline exists to do. The right fixes are exposing
`McpRuntimeSettings` in `config.toml` (already a recorded follow-up from
TKT-0017 and TKT-0019) or making that server index lazily. Both are outside
this ticket. The deadline is documented instead, with the failure mode stated.

**Budgets are reported, bounds are asserted.** The harness prints a MET or
NOT MET line against each of the plan's numbers, and separately asserts loose
regression bounds roughly an order of magnitude above the measured values. A
percentile measured on a shared machine is not a thing to gate a test suite
on; an order-of-magnitude regression is. The memory plateau is reported only,
never asserted, because it is a property of the machine and the soak length.
What is asserted there is the growth *rate* in bytes per delta, which is a
property of the code.

## Alternatives Considered

**Fixing the memory plateau.** The root cause is that
`render::transcript_lines` builds wrapped lines for every entry on each cache
rebuild, and the app slices the visible window out of the result. Limiting
rendering to visible content would fix both the plateau and the 15 ms render
work at once. It is also a substantial change to the render path in the last
step of a five-worker chain, with 19 snapshot goldens riding on it, and the
plan's own acceptance criterion permits "identifies specific remaining
performance gaps". Recorded as the first follow-up, with the measurement that
justifies it.

**Capping the retained transcript.** Dropping the oldest entries would flatten
the memory curve in a few lines, but without the ability to page history back
in it silently loses scrollback, which is a worse product behaviour than the
memory growth and a new product decision this ticket has no mandate to make.

**A memory-sampler dependency.** Rejected. `ps -o rss=` and `ps -o time=`
report the two numbers needed, are present wherever this suite runs, and cost
no dependency review. No new dependency was added by this ticket.

## Assumptions

1. **"Usable composer" means the composer placeholder is on screen.** The
   wordmark is block-drawing glyphs and carries no matchable text, so the
   launch measurement waits for `Ask Gritt to do something`, which is drawn
   only once the input exists and the loop is reading keys.
2. **A latency sample is the reducer call plus the frame that answers it.**
   That is what the run loop does between reading a key and putting a frame
   up. It is not presentation latency, and the report says so rather than
   implying the two are interchangeable.
3. **The scaled default run is not the recorded run.** The suite runs a
   1,000-message transcript and a 5 s soak by default; `GRITT_BENCH=1` runs
   the plan's 10,000 messages and the full five minutes. Every recorded number
   below is from the `GRITT_BENCH=1` release run, and the soak line prints its
   own length so a scaled run cannot be mistaken for the full one.
4. **The catalog is still warmed at startup, just not awaited.** A different
   choice, dropping it entirely, would have left `/models` empty on a cold
   start until the user opened it. The warm is silent because stderr belongs
   to the alternate screen.
5. **The live MCP smoke result required letting the index finish once.** The
   30 s deadline killed indexing before it completed, so the entry could never
   reach `ready` in this worktree: each attempt re-indexed from an incomplete
   write-ahead log. The index was allowed to complete once by a handshake
   probe that sent `initialize` and `tools/list` and nothing else. No tool was
   called and no tool result was requested.
6. **ADR-013 covers all four contract areas in one document** rather than four
   amendments to ADR-007, ADR-008, ADR-009, and ADR-012. The chain accepted
   them together and they reference each other; splitting them would have made
   the MCP trust rule readable in two places and complete in neither.

## Edge Cases and Failures

**The idle defect was invisible to every existing test.** Ratatui writes only
changed cells, so a redundant frame costs a cursor move and nothing else. No
assertion anywhere looked at how much a terminal received while nothing
happened. The regression test that now guards it asserts zero bytes over three
idle seconds, which is the only honest number: nothing is animated, so
anything above zero is the defect returning.

**The startup defect only appeared under a hung server.** With a healthy or
absent `.mcp.json`, launch measured 27 ms and looked fine. Writing the process
cleanup test, which deliberately configures a server that never speaks MCP, is
what produced a 60 second blank screen and exposed it. The cleanup test now
also serves as the regression guard: it could not pass before the fix.

**The live MCP smoke test cannot self-heal.** Recorded in full above. First
start indexes 292 files and takes 43.1 s against a 30 s deadline; the kill
leaves an incomplete index, so the next attempt starts over. Once the index
exists the check passes in 1.11 s.

**Render work at p95 is 14.979 ms against a 16 ms budget.** It passes, but the
margin is one millisecond, and it is measured with the transcript growing for
the whole run. This is the same root cause as the memory plateau and it will
be the first budget to break. Named in the follow-ups.

## Validation

Environment for every recorded number: Apple M1 Max, 10 cores, 32 GiB, macOS
26.3.1 build 25D771280a, arm64. `rustc 1.100.0-nightly (2e2b193f8 2026-09-02)`
on the pinned `nightly-2026-09-03` toolchain. Release profile. Base commit
`4bdbac3`. The PTY runs use a `portable-pty` pseudo-terminal with
`TERM=xterm-256color` and `NO_COLOR=1`; no graphical terminal emulator was
involved and no human ran any of it.

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | pass, 494 passed, 0 failed |
| `cargo test --manifest-path .agents/cli/Cargo.toml` | pass, 107 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | pass, 13 passed, 0 failed |
| `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke` | pass, 1 passed (see below) |
| `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live` | pass, 3 passed |
| `GRITT_LIVE_TESTS=1 cargo test -p gritt-provider --test live` | pass, 3 skipped honestly |
| `GRITT_BENCH=1 cargo test --release -p gritt-harness --test tui_load -- --nocapture --test-threads 1` | pass, 1 passed |
| `GRITT_BENCH=1 cargo test --release -p gritt-harness --test tui_responsiveness -- --nocapture --test-threads 1` | pass, 6 passed |
| `GRITT_BENCH=1 cargo test --release -p gritt --test tui_bench -- --nocapture --test-threads 1` | pass, 5 passed |

Workspace test count moved from 478 (TKT-0019) to 494: six microbenchmarks in
`tui_responsiveness`, the combined-load case in `tui_load`, five in
`tui_bench`, and three in `tui_pty` (process cleanup with a hung MCP server,
catalog readiness on the eager path, and malformed MCP configuration).

Docs link check: the repository has no link checker, in `scripts/`, in CI
configuration, or in the agent CLI. An ad-hoc check of every relative
Markdown link in `docs/`, `README.md`, `AGENTS.md`, and `CLAUDE.md` resolved
all 36 targets. External links were not fetched; this environment has no
network access to providers.

### Live smoke results

Per `.mcp.json` entry (the file declares exactly one):

| Entry | Transport | Result |
| --- | --- | --- |
| `gritt` (`.agents/gritt-agent mcp serve`) | stdio | **ready**, protocol `2025-06-18`, 3 tools (`search_local_memory`, `read_local_memory`, `delegate_run`), 1.11 s |

First start in this fresh worktree failed twice at the 30 s deadline while
indexing 292 local knowledge files. A direct handshake probe measured
`initialize` answering in **43.1 s** on that first start, then `tools/list` in
under 10 ms. No tool was called and nothing was written to the memory database
beyond the indexing the server performs on its own startup.

Per connector:

| Connector | Result |
| --- | --- |
| `codex` | ran. version 0.153.1, authenticated. Smoke turn completed in 7.60 s; resume across two turns in 15.96 s |
| `claude` | ran. version 2.1.261, authenticated. Smoke turn completed in 3.10 s |
| `cursor` | not exercised; the live suite has no case for it |
| `opencode` | not exercised; the live suite has no case for it |

Provider live tests: all three protocols skipped and said so.
`OPENROUTER_API_KEY`, `OPENAI_API_KEY`, and `ANTHROPIC_API_KEY` are all unset
in this environment, so no request was made to any provider.

## Benchmarks

Recorded with `GRITT_BENCH=1` in a release build on the machine described
above. There are three harnesses and they do not carry equal weight.

- **`crates/gritt-harness/tests/tui_load.rs` is the authoritative one.** It
  runs the plan's whole scenario as one workload against `LoopHarness`, which
  forwards to the same `on_message` and `on_action` the product calls, with
  streaming produced through the production `Ui` on the real channel. Queue
  depth, drain rate, and input latency under load come from here.
- **`crates/gritt/tests/tui_bench.rs`** measures the two budgets that belong
  to the process rather than the reducer, launch and idle, against the real
  binary in a pseudo-terminal.
- **`crates/gritt-harness/tests/tui_responsiveness.rs` holds
  microbenchmarks.** Each case isolates the reducer or the renderer. They are
  useful for attributing a regression and are not evidence about the loop;
  the first review of this ticket correctly rejected an earlier version of
  this report for quoting them as though they were.

### Against the plan's budget table

| Scenario | Budget | Measured | Verdict |
| --- | --- | --- | --- |
| Launch with existing config | first usable composer within 500 ms | 24 ms | **MET** |
| Launch, model list requested and never answered | independent of provider readiness | 42 ms, with the request confirmed to have reached the fixture and stayed open | **MET** |
| Typing, input to frame (idle transcript, micro) | p95 below 50 ms | p50 2.155 ms, **p95 2.271 ms**, n=500 | **MET** |
| Picker navigation (micro) | p95 below 50 ms | p50 2.492 ms, **p95 2.660 ms**, n=500 | **MET** |
| Scrolling (micro) | p95 below 50 ms | p50 2.171 ms, **p95 2.306 ms**, n=500 | **MET** |
| Input to frame under 1,000 deltas/s (integrated) | p95 below 50 ms | p50 19.8 to 29.4 ms, **p95 48.9 to 51.1 ms over six runs** | **NOT MET**, at the budget and over it more often than not |
| Sustained output, delta drain rate (integrated) | keep up with 1,000/s | **954 to 969/s** | **MET** |
| Sustained output, render work at 120x40 (micro) | p95 below 16 ms | p50 14.511 ms, **p95 15.083 ms**, n=686 | **MET**, by 0.9 ms |
| Render cap under load (integrated) | 30 fps | **19 fps**, 192 frames in 10 s | **MET** |
| Bounded queues under load (integrated) | bounded, nothing dropped | largest batch drained in one step **52**, final **0** | **MET** |
| Cancel under load (integrated) | visible canceling state within 100 ms | **27.9 to 52.0 ms** from Escape queued to the frame after the turn's token fired | **MET** |
| Idle CPU over 30 s | below 1% of one core | **0.2%** | **MET** |
| Idle screen, no continuous full redraw | no redraw | **0 bytes** over 30 idle seconds | **MET** |
| Resident memory plateau over a five-minute soak | a stable plateau | baseline 38,800 KiB, peak **762,336 KiB**; middle third 532,352 KiB, last third 762,336 KiB | **NOT MET** |

Two budgets are missed. Both are named below rather than softened.

The integrated rows come from `Scheduler::step`, the loop's own iteration,
driven against a `TestBackend` with input queued onto the same channel the
key reader thread writes to. Input latency is timed from the moment a key is
**queued**, so its scheduling wait is inside the number, and the
cancellation is a real Escape delivered through that channel while the
stream is still producing. An earlier version of this report measured a
reimplementation of the scheduler and was rejected for it.

### What the loop fixes changed

Before this ticket's loop fixes, measured on the same scheduler:

| Measure | Before | After |
| --- | --- | --- |
| Messages handled | 693 of 9,552 produced (69/s) | 9,605 of 9,602 produced (**960/s**) |
| Backlog | grew to **8,863** and never recovered | largest batch **52**, queue empty at the end |
| Frames in 10 s | 777 (78 fps) | **192 (19 fps)** |
| 1 MiB tool result delivered | no, still queued | **yes** |

The loop drew a frame per message and a turn emits a text delta per token
through the same channel, so throughput was capped at the frame rate while
the queue grew without bound. It now caps drawing at 30 fps, handles every
waiting message before the next draw, and takes input ahead of a queued
message. Nothing is dropped: every message reaches the same handler in the
same order, and only the intermediate frames between them are skipped.

The frame cap introduced one defect of its own, found in the second review
and fixed here: it could suppress the frame after an approval arrived, so a
keystroke landing in that window would approve a tool call or an MCP launch
that had never been drawn. The cap now yields to an undrawn approval, and a
decision key is refused until the prompt has been on screen. See
[the review fixes update](updates/2026-09-05-review-fixes.md).

An earlier version of this report claimed the `UiMsg` channel's producers
were "only discrete completions rather than a stream". That was wrong.
`ChannelUi::event` sends every streaming event through it, which is exactly
why the queue grew.

### The two misses, stated specifically

**Input to frame under sustained load: p95 between 48.9 and 51.1 ms across
six runs, against a 50 ms budget.** It sits on the line, clearing it in two
runs of six, so it is recorded as missed rather than met. One cycle is a coalesced drain
of about fifty deltas plus one frame, and the frame is a full-transcript
rebuild; a keypress waits for the cycle in progress. The load is 1,000 deltas
per second, roughly ten to twenty times what a provider actually streams, and
the microbenchmark figure of 2.271 ms is what typing costs when output is not
saturating the channel. Both are reported because neither alone describes the
product.

**Resident memory over the five-minute soak.** Over 300 seconds the harness
delivered 2,791,000 text deltas into a 10,000-message transcript and consumed
294 s of CPU. Resident memory rose from 38,800 KiB to 762,336 KiB and was
still rising: the last third of the run peaked 229,984 KiB above the middle
third. Growth is linear and steady at **265 bytes per delta**, which is what
the regression test bounds, at 2 KiB per delta. This soak is a saturating
reducer and renderer loop delivering roughly 9,300 deltas a second, not the
integrated 1,000 a second scenario, and its numbers are scoped to that.

The cause of both is the same. `render::transcript_lines` builds wrapped
`Line` values for every entry in the transcript on each cache rebuild, the
app then slices the roughly 35 visible lines out of the result, and every
incoming delta invalidates the cache. History paging, which the plan asks for
("page old history rather than keeping every rendered line in memory"), does
not exist. Limiting rendering to visible content is the single change that
would close the plateau, the 0.9 ms render-work margin, and the input
overshoot together. It is follow-up 1.

### Other recorded figures

| Measurement | Result |
| --- | --- |
| First frame after loading a 10,000-message transcript | 13.250 ms (a load cost, kept out of the input series) |
| 1 MiB tool result (micro) | reduce 3.876 ms, next frame 2.755 ms. Held as entry detail and drawn only when `/details` is on |
| 1 MiB tool result (integrated) | delivered through the real channel mid-stream and present in the transcript at the end of the run |
| Several MCP servers plus one hung server, under load | 3 of 4 reached `Ready`; the hung entry became `Failed { "initialize did not answer within 5s" }` and did not hold the others |
| Cancel under load (micro, reducer only) | 18.149 ms worst of 50 rounds |
| Frames drawn while MCP initialized (micro) | p50 1.451 ms, p95 3.846 ms, n=382 |

Queue bounds, for the record: the MCP lifecycle broadcast is bounded at 32
messages and every message carries the whole snapshot list, so a lagged
subscriber loses intermediate frames and never correctness. The `UiMsg`
channel is still an unbounded channel; what changed is that the loop now
drains it every iteration, so the largest batch one step handled was 52 and
the queue was empty at the end of the run. Measuring it from outside the
loop always finds it empty, because `step` drains inside itself, so the
batch size is the honest figure. A capacity that makes growth impossible is
follow-up 2.

## Regression review

### Fixed

1. **Idle redraw and idle CPU** (`crates/gritt-harness/src/tui/run.rs`, both
   the main loop and the fixture loop). The loop drew unconditionally at the
   top of each iteration and a 50 ms tick guaranteed 20 iterations a second.
   Now a `dirty` flag is set by a key event, a paste, a resize, or a harness
   message, and cleared by the draw; the tick sets nothing. Verified as
   measured above, and guarded by
   `an_idle_session_writes_nothing_to_the_terminal`, which asserts zero bytes
   over three idle seconds.
2. **Pre-terminal waits on launch** (`crates/gritt/src/main.rs`). The
   full-screen path awaited `warm_catalog` and `start_mcp` before entering the
   terminal. Both are spawned now. Guarded by
   `quitting_the_full_screen_mode_leaves_no_mcp_server_running`, which
   configures a server that never speaks MCP and could not have reached its
   first frame before this change.
3. **No coalescing, no frame cap, and an unbounded queue under load**
   (`crates/gritt-harness/src/tui/run.rs`). Found by the reviewer and
   confirmed by measuring the real loop: it drew a frame per message, so a
   turn streaming a delta per token through the same channel drained at 69
   messages a second against 1,000 produced, and the queue reached 8,863 and
   never recovered. A 1 MiB tool result produced mid-stream was still queued
   when the run ended. The loop now caps drawing at 30 fps, handles every
   waiting message before the next draw, and biases its `select!` so input is
   taken before a queued message. Drain rate 975/s, queue peak 52, final 0.
   See the [review fixes update](updates/2026-09-05-review-fixes.md).
4. **Catalog readiness on the eager path** (`crates/gritt/src/main.rs`).
   Moving the catalog warm off the launch path let `tui --session NAME
   --model retired-id` resolve against an empty catalog and persist the
   retired identifier. The warm is awaited on the eager path, which resolves
   and persists a model before the first frame, and stays in the background
   on the lazy path.
5. **Discarded MCP startup errors** (`crates/gritt/src/main.rs`,
   `crates/gritt-harness/src/tui/run.rs`). A malformed `.mcp.json` failed
   before any entry was published, so the interface said no servers were
   configured. Opening moved into the interface, and a failure arrives as a
   message that shows the configuration error.
6. **Approvals could be accepted before being displayed**
   (`crates/gritt-harness/src/tui/run.rs`). Found in the second review, and
   introduced by fix 3: the frame cap could suppress the frame after an
   approval arrived, so a keystroke landing in that window approved a tool
   call or an MCP launch that was never drawn. The cap now yields to an
   undrawn approval, and a decision key is refused until the prompt has been
   on screen once. Guarded by
   `an_approval_is_drawn_before_a_decision_key_is_accepted`, which pauses the
   clock so the cap is engaged deterministically and fails with the guard
   removed.
7. **An overstated secret guarantee in a doc comment**
   (`crates/gritt-harness/src/mcp/mod.rs`). `definition_summary` claimed a
   value reaching an *argument* through `${TOKEN}` could not be echoed. It
   cannot, but because arguments are never interpolated at all, not because
   they are redacted. A literal secret written directly into `args` would be
   displayed. Corrected to say that, and to say why `args` cannot be refused
   the way credential-named `env` and `headers` fields are.

### Reviewed and clean

**Secret handling** across the TUI, MCP, setup, and provider paths was audited
for any route by which a key, token, or header value could reach a log, error,
event, snapshot, transcript, session row, or `Debug` output. No reachable path
was found. The evidence, in brief: `Secret` has manual `Debug` and `Display`
that both emit `[redacted]` and no `Serialize`; `McpTransport` has a manual
`Debug` printing only `env`/`headers` key names and no `Serialize`;
`SetupSubmission` derives nothing at all; the setup form's `secret` field is
private with no read accessor, is rendered as bullets, and is cleared as it is
taken; MCP failure reasons, tool results, tool errors, `server_version`, and
`definition_summary` are all passed through `redact_text`/`redact_value`
against the entry's own resolved credentials; HTTP errors report a status
without a body and reqwest errors are reduced to host and path with the query
dropped. Every `expose()` call site was enumerated: four `Bearer` formatters
that immediately re-wrap in `Secret`, one keychain write, one dedupe compare,
one accessor, and the connector's own leak *detector*. No print or log macro
anywhere takes a secret-bearing value. The PTY tests assert the configured key
string never appears in terminal output.

**Connector authority labels** are distinct and consistent: a connector
session refuses `/connect`, `/models`, and `/effort` with a notice naming the
agent, the sidebar shows `Managed by agent` in place of model and effort, the
agent's own MCP state is reported as not reported rather than merged with
Gritt's, cost and context stay `unavailable`, and the `/connect` confirmation
states that Gritt supervises the agent and relays approvals but does not own
its model, effort, or permissions.

**Process cleanup** now has three exits covered: SIGINT during `gritt mcp
trust` startup, a descendant that outlives its parent and is reached through
the process group, and, new here, Ctrl-Q in the full-screen mode with a server
that never answers `initialize`. All three assert the child is gone by pid.

**Resume of pre-chain sessions** is covered by
`an_old_database_upgrades_in_place_and_keeps_its_rows`, which seeds a database
that knows only migration `0001` and asserts it reaches `4/4 applied` with its
rows intact. `SessionKind::Native.effort` carries `#[serde(default)]`, so a
row stored before the chain loads with effort `auto` and no MCP fields.

**Print and REPL with MCP configured**: `gritt mcp list` in this workspace
correctly reports the single entry as `awaiting approval` with its reason,
`gritt doctor` reports `product migrations: 4/4 applied` and every profile's
key state without printing a value, and the workspace suite's print and REPL
end-to-end cases pass unchanged.

**`--fixture` never launches a server.** The flag short-circuits before any
store, catalog, control plane, or MCP runtime is constructed; the MCP entries
on screen are five invented snapshots. `tui_pty` asserts a fixture run never
opens a session.

### Assessed and recorded, not fixed

| Item | Assessment |
| --- | --- |
| Per-press cursor build over runs of flag emoji (TKT-0018) | Measured there at about 1.2 ms per press over 20,000 adjacent flags in a debug build, against a 50 ms budget. The typing series here, p95 2.267 ms on ordinary text, leaves the same headroom. Not worth giving `Composer` a persistent grapheme cursor and changing it from plain comparable data. Left as recorded. |
| Diff overlay has no word wrap | Confirmed: the `Paragraph` is built without `.wrap(...)`, so long lines clip at the panel edge and there is no horizontal scroll. Now documented in `docs/terminal-modes.md` as a stated limitation. |
| `/effort` empty until the catalog lands | Confirmed and correct: on Chat Completions, unreported reasoning support refuses every explicit level, and capabilities are unreported until the list arrives. Now explained in `docs/providers.md` rather than left to surprise the reader. |
| `McpRuntimeSettings` not exposed in `config.toml` | Still true, and it is what makes the 30 s deadline unworkable for a server that indexes on first start. Documented in `docs/tools-and-permissions.md` with the failure mode named. Follow-up 3. |
| Full `git status` per refresh | Confirmed unchanged. It runs off the event path through a bounded worker, so it costs sidebar freshness rather than input latency. No measurement here contradicts that. Follow-up 6. |
| `artifact-dir = "."` | Still set. Every `cargo build` rewrites the committed `gritt` binary at the repository root. It happened here and the result was byte-identical, so nothing needed restoring, but that is luck rather than design. Follow-up 5. |
| No OS clipboard, no mouse | Unchanged. Ctrl-Y fills Gritt's own buffer, which `docs/terminal-modes.md` already states. Both need a dependency and a platform review. |
| First-start 41 s MCP index against the 30 s deadline | Measured at 43.1 s here. Deadline deliberately left at 30 s; the reasoning is under Key Decisions. |

## Real-terminal walkthrough

**No human ran any of this in a real terminal.** Everything below was driven
by writing bytes into a `portable-pty` pseudo-terminal and asserting on what
came back. That harness cannot judge spacing, colour, contrast, font
rendering, or perceived latency, and it cannot produce a chord the terminal
itself has to encode.

The release binary was driven through home, `/connect`, `/models`, `/effort`,
`/mcp`, `/sidebar`, a submitted prompt, a resize to the other reference size,
and Ctrl-Q, at **120x40** and **80x24**. Both sizes: the home screen drew,
`/connect` listed the configured profile, `/models` and `/effort` and `/mcp`
each opened and drew their subject, `/sidebar` redrew (13,076 bytes at 120x40
as a column, 10,589 bytes at 80x24 as a drawer), the submitted prompt appeared
in the transcript, the resize redrew, and Ctrl-Q restored the terminal with
exit status 0. The configured key string appeared nowhere in either stream.

### Checklist for the human walkthrough the chain still requires

1. **Shift-Enter inserts a newline** in your terminal. The harness writes
   bytes and cannot produce the chord; only a terminal that reports the
   modifier distinctly makes it work, and Ctrl-J is the documented fallback.
2. **A bracketed multiline paste whose first line starts with `/`** is
   inserted as text and does not run a command.
3. **The `/` suggestion list and the Ctrl-P palette** reach the same commands,
   read well, and highlight legibly in dark, light, and `NO_COLOR`.
4. **Ctrl-C during a stream with a hung MCP server** shows the canceling state
   immediately, returns the composer, and leaves no server process behind.
5. **A real key typed through `/connect` setup into the operating system
   keychain**: the field masks as you type, the profile lands in the config
   file you chose with Ctrl-D, the key is retrievable afterwards, and nothing
   in the transcript or the scrollback contains it.
6. **Home screen spaciousness and the wordmark's block glyphs** in your own
   font, against the OpenCode reference.
7. **Perceived latency** while typing and scrolling a long transcript. The
   measured numbers are input-to-frame, not photons.

## Completion Gate

- **Acceptance:** Partial, and stated precisely. Docs match the implemented
  commands and limitations, verified against source and corrected after
  review; benchmark evidence now measures the production load path and
  records p50/p95 latency, CPU, memory, queue behaviour, and drain rate from
  `Scheduler::step` rather than from a copy of it; the single `.mcp.json`
  entry has an honest result; full validation is green. Two of the plan's
  budgets are not met: the resident-memory plateau, and input-to-frame under
  a sustained 1,000 deltas per second, which measures p95 between 49.5 and
  51.1 ms against a 50 ms budget across five runs. Both have the same cause, named in follow-up 1. The plan's own
  acceptance criterion permits a recorded run that "identifies specific
  remaining performance gaps". **One verification is outstanding rather than
  failed: the real-terminal walkthrough by a human has not been performed.**
  No agent can perform it. It stays pending with the checklist below, and the
  chain's verification contract is not satisfied until a human completes and
  records it. Next actions: follow-up 1, and the human walkthrough.
- **Scope:** Held. Seven fixes, all of them responsiveness,
  startup-correctness, or secret-accuracy defects the parent criteria
  require, four of them found across two review rounds. One of those, the
  approval-before-draw defect, was introduced by this ticket's own frame cap
  and caught by review rather than by me. No new product capability, no change to
  the provider or session contract beyond recording it, no LSP or skill
  execution, no test or criterion weakened. No new dependency.
- **Validation:** Eleven commands, all pass. Counts and environment above. The
  live MCP smoke needed the server's index to complete once before it could
  pass; that is recorded rather than hidden. Two regressions found by the
  reviewer were reproduced before being fixed, and their guard tests were
  verified to fail with the fix reverted. The same was done for the
  approval-before-draw guard found in round two.
- **Security and safety:** No new unsafe file or network access, no injection
  path, no auth bypass, no destructive behaviour, no dependency added. The
  audit found no reachable secret leak. One doc comment overstating a
  guarantee was corrected. Five low-severity residuals are recorded as
  follow-ups 7 through 10; none is currently exploitable end to end.
- **Regression risk:** The redraw gate is the real one. If some future
  interface element is drawn from a clock, it will appear frozen, because the
  tick no longer draws. The comment at the flag says so, and the guard test
  asserts zero idle bytes, which would also fail loudly if something started
  animating. The startup change means a model list or MCP server that was
  previously ready at the first frame may now arrive a moment later; the
  interface already handles both as arriving state, and eleven `tui_pty` cases
  plus the walkthrough exercise it.
- **Follow-up:** Ten items below.
- **Assumptions:** Six, listed above.

## Follow-up

1. **Limit rendering to visible content.** `render::transcript_lines` builds
   every entry's wrapped lines on each rebuild and the app slices out the ~35
   visible ones; every delta invalidates the cache. It is now the cause of
   three separate misses and margins: the failed memory plateau, the
   render-work figure sitting 0.9 ms inside a 16 ms budget, and the 2.5 ms
   overshoot on input-to-frame under sustained load, because a full rebuild
   is most of the cycle a keypress waits for. Fixing it needs a line index
   over entries so a rebuild can start at the first visible entry, and it will
   churn the 19 snapshot goldens. Highest-value remaining performance work by
   a wide margin.
2. **The `UiMsg` channel into the run loop is still an unbounded channel.**
   The loop now drains it every iteration, so under the plan's load it peaks
   at 52 and returns to zero, which is what the earlier unbounded growth
   needed. What is still missing against the plan's wording is a capacity:
   backpressure on the producer rather than a consumer that happens to keep
   up. Any producer faster than the drain would still grow it.
3. **Wire `McpRuntimeSettings` into `config.toml`** (carried from TKT-0017 and
   TKT-0019). Without it the 30 s initialization deadline cannot be raised for
   a server that legitimately needs longer on a cold start, and this
   repository's own server is one.
4. **`gritt-agent mcp serve` should not index before answering `initialize`.**
   43.1 s measured here for 292 files. Indexing lazily, or answering the
   handshake first, would remove the failure entirely rather than working
   around it with a longer deadline.
5. **`.cargo/config.toml` sets `artifact-dir = "."`** (carried from TKT-0016
   and TKT-0017). Every build rewrites the committed root binary. Worth its
   own ticket: drop the setting, or move the artifact directory out of the
   repository.
6. **The change scan runs a full `git status` per refresh.** Off the event
   path, so it costs sidebar freshness rather than latency, but a watch-based
   source would be better on a large repository.
7. **`McpTransport::Http` `Debug` prints the URL with its query string.** A
   `${VAR}` cannot resolve into a URL, so this needs a user to write a literal
   token into the URL, and `definition_summary` already strips the query for
   the interface path. No production `{:?}` of the type exists today. Latent.
8. **A secret routed through a field whose name does not look like a
   credential is not registered for redaction.** `"headers": {"X-Tenant":
   "${GITHUB_TOKEN}"}` is sent on the wire but its value is not in the
   redaction set, so a server that echoed it back would not have it scrubbed.
   Requires both a misnamed field and an echoing server.
9. **The credential-header list is defined in three places** (`gritt-core`
   `CREDENTIAL_HEADERS`, the harness's inline match, and `http.rs`'s
   `is_auth_header`), governing parse-time refusal, redaction registration,
   and wire-time wrapping. Identical today; adding a header to one and not the
   others would silently drop a protection.
10. **`Secret` does not zeroize on drop.** Escaping the setup form drops the
    composer with the typed key still in its heap buffer. Consistent with the
    rest of the codebase; noted for completeness.

Carried forward unchanged from earlier workers: Anthropic capability parsing
into `reasoning_efforts` (TKT-0016), `toml_edit` for comment-preserving config
writes (TKT-0016), newer MCP protocol revisions (TKT-0017), HTTP resumability
(TKT-0017), OS clipboard and mouse support (TKT-0018), the flag-emoji cursor
cost (TKT-0018), the diff overlay's missing word wrap (TKT-0019), and the
REPL's stale-approval read window (pre-existing).

## Updates

- [2026-09-05 review fixes](updates/2026-09-05-review-fixes.md)

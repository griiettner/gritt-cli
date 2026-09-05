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
states, and three defects the measurements exposed are fixed.

Eight of the plan's nine budgets are met. The ninth, a resident-memory plateau
over a five-minute soak, is not, and the gap is named rather than softened:
the renderer materializes every transcript entry on each rebuild and there is
no history paging, so memory tracks the transcript instead of levelling off.

Three fixes landed, all of them responsiveness defects the plan names as
acceptance requirements:

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
| `cargo test --workspace --no-fail-fast` | pass, 490 passed, 0 failed |
| `cargo test --manifest-path .agents/cli/Cargo.toml` | pass, 107 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | pass, 11 passed, 0 failed |
| `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke` | pass, 1 passed (see below) |
| `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live` | pass, 3 passed |
| `GRITT_LIVE_TESTS=1 cargo test -p gritt-provider --test live` | pass, 3 skipped honestly |
| `GRITT_BENCH=1 cargo test --release -p gritt-harness --test tui_responsiveness -- --nocapture --test-threads 1` | pass, 6 passed |
| `GRITT_BENCH=1 cargo test --release -p gritt --test tui_bench -- --nocapture --test-threads 1` | pass, 4 passed |

Workspace test count moved from 478 (TKT-0019) to 490: six in
`tui_responsiveness`, four in `tui_bench`, and one process-cleanup test in
`tui_pty`.

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
above. The deterministic harness is
`crates/gritt-harness/tests/tui_responsiveness.rs`, driven through
`App::on_key`, `App::on_event`, and `render::draw` against a `TestBackend` at
120x40. The process measurements are `crates/gritt/tests/tui_bench.rs`,
driven against the real binary in a pseudo-terminal.

### Against the plan's budget table

| Scenario | Budget | Measured | Verdict |
| --- | --- | --- | --- |
| Launch with existing config | first usable composer within 500 ms, independent of provider and MCP readiness | 40 ms (alternate screen at 39 ms) | **MET** |
| Typing, input to frame | p95 below 50 ms | p50 2.151 ms, **p95 2.267 ms**, max 2.490 ms, n=500 | **MET** |
| Picker navigation, input to frame | p95 below 50 ms | p50 2.466 ms, **p95 2.618 ms**, max 3.749 ms, n=500 | **MET** |
| Scrolling, input to frame | p95 below 50 ms | p50 2.158 ms, **p95 2.298 ms**, max 2.400 ms, n=500 | **MET** |
| Sustained output, render work at 120x40 | p95 below 16 ms | p50 14.468 ms, **p95 14.979 ms**, max 28.967 ms, n=690 | **MET**, by 1.0 ms |
| Cancel under load | visible canceling state within 100 ms | **20.529 ms** worst of 50 rounds | **MET** |
| Idle CPU over 30 s | below 1% of one core | **0.2%** | **MET** (2.5% before the fix) |
| Idle screen, no continuous full redraw | no redraw | **0 bytes** written to the terminal over 30 idle seconds | **MET** (16,281 bytes before the fix) |
| Resident memory plateau over a five-minute soak with history paging | a stable plateau | baseline 33,616 KiB, peak **852,016 KiB**; middle third 527,152 KiB, last third 852,016 KiB | **NOT MET** |

### The plateau gap, stated specifically

Over 300 seconds the harness delivered 2,796,000 text deltas into a
10,000-message transcript and consumed 294 s of CPU. Resident memory rose from
33,616 KiB to 852,016 KiB and was still rising: the last third of the run
peaked 324,864 KiB above the middle third. Growth is linear and steady at
**300 bytes per delta**, which is what the regression test bounds (at 2 KiB
per delta, an order of magnitude of headroom).

The cause is not a leak. `render::transcript_lines` builds wrapped `Line`
values for every entry in the transcript on each cache rebuild, the app then
slices the roughly 35 visible lines out of the result, and every incoming
delta invalidates the cache. History paging, which the plan asks for
("page old history rather than keeping every rendered line in memory"), does
not exist: the transcript view holds every entry it has ever been given. So
memory tracks transcript size, which under a continuous stream never stops
growing.

The same cause sets the sustained-output render work at 14.979 ms against a
16 ms budget. Both numbers move together, and limiting rendering to visible
content is the single change that would fix them. It is follow-up 1.

### Other recorded figures

| Measurement | Result |
| --- | --- |
| First frame after loading a 10,000-message transcript | 12.917 ms (a load cost, kept out of the input series) |
| Sustained delta rate achieved | 9,986 deltas in 10.0 s = **999/s** against the plan's 1,000/s |
| Queue depth under that load | peak backlog **29** deltas, 690 frames; the loop drains the whole backlog before each frame, so the queue is bounded by what one frame's work admits |
| 1 MiB tool result | reduce 3.204 ms, next frame 3.040 ms. Held as entry detail and drawn only when `/details` is on, so frame cost is not proportional to result size |
| Several MCP servers plus one hung server | 3 of 4 reached `Ready`; the hung entry became `Failed { "initialize did not answer within 5s" }` and did not hold the others |
| Frames drawn while MCP initialized | p50 1.106 ms, p95 4.385 ms, max 16.567 ms, n=367 |
| Soak CPU | 294 s over 300 s wall, a saturating producer loop rather than an idle measurement |

Queue bounds in the product, for the record: the MCP lifecycle broadcast is
bounded at 32 messages and every message carries the whole snapshot list, so a
lagged subscriber loses intermediate frames and never correctness. The
`UiMsg` channel from background work into the loop is **unbounded**; nothing
in the measured scenarios made it grow, because every producer is a discrete
completion rather than a stream, but it is not a bounded queue with
backpressure as the plan's wording asks for. Recorded as follow-up 2.

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
3. **An overstated secret guarantee in a doc comment**
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

- **Acceptance:** Partial, and deliberately so. Docs match the implemented
  commands and limitations, verified against source; benchmark evidence
  records p50/p95 latency, CPU, memory, and queue behaviour; the single
  `.mcp.json` entry has an honest result; full validation is green. One of
  the plan's nine budgets, the resident-memory plateau, is not met. The plan's
  own acceptance criterion permits a recorded run that "identifies specific
  remaining performance gaps", and the gap is identified with its cause,
  its magnitude, and the change that would close it. Next action: follow-up 1.
- **Scope:** Held. Three fixes, all of them responsiveness or secret-accuracy
  defects the parent criteria require. No new product capability, no change to
  the provider or session contract beyond recording it, no LSP or skill
  execution, no test or criterion weakened. No new dependency.
- **Validation:** Ten commands, all pass. Counts and environment above. The
  live MCP smoke needed the server's index to complete once before it could
  pass; that is recorded rather than hidden.
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
   visible ones; every delta invalidates the cache. This is the cause of both
   the failed memory plateau (300 bytes per delta, 852 MB after five minutes)
   and the 14.979 ms render work against a 16 ms budget. Fixing it needs a
   line index over entries so a rebuild can start at the first visible entry,
   and it will churn the 19 snapshot goldens. Highest-value remaining
   performance work.
2. **The `UiMsg` channel into the run loop is unbounded.** The plan asks for
   bounded queues with backpressure. Nothing measured made it grow, because
   every producer is a discrete completion rather than a stream, but it is not
   what the plan specifies.
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

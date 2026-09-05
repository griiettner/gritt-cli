---
id: TKT-0019
namespace: griiettner
title: Review fixes for the TUI integration
artifact: update
status: done
owner: griiettner
created: 2026-09-05
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
---

# 2026-09-05 Review fixes

## Trigger

Semantic review of PR #11 returned `needs-fix` with two High and ten
Medium findings, all confirmed against the branch at `76aeb54`. The
reviewer's acceptance table marked "reject late asynchronous work" and
"connect installed agents" not met, and six other criteria partial.

## Changes per finding

**1 (High) — a pending resume could replace a session while its old driver
ran.** A session change is now *reserved*. `PendingOpen` in `run.rs` holds
the operation id, the prompt that triggered a lazy open, and the driver
being replaced; `idle_agent` is emptied when the request goes out, so a
prompt submitted mid-switch cannot start a turn on the session being left.
`UiMsg::Opened` carries the operation it answers and is applied only when
it is still the one the loop is waiting for; anything else closes its
driver. `ChannelUi` stamps every event, approval, and completion with the
session generation its turn started under, and the handler drops those
that no longer match — a stale approval is answered `Denied` rather than
shown, and a stale driver is dropped rather than restored as the idle one.
`App::session_transition` refuses submission and settings while a switch
is in flight. A refused or failed open restores the previous driver.

**2 (High) — cancelling the first lazy open stranded the interface.**
`Action::Cancel` now takes the reservation, clears `running` and
`session_transition`, returns the prompt to the composer through
`undo_submission`, restores the previous driver, and says the draft was
kept. A result queued before the cancellation can no longer match the
operation, so it cannot open a session afterwards.

**3 — MCP cancellation dropped initialization before cleanup.** The
cancellation token is now held by the loop (`Runtime::mcp_cancel`) and MCP
work runs detached. `cancel_work` and `spawn` both signal that token and
leave the future to finish, because it owns the child process and the
launch slot whose release happens in its own shutdown path.

**4 — installed agents could not be selected; connectors could not
resume.** `Notice` gained `confirm: Option<ConnectorId>`, so the agent
detail view has an explicit acceptance and Enter returns
`Action::SelectConnector`, which opens the session through
`ControlPlane::open`. `resume_by_id` now looks at the stored session kind
and routes a connector session through the same general operation; only a
native session goes through `open_draft`, whose validation rejects
connectors by design.

**5 — configuration and keychain reads on the event path.** Initial
profile enumeration moved out of `event_loop` into `load_profiles`, which
resolves credentials in `spawn_blocking` and delivers `UiMsg::Profiles`;
the connection dialog fills in when they arrive. The post-setup path does
the write, the reload, and the new summaries in one blocking worker and
returns a prepared `UiMsg::Reloaded`.

**6 — Git blocked Tokio workers.** `WorkspaceChanges` runs every Git
invocation and every filesystem read through `spawn_blocking`, behind a
semaphore of two, so a slow repository cannot occupy executor capacity or
let refreshes pile up.

**7 — provider selection bypassed pinning.** One
`refuses_pinned_change(profile, model)` check covers both halves and runs
before anything is mutated, so a pinned session's draft, catalog, sidebar
provider, and effort stay on what the driver is really using. A model of
the same name under another provider is now recognised as a change.

**8 — unknown usage became zero.** A count the provider did not report is
left unknown. A turn missing either half sets `UsageSection::incomplete`,
which withholds the cost estimate and says the totals are a floor. The
last request's prompt tokens moved to `last_request_input` with their own
label; `context_tokens` has no source yet, so occupancy stays unavailable.

**9 — connector sessions misattributed Gritt's MCP inventory.**
`apply_mcp` labels the list Gritt's whatever session is open and sets
`IntegrationsSection::connector_mcp`, which renders
`<agent>'s own MCP: not reported`.

**10 — MCP trust bypassed the approval overlay.** Approving from `/mcp`
now returns `McpRequest::RequestApproval`; the runtime fetches
`McpRuntime::definition_summary` and the App shows it in the shared modal
approval, with `mcp_approval` routing the answer to a trust decision
instead of a tool approval. The summary carries the command and its
arguments, or an endpoint with its query removed, plus environment and
header **names** only, redacted against the entry's own credentials. Every
mutating MCP action is refused while a turn or an approval is active.

**11 — successful setup left the model picker stale.** `setup_outcome`
returns an `Action`: on success it invalidates the catalog cached before
the credential existed, bumps the selection token, rebuilds the picker
underneath, and requests a fresh catalog, keeping the query, the highlight,
and the composer draft.

**12 — Git-quoted filenames.** `git status --porcelain=v1 -z` is parsed
from NUL-separated records, with a rename's origin record consumed rather
than reported. A Unicode path under `core.quotePath` and a filename
containing ` -> ` both survive.

**Optional, applied.** `SetupForm` has a hand-written `Debug` that prints
the key's length and never its buffer.

**Optional, recorded not applied.** The reviewer's note for TKT-0020: a
reducer frame count does not cover the pre-terminal catalog and MCP waits
inherited at startup, continuous draw attempts, UI queue bounds, or full
history loading. That belongs in `report.md`'s follow-up 2 and is added
there.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, 0 errors |
| `cargo test --workspace --no-fail-fast` | pass, 466 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | pass, 10 passed |
| `cargo test -p gritt-harness` | pass, 301 passed |
| `GRITT_LIVE_MCP_TESTS=1 ... --test mcp_live_smoke` | pass, 1 passed |

New coverage: five runtime-handler tests in `src/tui/run.rs` driving
`on_action` and `on_message` against a stub driver, which is where
findings 1, 2, and 3 live and where no reducer test could reach; a
partial-usage test; a `Debug` redaction test; two `-z` parsing tests
including a filename containing the rename separator; and the MCP
approval overlay asserted in both the reducer tests and
`tests/tui_integration.rs`, where the definition summary is checked to
contain the executable and the environment *name* while not containing its
value. The narrow PTY walkthrough now scrolls the drawer, which is how a
24-row terminal reaches the sections below the fold, and asserts that an
unreported token count is drawn `unavailable` rather than `0`.

## Remaining follow-up

The report's follow-up list stands. Two claims in it were corrected: the
sidebar no longer shows context occupancy at all on the live path, and the
walkthrough evidence section already stated that no human ran it in a real
terminal, which the reviewer confirmed is still outstanding for the
chain's verification requirement.

---

# Round 2

## Trigger

Re-review of PR #11 at `2c24cac` returned `needs-fix` with one High and
five Medium findings, all confirmed. Round-1 fixes 2, 4, 8, 9, 12 and the
refresh half of 11 were confirmed met; the rest were partial.

## Changes per finding

**1 (High) — superseding a resume stranded its reservation.** A session
change shared `Runtime::work` with ordinary requests, so `/sessions`
during a pending resume called `spawn`, aborted the open, and left
`pending_open` and `session_transition` behind. The session-list response
then cleared the loading line, and the reducer's Escape only looked at
`running` and `loading`, so nothing could unwind it: prompts, settings,
and further resumes stayed refused for the rest of the run. The session
change now has its own slot, `Runtime::open_work`, which no other request
touches, and `take_pending_open` aborts that task with the reservation so
the two cannot separate. Escape checks `session_transition` as well.
`a_session_list_during_a_resume_leaves_the_transition_recoverable` drives
the real `Action::RefreshSessions` plus `UiMsg::Sessions` sequence and
asserts both that the open survives it and that Escape recovers; the
round-1 supersession test no longer clears the reservation by hand, it
cancels the way a user does.

**2 — keychain reads still blocked the first draw.** `event_loop` still
called `profile_summaries()` synchronously. That line was meant to go in
round 1 and did not: the script that removed it aborted on a later
assertion before writing the file, and the follow-up script only reapplied
the other half. It is gone; `load_profiles` is the only path, and the
comment now says why nothing may enumerate profiles before the first draw.

**3 — the concurrency permit outlived nothing.** `blocking` held the
permit in the future that awaits the worker, so a cancelled caller
released it while its `spawn_blocking` was still running. The permit is
now owned and moved into the closure.
`cancelling_a_scan_does_not_release_its_worker_s_permit` cancels six scans
against a `git` that blocks for 120 ms and asserts the peak concurrent
invocation count stays within the bound; against the previous code it
records six. `record_write`'s `exists()` stat moved into the same blocking
path, because the event handler awaits it after every turn.

**4 — MCP operations lost their cancellation ownership.** `Runtime::mcp`
now holds an id and a token. `begin_mcp` signals the operation it replaces
rather than dropping the token, and `UiMsg::McpOutcome` carries the id, so
a completion from a superseded action clears neither the live token nor
the loading line. `overlapping_mcp_actions_keep_their_own_tokens_and_completions`
covers both halves.

**5 — setup bypassed session pinning.** `setup_outcome` adopted the saved
profile into the draft and the sidebar without the pinning check, so
`/connect` → add provider → save from a pinned session displayed a
provider the driver was not using. The shared `refuses_pinned_change` now
runs before the selection is adopted; the write itself is unaffected and
the explanation says a new session is what uses it. Two tests: the pinned
case saves without moving the selection or the composer draft, and the
unpinned case still adopts the profile and returns the catalog reload,
which also closes the reviewer's optional note that the returned action
was unasserted.

**6 — a delayed MCP approval could authorize mutation during a turn.**
The definition response was accepted whenever no approval was pending,
even if a turn had started since it was requested, and the decision
handler mutated without rechecking. The response is now refused when
settings are not editable, with a notice naming the server, and the
`Decide` handler enforces the same guard before touching the runtime.
`a_definition_arriving_after_a_turn_started_is_refused` covers both.

**Optional, applied.** Focused tests for explicit connector confirmation
(including that Escape starts nothing), for the Gritt-owned MCP label
beside the connector's unreported state, and for the catalog action
returned after a successful setup.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, 0 errors |
| `cargo test --workspace --no-fail-fast` | pass, 474 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | pass, 10 passed |
| `cargo test -p gritt-harness` | pass, 309 passed |
| `GRITT_LIVE_MCP_TESTS=1 ... --test mcp_live_smoke` | pass, 1 passed |

Honest note on one flake: `the_first_prompt_creates_the_session_and_new_keeps_it`
failed once when run immediately after the full workspace suite, and its
message was not captured. It passed on five subsequent runs, including the
same back-to-back sequence and a run under synthetic CPU load, so it reads
as contention rather than a defect. It is recorded here rather than left
out; if it recurs, the PTY waits are the place to look.

## Remaining follow-up

Unchanged from round 1, minus the items closed above. The manual terminal
walkthrough is still outstanding and is still the chain's, not this
ticket's, to close.

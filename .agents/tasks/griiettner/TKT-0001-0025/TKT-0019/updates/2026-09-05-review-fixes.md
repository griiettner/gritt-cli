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

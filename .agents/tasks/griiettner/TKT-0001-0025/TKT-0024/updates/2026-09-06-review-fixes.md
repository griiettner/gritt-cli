---
id: TKT-0024
namespace: griiettner
title: Review fixes for connector model discovery
artifact: update
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
---

# TKT-0024 Update: review round 1 fixes

## Trigger

A ticket and impact review of `tkt-0024-connector-models` returned
`NEEDS-FIX` with six findings. Findings 1 through 5 required behavior
changes plus tests that failed before the fix. Finding 6 required an
honest report of commands that this session actually ran.

## Finding 1: native selection regression

Confirmed. Choosing a connector model wrote the identifier into
`SessionDraft.model`. `/new` cleared connector identity and kept that
value, so `open_draft` opened a native session with a connector id.

The connector choice now lives on `App.connector_model`. The native draft
model is left alone. `/new` clears `connector_model`. `SelectConnector`
reads the connector field, not the draft.

Regression:
`a_new_draft_after_a_connector_choice_keeps_the_native_model` in
`crates/gritt-harness/src/tui/app/tests.rs`. Observed red because
`connector_model` was unset and `draft.model` became `gpt-5.4`.

## Finding 2: stale fallback reported as current

Confirmed. `discover_models_inner` treated a still-fresh `fetched_at` as
`Current` before looking at a later failed `last_attempt_at`. A failed
refresh of a fresh cache returned `CachedStale`, then the next ordinary
lookup returned `Current`.

Ordinary lookup now refuses `Current` when `last_attempt_at` is later
than `fetched_at`. A recent failed attempt still returns `CachedStale`.
A later retry after the interval still probes.

Regression:
`a_failed_refresh_stays_stale_on_the_next_ordinary_lookup` in
`crates/gritt-connector/tests/models.rs`. Observed red with
`Current { freshness: Current }`.

## Finding 3: empty output treated as a current catalog

Confirmed for Cursor and OpenCode. Empty or whitespace-only stdout
returned `Ok([])`, and a refresh wrote that empty list as `Current`,
replacing a good cache. Codex already rejected empty input because it
looks for `{`. Header-only non-empty text was already `Malformed`.

Both parsers now return `Malformed` when no model id is produced,
including empty and whitespace-only output. Discovery then keeps the last
good cache as stale.

Regressions:
`parsers_reject_empty_or_whitespace_output` and
`empty_listing_output_does_not_replace_a_good_cache` in
`crates/gritt-connector/tests/models.rs`. Observed red with `Ok([])` and
`Current { models: [] }`.

## Finding 4: print and REPL did not share the TUI discovery service

Confirmed. Print and REPL opened a session and printed a one-line
summary. Discovery inside `open` always passed `refresh = false`.
Neither path listed catalog entries or offered an explicit refresh.

`ControlPlane::open_with` now calls `connector_models(id, refresh_models)`
for a new connector session. Print and REPL pass `--refresh-models`.
Startup notes print `connector_model_lines`, which is the status line
plus each model id. REPL `/models` and `/models refresh` call the same
service. Resumed sessions still skip discovery.

Regressions:
`print_and_repl_list_catalog_entries_from_the_shared_service` and
`repl_models_lists_the_connector_catalog` in
`crates/gritt-harness/tests/connector_session.rs`, plus
`print_and_repl_accept_an_explicit_connector_catalog_refresh` in
`crates/gritt/src/main.rs`.

## Finding 5: TUI picker stayed on loading

Confirmed. `apply_connector_catalog` rebuilt picker rows and left the
open overlay's status and hint at loading. Existing tests constructed a
fresh picker from state instead of reading the overlay.

`refresh_open_picker` now copies title, hint, status, and rows through
`Picker::replace_contents`. Ctrl-R goes through `request_connector_catalog`
so the overlay can show loading during a refresh.

Regression:
`a_connector_catalog_result_updates_the_visible_picker` in
`crates/gritt-harness/src/tui/app/tests.rs`. Observed red with
`Loading { what: "codex models" }` after a current catalog arrived.

## Finding 6: report claims that could not be reproduced

Confirmed. The first report's live command passed three positional
filters to `cargo test`, which rejects that form with
`error: unexpected argument`. Independently reproduced before this round.

The empty-output claim in that report also contradicted the Cursor and
OpenCode parsers, which returned `Ok([])` for empty and whitespace-only
stdout. Finding 3 is the behavior fix. The original report's Validation
and Edge Cases sections are corrected in `report.md`.

This round ran each live listing test as its own `cargo test` filter.
`live_claude_model_listing` is not a test name. The matching test is
`live_claude_model_listing_is_unsupported`.

## Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass, no warnings |
| `cargo test --workspace --locked` | pass, 565 tests, 0 failed |
| `cargo build --release --locked` | pass |
| `./.agents/gritt-agent ticket validate` | pass, 0 warnings |
| `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live --locked live_codex_model_listing` | pass. Codex catalog from `codex debug models` |
| `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live --locked live_claude_model_listing` | pass. Matches `live_claude_model_listing_is_unsupported`. Claude is `Unsupported` |
| `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live --locked live_opencode_model_listing` | pass. OpenCode catalog from `opencode models` |

The first report's three-name live command was not run. `cargo test`
rejects it.

## Remaining follow-up

Unchanged from the original report. Claude still has no listing command.
Cursor still has no live binary on this machine. A resumed connector
session still ignores `--model`.

## Review round 2

Verdict: `pass`, on commit `9bc3d8dd26d3205ad6bdd1cfd16a2b66dcea1cf2`.

Each finding was checked against the diff and its new test:

1. `App.connector_model` holds the connector choice; `/new` clears it and
   leaves `draft.model` alone; `SelectConnector` reads the connector field
   (`tui/run.rs:1566`).
2. `failed_since_fetch` guards the `Current` path. A successful refresh
   stamps `fetched_at` and `last_attempt_at` from the same instant
   (`supervise.rs:417`), so a success is never misread as a failure.
3. Cursor and OpenCode parsers return `Malformed` for empty output.
4. `open_with` forwards `refresh_models` to the shared
   `ControlPlane::connector_models`; `--refresh-models` and REPL
   `/models [refresh]` use it; `connector_model_lines` is the one
   formatter.
5. `Picker::replace_contents` carries status and hint onto the open
   overlay; the test asserts loading, ready, and stale transitions.
6. The report names the commands that ran and the one that did not.

### Routing deviation

The documented reviewer route (`codex exec --model gpt-6-astra`) produced
the round 1 verdict, but its first attempt hung for 45 minutes on its
own `~/.codex/tmp/arg0` lock without contacting the model, and the round 2
run was killed by a harness restart before returning. The orchestrator
(Claude Code, this session) performed the round 2 review directly against
`review/ticket` and `review/impact`, using the round 1 findings as the
checklist.

### CI

`main` is red on all three platforms at `d007329` and the three commits
before it. On this branch macOS passes; Ubuntu fails on
`a_descendant_that_outlives_its_parent_is_cleaned_up_too`
(`tests/mcp_runtime.rs`) and Windows fails to compile a unix-only
`ExitStatusExt` import in `crates/gritt-harness/src/changes.rs:501`, added
by TKT-0019. Neither file is touched by this ticket. The merge gate is
the explicit review above, per the chain rule for unreliable CI. Follow-up:
a hygiene ticket to cfg-gate `changes.rs` for Windows and to look at the
Ubuntu cleanup test.

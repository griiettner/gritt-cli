---
id: TKT-0027
namespace: griiettner
title: Allow provider and model changes within an active conversation
artifact: report
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
skills:
  - codebase-design
  - tdd
  - dev-harness
  - dev-provider
  - write
---

# TKT-0027 Report: Allow provider and model changes within an active conversation

## Summary

Implemented live provider/model switching for native sessions with history,
through `/connect` then `/models` (or `/models` alone to stay on the current
provider). The session id and transcript stay the same; only the
provider-specific driver underneath is replaced, validated the same way a
new session's draft is, and seeded with whatever conversation text the
outgoing adapter still held locally so the new provider is not started
blind.

## Key Decisions

- Reused `AgentBuilder::resolve_startup`'s pinned single-candidate path for
  switch validation, so a switch gets the exact same typed errors and
  warnings a fresh session's draft or the existing picker already produce
  (`DraftError`/`DraftWarning`), rather than a parallel validation path.
- Added `ProviderAdapter::history()`: the provider-neutral `Message` turns
  an adapter still holds locally, oldest first, system excluded.
  `ChatCompletionsAdapter` and `MessagesAdapter` keep a local role-tagged
  array and report it; `ResponsesAdapter`'s continuation is the opaque
  server-side `previous_response_id`, so it reports an empty history (see
  Assumptions).
- `NativeAgent::switch_native` (reachable through `Driver::switch_native`,
  default-refused for connector sessions) validates, then commits: builds
  the new adapter, reads the outgoing adapter's `history()`, persists the
  new `(profile, model, effort)` on the *same* session row
  (`Store::set_native_provider`), and stores the replayed turns in
  `pending_replay` so the next request carries them ahead of the new
  prompt (`request()` peeks `pending_replay`; `run_turn` clears it once the
  request that carried it is the one actually sent — `request_preview`
  never touches it).
- The TUI reuses the existing `pending_open`/`open_work` reservation
  (exactly what `Resume` and `SelectConnector` already use) so a switch
  cannot race a turn or another transition, is cancellable with Escape,
  and a stale result is dropped without touching a newer reservation. A
  new `Action::SwitchNative` / `UiMsg::SwitchReady` pair splits the
  network-bound half (warming the new profile's catalog, no live driver
  needed) from the fast local commit (needs the live driver, runs inline
  once the message arrives).
- Effort is carried forward from the *live* driver's own effort
  (`app.status.effort`), never the picker's in-progress draft, with the
  same soft "falls back to the model default with a notice" behavior the
  existing picker uses for a level the new model cannot take — never a
  hard rejection of the whole switch over effort alone.
- `agent_for`'s continuation restore now compares `state.owner` against
  `gritt_provider::continuation_owner(adapter.protocol())` before calling
  `restore`, so a continuation row a switch left for a different protocol
  is skipped (the new adapter starts fresh) instead of failing the whole
  session open; a state that *is* for the right owner but genuinely
  unreadable still propagates as an error, exactly as before this ticket.

## Alternatives Considered

- Mutating provider/model fields on the existing driver in place: rejected
  in `plan.md` — it leaks provider lifecycle and continuation rules into a
  driver built for one adapter selection and makes a failed replacement
  hard to roll back safely.
- Routing a switch through `ControlPlane::open`'s "close the old session,
  open a new one" path: rejected — that produces a second session, not a
  switch of the one already open, and would lose the transcript's
  continuity and the session's identity.

## Assumptions

- The Responses protocol keeps its conversation on the provider's own
  servers behind `previous_response_id`; the harness does not make a live
  call to fetch it back for replay. A switch away from a Responses-backed
  session therefore carries only the system prompt and the new prompt to
  the replacement provider, not the prior turns' text. This is recorded in
  `docs/terminal-modes.md` and covered by
  `history_reports_locally_held_text_turns_and_is_empty_for_responses`.
  A different reading (fetching the response chain live) would add a
  network round trip and a new adapter capability outside this ticket's
  scope; flagged as a follow-up if the gap matters in practice.
- Replay carries plain user/assistant text only (`Message.content`), not
  individual tool calls or their results: reconstructing exact tool-call
  history across two different wire protocols would mean translating one
  provider's tool-call wire shape into another's, which none of the three
  adapters' local state supports today, and `Message` (the shape
  `PromptRequest` already uses) is the provider-neutral contract the plan
  names for this. A different reading (full tool-call replay) is a larger,
  separate change to the adapter contract.
- `keep_effort` for a switch is the session's live effort, tried against
  the new model and reset with a notice if unsupported, for both a
  provider change and a model-only change alike. `concept.md`'s success
  criteria state effort survives a switch "where valid" in general, not
  only for a model-only one, so I read the plan's "a model change ...
  revalidates effort" language as the common case, not an exclusion of the
  provider-change case.
- `control.rs::validate_resume`'s `DraftError::SessionPinned` rejection (a
  resume whose `--profile`/`--model` flags, or a Sessions-picker resume,
  name something the transcript was not produced under) is left unchanged.
  That is opening or resuming a session with an explicit mismatched
  request, a different action from a live `/connect`/`/models` switch on a
  session already open, and this ticket's scope and acceptance criteria
  are about the latter. `/connect` and `/models` are TUI-only concepts;
  print and REPL mode have no equivalent picker to change through, so
  neither needed changes.
- A resumed session immediately after a switch, before any turn completes
  on the new adapter, does not recover the switch's replayed history (it
  lives only in memory, never persisted); the session's actual stored
  transcript is untouched either way. Flagged as a narrow follow-up rather
  than fixed, since durably persisting it would mean writing a
  provider-neutral continuation row distinct from any adapter's own format.

## Edge Cases and Failures

- Rejection (unknown profile, missing credentials, unavailable model)
  leaves the driver, the stored session row, and the visible selection
  untouched: `a_rejected_switch_leaves_the_driver_and_the_stored_session_unchanged`.
- Cancelling mid-switch (Escape) restores the reserved driver exactly the
  way cancelling a `Resume` does: `cancelling_a_switch_restores_the_driver_it_reserved`.
- A stale `SwitchReady` from a switch that was cancelled and superseded by
  another one is dropped without touching the newer reservation:
  `a_stale_switch_result_is_dropped_without_touching_the_current_reservation`.
- Resuming a session right after a switch, before a turn overwrites the
  old continuation row, does not fail to open:
  `resuming_right_after_a_switch_does_not_fail_on_the_old_continuation`.
- Found during self-review (`code-review` skill, medium effort, run against
  the working diff) and fixed before closing:
  - Browsing `/connect` to a different profile before picking a model was
    clobbering `app.status.effort` — the live driver's confirmed effort —
    through the pre-existing `revalidate_effort()`, and `Action::SwitchNative`
    was reading that clobbered value as `keep_effort`. Fixed by leaving
    `status.effort` alone in `revalidate_effort()` when `session_pinned` is
    true, and reading `keep_effort` from `app.status.effort` instead of
    `app.draft.effort`.
  - `request_preview`, documented as inspecting the next request "without
    sending it," shared `request()`'s destructive `pending_replay.take()`
    with the real turn path, so calling it after a switch would have
    silently dropped the replay before the real turn ever ran. Fixed by
    having `request()` peek the replay (back to `&self`, matching its
    original signature) and having `run_turn` clear it once the request
    that carried it is the one actually sent.
  - `agent_for`'s continuation-restore tolerance originally swallowed
    *any* restore failure, not just the expected owner mismatch, which
    would have masked genuine continuation corruption as a benign
    post-switch case. Fixed by checking `state.owner` against the new
    adapter's own owner (`gritt_provider::continuation_owner`) before
    calling `restore` at all; a state that matches but is unreadable still
    propagates as an error.

## Validation

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean.
- `cargo test --workspace` — all green (0 failed across every crate),
  including:
  - 3 rewritten TUI unit tests in `tui/app/tests.rs` for the new
    behavior (asks the driver via `Action::SwitchNative` instead of
    refusing; reselecting the driver's own pair is a no-op; setup-from-a-
    pinned-session drafts the new profile the same way an unpinned draft
    does).
  - 4 new integration tests in `crates/gritt-harness/tests/native_switch.rs`
    (model switch keeps the session and replays history into the next
    request; provider switch reaches the new endpoint with the
    conversation carried over; a rejected switch leaves the driver and
    the stored row unchanged; resuming right after a switch does not fail
    on the stale continuation).
  - 3 new TUI reducer/runtime tests in `tui/run.rs` for the async switch
    (cancellation restores the reserved driver; a stale result is
    dropped without disturbing a newer reservation; a confirmed switch
    updates status and returns the driver).
  - 1 new provider contract test
    (`history_reports_locally_held_text_turns_and_is_empty_for_responses`)
    across all three protocols.
- Snapshot suite regenerated (`GRITT_UPDATE_SNAPSHOTS=1 cargo test -p
  gritt-harness --test tui_snapshots`) for the `effort` and `help` screens,
  diffs reviewed by hand: `effort_*` now shows Anthropic's per-level
  refusal reasons — matching that fixture's own long-standing comment,
  previously unreachable because the old pinned-session refusal silently
  blocked the profile change the fixture asks for — and also drops a
  stray leftover notice overlay the old refusal path left stacked behind
  the effort picker; the footer still correctly names the live driver's
  own profile, model, and effort throughout. `help_120x40` reflects the
  updated limitations line.
- Manual terminal pass: not run — no interactive terminal is available in
  this environment. Flagging per the harness verification rule for any
  TUI change; a human pass (resize, keyboard-only `/connect` → `/models`
  → Enter, plain print mode unaffected) is recommended before merge.

## Completion Gate

- Acceptance: all stated criteria met. `/connect` and `/models` change
  provider and model on a native session with history without creating a
  new session, validated the same way a fresh draft is, with the same
  typed rejections; the persisted session id, transcript, composer draft,
  and sidebar identity survive both a successful and a rejected switch;
  effort carries forward where valid and resets with the picker's own
  notice where it cannot; a switch cannot race a turn or another switch
  and a late result is ignored; connector sessions are unaffected (still
  refused via the pre-existing `is_native_setting` gate); help and notice
  text no longer says `/new` is required merely to change provider or
  model.
- Scope: stayed within `gritt-core`, `gritt-provider`, and `gritt-harness`.
  No changes to `crates/gritt`, no new provider protocol, no new transcript
  storage format, no change to connector-session model/permission

## Updates

- [2026-09-08 connector pricing](updates/2026-09-08-connector-pricing.md)
- [2026-09-08 pricing unit correction](updates/2026-09-08-pricing-unit-correction.md)
- [2026-09-08 model picker pricing](updates/2026-09-08-model-picker-pricing.md)
  behavior, and `/new` still starts a separate conversation unchanged.
- Validation: fmt, clippy, and the full workspace test suite all pass (see
  Validation); manual terminal pass not run (no terminal available here).
- Security and safety: no new file or network access path — the switch
  reuses the same credential-resolution and provider-endpoint code a fresh
  session already uses. No secret is newly logged, stored, or echoed;
  only profile/model/effort names move through the new code paths.
- Regression risk: `request()`/`request_preview()` internals changed (now
  peek rather than never touching `pending_replay`, which did not exist
  before); `select_profile`/`select_model`/`revalidate_effort` behavior
  changed for a pinned session (three existing unit tests rewritten to
  match, the rest of the 216 `gritt-harness` lib tests re-verified
  passing); `agent_for`'s continuation-restore is now conditional on an
  owner comparison rather than always attempting `restore` and always
  propagating its result.
- Follow-up:
  1. A switch away from a Responses-backed session carries no prior text
     to the new provider (documented and tested, not fixed).
  2. A resume immediately after a switch but before the next turn loses
     the in-memory replay; the persisted transcript itself is unaffected.
  3. Picking a new provider in `/connect` previews it in the sidebar
     (`sidebar.model.*`) before a model is chosen and the switch is
     confirmed. This matches the existing `Resume`/`SelectConnector`
     precedent in this codebase (neither reverts the sidebar preview on a
     rejected transition either), so it was left as-is, but is worth a
     dedicated look if it proves confusing in practice.
- Assumptions: listed above, each with what a different reading would have
  changed.

## Follow-up

See "Follow-up" under Completion Gate.

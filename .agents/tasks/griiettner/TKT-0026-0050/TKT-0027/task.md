---
id: TKT-0027
namespace: griiettner
title: Allow provider and model changes within an active conversation
artifact: task
status: ready
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0015
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
skills:
  - codebase-design
  - tdd
  - dev-harness
  - dev-provider
  - write
---

# TKT-0027 Task: Allow provider and model changes within an active conversation

## Goal

Allow a user to change provider and model for later turns without leaving the
current native conversation. Preserve the Gritt session identity and
provider-neutral transcript while safely replacing the provider-specific
driver between turns.

## Inputs

- ADR-007, provider and session contracts.
- ADR-008, remembered choices and model/catalog behavior.
- ADR-009, turn-boundary and elevated-mode rules.
- TKT-0015 plan/report, which records the current pinning limitation.
- TKT-0019 report and `crates/gritt-harness/src/tui/app/tests.rs`, which
  contain the existing pinning guard and its tests.

## Scope

- Extend the native session/control-plane seam so a validated provider/model
  selection can replace the active native driver while retaining the same
  session id and stored transcript.
- Replay or otherwise expose the normalized Gritt conversation to the new
  provider adapter through provider-neutral contracts. Do not reuse an old
  provider-specific continuation token after a provider change.
- Make `/connect` and `/models` work after history exists for native sessions,
  with safe async transition, cancellation, stale-result rejection, and
  rollback to the old driver on failure.
- Revalidate the selected model's effort capability and update status/catalog
  metadata after a successful switch.
- Preserve the composer draft, transcript, session/sidebar identity, and
  current driver on failed or cancelled switches.
- Update focused help/notice copy and add behavior tests at core/control-plane,
  provider, and TUI boundaries as needed.

## Out of Scope

- Switching connector sessions, or changing the model/permission behavior of
  installed agent CLIs.
- Automatic failover, switching in the middle of an active turn, or silently
  changing a resumed session without an explicit picker selection.
- Provider-specific logic in the TUI or harness, new provider protocols, or a
  new transcript storage format.
- Changing `/new`: it still starts a separate conversation.

## Acceptance Criteria

- With a native session containing at least one completed turn, `/connect`
  lists configured providers and selecting another provider replaces the
  driver for the same session. The next prompt reaches the selected provider.
- With a native session containing at least one completed turn, `/models`
  selects another model for the active provider. The next prompt reaches that
  model without creating a new session.
- The persisted session id, prior transcript, composer draft, sidebar
  identity, and phase remain intact after either switch.
- Provider/model validation occurs before replacement. Missing credentials,
  unavailable models, unsupported effort, cancellation, and startup errors
  leave the old driver and visible selection unchanged with a user-facing
  explanation.
- A switch cannot race a turn or another switch. Late catalog, turn, approval,
  or completion messages from the old generation are ignored.
- Connector sessions continue to refuse native `/connect`, `/models`, and
  `/effort` changes with their existing authority explanation.
- Help and notices no longer tell users that `/new` is required merely to
  change provider or model.

## Verification

- Add tests that observe the replacement through public session/control-plane
  interfaces, including same-session transcript preservation and rollback.
- Add TUI reducer/runtime tests for provider and model changes after history,
  draft preservation, cancellation, and stale-result rejection.
- Run `cargo fmt --all -- --check` and focused `cargo test` for
  `gritt-core`, `gritt-provider`, and `gritt-harness`.
- Run the relevant workspace tests and the manual terminal flow: complete a
  turn, change provider, complete a turn, change model, complete a turn, then
  resume the session and confirm the transcript remains intact.

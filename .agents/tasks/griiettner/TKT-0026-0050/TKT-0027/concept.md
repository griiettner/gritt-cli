---
id: TKT-0027
namespace: griiettner
title: Allow provider and model changes within an active conversation
artifact: concept
status: ready
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
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

# TKT-0027 Concept: Allow provider and model changes within an active conversation

## Problem

The full-screen TUI treats a native session as permanently pinned to the
provider profile and model that opened it. After the first response,
`/connect` and `/models` refuse to change the selection and explain that
`/new` is required. This makes the transcript, rather than the conversation,
the unit of provider choice. It prevents a user from moving an ongoing
conversation from one provider or model to another for comparison, cost,
capability, or fallback reasons.

This behavior is explicit in `SessionKind::Native`, `ControlPlane::open_draft`,
the TUI's `session_pinned` guard, and the help limitation rendered by the TUI.
TKT-0015 recorded it as a deliberate product gap and TKT-0019 shipped the
guard. It is now a follow-up product requirement, not a regression to hide.

## Intent

Let a user change the provider profile and model at a turn boundary while
remaining in the same Gritt conversation. The transcript and session identity
must survive the change, while the next turn must use the newly selected
provider and model. The existing picker flow should make the change directly;
`/new` must remain available for starting a separate conversation.

## Success Criteria

- A native conversation with history can open `/connect` and select another
  configured provider, then submit a prompt that is sent through that provider.
- A native conversation with history can open `/models` and select another
  model, then submit a prompt that is sent through that model.
- The transcript, session id, composer draft, effort setting where valid, and
  session/sidebar identity remain coherent after a successful switch.
- A failed or cancelled switch leaves the current driver and selection active;
  it does not silently create a second session or lose the draft.
- Switching is unavailable while a turn or another session transition is
  active, and no old-driver event can land in the new configuration.

---
id: TKT-0027
namespace: griiettner
title: Allow provider and model changes within an active conversation
artifact: plan
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

# TKT-0027 Plan: Allow provider and model changes within an active conversation

## Boundary assessment

The conversation belongs to Gritt and is persisted as provider-neutral events.
The active native driver owns the provider-specific request machinery and any
provider continuation state. The TUI owns picker state and must ask the
control plane to replace the active driver at a safe turn boundary.

```text
TUI picker/reducer
        |
        v
control plane: validate selection -> open replacement native driver
        |                                  |
        v                                  v
Gritt session/event store          provider adapter + request state
```

Two placements were considered:

1. Mutate provider and model fields on the existing driver. This is smaller,
   but leaks provider lifecycle and continuation rules into a driver that was
   built for one adapter selection. It also makes failed replacement difficult
   to roll back safely.
2. Replace the active native driver at a turn boundary while keeping the same
   Gritt session and transcript. This preserves the provider adapter boundary,
   gives the control plane one validation point, and lets the TUI retain its
   generation and reservation protections. This is the selected design.

## Decisions

- The capability applies to native sessions only. Connector sessions continue
  to expose the connector's own model and permission controls.
- A switch is allowed only between turns. It is rejected while a turn,
  picker-load transition, or prior switch is pending.
- The Gritt session id and stored transcript remain unchanged. The replacement
  driver starts from the normalized conversation history available in Gritt,
  not from an old provider-specific continuation token.
- The old driver is kept until the new selection has validated credentials,
  model availability, and effort compatibility. On any failure, the old
  driver remains live.
- A provider change clears the selected model until the new provider catalog
  resolves. A model change keeps the provider and revalidates effort. An
  unsupported effort falls back to the provider/model default with the same
  typed explanation used by the existing picker.
- The composer draft and visible transcript survive both successful and
  failed switches. A successful switch updates status, catalog facts, cost and
  capability metadata for subsequent turns.
- No automatic provider failover occurs during a switch or mid-turn. An
  explicit user selection is required.

## Sequence

1. Add or adapt a provider-neutral replacement operation at the control-plane
   and driver seam, with tests for same-session history and failure rollback.
2. Add the TUI reducer/runtime action for `/connect` and `/models` on a native
   session with history. Reuse existing async generation and reservation
   guards so stale loads and turn events cannot overwrite the replacement.
3. Update help and notices to describe the new behavior, removing the claim
   that `/new` is required for provider/model changes.
4. Verify focused unit/integration behavior, then run the relevant workspace
   checks and manual TUI flow.

## Decisions To Lock Before Execution

- The selected replacement-driver seam above is the required ownership model.
- Provider adapters remain responsible for wire-format history and
  continuation details. The harness must not add provider-specific branching.
- Existing connector behavior, automatic startup failover, session resume
  semantics, and `/new` naming remain unchanged unless a test exposes a
  direct conflict with the active-session switch.

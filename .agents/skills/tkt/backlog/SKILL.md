---
name: tkt-backlog
description: Governs parked work in backlog.yaml. Use when deferring, activating, or rejecting an idea instead of ticketing it now.
---

# Backlog

Read [tkt](../SKILL.md) first.

`.agents/tasks/backlog.yaml` is the parking lot for deliberately deferred work. Each item has an `id` (`BKL-XXX`), `title`, `summary`, `parked_reason`, a `revisit_when` trigger, and `references`. Schema: `.agents/schemas/backlog.schema.json`.

## Rules

- The backlog is not a decision record. Accepted decisions live under `.agents/memory/decisions/`. Backlog items only point at them.
- Before proposing a new model, tool, or architecture direction, check the backlog. When the idea is already parked, follow its `revisit_when` trigger instead of re-litigating it.
- Park an item when work is explicitly deferred with a known reactivation condition, instead of creating a ticket that would sit idle.
- Activate an item only when its `revisit_when` trigger is met or the user explicitly asks. Activation means creating a `TKT-NNNN` ticket that carries over the item's context and references, then deleting the item from `backlog.yaml` in the same change. Once a ticket for the item is `in_progress`, the backlog entry must be gone. An item must never exist in both places.
- When an item is rejected permanently rather than deferred, remove it from the backlog and record the rejection under `.agents/memory/decisions/` if it is worth remembering.
- Reference the originating `BKL-XXX` id in the new ticket's `task.md` so history stays traceable.

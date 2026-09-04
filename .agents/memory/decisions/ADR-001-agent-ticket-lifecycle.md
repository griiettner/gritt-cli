---
id: ADR-001
title: Agent ticket lifecycle
status: accepted
date: 2026-05-25
---

# ADR-001: Agent Ticket Lifecycle

> Ticket id format and folder layout were later revised by [ADR-003](ADR-003-ticket-id-and-chunking.md). The lifecycle rules below still stand.

## Decision

Use the ticket folder under `.agents/tasks/` as the canonical store for ticket-specific context.

Ticket folders can be slim or full:

- Slim chore: `task.md`, plus `report.md` after completion when useful.
- Full ticket: `concept.md`, `plan.md`, `task.md`, `report.md`, and optional `updates/`.

Do not create placeholder artifacts just to satisfy structure.

## Rationale

Small chores should not pay a full process cost. Complex work still benefits from complete context from idea to report.

## Consequences

- Ticket folder contents are source of truth.
- `index.yaml` files are generated helper metadata only.
- Later ticket-specific comments require an explicit ticket id and should create update files.

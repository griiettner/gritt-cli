---
name: tkt-artifacts
description: Defines ticket modes, artifact roles, frontmatter, and lifecycle states. Use when creating or updating any ticket file.
---

# Ticket artifacts

Read [tkt](../SKILL.md) first. Path resolution: [store](../store/SKILL.md). Load [`write`](../../write/SKILL.md) when writing any ticket artifact.

## Modes

Use the smallest mode that preserves useful future context.

Slim chore, for small, low-risk, localized work:

```txt
.agents/tasks/<github-login>/TKT-0001-0025/TKT-0009/
  task.md
  report.md      # after completion, only when future context is useful
```

Full ticket, for complex, risky, multi-step, cross-file, or future-reference-heavy work:

```txt
.agents/tasks/<github-login>/TKT-0001-0025/TKT-0010/
  concept.md
  plan.md
  task.md
  report.md
  updates/
```

Do not create placeholder artifacts just to satisfy structure. A missing `plan.md` or `report.md` is valid when the ticket has not needed that artifact yet.

Every created `task.md` must be executable by the owner without reopening
material requirements. Missing context belongs in `concept.md` or `plan.md`
before execution, with an explicit assumption or decision owner. Chain tickets
must include the complete delivery contract before `tkt-exec-chain` starts.

## Artifact roles

- `concept.md`: initial idea, user problem, rough scope, unknowns, success criteria.
- `plan.md`: decision-complete plan that leaves no implementation choices open.
- `task.md`: executable contract for the owner agent, including inputs, constraints, checklist, and verification.
- `report.md`: concise completion summary optimized for future agent retrieval.
- `updates/YYYY-MM-DD-<slug>.md`: later ticket-specific comments, fixes, improvements, regressions, or follow-ups.

## Frontmatter

Every artifact starts with YAML frontmatter:

```yaml
---
id: TKT-0010
namespace: griiettner
title: Short title
artifact: task
status: ready
owner: griiettner
created: 2026-05-25
updated: 2026-05-25
---
```

Allowed `artifact` values: `concept`, `plan`, `task`, `report`, `update`.
Allowed `status` values: `concept`, `planning`, `ready`, `in_progress`, `done`, `blocked`, `cancelled`.

## Lifecycle

1. New: create `TKT-NNNN/` and at least `task.md` or `concept.md`.
2. Plan: create `plan.md` only when the work needs a decision-complete plan.
3. Execute: run from `task.md` and keep implementation scoped to the ticket.
4. Report: after meaningful execution, create or refresh `report.md`.
5. Update: for later explicit `TKT-NNNN` comments or fixes, create an update file and link it from `report.md`.

Closing rules and report shape: [completion](../completion/SKILL.md).

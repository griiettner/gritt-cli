---
id: TKT-0006
namespace: griiettner
title: Add engineering discipline and agent handoff skills
artifact: plan
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
areas:
  - .agents/skills
  - .agents/cli
skills:
  - skill-management
  - dev-cli
  - tkt-plan
---

# TKT-0006 Plan: Add engineering discipline and agent handoff skills

## Sequence

1. Add the focused engineering and handoff skills.
2. Add standards/spec review sub-skills and context-pointer guidance.
3. Synchronize adapters and indexes.
4. Audit and validate the complete skill tree.

## Decisions

- Existing Gritt skills remain canonical. New skills compose with them and do
  not replace ticket execution, memory writes, or provider-specific rules.
- `CONTEXT.md` is not introduced as a second durable-memory system. Domain
  language belongs in existing memory or ticket artifacts unless a later ADR
  establishes a dedicated context store.
- Review standards and spec are nested under `review/`, so they are reusable
  review modes without becoming unrelated top-level commands.

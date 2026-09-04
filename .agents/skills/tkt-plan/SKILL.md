---
name: tkt-plan
description: Writes or refreshes a ticket's plan.md. Use when a ticket has open implementation decisions before execution, or on /tkt-plan.
---

# /tkt-plan

Read [tkt](../tkt/SKILL.md) first. Ticket resolution: [tkt/store](../tkt/store/SKILL.md). Load [`write`](../write/SKILL.md) on `plan.md`.

Resolve `TKT-NNNN` (or `<namespace>/TKT-NNNN`), read the existing ticket artifacts plus the relevant repo context, then write `plan.md` in that ticket folder when the work needs a decision-complete plan.

Rules:

- Do not write a plan placeholder. Skip the file when `task.md` is already executable as written.
- Ask only for product or implementation preferences that cannot be discovered from the repo.
- When updating a real plan, preserve intentional manual decisions unless the user asks for a rewrite.

---
name: tkt-plan
description: Writes or refreshes a ticket's plan.md. Use when a ticket has open implementation decisions before execution, or on /tkt-plan.
---

# /tkt-plan

Read [tkt](../tkt/SKILL.md) first. Ticket resolution: [tkt/store](../tkt/store/SKILL.md). Load [`write`](../write/SKILL.md) on `plan.md`.

Resolve `TKT-NNNN` (or `<namespace>/TKT-NNNN`), read the existing ticket artifacts plus the relevant repo context, then write `plan.md` in that ticket folder when the work needs a decision-complete plan.

For chain-managed work, `plan.md` is a pre-execution contract. It must settle
the worker order, worktree and branch pattern, PR and merge policy, review
gates, validation, external dependencies, and final completion condition. Do
not leave those as questions for the worker or PM to discover later.

Rules:

- Do not write a plan placeholder. Skip the file when `task.md` is already executable as written.
- Ask only for product or implementation preferences that cannot be discovered from the repo.
- When updating a real plan, preserve intentional manual decisions unless the user asks for a rewrite.
- A chain plan with unresolved material context is incomplete. Record a
  best-judgment assumption or return to ticket creation; do not start execution
  and ask the same requirements question there.

## Output

Report the qualified ticket, decisions locked, files in scope, and the checks that make the plan executable without reopening settled questions.

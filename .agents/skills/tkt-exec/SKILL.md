---
name: tkt-exec
description: Executes a ticket from its task.md and preserves completion context. Use when a named TKT-NNNN is ready to implement, or on /tkt-exec.
disable-model-invocation: true
---

# /tkt-exec

Read [tkt](../tkt/SKILL.md) first. Ticket resolution: [tkt/store](../tkt/store/SKILL.md). Autonomy and goal tracking: [tkt/autonomy](../tkt/autonomy/SKILL.md). Closing rules: [tkt/completion](../tkt/completion/SKILL.md). Code: [dev](../dev/SKILL.md) and one of its sub-skills. Review: [review/ticket](../review/ticket/SKILL.md). Load [`write`](../write/SKILL.md) on `report.md` and update files.

This skill writes code and ticket artifacts, so it needs a specific ticket to work from.

## Lifecycle gate

Inspect the artifact frontmatter before executing:

- `ready` or `in_progress`: execute within the recorded scope.
- `done`: do not replay the implementation. Report the existing completion and any follow-up ticket. Continue only when the user explicitly asks to reopen or re-verify, and record that work in `updates/YYYY-MM-DD-<slug>.md`.
- `blocked`: try to clear the recorded blocker first. Ask only when it meets a stop condition in [tkt/autonomy](../tkt/autonomy/SKILL.md).
- `cancelled`: stop unless the user explicitly reactivates the ticket.

## Sequence

1. Resolve `TKT-NNNN` (or `<namespace>/TKT-NNNN`). Read `task.md`, `plan.md` when present, and the plan sections they cite.
2. State the goal per [tkt/autonomy](../tkt/autonomy/SKILL.md). Keep it active for the whole run.
3. Load [dev](../dev/SKILL.md), then the one sub-skill that matches the work.
4. Execute the whole `task.md` contract. Resolve ambiguity with best judgement, record each call as an assumption, and keep going. Do not ask what the ticket, the plan, or the repo already answers.
5. Run the verification steps from `task.md`. At minimum run the [dev/cli](../dev/cli/SKILL.md) verify set.
6. Load [review/ticket](../review/ticket/SKILL.md) and run it against your own diff before writing the report. When the harness also offers a separate review agent, run that too and treat its verdict like a reviewer's; note its task or agent id, and if it stalls, errors, or only returns partial findings before you close the ticket, say so in `report.md` and to the user instead of quietly finishing the review yourself.
7. Close per [tkt/completion](../tkt/completion/SKILL.md): answer the gate, write or refresh `report.md`, add an update file when a report already exists.
8. Mark the goal complete only after steps 5 to 7 ran. Leave it active when anything is partial or was not run.

Do not commit, push, or open PRs.

For sequenced multi-agent work use [`tkt-exec-chain`](../tkt-exec-chain/SKILL.md) instead.

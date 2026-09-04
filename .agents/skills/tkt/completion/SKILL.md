---
name: tkt-completion
description: Gates ticket closure and fixes report and update file shape. Use when finishing a ticket or recording post-completion work.
---

# Ticket completion

Read [tkt](../SKILL.md) first. Load [`write`](../../write/SKILL.md) when writing `report.md` or an update file.

Any command or agent executing a ticket preserves completion context:

1. Run the task verification steps.
2. Complete the gate below.
3. Capture pass or fail plus important failures and fixes.
4. Create or update `report.md` when the result is useful future context.
5. When `report.md` already describes an earlier completion, create `updates/YYYY-MM-DD-<slug>.md` instead of rewriting history.
6. Refresh helper indexes only if the project is currently using them. Never trust an index over the ticket files.

## Completion gate

Answer these in the report or update file with a verdict, evidence, and a next action when needed:

1. Acceptance: are all acceptance criteria satisfied?
2. Scope: did the work stay within the ticket or request boundaries?
3. Validation: what checks passed, failed, or were not run?
4. Security and safety: did the change introduce unsafe file or network access, injection risk, auth or policy bypass, dependency risk, data exposure, or destructive behavior?
5. Regression risk: what existing behavior could be affected, and what mitigates that risk?
6. Follow-up: what remains incomplete, deferred, or worth a later ticket?
7. Assumptions: which judgement calls were made without asking, and what would a different choice have changed? See [autonomy](../autonomy/SKILL.md).

If any answer is `no`, `partial`, or `not run`, record the concrete next action instead of calling the work fully done.

## Report format

Keep `report.md` concise and retrieval-friendly:

```md
# TKT-NNNN Report: Title

## Summary

## Key Decisions

## Alternatives Considered

## Assumptions

## Edge Cases and Failures

## Validation

## Completion Gate

- Acceptance:
- Scope:
- Validation:
- Security and safety:
- Regression risk:
- Follow-up:
- Assumptions:

## Follow-up

## Updates

- [YYYY-MM-DD short update](updates/YYYY-MM-DD-short-update.md)
```

Do not paste long logs. Summarize failure causes, fixes, and remaining risks.

Write the report so a new session can continue without redoing work. Preserve: problems that came up and how they were resolved; options raised, tried, or set aside, and why; anything decided, ruled out, or established as a constraint, stated exactly; where things stand now; what is still open or promised; and details that are hard to reconstruct such as names, numbers, paths, and exact wording. Be complete on those even at the cost of length. Condense everything else.

## Update files

Require an explicit `TKT-NNNN` before creating an update file. When a user comment does not name a ticket, ask which ticket it belongs to.

Each update file captures:

- trigger
- changed files or affected behavior
- failure or edge case observed
- fix or decision made
- validation performed
- remaining follow-up

After creating an update file, add or refresh the `## Updates` list in `report.md` in reverse chronological order.

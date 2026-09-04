---
name: review-ticket
description: Reviews an implementation against its task.md and plan.md. Use when closing a TKT-NNNN or reviewing it.
---

# Ticket review

Read [review](../SKILL.md) first. This is the review step [tkt-exec](../../tkt-exec/SKILL.md) runs before closing a ticket. Load [`write`](../../write/SKILL.md) when writing the report standalone; when this runs inside `tkt-exec`, that skill already loads it for `report.md`.

## Read the contract

Read `task.md`'s acceptance criteria, scope, and out-of-scope sections, and `plan.md`'s decisions when present. The ticket is the contract; do not review against a stricter or looser standard than what it states.

## Check the diff against it

- Acceptance criteria: met, partial, or not met, with the evidence for each.
- Scope: any file changed that scope excludes, or that out-of-scope forbids.
- Plan rules: any decision in `plan.md` the implementation did not honor.
- The usual review concerns: unrelated files, missing tests, secrets, and anything [impact](../impact/SKILL.md) would flag in the diff itself.

## Report

When this runs inside [tkt-exec](../../tkt-exec/SKILL.md), fold the findings straight into the completion gate answers in `report.md` rather than producing a separate deliverable. When invoked standalone, report findings ranked most severe first, the same as [impact](../impact/SKILL.md).

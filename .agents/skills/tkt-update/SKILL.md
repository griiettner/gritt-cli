---
name: tkt-update
description: Records later ticket work as an update file. Use when follow-up lands on a ticket that already has a report, or on /tkt-update.
---

# /tkt-update

Read [tkt](../tkt/SKILL.md) first. Update file contents and the report `## Updates` list: [tkt/completion](../tkt/completion/SKILL.md). Load [`write`](../write/SKILL.md) on the update file and any new `report.md`.

Only create an update when the user names `TKT-NNNN` or the command argument resolves one. Ask which ticket it belongs to when none is named.

## Output

Report the qualified ticket, update path, linked report change, and any later
work that remains open.

Resolve the ticket through [tkt/store](../tkt/store/SKILL.md), using `<namespace>/TKT-NNNN` when the bare id is ambiguous. Write `updates/YYYY-MM-DD-<slug>.md` in that ticket folder, then link it from `report.md` in reverse chronological order. Create a concise `report.md` first when the ticket has none.

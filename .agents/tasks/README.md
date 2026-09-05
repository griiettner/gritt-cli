# Agent Tasks

This directory is the canonical ticket lifecycle store for agents.

Load only the specific ticket folder needed for the current work. `.agents/tasks/index.yaml` is a generated chunk router, not the source of truth.

Tickets are `TKT-NNNN` and live in chunk folders of 25, so a ticket sits at `.agents/tasks/TKT-SSSS-EEEE/TKT-NNNN/`. Each chunk folder carries its own `index.yaml` shard.

To read ticket context: use the top-level `index.yaml` to choose the relevant chunk, read that chunk's `index.yaml` shard, then open only the ticket folders you need. Do not read every shard.

`.agents/tasks/backlog.yaml` is the parking lot for deliberately deferred work (`BKL-XXX` items). See the `tkt` skill for the rules that govern it.

Ticket folders may be slim or full. Slim chores can have only `task.md` plus a useful `report.md` after completion. Complex tickets can use the full lifecycle:

- `concept.md` - initial idea, motivation, rough scope, and unknowns.
- `plan.md` - decision-complete plan.
- `task.md` - executable agent contract.
- `report.md` - stable completion report optimized for future agent context.
- `updates/` - later ticket-specific fixes, improvements, comments, or regressions.

Do not create placeholder artifacts just to fill the structure.

Validate ticket structure with:

```bash
./gritt-agent ticket sync
./gritt-agent ticket validate
```

---
name: handoff
description: Captures compact continuation context for another agent or session. Use when pausing work, transferring ownership, or ending with incomplete execution.
disable-model-invocation: true
---

# Handoff

Prefer the canonical ticket for durable work. Use a handoff only for current
execution context that would otherwise require rediscovery.

## Workflow

1. State the active goal and ticket or qualified work item.
2. Record completed work, files changed, decisions, assumptions, and failed
   approaches.
3. Record exact validation results and the first command the next agent should
   run.
4. List unresolved questions, blockers, risks, and files that must not be
   touched.
5. Write the handoff under `.agents/handoffs/YYYY-MM-DD-<slug>.md` only when
   the repository has adopted that location; otherwise put it in the ticket.

## Completion criteria

- A new agent can resume without repeating discovery.
- The next action is concrete and safe.
- Secrets, speculative theories, and stale run logs are excluded.

## Output

Return the handoff path, current state, next command, blockers, and validation
status.

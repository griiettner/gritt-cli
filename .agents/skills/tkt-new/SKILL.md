---
name: tkt-new
description: Creates a new agent ticket under .agents/tasks/. Use when new work should be recorded as a TKT-NNNN, or on /tkt-new.
---

# /tkt-new

Read [tkt](../tkt/SKILL.md) first. Namespace and allocation: [tkt/store](../tkt/store/SKILL.md). Mode and frontmatter: [tkt/artifacts](../tkt/artifacts/SKILL.md). Load [`write`](../write/SKILL.md) on the ticket prose.

Before running the allocator, reason about the requested work:

1. Check whether an existing ticket already covers the work. Update that ticket
   instead of creating a duplicate.
2. Decide whether this is a slim ticket, a full ticket, or a chain. A single
   task gets one ticket folder. A full ticket may contain `concept.md`,
   `plan.md`, and `task.md`; those are artifacts, not separate tickets.
3. Use `tkt-new-chain` only when the work needs separate worker branches, PRs,
   and reviewer gates. Do not use it merely because the work has multiple
   files.

Then run the allocator. Never choose or skip an id manually. The allocator
rejects any namespace with a missing earlier id instead of silently advancing.
Ticket creation and index synchronization are treated as one operation. If
synchronization fails, creation is rolled back, so do not retry blindly:

```bash
node .agents/tools/agent-tools/tkt-new.mjs --title "Short title"
```

Add `--create-concept` and `--create-plan` when those artifacts are needed. Refresh identity alone with `node .agents/tools/agent-tools/tkt-identity.mjs`. After an ambiguous failure, check the ticket directory and indexes before running the allocator again.

Default to a slim ticket: `task.md` alone when the work is already clear, `concept.md` plus `task.md` when the idea needs framing. Use the full lifecycle only for complex, risky, multi-step, cross-file, or reference-heavy work.

Do not use this skill when an existing ticket already covers the work. For chain-managed work use [`tkt-new-chain`](../tkt-new-chain/SKILL.md).

## Output

Report the qualified id (`<github-login>/TKT-NNNN`), the path, and the chosen mode.

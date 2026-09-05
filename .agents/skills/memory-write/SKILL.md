---
name: memory-write
description: "Write or update .agents/memory/ files when new durable project knowledge is created: architectural decisions, API contracts, confirmed conventions, or patterns future sessions should know. Also use when the user asks to \"learn from this\", \"save what we discussed\", \"digest this conversation\", or similar — extract durable knowledge from the active conversation the same way."
---

# Memory Write

## Purpose

Write or update `.agents/memory/` files when new durable project knowledge is created: architectural decisions, API contracts, confirmed conventions, or patterns future sessions should know.

This skill has no external CLI dependency — every write is a direct file edit under `.agents/memory/`, following the frontmatter and indexing conventions in `.agents/memory/decisions/ADR-002-memory-routing.md`. It differs from `reflect` in trigger, not mechanism: `reflect` runs once at session end and covers everything worth keeping; `memory-write` runs any time a piece of durable knowledge is confirmed mid-session, including on explicit request ("learn from this", "save what we discussed", "digest this conversation").

When triggered from a conversation-digest request, this skill can only see the active conversation context (or a transcript the user explicitly provides) — never claim access to any other session, store, or history. Inventory the conversation for problems solved, decisions made (with rationale), rules that emerged, and gotchas discovered, then filter and route each item using the same decision tree below. Discard failed-attempt noise, obvious answers, and session-specific state.

## Before writing

Check for existing coverage first, to avoid duplicating or fragmenting knowledge:

1. Read `.agents/memory/MEMORY.md` to pick the right category.
2. Read that category's `index.yaml` (e.g. `.agents/memory/decisions/index.yaml`).
3. Reuse and update an existing memory file when the topic already exists.
4. Create a new file only when no existing memory topic fits.

## Decision tree — where does this knowledge go?

| Type | Action |
| --- | --- |
| Architectural decision or new API contract | Create or update a file under `.agents/memory/decisions/` (new `ADR-NNN-<slug>.md` for a decision with rationale) |
| System structure or component relationship | Update `.agents/memory/architecture/overview.md` |
| Non-negotiable rule or working principle | Update `.agents/memory/principles/constitution.md` |
| Reusable rule confirmed by evidence, no natural category fit | Add a new file under the closest-fitting category, or extend `constitution.md` |
| Uncertain pattern, not yet confirmed by evidence | Do not write to durable memory — note it for a later `reflect` pass instead |
| Transient session detail, one-off workaround | Nowhere — do not write |

This scaffold has no staging/candidate tier and no episode log. If a claim isn't confirmed enough to write directly, it isn't confirmed enough to store — leave it out rather than staging it.

## Memory file format

Every durable memory `.md` file needs YAML frontmatter — match the fields already used in that category (e.g. `id`, `title`, `status`, `date`, `related_ticket` for decisions). Keep files dense: bullets over prose.

Include `tags` and `read_when`: the generated index is built from these, and `read_when` is what tells a future agent whether this file is worth opening.

```markdown
---
id: <e.g. ADR-004>
title: <short title>
status: accepted
date: YYYY-MM-DD
tags:
  - <topic>
read_when:
  - <situation in which a future agent should read this>
---
# <Title>

## Decision
- fact

## Rationale
- fact
```

## After writing

Category `index.yaml` files are **generated** from the frontmatter above — the sync tool reads each memory file's `id`, `title`, `tags`, and `read_when` and rewrites the index. Never hand-edit an `index.yaml`; a hand-edit is overwritten on the next sync. Regenerate instead:

```bash
./gritt-agent ticket sync
```

## Output

- State which memory file was created or updated, and why it belongs there.
- Confirm the sync tool regenerated the category index.
- Note if nothing was written because the claim wasn't confirmed yet.

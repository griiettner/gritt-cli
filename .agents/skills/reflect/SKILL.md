---
name: reflect
description: "End-of-session retrospective. Capture lessons with evidence, update the relevant .agents/memory/ file, and write a dated reflection to .agents/memory/reflections/."
---

# Reflect

Load [`write`](../write/SKILL.md) when writing a memory file or the dated reflection.

## Purpose

End-of-session retrospective. Takes ~2 minutes. Surface what's worth keeping from the session, write it directly into `.agents/memory/`, and leave a dated reflection log behind.

This skill has no external CLI dependency — every write is a direct file edit under `.agents/memory/`, following the frontmatter and indexing conventions in `.agents/memory/decisions/ADR-002-memory-routing.md`.

## Step 1 — Gather context

1. `git --no-pager log --oneline -20` — what actually changed.
2. `git status --short` — uncommitted session work.
3. Any ticket files touched this session, under `.agents/tasks/TKT-SSSS-EEEE/TKT-NNNN/` (skim only).

## Step 2 — Identify what's worth preserving

| Signal | Action |
| --- | --- |
| Rule confirmed by evidence (bug fixed, test passed, decision validated) | Add or update a durable memory file (see Step 2a) |
| Architectural change or new API contract | Update `.agents/memory/architecture/overview.md` |
| Durable decision with rationale | Add a new `ADR-NNN-<slug>.md` under `.agents/memory/decisions/` |
| Non-negotiable rule or working principle | Update `.agents/memory/principles/constitution.md` |
| Pattern observed but not yet confirmed, or nothing generalizable | Skip — leave it to the reflection log only (Step 3) |

Aim for 1–3 items. Do not stage vague or session-specific claims.

### Step 2a — Write the memory file

Every durable memory `.md` file needs YAML frontmatter. Match the fields already used in that category (e.g. `id`, `title`, `status`, `date`, `related_ticket` for decisions). For a new ADR, pick the next `ADR-NNN` id in sequence.

Category `index.yaml` files are **generated** from that frontmatter — the `agent-tools:tkt-sync` Nx target reads each memory file's `id`, `title`, `tags`, and `read_when` and rewrites the index. Never hand-edit an `index.yaml`; a hand-edit is overwritten on the next sync. To get a good index entry, put `tags` and `read_when` in the memory file's frontmatter, then regenerate:

```bash
node .agents/tools/agent-tools/tkt-sync.mjs
```

## Step 3 — Write the reflection file

Create `.agents/memory/reflections/YYYY-MM-DD-<slug>.md` (slug: 2-4 words). Create the `reflections/` directory first if it doesn't exist yet — it isn't part of the scaffold by default.

```markdown
---
date: YYYY-MM-DD
tags: [reflection]
---
# <Session summary title>

## What happened

## What was learned (with evidence)

## Follow-ups
```

## Output

- State which memory files were added or updated.
- Confirm the reflection file was written and that the `agent-tools:tkt-sync` Nx target regenerated the indexes.
- Call out anything skipped as session-specific or unconfirmed.

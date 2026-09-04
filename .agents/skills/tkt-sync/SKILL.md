---
name: tkt-sync
description: Regenerates skill adapters and ticket and memory indexes, then validates tickets. Use after adding, removing, or renaming a skill, ticket, or memory file, or on /tkt-sync.
---

# /tkt-sync

Tooling launcher. It does not need the `tkt` hub.

Run all three, in order:

```bash
node .agents/tools/agent-tools/sync-skills.mjs
node .agents/tools/agent-tools/tkt-sync.mjs
node .agents/tools/agent-tools/tkt-validate.mjs .agents/tasks
```

- `sync-skills` rewrites `.claude/skills/` stubs and each skill's `agents/openai.yaml` policy block from `.agents/skills/*/SKILL.md`.
- `tkt-sync` rewrites `.agents/tasks/**/index.yaml` and `.agents/memory/*/index.yaml` from frontmatter.
- `tkt-validate` checks ticket folders, frontmatter, and chain links.

Add `--check` to the first two to report drift without writing.

Report what changed and any validation error. Never hand-edit a generated file to make the check pass. The tools are idempotent and safe to rerun.

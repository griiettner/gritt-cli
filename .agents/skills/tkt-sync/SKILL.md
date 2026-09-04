---
name: tkt-sync
description: Regenerates skill adapters and ticket and memory indexes, then validates tickets. Use after adding, removing, or renaming a skill, ticket, or memory file, or on /tkt-sync.
---

# /tkt-sync

Tooling launcher. It does not need the `tkt` hub.

Run all three, in order:

```bash
.agents/cli/target/release/gritt-agent skill sync
.agents/cli/target/release/gritt-agent ticket sync
.agents/cli/target/release/gritt-agent ticket validate
```

Build the binary first when it is missing: `cargo build --release --manifest-path .agents/cli/Cargo.toml`.

- `skill sync` rewrites `.claude/skills/` stubs and each skill's `agents/openai.yaml` policy block from `.agents/skills/*/SKILL.md`.
- `ticket sync` rewrites `.agents/tasks/**/index.yaml` and `.agents/memory/*/index.yaml` from frontmatter.
- `ticket validate` checks ticket folders, frontmatter, and chain links.

Add `--check` to the first two to report drift without writing.

Report what changed and any validation error. Never hand-edit a generated file to make the check pass. The tools are idempotent and safe to rerun.

---
id: TKT-0006
namespace: griiettner
title: Add engineering discipline and agent handoff skills
artifact: task
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
areas:
  - .agents/skills
  - .agents/cli
skills:
  - skill-management
  - dev-cli
  - tkt-plan
---

# TKT-0006 Task: Add engineering discipline and agent handoff skills

## Goal

Add the Matt Pocock-inspired engineering discipline skills and review modes
that are missing from Gritt.

## Inputs

- `.agents/skills/`
- `.agents/memory/architecture/overview.md`
- `.agents/skills/tkt-plan/SKILL.md`
- `.agents/skills/review/SKILL.md`
- `.agents/skills/skill-management/SKILL.md`

## Scope

- Add top-level `grill`, `domain-modeling`, `tdd`, `diagnose`,
  `codebase-design`, `handoff`, `wayfinder`, and `writing-for-agents` skills.
- Add nested `review/standards` and `review/spec` procedures and route them from
  the review family index.
- Give each skill a precise trigger, ordered workflow, completion criteria, and
  output contract. Keep references short and Gritt-specific.
- Update skill-management guidance with context-pointer and invocation-axis
  rules.

## Out of Scope

- Do not add a second context or glossary storage system.
- Do not replace existing ticket, memory, or review lifecycle behavior.
- Do not add external dependencies, network calls, or a live automation loop.

## Acceptance Criteria

- All eight new top-level skills exist with valid frontmatter and synchronized
  Claude/Codex adapters.
- Review has separate standards and spec sub-skills with routing metadata.
- `skill audit --strict`, `skill sync --check`, and `ticket validate` pass.
- Existing CLI tests and clippy remain green.

## Verification

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path .agents/cli/Cargo.toml`
- `.agents/cli/target/release/gritt-agent skill sync`
- `.agents/cli/target/release/gritt-agent skill audit --strict`
- `.agents/cli/target/release/gritt-agent skill sync --check`
- `.agents/cli/target/release/gritt-agent ticket validate`

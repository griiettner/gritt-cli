---
id: TKT-0005
namespace: griiettner
title: Strengthen skills with audits, control loops, feedback, and visual explanations
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
  - write-docs
---

# TKT-0005 Task: Strengthen skills with audits, control loops, feedback, and visual explanations

## Goal

Strengthen Gritt's skill system with deterministic audits, reusable control
loop and feedback patterns, and a visual explanation workflow.

## Inputs

- `.agents/skills/skill-management/SKILL.md`
- `.agents/skills/skill-management/audit/SKILL.md`
- `.agents/cli/src/skill/`
- `.agents/cli/src/main.rs`
- `AGENTS.md` and `.agents/memory/architecture/overview.md`

## Scope

- Add `skill audit [--skill NAME] [--strict]` to `gritt-agent`.
- Validate canonical skill metadata, slug alignment, local references, and
  explicit output or verification guidance. Report warnings separately.
- Add tests for clean, malformed, missing-reference, and strict-mode cases.
- Add `control-loop` with local-first sensor/controller/actuator guidance,
  feedback-memory and PR-bounding templates.
- Add `show-me` with compact diagrams, diffs, Mermaid, and focused HTML rules.
- Update skill-management and adapter/index metadata without changing the
  canonical routing model.

## Out of Scope

- A live GitHub Actions workflow for an unspecified maintenance target.
- Marketplace packaging or remote installation of this repository's skills.
- Rewriting existing skills solely to satisfy new non-fatal audit warnings.

## Acceptance Criteria

- `skill audit` exits zero for metadata-clean skills and reports all findings
  with paths; `--strict` exits nonzero when warnings exist.
- Audit never writes files and can target one skill or the whole canonical tree.
- Local relative Markdown references resolve, and frontmatter name matches the
  skill directory.
- `control-loop` and `show-me` are invocable top-level skills with references.
- `.claude/skills/` and `agents/openai.yaml` outputs are synchronized.
- CLI formatting, clippy, tests, build, ticket validation, and skill sync check
  pass.

## Verification

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all --check`
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path .agents/cli/Cargo.toml`
- `.agents/cli/target/release/gritt-agent skill audit`
- `.agents/cli/target/release/gritt-agent skill sync --check`
- `.agents/cli/target/release/gritt-agent ticket validate`

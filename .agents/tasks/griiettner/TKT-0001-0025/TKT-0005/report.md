---
id: TKT-0005
namespace: griiettner
title: Strengthen skills with audits, control loops, feedback, and visual explanations
artifact: report
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

# TKT-0005 Report: Strengthen skills with audits, control loops, feedback, and visual explanations

## Summary

Implemented the skill-system improvements identified from the HumanLayer
comparison. Gritt now has a read-only semantic skill audit, explicit output
contracts across existing top-level skills, a reusable control-loop skill with
feedback and workflow templates, a visual explanation skill, and an
AGENTS.md-improvement skill.

## Key Decisions

- Audit warnings are advisory by default and fail only with `--strict`.
- Scheduled automation remains a template. No live workflow was enabled without
  a concrete maintenance target, cadence, credentials, and approval boundary.
- Gritt keeps its indexed progressive-disclosure model instead of copying
  Claude-specific `<important if>` prompt markup.

## Alternatives Considered

- A marketplace manifest was not added. This repository's canonical skills are
  project-local and already have generated Claude and Codex adapters.
- Existing skills were not rewritten wholesale. Only missing output contracts
  and the requested management guidance were added.

## Assumptions

- “Apply all gaps” means improve the repository's skill system and add the
  necessary reusable skills, not install HumanLayer's external plugins.
- A generic workflow template is safer than enabling unattended CI for an
  unspecified target. A different choice would require adding repository
  secrets and a real sensor/controller/actuator implementation.

## Edge Cases and Failures

- The first ticket allocation attempt was blocked by the sandbox's read-only
  `.agents` boundary. The same governed allocator succeeded with explicit
  write approval.
- The first audit test exposed that URL and anchor filtering belonged in the
  reference parser. That parser now returns only local references.

## Validation

- `cargo fmt --manifest-path .agents/cli/Cargo.toml --all`
- `cargo test --manifest-path .agents/cli/Cargo.toml` passed all unit and
  integration tests.
- `cargo clippy --manifest-path .agents/cli/Cargo.toml --all-targets -- -D warnings` passed.
- Release build passed.
- `gritt-agent skill audit --strict` passed for 20 skills with zero warnings.
- `gritt-agent skill sync --check` passed with no drift.
- `gritt-agent ticket validate` passed with zero warnings.

## Completion Gate

- Acceptance: yes. All ticket criteria are satisfied.
- Scope: yes. Changes are limited to the CLI skill subsystem, canonical skills,
  generated adapters, indexes, and TKT-0005 artifacts.
- Validation: yes. The full CLI verification and maintenance gates passed.
- Security and safety: yes. The audit is read-only. The workflow is inert until
  its placeholder commands and policy are replaced. No secrets or network
  behavior were added to the CLI.
- Regression risk: low. The new command is additive, skills are opt-in except
  for the intentionally safe `show-me` skill, and generated adapters passed
  drift checks.
- Follow-up: a future ticket can instantiate one concrete control loop and
  validate it through a real reviewed PR.
- Assumptions: recorded above.

## Follow-up

Instantiate a concrete scheduled loop only after its sensor, controller,
actuator, credentials, cadence, and open-PR policy are defined.

## Updates

- None.

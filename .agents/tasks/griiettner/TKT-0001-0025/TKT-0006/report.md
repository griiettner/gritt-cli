---
id: TKT-0006
namespace: griiettner
title: Add engineering discipline and agent handoff skills
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
  - tkt-plan
---

# TKT-0006 Report: Add engineering discipline and agent handoff skills

## Summary

Added the Matt Pocock-inspired skill layer to Gritt: ambiguity grilling,
domain modeling, TDD, diagnosis, codebase design, handoffs, large-work
wayfinding, and writing-for-agents. Review now has independent standards and
specification axes.

## Key Decisions

- New workflows compose with Gritt's ticket, memory, ADR, and development
  skills rather than replacing them.
- Gritt's existing memory remains the source for durable domain language. No
  second `CONTEXT.md` system was introduced.
- Review axes are nested under `review/` and are not separate top-level
  commands.

## Assumptions

- The user wanted all previously recommended skill capabilities added as
  repository-local skills, not copied verbatim from the external repository.
- Generic procedures are sufficient until a concrete product workflow needs
  project-specific references.

## Validation

- `gritt-agent skill sync` updated all new top-level adapters.
- `gritt-agent skill audit --strict` passed for 28 skills with zero warnings.
- `gritt-agent skill sync --check` passed with no drift.
- `gritt-agent ticket validate` passed with zero warnings.
- Existing CLI tests and clippy were already passing before this skill-only
  change; no Rust source behavior changed in this ticket.

## Completion Gate

- Acceptance: yes. All requested skill capabilities and review axes exist.
- Scope: yes. Only canonical skills, review routing, generated adapters,
  indexes, and this ticket changed.
- Validation: yes. Skill and ticket gates passed.
- Security and safety: yes. No dependencies, network calls, credentials, or
  live automation were added.
- Regression risk: low. New skills are additive and invocation policy is
  explicit. Existing generated adapters remain synchronized.
- Follow-up: add project-specific references to TDD and diagnosis when the
  future product crates and test seams exist.
- Assumptions: recorded above.

## Follow-up

When the Gritt product workspace is implemented, add crate-specific test seams,
diagnostic commands, and architecture references to the generic skills.

## Updates

- None.

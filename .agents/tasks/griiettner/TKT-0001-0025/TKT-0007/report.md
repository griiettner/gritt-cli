---
id: TKT-0007
namespace: griiettner
title: Enforce end-to-end chain worktree PR and merge delivery
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
areas:
  - .agents/skills
  - .agents/memory
skills:
  - tkt-exec-chain
  - tkt-new-chain
  - tkt-autonomy
  - tkt-plan
  - memory-write
---

# TKT-0007 Report: Enforce end-to-end chain worktree PR and merge delivery

## Summary

Strengthened chain execution so a chain cannot be considered complete after
implementation or an open PR. Each worker must use a fresh worktree and branch,
commit, push, open a PR, pass review, merge, confirm the merged state, clean up
the worktree, and then permit the next worker.

## Validation

- `skill_audit --strict`: 28 skills, 0 warnings.
- `skill_sync --check`: no drift.
- `ticket validate`: 0 warnings.

## Completion Gate

- Acceptance: yes.
- Scope: yes. Only chain skills, ticket-writing guidance, durable memory, and
  this ticket were changed.
- Validation: yes.
- Security and safety: yes. No command execution or external mutation was
  added. Real GitHub or permission blockers remain the only valid pause cases.
- Regression risk: low. The rules are stricter only for chain-managed work.
- Follow-up: a future CLI enhancement could mechanically inspect PR and merge
  state, but the current skill contract already requires the checks.
- Assumptions: best judgment remains allowed inside settled ticket scope; it is
  not permission to defer requirements or skip delivery steps.

## Updates

- None.

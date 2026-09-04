---
id: TKT-0007
namespace: griiettner
title: Enforce end-to-end chain worktree PR and merge delivery
artifact: task
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

# TKT-0007 Task: Enforce end-to-end chain worktree PR and merge delivery

## Goal

Make chain execution an end-to-end delivery contract. Every worker must use a
new worktree and branch, commit, push, open a PR, pass review, merge, confirm
the merged state, and only then allow the next worker.

## Inputs

- `.agents/skills/tkt-exec-chain/SKILL.md`
- `.agents/skills/tkt-new-chain/SKILL.md`
- `.agents/skills/tkt-autonomy/SKILL.md`
- `.agents/skills/tkt-plan/SKILL.md`
- `.agents/memory/principles/constitution.md`

## Scope

- Strengthen chain delivery and worker sequence rules.
- Make missing material ticket context a ticket-writing failure.
- Record the policy in durable principles and architecture memory.

## Out of Scope

- Changing CLI behavior or automatically creating GitHub branches and PRs.
- Changing one-shot ticket execution.

## Acceptance Criteria

- Chain skills explicitly require worktree, branch, commit, PR, review, merge,
  merged-state confirmation, and cleanup.
- The next worker is blocked until the previous PR is merged.
- Execution may pause only for a genuine external blocker with recovery action.
- Chain tickets must contain complete material context before execution.
- Durable memory states the contract.

## Verification

- `gritt-agent skill audit --strict`
- `gritt-agent skill sync --check`
- `gritt-agent ticket validate`

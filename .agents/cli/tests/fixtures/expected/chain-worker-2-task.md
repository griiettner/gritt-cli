---
id: TKT-0005
namespace: alice
title: Second step
artifact: task
status: planning
owner: alice
created: {{TODAY}}
updated: {{TODAY}}
chain_role: worker
chain_parent: TKT-0003
dependencies:
  - TKT-0004
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0005 Task: Second step

## Chain Role

Worker 2 of 2 in the TKT-0003 chain.
Start from a freshly updated `main` only after TKT-0004 merges and passes review.

Branch: `tkt-0005-02-two`

## Goal

TODO(tkt): state what this single step delivers.

## Scope

- TODO(tkt): keep this to the one step; anything else belongs to another worker.

## Out of Scope

- TODO(tkt): name the neighbouring steps this worker must not touch.

## Acceptance Criteria

- TODO(tkt): give concrete criteria the reviewer can check on the PR.

## Verification

- TODO(tkt): name the commands and manual checks for this step.
- Run `gritt-agent ticket chain-check --ticket TKT-0005 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.

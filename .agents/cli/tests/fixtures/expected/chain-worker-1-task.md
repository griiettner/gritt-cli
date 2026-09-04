---
id: TKT-0004
namespace: alice
title: First step
artifact: task
status: ready
owner: alice
created: {{TODAY}}
updated: {{TODAY}}
chain_role: worker
chain_parent: TKT-0003
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0004 Task: First step

## Chain Role

Worker 1 of 2 in the TKT-0003 chain.
Start from a freshly updated `main`. This is the first worker in the chain.

Branch: `tkt-0004-01-one`

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
- Run `gritt-agent ticket chain-check --ticket TKT-0004 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.

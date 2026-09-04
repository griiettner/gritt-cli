---
id: TKT-0003
namespace: alice
title: Sample chain
artifact: plan
status: planning
owner: alice
created: {{TODAY}}
updated: {{TODAY}}
chain_role: orchestrator
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0003 Plan: Sample chain

## Sequence

1. TKT-0004 on `tkt-0004-01-one`. TODO(tkt): describe the step.
2. TKT-0005 on `tkt-0005-02-two`. TODO(tkt): describe the step.
3. TKT-0006 runs the final integrated review.

After each merge the reviewer runs the chain check, then a semantic pass.

## Decisions To Lock Before Execution

- TODO(tkt): record any open process or implementation decision, or state none.

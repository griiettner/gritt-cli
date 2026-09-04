---
id: TKT-0003
namespace: alice
title: Sample chain
artifact: task
status: planning
owner: alice
created: {{TODAY}}
updated: {{TODAY}}
chain_role: orchestrator
chain_children:
  - TKT-0004
  - TKT-0005
  - TKT-0006
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0003 Task: Sample chain

## Goal

TODO(tkt): state the concrete outcome this chain delivers.

## Chain Execution Contract

- Execution mode: `tkt-exec-chain`
- Base branch: `main`
- Branch naming pattern: `tkt-{id}-{step}-{slug}` (`{id}` is the worker ticket number, `{step}` the two-digit step, `{slug}` the step slug)
- Merge policy: Each worker opens a PR against main; reviewer runs after every PR; do not wait for CI/CD before merge when quota is unreliable.
- Reviewer gate: reviewer runs after every worker PR
- Child tickets: required and fixed as TKT-0004 through TKT-0006
- Validation required on every worker step: TODO(tkt): name the checks
- Benchmark requirements: TODO(tkt): name them or state none
- Final completion condition: TODO(tkt): state it
- Concurrency: exactly one active worker; no later step starts before the previous PR merges

## Child Ticket Chain

1. [TKT-0004 First step](../TKT-0004/task.md)
2. [TKT-0005 Second step](../TKT-0005/task.md)
3. [TKT-0006 Review integrated Sample chain chain](../TKT-0006/task.md) (final reviewer)

The orchestrator activates exactly one worker ticket at a time. Every
worker opens one PR and receives a reviewer verdict before merge. The next
worker is activated only after that merge.

## Inputs

- TODO(tkt): list the plans, ADRs, and package READMEs a worker must read.

## Scope

- TODO(tkt): describe the work covered by the child chain.

## Out of Scope

- TODO(tkt): describe what the chain must not change.

## Acceptance Criteria

- TODO(tkt): give concrete, checkable criteria.

## Verification

- TODO(tkt): name the checks every worker and reviewer pass must respect.
- Run `gritt-agent ticket chain-check --ticket TKT-0003 --base main` before semantic review.

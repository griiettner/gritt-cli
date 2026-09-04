---
id: TKT-0006
namespace: alice
title: Review integrated Sample chain chain
artifact: task
status: planning
owner: alice
created: {{TODAY}}
updated: {{TODAY}}
chain_role: reviewer
chain_parent: TKT-0003
dependencies:
  - TKT-0004
  - TKT-0005
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0006 Task: Review integrated Sample chain chain

## Chain Role

Final reviewer ticket for the TKT-0003 chain. Per-worker PR review stays
mandatory throughout the chain. This ticket runs the integrated pass after
TKT-0005 and every earlier worker ticket have merged.

## Goal

Independently determine whether the merged result satisfies the parent
contract without scope drift, integration gaps, regressions, or missing
evidence.

## Review Scope

- Re-run deterministic ticket and chain validation.
- Review the full diff across TKT-0004 through TKT-0005.
- Load `review/ticket` against TKT-0003's task.md for completion readiness, and `review/impact` across the merged diff for integration conflicts.
- TODO(tkt): name the architecture and behavior checks specific to this chain.

## Acceptance Criteria

- Every parent and child acceptance criterion has evidence.
- All worker PRs have recorded reviewer verdicts and required validation.
- No unresolved high or medium finding blocks completion.
- TKT-0003 receives a completion report only after this reviewer returns `pass`.

## Verification

- Run `gritt-agent ticket validate`.
- Run `gritt-agent ticket chain-check --ticket TKT-0006 --base main`.
- Re-run the scoped command set recorded by the parent and worker tickets.
- Produce a typed verdict: `pass`, `needs-fix`, or `blocked`, with findings
  and next actions.

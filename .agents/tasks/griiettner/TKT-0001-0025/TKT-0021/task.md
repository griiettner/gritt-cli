---
id: TKT-0021
namespace: griiettner
title: Review integrated OpenCode-inspired agent TUI and MCP harness chain
artifact: task
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-05
chain_role: reviewer
chain_parent: TKT-0015
dependencies:
  - TKT-0016
  - TKT-0017
  - TKT-0018
  - TKT-0019
  - TKT-0020
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
  - docs
  - .agents/plans
skills:
  - tkt
  - tkt-exec-chain
  - dev-harness
  - dev-provider
  - codebase-design
  - tdd
  - write-plan
---

# TKT-0021 Task: Review integrated OpenCode-inspired agent TUI and MCP harness chain

## Chain Role

Final reviewer ticket for the TKT-0015 chain. Per-worker PR review stays
mandatory throughout the chain. This ticket runs the integrated pass after
TKT-0020 and every earlier worker ticket have merged.

## Goal

Independently determine whether the merged result satisfies the parent
contract without scope drift, integration gaps, regressions, or missing
evidence.

## Review Scope

- Re-run deterministic ticket and chain validation.
- Review the full diff across TKT-0016 through TKT-0020.
- Load `review/ticket` against TKT-0015's task.md for completion readiness, and `review/impact` across the merged diff for integration conflicts.
- Check that the TUI remains a client of the control plane, provider quirks
  stay behind adapters, native and connector authority remain distinct, every
  `.mcp.json` entry is accounted for, MCP tools pass through policy and shared
  events, secrets are protected, and sidebar values distinguish known,
  estimated, and unavailable data. Check home/conversation layout, command
  parity, narrow-terminal behavior, scroll hold, cancellation, process
  cleanup, performance evidence, and the model-switching limitation.

## Acceptance Criteria

- Every parent and child acceptance criterion has evidence.
- All worker PRs have recorded reviewer verdicts and required validation.
- No unresolved high or medium finding blocks completion.
- TKT-0015 receives a completion report only after this reviewer returns `pass`.

## Verification

- Run `gritt-agent ticket validate`.
- Run `gritt-agent ticket chain-check --ticket TKT-0021 --base main`.
- Re-run the scoped command set recorded by the parent and worker tickets.
- Produce a typed verdict: `pass`, `needs-fix`, or `blocked`, with findings
  and next actions.

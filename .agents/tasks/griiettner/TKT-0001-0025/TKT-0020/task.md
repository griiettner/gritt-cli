---
id: TKT-0020
namespace: griiettner
title: Complete documentation, performance benchmarks, and integrated TUI hardening
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
dependencies:
  - TKT-0019
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

# TKT-0020 Task: Complete documentation, performance benchmarks, and integrated TUI hardening

## Chain Role

Worker 5 of 5 in the TKT-0015 chain.
Start from a freshly updated `main` only after TKT-0019 merges and passes review.

Branch: `tkt-0020-05-hardening`

## Goal

Close the integrated feature with documentation, deterministic responsiveness
benchmarks, available live MCP smoke checks, and fixes for regressions found
across the merged chain.

## Scope

- Update terminal, provider, tool/permission, and getting-started docs. Add
  benchmark fixtures for long transcripts, high-rate deltas, MCP load, hung
  servers, large results, and cancellation. Run full workspace validation,
  inspect secrets and connector authority, and fix only defects required by
  the parent acceptance criteria.

## Out of Scope

- Do not add new product capabilities, change the plan's provider/session
  contract, introduce LSP or skill execution, or bypass failing tests by
  weakening acceptance criteria. Do not rewrite unrelated existing docs.

## Acceptance Criteria

- Docs match the implemented commands and limitations; benchmark evidence
  records p50/p95 latency, CPU, memory, and queue behavior; all available
  `.mcp.json` entries have honest smoke-test results; full validation is green
  or has explicitly recorded environment-only limitations; and no unresolved
  medium or high regression remains.

## Verification

- Run `cargo fmt --all --check`, workspace clippy, `cargo test --workspace`,
  the agent CLI tests, TUI/MCP benchmarks, available live connector and MCP
  smoke tests, docs link checks where available, and chain-check. Perform the
  manual real-terminal walkthrough and record the machine and terminal.
- Run `gritt-agent ticket chain-check --ticket TKT-0020 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.

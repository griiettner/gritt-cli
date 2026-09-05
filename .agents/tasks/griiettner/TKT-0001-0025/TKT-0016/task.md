---
id: TKT-0016
namespace: griiettner
title: Define model, effort, session-draft, and provider setup contracts
artifact: task
status: ready
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
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

# TKT-0016 Task: Define model, effort, session-draft, and provider setup contracts

## Chain Role

Worker 1 of 5 in the TKT-0015 chain.
Start from a freshly updated `main`. This is the first worker in the chain.

Branch: `tkt-0016-01-contracts`

## Goal

Freeze the provider-neutral effort and session-draft contracts that later TUI
and provider work can consume, plus typed setup outcomes for selecting a
provider profile, model, and credential reference.

## Scope

- Add `ReasoningEffort`, request-option compatibility, native session effort,
  model capability representation, session-draft validation, and provider
  setup result types in the appropriate core, provider, harness, and binary
  seams. Add JSON compatibility and contract tests.

## Out of Scope

- Do not implement MCP transports or `.mcp.json` loading, Ratatui rendering,
  slash commands, sidebar visuals, performance benchmarks, or documentation
  beyond contract comments needed for the public types.

## Acceptance Criteria

- Later workers can construct a draft, validate profile/model/effort together,
  persist native effort with backward-compatible session data, and send typed
  request options without parsing error strings. Existing provider and session
  fixtures still deserialize.

## Verification

- Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test -p gritt-core -p gritt-provider -p gritt-harness`, and the
  workspace tests relevant to changed contracts. Review serialized fixtures for
  additive compatibility, then run chain-check before review.
- Run `gritt-agent ticket chain-check --ticket TKT-0016 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.

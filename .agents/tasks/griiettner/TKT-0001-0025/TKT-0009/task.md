---
id: TKT-0009
namespace: griiettner
title: Establish the Rust workspace, MIT licensing, domain contracts, unified events, configuration, and single embedded Turso database schema
artifact: task
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0009 Task: Establish the Rust workspace, MIT licensing, domain contracts, unified events, configuration, and single embedded Turso database schema

## Chain Role

Worker 1 of 5 in the TKT-0008 chain.
Start from a fresh worktree branched from `feature/tkt-0008-gritt-cli`, which is created from `main` before this worker.

Branch: `tkt-0009-01-contracts`

## Goal

Create the compilable Rust product workspace and freeze the domain, configuration, event, and persistence seams that every later worker consumes.

## Scope

- Add the Cargo workspace and MIT licensing.
- Define provider-neutral events, sessions, tools, approvals, connector capabilities, configuration precedence, and error types.
- Integrate one embedded Turso/libSQL database with separate migrations or table namespaces for `gritt-agent` memory and Gritt sessions, telemetry, and analytics.
- Add safe secret references, opt-in embedding/reranking environment configuration, and schema tests.

## Out of Scope

- Do not implement provider HTTP/SSE behavior, terminal rendering, tool execution, connector processes, packaging, or live model tests. Those belong to TKT-0010 through TKT-0013.

## Acceptance Criteria

- Workspace compiles with the chosen Rust toolchain and MIT license files are present.
- Public contracts cover all plan entities without provider-specific leakage.
- A temporary database can create and migrate both namespaces, and existing `gritt-agent` memory commands retain their contract.
- Secrets are represented only by key references and environment names, never persisted values.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `gritt-agent ticket chain-check --ticket TKT-0009 --base feature/tkt-0008-gritt-cli`
- Run schema migration tests against a temporary local database and inspect the generated license and workspace metadata.
- Run `gritt-agent ticket chain-check --ticket TKT-0009 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.

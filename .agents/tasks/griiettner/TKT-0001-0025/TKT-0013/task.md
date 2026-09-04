---
id: TKT-0013
namespace: griiettner
title: Complete cross-platform reproducible builds, diagnostics, documentation, end-to-end verification, and integrated hardening
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
dependencies:
  - TKT-0012
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0013 Task: Complete cross-platform reproducible builds, diagnostics, documentation, end-to-end verification, and integrated hardening

## Chain Role

Worker 5 of 5 in the TKT-0008 chain.
Start from a fresh worktree branched from the latest merged `feature/tkt-0008-gritt-cli` only after TKT-0012 merges and passes review.

Branch: `tkt-0013-05-release`

## Goal

Turn the integrated CLI into a reproducible, documented release candidate and close cross-step defects found by end-to-end verification.

## Scope

- Add reproducible build workflows and checksums for macOS, Windows, and Linux without signed installers.
- Complete diagnostics, configuration help, user documentation, plan and ADR references, and upgrade guidance.
- Run end-to-end native and connector flows, cross-platform checks available in the environment, and hardening for errors, cancellation, and migrations.
- Fix only integration defects required to satisfy the parent contract and record follow-ups for deferred work.

## Out of Scope

- Do not add a desktop frontend, cloud service, hosted telemetry, signed distribution, or new product feature outside the plan.

## Acceptance Criteria

- Reproducible build instructions and checksums work for all three target platforms or clearly report environment-only limitations.
- Documentation describes provider setup, key handling, tools, connectors, local database namespaces, telemetry, embeddings, reranking, and privacy boundaries.
- End-to-end tests cover native planning/coding, approvals, resume, cancellation, connector failure, and migration behavior.
- Full validation is green and all known deviations are recorded as follow-ups rather than hidden.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --workspace`
- Run reproducible build checks and checksum comparison for available targets.
- Run live connector tests when available and fixture tests otherwise.
- `gritt-agent ticket chain-check --ticket TKT-0013 --base feature/tkt-0008-gritt-cli`
- Run `gritt-agent ticket chain-check --ticket TKT-0013 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.

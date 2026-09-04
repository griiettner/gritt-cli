---
id: TKT-0008
namespace: griiettner
title: Build the complete Gritt local AI coding agent CLI
artifact: task
status: planning
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: orchestrator
chain_children:
  - TKT-0009
  - TKT-0010
  - TKT-0011
  - TKT-0012
  - TKT-0013
  - TKT-0014
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0008 Task: Build the complete Gritt local AI coding agent CLI

## Goal

griiettner/TKT-0008 delivers a complete, MIT-licensed, terminal-first Rust Gritt CLI for macOS, Windows, and Linux. It supports native and supervised external coding agents, provider-neutral sessions and events, approved command execution, local memory and sessions in one embedded Turso/libSQL database, opt-in embeddings and reranking, local telemetry, and reproducible builds.

## Chain Execution Contract

- Execution mode: `tkt-exec-chain`
- Base branch: `main`
- Chain integration branch: `feature/tkt-0008-gritt-cli`, created from `main` before worker 1.
- Branch naming pattern: `tkt-{id}-{step}-{slug}` (`{id}` is the worker ticket number, `{step}` the two-digit step, `{slug}` the step slug)
- Merge policy: Each worker opens a PR from its worktree branch into `feature/tkt-0008-gritt-cli`; reviewer runs after every PR and the PM merges it into that integration branch. After worker 5, open one master PR from `feature/tkt-0008-gritt-cli` into `main` and merge it after the final reviewer pass. Do not wait for CI/CD before merge when quota is unreliable.
- Reviewer gate: reviewer runs after every worker PR
- Child tickets: required and fixed as TKT-0009 through TKT-0014
- Validation required on every worker step: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, focused and full `cargo test`, `gritt-agent ticket chain-check`, `gritt-agent ticket validate`, and relevant live or fixture integration tests.
- Benchmark requirements: no performance target is imposed. Record build reproducibility, test duration, and connector smoke-test results when available.
- Final completion condition: all five worker PRs are reviewed and merged into the chain feature branch, the master PR to `main` is merged, the final reviewer returns `pass`, and all acceptance and verification evidence is recorded.
- Concurrency: exactly one active worker; no later step starts before the previous PR merges

## Child Ticket Chain

1. [TKT-0009 Establish the Rust workspace, MIT licensing, domain contracts, unified events, configuration, and single embedded Turso database schema](../TKT-0009/task.md)
2. [TKT-0010 Implement provider adapters, streaming normalizers, model caching, capability checks, opt-in embeddings and reranking](../TKT-0010/task.md)
3. [TKT-0011 Implement sessions, planning and coding phases, permissions, workspace-bounded tools, terminal modes, approvals, cancellation, and telemetry](../TKT-0011/task.md)
4. [TKT-0012 Implement supervised native and external connectors with PTY fallback, live Codex and Claude Code tests, and normalized events](../TKT-0012/task.md)
5. [TKT-0013 Complete cross-platform reproducible builds, diagnostics, documentation, end-to-end verification, and integrated hardening](../TKT-0013/task.md)
6. [TKT-0014 Review integrated Build the complete Gritt local AI coding agent CLI chain](../TKT-0014/task.md) (final reviewer)

The orchestrator activates exactly one worker ticket at a time. Every
worker opens one PR and receives a reviewer verdict before merge. The next
worker is activated only after that merge.

## Inputs

- `.agents/plans/plan1.md` and `plan1.html`.
- ADR-001 through ADR-011 under `.agents/memory/decisions/`.
- `.agents/brain/README.md`, `.agents/brain/architecture.md`, and `.agents/brain/providers.md`.
- `.agents/cli/` and its README, tests, and current embedded database schema.
- `AGENTS.md`, the applicable `dev-cli`, `dev-provider`, `dev-harness`, `codebase-design`, `domain-modeling`, `tdd`, and `write` skills.

## Scope

- Build the complete native provider path and terminal harness described in `plan1.md`.
- Add supervised native, Codex, Claude Code, Cursor, and OpenCode connector support, with live tests when installed and fixtures otherwise.
- Share one embedded Turso/libSQL database between `gritt-agent` memory and Gritt product data using namespaced migrations.
- Include opt-in embedding and reranking providers, local telemetry and analytics, diagnostics, documentation, and reproducible release verification.

## Out of Scope

- A non-terminal desktop or web frontend.
- Gritt Cloud, Turso Cloud, remote telemetry, or analytics uploads.
- Signed installers, hosted update infrastructure, or cloud session synchronization.
- New provider protocols or connector behaviors not supported by the plan and ADRs.
- Changes to the established `gritt-agent` ticket, skill, and memory contracts unless required to share the embedded database safely.

## Acceptance Criteria

- The Rust workspace builds and tests on macOS, Windows, and Linux targets, with MIT license files and reproducible build instructions.
- Native OpenRouter, OpenAI Responses and Chat Completions, Anthropic Messages, and generic OpenAI-compatible profiles stream through one event model.
- Model lists refresh at most daily, stale cache fallback is visible, provider capabilities are enforced, and alias remapping follows the recorded policy.
- Planning and coding phases share named, resumable sessions. Native file and shell tools are workspace-bounded and pass the allow/ask/deny policy before execution.
- Print, REPL, and full-screen Ratatui modes expose streamed output, approvals, cancellation, and diff review.
- Native, Codex, Claude Code, Cursor, and OpenCode connectors preserve external authority, normalize events, supervise processes, and pass live or fixture coverage.
- One local embedded database stores memory, sessions, telemetry, and analytics in separate namespaces. Secrets never enter config values, logs, fixtures, transcripts, or telemetry.
- Every worker PR and the final master PR is reviewed, merged, and recorded with validation evidence.

## Verification

- Every worker runs formatting, clippy, focused tests, full tests, ticket validation, and its task-specific live or fixture checks.
- The PM runs `gritt-agent ticket chain-check --ticket TKT-0008 --base main` before every semantic review and confirms each PR is merged before dispatching the next worker.
- The final reviewer runs the complete Rust test suite, cross-target reproducibility checks available in the environment, live connector tests when credentials and CLIs exist, fixture tests otherwise, and a full diff impact review.
- Run `gritt-agent ticket chain-check --ticket TKT-0008 --base main` before semantic review.

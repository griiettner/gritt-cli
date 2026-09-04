---
id: architecture-model-routing
title: Agent model and CLI routing
type: architecture
status: active
created: 2026-08-14
updated: 2026-09-03
tags:
  - agents
  - models
  - cli
  - workflow
read_when:
  - changing subagent routing
  - adding a model or CLI backend
  - diagnosing delegation or model-slug failures
---

# Agent model and CLI routing

## Supported CLIs

Delegation uses the installed Claude Code, Grok, and Codex CLIs. Cursor and
OpenCode are outside the routing contract for this machine.

## Role boundaries

- Claude Fable 5.1 at medium effort orchestrates work and owns context-dependent decisions.
- Grok 4.6 at high effort implements clear tasks and written plans.
- GPT 6 Astra reviews completed work without editing it.
- GPT 5.6 Sol at medium effort is the reviewer fallback when GPT 6 Astra fails.
- GPT 5.6 Luna writes reports, ticket artifacts, and other authoring deliverables.

The split is intentionally small. Each role has one primary CLI and model.
Only review has an automatic fallback.

## Context locality

Delegated processes start cold and do not inherit the current conversation.
Prompts must include the task, constraints, relevant paths, expected output,
and verification requirements. Keep judgments that rely on accumulated context
in the orchestrator session.

## Invocation safety

- Use `claude -p`, `grok -p`, and `codex exec` for non-interactive calls.
- Give implementation workers explicit scope and validation requirements.
- Give reviewers explicit no-edit instructions and inspect the diff afterward.
- Do not silently substitute unavailable CLIs or models outside the documented reviewer fallback.

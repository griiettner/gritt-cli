---
id: architecture-model-routing
title: Agent model and CLI routing
type: architecture
status: active
created: 2026-08-14
updated: 2026-09-04
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
Review and Implementation are the only roles with a documented fallback.

### Grok CLI classifier block (2026-09-04)

Grok CLI's non-interactive route (`grok -p`, `--always-approve`) is blocked
by the auto-mode safety classifier on this machine. This reproduced across
two separate chain runs (TKT-0008 and the follow-up chain) and is treated as
a standing environment constraint, not a transient failure — do not keep
retrying `grok -p` inside the same chain once it has been denied once.

Two independent mechanisms can deny a `grok` call:

1. The Bash permission allow-list only matches bare-prefix commands
   (`Bash(grok:*)`). Chaining `grok` into a compound command with `;`, `&&`,
   or `|` defeats the match even though the allow rule exists.
2. The auto-mode classifier separately denies headless/auto-approve
   invocation of another autonomous coding-agent CLI, regardless of the
   permission allow-list. This looks like an intentional guardrail against
   one agent spawning another unsupervised agent and is not expected to be
   fixable through `.claude/settings.json`.

The sanctioned fallback (see `.agents/MODELS.md`) is a forked in-harness
Claude Code agent — not a `claude -p` subprocess — running `claude-opus-4-8`
at effort `high` for the Implementation role. This is the only fallback that
changes CLI rather than just model, and it must be recorded in the chain
report every time it's used, never substituted silently.

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

---
id: models
name: models
summary: Model and CLI routing contract for delegated work.
status: active
load: on_demand
read_when:
  - choosing a model
  - delegating work
  - invoking Codex, Claude Code, or Grok CLI
---

# Model routing

Read this file before delegating work. Detailed rationale and troubleshooting
live in `.agents/memory/architecture/model-routing.md`.

## Available CLIs

This project delegates through three local CLIs:

| CLI | Binary | Non-interactive form |
| --- | --- | --- |
| Claude Code | `claude` | `claude -p` |
| Grok CLI | `grok` | `grok -p` |
| Codex | `codex` | `codex exec` |

Cursor and OpenCode are not part of the routing contract.

Before the first delegation, verify the required binary:

```bash
command -v claude >/dev/null 2>&1
command -v grok >/dev/null 2>&1
command -v codex >/dev/null 2>&1
```

## Role routing

| Role | CLI | Model | Effort | Fallback |
| --- | --- | --- | --- | --- |
| Orchestrator | Claude Code | Fable 5.1 | medium | none |
| Implementation | Grok CLI | Grok 4.6 | high | forked in-harness Claude Code agent, Opus 4.8, high |
| Reviewer | Codex | GPT 6 Astra | model default | GPT 5.6 Sol, medium |
| Author, report, and TKT writer | Codex | GPT 5.6 Luna | model default | none |

Use these command forms:

```bash
claude -p --model fable --effort medium "<prompt>"
grok -p --model grok-4.6 --reasoning-effort high "<prompt>"
codex exec --model gpt-6-astra "<prompt>"
codex exec --model gpt-5.6-sol -c model_reasoning_effort='medium' "<prompt>"
codex exec --model gpt-5.6-luna "<prompt>"
```

The GPT 5.6 Sol command is only the reviewer fallback. Use it when GPT 6 Astra
is unavailable, not as a general substitute for another role.

Grok CLI's non-interactive route (`grok -p` / `--always-approve`) is blocked
by the auto-mode safety classifier in this environment, categorically, not as
a permission-allow-list gap. When `grok -p` is unavailable or classifier-
blocked, use a forked in-harness Claude Code agent (the Agent tool, not a
`claude -p` subprocess) running model `claude-opus-4-8` at effort `high` as
the Implementation worker instead. This is the only role with a fallback that
changes CLI, not just model — record every use of it in the chain/orchestrator
report; it must never happen silently.

## Local overrides

`.agents/.env` may replace a role command. If a matching variable is set,
source the file and run its value verbatim.

| Variable | Role |
| --- | --- |
| `AGENT_ORCHESTRATOR` | Orchestrator |
| `AGENT_IMPLEMENTATION` | Implementation |
| `AGENT_REVIEWER` | Reviewer |
| `AGENT_AUTHOR` | Author, report, and TKT writer |

Ignore an override when it is absent, empty, or set to `none`.

## Delegation rules

- Delegate implementation only after the orchestrator has produced a clear task or plan.
- Keep context-dependent decisions in the orchestrator session.
- Make every delegated prompt self-contained because workers start cold.
- Tell reviewers not to edit application code, then inspect the diff after review.
- Use GPT 5.6 Sol only after the GPT 6 Astra reviewer call fails.
- Use the Opus 4.8 in-harness fallback only after `grok -p` fails or is
  classifier-blocked, and state the deviation in the report every time.
- Do not silently replace any other unavailable CLI or model. Report the blocker.

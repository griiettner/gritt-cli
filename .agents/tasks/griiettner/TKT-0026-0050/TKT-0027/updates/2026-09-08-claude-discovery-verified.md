---
id: TKT-0027
namespace: griiettner
title: Verify Claude Code model discovery with subscription authentication
artifact: update
status: done
owner: griiettner
created: 2026-09-08
updated: 2026-09-08
---

# Claude Code model discovery

The API-only experiment failed on this installation because Gritt had no
Anthropic API key. Compilation did not validate the requested user flow.

Claude Code's stream-json initialization response provides a models array
using its own authentication. The connector now sends an initialization
control request followed by EOF, parses only model ids and display labels,
and caches those through the existing discovery service. Discovery disables
hooks and MCP servers and submits no user prompt. Timeout and process cleanup
use the existing probe machinery. Native Anthropic discovery is unchanged;
the experimental API catalog remains a fallback if CLI discovery fails.

The initialization response contains account data, which is not copied into
the catalog. It reports no prices, so the picker leaves pricing absent.

## Verification

- Release build completed without warnings.
- All 15 connector model tests passed, including initialization over stdin
  and forwarding the selected model to the agent.
- All 13 connector unit tests passed.
- Live Claude Code discovery test passed against installed version 2.1.263.
- Actual Gritt REPL startup and `/models refresh` returned Default, Opus,
  Fable, Sonnet, and Haiku without an Anthropic API key.
- Actual full-screen terminal: `/connect`, Claude Code, Ctrl-R, filter Haiku,
  Enter, submit a tool-free test prompt. The selected session returned
  `GRITT_MODEL_OK` and finished successfully.

## Completion gate

- Acceptance: model discovery, refresh, selection, and a real turn passed.
- Scope: connector discovery plus the experimental catalog routing.
- Safety: no credential extraction; initialization uses Claude Code's login.
- Regression risk: the control-response schema can change in future CLI
  versions; malformed responses use existing cache/failure handling.
- Follow-up: pricing is still unavailable from this metadata source.
- Assumption: prefer Claude Code's own catalog for subscription users;
  use the configured API catalog only as a fallback.

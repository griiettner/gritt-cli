---
id: ADR-007
title: Provider and session contracts
status: accepted
date: 2026-09-04
tags:
  - providers
  - sessions
  - events
read_when:
  - adding a provider adapter
  - changing event or session types
  - adding a continuation mechanism
---

# ADR-007: Provider and session contracts

## Decision

Gritt routes requests by configured provider profile, never by model-name
guessing. Every adapter emits one provider-neutral event model for streamed
text, reasoning summaries, tool calls and results, approvals, usage, status,
errors, and completion. Provider-specific data is optional diagnostic metadata.

OpenAI profiles support both Responses and Chat Completions. OpenRouter and
generic endpoints use Chat Completions first. Anthropic uses Messages. Each
wire envelope has its own normalizer and tool-schema generator.

Sessions belong to Gritt. They are named, listable, resumable, and removable,
and store adapter continuation state behind the session interface. Planning is
conversation-first. Coding is the tool-using execution phase. Both phases use
the same session and event model.

## Rationale

The shared model lets native and connector sessions appear in one interface
without making higher layers provider-aware.

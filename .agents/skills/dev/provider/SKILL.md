---
name: dev-provider
description: Builds provider adapters, model list caching, and response normalizers. Use when touching HTTP clients, SSE parsing, tool schemas, or provider fixtures.
---

# Provider

Read [dev](../SKILL.md) first. The plan's "Providers" section fixes the provider set, protocols, and implementation order. Read it before changing a request builder.

## Adapter contract

One trait in `gritt-core`, implemented per protocol in `gritt-provider`:

- send a prompt and receive a stream of provider-neutral events
- submit tool results
- restore a session from stored continuation state
- report capabilities for a selected model
- report errors in the internal error kinds

Implementation order: OpenAI-compatible Chat Completions (covers OpenRouter and the generic profile), then OpenAI Responses, then Anthropic Messages. Do not start a later one early.

A profile selects an adapter and supplies base URL and key variable. The same Chat Completions adapter serves OpenRouter, OpenAI in Chat Completions mode, and any generic endpoint. Do not fork it per vendor.

## Model lists

- Each adapter fetches its provider's list (`GET /models` for OpenAI-compatible, `GET /v1/models` for Anthropic) with the profile's key.
- Cache to disk per profile with a fetch timestamp. Expose refresh and an expiry policy. On failure, use the last cached list and mark it stale in response metadata.
- Record whatever capabilities the provider reports: context length, tools, vision, structured output, pricing. Do not fill gaps with guesses.
- Aliases map to a profile and model id. Resolution fails when an alias is ambiguous across profiles.

## Request builder

- Include only fields the selected model is known to support. Raise the unsupported-capability error when the user asked for a feature the list does not report. Never send it and hope.
- `model` is the only required identity field on every protocol.
- Anthropic requires `max_tokens` and a versioned `anthropic-version` header. OpenRouter accepts optional attribution headers. Keep these inside the adapter.

## Normalizers

One normalizer per wire envelope. They share the event model, not parsing code.

- Chat Completions: `choices[].message` and streamed `choices[].delta`, tool calls assembled from indexed fragments, `finish_reason`, `usage`.
- Responses: top-level `output` items (`reasoning`, `message` with `output_text`, `function_call`), the `response.*` event set, `usage`. Store the top-level response `id` and send it verbatim as the next `previous_response_id`.
- Messages: `content` blocks (`text`, `tool_use`, `thinking`), the `message_start`, `content_block_*`, `message_delta`, and `message_stop` events, `stop_reason`, `usage`.

## Streaming

- Parse `text/event-stream` incrementally. Never buffer the whole body.
- Preserve event order and any sequence field. A gap or reorder is a diagnostic warning, not a silent fix.
- Unknown event types are logged and skipped, never fatal.
- Cancellation drops the connection and emits a terminal cancelled event.

## Tool schemas

Generate tool definitions per adapter. Chat Completions and Responses take `function` tools with JSON schema `parameters`. Messages takes `input_schema`. Optional fields such as `strict` are emitted only where the protocol documents them. Keep every quirk in the adapter, not in the tool definition.

## Fixtures and tests

Every adapter behavior is covered by a recorded fixture under the crate's `tests/fixtures/<protocol>/` before it is covered by a live call:

- plain text, streaming, reasoning, tool call, tool result, continuation, and a provider error body
- redact keys and any prompt content when recording
- keep fixtures free of base64 blobs such as inline images or files; replace them with a short placeholder and note the original length
- name the fixture by case, for example `chat-completions/stream-tool-call.sse`

Contract tests replay fixtures through the full normalizer and assert the event sequence. Live tests are gated by `GRITT_LIVE_TESTS=1` plus a key for the profile under test and are never required for a pass.

## Output

Adapter and protocol touched, fixtures added, capability rules applied, and any provider question left open. Update the ticket report when the work is ticket-driven.

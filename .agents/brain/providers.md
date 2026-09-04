---
id: brain-providers
title: Agent brain providers
status: active
date: 2026-08-13
tags:
  - agent-brain
  - providers
  - security
  - local-first
read_when:
  - enabling an embedding or generation provider
  - reviewing network behavior
  - adding provider configuration
---

# Agent Brain Providers

## Configuration contract

Each capability has exactly one key whose value is the model identifier to use.
There is no separate `*_MODEL` key.

| Key                        | Capability                    | Default |
| -------------------------- | ----------------------------- | ------- |
| `AGENT_AI_PROVIDER`        | Generation and summarization  | off     |
| `AGENT_EMBEDDING_PROVIDER` | Semantic retrieval vectors    | off     |
| `AGENT_RERANK_PROVIDER`    | Retrieved candidate reranking | off     |

A missing key, an empty value, and `none` are equivalent, so the capability
stays off. Nothing needs to be set to `none` explicitly, and `.agents/.env`
itself is optional. The current `gritt-agent` CLI reads none of these keys; the
contract is recorded here for the phase that adds providers.

## Default configuration

No `.agents/.env` file, or the file present with every provider key commented
out. This mode uses the embedded Turso engine and local FTS only.

## Enabled configuration

```text
AGENT_MEMORY_API_KEY=<key for an OpenAI-compatible endpoint>
AGENT_MEMORY_BASE_URL=https://openrouter.ai/api
AGENT_AI_PROVIDER=gpt-5-nano
AGENT_EMBEDDING_PROVIDER=text-embedding-3-small
AGENT_RERANK_PROVIDER=rerank-3.5
```

Provider initialization must resolve each capability from the environment
before making a request. No provider may
make a network request when its resolved value is `null` or when the gateway
credentials are absent. Indexing and search always succeed offline: missing
providers leave Turso FTS as the only retrieval path, and any gateway failure
falls back to local FTS without failing the request.

## Storage contract

All optional providers reach the configured OpenAI-compatible endpoint through
the shared `AGENT_MEMORY_API_KEY` / `AGENT_MEMORY_BASE_URL` pair. `text-embedding-3-small` vectors are 1536-dimensional and require a
`F32_BLOB(1536)` column. Local FTS content remains authoritative when no embedding
is available. Reranking only reorders retrieved candidates; it
does not generate query expansions or require direct AWS credentials.

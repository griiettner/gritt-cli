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

A missing key, an empty value, and `none` are equivalent: `config.mjs` resolves
all three to `null`, so the capability stays off. Nothing needs to be set to
`none` explicitly, and `.agents/.env` itself is optional.

## Default configuration

No `.agents/.env` file, or the file present with every provider key commented
out. This mode uses local libSQL and FTS5 only.

## Enabled configuration

```text
AGENT_MEMORY_API_KEY=<key for an OpenAI-compatible endpoint>
AGENT_MEMORY_BASE_URL=https://openrouter.ai/api
AGENT_AI_PROVIDER=gpt-5-nano
AGENT_EMBEDDING_PROVIDER=text-embedding-3-small
AGENT_RERANK_PROVIDER=rerank-3.5
```

Provider initialization must check `providers.ai`, `providers.embedding`, and
`providers.rerank` from `config.mjs` before making a request. No provider may
make a network request when its resolved value is `null` or when the gateway
credentials are absent. Indexing and search always succeed offline: missing
providers leave FTS5 as the only retrieval path, and any gateway failure falls
back to FTS5 without failing the request.

## Storage contract

All optional providers reach the configured OpenAI-compatible endpoint through
the shared `AGENT_MEMORY_API_KEY` / `AGENT_MEMORY_BASE_URL` pair. `text-embedding-3-small` vectors are 1536-dimensional and require a
`F32_BLOB(1536)` column. FTS5 content remains authoritative when no embedding
is available. Reranking only reorders retrieved candidates; it
does not generate query expansions or require direct AWS credentials.

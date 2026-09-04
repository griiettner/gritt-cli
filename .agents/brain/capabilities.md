---
id: brain-capabilities
title: Agent brain capabilities
status: active
date: 2026-08-13
tags:
  - agent-brain
  - capabilities
  - local-first
read_when:
  - deciding whether a tool needs a model
  - adding a fallback path
  - diagnosing unavailable AI services
---

# Agent Brain Capabilities

## Always available

- Index supported workspace documents.
- Split documents into line-addressable chunks.
- Search indexed chunks with SQLite FTS5.
- Read an indexed document.
- Return source paths and line ranges for retrieved chunks.
- Rebuild the local index.
- Store and retrieve lesson lifecycle artifacts.
- Produce deterministic workspace activity reports.

## Optional local intelligence

- Local embedding model for semantic retrieval.
- Local Ollama model for lesson extraction or summarization.

These capabilities may require model files and CPU/RAM, but do not require
external communication after models are installed.

## Optional external intelligence

- OpenAI-compatible embeddings.
- OpenAI-compatible chat or completion APIs.

External providers are disabled unless explicitly configured. An API key alone
must not silently enable network access.

## Fallback rule

When an optional capability is unavailable, the tool must return a useful
deterministic result or an explicit capability status. It must not fail the
entire agent workflow.

---
id: brain-architecture
title: Agent brain architecture
status: active
date: 2026-08-13
tags:
  - agent-brain
  - architecture
  - mcp
read_when:
  - changing agent awareness or retrieval
  - adding a brain tool
  - changing the local memory database
---

# Agent Brain Architecture

```text
Workspace files
      |
      v
Local indexer
      |
      v
Local Turso file database
      |
      +--> Turso FTS lexical search
      |
      +--> reserved F32_BLOB vector column (unused)
      |
      v
Local MCP server
      |
      +--> Cursor
      +--> Claude Code
```

## Boundaries

- `.agents/brain/` documents agent infrastructure.
- `.agents/memory/` stores project knowledge and architectural decisions.
- `.agents/cli/` contains the `gritt-agent` binary that indexes, searches, and serves memory.
- `.agents/tools/` holds only a README that points at the CLI. The Node
  scripts it once held were ported into `gritt-agent` and removed.
- `.agents/brain/data/` contains developer-local generated state and is
  ignored by Git.

## Retrieval policy

Turso FTS is the required baseline and the only retrieval path the CLI implements.
If embeddings or reranking are added later, vector search must never be
required to index or retrieve workspace knowledge, and any provider failure
must fall back to local Turso FTS.

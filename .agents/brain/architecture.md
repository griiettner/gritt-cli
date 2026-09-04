---
id: brain-architecture
title: Agent brain architecture
status: active
date: 2026-08-13
tags:
  - agent-brain
  - architecture
  - turso
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
Turso/libSQL file database
      |
      +--> SQLite FTS5 lexical search
      |
      +--> optional F32_BLOB vector search
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
- `.agents/tools/` contains executable workflow tools.
- `.agents/brain/data/` contains developer-local generated state and is
  ignored by Git.

## Retrieval policy

FTS5 is the required baseline. Optional optional providers may embed chunks and
rerank candidates, but vector search must never be required to index or
retrieve workspace knowledge. Any provider failure falls back to FTS5.

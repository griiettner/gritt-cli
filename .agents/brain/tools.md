---
id: brain-tools
title: Agent brain tools
status: active
date: 2026-08-13
tags:
  - agent-brain
  - tools
  - mcp
read_when:
  - using or adding agent tools
  - changing the local MCP contract
  - documenting developer commands
---

# Agent Brain Tools

## Commands

```bash
npm run agent-memory:index
npm run agent-memory:provider-check
npm run agent-memory:mcp
npm run agent-memory:dashboard
```

The index command is safe to rerun. When optional providers are configured in
`.agents/.env`, indexing also embeds pending chunks and search may use vector
retrieval plus reranking. Both enhancements fall back to FTS5 on any failure.
The MCP server also refreshes the index before accepting requests and starts
the dashboard automatically.

The dashboard is available at `http://127.0.0.1:8282` while
`agent-memory:dashboard` is running.
The engine fixes the port at `8282` and starts the dashboard with the MCP
server; neither behavior requires environment configuration.

## MCP tools

| Tool                  | Capability                                                    |
| --------------------- | ------------------------------------------------------------- |
| `search_local_memory` | Chunk search with FTS5 baseline; optional vector + rerank     |
| `read_local_memory`   | Read one indexed workspace document                           |

The dashboard reports document population, embedding coverage, recent indexed
documents, and index-run history.

Future tools should preserve the same local-first behavior:

- `recent_workspace_activity`
- `memory_status`
- `stage_lesson`
- `search_semantic_memory`

Every tool that depends on an optional provider must expose whether it used a
fallback or an unavailable capability.

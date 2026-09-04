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
cargo build --release --manifest-path .agents/cli/Cargo.toml
.agents/cli/target/release/gritt-agent memory index
.agents/cli/target/release/gritt-agent memory search "query terms" --limit 10
.agents/cli/target/release/gritt-agent memory serve
```

The index command is safe to rerun. It walks supported documents, stores
line-addressable chunks, and drops entries whose source file is gone. `memory
search` prints the same numbered citations the MCP tool returns. `memory serve`
refreshes the index before accepting requests and speaks MCP over stdio.

## MCP tools

| Tool                  | Capability                                  |
| --------------------- | ------------------------------------------- |
| `search_local_memory` | Chunk search with local Turso FTS            |
| `read_local_memory`   | Read one indexed workspace document         |

Both tools return text with `path:start-end` citations. There is no vector
retrieval, reranking, or dashboard in the current CLI.

Future tools should preserve the same local-first behavior:

- `recent_workspace_activity`
- `memory_status`
- `stage_lesson`
- `search_semantic_memory`

Every tool that depends on an optional provider must expose whether it used a
fallback or an unavailable capability.

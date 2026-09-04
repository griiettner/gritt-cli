---
id: brain-index
title: gritt-cli repository agent brain
status: active
date: 2026-09-03
tags:
  - agent-brain
  - local-first
  - rag
  - mcp
read_when:
  - configuring agent awareness
  - onboarding a developer
  - adding local agent tools
  - changing memory retrieval or providers
---

# gritt-cli repository agent brain

The agent brain is the repository-local awareness layer for gritt-cli.
It gives developers and coding agents a fast way to search project context
without loading the whole repository into every session.

The brain is intentionally small, local, and provider-independent:

- It works without an LLM.
- It works without an API key.
- It works without network access.
- It works on Windows, macOS, and Linux.
- It needs only the Rust toolchain to build.

This document is written for both human developers and AI agents. It explains
what the brain is, how it operates, how to use it, and why it exists.

## Why we use it

Large repositories contain useful context in architecture decisions, package
READMEs, tickets, agent instructions, source files, and configuration. Asking
every agent to read everything is slow, expensive, and unreliable.

The brain provides:

1. A local searchable copy of relevant workspace knowledge.
2. A standard MCP interface for Claude Code and other MCP clients.
3. Source-aware results with file paths and line ranges.
4. A terminal search command for humans and scripted checks.

The intended session pattern is:

```text
Start task
  → query local memory first
  → inspect only the relevant files
  → implement or diagnose
  → reindex changed knowledge
```

Agents should query `gritt-local-memory` before using `rg`, grepping source,
or exploring implementation details. Memory may reveal an existing decision,
pattern, ticket, or known limitation.

## Architecture

```text
Workspace files
      |
      v
gritt-agent memory index
      |
      v
SQLite file (.agents/brain/data/agent-memory.db)
      |
      +--> document metadata
      +--> line-addressable chunks
      +--> SQLite FTS5 indexes
      +--> reserved vector column (unused)
      |
      v
gritt-agent memory serve  (gritt-local-memory MCP)
      |
      +--> Claude Code
      +--> any stdio MCP client
```

The implementation lives in the `gritt-agent` crate:

```text
.agents/cli/src/memory/
├── chunk.rs      Document chunking
├── db.rs         SQLite connection and schema initialization
├── index.rs      Workspace indexer
├── mcp.rs        Stdio MCP server
├── schema.sql    Database schema
└── search.rs     FTS5 retrieval
```

Generated state is stored at:

```text
.agents/brain/data/agent-memory.db
```

`.agents/brain/data/` is ignored by Git. Each developer has an independent
local index. The database never leaves the machine and the CLI makes no
network requests.

## What gets indexed

The indexer includes:

```text
*.md
*.mdx
*.yaml
*.yml
*.json
```

It excludes generated or unsuitable directories:

```text
.git/
.agents/brain/data/
.nx/
.playwright-mcp/
node_modules/
dist/
coverage/
.output/
target/
```

The index is rebuilt incrementally by path and content hash. Removed source
files are removed from the local index on the next run.

### Chunking

Documents are split into line-addressable chunks:

- Markdown headings begin logical sections.
- Sections longer than 80 lines are split into windows that overlap by 10 lines.
- Non-Markdown documents are split by line count.
- Chunks retain heading, start-line, and end-line metadata.
- Chunk searches return citations such as:

```text
.agents/brain/README.md:67-82
```

Retrieval uses FTS5 over these chunks. Every query term must match. There is
no query expansion and no generative model.

## Quick start

From the repository root:

```bash
cargo build --release --manifest-path .agents/cli/Cargo.toml
.agents/cli/target/release/gritt-agent memory index
.agents/cli/target/release/gritt-agent memory search "provider adapter"
```

The index command is safe to run repeatedly. The same commands work in
PowerShell on Windows with the `.exe` suffix on the binary.

## MCP integration

The MCP server is configured in:

```text
.mcp.json
```

Its name is:

```text
gritt-local-memory
```

The transport is stdio. Claude Code starts
`.agents/cli/target/release/gritt-agent memory serve` when the project
configuration loads. The server reindexes before it accepts requests. It is
not a TCP server. Build the binary before the first client start, otherwise
the client reports that the command does not exist.

### Available MCP tools

| Tool                  | Purpose                                     |
| --------------------- | ------------------------------------------- |
| `search_local_memory` | Search indexed chunks with source citations |
| `read_local_memory`   | Read one indexed document by relative path  |

`search_local_memory` takes `query` and an optional `limit` from 1 to 50. It
returns the matching document title, heading, path, line range, and chunk
content. `read_local_memory` is useful after a search identifies the
authoritative file.

### Agent usage rule

Before inspecting code for a task:

1. Query `gritt-local-memory`.
2. Use the result paths to identify relevant package docs, decisions, and
   tickets.
3. Read the smallest required file set.
4. Only then search implementation code.

This avoids repeatedly rediscovering project knowledge from source files.

## Environment and providers

The current CLI reads no provider configuration. Indexing and search are
local SQLite operations, so the default and only mode is:

```text
Local SQLite + FTS5 + MCP
No embeddings
No reranking
No query expansion
No external requests
```

The schema reserves an `F32_BLOB(1536)` column on documents and chunks for a
future embedding phase. `providers.md` records the configuration contract that
phase must follow. Nothing in this repository sends data to a provider today.

## Database model

The local schema contains:

| Table                 | Purpose                             |
| --------------------- | ----------------------------------- |
| `documents`           | One row per indexed source file     |
| `document_chunks`     | Line-addressable content chunks     |
| `documents_fts`       | File-level FTS5 index               |
| `document_chunks_fts` | Chunk retrieval index               |
| `index_runs`          | Index execution history             |

SQLite is compiled into the binary through the `rusqlite` bundled build with
FTS5 enabled. No system SQLite is required.

## Troubleshooting

### No results

Rebuild the index:

```bash
.agents/cli/target/release/gritt-agent memory index
```

Then retry the search. Remember that every query term must match; drop terms
that may not appear in the document.

### Stale results

The MCP server indexes on startup. Restart the MCP process or run the index
command manually after changing source documents.

### Corrupt or incompatible local database

The database is generated state and can be recreated:

```text
Delete .agents/brain/data/agent-memory.db
Run the index command again
```

Do not delete committed source files or `.agents/memory/` files.

### MCP does not appear

Check that:

1. The binary exists at `.agents/cli/target/release/gritt-agent`.
2. The workspace is opened at the repository root.
3. The MCP configuration contains `gritt-local-memory`.
4. The MCP client has reloaded the workspace configuration.

## Security and privacy

- The only mode is local.
- The database is ignored and should not be committed.
- The CLI reads no API keys and stores none in the database.
- Never place credentials in MCP JSON or documentation.
- Do not index confidential material unless the team has agreed that it
  belongs in the local developer index.

## Related documentation

| Topic                               | Document          |
| ----------------------------------- | ----------------- |
| Storage and component relationships | `architecture.md` |
| Required and optional capabilities  | `capabilities.md` |
| Provider and network policy         | `providers.md`    |
| Processes and persistence           | `services.md`     |
| Commands and MCP tools              | `tools.md`        |
| The CLI crate                       | `../cli/README.md` |

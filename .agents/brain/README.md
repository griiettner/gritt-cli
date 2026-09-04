---
id: brain-index
title: gritt-cli repository agent brain
status: active
date: 2026-08-14
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
- Optional models improve retrieval later; they are not required for the
  baseline.

This document is written for both human developers and AI agents. It explains
what the brain is, how it operates, how to use it, and why it exists.

## Why we use it

Large repositories contain useful context in architecture decisions, package
READMEs, tickets, agent instructions, source files, and configuration. Asking
every agent to read everything is slow, expensive, and unreliable.

The brain provides:

1. A local searchable copy of relevant workspace knowledge.
2. A standard MCP interface for Cursor and Claude Code.
3. Source-aware results with file paths and line ranges.
4. A dashboard showing whether memory is being populated.
5. A clean fallback when optional AI services are unavailable.

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
Local indexer
      |
      v
Turso/libSQL-compatible SQLite file
      |
      +--> document metadata
      +--> line-addressable chunks
      +--> SQLite FTS5 indexes
      +--> optional vector column
      |
      v
gritt-local-memory MCP
      |
      +--> Cursor
      +--> Claude Code
```

The implementation lives in:

```text
.agents/tools/agent-memory/
├── chunk.mjs       Document chunking
├── db.mjs          Local libSQL connection and schema initialization
├── dashboard.mjs   Local monitoring dashboard
├── index.mjs       Workspace indexer
├── schema.sql      Database schema
├── search.mjs      FTS5 retrieval
└── server.mjs      Stdio MCP server
```

Generated state is stored at:

```text
.agents/brain/data/agent-memory.db
```

`.agents/brain/data/` is ignored by Git. Each developer has an independent
local index. The database is not synchronized with Turso Cloud and does not
communicate with any remote service by default.

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
```

The index is rebuilt incrementally by path and content hash. Removed source
files are removed from the local index on the next run.

### Chunking

Documents are split into line-addressable chunks:

- Markdown headings begin logical sections.
- Large sections are split into bounded chunks.
- Non-Markdown documents are split by line count.
- Chunks retain heading, start-line, and end-line metadata.
- Chunk searches return citations such as:

```text
.agents/brain/README.md:67-82
```

The current offline retriever uses FTS5 over these chunks. It does not use
query expansion or a generative model.

## Quick start

From the repository root:

```bash
npm install
npm run agent-memory:index
npm run agent-memory:mcp
```

The index command is safe to run repeatedly.

To use optional configuration:

```text
.agents/.env-template → .agents/.env
```

The Node tools load `.agents/.env` automatically. Existing process
environment variables take precedence. No shell-specific `source` command
is required.

### Windows PowerShell

```powershell
Copy-Item .agents/.env-template .agents/.env
npm install
npm run agent-memory:index
npm run agent-memory:mcp
```

### macOS or Linux

```bash
cp .agents/.env-template .agents/.env
npm install
npm run agent-memory:index
npm run agent-memory:mcp
```

The same npm commands work on all three platforms.

## MCP integration

The MCP server is already configured in:

```text
.cursor/mcp.json
.mcp.json
```

Its name is:

```text
gritt-local-memory
```

The transport is stdio. It is started by Cursor or Claude Code when the
workspace MCP configuration is loaded; it is not a TCP server.

### Available MCP tools

| Tool                  | Purpose                                     |
| --------------------- | ------------------------------------------- |
| `search_local_memory` | Search indexed chunks with source citations |
| `read_local_memory`   | Read one indexed document by relative path  |

`search_local_memory` returns the matching document title, heading, path,
line range, and relevant chunk content. `read_local_memory` is useful after a
search identifies the authoritative file.

### Agent usage rule

Before inspecting code for a task:

1. Query `gritt-local-memory`.
2. Use the result paths to identify relevant package docs, decisions, and
   tickets.
3. Read the smallest required file set.
4. Only then search implementation code.

This avoids repeatedly rediscovering project knowledge from source files.

## Dashboard

The local dashboard shows how the brain is populated:

```text
http://127.0.0.1:8282
```

Start it directly with:

```bash
npm run agent-memory:dashboard
```

Starting the MCP server also starts the dashboard automatically if it is not
already running. The engine fixes this behavior and the loopback port at
`8282`; neither requires environment configuration.

The dashboard displays:

- Index size
- Indexed document count
- Chunk count
- Embedding coverage
- Collection-like top-level path groups
- Top file types
- Recently modified documents
- Recent index runs
- Local retrieval and network status

The dashboard is bound to `127.0.0.1`, so it is local to the developer
machine.

## Environment and providers

`.agents/.env` is optional and holds provider configuration only. Every
provider key ships commented out in `.env-template`, so the default is:

```text
No .agents/.env, or no provider key set
```

Each capability has one key whose value is the model identifier to use —
generation for `AGENT_AI_PROVIDER`, vectors for `AGENT_EMBEDDING_PROVIDER`,
candidate reordering for `AGENT_RERANK_PROVIDER`. A missing key, an empty
value, and `none` all resolve to off, so nothing has to be set to `none`.

The default means:

```text
Local libSQL + FTS5 + MCP
No embeddings
No reranking
No query expansion
No external requests
```

The schema reserves an `F32_BLOB(1536)` column for
`text-embedding-3-small`. The optional provider configuration
uses:

```env
AGENT_MEMORY_API_KEY=
AGENT_MEMORY_BASE_URL=https://openrouter.ai/api
AGENT_EMBEDDING_PROVIDER=text-embedding-3-small
AGENT_RERANK_PROVIDER=rerank-3.5
```

Those providers are configuration contracts for the next phase. The current
working retrieval path is FTS5; the embedding and reranking
adapters are not required by, or used by, the offline baseline.

When providers are added, the intended pipeline is:

```text
Original query
  → FTS/vector candidate retrieval
  → optional Cohere reranking
  → cited results
```

There will be no automatic query expansion.

## Database model

The local schema contains:

| Table                 | Purpose                             |
| --------------------- | ----------------------------------- |
| `documents`           | One row per indexed source file     |
| `document_chunks`     | Line-addressable content chunks     |
| `documents_fts`       | File-level FTS5 compatibility index |
| `document_chunks_fts` | Offline retrieval index             |
| `index_runs`          | Index execution history             |

The database is Turso/libSQL-compatible through `@libsql/client`, but local
development uses a `file:` URL. No auth token, `syncUrl`, or cloud database is
configured.

## Troubleshooting

### No results

Rebuild the index:

```bash
npm run agent-memory:index
```

Then retry the MCP search.

### Dashboard does not open

Start it directly:

```bash
npm run agent-memory:dashboard
```

The dashboard uses fixed loopback port `8282`. If that port is occupied, stop
the conflicting process before restarting the dashboard.

### Stale results

The MCP server indexes on startup. Restart the MCP process or run the index
command manually after changing source documents.

### Corrupt or incompatible local database

The database is generated state and can be recreated:

```text
Delete `.agents/brain/data/agent-memory.db`
Run npm run agent-memory:index
```

Do not delete committed source files or `.agents/memory/` files.

### MCP does not appear

Check that:

1. Dependencies are installed with `npm install`.
2. The workspace is opened at the repository root.
3. The MCP configuration contains `gritt-local-memory`.
4. `node` is available on `PATH`.
5. The MCP client has reloaded the workspace configuration.

## Security and privacy

- The default path is local-only.
- The database is ignored and should not be committed.
- API keys belong only in `.agents/.env` or the developer environment.
- Never place credentials in `.env-template`, MCP JSON, or documentation.
- External providers must be explicitly enabled before future adapters make
  requests.
- Do not index confidential material unless the team has agreed that it
  belongs in the local developer index.
- The dashboard listens only on loopback.

## Related documentation

| Topic                               | Document                                                        |
| ----------------------------------- | --------------------------------------------------------------- |
| Storage and component relationships | `architecture.md`                                               |
| Required and optional capabilities  | `capabilities.md`                                               |
| Provider and network policy         | `providers.md`                                                  |
| Processes and persistence           | `services.md`                                                   |
| Commands and MCP tools              | `tools.md`                                                      |
| Durable agent context boundaries    | `../memory/architecture/agent-context-boundaries.md`            |
| Local libSQL decision               | `../memory/decisions/ADR-013-local-agent-memory-with-libsql.md` |

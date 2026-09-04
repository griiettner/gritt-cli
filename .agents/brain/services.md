---
id: brain-services
title: Agent brain services
status: active
date: 2026-08-13
tags:
  - agent-brain
  - services
  - mcp
  - turso
read_when:
  - starting or diagnosing local agent services
  - changing MCP startup
  - changing local persistence
---

# Agent Brain Services

| Service                  | Default                | Purpose                                        |
| ------------------------ | ---------------------- | ---------------------------------------------- |
| Local SQLite file        | enabled                | Stores indexed documents and chunks            |
| Workspace indexer        | manual and MCP startup | Synchronizes supported files into the database |
| `gritt-local-memory` MCP | project-configured     | Exposes local search and document tools        |
| Local embeddings         | not implemented        | Reserved schema column only                    |

The database is a plain SQLite file opened by the bundled SQLite inside
`gritt-agent`. No cloud URL, authentication token, or remote endpoint is
configured, and the CLI makes no network requests.

Generated database state belongs in `.agents/brain/data/agent-memory.db` and
must not be committed. The root `.gitignore` excludes that directory.

The runtime is one Rust binary using stdio MCP transport. It does not depend on
Node, Bash, Homebrew, launchd, or Unix-specific path syntax, so the same setup
works on Windows, macOS, and Linux.

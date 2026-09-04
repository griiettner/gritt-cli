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

| Service                    | Default                | Purpose                                        |
| -------------------------- | ---------------------- | ---------------------------------------------- |
| Local libSQL file          | enabled                | Stores indexed documents and optional vectors  |
| Workspace indexer          | manual and MCP startup | Synchronizes supported files into the database |
| `gritt-local-memory` MCP | project-configured     | Exposes local search and document tools        |
| Local embeddings           | disabled               | Optional semantic retrieval                    |
| Ollama                     | disabled               | Optional generation                            |

The default services use `file:` database access and do not configure
`syncUrl`, cloud URLs, authentication tokens, or remote endpoints.

Generated database state belongs in `.agents/brain/data/agent-memory.db` and
must not be committed.

The runtime uses Node APIs, npm scripts, and stdio MCP transport only. It does
not depend on Bash, Homebrew, launchd, `/opt/homebrew`, or Unix-specific path
syntax, so the same setup works on Windows, macOS, and Linux.

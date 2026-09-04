---
id: ADR-005
title: Local Turso memory store
status: accepted
date: 2026-09-04
related_ticket: griiettner/TKT-0004
tags:
  - memory
  - turso
  - local-first
read_when:
  - changing the local memory database
  - adding memory synchronization or credentials
  - changing full-text retrieval
---

# ADR-005: Local Turso memory store

## Decision

The `gritt-agent` memory subsystem uses Turso 0.7.2 in local mode. The
database remains at `.agents/brain/data/agent-memory.db`, one independent
generated file per checkout. The Turso `sync` feature is disabled. The CLI
has no Cloud URL, token, remote endpoint, or memory-related network path.

Search uses Turso's Tantivy-backed FTS indexes. The external contract remains
unchanged: every normalized query term must match, results are ranked, and
the CLI and MCP server return the same source-aware citation format.

## Rationale

- The requested storage engine is Turso, but shared or hosted memory is not
  required.
- Local mode preserves ADR-004's single-binary, offline operation and needs
  no service or account.
- Turso does not implement SQLite FTS5 virtual tables. Its local FTS indexes
  provide the needed lexical retrieval without adding a provider.
- Keeping the existing database path avoids configuration and caller changes.

## Consequences

- `rusqlite` is removed. `turso` and `tokio` are runtime dependencies.
- Database calls are asynchronous inside the memory modules and command entry
  point.
- New Turso FTS indexes use distinct names from the legacy FTS5 virtual tables,
  so the existing generated database can be reindexed in place. If a damaged
  cache still fails to open, it can be deleted and rebuilt without losing
  canonical data.
- Turso FTS currently requires its experimental index-method switch. Tests
  cover indexing, search, and the MCP contract without network access.
- Any future Cloud sync proposal must update this ADR and explicitly design
  credentials, ownership, offline behavior, and access control first.

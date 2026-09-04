---
id: ADR-003
title: Ticket id format and chunked task folders
status: accepted
date: 2026-07-13
tags:
  - tasks
  - routing
read_when:
  - Creating, locating, or migrating a ticket folder
  - Changing ticket routing or index generation
---

# ADR-003: Ticket Id Format And Chunked Task Folders

Supersedes the ticket id format and folder layout in [ADR-001](ADR-001-agent-ticket-lifecycle.md). The lifecycle and artifact rules in ADR-001 are unchanged.

## Decision

Ticket ids are `TKT-NNNN` — four digits, zero-padded.

Ticket folders are grouped into chunk folders of 25, so a ticket lives at `.agents/tasks/TKT-SSSS-EEEE/TKT-NNNN/`. Resolve the chunk from the ticket number `N`:

```
start = ((N - 1) // 25) * 25 + 1
end   = start + 24
path  = .agents/tasks/TKT-{start:04d}-{end:04d}/TKT-{N:04d}/
```

Each chunk folder carries its own generated `index.yaml` shard. The top-level `.agents/tasks/index.yaml` is a generated router that lists the chunks and points at each shard.

## Rationale

- Four digits leave headroom past 999 tickets without a second renumbering.
- A flat task directory grows unbounded and forces an agent to scan every ticket folder to find one.
- Chunking bounds the routing cost: the agent reads the router, picks one chunk, and reads only that shard.
- Keeping each shard next to the tickets it describes means the shard moves with its chunk and cannot drift into a separate `indexes/` directory.

## Consequences

- Ticket folder contents remain source of truth; both index tiers are regenerable.
- `python3 .agents/tools/tkt_sync.py` creates chunk folders and regenerates both tiers. Never hand-edit an `index.yaml`.
- `python3 .agents/tools/tkt_validate.py .agents/tasks` enforces `TKT-NNNN` ids and chunk folder names.
- Agents must resolve the chunk before opening a ticket; a bare ticket id is not a path.
- Legacy flat paths (`.agents/tasks/TKT-NNN/`) are migration-only references. Do not create new ticket context outside a chunk folder.

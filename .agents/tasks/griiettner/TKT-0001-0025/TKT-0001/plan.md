---
id: TKT-0001
namespace: griiettner
title: Build project-local agent CLI
artifact: plan
status: ready
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0001 plan: Build project-local agent CLI

## Decisions

- Implement an independent Rust crate at `.agents/cli/` with binary name `gritt-agent`.
- Keep the crate outside the future product workspace by declaring its own workspace root.
- Store the SQLite database at `.agents/brain/data/agent-memory.db`.
- Use SQLite FTS5 for baseline retrieval. Do not add embeddings, reranking, or remote providers.
- Expose MCP over standard input and output from `gritt-agent memory serve`.
- Preserve current ticket paths, frontmatter, allocation rules, and generated index formats.
- Use registry dependencies only. Check current versions, licenses, maintenance, and platform support before adding them.
- Keep the old scripts until each replacement command passes equivalent tests.

## Command surface

```text
gritt-agent memory index
gritt-agent memory search <query>
gritt-agent memory serve
gritt-agent ticket new --title <title>
gritt-agent ticket sync
gritt-agent ticket validate
gritt-agent skill sync
```

## Sequence

1. Create the independent crate, command parser, repository-root discovery, and tests.
2. Implement SQLite schema initialization, deterministic indexing, FTS5 search, and MCP tools.
3. Port ticket allocation, synchronization, and validation without changing artifact formats.
4. Port canonical skill synchronization and generated Claude Code and Codex metadata behavior.
5. Run parity tests against temporary repository fixtures.
6. Update agent instructions and MCP configuration to invoke `gritt-agent`.
7. Remove replaced Node scripts and stale Node-specific documentation after parity passes.

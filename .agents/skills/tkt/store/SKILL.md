---
name: tkt-store
description: Resolves ticket namespaces, ids, and chunk paths. Use when locating an existing ticket folder or allocating a new id.
---

# Ticket store

Read [tkt](../SKILL.md) first.

Ticket ids are `TKT-NNNN` (four digits, zero-padded, for example `TKT-0019`). They are unique inside a namespace, not across the whole tree.

## Namespaces

- New tickets belong to the current developer's GitHub login, for example `griiettner`.
- Path: `.agents/tasks/<github-login>/TKT-SSSS-EEEE/TKT-NNNN/`.
- Qualified id: `<github-login>/TKT-NNNN`.
- Tickets already at `.agents/tasks/TKT-SSSS-EEEE/TKT-NNNN/` are the `_shared` namespace. Leave them there.

A bare `TKT-NNNN` prefers the current identity namespace, then a unique match across namespaces. When both exist, use the qualified id.

## Allocation

Do not pick or skip ids manually. Create tickets with:

```bash
node .agents/tools/agent-tools/tkt-new.mjs --title "Short title"
```

Use `agent-tools:tkt-new-chain` for chain-managed work. The tool resolves GitHub identity, allocates the next number in that login's contiguous sequence, and writes the folder. If an earlier id is missing, allocation fails until the gap is restored or explicitly accounted for.

Identity resolution order: `--namespace`, `GRITT_TKT_NAMESPACE`, `.agents/state/identity.local.yaml`, then `gh api user --jq .login`. Persist it with `node .agents/tools/agent-tools/tkt-identity.mjs`.

## Chunk resolution

From ticket number `N`:

```
start = ((N - 1) // 25) * 25 + 1
end   = start + 24
path  = .agents/tasks/<namespace>/TKT-{start:04d}-{end:04d}/TKT-{N:04d}/
```

Shared tickets omit `<namespace>/`. So `griiettner/TKT-0001` lives at `.agents/tasks/griiettner/TKT-0001-0025/TKT-0001/`, and `_shared/TKT-0019` lives at `.agents/tasks/TKT-0001-0025/TKT-0019/`.

## Indexes

Each chunk folder carries its own shard index. The top-level `.agents/tasks/index.yaml` lists namespaces and chunks. Neither index is canonical. Both regenerate through `agent-tools:tkt-sync`.

## Legacy paths

Flat task paths with old three-digit ids (`.agents/tasks/TKT-NNN/`) are migration-only. Do not create new ticket context outside a chunk folder.

---
id: TKT-0002
namespace: griiettner
title: Finish migrating agent-tools to gritt-agent
artifact: concept
status: concept
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0002 Concept: Finish migrating agent-tools to gritt-agent

## Problem

TKT-0001 built the `gritt-agent` Rust CLI and moved local memory, ticket
sync/validate/new, and skill sync off Node. Its `task.md` explicitly put
"ticket chains, migration helpers, trust configuration, commit automation,
or skill creation" out of scope, so six Node scripts and their shared `lib/`
modules under `.agents/tools/agent-tools/` are still required:

- `create-skill.mjs`
- `tkt-identity.mjs`
- `tkt-new-chain.mjs`
- `tkt-chain-check.mjs`
- `trust-codex-project.mjs`
- `migrate-cursor-setup.mjs`

That scope line was wrong for the actual goal, which is for `gritt-agent` to
be the sole driver of every tool this repository needs. None of the six
scripts do anything Rust cannot: file writes, frontmatter rendering, a few
`git` and `gh` subprocess calls, and one hand-rolled TOML section editor.

## Intent

Port all six scripts into `gritt-agent`, prove parity against their current
behavior with real tests, then delete the Node originals and every shared
`lib/` module nothing else still imports. When this ticket closes,
`.agents/tools/agent-tools/` should not exist, and no skill or doc should
name a `.mjs` script under it.

## Success Criteria

- Every one of the six behaviors exists as a `gritt-agent` subcommand with
  unit and integration test coverage in `.agents/cli/`.
- `.agents/tools/agent-tools/` is deleted, including `lib/cli.mjs`,
  `lib/fs-utils.mjs`, `lib/tkt-store.mjs`, `lib/tkt-identity.mjs`,
  `frontmatter-utils.mjs`, and `agent-tools.test.mjs`, once nothing imports
  them.
- Every skill, `README.md`, and `.agents/settings.json` permission entry
  that named one of the six scripts now names the `gritt-agent` subcommand.
- `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` pass in
  `.agents/cli/` with the new commands included.

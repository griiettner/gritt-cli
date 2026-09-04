---
id: TKT-0010
namespace: griiettner
title: Implement provider adapters, streaming normalizers, model caching, capability checks, opt-in embeddings and reranking
artifact: task
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
dependencies:
  - TKT-0009
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0010 Task: Implement provider adapters, streaming normalizers, model caching, capability checks, opt-in embeddings and reranking

## Chain Role

Worker 2 of 5 in the TKT-0008 chain.
Start from a fresh worktree branched from the latest merged `feature/tkt-0008-gritt-cli` only after TKT-0009 merges and passes review.

Branch: `tkt-0010-02-providers`

## Goal

Implement the provider layer so configured endpoints stream reliable provider-neutral events and expose cached capabilities without coupling the harness to wire formats.

## Scope

- Implement OpenRouter and generic Chat Completions, OpenAI Responses and Chat Completions, and Anthropic Messages adapters.
- Add per-envelope normalizers, tool-schema generation, continuation state, internal error kinds, and capability reporting.
- Add daily model-list cache refresh with stale fallback and automatic alias remapping from provider replacements or configured mappings.
- Add opt-in embedding and reranking adapters selected by environment variables, with no network activity when unset.

## Out of Scope

- Do not implement terminal UI, permission evaluation, native tool execution, connector process supervision, release packaging, or non-provider session views. Those belong to TKT-0011 through TKT-0013.

## Acceptance Criteria

- All listed provider profiles stream the shared event model from recorded fixtures.
- Normalizers preserve tool calls, usage, continuation identifiers, errors, and capability metadata without provider branches above the adapter.
- Cache refresh occurs at most once per day, stale fallback is marked, and alias remapping is deterministic and tested.
- Embedding and reranking are disabled by default and use only the configured environment endpoints.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test -p gritt-provider` and the full workspace tests
- Replay provider fixtures for every normalizer and error path.
- `gritt-agent ticket chain-check --ticket TKT-0010 --base feature/tkt-0008-gritt-cli`
- Run `gritt-agent ticket chain-check --ticket TKT-0010 --base main` before semantic review.

## Handoff

Report branch name, PR link, validation output, and unresolved risks to the
PM, then stop. Do not start the next step.

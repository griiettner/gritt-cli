---
id: TKT-0013
namespace: griiettner
title: Complete cross-platform reproducible builds, diagnostics, documentation, end-to-end verification, and integrated hardening
artifact: update
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0013 update: Final review hardening

## Trigger

The TKT-0014 integrated review of `feature/tkt-0008-gritt-cli` at `3f3db4d`
returned `needs-fix` with five Medium and three Low findings. This ticket's
scope covers integrated hardening, so the fixes landed here on branch
`tkt-0013-06-hardening`.

## Changed files and behavior

- `crates/gritt-harness/src/store/mod.rs`: each migration's statements and
  its ledger row now commit in one transaction, with rollback on failure.
  A column that an interrupted run already added is detected through
  `PRAGMA table_info` and the `ALTER TABLE` skipped, so the ledger can
  catch up instead of failing on a duplicate column. Comment lines are
  stripped before statements are split, because the content-log migration
  has a semicolon inside a comment.
- `crates/gritt-harness/src/native_connector.rs`: the native connector's
  event channel is unbounded, so a slow consumer never loses an event, an
  approval request in particular. A failed send now means only that the
  receiver is gone.
- `crates/gritt-harness/src/agent.rs` and `crates/gritt/src/main.rs`:
  `AgentBuilder::session_profile` resolves the profile a session will run
  on (a resumed session's own profile, or the qualified name, alias, hint,
  or default) and the binary warms that profile's catalog before opening.
- `Cargo.toml` and the crate manifests: chrono's `clock` feature moved out
  of the workspace default. `gritt-core` keeps `std` and `serde` only; the
  four I/O crates enable `clock`. `cargo tree -p gritt-core` no longer
  shows `iana-time-zone`.
- `.github/workflows/agent-cli.yml`: installs the toolchain pinned in
  `rust-toolchain.toml` instead of a floating `stable`, so both workflows
  test the same compiler. What the workflow tests is unchanged.
- Worker reports TKT-0009 to TKT-0013 gained a `## Reviewer Verdict`
  section recording the initial verdict, fix commits, final pass, and merge
  commit. This report gained the benchmark ledger and dropped the resolved
  stale-answer item from Follow-up.

## Fixes and edge cases

The first transactional migration attempt split statements on semicolons
before removing comments and broke every end-to-end test with a syntax
error from the content-log comment. Stripping comment lines first fixed it.

## Validation

All from the worktree root on 2026-09-04:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: pass, 201 tests. New: half-applied column
  migration recovery, failed migration leaves no ledger row, alter parsing,
  lossless delivery of 400 events to a slow consumer, and profile
  resolution for an alias, a qualified name, the default, and a resumed
  session on a non-default profile.
- `cargo tree -p gritt-core | grep -c iana-time-zone`: 0.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: pass, 97 tests.
- `gritt-agent ticket validate`: see the PR.
- `gritt-agent ticket chain-check --ticket TKT-0013 --base feature/tkt-0008-gritt-cli`: see the PR.

## Remaining follow-up

Unchanged from the report: Windows paths untested until the CI matrix
runs, live provider HTTPS tests need a key, hand-authored provider
fixtures, and the manual TUI checks named in Follow-up.

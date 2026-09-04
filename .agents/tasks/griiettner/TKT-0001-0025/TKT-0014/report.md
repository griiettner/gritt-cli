---
id: TKT-0014
namespace: griiettner
title: Review integrated Build the complete Gritt local AI coding agent CLI chain
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: reviewer
chain_parent: TKT-0008
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0014 Report: Review integrated Build the complete Gritt local AI coding agent CLI chain

## Summary

Final typed verdict: pass. The integrated branch `feature/tkt-0008-gritt-cli`
at bde4625 satisfies the TKT-0008 contract. A first integrated pass at
3f3db4d returned needs-fix with five Medium and three Low findings; PR #6
(TKT-0013 hardening) resolved them and the second pass at bde4625 returned
pass with three Low follow-ups. The reviewer was Codex `gpt-5.6-sol` at
medium effort in a read-only sandbox, the documented fallback after
`gpt-6-astra` was rejected by the account. The PM reran every deterministic
check in a writable checkout.

## Key Decisions

- The eight first-pass findings were routed to a hardening worker on a fresh
  branch and PR under TKT-0013's integrated-hardening scope rather than
  patched on the integration branch.
- Low findings from the second pass are follow-ups, not blockers. Two of
  them were plain text fixes and were applied in the PM's closing commit:
  the obsolete stale-answer bullet in TKT-0011's report and the `ring`
  comment in the root `Cargo.toml`.

## Validation

Run by the PM on bde4625 in `../gritt-cli-tkt-0014`:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: 201 passed, 0 failed.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: 97 passed.
- `cargo tree -p gritt-core`: no `iana-time-zone`, no I/O crates.
- `gritt-agent ticket validate`: ok, 0 warnings.
- `gritt-agent ticket chain-check --ticket TKT-0014 --base main` and
  `--ticket TKT-0008 --base main`: ok. The warnings list the merged worker
  ticket folders, which the integrated diff legitimately contains.
- `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`:
  3 passed (Codex smoke, Codex resume, Claude Code smoke) in 12.9 s.

## Acceptance Evidence

Parent criteria from TKT-0008:

1. Workspace builds and tests on macOS, Windows, and Linux with MIT files and
   reproducible build instructions: partial on this machine, met by design.
   macOS aarch64 was rebuilt twice with identical checksums; Linux and
   Windows run in `.github/workflows/product.yml` and local cross checks
   fail only for lack of a cross C compiler.
2. Native OpenRouter, OpenAI Responses and Chat Completions, Anthropic
   Messages, and generic profiles stream one event model: met,
   `crates/gritt-provider/tests/contract.rs`.
3. Daily model refresh, visible stale fallback, capability enforcement, alias
   remapping: met, `crates/gritt-provider/tests/models_cache.rs` and
   `crates/gritt-provider/src/alias.rs`.
4. Planning and coding phases share named resumable sessions; tools are
   workspace-bounded and policy-gated: met under the recorded shell-authority
   exception, `crates/gritt-harness/tests/native_session.rs`.
5. Print, REPL, and Ratatui modes with streaming, approvals, cancellation,
   and diff review: met, `crates/gritt/tests/tui_pty.rs` and
   `crates/gritt/tests/e2e.rs`.
6. Native, Codex, Claude Code, Cursor, and OpenCode connectors preserve
   external authority, normalize events, supervise processes, and have live
   or fixture coverage: met, `crates/gritt-connector/tests/connectors.rs`
   and the live results above. Cursor is fixture-only because its CLI is not
   installed here.
7. One embedded database with separate namespaces and no secrets in config,
   logs, fixtures, transcripts, or telemetry: met,
   `crates/gritt-harness/src/store/product_schema.sql` and the coexistence
   test in `crates/gritt-harness/src/store/mod.rs`.
8. Every worker PR and the master PR reviewed, merged, and recorded: met for
   PRs #1 to #6 with typed verdicts in each worker report; the master PR #7
   merge is recorded in TKT-0008's report.

Child criteria: TKT-0009 to TKT-0013 all met; each worker report carries its
own evidence and a Reviewer Verdict section.

## Findings

First pass at 3f3db4d, all resolved by PR #6: non-atomic migrations, lossy
native connector channel, catalog warming on the wrong profile, chrono
`clock` in `gritt-core`, missing typed worker verdicts, missing test
duration, a stale follow-up, and the toolchain pin silently overriding the
`agent-cli` workflow.

Second pass at bde4625, Low only:

- `gritt key-set` echoes the key when stdin is a terminal. Follow-up for a
  no-echo prompt (originating in TKT-0009).
- Obsolete stale-answer bullet in TKT-0011's report. Fixed in the closing
  commit.
- `Cargo.toml` comment called `ring` pure Rust. Fixed in the closing commit.

## Completion Gate

- Acceptance: yes, with the platform evidence split recorded in criterion 1.
- Scope: yes. The review touched no code; the closing commit changed only
  ticket artifacts and one comment.
- Validation: yes, as listed.
- Security and safety: key redaction is enforced in errors, events,
  continuation state, telemetry, connector output, diagnostics, and config
  parse errors; the policy engine gates every native tool; connectors keep
  their own authority; no cloud path exists.
- Regression risk: low. `gritt-agent` is excluded from the product workspace
  and its 97 tests pass; the shared database gains only `gritt_` objects.
- Follow-up: below.
- Assumptions: recorded PM rulings (no OS shell sandbox, permissive
  unreported capabilities with a diagnostic, external agents keep their
  environment, key-redacted tool content in session events) were treated as
  accepted decisions.

## Follow-up

- No-echo `gritt key-set` prompt (TKT-0009).
- Live provider HTTPS tests and recorded fixtures once a key exists
  (TKT-0010).
- Windows shell, `taskkill`, and PTY paths from the first `product.yml` run;
  REPL arrow-key editing; manual TUI diff scrolling, Ctrl-J editing, and
  session-list resume (TKT-0011, TKT-0013).
- Claude Code approval relay, Cursor recordings, live Claude Code and
  OpenCode resume checks (TKT-0012).
- First successful Linux and Windows reproducibility jobs (TKT-0013).

## Updates

- None.

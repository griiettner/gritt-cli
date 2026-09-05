---
id: TKT-0021
namespace: griiettner
title: Review integrated OpenCode-inspired agent TUI and MCP harness chain
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-05
chain_role: reviewer
chain_parent: TKT-0015
dependencies:
  - TKT-0016
  - TKT-0017
  - TKT-0018
  - TKT-0019
  - TKT-0020
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
  - docs
  - .agents/plans
skills:
  - tkt
  - tkt-exec-chain
  - dev-harness
  - dev-provider
  - codebase-design
  - tdd
  - write-plan
---

# TKT-0021 Report: Review integrated OpenCode-inspired agent TUI and MCP harness chain

## Summary

Final typed verdict: pass, at `main` 9357da5. The first integrated pass at
421a8b7 returned needs-fix on one Medium finding: the startup profile lookup
could finish after a successful provider setup and overwrite the reloaded
profile list, which removed the new provider from `/connect` and disabled
explicit `/effort` choices. That fix landed as PR #13 on its own branch,
passed its own review, and merged as 9357da5. The second integrated pass
found nothing else blocking. The only item still open for the parent is the
human real-terminal walkthrough, which no agent in the chain can perform.

The reviewer was Codex `gpt-6-astra` in a read-only sandbox for every
per-PR round and both integrated passes. The PM reran every deterministic
check in a writable checkout before each verdict was acted on.

## Key Decisions

- The integrated finding was routed to a fresh worker branch and PR under
  TKT-0020's scope instead of being patched on `main`, matching the TKT-0014
  precedent from the previous chain.
- The two explained responsiveness misses recorded by TKT-0020 (p95 input to
  frame under 1,000 deltas per second at 62.8 to 67.1 ms against 50 ms, and
  no five-minute memory plateau) were accepted under the parent's "meets or
  explains" wording. Both share one named cause: the whole transcript is
  rebuilt per frame and history is not paged. They stay a follow-up, not a
  blocker.
- The human walkthrough is recorded as an external completion dependency of
  the parent, not as a worker defect.

## Validation

Run by the PM on `main` at 9357da5 in the primary checkout:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace --no-fail-fast`: 499 passed, 0 failed.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: 107 passed, 0 failed.
- `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`:
  3 passed (Codex smoke, Codex resume, Claude Code smoke).
- `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke`:
  1 passed; the single `.mcp.json` entry `gritt` reports ready, protocol
  2025-06-18, 3 tools. The check fails with a database lock error whenever
  another `gritt-agent mcp serve` process runs in the same checkout, which
  the Codex reviewer spawns for its own session; every such failure was
  rerun clean once the lock cleared.
- `GRITT_BENCH=1 cargo test --release -p gritt-harness --test tui_load` and
  `-p gritt --test tui_bench`: pass.
- `gritt-agent ticket validate`: ok, 0 warnings.
- `gritt-agent ticket chain-check --ticket TKT-0021 --base main` and
  `--ticket TKT-0015 --base main`: ok. The warnings name the missing closing
  reports and the fact that the final pass runs on `main`; both are expected
  for the reviewer and orchestrator tickets.
- The committed root `gritt` binary is unchanged at 32,278,416 bytes.

## Acceptance Evidence

Parent criteria from TKT-0015, as assessed in the second integrated pass:

| Criterion | Status | Evidence |
| --- | --- | --- |
| Create and resume sessions with explicit provider, model, and effort | met | TKT-0016 draft contracts and `crates/gritt-harness/tests/session_draft.rs`; TKT-0019 `tests/tui_integration.rs`; PR #13 generation guard |
| `/connect`, `/models`, `/effort`, `/mcp`, `/sidebar`, and session commands through shared reducers | met | `crates/gritt-harness/src/tui/command.rs` registry and `app.rs` reducers; parity test in `src/tui/app/tests.rs` |
| Every configured MCP server has visible lifecycle state | met | generic `.mcp.json` parser and runtime under `crates/gritt-harness/src/mcp/`; the sole workspace entry reports ready with 3 tools |
| Approved native MCP tools execute through the policy engine | met | `crates/gritt-harness/tests/mcp_native_session.rs` and `tests/tui_integration.rs` approved, denied, and declined cases |
| Sidebar reports known session and workspace state | met | usage, cost, and generation-rejection tests in `src/tui/app/tests.rs`; unknown values render as unavailable |
| Responsiveness evidence meets or explains the budgets | met | TKT-0020 report, Benchmarks section, with the two misses and their cause named |
| All worker PRs merged | met | PRs #8 through #13 in first-parent history of `main` |
| Final reviewer returns pass | met | this report |
| Every `.mcp.json` entry accounted for | met | one entry, `gritt`, live smoke ready |
| OpenCode/Crush flows pass focused visual and interaction checks | partial | TestBackend goldens, reducer tests, and PTY walkthroughs cover the required layouts and interactions; the human presentation checks are pending |
| Existing workspace tests remain green | met | 499 passed, 0 failed on 9357da5 |

Child tickets: TKT-0016, TKT-0017, TKT-0018, TKT-0019, and TKT-0020 are each
assessed met against their own acceptance criteria in the integrated pass;
the per-ticket evidence is in each `report.md` and its update file.

## Findings History

| Ticket | PR | Rounds before pass | Highest severity per round |
| --- | --- | --- | --- |
| TKT-0016 | #8 | 2 | Medium, Medium |
| TKT-0017 | #9 | 5 | High, High, High, High, High |
| TKT-0018 | #10 | 4 | High, Medium, Medium, Medium |
| TKT-0019 | #11 | 5 | High, High, Medium, Medium, Medium |
| TKT-0020 | #12 | 4 | High, High, High, Medium |
| TKT-0020 fix | #13 | 0 | none |
| TKT-0021 integrated | none | 1 | Medium |

Every round's findings, fixes, and validation are recorded in the worker
update files.

## Completion Gate

- Acceptance: yes for the review contract. Every parent and child criterion
  has evidence; the one parent criterion still partial is the human
  presentation check, recorded below.
- Scope: yes. The review edited no product files; the one fix went through
  its own branch and PR.
- Validation: yes, as listed above.
- Security and safety: no new finding. The MCP credential redaction, trust
  fingerprint, `mcp__*` ask default, masked setup input, and the five
  low-severity residuals recorded by TKT-0020 stand as recorded.
- Regression risk: low. Print and REPL retain their paths, pre-chain session
  rows still load with effort defaulting to `auto`, and `gritt-core` gained
  no I/O dependency.
- Follow-up: the human real-terminal walkthrough, then the follow-ups listed
  in the TKT-0020 report.
- Assumptions: none beyond those recorded in the worker reports.

## Follow-up

- A human runs the seven-item checklist in the TKT-0020 report's
  "Real-terminal walkthrough" section and records the result as a TKT-0015
  update file. The parent ticket moves to `done` after that.
- TKT-0020's follow-up list stays authoritative for performance, MCP
  deadline configuration, lazy server indexing, and the `artifact-dir`
  trap.

## Updates

- None.

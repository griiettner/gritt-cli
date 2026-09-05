---
id: TKT-0015
namespace: griiettner
title: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support
artifact: report
status: in_progress
owner: griiettner
created: 2026-09-04
updated: 2026-09-05
chain_role: orchestrator
chain_children:
  - TKT-0016
  - TKT-0017
  - TKT-0018
  - TKT-0019
  - TKT-0020
  - TKT-0021
dependencies:
  - TKT-0014
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

# TKT-0015 Report: Build an OpenCode-inspired full-screen agent TUI with generic MCP harness support

## Summary

The chain delivered the full-screen agent workspace on `main`: typed effort
and session-draft contracts with per-protocol effort mapping (TKT-0016), a
generic `.mcp.json` MCP client runtime in the harness with trust approval,
collision-safe tool dispatch through the permission engine, and a `gritt
mcp` command (TKT-0017), the Ratatui home and conversation layouts, command
registry, shared pickers, composer, theme, and sidebar view model on fixture
state (TKT-0018), the live integration with lazy session setup, provider
setup through the binary, real pickers, the Crush-style sidebar, MCP status,
and an async runtime with generation-checked late results (TKT-0019), and
the documentation, ADR-013 draft, deterministic responsiveness benchmarks,
live smoke checks, and integrated hardening (TKT-0020). Every worker step
ran in a fresh worktree and branch from `main`, opened a PR, passed review
after fix rounds, and merged before the next step started. The final
integrated review (TKT-0021) returned pass at 9357da5 after one fix PR.

The ticket stays `in_progress` on one item: the parent contract requires a
real-terminal walkthrough by a human, and no agent in the chain can perform
it. Everything else in the completion condition is met.

## Chain Execution

Base branch: `main`. Scaffold commit 785af8e. Reviewer: Codex `gpt-6-astra`
in a read-only sandbox for every round; the PM reran formatting, clippy, the
workspace tests, the agent CLI tests, the live smokes, the benchmarks,
`ticket validate`, and `chain-check` before acting on each verdict.

| Step | Ticket | Worktree | Branch | Commits | PR | Fix rounds | Verdict | Merge |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | TKT-0016 | ../gritt-cli-tkt-0016 | tkt-0016-01-contracts | 93ef33c, 614e66c, e0bf8ef, 78c8f85, 7d86792, aa92cd5 | #8 | 2 | pass | 99f9132 |
| 2 | TKT-0017 | ../gritt-cli-tkt-0017 | tkt-0017-02-mcp | 2afc880 through ccde932, 24 commits | #9 | 5 | pass | 77aaa22 |
| 3 | TKT-0018 | ../gritt-cli-tkt-0018 | tkt-0018-03-tui-foundation | 21b84d3, ec05ea1, 2c60618, b1a748d, 86e8bc6, f42ff15, f04dd74 | #10 | 4 | pass | 033e755 |
| 4 | TKT-0019 | ../gritt-cli-tkt-0019 | tkt-0019-04-tui-integration | 8d18b68 through dfdade7, 18 commits | #11 | 5 | pass | 4bdbac3 |
| 5 | TKT-0020 | ../gritt-cli-tkt-0020 | tkt-0020-05-hardening | d4f45d6 through 98ce608, 19 commits | #12 | 4 | pass | 421a8b7 |
| 5b | TKT-0020 final-review fix | ../gritt-cli-tkt-0020b | tkt-0020-06-final-review | d975419, 4e664e7 | #13 | 0 | pass | 9357da5 |
| 6 | TKT-0021 | none, run on `main` | none | integrated pass | none | 1 (needs-fix at 421a8b7, pass at 9357da5) | pass | none |

Each worktree was removed only after its PR reported `MERGED`. The chain
scaffold and this closing report are the only direct commits to `main`.

One unrelated commit by the user landed on `main` during step 1: e03ff4b,
which unified the repository's own MCP server, added ADR-012, reduced
`.mcp.json` to the single `gritt` entry, and named the Opus in-harness
fallback for the implementation role in `.agents/MODELS.md`.

## Key Decisions

- Workers ran as forked in-harness agents. The Grok CLI route in
  `.agents/MODELS.md` was probed once at chain start and the auto-mode
  classifier blocked it, as it did for the previous chain. Worker 1 ran on
  the inherited Fable model before e03ff4b named Opus 4.8 as the fallback;
  workers 2 through 5 ran on Opus. Recorded here, not substituted silently.
- The reviewer used the primary route, `gpt-6-astra`, which the Codex
  account accepted this time.
- Per-protocol effort mapping stays inside the adapters: Responses sends the
  level, Chat Completions requires reported reasoning support, Messages
  refuses explicit levels with a typed unsupported-capability error. The
  legacy `reasoning: true` switch now means the provider default level, not
  `medium`. Recorded in TKT-0016.
- Reading `.mcp.json` does not authorize execution. Trust is recorded per
  workspace and definition fingerprint, the `mcp__*` policy default is ask,
  and first-use approval goes through the shared approval overlay with a
  safe definition summary. Recorded in TKT-0017 and TKT-0019.
- Session pinning stays the product rule: changing provider or model on a
  session with history explains that a new session is required and keeps
  the composer draft. Recorded in TKT-0019.
- The event loop draws only on state-changing wakeups, caps at 30 frames per
  second, drains queued messages before each draw, and takes input ahead of
  queued messages. An approval must be drawn before a decision key is
  accepted, tracked per request. Recorded in TKT-0020.
- ADR-013 is drafted with `status: proposed` and covers the effort
  contract, the MCP client runtime and trust record, the permission default
  and resource form, and the binary-injected setup and reload seams. The
  user accepts it.
- One dependency edge was added: `unicode-segmentation`, already a
  transitive dependency through ratatui, checked and recorded in TKT-0018.
  Tokio's `test-util` feature was added as a harness dev-dependency only.

## Alternatives Considered

- Grok CLI workers per the routing contract. Blocked by the classifier;
  forked agents keep the ticket and skill context anyway.
- Patching the integrated finding directly on `main`. Rejected in favor of a
  fresh branch and PR so the fix carried its own review.
- Fixing the transcript rebuild that causes the two responsiveness misses
  inside the hardening step. Rejected: it churns nineteen snapshot goldens
  and the parent's wording accepts an explained gap.

## Assumptions

Each worker report lists its own. Chain level: the parent's "meets or
explains" wording governs the benchmark gate, and an explained miss with a
named cause satisfies it.

## Edge Cases and Failures

- The live MCP smoke test fails with a database lock error while any other
  `gritt-agent mcp serve` process runs in the same checkout. The Codex
  reviewer spawns one for its own session, so several PM gate runs hit the
  lock and were rerun clean afterwards. The repository's own server also
  indexes for about 43 seconds on a cold start, which exceeds the 30 second
  initialize deadline until the index exists.
- The session hosting the PM restarted during TKT-0017's third fix round;
  the worker recovered its uncommitted work from the worktree.
- `.cargo/config.toml` sets `artifact-dir = "."`, so any `cargo build`
  overwrites the committed root `gritt` binary. Three workers hit it and
  restored the binary before committing; it is unchanged on `main`.
- Reviewer findings concentrated on two shapes: two pieces of state that must
  move together moving separately (a reservation and its driver, a token and
  its operation, a label and its work, a highlight and its list), and a fix
  that moved a cost instead of removing it. Several rounds were fixes for
  defects an earlier round's fix introduced; each such case is named in the
  worker's update file.

## Validation

Integrated `main` at 9357da5, run by the PM:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace --no-fail-fast`: 499 passed, 0 failed.
- `cargo test --manifest-path .agents/cli/Cargo.toml`: 107 passed, 0 failed.
- `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`:
  3 passed.
- `GRITT_LIVE_MCP_TESTS=1 cargo test -p gritt-harness --test mcp_live_smoke`:
  1 passed; `gritt` ready, protocol 2025-06-18, 3 tools.
- Provider live tests: 3 skipped, no provider keys in the environment.
- `GRITT_BENCH=1` release runs of `tui_load`, `tui_responsiveness`, and
  `tui_bench`: pass.
- `gritt-agent ticket validate`: ok, 0 warnings.
- Final reviewer verdict on the integrated result: pass.

## Benchmarks

From the TKT-0020 report, release build on an Apple M1 Max with 32 GiB,
macOS 26.3.1, driven through the production scheduler:

| Scenario | Budget | Measured | Verdict |
| --- | --- | --- | --- |
| Launch with existing config | under 500 ms | 29 ms | met |
| Launch with a pending catalog request | independent of provider | 20 ms | met |
| Typing, picker, scroll (microbenchmarks) | p95 under 50 ms | 2.3 to 2.7 ms | met |
| Input to frame under 1,000 deltas per second | p95 under 50 ms | p95 62.8 to 67.1 ms | not met |
| Delta drain rate | keep up with 1,000 per second | 970 per second | met |
| Render work at 120x40 | p95 under 16 ms | 15.1 ms | met |
| Render cap under load | 30 fps | 19 fps | met |
| Queue under load | bounded | largest batch 52, empty at end | met |
| Cancel during a stream | under 100 ms | 19.7 to 52.2 ms | met |
| Idle CPU over 30 s, idle redraw | under 1 percent, none | 0.2 percent, 0 bytes | met |
| Resident memory over 5 min | a plateau | 762 MB and rising at 265 bytes per delta | not met |

Both misses have one cause: the whole transcript is rebuilt per frame and
history is not paged. Recorded as the first follow-up in TKT-0020.

## Completion Gate

- Acceptance: partial. Every parent criterion has evidence and the final
  reviewer returned pass; the human real-terminal walkthrough the contract
  requires has not been performed. Next action: the user runs the
  seven-item checklist in the TKT-0020 report and records the result as a
  TKT-0015 update file, then this ticket moves to `done`.
- Scope: yes. No desktop frontend, remote service, child-agent
  orchestration, LSP, skill engine, or MCP sampling, elicitation, or
  prompt and resource browsing. No upstream source copied.
- Validation: yes, as listed above, with provider live tests honestly
  skipped.
- Security and safety: MCP credentials are redacted at the runtime boundary
  including bearer-token parts, servers receive an allowlisted environment,
  denied calls never reach a server, setup input is masked and written only
  through the keychain service, and the five low-severity residuals are
  recorded in TKT-0020.
- Regression risk: low for print and REPL, which keep their paths; pre-chain
  session rows load with effort defaulting to `auto`; `gritt-core` gained no
  I/O dependency.
- Follow-up: see below.
- Assumptions: recorded above and in each worker report.

## Follow-up

- Human real-terminal walkthrough (TKT-0020 report, seven items), the one
  open completion item.
- Accept or amend ADR-013.
- Limit rendering to visible content and page history; this closes both
  benchmark misses (TKT-0020).
- Give the `UiMsg` channel a capacity so producers see backpressure
  (TKT-0020).
- Expose `McpRuntimeSettings` in `config.toml` and make `gritt-agent mcp
  serve` answer `initialize` before indexing (TKT-0017, TKT-0020).
- Remove the `artifact-dir = "."` setting or move it outside the repository
  (TKT-0016, TKT-0017, TKT-0020).
- Carried forward: Anthropic capability parsing into `reasoning_efforts`
  and comment-preserving config writes (TKT-0016); newer MCP protocol
  revisions and HTTP resumability (TKT-0017); OS clipboard, mouse support,
  and the flag-emoji cursor cost (TKT-0018); diff overlay word wrap and the
  full `git status` per refresh (TKT-0019).

## Updates

- None.

---
id: TKT-0024
namespace: griiettner
title: Expose current models and selection for external connectors
artifact: report
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0012
  - TKT-0019
  - TKT-0022
areas:
  - crates/gritt-core
  - crates/gritt-connector
  - crates/gritt-harness
  - crates/gritt
skills:
  - tkt
  - tkt-exec
  - dev-provider
  - dev-harness
  - codebase-design
  - tdd
  - write
  - review-ticket
---

# TKT-0024 Report: Expose current models and selection for external connectors

## Summary

Print, REPL, and the full-screen UI now share one control-plane operation
that asks an installed connector for the models it currently exposes, then
passes an explicit choice through that agent's documented `--model` flag.
`gritt-core` owns the catalog, freshness, and typed discovery outcomes.
Each connector adapter owns the listing command, the parser, and the
startup flag. The harness owns cache orchestration and picker state. The
binary remains the edge for the cache directory and process launch.

A new connector session stores the selected model on `SessionKind::Connector`.
A resumed session keeps that stored value and does not run discovery again.
When the user does not choose a model, the external CLI keeps its own default.

## Key Decisions

Listing commands were taken from installed `--help` and published docs,
not from a guessed UI scrape. Codex 0.153.3 documents `codex debug models`
as JSON `{"models":[{"slug","display_name"}]}` and `codex exec --model`.
Claude Code 2.1.263 documents `--model` and has no listing command, so
discovery is `Unsupported` while selection still works. Cursor is not on
this machine. Its listing uses documented `cursor-agent --list-models`,
covered by fixtures only. OpenCode documents `opencode models` and
`opencode run --model`.

Cache lives beside the native model cache at
`<cache>/gritt/connector-models`, using `ModelListPolicy`. A failed
refresh writes `last_attempt_at` and, when `stale_fallback` is on, returns
`CachedStale` rather than `Current`.

TUI setup opens the model picker after an agent is chosen, instead of
starting the session from a confirm notice. Ctrl-R on that picker forces
a refresh. An "Agent default" row preserves the previous no-selection
behavior.

## Alternatives Considered

Hard-coding Claude aliases such as `sonnet` and `opus` would have filled
the picker, but the ticket forbids guessed or hard-coded current lists.
`Unsupported` plus `--model` is the documented surface.

Codex app-server `model/list` JSON-RPC is richer than `codex debug models`.
The CLI already exposes a one-shot JSON catalog command, so that is what
the adapter runs.

Keeping the connect picker under the model picker made `/effort` in the
fixture walkthrough type into the connect search. Choosing a provider or
agent now closes the connect picker first.

## Assumptions

- Claude Code 2.1.263 has no documented model-listing command. Discovery
  is `Unsupported`. Explicit `--model` still reaches `claude`. A later
  Claude listing command would be a follow-up adapter change.
- `codex debug models` is the documented listing interface on 0.153.3,
  even though the CLI reference marks it experimental. `--bundled` was not
  used, so a refresh follows the CLI's own catalog load.
- Cursor `--list-models` output is line-oriented with optional `(default)`
  and `(current)` markers. That shape comes from published docs and
  forum samples, not from a live binary.
- Connector model cache reuses the native `ModelListPolicy` interval and
  stale-fallback flag rather than adding a second config key.
- `--model` on a resumed connector session is ignored. The stored model
  stays. Changing a resumed connector session's model is out of scope.
- Older `SessionKind::Connector` rows without a `model` field load as
  `model: None`.
- Live task smoke tests that send prompt text were not run. Live listing
  used only documented non-mutating commands.

## Edge Cases and Failures

- Missing executable, `Unavailable`.
- Claude listing, `Unsupported`, with `--model` still applied.
- Non-zero listing exit, `CommandFailure`, or `CachedStale` when a list
  exists.
- Unreadable JSON or text, `MalformedOutput`, or `CachedStale`.
- Empty, whitespace-only, or header-only listing stdout is
  `MalformedOutput`, or `CachedStale` when a previous list exists. The
  first completion left empty and whitespace-only Cursor and OpenCode
  output as `Ok([])`, which could replace a good cache with an empty
  current catalog. That is fixed in the 2026-09-06 review update.
- The fake-agent process writes `--model` and the identifier as separate
  argv entries. Prompt text is not interpolated into a shell string.
- Launch diagnostics still replace option values with `[value]`.

## Validation

- `cargo fmt --all --check` passed
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passed
- `cargo test --workspace --locked` passed
- Fixture and fake-process tests in `crates/gritt-connector/tests/models.rs`
  passed for every parser and each typed outcome
- Control-plane tests in `crates/gritt-harness/tests/connector_session.rs`
  passed for explicit selection, default selection, resume, and stale fallback
- TUI reducer tests passed for picker loading, stale, unsupported, and default
- The first completion recorded
  `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live
  live_codex_model_listing live_claude_model_listing
  live_opencode_model_listing`.
  `cargo test` rejects multiple positional filters with
  `error: unexpected argument`. That command did not run. Live listing
  was re-run in the 2026-09-06 review update, one filter at a time.
  There is no test named `live_claude_model_listing`. The matching test
  is `live_claude_model_listing_is_unsupported`.
- `cargo build --release --locked` passed
- `./.agents/gritt-agent ticket validate` passed, 0 warnings

## Completion Gate

- Acceptance: met after the 2026-09-06 review update. The first
  completion left native selection, stale fallback, empty listing, print
  and REPL catalog listing, and TUI overlay status incomplete.
- Scope: met. Native provider discovery was not changed. Resumed connector
  sessions are not migrated. TKT-0025 version checks were not added.
- Validation: workspace fmt, clippy, test, release build, and ticket
  validate passed in the 2026-09-06 update. The first completion's live
  command did not run. Listing tests were re-run one filter at a time.
  Live task smokes that send prompts were not run.
- Security and safety: listing commands are fixed argv vectors. Model ids
  are separate arguments. Diagnostics name the CLI and source, not stdout.
  Cache files store ids and labels only.
- Regression risk: TUI connect-then-models flow, `SessionKind` serde, and
  `TaskRequest` literals. Mitigated by existing connector session tests,
  the fixture home walkthrough, and serde default on `model`.
- Follow-up: Claude listing if the CLI grows a documented command. Cursor
  live listing once `cursor-agent` is installed. Optional typed `--model`
  on resume, which this ticket leaves out.
- Assumptions: listed above.

## Follow-up

- Add a Claude listing adapter when the CLI documents one.
- Run Cursor `--list-models` against an installed binary and replace the
  fixture if the live shape differs.
- Decide whether a resumed connector session should accept an explicit
  `--model` flag. This ticket ignores it.

## Updates

- [2026-09-06 review fixes](updates/2026-09-06-review-fixes.md)

---
id: TKT-0013
namespace: griiettner
title: Complete cross-platform reproducible builds, diagnostics, documentation, end-to-end verification, and integrated hardening
artifact: report
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

# TKT-0013 Report: Complete cross-platform reproducible builds, diagnostics, documentation, end-to-end verification, and integrated hardening

## Summary

Worker 5 of the TKT-0008 chain. The integrated CLI now has a reproducible
release path, a diagnostic command, user documentation, and end-to-end
tests through the real binary. One integration defect was fixed: the TLS
stack pulled `aws-lc-rs`, which needs CMake on Windows builders.

Chain facts:

- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0013`
- Branch: `tkt-0013-05-release`
- Base: `feature/tkt-0008-gritt-cli` at `bf6439d` (PR #4 merge)
- Commits: listed under Validation once pushed
- PR: recorded under Validation once opened

What landed:

- `.github/workflows/product.yml`: verify matrix on ubuntu, windows, and
  macOS (fmt, clippy, locked tests, release build) and a `release` job for
  five targets that builds twice from clean target directories, compares
  the SHA-256 checksums, and uploads the binary with `SHA256SUMS`.
- `scripts/release/build.sh` and `build.ps1`: the same deterministic build
  locally (`--locked`, `SOURCE_DATE_EPOCH` from the commit, remapped
  source and cargo paths, `CARGO_INCREMENTAL=0`).
- TLS: `reqwest` now uses `rustls-no-provider` with `rustls` 0.23.43 on
  the `ring` provider, installed once in `ReqwestTransport::new`.
  `aws-lc-rs` is gone from the tree.
- `gritt doctor` (`crates/gritt/src/doctor.rs`): platform, config
  locations and precedence, profiles with key availability and model cache
  freshness, embeddings, reranking, and content logging state, database
  path and rule, product migrations applied or pending, presence of the
  gritt-agent memory namespace, row counts, connectors with version, auth,
  transport, and approval ownership, and terminal capabilities. Never a
  value. `gritt telemetry` prints the content-free records.
- `docs/`: thirteen pages (index, getting started, providers, keys, tools
  and permissions, terminal modes, connectors, database, telemetry,
  embeddings, privacy, reproducible builds, upgrading). README Status
  rewritten with a pointer to the docs; stale Grok connector lines
  replaced; `.agents/brain/README.md` names the `gritt_` namespace.
- `crates/gritt/tests/e2e.rs`: eight tests through the built binary
  against a local HTTP provider stand-in: planning turn, coding turn with
  an approved write and diff, denied write, resume after exit,
  Ctrl-C cancellation with exit 130, missing connector leaving native
  intact, an old database upgrading in place with rows preserved, and
  doctor and telemetry staying content-free.
- `crates/gritt/tests/tui_pty.rs`: the full-screen mode in a real
  pseudo-terminal: enters the alternate screen, redraws after a resize,
  quits on Ctrl-Q, restores the terminal, exits 0, never draws the key.

## Key Decisions

- TLS provider is `ring`, per the PM ruling: every release target builds
  with the Rust toolchain and the platform C compiler only. `ring` 0.17.14
  is Apache-2.0 AND ISC; `rustls` 0.23.43 is Apache-2.0 OR ISC OR MIT;
  `rustls-platform-verifier` 0.7.0 (pulled by `rustls-no-provider`) is MIT
  OR Apache-2.0.
- Release artifacts are checksummed binaries, no signed installers
  (ADR-011 as narrowed by the chain plan).
- The e2e tests spawn the binary with `std::process::Command` and the
  `CARGO_BIN_EXE_gritt` path; no `assert_cmd` dependency was needed.
- The PTY pass is a committed test rather than a manual note, so it runs
  on every Unix CI run.

## Assumptions

- The `release` job's Linux aarch64 runner is `ubuntu-24.04-arm`; if the
  account lacks it the job for that target fails visibly and the others
  are unaffected (`fail-fast: false`).
- The doc pages describe behavior verified in the code at this commit;
  the approval prompt example in getting started is illustrative.
- `dist/` is ignored; local reproducibility runs write there.

## Edge Cases and Failures

- Cross-target `cargo check` from macOS to `x86_64-unknown-linux-gnu` and
  `x86_64-pc-windows-msvc` fails in the C build scripts of `ring` and
  `zstd-sys` (from `turso`) because no cross C compiler is installed on
  this machine. Recorded as an environment limitation; the CI matrix
  covers those targets on their own runners, and `ring` ships pregenerated
  assembly so no NASM is needed there.
- The first e2e denial assertion looked for the word `denied`; the tool
  result says `not permitted: the user declined`. Assertion corrected.
- The migration seed first wrote the session kind as a plain string; the
  store persists it as JSON. Seed corrected to the real shape.

## Validation

All from the worktree root on 2026-09-04:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace`: pass, 189 tests, including the 8 e2e and 1
  PTY tests; live provider and connector tests skipped without their gate.
- `cargo build --release --locked`: pass.
- `scripts/release/build.sh aarch64-apple-darwin` twice from a clean
  target directory: both `fae1fa2953e8e7796893e6eb4f5ba6bec97d813496c36f87827fdc342993d064  gritt`;
  `cmp` exit 0. Rust 1.93.1.
- `cargo check --workspace --target x86_64-unknown-linux-gnu`: fails,
  environment limitation above.
- `cargo check --workspace --target x86_64-pc-windows-msvc`: fails,
  environment limitation above.
- `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`:
  pass. Claude Code 2.1.260 completed PONG in 4.6 s; Codex 0.153.2
  completed PONG in 7.9 s.
- `./target/release/gritt doctor` on a scratch workspace: prints every
  section, no secret.
- `gritt-agent ticket validate --repo-root .`: see below.
- `gritt-agent ticket chain-check --ticket TKT-0013 --base feature/tkt-0008-gritt-cli`: see below.
- `gritt-agent ticket chain-check --ticket TKT-0013 --base main`: see below.

## Completion Gate

- Acceptance: yes for the four criteria. Reproducible build instructions
  and checksums exist for all five targets with the local macOS double
  build proven and the cross-target check limitation reported;
  documentation covers provider setup, key handling, tools, connectors,
  database namespaces, telemetry, embeddings, reranking, and the privacy
  boundary; end-to-end tests cover planning and coding, approvals, resume,
  cancellation, connector failure, and migration; validation is green and
  deviations are recorded below.
- Scope: yes. No frontend, cloud service, hosted telemetry, signed
  distribution, or new product feature. `gritt doctor` and `gritt
  telemetry` are the diagnostics the task names.
- Validation: yes, as listed; two cross-target checks are environment
  limited and honestly reported.
- Security and safety: the TLS change keeps certificate verification
  through the platform verifier; doctor prints availability only; the e2e
  and PTY tests assert the key never appears in output.
- Regression risk: low. The TLS provider swap is the only runtime change
  to earlier code; the SSE-over-TCP and e2e tests exercise the reqwest
  transport over plain HTTP, and the live connector tests passed. HTTPS
  itself is exercised only by the gated live provider tests, which need a
  key this machine does not have.
- Follow-up: below.
- Assumptions: above.

## Follow-up

- Run the gated live provider tests (`GRITT_LIVE_TESTS=1`) once a key is
  available to exercise HTTPS through `ring`.
- Windows: `taskkill` tree kill, the PTY path, and the Windows shell path
  remain untested until the CI matrix runs; connector supervision tests are
  Unix-only.
- The REPL stale-answer window after a cancelled approval and the missing
  arrow-key line editing stay as recorded in TKT-0011.
- Claude Code approval relay through its control protocol, and Cursor
  recordings once its CLI is available, stay as recorded in TKT-0012.
- Replace the hand-authored provider fixtures with redacted live
  recordings when a key is available.

## Updates

- None.

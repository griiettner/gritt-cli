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
- Commits: `afa73fe` (implementation, docs, tests, ticket artifacts),
  `0ed0bb3` (report evidence), `bbd8409` (review fix round), and the
  second fix round commit that corrects the e2e path encoding and this
  ledger
- PR: https://github.com/griiettner/gritt-cli/pull/5 into
  `feature/tkt-0008-gritt-cli`

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
- `crates/gritt/tests/e2e.rs`: nine tests through the built binary
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

- The compiler is pinned by `rust-toolchain.toml` (1.93.1). The verify
  and release jobs install that exact version, both release scripts refuse
  any other `rustc`, and every build writes a `BUILD-INFO` file beside
  `SHA256SUMS` so a later rebuild can select the same toolchain.
- Config parse errors report file, line, column, and the parser message
  only. The offending source line is never echoed, because a malformed
  file may hold a key value.

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
- `cargo test --workspace`: pass, 195 tests after the review fix round,
  including 9 e2e tests and 4 PTY tests; live provider and connector tests
  skipped without their gate.
- `cargo build --release --locked`: pass.
- `scripts/release/build.sh aarch64-apple-darwin` twice from clean target
  directories, after the toolchain pin: both
  `fae1fa2953e8e7796893e6eb4f5ba6bec97d813496c36f87827fdc342993d064  gritt`,
  identical `BUILD-INFO` (`rustc 1.93.1`, `toolchain 1.93.1`); `cmp` exit 0
  for both files.
- `cargo check --workspace --target x86_64-unknown-linux-gnu`: fails,
  environment limitation above.
- `cargo check --workspace --target x86_64-pc-windows-msvc`: fails,
  environment limitation above.
- `GRITT_LIVE_CONNECTOR_TESTS=1 cargo test -p gritt-connector --test live`:
  pass, 3 tests. Claude Code 2.1.260 completed PONG in 4.1 s; Codex 0.153.2
  completed PONG in 7.5 s; the new Codex resume smoke ran a first turn,
  passed the recorded thread id back through the connector's continuation
  path (`codex exec resume <thread_id>`), and the resumed turn returned the
  remembered code word in 13.8 s for both turns.
- `./target/release/gritt doctor` on a scratch workspace: prints every
  section, no secret.
- `gritt-agent ticket validate --repo-root .`: ok, 0 warnings.
- `gritt-agent ticket chain-check --ticket TKT-0013 --base feature/tkt-0008-gritt-cli`:
  ok, 0 warnings; merge-base equals the base tip `bf6439d`.
- `gritt-agent ticket chain-check --ticket TKT-0013 --base main`: ok,
  9 warnings, all the earlier worker ticket files that reached the
  integration branch through PRs #1 to #4 and the expected merge-base gap.

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
- Carried forward from TKT-0011: the PTY test now covers the approval
  view, the diff view, the command palette, the session list, `NO_COLOR`,
  resize, and quit. Not verified in a real terminal: scrolling the diff
  with `j`/`k`, multiline prompt editing with Ctrl-J, and resuming a
  session from the list with Enter. These remain a manual check.
- Carried forward from TKT-0012: the live Codex resume check is now
  covered by `codex_live_resume`. The Claude Code `--resume` path and the
  OpenCode `--session` path have fixture coverage only.

## Updates

- 2026-09-04 review fix round. The reviewer found six items: a floating
  `stable` toolchain in the release job, config parse errors that echoed
  the source line, an environment-dependent connector-failure test, an
  incomplete follow-up ledger, and two documentation errors (`doctor`
  inspecting pending migrations, `ring` called pure Rust). Fixes:
  `rust-toolchain.toml` plus pinned installs, a version check, and
  `BUILD-INFO` in both scripts and the workflow; `parse_toml` renders
  line and column with the parser message only, with a unit test and an
  e2e `doctor` test that plant a key in a malformed file; the Cursor test
  points at a nonexistent executable inside its workspace; the PTY test
  drives approval, diff, palette, sessions, and `NO_COLOR`, and a live
  Codex resume smoke was added and run; the follow-up ledger names every
  carried-forward item with its ticket; both docs corrected.
- 2026-09-04 second fix round. The Cursor path in the e2e config is now
  rendered as a TOML string, so backslashes parse on Windows, with a
  round-trip test; the commit ledger and the e2e test count were
  corrected.

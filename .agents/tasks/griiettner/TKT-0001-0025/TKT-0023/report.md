---
id: TKT-0023
namespace: griiettner
title: Expose reusable control plane API for T3Code
artifact: report
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
areas:
  - core
  - harness
  - cli
skills:
  - codebase-design
  - dev-harness
  - dev-cli
---

# TKT-0023 Report: Expose reusable control plane API for T3Code

## Summary

Investigated whether Gritt already has the reusable, non-terminal control-plane
seam ADR-011 calls for, and found that it does: `ControlPlane`, `AgentBuilder`,
`Driver`, and the `Ui` trait in `gritt-harness` already carry provider/profile
resolution, session lifecycle, execution mode and effort selection, permission
decisions, last-used preferences, and normalized events, with zero references
to Ratatui, Crossterm, or any terminal type. The gap was proof and
documentation, not extraction. Closed it with a dedicated non-terminal Rust
client fixture and boundary documentation, without moving or rewriting any
existing orchestration code.

## Key Decisions

- No code was extracted or moved. `control.rs`, `agent.rs`, `driver.rs`,
  `policy.rs`, `startup.rs`, `setup.rs`, `draft.rs`, `store/`, `mcp/`, and
  `tools.rs` were already free of terminal dependencies; only `tui/` and
  `modes/` are terminal-specific, and neither is imported by the shared
  modules. Confirmed with a direct grep for `ratatui`/`crossterm` across
  every shared module (`crates/gritt-harness/src/{control,agent,driver,
  policy,startup,setup,draft,tools,native_connector,connector_session,
  telemetry}.rs`, `store/*.rs`, `mcp/*.rs`) — the only hit was the doc
  comment I added stating what the module does *not* depend on.
- Added `crates/gritt-harness/tests/control_plane_client.rs`: a new
  integration test file that builds `AgentBuilder`/`ControlPlane` directly
  (the same construction the CLI binary and other test fixtures use),
  implements a `Ui` (`ClientUi`) that never renders anything, and drives the
  full non-terminal flow the acceptance criteria describe: draft-based
  profile/model/mode/effort selection, opening a new session, running a
  turn, consuming normalized events, resuming by name, and answering a
  permission decision directly. This is the "minimal non-terminal client
  fixture" the task asked for, following the same `Fixture`/`fixture_plane`
  pattern already used in `session_draft.rs` and `native_session.rs`.
- Documented the boundary in two places rather than writing a new ADR: a
  new "Control plane API" section in the root `README.md` (Architecture),
  and expanded module doc comments on `control.rs`'s module header and
  `agent.rs`'s `Ui` trait, naming ADR-011 and pointing at the new fixture
  test as the reference example. No new ADR was warranted — ADR-006,
  ADR-011, and ADR-013 already establish every rule this ticket relies on;
  this ticket recorded proof and pointers, not a new architectural decision.
- Left `gritt-harness`'s `Cargo.toml` unchanged (still depends on
  `ratatui`/`crossterm` unconditionally, and `lib.rs` still declares
  `pub mod tui` unconditionally). The acceptance criterion is about the
  *module* not referencing terminal types, which is already true; splitting
  `tui` behind a Cargo feature or a separate crate is a packaging change
  the ticket's scope and plan.md decisions don't ask for, and out-of-scope
  explicitly excludes building the T3Code client. Recorded as a follow-up
  below.

## Alternatives Considered

- Extracting `ControlPlane`/`AgentBuilder`/`Driver` into a new crate
  (e.g. `gritt-control`) separate from `gritt-harness`. Rejected: nothing
  in the acceptance criteria requires a new crate boundary, ADR-006's
  four-crate layout is locked, and the existing module already satisfies
  "no shared API module depends on Ratatui/Crossterm" without one. This
  would be a larger, unrequested refactor.
  See [[codebase-design]] for the boundary-evaluation approach applied
  before ruling this out.
- Feature-gating `ratatui`/`crossterm` in `gritt-harness`'s `Cargo.toml` so
  a hypothetical future dependent (like a real T3Code crate) would not
  transitively compile them. Rejected for this ticket: the acceptance
  criteria are stated at the module-dependency level, not the crate-link
  level, and ADR-011 already treats the first non-terminal frontend as
  in-process without naming a packaging requirement. Flagged as a
  follow-up rather than done silently, since it does change build weight
  for a real second consumer.
- Writing a new ADR for "the control plane is the frozen public seam."
  Rejected: ADR-011 already states this decision almost verbatim ("The
  first non-terminal frontend uses an in-process API over the same control
  plane... The terminal application... must not depend on the future
  frontend"). Restating it as a new ADR would just duplicate ADR-011
  without a new hard-to-reverse choice, which the [[codebase-design]]
  completion criteria call out as unwarranted.

## Assumptions

- Read "module" in the acceptance criterion ("No shared API module depends
  on Ratatui, Crossterm...") as source-level: the shared `.rs` files must
  not `use` or reference those crates, not that `gritt-harness`'s
  `Cargo.toml` must exclude them entirely. Under the stricter crate-level
  reading, the Cargo-feature follow-up below would be required work rather
  than optional. Chose the narrower reading because plan.md's own decisions
  describe the seam in terms of "the API" and module boundaries, not crate
  packaging, and because the out-of-scope list explicitly excludes building
  the T3Code client that would actually need the lighter dependency graph.
- Treated "shared errors identify typed causes and preserve secret
  redaction" as already satisfied by the existing `ErrorKind` enum and the
  redaction tests already in `startup_failover.rs` and
  `native_session.rs`, rather than adding new error-serialization work
  (e.g. `impl Serialize for Error`). No cross-process boundary exists yet
  (ADR-011: first integration is in-process), so there is no concrete
  caller that needs `Error` to serialize; adding it now would be
  speculative. Flagged as a follow-up if a process boundary is ever added.
- Did not add a filesystem abstraction trait for `Workspace` (it does
  direct `std::fs` I/O, unlike `KeyProvider`/`HttpTransport`/`SessionStore`/
  `ProviderSetup`). ADR-011's first non-terminal frontend runs in-process
  on the same machine, so this is consistent with the existing design; it
  would only need abstraction if a future frontend runs sandboxed or
  remote, which is out of scope.

## Edge Cases and Failures

None encountered specific to this change. The one test failure seen during
verification (`the_fixture_home_walkthrough_runs_by_keyboard_and_never_opens_a_session`
in `crates/gritt/tests/tui_pty.rs`, a real-PTY test waiting for rendered text)
was confirmed pre-existing: it fails identically with my changes stashed out,
against the unmodified `main` tree. Not touched by this ticket's scope (no
`tui`/`modes` files were changed) and not caused by this work.

## Validation

- `cargo fmt --all --check` — clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — clean.
- `cargo test --workspace --locked` — all green except the one pre-existing,
  environment-sensitive PTY failure above (reproduced on the unmodified
  baseline via `git stash`; same failure, same panic message, before and
  after this change).
- `cargo build --release --locked` — succeeds. (The build regenerates the
  repo-root `gritt` binary that is intentionally tracked in git per
  `.gitignore`'s comment and ADR-011's artifact-dir setup; restored it to
  its committed bytes afterward with `git checkout -- gritt` so this
  ticket's diff stays limited to source and docs.)
- New dedicated non-terminal integration test:
  `crates/gritt-harness/tests/control_plane_client.rs`, both tests passing:
  - `a_non_terminal_client_selects_profile_model_mode_and_effort_then_runs_a_turn`
  - `a_non_terminal_client_answers_a_permission_decision_without_rendering_anything`
- Grep confirmed no `ratatui`/`crossterm` reference in any shared
  control-plane module (only the new doc-comment prose mentions the names).

## Completion Gate

- Acceptance: all six criteria in `task.md` are met.
  1. Fixture creates/resumes a native session, selects profile/model/mode/
     effort via `SessionDraft`, submits a prompt, and consumes normalized
     `Event`s — met, `control_plane_client.rs`.
  2. Fixture uses the same `ControlPlane`/`AgentBuilder`/startup/policy
     services as CLI/REPL/TUI — met by construction (same types, same
     `open_draft` path every mode uses).
  3. No shared API module depends on Ratatui/Crossterm/terminal
     dimensions/escape sequences — met, verified by grep; module-level, not
     a Cargo-dependency-level change (see Assumptions and Follow-up).
  4. Shared errors identify typed causes and preserve secret redaction —
     met; already true via `ErrorKind` and existing redaction tests, not
     modified.
  5. Existing CLI/REPL/TUI behavior remains covered by current tests —
     met; full workspace suite passes except the pre-existing, unrelated
     PTY flake documented above.
  6. API boundaries and ownership documented for the future T3Code client
     — met: README "Control plane API" section plus `control.rs`/`agent.rs`
     doc comments.
- Scope: stayed within task.md's scope. No T3Code UI was built, no socket
  or subprocess protocol was added, no normalized event was changed to
  carry rendered text, and no provider/tool/session logic was duplicated
  or moved. Touched files: `README.md`, `crates/gritt-harness/src/agent.rs`
  (doc comment only), `crates/gritt-harness/src/control.rs` (doc comment
  only), and the new `crates/gritt-harness/tests/control_plane_client.rs`.
- Validation: see above. All required commands ran and passed; the one
  failure is pre-existing and reproduced on the unmodified baseline.
- Security and safety: no new file, network, or credential-handling code
  was added. The new test uses the same fixture-transport/static-key
  pattern every other harness integration test already uses; no real
  network or filesystem access outside a temp directory.
- Regression risk: low. Only doc comments changed in existing modules
  (no behavior change) plus one new, additive test file. Full test suite
  and clippy/fmt confirm no regression.
- Follow-up: see below.
- Assumptions: see Assumptions section above.

## Follow-up

- If a real second Rust consumer of `gritt-harness` appears (an actual
  T3Code crate), consider feature-gating `ratatui`/`crossterm` (and
  `pub mod tui`) in `gritt-harness` so that consumer does not transitively
  compile them. Not done here because no such consumer exists yet and the
  acceptance criteria were satisfied at the module-dependency level.
- If Gritt ever adds a process boundary for a non-terminal frontend (a
  local socket, per ADR-011's deferred option), revisit whether `Error`
  needs a `Serialize` impl and a `redact_error` helper mirroring
  `redact_event`, since today's redaction discipline for `Error.message`/
  `.diagnostic` is enforced by convention and tests rather than a single
  structural chokepoint.

## Updates

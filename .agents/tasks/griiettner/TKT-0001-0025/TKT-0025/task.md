---
id: TKT-0025
namespace: griiettner
title: Detect and offer updates for installed provider CLIs
artifact: task
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0024
areas:
  - crates/gritt-core
  - crates/gritt-provider
  - crates/gritt-harness
  - crates/gritt
skills:
  - dev-provider
  - dev-harness
  - dev-cli
  - codebase-design
  - tdd
  - write
---

# TKT-0025 Task: Detect and offer updates for installed provider CLIs

## Goal

Tell users when an installed connector CLI is out of date and offer a safe,
user-approved update using the package manager or installer that actually owns
the executable.

## Inputs

- Connector model discovery and lifecycle operation from TKT-0024.
- Connector version and auth reporting from TKT-0012 and `dev-harness`.
- Process supervision, approval, cancellation, and cleanup contracts from
  ADR-010 and the harness implementation.
- Configuration, cache, redaction, and failure rules from ADR-008.

## Scope

- Add a provider-neutral connector version status with installed version,
  latest known version, comparison result, installation source, checked time,
  freshness, and typed failure state.
- Detect installation ownership for Homebrew, npm, pipx, Cargo, and supported
  vendor or direct installers using executable paths and package metadata.
  Return unknown or ambiguous instead of guessing.
- Add provider-specific latest-version checks and fixed update actions for the
  connectors covered by TKT-0024. Use documented package or release sources.
- Show the status during connector setup and expose a refresh operation through
  print, REPL, and TUI clients without blocking normal startup.
- Prompt for explicit approval before any update. Run the exact executable and
  argument vector through the supervised process runner, capture bounded
  content-free output, and recheck the version after a successful update.
- Add tests and fixtures for each package manager, unknown and ambiguous
  ownership, current and outdated versions, offline checks, malformed version
  data, declined updates, failed updates, cancellation, and post-update
  verification.

## Out of Scope

- Updating Gritt itself or implementing a hosted auto-update service.
- Running updates automatically at startup or without a per-action approval.
- Guessing a package manager from a path alone when package metadata conflicts
  or is absent.
- Modifying package-manager configuration, shell profiles, credentials, or
  vendor settings.
- Changing external agent authority, model catalog parsing, native provider
  versioning, or mid-session connector behavior.

## Acceptance Criteria

- For a current installed CLI, Gritt reports the installed version and source
  and does not offer an update.
- For an outdated CLI with a known owner, Gritt reports both versions, names
  the owner, displays the exact update action, and updates only after explicit
  approval.
- Homebrew, npm, pipx, Cargo, and each supported vendor installer have
  detector and update-command fixture coverage. The tests prove that the
  action is an argument vector, not a shell string.
- Unknown or ambiguous ownership offers no guessed update and explains what
  the user can do next.
- Version checks and updates are nonblocking, cancellable, timeout-bounded,
  and do not prevent a usable connector from starting when the check fails.
- A successful update triggers a fresh version check. A failed or declined
  update leaves the connector usable and reports the outcome.
- Print, REPL, and TUI clients use the same lifecycle service and display
  stale, unavailable, and current states consistently.
- Logs, fixtures, errors, and transcripts contain no keys, prompt text, tool
  content, or package-manager credentials.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
- Detector and update-action fixture tests for Homebrew, npm, pipx, Cargo,
  vendor, unknown, and ambiguous sources.
- Harness tests for approval, cancellation, timeout, process cleanup, and
  post-update recheck.
- Print, REPL, and TUI tests for nonblocking status display and shared outcomes.
- `cargo build --release --locked`
- `./.agents/gritt-agent ticket validate`

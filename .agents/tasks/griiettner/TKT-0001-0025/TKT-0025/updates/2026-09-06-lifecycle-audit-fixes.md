---
id: TKT-0025
namespace: griiettner
title: Fix update cancellation, credential handling, ownership, and status
artifact: update
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
---

# TKT-0025 Update: lifecycle audit fixes

## Trigger

The user requested fixes for the eight findings in the combined TKT-0024 and
TKT-0025 review. This update records the version and update changes. The paired
TKT-0024 update records catalog state and shared probe supervision.

## Changes and evidence

- CLI and REPL updates accept a cancellation future through the shared control
  plane. Ctrl-C stops the updater and waits for cleanup. The CLI exits with 130;
  the REPL stays open. The previous standalone invocation exited while its
  separate updater process group continued running. A binary-level fixture now
  proves both paths terminate the updater, and the existing task-abort test
  continues to cover TUI cancellation.
- Update stdout and stderr are discarded after measuring activity. Failure
  diagnostics retain command and exit status. Replacing only known provider
  secrets could expose credentials printed from package-manager configuration;
  a synthetic credential reproduced that failure. The public failure output
  field remains for compatibility but is empty for external updates.
- npm ownership is checked with the selected manager's `npm root --global`.
  Only an executable inside that global package gets an update action. The
  action pins the manager path and prefix. Local packages, another manager's
  installation, and globally linked local packages offer no update. Tests prove
  the command remains an argument vector. The local and linked-package tests
  were observed failing before the corresponding corrections.
- Version caches include the executable, installation source, querying manager,
  and query arguments. Mismatched and legacy entries are ignored. The source
  change test previously reused npm's answer for a Homebrew installation and
  now performs a fresh query. Failed refreshes still preserve a matching stale
  answer and retry timestamp.
- Initial TUI session adoption schedules the same advisory version check as
  later session transitions. Its regression test previously timed out waiting
  for the missing result. The newer muted/warning footer styling is preserved.
- Removed the unused `InstallSource::is_known` method. Shared process guards
  replace the updater-only drop guard and also protect model/version probes.

## Decisions

Cancellation crosses the core interface as an existing `BoxFuture`, keeping
runtime-specific cancellation tokens out of the core crate. The REPL reuses its
existing cancellation handle. A separate polling flag or a process registry
exposed through the public connector interface was unnecessary.

Automatic npm updates require positive evidence of a global installation.
Updating a local package globally, or changing package-manager configuration,
would affect the wrong installation. Prefix handling follows the official
[npm folder documentation](https://docs.npmjs.com/cli/v11/configuring-npm/folders/).
The bounded root probe runs on requested/background checks; offline startup
does not run npm to authorize an action.

## Validation and completion gate

- Acceptance: all eight review findings are addressed across this update and
  the paired TKT-0024 update.
- Scope: no actual agent installation was updated, no credentials or package
  configuration were changed, and no dependency was added.
- Validation: `cargo test --workspace --locked`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo fmt --all --check`, and `cargo build --release --locked` passed.
  After the final npm-link correction, connector version tests, clippy, and
  release build were repeated. Ticket sync and validation passed.
- Failure history: early focused tests reproduced the reported failures. One
  parallel probe fixture timed out before writing its PID; its startup allowance
  was increased. A missing-manager assertion also required preserving the
  probe's existing content-free `cannot run` diagnostic.
- Security and safety: unknown package-manager content never enters failures;
  npm commands target verified global installations; interruption stops process
  groups before returning. Legacy caches cannot authorize a current result.
- Regression risk: the workspace covers shared probe callers, session lifecycle,
  update approval, stale fallback, and PTY behavior. The independent caller
  review found no introduced TUI, CLI, or REPL integration defects.
- Assumptions: raw update output is intentionally unavailable even for failed
  commands. A user can run the displayed command directly for installer-specific
  diagnostics. An unverifiable npm installation requires its original installer.
- Follow-up: no remaining finding from this audit. Live package-manager updates
  and cross-platform live checks were not run. Existing Windows/Ubuntu CI and
  Cursor verification follow-ups in the original report remain separate.

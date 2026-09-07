---
id: TKT-0025
namespace: griiettner
title: Detect and offer updates for installed provider CLIs
artifact: report
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
dependencies:
  - TKT-0024
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
  - dev-cli
  - codebase-design
  - tdd
  - write
  - review-ticket
---

# TKT-0025 Report: Detect and offer updates for installed provider CLIs

## Summary

Gritt now tells the user when an installed connector CLI is behind the
newest version its installer publishes, and runs that installer's
documented update command after an explicit approval. Print, REPL, and
the full-screen UI share one control-plane operation for the check
(`ControlPlane::connector_version`) and one for the update
(`ControlPlane::connector_update`), with one line formatter
(`connector_version_lines`).

`gritt-core` owns the provider-neutral types: `InstallSource`,
`ConnectorVersionStatus`, the typed `ConnectorVersionCheck` and
`ConnectorUpdateOutcome` outcomes, `VersionCheckMode`, and `UpdateAction`,
which is an executable plus a fixed argument vector. The `Connector`
trait gained `check_version` and `update` with `Unsupported` and
`NoAction` defaults, so the native connector needed no change.
`gritt-connector` owns the evidence-based owner detector
(`install.rs`), the documented latest-version queries and their parsers,
the version cache (`versions.rs`, beside the model cache), and the update
runner on the supervised process path. Each protocol declares its vendor
installer's directory markers and self-update subcommand. The harness
orchestrates; the binary stays the edge for cache directories and the
terminal prompt.

Owner detection resolves the executable's symlink and requires evidence:
a Homebrew `Cellar` or `Caskroom` component, an npm `node_modules`
package directory with `package.json`, a pipx `venvs/<pkg>` directory
with `pipx_metadata.json`, a Cargo `.crates.toml` entry naming the
binary, or a vendor marker under the home directory. Two matches are
`Ambiguous`; none is `Unknown`. Neither gets a command; both carry a
`next_step` sentence.

## Key Decisions

- The newest version comes from the owner's own documented query, run as
  an argument vector through the same probe path as `--version`:
  `brew info --json=v2 <name>`, `npm view <pkg> version`, and
  `cargo search <crate> --limit 1`. This keeps the user's registry and tap
  configuration in play and adds no HTTP client to `gritt-connector`.
  pipx and the three vendor installers publish nothing Gritt can read, so
  their checks return `LatestUnavailable { UnsupportedSource }` with the
  update command still available.
- Update commands are fixed per owner: `brew upgrade [--cask] <name>`,
  `npm install -g <pkg>@latest`, `pipx upgrade <pkg>`,
  `cargo install <crate>`, and the vendor self-updates `claude update`,
  `cursor-agent update`, `opencode upgrade` run on the connector
  executable itself. The package or formula name comes from the path
  evidence, not from a table, so an unexpectedly named install is
  updated by the name that is really installed.
- A current or newer install has `update: None` (acceptance criterion 1).
  An outdated one, or one whose newest version is unknown, keeps the
  command. `ConnectorVersionCheck::update_available` is true only for a
  fresh `Checked` result with `Outdated`; a stale cache is shown as stale
  and never as an offer.
- Freshness reuses `ModelListPolicy` (interval and stale fallback) and the
  TKT-0024 `checked_at` / `last_attempt_at` pattern, including
  `failed_since_check` so a failed refresh cannot let a later plain lookup
  report `Checked`.
- Opening a connector session runs the check in `Offline` mode only:
  installed version and owner from local evidence, newest from the cache.
  Print and REPL print that as a startup note. The full-screen UI then
  runs a `Cached` check detached from the user's request queue and shows
  the result in the sidebar. No client waits on a package manager to
  start a session.
- Approval is per invocation and belongs to the client: `gritt connectors
  --update NAME` asks `[y/N]` on the terminal or takes `--yes`; REPL
  `/update` asks on the prompt; the TUI `/update` opens the same modal
  overlay a tool approval uses, with `resource` set to the exact command
  and the version lines as the preview. The approved vector is the one
  displayed; nothing is re-derived when it runs.
- The update runner reads lines with the connector's idle timeout as the
  silence bound and a 15-minute hard cap, keeps the last 12 lines
  redacted and capped at 240 characters, and kills the process tree on
  timeout or when its future is dropped (`KillOnDrop`). A success is
  followed by a `Refresh` check.

## Alternatives Considered

- Fetching the newest version from registries over HTTP. Rejected: it
  bypasses the user's registry, tap, and proxy configuration and would
  need a network client in the connector crate.
- Guessing the owner from a `/opt/homebrew` or `~/.local/bin` prefix.
  Rejected by the ticket; npm globals under a Homebrew node live under the
  same prefix, and the Claude native installer keeps a `node_modules`
  tree the npm detector would otherwise claim. The evidence rules and the
  ambiguous outcome exist for these two cases.
- Running the version query at session open. Rejected: a slow or offline
  package manager would delay the session.
- A separate config key for the check interval. Rejected for the same
  reason TKT-0024 reused `ModelListPolicy`.

## Assumptions

1. Vendor markers: Claude Code native installer under `.local/share/claude/`
   or `.claude/local/`; Cursor CLI under `.local/share/cursor-agent/`;
   OpenCode install script under `.opencode/bin/` (confirmed on this
   machine). Cursor is not installed here; its marker and `update`
   subcommand come from published docs.
2. Codex's `codex update` subcommand is not used: Codex has no
   documented vendor directory, and the two installs Gritt can identify
   (npm, Homebrew cask) are updated by their owners.
3. A cask version may carry a build after a comma; the part before it is
   the version.
4. `find_executable` resolves a bare manager name on `PATH`; tests inject
   fake managers with `with_manager_programs` and a scratch home with
   `with_install_env`, so no test depends on the host's package managers.
5. `--version` is probed again for the check even though `info()` probed
   it at open. It is a local, bounded probe; sharing the value would have
   coupled the session and the check.
6. `gritt doctor` was not extended; `gritt connectors --check` is the
   diagnostic surface for versions.

## Edge Cases and Failures

- Missing executable: `NotInstalled`, no query, no action.
- Native connector: `Unsupported`; `connector_update` returns `NoAction`.
- Unknown or ambiguous owner: `LatestUnavailable { UnsupportedSource }`
  with `next_step`; no action; `/update` and `--update` say so.
- Query failures classify by output: `Network` (ENOTFOUND, EAI_AGAIN,
  ECONNREFUSED, "could not resolve", "offline", ...), `Authentication`
  (401, 403, "unauthorized", ...), `Timeout` from the probe deadline,
  `CommandFailure` otherwise, `MalformedResponse` for empty or unreadable
  output. The text stays in the process.
- A failed query with a cached answer returns `CachedStale`; the next
  `Cached` lookup stays stale; `Refresh` retries.
- A package manager that is not installed is `CommandFailure`, not a
  panic.
- Update failure keeps the connector usable at its old version and
  reports the exit status with redacted output; a silent update is
  `TimedOut` and its process tree is killed; dropping the update future
  kills the process; a declined update runs nothing.
- Windows: no `std::os::unix` use outside `#[cfg(unix)]`; the new test
  files are `#![cfg(unix)]` because they build shell fixtures.

## Validation

- `cargo fmt --all --check` passed
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passed
- `cargo test --workspace --locked` passed after the fix recorded below
- `crates/gritt-connector/tests/versions.rs`: detector fixtures for
  Homebrew formula and cask, npm (scoped, manifest required), pipx
  (metadata required), Cargo (`.crates.toml` required), vendor
  (home-relative only), ambiguous (npm plus vendor); connector checks for
  outdated, current, fresh cache, refresh, stale fallback, network,
  authentication, malformed, empty, offline, missing manager, Homebrew and
  Cargo queries, unknown owner, not installed; update runs for success
  with recheck, failure with redaction, timeout with process kill,
  cancellation with process kill, and missing executable.
- `crates/gritt-harness/tests/connector_session.rs`: offline status on a
  new connector session and none on resume; REPL `/version`, `/update`
  with `y`, and the recheck; declined twice and a failed update leaving
  the connector usable.
- `crates/gritt-harness/src/tui/app/tests.rs`: sidebar line and modal
  approval from a checked result, approval yielding the displayed vector,
  decline, late and foreign results dropped, `/new` reset, unknown owner
  offering nothing, native sessions explaining themselves.
- `crates/gritt/src/main.rs`: `connectors --check --refresh`,
  `--update NAME --yes`, and the `requires` rules.
- `crates/gritt/tests/e2e.rs`: `gritt connectors --check`, `--update`
  declined without a terminal answer, `--update --yes` running the vendor
  command and rechecking, and `--update native` refused.
- `cargo build --release --locked` passed
- `./.agents/gritt-agent ticket validate` passed
- Live checks against this machine's real installs were not run as tests;
  the evidence formats (`Caskroom/codex/0.153.4`,
  `lib/node_modules/@anthropic-ai/claude-code`, `~/.opencode/bin`,
  `brew info --json=v2`, `npm view`, `cargo search`) were read by hand
  before the detectors were written.

## Completion Gate

- Acceptance: yes. Current installs offer nothing; outdated installs with
  a known owner show both versions, the owner, and the exact vector, and
  run it only after approval; every listed owner has detector and
  command fixtures; unknown and ambiguous owners explain the next step;
  checks and updates are bounded, cancellable, and off the session-open
  path; a success rechecks and a failure or decline leaves the connector
  usable; the three clients use one service; no key, prompt, or tool
  content reaches a log, fixture, error, or transcript.
- Scope: yes. Gritt's own updates, automatic updates, package-manager
  configuration, native provider versioning, and model catalog parsing
  were not touched. `crates/gritt-harness/src/changes.rs` (the
  pre-existing Windows compile failure from TKT-0019) was left alone.
- Validation: the commands above; see the update file for the one test
  fixed during execution.
- Security and safety: every command is a fixed vector; no shell; no
  value from a prompt, credential, or agent output enters one; the
  runner redacts known secrets from the kept output; detection reads
  only executable paths, `package.json`, `pipx_metadata.json`, and
  `.crates.toml`.
- Regression risk: traced per `review/impact`. Additive by default:
  `Connector` and `Protocol` gained default methods, so the native
  connector and every protocol compile unchanged; `Opened` gained a field
  and all five constructors set it; `default_connectors_configured`
  gained a parameter and its one caller passes it; `version_at_least`
  delegates to `compare_versions` with the same component parse and the
  same answers (covered by its existing test); `ModelSection`, `Work`,
  `Command`, and `UiMsg` grew, and every match compiled. Two things run
  on the default path that did not before: opening a connector session
  probes `--version` once more and reads local install evidence (bounded
  by the health timeout, no network), and the full-screen UI then runs
  the owner's version query detached, at most once per
  `model_list.refresh_interval_secs`, which can reach npm, Homebrew, or
  crates.io. The second is what the ticket asks for; it never blocks the
  session and a failure only marks the status stale. The help overlay is
  taller, which the regenerated snapshots record.
- Follow-up: below.
- Assumptions: listed above.

## Follow-up

- A hygiene ticket for `main`'s red CI: the unix-only import in
  `crates/gritt-harness/src/changes.rs:501` and the Ubuntu
  `mcp_runtime` descendant-cleanup test.
- Re-verify the Cursor marker and `cursor-agent update` on a machine
  with Cursor installed.
- `gritt doctor` could print the version status once its output format
  is settled.

## Updates

- [2026-09-06 lifecycle audit fixes](updates/2026-09-06-lifecycle-audit-fixes.md)
- [2026-09-06 execution notes](updates/2026-09-06-execution.md)

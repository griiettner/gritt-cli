---
id: TKT-0025
namespace: griiettner
title: Detect and offer updates for installed provider CLIs
artifact: plan
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
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

# TKT-0025 Plan: Detect and offer updates for installed provider CLIs

## Boundary

Keep version and installation-source detection behind a connector lifecycle
service. The service returns a provider-neutral status containing installed
version, latest version when known, source, check time, and an optional typed
update action. Connector adapters own version commands and vendor metadata.
The binary or harness owns user approval and process execution. The UI only
renders the status and requests the action through the shared service.

## Sequence

1. Inventory the connector executable names, existing capability/version
   reporting, and the platform process runner. Document the version endpoint
   or command and package metadata available for each CLI.
2. Implement source detectors with explicit precedence and an ambiguous result.
   Inspect executable paths and package-manager metadata without reading
   secrets or shell startup files. Support Homebrew, npm, pipx, Cargo, and
   documented vendor installers where evidence identifies the source.
3. Implement latest-version checks using the provider's documented release or
   package source. Bound timeouts, cache results, and classify network,
   authentication, malformed-response, and unsupported-source failures.
4. Create a typed update action that contains an executable plus fixed
   arguments. Display the command for confirmation, require an explicit user
   approval, run it through the existing supervised process path, and recheck
   the installed version after success.
5. Wire status and update prompts into connector setup and the existing print,
   REPL, and TUI paths. Startup must remain usable when the check is slow,
   offline, unsupported, or unavailable.
6. Add fake-manager and fake-CLI fixtures for every detector, version result,
   ambiguity case, approval path, failure path, and post-update recheck.

## Decisions

- Version checks are advisory and asynchronous. They never prevent a connector
  from starting unless the connector itself rejects the installed version.
- Gritt may cache a successful latest-version result for the configured check
  interval and must show its age. A failed refresh keeps the last result marked
  stale.
- A source detector must have evidence before it emits an update command. If
  multiple managers could own the executable, report ambiguous and offer no
  automatic update until the user chooses a source.
- Updates are opt in for each invocation. Gritt never runs a package-manager
  command at startup without approval, never uses a shell string, and never
  changes the user's package manager configuration.
- The first implementation covers the connectors in TKT-0024 and the package
  managers listed in the concept. Adding a new manager requires a detector,
  fixed command definition, fixtures, and documentation.

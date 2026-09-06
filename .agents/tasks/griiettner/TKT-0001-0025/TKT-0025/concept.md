---
id: TKT-0025
namespace: griiettner
title: Detect and offer updates for installed provider CLIs
artifact: concept
status: ready
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

# TKT-0025 Concept: Detect and offer updates for installed provider CLIs

## Problem

Connector model catalogs and protocol behavior come from installed provider
CLIs, which can be older than the current service. Gritt currently has no
reliable way to tell the user that a connector CLI is outdated or how to update
it. The correct update command depends on whether the executable came from
Homebrew, npm, pipx, Cargo, a vendor installer, or another source.

## Intent

Check the installed connector version against the latest version source that
the provider documents, identify the installation manager when possible, and
offer a user-confirmed update using the matching package-manager command. The
result must be safe when the install source is unknown or ambiguous.

## Success Criteria

- Gritt reports installed version, latest known version, and the source and
  time of the check without blocking connector startup.
- An outdated CLI shows the detected installation source and the exact update
  action Gritt would run, then waits for explicit user approval.
- Supported sources include Homebrew, npm, pipx, Cargo, and vendor or direct
  installs when a documented update command exists. Unknown sources remain
  visible and actionable without an unsafe guessed command.
- Update failures preserve the existing connector, show a concise diagnostic,
  and never expose secrets or prompt content.

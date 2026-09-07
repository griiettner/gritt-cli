---
id: TKT-0024
namespace: griiettner
title: Fix connector catalog state and probe cleanup
artifact: update
status: done
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
---

# TKT-0024 Update: lifecycle audit fixes

## Trigger

The review of TKT-0024 and TKT-0025 found three remaining defects affecting
model discovery. The user requested fixes. Work started from `9063266`, which
also includes the later connector version footer styling.

## Changes and evidence

- Adopting a session clears the connector draft and invalidates outstanding
  catalog requests. Previously, resuming a native session after choosing a
  connector made `/models` reopen that connector's picker.
- Changing connector identity clears the old catalog before loading. Previously,
  models from the previous connector remained selectable with the new badge.
  `ConnectorCatalogView.connector` now controls this check instead of being
  write-only state.
- Model and version probes use the supervised process group. Timeout cleanup
  waits for termination; dropping a probe signals its process group before
  scheduling reaping. Previously, only the direct child was killed and its
  descendants survived. Captured probe output also has a size limit.

The adoption, foreign-model, and probe-descendant tests were observed failing
before their fixes and passing afterward. The first foreign-model fixture used
display names instead of the runtime's canonical connector names; correcting
that fixture exposed the intended stale-row failure.

## Validation and completion gate

- Acceptance: the three findings above are fixed. Selection, default-model,
  resume, malformed-output, and stale-cache coverage remains green.
- Scope: connector lifecycle and TUI state only. Native provider discovery and
  the subsequent footer styling are preserved.
- Validation: `cargo test --workspace --locked`, workspace clippy with
  `-D warnings`, formatting, and release build passed. This includes connector
  fixtures, reducer tests, and PTY walkthroughs. Ticket validation is recorded
  with the paired TKT-0025 update.
- Security and safety: probe cancellation covers descendants and retains no
  additional diagnostic content. No dependency was added.
- Regression risk: shared probes also serve version and authentication checks;
  the workspace connector tests cover those callers. An independent caller
  review found no new issues in adoption or catalog invalidation.
- Assumptions: session adoption consumes the connector draft; changing a live
  connector's model remains outside this ticket's scope.
- Follow-up: no remaining finding from this audit. Live authenticated agent
  turns and additional manual UI checks were not run; existing live-platform
  follow-ups in the original report remain open.

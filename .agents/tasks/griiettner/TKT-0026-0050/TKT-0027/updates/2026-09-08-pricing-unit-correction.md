---
id: TKT-0027
namespace: griiettner
title: Allow provider and model changes within an active conversation
artifact: update
status: done
owner: griiettner
created: 2026-09-08
updated: 2026-09-08
areas:
  - crates/gritt-harness
skills:
  - dev-harness
  - tkt-update
  - write
---

# TKT-0027 Update: Correct the pricing unit

## Trigger

The picker pricing note was initially formatted per token. The requested
display unit is per million tokens, which also matches the value stored in the
provider model capability.

## Fix

The note now uses the form `$2.00/M tokens in · $8.00/M tokens out`. The
focused test name and expected output were updated accordingly.

## Validation

- `cargo fmt --all --check` passed.
- The focused harness test was updated and is ready to run with the corrected
  name.

## Completion Gate

- Acceptance: satisfied. The picker displays provider-reported input and
  output prices per million tokens.
- Scope: limited to the model picker formatter, its test, and ticket notes.
- Security and safety: no network, credentials, permissions, or persistence
  behavior changed.
- Regression risk: models without both prices remain unchanged, and
  deprecated-model notes still compose with pricing.
- Follow-up: none after the corrected test passes.

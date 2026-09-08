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
  - codebase-design
  - dev-harness
  - tkt-update
  - write
---

# TKT-0027 Update: Show model pricing in the picker

## Trigger

The model picker listed provider and model identity but did not expose the
pricing already returned in each model's capabilities.

## Changed behavior

The native model picker now shows both reported rates as a third row, using the
equivalent dollar cost per token for input and output. It only shows the note
when both prices are present. Deprecated-model text remains visible and is
combined with the pricing note when both apply.

## Files

- `crates/gritt-harness/src/tui/app.rs`: format pricing and add it to model
  picker rows.
- `crates/gritt-harness/src/tui/app/tests.rs`: cover the displayed conversion
  from per-million prices to per-token prices.

## Validation

- `cargo fmt --all --check` passed.
- `cargo test -p gritt-harness
  the_model_picker_shows_reported_input_and_output_price_per_token` passed.
- `cargo test -p gritt-harness` passed: 217 unit tests and all harness
  integration suites passed.
- `.agents/gritt-agent ticket sync` and `.agents/gritt-agent ticket validate`
  passed with zero warnings.

## Completion Gate

- Acceptance: satisfied for the pricing-display follow-up. Existing provider
  model capabilities remain the source of truth.
- Scope: stayed within the TUI model picker and its test.
- Validation: formatting and the focused harness test passed.
- Security and safety: no keys, network access, permissions, or persistence
  behavior changed.
- Regression risk: picker rows with missing pricing are unchanged; deprecated
  rows retain their existing note.
- Follow-up: none for this pricing-display change.
- Assumptions: per-token display is more direct for the request than the
  internal per-million representation, and eight decimal places are enough to
  make the small rates readable.

---
id: TKT-0016
namespace: griiettner
title: Define model, effort, session-draft, and provider setup contracts
artifact: update
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0015
areas:
  - crates/gritt-harness
  - crates/gritt-provider
skills:
  - tkt
  - tkt-exec-chain
  - dev-harness
  - dev-provider
---

# 2026-09-04 Review fixes

## Trigger

The reviewer returned needs-fix on PR #8 with two confirmed findings in
`crates/gritt-harness/src/control.rs`.

## Changes

1. Catalog ids with a profile-name prefix. `alias::resolve` reads
   `openai/gpt-5-nano` as a qualified name whenever a profile called
   `openai` is configured, so selecting that OpenRouter catalog model
   under `openrouter` rejected the draft with `ModelOutsideProfile`, and
   resuming a session pinned to it rejected with `SessionPinned`. A new
   `ControlPlane::resolve_under_profile` takes an id the selected
   profile's catalog lists as that model (applying a declared deprecation
   replacement) before any alias or qualified reading. Resume compares the
   requested name with the exact stored id first and only resolves when
   they differ.
2. Removed stored profile. `validate_resume` now checks that the stored
   profile still exists in config before touching the catalog and returns
   `DraftOutcome::Rejected` with `DraftError::UnknownProfile`. Before, the
   generic `Config` error from `warm_catalog` propagated as `Err`.

Follow-ups from the review applied because they were cheap:

- Resume warms the catalog before model resolution, so the outcome does
  not depend on whether a picker loaded the list earlier. The catalog
  state now also travels with a `SessionPinned` rejection.
- The old-continuation compatibility test covers Chat Completions and
  Messages as well as Responses, and the legacy wire-shape test is named
  for the deliberate change
  (`legacy_reasoning_switch_enables_the_provider_default_level_instead_of_medium`).

Deferred to the setup UI ticket: masked-input and cancellation tests.

## Edge case observed

Adding an `openai` profile to the shared test config made the bare
default model `openai/gpt-5-nano` resolve as OpenAI's model in tests that
never load a catalog. That is the qualified-name rule working as
specified when no catalog id matches, so the four-profile config is
limited to the two regression tests through `fixture_plane_with`.

## Validation

- `cargo fmt --all --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo test --workspace`: pass; `session_draft` is now 10 tests (the
  two new ones are
  `catalog_ids_with_a_profile_name_prefix_stay_in_the_selected_profile`
  and `a_removed_profile_rejects_resume_without_touching_the_session`),
  every other count unchanged
- `gritt-agent ticket sync`, `ticket validate`, and
  `ticket chain-check --ticket TKT-0016 --base main`: see the commit
  message and PR thread for the run on the final tree

## Remaining follow-up

None new. The report's follow-up list still stands (ADR-007 update,
Anthropic capability parsing, config reload after `save_profile`, TOML
comment preservation, effort in the status bar).

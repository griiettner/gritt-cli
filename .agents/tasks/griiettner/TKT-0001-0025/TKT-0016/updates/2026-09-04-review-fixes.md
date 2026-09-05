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

## Second round: deprecation regression

Trigger: the re-review found that `resolve_under_profile` returned a
deprecated catalog id unchanged when the provider declared no
replacement, skipping the configured-alias-or-reject step that
`alias::resolve` applied. A draft could persist and send a deprecated id.

Change: the deprecation policy moved out of `alias::resolve` into a
public `alias::apply_deprecation(config, catalog, profile, model)` in
`crates/gritt-provider/src/alias.rs`; `resolve` calls it after name
resolution and `resolve_under_profile` calls it on an exact catalog hit.
Catalog-id-before-alias precedence is unchanged. A deprecated id remaps
to the provider-declared replacement, then to a configured profile alias
or global alias into the same profile, and is otherwise rejected as
`DraftError::ModelResolution` with the resolver's message. On resume, a
deprecated name whose replacement is the stored model resumes; one with
no replacement cannot match the pin and is `SessionPinned`.

Tests: `deprecated_catalog_ids_are_remapped_or_rejected_on_creation_and_resume`
(provider replacement, configured alias replacement, no replacement
rejected with nothing created, stored session carries the replacement,
resume with a remapped name, resume with an unreplaceable name) and the
optional `resume_resolves_against_the_catalog_it_just_warmed` (cold
in-memory catalog, fresh disk cache holding the deprecation, resume
remaps through the list that `validate_resume` warmed).

Validation on the final tree: `cargo fmt --all --check` pass,
`cargo clippy --workspace --all-targets -- -D warnings` pass,
`cargo test --workspace` pass with `session_draft` at 12 and every other
count unchanged; `ticket sync`, `ticket validate`, and `ticket
chain-check --ticket TKT-0016 --base main` pass with no warnings.

## Remaining follow-up

None new. The report's follow-up list still stands (ADR-007 update,
Anthropic capability parsing, config reload after `save_profile`, TOML
comment preservation, effort in the status bar).

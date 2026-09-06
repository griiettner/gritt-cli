---
id: ADR-008
title: Configuration secrets and caches
status: accepted
date: 2026-09-04
tags:
  - configuration
  - security
  - providers
read_when:
  - adding a config value
  - loading or storing a provider key
  - changing model-list refresh behavior
  - changing how a new session picks its profile, model, or effort
---

# ADR-008: Configuration secrets and caches

## Decision

Configuration precedence is command-line flags, project config, user config,
environment variables, then built-in defaults. Config files name key
variables but never contain key values. Provider keys are read from the OS
keychain first, then the named environment variable. The implementation will
use the maintained `keyring` 4.2.0 crate's cross-platform API. Entering a key
writes the keychain. If no keychain is available, environment-only operation
remains the fallback. Gritt never writes keys to an application file.

Model lists refresh at most once per day by default. A failed refresh uses the
last cached list and marks it stale. Provider capability data controls which
features Gritt advertises.

Logs are structured and content-free by default. When content logging is
explicitly enabled, retention is seven days. Keys are always redacted and
never enter logs, fixtures, errors, or transcripts.

## Startup failover and remembered choices (2026-09-06)

`fallback_profiles` is an ordered list of profiles a new native session
tries after the default. The list is validated at load: an unknown name or
a repeated one, the default included, fails the configuration. A profile
may name a `fallback_model` for the case where startup lands on it without
a model its list contains.

With more than one candidate, startup probes each profile before opening
the session: key resolution, a live `GET /models`, and the model's presence
in the list. This is the one place the daily refresh interval does not
apply. A single candidate, whether pinned by the user or the only profile
configured, keeps the cached load, so a configuration without the list
behaves as before. Skipped profiles are reported by class with key values
redacted; failover is never attempted mid-turn or on a resumed session.

The last new native session to complete a turn records its profile, model,
and effort per workspace in the product database (migration
`0005_last_used`). For each field the precedence is flag or picker, then
the remembered value, then the configured default. The remembered value
sits between the flags and the configuration: `default_profile` and
`default_model` are the starting point for a workspace and the fallback
when a remembered profile is no longer configured, and the last session's
choices carry forward from there. The record holds names and an effort
label only, never a credential. A missing key on a single candidate is
still reported on the first request, as before this decision; only a
chain treats it as a reason to move to the next profile.

## Rationale

The OS keychain means the platform credential store, such as macOS Keychain,
Windows Credential Manager, or a Linux Secret Service provider. It avoids
inventing another encrypted-file format and keeps secrets out of the repo.

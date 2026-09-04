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

## Rationale

The OS keychain means the platform credential store, such as macOS Keychain,
Windows Credential Manager, or a Linux Secret Service provider. It avoids
inventing another encrypted-file format and keeps secrets out of the repo.

---
id: TKT-0022
namespace: griiettner
title: Add provider failover and remember session preferences
artifact: concept
status: concept
owner: griiettner
created: 2026-09-06
updated: 2026-09-06
areas:
  - provider
  - harness
  - cli
skills:
  - dev-provider
  - dev-harness
  - dev-cli
---

# TKT-0022 Concept: Add provider failover and remember session preferences

## Problem

Gritt currently selects the configured default provider and stops when that
provider is unavailable. A user with several configured providers must change
the profile manually. New sessions also return to the configured model and
provider-default effort instead of using the choices from the last successful
session.

## Intent

Probe configured native providers in an explicit fallback order, select the
first provider that has usable credentials and a reachable model endpoint, and
remember the last successful native profile, model, and effort for future new
sessions. Existing sessions remain pinned to their stored provider and model.

## Success Criteria

- A configured default provider is tried first, followed by explicitly ordered
  fallback profiles.
- Authentication, connectivity, and model availability failures move startup
  to the next eligible profile with a visible diagnostic.
- A successful new native session records its profile, model, and effort as the
  last-used preferences without storing secrets.
- New sessions use those preferences only when no explicit CLI, draft, or
  configuration choice overrides them.
- Resuming a session never silently changes its provider, model, or effort.

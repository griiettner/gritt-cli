---
id: ADR-011
title: Release and frontend boundary
status: accepted
date: 2026-09-04
tags:
  - release
  - frontend
  - distribution
read_when:
  - preparing a release
  - adding a non-terminal frontend
  - changing the control-plane boundary
---

# ADR-011: Release and frontend boundary

## Decision

The first release ships as one native binary per supported operating system,
macOS, Windows, and Linux. Release work includes signing, checksums,
reproducible builds, diagnostics, and an upgrade path.

The first non-terminal frontend uses an in-process API over the same control
plane. A local socket is deferred until a separate process boundary is needed.
The terminal application remains the reference client and must not depend on
the future frontend.

## Rationale

An in-process API keeps the first product small and avoids inventing a local
protocol before there is a second process that needs it.

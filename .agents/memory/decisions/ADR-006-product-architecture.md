---
id: ADR-006
title: Gritt product architecture
status: accepted
date: 2026-09-04
tags:
  - architecture
  - rust
  - workspace
read_when:
  - adding a product crate
  - changing crate boundaries
  - deciding where product I/O belongs
---

# ADR-006: Gritt product architecture

## Decision

Gritt is an open-source Rust application released for macOS, Windows, and
Linux. The Cargo workspace has four layers: `gritt-core` contains provider-
neutral contracts with no I/O, `gritt-provider` contains provider adapters,
`gritt-harness` contains the terminal interface, policy engine, sessions, and
built-in tools, and `gritt-connector` contains external-agent supervision.
The `gritt` binary owns argument parsing, configuration, key loading, and mode
selection.

Dependencies point upward only. The core crate never gains filesystem,
network, terminal, or provider dependencies.

## Rationale

This keeps contracts testable and prevents provider or interface details from
spreading through the product. One Rust binary keeps installation simple on
all three target operating systems.

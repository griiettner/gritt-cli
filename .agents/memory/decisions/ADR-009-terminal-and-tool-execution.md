---
id: ADR-009
title: Terminal interface and tool execution
status: accepted
date: 2026-09-04
tags:
  - terminal
  - tools
  - permissions
read_when:
  - building the terminal harness
  - adding a native tool
  - changing approval or cancellation behavior
---

# ADR-009: Terminal interface and tool execution

## Decision

The full-screen terminal interface uses Ratatui 0.30.2 with its default
Crossterm 0.29 backend. Both are MIT licensed and support macOS, Windows, and
Linux. Ratatui is the maintained successor to `tui-rs`; the project reuses
the crates and their widgets, rendering, terminal setup, and event handling,
not application code from another project.

Print mode is always available and scriptable. REPL mode adds history and
continuation. The full-screen mode adds streamed transcript, tool activity,
multiline input, status, approvals, diff review, cancellation, command
palette, and task views.

Native tools are workspace-bounded file read and write plus shell execution.
The policy engine returns `allow`, `ask`, or `deny` based on tool and resource,
with workspace-aware wildcard rules, before every native execution. Child
processes are tracked and cancellation terminates them.

## Rationale

This stack is maintained, cross-platform, open source, and can be reused
without committing the product to a desktop UI. Print mode keeps every feature
usable in automation.

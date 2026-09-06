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

Native tools are workspace-bounded file read and write plus shell execution by default.
The policy engine returns `allow`, `ask`, or `deny` based on tool and resource,
with workspace-aware wildcard rules, before every native execution. Child
processes are tracked and cancellation terminates them.

## Execution modes (2026-09-06)

The user requested four native modes, shared by print, REPL, and the TUI.

- Planning exposes only `file_read` inside the workspace. The execution gate
  rejects writes, shell calls, and MCP calls even if a provider emits them.
- Supervised follows configured policy and asks through the interface for
  `ask` outcomes. A non-interactive interface denies unanswered prompts.
- Auto Approve accepts `ask` outcomes while preserving `deny` and workspace
  file boundaries.
- Full Access is an explicit override in the policy engine. It permits
  otherwise denied native and discovered MCP calls and file access outside
  the workspace, subject to OS permissions. MCP server trust, tool identity
  checks, secret redaction, and cancellation still apply.

`ExecutionMode` is a core value. The native driver owns its application;
interfaces select it only between turns. Modes derive the persisted planning
or coding phase. Elevated authority is not restored from history. A launch
flag or a new mode selection is required, and resumed provider history is
told the effective mode before the next turn. External connectors retain
their own permission controls and refuse the native mode picker.

The alternative of storing elevated authority on the session was rejected
because resuming a transcript should not silently enable Full Access.

## Rationale

This stack is maintained, cross-platform, open source, and can be reused
without committing the product to a desktop UI. Print mode keeps every feature
usable in automation.

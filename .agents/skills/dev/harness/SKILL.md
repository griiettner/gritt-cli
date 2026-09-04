---
name: dev-harness
description: Builds the terminal harness, permission engine, sessions, built-in tools, and connectors. Use when touching the TUI, approvals, cancellation, session storage, or an external agent connector.
---

# Harness

Read [dev](../SKILL.md) first. The plan's "Harness and interface" and "Connectors" sections define scope and order. Do not build a later milestone before the earlier one is reliable.

## Milestones

1. Phase 1: streamed transcript, tool approval, command cancellation, session resume, print mode.
2. Phase 3: full-screen navigation, diff review before file writes, command palette, task views, child sessions.
3. Phase 4: connector control plane. Native path first, then Codex and Claude Code, then Cursor and OpenCode after their interfaces are evaluated.

Autonomous background work waits until cancellation, permissions, and session recovery are proven.

## Two paths, one interface

- Native path: Gritt owns the loop. The permission engine gates every tool call, and built-in tools execute inside Gritt.
- Connector path: the external agent owns the loop. Gritt supervises the process, converts its output to events, relays its approval requests to the user, and records decisions. Gritt does not re-run the agent's tools or second-guess its permissions.

The transcript, status bar, session list, and history treat both the same, with a source field on every event.

## Permission engine

- Outcomes are `allow`, `ask`, `deny`. Match on tool name and resource. Support wildcard resource rules.
- Workspace-aware defaults: file reads inside the workspace allow, file writes ask, shell ask, network ask, destructive operations ask with a stronger prompt, anything outside the workspace deny.
- The engine runs before execution, every time, on the native path. No tool may bypass it.
- Every prompt shows the tool, the target, the relevant arguments, and a one-line reason.
- The transcript records the decision. It does not record sensitive argument values unless logging is opted in.

## Built-in tools

First version is two tools: file read and write within the configured workspace, and shell execution under approval. Keep them narrow. A provider-native tool is deferred unless it improves reliability without making a session provider-specific.

Child processes Gritt starts are tracked so cancellation stops them. A tool that leaves an orphan process is a bug.

## Sessions

- Sessions are named, listable, resumable, removable, and owned by Gritt, whichever path produced them.
- Store the continuation state an adapter or connector needs behind the session interface. Nothing above the interface knows the field name.
- Store local conversation and tool metadata beside it.
- Leave room for child sessions in the model from the start. Do not implement them before Phase 3.

## Terminal UI

- Print mode is the fallback and must never break. Every harness feature degrades to it.
- Pick the TUI crate in a Phase 0 ticket after the reference study. Record accessibility and platform findings there. Do not pick one in passing.
- Keep normal shell use and agent mode distinct. The user launches Gritt from a terminal, returns to the shell, or runs a standalone full-screen session.
- Status bar shows model, provider profile or connector, session, token usage when available, and connection state.
- Key entry in the interface writes the keychain and echoes nothing.
- Honor terminal resize, `NO_COLOR`, and reduced-motion preferences where the crate allows it.

## Connectors

The connector contract is a normalized event stream, not a wrapper around one SDK:

- start a task with a prompt and workspace, send follow-up input, stream events, answer an approval, cancel, resume or inspect when supported, and report capabilities, version, and auth state
- the native path implements the same contract so the control plane never special-cases it
- external connectors launch the installed agent through a PTY or its machine-readable interface. Prefer structured output. Terminal scraping is the last resort.
- every connector is optional. A missing or outdated agent never breaks the native path.
- show capability differences instead of faking parity. Keep raw connector metadata for troubleshooting.

## Verify

Run the [cli](../cli/SKILL.md) verification set. Add for this domain:

- a permission test per outcome and per default rule
- a cancellation test that proves the request and any child process stopped
- a session round-trip test through the store interface for a native and a connector session
- a connector test against a fake agent process that emits scripted output
- a manual pass in a real terminal for any UI change: resize, keyboard-only navigation, and the plain print mode path

## Output

Milestone the work belongs to, which path it affects, permission or session behavior changed, tests added, and any terminal or platform limitation observed. Update the ticket report when the work is ticket-driven.

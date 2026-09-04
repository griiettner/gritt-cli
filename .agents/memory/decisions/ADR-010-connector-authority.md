---
id: ADR-010
title: Connector authority and supervision
status: accepted
date: 2026-09-04
tags:
  - connectors
  - supervision
  - approvals
read_when:
  - adding an external agent connector
  - changing PTY or machine-readable process handling
  - deciding who owns a connector approval
---

# ADR-010: Connector authority and supervision

## Decision

External agents retain their own harness authority and can run commands and
tools exactly as their documented product mode permits. Gritt supervises the
process, normalizes its events, displays capability and auth state, relays
approval requests, records decisions, and handles follow-up input, resume,
cancellation, health checks, and process failure.

Connectors prefer documented machine-readable protocols. PTY transport is the
fallback. Terminal scraping is the last resort. A missing or outdated external
agent never breaks the native path.

The connector order is native, Codex, Claude Code, then Cursor and OpenCode.

## Rationale

This preserves the behavior users expect from Claude Code, Codex, and similar
agents while keeping Gritt responsible for supervision and shared history.

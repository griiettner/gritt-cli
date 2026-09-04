---
id: TKT-0008
namespace: griiettner
title: Build the complete Gritt local AI coding agent CLI
artifact: concept
status: concept
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: orchestrator
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0008 Concept: Build the complete Gritt local AI coding agent CLI

## Problem

Gritt currently has planning material and repository tooling, but no complete product path for running native and installed AI coding agents from one local interface. The project needs a shippable terminal CLI that keeps provider details, permissions, sessions, connectors, and local memory under one coherent contract.

## Intent

Deliver every phase in `plan1.md`: a Rust terminal CLI for macOS, Windows, and Linux with native provider sessions, supervised external connectors, workspace-bounded tools, planning and coding phases, and reproducible release builds. The implementation stays local except for configured model endpoints and connector traffic. Memory, sessions, telemetry, and analytics use one embedded Turso/libSQL database with separate table namespaces.

## Success Criteria

- A new user can install one reproducible binary, configure a provider, plan work, approve tools, run coding tasks, resume sessions, and inspect content-free telemetry locally.
- OpenRouter, OpenAI, Anthropic, and generic OpenAI-compatible profiles work through provider-neutral events, with daily model-cache refresh and stale fallback.
- Native, Codex, and Claude Code paths run through the same session and approval model, with live connector tests where the CLIs are installed.
- `gritt-agent` tooling and the Gritt product share one embedded database without leaking secrets or requiring Gritt Cloud or Turso Cloud.

---
id: TKT-0006
namespace: griiettner
title: Add engineering discipline and agent handoff skills
artifact: concept
status: concept
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
areas:
  - .agents/skills
  - .agents/cli
skills:
  - skill-management
  - dev-cli
  - tkt-plan
---

# TKT-0006 Concept: Add engineering discipline and agent handoff skills

## Problem

Gritt has lifecycle and architecture skills, but it lacks reusable engineering
disciplines for ambiguity reduction, domain language, test-first work,
diagnosis, design quality, session handoff, and agent-facing documentation.

## Intent

Add focused, composable skills adapted from proven engineering workflows while
keeping Gritt's ticket, memory, Rust, and invocation-policy boundaries.

## Success Criteria

- The recommended engineering disciplines are invocable and routed through
  Gritt's canonical `.agents/skills/` tree.
- Review can run independent standards and specification passes.
- All new skills have explicit triggers, completion criteria, and outputs.
- Adapters, indexes, audits, and validation remain clean.

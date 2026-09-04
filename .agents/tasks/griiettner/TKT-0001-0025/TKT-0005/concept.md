---
id: TKT-0005
namespace: griiettner
title: Strengthen skills with audits, control loops, feedback, and visual explanations
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
  - write-docs
---

# TKT-0005 Concept: Strengthen skills with audits, control loops, feedback, and visual explanations

## Problem

The repository has strong skill governance but lacks a semantic skill audit,
standard response contracts, a reusable recurring-maintenance loop, and a
small visual explanation workflow. These gaps make skill quality depend on
manual review and make recurring agent work harder to reproduce safely.

## Intent

Add deterministic tooling and reusable skills that cover those gaps while
preserving `.agents/skills/` as the canonical source and keeping generated
adapters intact.

## Success Criteria

- `gritt-agent skill audit` reports metadata, reference, and completion-contract
  problems with useful file paths and supports strict mode.
- New control-loop and visual-explanation skills provide local-first,
  checkable procedures and reusable references.
- Existing skill-management guidance documents response templates, feedback
  memory, and the audit command.
- Generated Claude and Codex adapters are synchronized and all CLI tests pass.

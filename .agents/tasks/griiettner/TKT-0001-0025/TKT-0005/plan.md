---
id: TKT-0005
namespace: griiettner
title: Strengthen skills with audits, control loops, feedback, and visual explanations
artifact: plan
status: done
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

# TKT-0005 Plan: Strengthen skills with audits, control loops, feedback, and visual explanations

## Sequence

1. Add the CLI audit as a read-only command with unit and integration tests.
2. Add the control-loop and visual-explanation skills plus reusable references.
3. Update skill-management guidance and regenerate adapters and indexes.
4. Run the CLI and repository verification set.

## Decisions

- Audit warnings remain non-fatal by default so existing skills can be
  inventoried without a flag day. `--strict` makes warnings fail the command.
- The control-loop skill supplies templates and local execution gates, but does
  not install a live scheduled workflow without a concrete repository target,
  credentials, and a human-approved cadence.
- Visual explanations use Markdown-native diagrams by default and HTML only
  when the visual carries more information than a compact text diagram.

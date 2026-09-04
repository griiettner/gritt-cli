---
name: codebase-design
description: Evaluates module boundaries and public seams for deep, maintainable design. Use when adding an abstraction, moving responsibilities, or deciding where behavior belongs.
---

# Codebase design

Read architecture memory, relevant ADRs, callers, and tests before proposing a
new boundary.

## Workflow

1. State the behavior and the proposed public interface.
2. Trace current callers, data ownership, dependencies, and failure paths.
3. Test whether the module hides substantial behavior behind a small interface.
4. Compare at least two placements or interface shapes when the boundary is
   material. Prefer the option with fewer leaks and a clearer seam.
5. Check Gritt's provider-neutral, native/connector, secret, and crate-boundary
   rules.
6. Record only hard-to-reverse, surprising, trade-off-driven choices as ADRs.

## Completion criteria

- Ownership and dependency direction are explicit.
- The interface is testable without reaching into internals.
- Provider-specific or connector-specific behavior stays behind its adapter.
- Rejected alternatives and their costs are recorded when material.

## Output

Return the boundary diagram, callers, options considered, chosen seam, risks,
and whether an ADR or ticket update was warranted.

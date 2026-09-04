---
name: dev
description: Routes Gritt engineering work to a domain sub-skill. Use when implementing the Rust workspace, a provider adapter, the harness, or a connector.
disable-model-invocation: true
---

# Dev skills

Shared contract for implementation in this repository. **Read this file first**, then load **one** domain sub-skill. Do not load all domains.

Gritt is a local agent workspace written in Rust. A user either supplies a provider key and runs Gritt's native harness, or points Gritt at an installed agent and lets that agent's harness own the loop. The plan at [`.agents/plans/plan1.md`](../../plans/plan1.md) is the contract. Its scope and phases are locked. Its "Open questions" are the user's decisions. Do not settle one silently. Put the decision in a ticket `plan.md` first.

## Common rules

- Rust only for the product. No runtime dependency at install time. Node exists in this repository only for the `.agents/tools/` maintenance scripts.
- The provider-neutral boundary is hard. Code above an adapter never learns which provider served a request. Provider-specific fields travel only as optional diagnostic metadata on events.
- Routing is by configured provider profile. Never infer a provider from a model name.
- Capabilities come from the provider's model list. Do not advertise a feature the provider does not report, and refuse an explicitly requested unsupported feature with a clear error.
- Native and connector sessions produce the same event types. A feature that only works for one path is flagged as such in the interface, not hidden.
- Keys live in the keychain or the environment. Config files name the variable, never the value. Keys never enter logs, fixtures, errors, or transcripts. Content logging is opt-in and off by default.
- Reference projects (OpenCode, Warp, T3 Code) are for behavior study. Reimplement. Record any borrowed snippet and its license in the ticket.
- Edit surgically. When a change touches a few lines, edit those lines rather than rewriting the file. Rewrite a file only when it is short or most of it is changing.
- Work the plan phases in order. Do not start a later phase while an earlier exit criterion is open.

## Sub-skills

Nested under `dev/`. Not separately invocable. Load on demand:

| Sub-skill | Load when |
| --- | --- |
| [cli](cli/SKILL.md) | Cargo workspace layout, crate boundaries, config precedence, key loading, print and REPL modes, error reporting, verification |
| [provider](provider/SKILL.md) | Provider adapters, model list cache, Chat Completions, Responses, and Messages normalizers, SSE parsing, tool schema generation, recorded fixtures |
| [harness](harness/SKILL.md) | Terminal UI, permission policy, sessions, built-in tools, cancellation, and the connector control plane |

Routing metadata: [`index.yaml`](index.yaml).

## How to use

1. Read this file.
2. Match the task to one row in the table.
3. Load that sub-skill only.
4. When the work is ticket-driven, [tkt](../tkt/SKILL.md) owns the lifecycle and the report. This family owns the code.

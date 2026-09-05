---
id: ADR-013
title: Reasoning effort, the MCP client runtime, and TUI runtime seams
status: proposed
date: 2026-09-05
tags:
  - providers
  - sessions
  - mcp
  - permissions
  - tui
read_when:
  - adding or changing a reasoning effort mapping for a protocol
  - changing MCP trust, dispatch names, or the tool permission default
  - changing what the binary injects into the full-screen runtime
---

# ADR-013: Reasoning effort, the MCP client runtime, and TUI runtime seams

Extends ADR-007, ADR-008, ADR-009, and ADR-012. The TKT-0015 chain
(TKT-0016 through TKT-0020) accepted four durable contract changes while
building the full-screen agent workspace. They are recorded here because
later work has to follow them, not because the chain reopened those ADRs.

## Decision

### 1. Reasoning effort is provider-neutral, and its mapping is per protocol

Effort is a Gritt concept with four values: `auto`, `low`, `medium`, `high`.
It travels on `RequestOptions.effort`, is stored on `SessionKind::Native`,
and is `None` for connector sessions, which manage their own. `auto` sends
no field on any protocol.

One function decides whether an explicit level may be sent, and both the
provider adapters and the harness draft validator call it, so they refuse
the same cases for the same typed reason:

- Responses sends `reasoning.effort` unless the model list reports the model
  without reasoning support.
- Chat Completions has no protocol-level effort field. The OpenRouter form
  is sent only when the list reports reasoning support for that model.
  Unreported support is not a safe mapping and is refused.
- Messages refuses every explicit level. The older `thinking` budget and the
  newer `output_config.effort` are accepted by disjoint sets of models, and
  Anthropic's list carries no capability flags to route on.
- When a list names explicit levels, only those are accepted, on any
  protocol.

A refusal is an unsupported-capability error raised before a request is
sent, carrying a typed reason. A level is never inferred from a model name.
The legacy boolean `reasoning` means "on at the provider's default level"
and does not stand in for `medium`; combined with an explicit level it sends
the level, and `false` with an explicit level is a configuration error.

### 2. The MCP client runtime belongs to the harness

Gritt is an MCP client as well as an MCP server. The client runtime lives in
`gritt-harness`, one instance per workspace, and reads
`<workspace>/.mcp.json` without ever writing it. It is created for every
mode but launches nothing until a definition is approved, and it is opened
only on the native path.

Reading a file does not authorize running what it names. A trust record is
keyed on the workspace, the server name, and a fingerprint of the raw entry
before interpolation. Editing the entry changes the fingerprint, so the
approval no longer applies and Gritt asks again. Trust decisions are stored
in the session database, not in the workspace file. Approval is offered both
interactively and through `gritt mcp list|trust|forget`, over one typed
decision API.

Credential-looking fields must be exactly a `${VAR}` reference; a literal is
refused without echoing it. Children get a cleared environment plus a fixed
allowlist and the variables the entry declares, and run in their own process
group so cleanup reaches their descendants.

### 3. MCP tools pass the permission engine, with their own default and resource form

A discovered tool reaches the model as `mcp__<server>__<tool>` and its
permission resource is `mcp:<server>/<tool>`. The workspace policy defaults
gain one rule, `mcp__*` on any resource with outcome `ask`, placed after
`network` and before the catch-all deny. There is no bypass: an MCP call
passes the same engine as a native tool, and a server's own annotations are
display information that never grant permission.

The call that executes is the call that was approved. A tool is frozen for
the turn as its server, original name, and runtime generation, because a
reload can hand the same dispatch name to a different tool.

### 4. The binary injects setup, config reload, and workspace observation

The full-screen runtime stays a client of the control plane. Three seams let
the binary supply what only it may do, keeping configuration-layer merging
and keychain access out of the harness (ADR-006):

- provider setup: writing a profile to a config file and a key to the
  keychain, with the profile written first so a refused keychain still
  leaves a usable profile;
- config reload: saving a profile does not change the running configuration
  until the plane is rebuilt around a reloaded config;
- workspace observation: a harness service that reports changed files,
  including read-only `git` invocations through an injected runner. The
  invocation set is fixed and never interpolates user text into a command
  position.

MCP lifecycle reaches an interface by subscription, publishing the whole
snapshot list on every change, so nothing polls and a lagged subscriber
still converges on the current state.

## Rationale

Each of these was forced by a case the chain could not otherwise state
honestly. Effort is the clearest: the three protocols genuinely disagree,
and a uniform mapping would have to guess for Anthropic and for any model
whose list reports nothing. Refusing with a typed reason is the only option
that neither lies nor blocks the providers that do support it.

Trust keyed on a fingerprint exists because the alternative, trusting a
server by name, would let an edited `.mcp.json` run something else under an
approval the user already gave. The `ask` default for `mcp__*` exists
because leaving MCP under the catch-all deny would mean no MCP tool runs
until a user writes a rule by hand.

## Consequences

- Anthropic models cannot take an explicit effort level until the Models API
  capability data is parsed into reported levels. That is follow-up work,
  not a name-based guess.
- `/effort` on a cold start offers nothing explicit on Chat Completions,
  because the catalog has not arrived and unreported support refuses. The
  levels appear when the list loads.
- Runtime bounds (initialization and call timeouts, concurrency, result
  size, pagination) are settings on the runtime and are not yet exposed in
  `config.toml`. The 30 s initialization default fails a server that indexes
  before answering `initialize` on its first run in a fresh checkout.
- Gritt now runs `git`. It is read-only and fixed, but it is a new external
  dependency of the harness.
- A newer MCP revision than the three Gritt speaks is refused with a stated
  reason rather than attempted.

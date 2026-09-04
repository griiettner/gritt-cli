---
name: dev-cli
description: Builds the Gritt binary and Cargo workspace. Use when adding a crate, command, config option, key source, or run mode, or when verifying a change.
---

# CLI

Read [dev](../SKILL.md) first.

## Workspace

Default layout until a Phase 0 ticket records a different one. Create the crate before writing code in it, and keep the dependency direction one way:

```text
crates/
  gritt-core/       contracts: event, session, tool, config, adapter, connector types. No I/O.
  gritt-provider/   adapters, model list cache, SSE parser, normalizers. Depends on core.
  gritt-harness/    terminal UI, permission engine, session store, built-in tools. Depends on core and provider.
  gritt-connector/  process supervision and external agent connectors. Depends on core.
  gritt/            the binary: argument parsing, config and key loading, mode selection. Depends on everything.
```

`gritt-core` must compile with no network, filesystem, or terminal dependency. If a type needs `reqwest` or `tokio` to exist, it belongs one crate up.

Keep the workspace `Cargo.toml` as the single place for shared dependency versions. Do not pin a version in a member crate.

## Adding a dependency

1. Check license, maintenance, platform support, and transitive size on crates.io or docs.rs. Recognizing a crate name is not knowing its current state; do not take a version, API, or maintenance status from memory. Record the check in the ticket.
2. Prefer a small standalone crate over an application-level crate lifted from a reference project.
3. No Git dependencies. Registry versions only.

## Config precedence

Implement exactly this order and no other source:

1. command-line flags
2. project config, then user config
3. environment variables
4. built-in defaults

The config file holds provider profiles (protocol, base URL, key variable name), model aliases, default model and provider, list refresh policy, tool policy, connector settings, and interface preferences. It never holds a key value. Fail loudly if one is found.

## Keys

Resolve a key in this order: operating system keychain entry for the profile, then the environment variable the profile names. Nothing else. Entering a key through the interface writes the keychain, never a file. Redact keys in every error and debug path before the value can reach a formatter.

## Modes

- Print mode: one prompt in, streamed text out, exit code reflects the result. Scriptable. No full-screen UI. Every feature must work here first.
- REPL mode: interactive loop with history and session continuation.
- Full-screen harness: [harness](../harness/SKILL.md).

## Errors

- Provider errors keep the provider's error body in the diagnostic payload and show a one-line human message.
- Unsupported capability is its own error kind, raised before the request is sent.
- Stale or missing model list is its own error kind. Say whether Gritt fell back to the cached list.
- Missing key is its own error kind and names the profile and the variable it looked for, never a value.
- Never print a key, a full prompt, or tool content in an error by default.

## Verify

Run the narrowest set that covers the change, then the workspace set before handoff:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

Contract tests against recorded fixtures run in `cargo test`. Live tests run only with `GRITT_LIVE_TESTS=1` and a key for the selected profile, and are never required for a pass.

## Output

Name the crate and module touched, the plan phase the work lands in, the exact commands run, and any dependency added with its license. Update the ticket report when the work is ticket-driven.

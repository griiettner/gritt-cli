---
id: TKT-0001
namespace: griiettner
title: Build project-local agent CLI
artifact: concept
status: concept
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0001 concept: Build project-local agent CLI

## Problem

Repository maintenance and local memory depend on a collection of Node scripts.
Most ticket tools use Node built-ins, while local memory also requires packages
that are not installed or declared in this checkout. Agents cannot query local
memory, and maintaining a separate JavaScript toolchain conflicts with the
project's goal of using a small, local CLI.

## Intent

Create one project-local Rust binary for the repository operations agents need
at session start and during ticket work. The CLI should run without Node or
Python and keep its source, binary, database, and configuration inside the
repository.

## Success criteria

- An agent can index and search local project memory from the terminal.
- The same binary can serve local memory through MCP over standard input and output.
- Core ticket and skill metadata operations no longer require Node.
- The CLI is independent from the future Gritt product workspace.
- Existing scripts are removed only after command and fixture parity is proven.

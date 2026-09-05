---
id: TKT-0020
namespace: griiettner
title: Reject stale startup profile results after configuration reload
artifact: update
status: done
owner: griiettner
created: 2026-09-05
updated: 2026-09-05
chain_role: worker
chain_parent: TKT-0015
---

# TKT-0020 Update: reject stale startup profile results after configuration reload

## Trigger

The chain's final integrated review, TKT-0021, returned `needs-fix` on one
confirmed Medium finding against the merged result at `421a8b7`. Every other
parent and child criterion it assessed was met or explained. This is that fix
and its regression, on a new branch as the chain contract requires.

## The defect

Startup resolves credential availability for every configured profile.
`keys.key()` reaches the operating system keychain, which can block for as
long as it likes, so the work is spawned and the result arrives later as
`UiMsg::Profiles`. The task captures the control plane when it starts.

Meanwhile a user can run `/connect`, configure a provider, and save it.
Setup re-reads the configuration, rebuilds the plane around it, and installs
both through `UiMsg::Reloaded`.

Nothing ordered those two. `UiMsg::Profiles` was applied unconditionally, so
a startup lookup that finished after the reload replaced the list the reload
had just installed. The consequences are not cosmetic:

- the provider the user just configured disappears from `/connect`;
- `/effort` stops offering explicit levels for it, because it looks the
  provider's protocol up in `app.profiles` and no longer finds it;
- reopening either picker does not help, because neither re-reads profiles.

The window is exactly as long as the keychain takes, which is the one part
of startup with no bound.

## The change

`Runtime` carries a `config_generation`, starting at zero and moving only
when a reload replaces the plane. Every producer of `UiMsg::Profiles` tags
its result with the generation it was computed against: `load_profiles`
captures it beside the plane it is about to read, and the setup task captures
it before it starts. The handler refuses a result whose generation is no
longer current, and `UiMsg::Reloaded` bumps the generation as it installs the
new plane and list.

This is the same shape as the late-result guards the chain already uses for
sessions, catalogs, and ordinary operations: the result carries the identity
of the request it answers, and anything the interface has moved past is
dropped rather than applied.

## Validation

`a_profile_lookup_from_before_a_reload_is_refused` runs the two messages in
the defect's own order: the refreshed configuration is applied first, and the
older startup response is delivered after it. It asserts the reload's list
survives, and then that a lookup issued against the current generation still
applies, so the guard cannot pass by refusing everything. Verified to fail
with the guard removed: "a profile lookup from before the reload replaced the
list it installed".

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass, no warnings |
| `cargo test --workspace --no-fail-fast` | 499 passed, 0 failed |
| `cargo test -p gritt --test tui_pty` | 13 passed, 0 failed |

Workspace tests moved from 498 to 499. Benchmarks were not rerun: this change
adds a comparison on a message that arrives a handful of times per session
and touches nothing on the render or scheduling path, so the recorded
measurements still describe the build.

## Remaining follow-up

Unchanged, and deliberately so: the report's benchmark and completion-gate
content is untouched by this update. The seven-item human real-terminal
walkthrough remains the outstanding external gate to closing the parent, and
it is the chain orchestrator's to close rather than a worker defect.

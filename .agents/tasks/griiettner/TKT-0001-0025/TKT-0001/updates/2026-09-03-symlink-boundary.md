---
id: TKT-0001
namespace: griiettner
title: Reject symlinks during repository traversal
artifact: update
status: done
owner: griiettner
created: 2026-09-03
updated: 2026-09-03
---

# TKT-0001 update: Reject symlinks during repository traversal

## Trigger

The ticket review found that shared filesystem traversal followed symlinked
files and directories. Memory indexing could store content from outside the
repository, and skill synchronization could write metadata through an external
directory symlink.

## Change

Directory enumeration now excludes symlinked directories. Memory indexing also
excludes symlinked files. This applies the same boundary to memory, ticket, and
skill traversal without changing ordinary repository behavior.

Regression tests cover an external file symlink during memory indexing and an
external skill-directory symlink during skill synchronization.

## Completion gate

- Acceptance: yes. Repository traversal no longer follows either reproduced
  symlink escape path.
- Scope: yes. The change is limited to shared traversal behavior, focused
  regression tests, and this update record.
- Validation: `cargo fmt`, `cargo clippy`, `cargo test`, and the release build
  pass. Ticket and skill generated metadata checks also pass.
- Security and safety: the fix prevents the confirmed external read and write.
  It adds no network access, dependency, or destructive operation.
- Regression risk: low. Legitimate symlinked repository content is no longer
  indexed or treated as a canonical ticket or skill directory.
- Follow-up: none for the reviewed defect.
- Assumptions: symlinked repository content is not canonical. This matches the
  existing rule that symlinked directories must not be traversed.

# Local database

Gritt keeps sessions, events, continuation state, telemetry, analytics,
and the optional content log in one embedded Turso/libSQL file. It is the
same engine and, inside a repository that has an `.agents/` folder, the
same file that the `gritt-agent` tooling uses for local memory (ADR-005,
TKT-0008 plan).

## Location

| Case | Path |
| --- | --- |
| `--database PATH` | that file |
| the workspace has `.agents/` | `<workspace>/.agents/brain/data/agent-memory.db` |
| otherwise | the user data directory, `gritt/gritt.db` |

`gritt doctor` prints the resolved path and which rule chose it.

## Namespaces

The two products never touch each other's tables.

| Namespace | Owner | Tables |
| --- | --- | --- |
| memory | `gritt-agent` | `documents`, `document_chunks`, `index_runs`, and their FTS indexes |
| product | `gritt` | `gritt_sessions`, `gritt_session_events`, `gritt_session_continuations`, `gritt_telemetry_events`, `gritt_analytics_records`, `gritt_content_log`, `gritt_schema_migrations` |

Product migrations are recorded in `gritt_schema_migrations` and applied
on open, in order, once each. They are additive: no migration drops or
rewrites existing rows. A test applies the memory schema first, then the
product migrations, and proves every memory object survives. The tooling's
own tests prove the memory commands keep their contract.

## Inspecting

```bash
gritt doctor          # path, applied migrations, memory namespace, row counts
gritt session list    # sessions with kind, phase, and last update
gritt session show X  # the event log of one session
gritt telemetry       # every telemetry and analytics row
```

Any SQLite-compatible client that understands the Turso file format can
open the file read-only for deeper inspection.

## Secrets

No key value is ever written. Session events, continuation state,
approval records, connector diagnostics, and content-log rows pass through
the key redactor before they are stored.

## Recovery

The database is generated state. If it is damaged, delete it: sessions
and telemetry are lost, canonical files are not. In a repository the
`gritt-agent memory index` command rebuilds the memory namespace.

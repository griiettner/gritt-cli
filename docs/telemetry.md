# Telemetry and analytics

Telemetry and analytics are local records in the product namespace of the
[local database](database.md). Nothing is uploaded. There is no Gritt
Cloud, no Turso Cloud, and no remote endpoint (ADR-005, ADR-008).

## What is recorded

`gritt_telemetry_events`: an event name (for example `turn`), the session
id, a timestamp, a duration in milliseconds, a status (completed, failed,
cancelled), and integer counters such as input and output tokens, tool
calls, and approvals.

`gritt_analytics_records`: a metric name (for example `tokens_total`), the
session id, a timestamp, an integer value, and string labels such as the
provider profile, model, connector id, or phase.

Connector sessions are recorded the same way.

## What is never recorded

Prompts, model output, file content, shell output, tool arguments, key
values, and transcript text. A test runs a session with a distinctive
prompt and asserts that no telemetry or analytics row contains it.

## The content log

Content logging is off by default. When `logging.content_logging = true`
is set in the config, prompts and responses are written to
`gritt_content_log` with the active keys redacted. Rows older than
`logging.content_retention_days` (default 7) are deleted every time the
database opens, whether or not content logging is still on, so turning it
off never preserves old content past the window.

```toml
[logging]
content_logging = false
content_retention_days = 7
```

## Reading it

```bash
gritt telemetry
gritt doctor
```

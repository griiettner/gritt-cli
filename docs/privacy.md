# Privacy boundary

Gritt is local software. This page states exactly what leaves the machine.

## Leaves the machine

- Requests to the provider endpoints you configured, carrying the prompt,
  the transcript needed for the turn, tool definitions, and tool results.
- Requests to the embedding or reranking endpoint, only when you enabled
  them through the environment (see [Embeddings and reranking](embeddings.md)).
- Whatever an external agent you launched through a connector sends to its
  own service, under its own account and policy.

## Never leaves the machine

- Sessions, events, telemetry, analytics, and the content log. They live
  in the [local database](database.md). There is no cloud sync, no
  remote telemetry, and no analytics upload.
- Provider keys. They stay in the operating system keychain or your
  environment and are redacted from every output path ([Keys](keys.md)).
- Model list caches, which are read from the provider but stored locally.

## Inside the machine

- Native file tools are confined to the workspace. Shell commands are not
  confined by the operating system and run with your authority; Gritt
  guards them with approvals, a stronger prompt for anything that reaches
  outside the workspace, and a credential-free child environment
  ([Tools and permissions](tools-and-permissions.md)).
- External agents keep their own tool authority and their own environment
  ([Connectors](connectors.md)).
- Structured logs are content-free by default. Content logging is opt-in
  with a seven-day retention ([Telemetry and analytics](telemetry.md)).

`gritt doctor` shows every configured endpoint, key availability, and
whether embeddings, reranking, and content logging are on.

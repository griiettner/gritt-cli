# Embeddings and reranking

Embedding and reranking providers are opt-in and disabled by default. They
are configured only through environment variables, matching the contract
in `.agents/brain/providers.md`; a project or user config file that
contains an `[embeddings]` or `[rerank]` section is refused, so a checked-in
file can never enable network activity.

| Variable | Meaning |
| --- | --- |
| `AGENT_EMBEDDING_PROVIDER` | Embedding model id, for example `text-embedding-3-small` |
| `AGENT_RERANK_PROVIDER` | Reranking model id, for example `rerank-3.5` |
| `AGENT_MEMORY_BASE_URL` | The OpenAI-compatible endpoint both use |
| `AGENT_MEMORY_API_KEY` | The variable holding the key; Gritt stores only the variable name |

A missing key, an empty value, and `none` all mean disabled. When a
capability is disabled no HTTP client is built for it and no request is
ever issued; a test proves it. When enabled, requests go only to the
configured base URL. `gritt doctor` reports whether each capability is
enabled.

Local memory retrieval in `gritt-agent` never depends on these providers:
Turso full-text search remains the baseline and any provider failure falls
back to it.

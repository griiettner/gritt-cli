---
id: TKT-0010
namespace: griiettner
title: Implement provider adapters, streaming normalizers, model caching, capability checks, opt-in embeddings and reranking
artifact: report
status: done
owner: griiettner
created: 2026-09-04
updated: 2026-09-04
chain_role: worker
chain_parent: TKT-0008
areas:
  - .agents/tasks
  - .agents/skills
skills:
  - tkt
  - tkt-exec-chain
---

# TKT-0010 Report: Implement provider adapters, streaming normalizers, model caching, capability checks, opt-in embeddings and reranking

## Summary

Worker 2 of the TKT-0008 chain. `gritt-provider` now implements the three
wire protocols behind the `ProviderAdapter` trait from TKT-0009: Chat
Completions (one implementation for OpenRouter, OpenAI in chat mode, and
generic endpoints), OpenAI Responses with `previous_response_id`
continuation, and Anthropic Messages. Each protocol has its own normalizer,
request builder, and tool-schema generator. The crate also holds the
`reqwest` transport, an incremental SSE parser, cancellation, the daily
model-list cache with stale fallback, alias and deprecation resolution, and
the opt-in embedding and rerank clients. Nothing above an adapter sees a
provider field except through the event `diagnostic`.

## Chain Evidence

- Base branch: `feature/tkt-0008-gritt-cli` at `d0adcb2` (PR #1 merge).
- Worktree: `/Users/griiettner/Projects/grittflow/gritt-cli-tkt-0010`.
- Branch: `tkt-0010-02-providers`.
- Commits: `1070cb9` (provider layer, fixtures, ticket artifacts) and the
  report update listed under Updates.
- PR: https://github.com/griiettner/gritt-cli/pull/2 into
  `feature/tkt-0008-gritt-cli`.
- `gritt-core` change: none. The TKT-0009 contracts were sufficient.

## What Landed

`crates/gritt-provider/src/`:

- `transport.rs`: `HttpTransport` trait, `HttpRequest` with a `HeaderValue`
  that keeps secrets redacted through `Debug`, `ReqwestTransport`, and
  `FixtureTransport` (replays recorded bodies in fixed-size chunks and
  records every request).
- `sse.rs`: byte-at-a-time `SseParser` and `sse_stream`. Handles `\n`,
  `\r`, and `\r\n` line endings, comments, multi-line `data:`, `event:`,
  `id:`, `[DONE]`, and chunk boundaries anywhere, including inside UTF-8.
- `cancel.rs`: `CancellationToken`. Cancelling drops the body stream and
  ends the event stream with `Cancelled`; a send after cancel fails with
  `ErrorKind::Cancelled`.
- `adapter.rs`: `AdapterContext`, `KeyProvider` (`StaticKey`, `EnvKeys`),
  `CapabilitySource` (`NoCapabilities`), the capability gate, provider error
  construction, `EventEmitter` with a monotonic sequence, the `Normalizer`
  trait, and `normalized_stream`, which runs a normalizer over the body and
  honors cancellation.
- `chat_completions.rs`, `responses.rs`, `messages.rs`: the adapters.
- `models.rs`: `ModelCache` (JSON per profile under
  `<user cache dir>/gritt/models/`, override path available), `fetch_models`,
  `load_models`, `parse_model_list`, and the in-memory `ModelCatalog` that
  adapters use as their capability source.
- `alias.rs`: `resolve` for qualified names, global aliases, per-profile
  aliases, and deprecated models.
- `embeddings.rs`: `EmbeddingClient` and `RerankClient` built only from the
  env-only config.

`crates/gritt-provider/tests/`: `contract.rs`, `models_cache.rs`,
`sse_tcp.rs`, `live.rs`, `common/mod.rs`, and `fixtures/` with a README and
seven files per protocol.

## Key Decisions

- Base URL conventions. OpenAI-compatible profiles include the version
  segment (`https://openrouter.ai/api/v1`) and use `{base}/chat/completions`,
  `{base}/responses`, `{base}/models`. Anthropic profiles are the API root
  (`https://api.anthropic.com`) and use `{base}/v1/messages` and
  `{base}/v1/models`.
- Capability gate. A model whose list entry reports `tools`,
  `structured_output`, or `reasoning` as `false` makes a request that asks
  for it fail with `UnsupportedCapability` before any HTTP call. Messages
  refuses structured output outright since the protocol has no
  response-format field.
- Recorded exception to the dev/provider rule that only reported support is
  advertised: a `None` capability does not block the request, because the
  OpenAI and Anthropic model lists report no capability flags at all and a
  strict `Some(true)` gate would refuse tools on every native profile. The
  PM approved this during the TKT-0010 review. The gap is visible instead: a
  `capability_warning` diagnostic naming the unreported features and the
  model is attached to the first event of that stream.
- Redaction. Every key resolved for a request is registered with the event
  emitter. Provider error messages and bodies, stream error elements, and
  event diagnostics are redacted against those keys before anything is
  retained, and a retained body is capped at 4096 characters. An endpoint
  that echoes a credential therefore cannot place it in an error,
  transcript, or telemetry record.
- Wire sequence. The Responses normalizer tracks `sequence_number` without
  reordering events. A gap or regression attaches a `sequence_warning`
  diagnostic to the events of that element, and the completion diagnostic
  carries `last_wire_sequence` and every warning.
- Cancellation covers the connection phase. `send_checked` selects the
  transport send against the token, so a stalled connect is dropped and the
  turn ends with the terminal `Cancelled` event. A turn started while the
  token is already cancelled ends the same way without a request. The token
  has a `reset` so a session can continue after a cancelled turn.
- Refresh throttling. The cache file records `last_attempt_at`. After a
  failed refresh the stale list is served without another fetch until the
  interval passes; `force_refresh` bypasses the throttle.
- Cache file names append an FNV-1a hash of the profile name, so `a/b` and
  `a_b` no longer share a file.
- Continuation state. Chat Completions and Messages store the wire-form
  conversation; Responses stores `previous_response_id` and the
  instructions. Every state carries the event sequence so a restored adapter
  keeps numbering monotonic. `restore` rejects state from another owner.
- Event diagnostics. Text and reasoning deltas carry no diagnostic; tool
  calls, usage, completion, and errors carry the protocol name, raw stop
  reason or status, response ids, and any skipped element types.
- Unknown stream elements are recorded in the completion diagnostic under
  `skipped` and never end the stream.
- Model cache and deprecation. Refresh at most once per
  `model_list.refresh_interval_secs` (default one day). Failed refresh with
  a cached list returns `Stale`; with `stale_fallback = false` it is
  `StaleModelList`; no cache is `MissingModelList`. Deprecated ids remap to
  the provider-declared `replaced_by` (also read from `replacement` and
  `deprecation.replacement`), then to a configured alias, then fail with a
  message naming the exact alias line to add.
- Prices per million tokens are rounded to a nanodollar so they survive a
  JSON round trip through the cache unchanged.
- Embeddings and reranking. Clients exist only when the env-only config has
  a model; the transport factory is not even called otherwise. Endpoints are
  `{AGENT_MEMORY_BASE_URL}/v1/embeddings` and `/v1/rerank`; rerank only
  reorders the given documents.
- OpenRouter attribution headers (`HTTP-Referer`, `X-Title`) are added only
  when the base URL is on `openrouter.ai`, inside the Chat Completions
  adapter.

## Dependency Checks

Versions and licenses verified with `cargo info` on 2026-09-04:

| Crate | Version | License | Features |
| --- | --- | --- | --- |
| reqwest | 0.13.4 | MIT OR Apache-2.0 | `rustls`, `http2`, `json`, `stream`; default features off |
| futures | 0.3.34 | MIT OR Apache-2.0 | |
| bytes | 1.12.1 | MIT | |
| tokio (already present) | 1.53.1 | MIT | added `time` feature |

`reqwest`'s `rustls` feature selects the `aws-lc-rs` crypto provider. That
builds from source with `cmake` absent on macOS here; TKT-0013 must confirm
it on the Windows and Linux release builders or switch to
`rustls-no-provider` plus `ring`.

## Assumptions

- Fixtures are hand-authored from the documented wire formats because no
  provider key is present on this machine. `tests/fixtures/README.md`
  records that and the replacement rule. Live tests exist for all three
  protocols behind `GRITT_LIVE_TESTS=1` and skip silently otherwise.
- The Chat Completions `reasoning` request field uses the OpenRouter shape
  and is only sent when the model list reports reasoning support.
- The Messages thinking budget is a fixed 1024 tokens when reasoning is
  requested; the harness can raise it later through `RequestOptions`.
- Model list parsing reads OpenRouter's `supported_parameters`,
  `architecture.input_modalities`, `context_length`, and `pricing`. Plain
  OpenAI and Anthropic entries produce all-`None` capabilities.

## Edge Cases and Failures

- The first cache test failed because `0.00000005 * 1e6` is
  `0.049999999999999996`, which serde_json wrote as `0.05`. Rounding the
  per-million price fixed it.
- `PartialToolCall` falls back to `call_<index>` when a provider omits the
  call id and to `{"_raw": "..."}` when arguments are not valid JSON, so a
  malformed tool call still surfaces as an event instead of a crash.

## Validation

All run from the worktree root on 2026-09-04:

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test -p gritt-provider`: pass, 44 tests after the review fix round
  (19 unit, 17 contract, 4 cache, 1 TCP end to end, 3 live tests that skip
  without keys). Before the fix round: 34.
- `cargo test --workspace`: pass, 70 tests after the fix round (60 before).
- `cargo build --release`: pass, 1m 34s.
- `gritt-agent ticket validate --repo-root .`: `tkt_validate ok (0
  warnings)`.
- `gritt-agent ticket chain-check --repo-root . --ticket TKT-0010 --base
  origin/feature/tkt-0008-gritt-cli`: `tkt_chain_check ok (0 warning(s))`,
  45 changed files, merge-base equal to the base tip `d0adcb2`. The remote
  ref was used because the local `feature/tkt-0008-gritt-cli` branch in the
  main checkout still pointed at `ab3e34a`; the PM should fast-forward it.
- `gritt-agent ticket chain-check --repo-root . --ticket TKT-0010 --base
  main`: `tkt_chain_check ok (3 warning(s))`. The warnings are the TKT-0009
  ticket files that reached the feature branch through PR #1 and the
  expected merge-base gap between `main` and the feature branch.

## Completion Gate

- Acceptance: yes. All three profiles stream the shared event model from
  fixtures; normalizers preserve tool calls, usage, continuation ids,
  errors, and diagnostics with no provider branch above the adapter; cache
  refresh is daily with marked stale fallback and deterministic alias
  remapping; embeddings and reranking are disabled by default and use only
  the configured environment endpoint.
- Scope: yes. No terminal UI, permission evaluation, tool execution,
  connector process code, or packaging was added. `gritt-core` is untouched.
- Validation: yes, as listed above.
- Security and safety: keys travel only as `Secret` values inside
  `HeaderValue::Secret`, which prints `[redacted]`; tests assert no key
  reaches request debug output or error messages. Fixtures contain no keys.
  Network access happens only through configured profile URLs and the
  env-only gateway.
- Regression risk: low. The crate was a skeleton before; `gritt-harness`
  and `gritt` compile unchanged against it.
- Follow-up: see below.
- Assumptions: recorded above.

## Follow-up

- TKT-0011 wires `KeyResolver` from the binary into `KeyProvider`, loads the
  catalog at startup, and passes `RequestOptions` from the session.
- TKT-0011 calls `CancellationToken::reset` after draining a cancelled
  stream and before the next turn on the same adapter; without it every
  later turn ends with `Cancelled` immediately.
- TKT-0011 may persist event diagnostics as recorded; they are already
  redacted and capped at the adapter.
- TKT-0013 verifies the `aws-lc-rs` build on Windows and Linux.
- Replace hand-authored fixtures with redacted live recordings when a key is
  available.

## Updates

- 2026-09-04 review fix round. The PR #2 reviewer returned `needs-fix` with
  seven findings: provider bodies could carry an echoed key into errors and
  diagnostics; a failed refresh was retried on every load; unreported
  capabilities passed silently; Responses `sequence_number` was ignored;
  cancellation was not observed during connect; cache file names collided;
  and only the OpenRouter chat profile had fixture coverage. All seven are
  fixed in the second commit with tests for each, plus a resettable
  cancellation token for TKT-0011. The validation set was rerun and is
  recorded above.
- 2026-09-04 report update. Added the commit, PR #2, and chain-check
  evidence after the PR was opened. No code changed.

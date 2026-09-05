# Providers and models

## Profiles

A profile names a protocol, a base URL, and a key reference. Gritt routes
by profile only (ADR-007). Supported protocols:

| Protocol | `protocol` value | Serves |
| --- | --- | --- |
| OpenAI-compatible Chat Completions | `chat_completions` | OpenRouter, OpenAI in chat mode, any compatible endpoint |
| OpenAI Responses | `responses` | OpenAI |
| Anthropic Messages | `messages` | Anthropic |

Examples:

```toml
[profiles.openai]
name = "openai"
protocol = "responses"
base_url = "https://api.openai.com/v1"
[profiles.openai.key]
keychain_service_entry = "gritt/openai"
env_var_name = "OPENAI_API_KEY"

[profiles.anthropic]
name = "anthropic"
protocol = "messages"
base_url = "https://api.anthropic.com"
[profiles.anthropic.key]
keychain_service_entry = "gritt/anthropic"
env_var_name = "ANTHROPIC_API_KEY"

[profiles.local]
name = "local"
protocol = "chat_completions"
base_url = "http://127.0.0.1:8080/v1"
[profiles.local.key]
keychain_service_entry = "gritt/local"
env_var_name = "LOCAL_LLM_KEY"
```

OpenAI-compatible base URLs include `/v1`. The Anthropic base URL is the
API root; Gritt appends `/v1/messages` and `/v1/models`. Required headers
such as `anthropic-version` and OpenRouter attribution stay inside the
adapter. Nothing above an adapter learns which provider served a request;
provider-specific fields travel only as diagnostic metadata on events.

## Model lists and capabilities

Each profile's model list is fetched from the provider (`GET /models`, or
`GET /v1/models` for Anthropic) and cached per profile under the user cache
directory (`gritt/models/`). Refresh happens at most once per day. When a
refresh fails Gritt uses the last cached list and marks it stale; a failed
refresh is not retried until the interval passes. With no cache and a failed
fetch the model list is reported missing and capabilities are unreported.

Capabilities recorded from the list: context length, tools, vision,
structured output, reasoning, and pricing. Gritt does not fill gaps with
guesses. A feature the provider reports as unsupported is refused before a
request is sent. A feature the provider does not report at all is allowed
and flagged with a `capability_warning` diagnostic on the first event of the
turn, because OpenAI and Anthropic lists report no capability flags.

Skip the list for one run with `--no-models`. `gritt doctor` shows each
profile's cache state.

## Reasoning effort

Effort is provider-neutral in Gritt: `auto`, `low`, `medium`, or `high`. It
is chosen with `/effort` in the full-screen mode, stored with a native
session, and sent through the provider adapter. `auto` means Gritt sends no
effort field at all, on every protocol, so it is always accepted.

The three explicit levels are not uniform across protocols, and Gritt refuses
a mapping it cannot make safely rather than claiming every level means the
same thing everywhere. One rule decides, and both the adapter and the
session-draft validator ask it, so they refuse the same cases for the same
stated reason:

| Protocol | Explicit level |
| --- | --- |
| Responses | Sent as `reasoning.effort`, unless the model list reports the model without reasoning support |
| Chat Completions | Sent as `reasoning.effort` only when the provider's list reports reasoning support for that model; refused when support is unreported |
| Messages | Refused. There is no field that is safe for every model, and Anthropic's list reports no capability flags to route on |

Chat Completions has no effort field in the protocol itself; what Gritt sends
is the OpenRouter form. Unreported support is not a safe mapping, so an
unreported model refuses an explicit level instead of guessing. On Messages
the older `thinking` budget is rejected by newer models and the newer
`output_config.effort` by older ones, and with nothing to route on but the
model name, every explicit level is refused. Guessing from a model name is
not done anywhere.

When a model list does name explicit levels, only those are accepted, on
every protocol. No list Gritt parses today reports them, so this rule is
inert until a provider starts publishing them.

A refusal happens before any request is sent. It is an unsupported-capability
error naming the model and the reason, and the reason travels in the
diagnostic so an interface can distinguish "this model does not reason" from
"this protocol cannot carry a level" from "the list offers other levels".
This is why `/effort` on a cold Chat Completions start offers nothing
explicit: the catalog has not arrived, so capabilities are unreported, and
the levels appear once it loads.

### The legacy reasoning switch

`RequestOptions` still carries a boolean `reasoning` from before effort
existed. The two combine as follows:

- `reasoning = true` with effort `auto` means reasoning on at the provider's
  default level. Responses sends `reasoning: {summary: "auto"}` and Chat
  Completions sends `reasoning: {enabled: true}`, the latter only when the
  list reports reasoning support. Messages keeps its `thinking` budget. It
  does not stand in for `medium`.
- `reasoning = true` with an explicit level sends the level, under the table
  above.
- `reasoning = false` with an explicit level is contradictory and is refused
  as a configuration error rather than resolved in either direction.

## Aliases and deprecated models

Aliases map a short name to a profile and model id:

```toml
[aliases]
fast = "openrouter/openai/gpt-5-nano"

[profiles.openrouter.aliases]
nano = "openai/gpt-5-nano"
```

An alias that resolves in more than one profile is an error. A deprecated
model remaps automatically to the replacement the provider declares in its
model list, then to an explicit entry in the alias map. When neither exists
Gritt refuses the alias with an error naming both options. Remapping is
deterministic and covered by tests.

## Errors

Provider errors keep the provider's body in the diagnostic payload, capped
and key-redacted, and show a one-line message. Unsupported capability,
stale model list, missing model list, and missing key are their own error
kinds. A missing-key error names the profile and the variable it looked
for, never a value.

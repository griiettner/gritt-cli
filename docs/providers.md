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

# Provider fixtures

Recorded-shape wire bodies replayed through the transport and normalizer
contract tests in `../contract.rs`.

Origin: no provider key exists on the machine that authored TKT-0010, so
every fixture was written by hand from the documented wire formats
(OpenAI Chat Completions streaming chunks, OpenAI Responses `response.*`
events, Anthropic Messages `message_*` and `content_block_*` events) rather
than captured from a live call. Identifiers, token counts, and model names
are placeholders. When a live recording replaces a file, keep the same name,
redact the key and any prompt content, and replace base64 blobs with a short
placeholder that notes the original length.

Layout: one folder per protocol, one file per case:

- `stream-text.sse`: plain streamed text with usage.
- `stream-reasoning.sse` or `stream-thinking.sse`: reasoning summary before text.
- `stream-tool-call.sse` or `stream-tool-use.sse`: a `file_read` tool call.
- `stream-tool-result.sse`: the turn that follows a submitted tool result.
- `stream-error.sse`: an error element inside the stream body.
- `error.json`: a non-2xx error body.
- `models.json`: the provider's model list response.

The Responses folder also has `stream-sequence-gap.sse`: `stream-text.sse`
with `sequence_number` jumping from 4 to 7, for the wire-sequence warning.

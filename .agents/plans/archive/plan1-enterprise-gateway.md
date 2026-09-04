
# Initial plan: unified enterprise GenAI CLI

## Objective

Build one team CLI that can call the model families exposed by PwC's internal GenAI gateway through a consistent command and configuration model.

The production target is the internal GenAI gateway. OpenRouter will be the local development substitute because the enterprise gateway is not available from this machine. The provider connection must remain configurable so local tests do not dictate the production design.

The first version should support OpenAI GPT models and Bedrock-hosted Claude models. Azure and Vertex model support should remain possible through the routing design, but should not be treated as complete until their gateway behavior is validated.

## Verified constraints

- The gateway base URL is `https://genai-sharedservice-americas.pwcinternal.com`.
- Authentication uses one API key, `GENAI_API_KEY`.
- The gateway runs on LiteLLM.
- It exposes an OpenAI-compatible Chat Completions endpoint and an OpenAI Responses endpoint.
- The enterprise inference endpoint is `POST {GENAI_BASE_URL}/v1/responses`. The catalog endpoint is separate and remains under `/genai/api/v1/models/?team_uuid=<team_uuid>`.
- The Responses endpoint has been tested successfully for plain text, tool calls, streaming, and `previous_response_id` chaining.
- A Responses request supports `tools`, `reasoning`, `store`, `previous_response_id`, `stream`, `temperature`, `max_tokens`, and `top_p`, subject to the selected model's catalog capabilities.
- Real responses use the standard Responses shape with top-level `output` items. Observed items include `reasoning`, `message` with `output_text`, and `function_call`.
- Invalid requests return a LiteLLM error envelope with `error.message`, `type`, `param`, `code`, and optional `provider_specific_fields`.
- Model IDs are namespaced by backend, including `openai.us.gpt-5.6`, `bedrock.anthropic.claude-sonnet-5`, `azure.text-embedding-3-small`, and `bedrock.cohere.rerank-3-5`.
- OpenAI SDK and OpenAI Agents SDK clients can use a custom `base_url` and match the gateway's OpenAI-style interface.
- Codex CLI works with OpenAI-native models through the gateway.
- Codex CLI fails with a Bedrock-routed Claude model because it sends `strict: true` in tool schemas and the LiteLLM Bedrock bridge forwards a field that Bedrock's native Anthropic interface rejects. This is a gateway compatibility issue, not a client setting that Codex can currently fix.
- Claude Agent SDK compatibility with this gateway is not yet established.

## Model catalog

The gateway's model catalog should be the runtime source of truth. It is available at:

`GET /genai/api/v1/models/?team_uuid=<team_uuid>`

The request uses the GenAI bearer token and returns a paginated response. The sample contains 172 model records, 135 aliases, 307 routable names, four providers, and nine model types.

Each model record provides routing and capability data, including:

- `model_name`, `canonical_model`, and `model_route_key`
- `provider`, `model_version`, and `region`
- input and output token limits
- model type and deployment count
- input and output cost per million tokens
- vision, function calling, system message, response schema, and PDF support
- the list of supported OpenAI parameters
- aliases and their routable names

The CLI should fetch and cache this catalog rather than hard-code the full list. The cache should have a clear refresh command, an expiry policy, and a safe fallback to the last known catalog. Model selection should use `model_name` and aliases as user-facing names, while `canonical_model` and `model_route_key` remain internal routing metadata unless the gateway requires them in requests.

The request builder should use `supported_openai_params` as a guard. It should omit optional fields that the selected model does not advertise and return a clear error when the user explicitly requests an unsupported feature. This is especially important for `reasoning`, `thinking`, `temperature`, tool fields, and response formats, which vary across the catalog.

## Recommended architecture

### Provider-neutral CLI layer

The CLI owns model selection by ID or alias, interactive REPL mode, non-interactive print mode, streaming output, conversation history, session lifecycle, tool approval, tool results, configuration, and error reporting. It must not expose whether a request is handled by the OpenAI SDK, Claude Agent SDK, or the local OpenRouter substitute.

### Model registry and routing

Use a model registry populated from the gateway catalog, with explicit backend metadata. Each entry identifies the public model ID, aliases, provider, provider adapter, endpoint mode, limits, supported parameters, capabilities, and compatibility restrictions.

The `provider` field is more reliable than a simple prefix rule. The catalog currently includes `azure`, `bedrock`, `openai`, and `vertex_ai`, with examples such as `azure.gpt-4.1`, `bedrock.anthropic.claude-opus-4-8`, `openai.us.gpt-5.6-terra`, and `vertex_ai.gemini-3.5-flash`. Prefix matching can provide an offline fallback, but an explicit catalog entry must win when a model needs special handling.

The registry should filter models by `model_type`, supported capabilities, access groups, and team visibility before presenting them to users. It should not advertise tools, vision, PDF input, structured output, or system messages unless the catalog reports support for the selected model.

### Provider adapters

Define a small internal adapter contract for sending prompts, receiving streamed events, submitting tool results, restoring sessions, and reporting provider errors.

Implement adapters in this order:

1. OpenAI adapter using the OpenAI SDK or OpenAI Agents SDK.
2. OpenRouter adapter for local testing. Reuse the OpenAI-compatible path where possible, with an OpenRouter base URL and key supplied through configuration.
3. Claude or Bedrock adapter after the tool-schema validation spike passes. Use Claude Agent SDK if it works with the gateway without a brittle translation layer. Otherwise evaluate a plain Anthropic SDK client or a deliberately small custom adapter.

Normalize messages and events at the adapter boundary. Do not force every provider capability to look identical. Report unsupported features clearly.

### Sessions and events

Represent streamed text, tool calls, tool results, usage, errors, and completion as provider-neutral events. Keep provider-specific fields as optional diagnostic metadata.

Start with session state owned by the CLI. If the Responses API's server-side response IDs are used, store them behind the same session interface. Send the top-level `id` from the prior response verbatim as the next `previous_response_id`. Do not rely on the response's echoed `previous_response_id` field because the gateway may reserialize it to a different value.

The response normalizer should convert `output` items, output text, function calls, finish status, usage, and reasoning summaries into the internal event model. Keep Chat Completions normalization separate because its `choices[].message` envelope is a different response contract.

For streaming, parse Server-Sent Events with `text/event-stream` and preserve event order and `sequence_number`. The confirmed event set includes `response.created`, `response.in_progress`, `response.output_item.added`, `response.output_item.done`, `response.content_part.added`, `response.output_text.delta`, `response.output_text.done`, `response.content_part.done`, and `response.completed`.

### Tools

The first version should provide a small built-in tool set:

- file read and write within the configured workspace
- shell command execution with approval controls

Tool definitions must be generated per adapter because accepted schemas differ across OpenAI-compatible and Anthropic or Bedrock paths. The CLI must not assume that OpenAI's `strict` field is accepted by a Bedrock-routed model.

Keep the built-in tools narrow. Defer provider-native tools unless they improve reliability without making sessions provider-specific.

## Early validation spike

Before committing to Claude Agent SDK as the Bedrock implementation, run a small compatibility spike against an environment that can reach the enterprise gateway. OpenRouter can validate the general adapter, routing, and streaming behavior, but it cannot prove compatibility with this gateway's LiteLLM-to-Bedrock bridge.

Test a Bedrock-routed Claude model with:

1. A plain text request without tools.
2. One minimal custom tool.
3. A tool call followed by a tool result.
4. Streaming text and streamed tool events.
5. Session continuation, if the selected SDK supports it.

Run the cases through Claude Agent SDK and, if necessary, a plain Anthropic SDK client. Capture the exact request and response shapes and rejected fields. The decision gate is whether the client can avoid unsupported OpenAI-only tool fields while retaining the tool behavior required by the CLI.

Possible outcomes are direct Claude Agent SDK support, text-only support, support through a small schema translation layer, or a documented gateway dependency. Do not hide a gateway limitation in the CLI.

## Language and runtime

Standalone distribution is now a primary requirement, and Rust is the recommended implementation language.

Rust provides a single native executable, avoids requiring a Python or Node runtime on managed machines, fits a terminal-first application, and gives the project strong control over concurrency, cancellation, process execution, and memory safety. It also makes it easier to ship one binary per target platform with a predictable upgrade path.

The cost is real. Rust does not provide the same direct access to the Python or TypeScript agent SDKs already researched. The CLI would own its HTTP clients, SSE parser, Responses and Chat Completions normalizers, tool loop, session handling, and provider adapters. That is more implementation work, but it also avoids coupling the product to SDK behavior that already differs across providers and has exposed the Bedrock tool-schema problem.

Python remains a valid fallback if the Bedrock spike shows that a vendor SDK is required for a capability Rust cannot reproduce safely. A two-process Rust shell with a Python sidecar should be treated as a last resort because it weakens the standalone installation goal.

### Initial recommendation

Use Rust for the CLI and implement the confirmed OpenAI-compatible protocols directly over HTTPS. Start with `POST /v1/responses`, its standard `output` items, and its SSE event taxonomy. Add a separate Chat Completions normalizer and keep Bedrock support behind the validation gate. Revisit Python only if a required provider capability cannot be implemented through the documented HTTP contracts.

## Terminal harness and interaction model

The CLI should feel like a terminal workspace for agent tasks, not only a prompt followed by printed text. The design can take useful patterns from OpenCode's agent and permission model and Warp's separation between normal terminal use and agent workflows. OpenCode documents child sessions, configurable tool permissions, and wildcard resource rules. Warp documents distinct terminal and agent modes.

Warp's public repository is a useful Rust reference implementation. Its workspace includes terminal, core, UI, TUI, logging, process, and agent-related crates, and its project documentation describes a shared core with separate GUI and headless TUI front ends. Study that code before selecting our own terminal stack. The project should copy functionality by writing an independent implementation, not copy substantial source code. Small snippets may be reused only when they are clearly understood and the license permits it. Reusing a crate is appropriate only after checking its license, maintenance status, platform assumptions, and dependency size. Warp's application crates may be tightly coupled to its own entity and UI systems, so the default expectation should be to reuse ideas and standalone dependencies, not import the full application architecture.

The initial reference study should inspect:

- headless TUI and cell-grid rendering
- terminal input, shell, PTY, and process lifecycle code
- workspace and session state management
- agent UI, tool activity, and approval flows
- logging, diagnostics, and integration test patterns
- the license for each candidate crate and its transitive dependencies
- a short design note for each borrowed behavior, showing how our implementation differs from the reference

### Core harness experience

Provide a full-screen terminal interface with:

- a conversation pane that streams text and tool activity
- a prompt editor with multiline input and history
- a compact status bar showing model, provider, session, token usage when available, and connection state
- a tool activity view showing command, arguments, result, duration, and approval status
- a diff or change review view before file writes are accepted
- cancellation that stops the active request and any child process the CLI started
- a command palette for model selection, session actions, configuration, and help
- a plain print mode that bypasses the full-screen interface for scripts

Keep normal shell use and agent mode distinct. The user should be able to launch the agent from an existing terminal, return to the shell, or run the agent as a standalone full-screen session.

### Permissions and safety

Implement a permission policy with `allow`, `ask`, and `deny` outcomes. Match policies against tool names and resources, with workspace-aware defaults and explicit confirmation for shell commands, file writes, network access, and destructive operations.

Every approval should show the tool, target, relevant arguments, and a concise reason. The policy engine must run before execution, and the transcript should record the decision without recording sensitive values by default.

### Sessions and task structure

Support named sessions that can be resumed, listed, and removed. Store the top-level Responses ID required for continuation, plus local conversation and tool metadata.

Add task-oriented controls after the basic loop works:

- create a task from a prompt
- pause, resume, or cancel a task
- inspect the current plan and completed actions
- open a child session for a focused subtask or reviewer
- return from a child session with a compact result

Child sessions should be a later milestone than the single-agent loop, but the session model should leave room for them from the start.

### Harness milestones

The first harness release should include streaming transcript output, tool approval, command cancellation, session resume, and print mode. Add full-screen navigation, diff review, command palette, task views, and child sessions in the next milestone. Do not add autonomous background work until cancellation, permissions, and session recovery are reliable.

## Rust control plane and agent connectors

The project can grow into a Rust version of the T3 Code model, with a local control plane that presents multiple coding agents through one interface. T3 Code describes itself as a web and desktop GUI for running coding agents locally and currently lists Codex, Claude, Cursor, Grok Build, and OpenCode as supported agents. The proposed CLI would provide the native agent and would also connect to other installed agents.

### Control plane

Keep the control plane separate from any one agent implementation. It should own:

- workspace and project selection
- connector discovery and health checks
- thread and session management
- unified event storage
- permissions and approval display
- task status, cancellation, and retry controls
- transcript and tool activity rendering
- connector capability reporting

The control plane should not pretend that all agents have the same context model or tool behavior. It should show capability differences and preserve the raw connector metadata for troubleshooting.

### Connector contract

Define a connector contract around normalized events rather than around a specific SDK. A connector should expose:

- start a task with a prompt and workspace
- send follow-up input
- stream text, reasoning summaries, tool calls, tool results, approvals, and status changes
- accept or reject an approval request
- cancel the active task
- resume or inspect a session when the underlying agent supports it
- report capabilities, version, authentication state, and limitations

The native GenAI connector should call the Rust HTTP adapters directly. External connectors should launch and supervise installed CLIs through PTYs or their documented machine-readable interfaces. Prefer structured output and local APIs when an agent provides them. Use terminal scraping only as a compatibility fallback.

### Initial connectors

1. GenAI connector. Native Rust implementation for the enterprise gateway and OpenRouter profile.
2. Codex connector. Supervise the installed Codex CLI and map its output, approvals, sessions, and errors into the common event model.
3. Claude Code connector. Use its supported CLI or SDK integration and document which session and permission features are available.
4. Cursor connector. Validate the available command-line interface before committing to the connector. Cursor may expose less control than a purpose-built agent CLI.
5. OpenCode connector. Add when its machine-readable interface and permission behavior can be integrated cleanly.

Each connector should be optional. A missing or outdated external CLI must not prevent the native GenAI connector from working.

### T3 Code style application

Use the T3 Code pattern of keeping agent execution local while the UI manages multiple threads and tools. Recreate the needed behavior in Rust rather than copying T3 Code source. Start with a terminal control plane because it shares the existing harness work. A desktop front end can use the same control-plane API later.

### Difficulty assessment

The native connector is moderate work because the gateway protocol is already known. External connectors are harder because each one may differ in output format, authentication, session persistence, tool approval, cancellation, and context handling. The largest risk is not drawing the UI. It is maintaining reliable translations between agents that were not designed to be controlled by the same host.

Treat connector parity as capability based. The UI can show a common task timeline for every connector, while advanced controls appear only when the connector supports them.

## Configuration and model naming

Follow the existing `AGENT_MODEL_*` naming convention in the organization's `.agents/.env` files. Use this precedence:

1. command-line flags
2. project or user config file
3. `AGENT_MODEL_*` and provider environment variables
4. built-in defaults

The config file should support provider definitions, base URLs, environment-variable-based key names, team UUID, catalog endpoint, catalog refresh policy, model IDs, aliases, adapter selection, endpoint mode, capability overrides, default model, tool policy, shell approval settings, and an optional OpenRouter development profile.

Do not store API keys in the config file. The OpenRouter profile is for local development. The enterprise profile points to the GenAI gateway and uses `GENAI_API_KEY`.

## First-version scope

Include model catalog discovery and caching, model ID and alias selection, capability-aware model display, OpenRouter local testing, enterprise gateway configuration, non-interactive print mode, interactive REPL mode, streamed text, basic session history, file and shell tools with confirmation for risky operations, clear unsupported-capability errors, and structured logs for routing and provider failures.

Defer broad plugin systems, provider-specific UI behavior, embeddings and reranking commands, server-side session persistence beyond initial Responses API tests, automatic fallback after failed tool calls, advanced multi-agent orchestration, and reproducing every feature of Codex CLI or Claude Code.

## Phased milestones

### Phase 0: repository and protocol spike

- Confirm packaging, test, and supported operating-system conventions.
- Confirm Rust toolchain, cross-compilation, signing, and release requirements.
- Review the public Warp Rust repository and record which crates are reusable, reference-only, or unsuitable because of coupling or licensing.
- Define provider-neutral request, event, session, and tool contracts.
- Define the model catalog client, cache format, refresh behavior, and team visibility rules.
- Use `/v1/responses` as the confirmed enterprise inference path.
- Capture representative response fixtures for plain text, streaming, reasoning, tool calls, chaining, and LiteLLM errors.
- Prototype the Rust HTTPS client and SSE event parser against OpenRouter.
- Add an OpenRouter profile for local development.
- Record enterprise gateway assumptions without requiring access from this machine.

Exit criteria: Rust packaging is viable for the target platforms, the adapter contract is agreed, the catalog model is defined, the SSE parser handles the confirmed events, and a local request succeeds through OpenRouter.

### Phase 1: OpenAI path

- Implement the OpenAI-compatible adapter.
- Load the gateway catalog and route selected models by catalog provider metadata.
- Support OpenAI-native models through gateway configuration.
- Validate Chat Completions and Responses behavior, including request fields, model name normalization, response normalization, SSE event ordering, and `previous_response_id` handling.
- Implement print mode, REPL mode, streaming, basic sessions, and initial tool approval.
- Implement the first terminal harness with a streamed transcript, tool approval, and cancellation.
- Run the same tests against OpenRouter.

Exit criteria: OpenAI models work through one CLI interface in both modes, with documented differences between OpenRouter and the enterprise gateway.

### Phase 2: Claude and Bedrock path

- Run the compatibility spike against the enterprise gateway.
- Implement the selected Claude or Bedrock adapter.
- Add capability detection and tool-schema handling for the Bedrock route.
- Test text, streaming, tools, and continuation independently.

Exit criteria: the CLI supports the promised Bedrock feature set or reports unsupported cases explicitly with any required gateway change documented.

### Phase 3: harness, parity, and distribution

- Compare session behavior across adapters.
- Add catalog refresh, offline cache recovery, alias management, and capability-aware model selection.
- Close important gaps in streaming, tool calls, cancellation, usage reporting, and errors.
- Add full-screen navigation, diff review, command palette, named sessions, and task controls.
- Add Azure and Vertex routing only after endpoint behavior is verified.
- Finalize config defaults, documentation, upgrades, diagnostics, signing, and platform packaging.

Exit criteria: a new team member can install the CLI, configure a model alias, run both modes, and understand provider-specific limitations from the CLI output.

### Phase 4: connector control plane

- Define and test the connector contract and normalized event model.
- Promote the native GenAI implementation to the first connector.
- Add process supervision, PTY handling, timeouts, cancellation, and health checks for external connectors.
- Add Codex and Claude Code connectors first because their agent workflows are closest to the target use case.
- Evaluate Cursor and OpenCode interfaces before implementation.
- Add multiple threads, connector capability display, and cross-connector task history.
- Keep connector-specific limitations visible in the UI and documentation.

Exit criteria: the Rust control plane can run the native GenAI connector and at least two external agent connectors, show their activity in one interface, and recover cleanly from cancellation, process exit, and connector failure.

## Open questions

- Does the strict tool-schema failure affect Claude Agent SDK or a plain Anthropic client against this gateway?
- Can the gateway accept Anthropic or Bedrock requests directly, or does it require OpenAI-style requests for all model families?
- Which of Chat Completions and Responses should be the default for OpenAI-native models?
- Which SDK provides the best control over schemas, streaming, cancellation, and continuation?
- Which operating systems must the distribution support, and can the team use a single compiled binary?
- Can the Rust HTTP and SSE implementation cover the required OpenAI and Anthropic or Bedrock behavior without a vendor SDK?
- Which Rust terminal UI framework and process execution model meet the team's accessibility and platform requirements?
- Which Warp crates, if any, can be reused without taking on WarpUI or other application-level coupling?
- Does the license of each candidate Warp crate permit the intended internal distribution and future publication model?
- How should aliases and model deprecations be managed centrally?
- How long should the catalog cache remain valid, and should the CLI fail closed when it is stale?
- `model` is confirmed as the required inference field. `model_route_key` is not required by any tested request, but its status as an optional field remains unverified because the OpenAPI document was not accessible.
- Does a generic requested name such as `gpt-4o` get normalized by the gateway, or must the CLI resolve it to a catalog `model_name` before sending the request?
- Should cost and token limits from the catalog be displayed before a request or only used for local budgeting?
- Do Azure and Vertex-routed models preserve the same OpenAI-compatible behavior?
- What logging and data-handling rules apply to prompts, tool inputs, outputs, and provider errors?

## Risks and mitigations

### Bedrock tool-schema incompatibility

The Codex failure shows that an OpenAI-compatible request can still fail when LiteLLM translates tools to Bedrock. Validate the exact SDK request shape early, keep schema generation inside adapters, and do not claim tool parity until the spike passes.

### OpenRouter is not a gateway replica

OpenRouter can test routing, authentication configuration, streaming, and the provider-neutral CLI contract. It cannot prove the behavior of PwC's LiteLLM gateway or its Bedrock bridge. Keep gateway validation as a separate acceptance step.

### Azure and Vertex behavior is unknown

Backend prefixes do not guarantee protocol compatibility. Add these routes only after testing request formats, streaming, tools, and errors against the actual gateway.

### Catalog access and freshness

The model catalog requires a team UUID and bearer authentication. It may expose only the models available to the current team, and its contents can change independently of the CLI release. Handle pagination, access failures, expired caches, aliases, and removed models explicitly. Do not silently route a removed or inaccessible model using stale metadata.

### Two SDKs increase maintenance

The CLI will depend on SDKs with different release schedules and event models. Isolate them behind adapters, pin compatible versions, maintain contract tests, and expose capabilities instead of forcing false parity.

### Enterprise data handling

Prompts, tool inputs, and outputs may contain sensitive company data. Avoid logging content by default. Make diagnostic logging opt in, document retention behavior, and confirm enterprise requirements before adding telemetry.

### Distribution friction

A standalone binary is the target, but release signing, cross-platform builds, updates, and managed-machine trust still need early testing. Use reproducible release builds and publish checksums. Provide a package-manager route only as a secondary installation option.

### Rust protocol ownership

Implementing the agent loop directly in Rust reduces runtime dependencies but transfers responsibility for protocol compatibility from upstream SDKs to this project. Reduce the risk with recorded request and response fixtures, contract tests for every adapter, strict capability checks from the model catalog, and a small provider-neutral internal API.

### External connector fragility

CLI connectors depend on tools whose output formats, flags, authentication flows, and session behavior may change outside this project. Prefer documented JSON or event protocols, pin or check compatible versions, run connector health checks, and keep each connector isolated so one broken integration does not affect the native connector.

### Reusing Warp code

Warp's open-source repository may provide valuable reference code, but its workspace is large and some crates are part of a tightly connected application. Use it to understand behavior, interaction design, and engineering patterns. Recreate those behaviors in our own modules and avoid copying substantial source files, distinctive internal abstractions, or large code sections. Do not copy code or add Git dependencies until the specific crate license, API stability, platform support, and dependency graph have been reviewed. Prefer small, independently maintained crates when they meet the need, and keep a record of any small borrowed snippet and its license.

## Recommended starting point

Use Rust for a standalone, provider-neutral terminal CLI with a catalog-backed model registry and a direct OpenAI-compatible HTTP adapter as the first production path. Use OpenRouter as the local test profile. Build the terminal harness around streaming, permissions, sessions, task state, and reviewable tool activity. Treat Bedrock Claude support as a gated second phase, dependent on the compatibility spike. Keep the enterprise gateway, OpenRouter, and future Azure or Vertex routes as configuration profiles rather than separate command implementations.

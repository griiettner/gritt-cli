//! Replays every recorded fixture through the transport and normalizer and
//! asserts the provider-neutral event sequence for each protocol.

mod common;

use std::sync::Arc;

use common::*;
use gritt_core::event::{EventKind, StopReason};
use gritt_core::provider::{ModelCapabilities, Protocol};
use gritt_core::tool::{ToolCallId, ToolResult};
use gritt_core::ErrorKind;
use gritt_provider::{adapter_for, FixtureResponse};

const PROTOCOLS: [Protocol; 3] = [
    Protocol::ChatCompletions,
    Protocol::Responses,
    Protocol::Messages,
];

fn sse(protocol: Protocol, name: &str) -> FixtureResponse {
    FixtureResponse::sse(fixture(protocol_dir(protocol), name))
}

#[tokio::test]
async fn plain_text_streams_the_same_events_on_every_protocol_and_chunk_size() {
    for protocol in PROTOCOLS {
        let mut reference: Option<Vec<String>> = None;
        for chunk_size in [1usize, 7, 64, 4096] {
            let (context, transport, _) =
                make_context(protocol, vec![sse(protocol, "stream-text.sse")], chunk_size);
            let adapter = adapter_for(context);
            let stream = adapter.send(prompt(protocol, false)).await.unwrap();
            let events = collect(stream).await;
            assert_eq!(
                text_of(&events),
                "Hello, world",
                "{protocol:?} chunk {chunk_size}"
            );
            assert_monotonic(&events);
            let kinds = kinds(&events);
            assert_eq!(kinds.last().unwrap(), "completed:EndTurn", "{protocol:?}");
            assert!(kinds.contains(&"usage".to_string()), "{protocol:?}");
            match &reference {
                Some(reference) => assert_eq!(&kinds, reference, "{protocol:?} chunk {chunk_size}"),
                None => reference = Some(kinds),
            }
            let usage = events
                .iter()
                .find_map(|event| match &event.kind {
                    EventKind::Usage { usage } => Some(*usage),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                usage.input_tokens,
                Some(match protocol {
                    Protocol::Messages => 12,
                    _ => 10,
                })
            );
            let request = &transport.requests()[0];
            let body = request.body_json().unwrap();
            assert_eq!(body["model"], model_for(protocol));
            assert_eq!(body["stream"], true);
            let debug = format!("{request:?}");
            assert!(!debug.contains(TEST_KEY), "key leaked into request debug");
            assert!(debug.contains("[redacted]"));
        }
    }
}

#[tokio::test]
async fn request_shapes_keep_provider_quirks_inside_the_adapter() {
    let (context, transport, _) = make_context(
        Protocol::ChatCompletions,
        vec![sse(Protocol::ChatCompletions, "stream-text.sse")],
        64,
    );
    let adapter = adapter_for(context);
    collect(
        adapter
            .send(prompt(Protocol::ChatCompletions, true))
            .await
            .unwrap(),
    )
    .await;
    let request = &transport.requests()[0];
    assert_eq!(request.url, "https://openrouter.ai/api/v1/chat/completions");
    let names: Vec<&str> = request.headers.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"http-referer") && names.contains(&"x-title"));
    let body = request.body_json().unwrap();
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "file_read");
    assert!(body["tools"][0]["function"]["parameters"].is_object());

    let (context, transport, _) = make_context(
        Protocol::Responses,
        vec![sse(Protocol::Responses, "stream-text.sse")],
        64,
    );
    let adapter = adapter_for(context);
    collect(
        adapter
            .send(prompt(Protocol::Responses, true))
            .await
            .unwrap(),
    )
    .await;
    let request = &transport.requests()[0];
    assert_eq!(request.url, "https://api.openai.com/v1/responses");
    let body = request.body_json().unwrap();
    assert_eq!(body["instructions"], "You are terse.");
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["tools"][0]["name"], "file_read");
    assert!(body.get("previous_response_id").is_none());

    let (context, transport, _) = make_context(
        Protocol::Messages,
        vec![sse(Protocol::Messages, "stream-text.sse")],
        64,
    );
    let adapter = adapter_for(context);
    collect(
        adapter
            .send(prompt(Protocol::Messages, true))
            .await
            .unwrap(),
    )
    .await;
    let request = &transport.requests()[0];
    assert_eq!(request.url, "https://api.anthropic.com/v1/messages");
    let names: Vec<(&str, String)> = request
        .headers
        .iter()
        .map(|(n, v)| (n.as_str(), format!("{v:?}")))
        .collect();
    assert!(names.contains(&("anthropic-version", "2023-06-01".into())));
    assert!(names
        .iter()
        .any(|(n, v)| *n == "x-api-key" && v == "[redacted]"));
    let body = request.body_json().unwrap();
    assert_eq!(body["system"], "You are terse.");
    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(body["messages"][0]["role"], "user");
}

#[tokio::test]
async fn reasoning_summaries_are_normalized() {
    for (protocol, name) in [
        (Protocol::ChatCompletions, "stream-reasoning.sse"),
        (Protocol::Responses, "stream-reasoning.sse"),
        (Protocol::Messages, "stream-thinking.sse"),
    ] {
        let (context, _, _) = make_context(protocol, vec![sse(protocol, name)], 16);
        let adapter = adapter_for(context);
        let events = collect(adapter.send(prompt(protocol, false)).await.unwrap()).await;
        assert_eq!(
            reasoning_of(&events),
            "Plan: answer briefly.",
            "{protocol:?}"
        );
        assert_eq!(text_of(&events), "Brief answer.", "{protocol:?}");
        let kinds = kinds(&events);
        assert!(
            kinds.iter().position(|k| k == "reasoning") < kinds.iter().position(|k| k == "text"),
            "{protocol:?}: reasoning must precede text"
        );
        assert_eq!(kinds.last().unwrap(), "completed:EndTurn");
    }
}

#[tokio::test]
async fn tool_calls_then_tool_results_continue_the_conversation() {
    for (protocol, call_fixture, result_fixture) in [
        (
            Protocol::ChatCompletions,
            "stream-tool-call.sse",
            "stream-tool-result.sse",
        ),
        (
            Protocol::Responses,
            "stream-tool-call.sse",
            "stream-tool-result.sse",
        ),
        (
            Protocol::Messages,
            "stream-tool-use.sse",
            "stream-tool-result.sse",
        ),
    ] {
        let (context, transport, _) = make_context(
            protocol,
            vec![sse(protocol, call_fixture), sse(protocol, result_fixture)],
            5,
        );
        let adapter = adapter_for(context);
        let events = collect(adapter.send(prompt(protocol, true)).await.unwrap()).await;
        let call = events
            .iter()
            .find_map(|event| match &event.kind {
                EventKind::ToolCall { call } => Some(call.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{protocol:?}: no tool call"));
        assert_eq!(call.name, "file_read");
        assert_eq!(call.arguments["path"], "README.md");
        assert!(
            matches!(
                events.last().unwrap().kind,
                EventKind::Completed {
                    stop_reason: StopReason::ToolUse
                }
            ),
            "{protocol:?}"
        );
        let expected_id = match protocol {
            Protocol::Messages => "toolu_fx_1",
            _ => "call_fx_1",
        };
        assert_eq!(call.id.0, expected_id);

        let continuation = adapter.continuation().await.unwrap().unwrap();
        assert_eq!(
            continuation.owner,
            match protocol {
                Protocol::ChatCompletions => "chat_completions",
                Protocol::Responses => "responses",
                Protocol::Messages => "messages",
            }
        );

        let follow_up = adapter
            .submit_tool_results(vec![ToolResult {
                call_id: ToolCallId(expected_id.into()),
                name: "file_read".into(),
                is_error: false,
                output: "hi".into(),
            }])
            .await
            .unwrap();
        let events = collect(follow_up).await;
        assert_eq!(text_of(&events), "The README says hi.", "{protocol:?}");
        assert_eq!(kinds(&events).last().unwrap(), "completed:EndTurn");

        let second = &transport.requests()[1];
        let body = second.body_json().unwrap();
        match protocol {
            Protocol::ChatCompletions => {
                let messages = body["messages"].as_array().unwrap();
                let assistant = &messages[messages.len() - 2];
                assert_eq!(assistant["tool_calls"][0]["id"], "call_fx_1");
                let tool = messages.last().unwrap();
                assert_eq!(tool["role"], "tool");
                assert_eq!(tool["tool_call_id"], "call_fx_1");
                assert_eq!(tool["content"], "hi");
            }
            Protocol::Responses => {
                assert_eq!(body["previous_response_id"], "resp_fx3");
                assert_eq!(body["input"][0]["type"], "function_call_output");
                assert_eq!(body["input"][0]["call_id"], "call_fx_1");
                let after = adapter.continuation().await.unwrap().unwrap();
                assert_eq!(after.state["previous_response_id"], "resp_fx4");
            }
            Protocol::Messages => {
                let messages = body["messages"].as_array().unwrap();
                let assistant = &messages[messages.len() - 2];
                assert_eq!(assistant["role"], "assistant");
                assert_eq!(assistant["content"][1]["type"], "tool_use");
                let user = messages.last().unwrap();
                assert_eq!(user["content"][0]["type"], "tool_result");
                assert_eq!(user["content"][0]["tool_use_id"], "toolu_fx_1");
            }
        }
    }
}

#[tokio::test]
async fn continuation_state_restores_into_a_fresh_adapter() {
    for protocol in PROTOCOLS {
        let (context, _, _) = make_context(protocol, vec![sse(protocol, "stream-text.sse")], 32);
        let adapter = adapter_for(context);
        let events = collect(adapter.send(prompt(protocol, false)).await.unwrap()).await;
        let state = adapter.continuation().await.unwrap().unwrap();
        let (context, _, _) = make_context(protocol, vec![sse(protocol, "stream-text.sse")], 32);
        let restored = adapter_for(context);
        assert!(restored.continuation().await.unwrap().is_none());
        restored.restore(state.clone()).await.unwrap();
        assert_eq!(restored.continuation().await.unwrap().unwrap(), state);
        let next = collect(restored.send(prompt(protocol, false)).await.unwrap()).await;
        assert!(
            next[0].sequence > events.last().unwrap().sequence,
            "{protocol:?}: sequence continues after restore"
        );
        let wrong = adapter_for(
            make_context(
                match protocol {
                    Protocol::ChatCompletions => Protocol::Responses,
                    _ => Protocol::ChatCompletions,
                },
                vec![],
                32,
            )
            .0,
        );
        assert!(wrong.restore(state).await.is_err());
    }
}

#[tokio::test]
async fn provider_error_bodies_stay_in_the_diagnostic() {
    for (protocol, status) in [
        (Protocol::ChatCompletions, 404u16),
        (Protocol::Responses, 401),
        (Protocol::Messages, 401),
    ] {
        let body = fixture(protocol_dir(protocol), "error.json");
        let (context, _, _) = make_context(protocol, vec![FixtureResponse::json(status, body)], 16);
        let adapter = adapter_for(context);
        let error = adapter
            .send(prompt(protocol, false))
            .await
            .err()
            .expect("expected an error");
        assert_eq!(error.kind, ErrorKind::Provider, "{protocol:?}");
        assert!(error
            .message
            .starts_with(&format!("provider returned {status}")));
        assert!(!error.message.contains(TEST_KEY));
        let diagnostic = error.diagnostic.unwrap();
        assert_eq!(diagnostic["status"], status);
        assert!(diagnostic["body"]["error"].is_object());
    }
}

#[tokio::test]
async fn stream_errors_end_the_stream_with_an_error_event() {
    for protocol in PROTOCOLS {
        let (context, _, _) = make_context(protocol, vec![sse(protocol, "stream-error.sse")], 9);
        let adapter = adapter_for(context);
        let events = collect(adapter.send(prompt(protocol, false)).await.unwrap()).await;
        let kinds = kinds(&events);
        assert_eq!(kinds.last().unwrap(), "error", "{protocol:?}: {kinds:?}");
        assert_eq!(kinds.iter().filter(|k| *k == "error").count(), 1);
        assert!(!kinds.iter().any(|k| k.starts_with("completed")));
        let EventKind::Error {
            error_kind,
            message,
        } = &events.last().unwrap().kind
        else {
            unreachable!()
        };
        assert_eq!(*error_kind, ErrorKind::Provider);
        assert!(!message.is_empty());
        assert!(events.last().unwrap().diagnostic.is_some());
    }
}

#[tokio::test]
async fn unsupported_capabilities_are_refused_before_any_request() {
    for protocol in PROTOCOLS {
        let capabilities = ModelCapabilities {
            tools: Some(false),
            ..Default::default()
        };
        let (context, transport, _) = make_context_with(
            protocol,
            vec![sse(protocol, "stream-text.sse")],
            16,
            Arc::new(FixedCapabilities(capabilities)),
        );
        let adapter = adapter_for(context);
        let error = adapter
            .send(prompt(protocol, true))
            .await
            .err()
            .expect("expected an error");
        assert_eq!(error.kind, ErrorKind::UnsupportedCapability, "{protocol:?}");
        assert!(error.message.contains("tools"));
        assert_eq!(
            transport.request_count(),
            0,
            "{protocol:?}: request must not be sent"
        );
        let reported = adapter.capabilities(model_for(protocol)).await.unwrap();
        assert_eq!(reported.tools, Some(false));
    }
}

#[tokio::test]
async fn cancellation_ends_the_stream_with_a_cancelled_event() {
    for protocol in PROTOCOLS {
        let (context, transport, cancel) = make_context(
            protocol,
            vec![
                sse(protocol, "stream-text.sse"),
                sse(protocol, "stream-text.sse"),
            ],
            4,
        );
        let adapter = adapter_for(context);
        let stream = adapter.send(prompt(protocol, false)).await.unwrap();
        cancel.cancel();
        let events = collect(stream).await;
        assert_eq!(kinds(&events), vec!["cancelled"], "{protocol:?}");

        // While the token stays cancelled, a new turn ends immediately with
        // the terminal event and no request leaves the adapter.
        let events = collect(adapter.send(prompt(protocol, false)).await.unwrap()).await;
        assert_eq!(kinds(&events), vec!["cancelled"], "{protocol:?}");
        assert_eq!(transport.request_count(), 1, "{protocol:?}");

        // After a reset the same adapter serves the next turn.
        cancel.reset();
        let events = collect(adapter.send(prompt(protocol, false)).await.unwrap()).await;
        assert_eq!(text_of(&events), "Hello, world", "{protocol:?}");
        assert_eq!(transport.request_count(), 2, "{protocol:?}");
    }
}

#[tokio::test]
async fn unknown_stream_elements_are_skipped_not_fatal() {
    let body = format!(
        "event: response.brand_new_thing\ndata: {{\"type\":\"response.brand_new_thing\"}}\n\n{}",
        String::from_utf8(fixture("responses", "stream-text.sse")).unwrap()
    );
    let (context, _, _) = make_context(Protocol::Responses, vec![FixtureResponse::sse(body)], 32);
    let adapter = adapter_for(context);
    let events = collect(
        adapter
            .send(prompt(Protocol::Responses, false))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(text_of(&events), "Hello, world");
    let completed = events.last().unwrap();
    assert_eq!(
        completed.diagnostic.as_ref().unwrap()["skipped"][0],
        "response.brand_new_thing"
    );
}

#[tokio::test]
async fn messages_refuses_structured_output_before_sending() {
    let (context, transport, _) = make_context(Protocol::Messages, vec![], 16);
    let adapter = adapter_for(context);
    let mut request = prompt(Protocol::Messages, false);
    request.options.structured_output = Some(serde_json::json!({ "type": "object" }));
    let error = adapter
        .send(request)
        .await
        .err()
        .expect("expected an error");
    assert_eq!(error.kind, ErrorKind::UnsupportedCapability);
    assert_eq!(transport.request_count(), 0);
}

#[tokio::test]
async fn every_chat_completions_profile_streams_and_only_openrouter_gets_attribution() {
    for profile in chat_profiles() {
        let name = profile.name.clone();
        let expected_url = format!(
            "{}/chat/completions",
            profile.base_url.trim_end_matches('/')
        );
        let (context, transport, _) = make_context_for(
            profile,
            vec![sse(Protocol::ChatCompletions, "stream-text.sse")],
            16,
        );
        let adapter = adapter_for(context);
        let events = collect(
            adapter
                .send(prompt(Protocol::ChatCompletions, true))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(text_of(&events), "Hello, world", "{name}");
        assert_eq!(
            kinds(&events).last().unwrap(),
            "completed:EndTurn",
            "{name}"
        );
        let request = &transport.requests()[0];
        assert_eq!(request.url, expected_url, "{name}");
        let names: Vec<&str> = request.headers.iter().map(|(n, _)| n.as_str()).collect();
        let attributed = names.contains(&"http-referer") || names.contains(&"x-title");
        assert_eq!(attributed, name == "openrouter", "{name}");
        assert!(!format!("{request:?}").contains(TEST_KEY), "{name}");
    }
}

#[tokio::test]
async fn provider_bodies_that_echo_the_key_are_redacted_everywhere() {
    for protocol in PROTOCOLS {
        let body = format!(
            r#"{{"error":{{"message":"invalid key {TEST_KEY}","type":"auth","echo":"Bearer {TEST_KEY}"}}}}"#
        );
        let (context, _, _) = make_context(protocol, vec![FixtureResponse::json(401, body)], 16);
        let adapter = adapter_for(context);
        let error = adapter
            .send(prompt(protocol, false))
            .await
            .err()
            .expect("expected an error");
        assert_eq!(error.kind, ErrorKind::Provider, "{protocol:?}");
        let display = error.to_string();
        let debug = format!("{error:?}");
        let diagnostic = serde_json::to_string(&error.diagnostic).unwrap();
        for text in [&display, &debug, &diagnostic] {
            assert!(!text.contains(TEST_KEY), "{protocol:?}: key leaked");
            assert!(text.contains("[redacted]"), "{protocol:?}");
        }
        assert!(error.message.ends_with("invalid key [redacted]"));

        // The same key inside a stream error element is redacted on the event.
        let stream_body = format!(
            "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\",\"message\":\"rejected {TEST_KEY}\",\"code\":529}}}}\n\n"
        );
        let (context, _, _) = make_context(protocol, vec![FixtureResponse::sse(stream_body)], 8);
        let adapter = adapter_for(context);
        let events = collect(adapter.send(prompt(protocol, false)).await.unwrap()).await;
        let serialized = serde_json::to_string(&events).unwrap();
        assert!(
            !serialized.contains(TEST_KEY),
            "{protocol:?}: key leaked into events"
        );
    }
}

#[tokio::test]
async fn oversized_provider_bodies_are_capped_in_the_diagnostic() {
    let body = format!(r#"{{"error":{{"message":"{}"}}}}"#, "x".repeat(10_000));
    let (context, _, _) = make_context(
        Protocol::ChatCompletions,
        vec![FixtureResponse::json(500, body)],
        512,
    );
    let adapter = adapter_for(context);
    let error = adapter
        .send(prompt(Protocol::ChatCompletions, false))
        .await
        .err()
        .expect("expected an error");
    let diagnostic = error.diagnostic.unwrap();
    assert_eq!(diagnostic["body"]["truncated"], true);
    assert!(diagnostic["body"]["raw"].as_str().unwrap().chars().count() <= 4096);
    assert!(error.message.chars().count() <= 500);
}

#[tokio::test]
async fn unreported_capabilities_are_flagged_on_the_first_event() {
    for protocol in PROTOCOLS {
        let (context, _, _) = make_context(protocol, vec![sse(protocol, "stream-text.sse")], 32);
        let adapter = adapter_for(context);
        let mut request = prompt(protocol, true);
        request.options.reasoning = Some(true);
        let events = collect(adapter.send(request).await.unwrap()).await;
        let first = events.first().unwrap();
        let warning = &first.diagnostic.as_ref().expect("diagnostic")["capability_warning"];
        assert_eq!(
            warning["features"],
            serde_json::json!(["tools", "reasoning"]),
            "{protocol:?}"
        );
        assert_eq!(warning["model_list_entry"], false);
        assert!(events[1..].iter().all(|event| event
            .diagnostic
            .as_ref()
            .is_none_or(|d| d.get("capability_warning").is_none())));

        let (context, _, _) = make_context_with(
            protocol,
            vec![sse(protocol, "stream-text.sse")],
            32,
            Arc::new(FixedCapabilities(ModelCapabilities {
                tools: Some(true),
                reasoning: Some(true),
                ..Default::default()
            })),
        );
        let adapter = adapter_for(context);
        let mut request = prompt(protocol, true);
        request.options.reasoning = Some(true);
        let events = collect(adapter.send(request).await.unwrap()).await;
        assert!(events.iter().all(|event| event
            .diagnostic
            .as_ref()
            .is_none_or(|d| d.get("capability_warning").is_none())));
    }
}

#[tokio::test]
async fn responses_wire_sequence_gaps_are_warned_not_reordered() {
    let (context, _, _) = make_context(
        Protocol::Responses,
        vec![sse(Protocol::Responses, "stream-sequence-gap.sse")],
        16,
    );
    let adapter = adapter_for(context);
    let events = collect(
        adapter
            .send(prompt(Protocol::Responses, false))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(text_of(&events), "Hello, world");
    assert_monotonic(&events);
    let flagged: Vec<&gritt_core::event::Event> = events
        .iter()
        .filter(|event| {
            event
                .diagnostic
                .as_ref()
                .is_some_and(|d| d.get("sequence_warning").is_some())
        })
        .collect();
    assert_eq!(flagged.len(), 1);
    let warning = &flagged[0].diagnostic.as_ref().unwrap()["sequence_warning"];
    assert_eq!(warning["wire_sequence"], 7);
    assert_eq!(warning["expected_wire_sequence"], 5);
    assert!(matches!(flagged[0].kind, EventKind::TextDelta { .. }));
    let completed = events.last().unwrap().diagnostic.as_ref().unwrap();
    assert_eq!(completed["last_wire_sequence"], 10);
    assert_eq!(completed["sequence_warnings"].as_array().unwrap().len(), 1);

    let (context, _, _) = make_context(
        Protocol::Responses,
        vec![sse(Protocol::Responses, "stream-text.sse")],
        16,
    );
    let adapter = adapter_for(context);
    let events = collect(
        adapter
            .send(prompt(Protocol::Responses, false))
            .await
            .unwrap(),
    )
    .await;
    assert!(events.iter().all(|event| event
        .diagnostic
        .as_ref()
        .is_none_or(|d| d.get("sequence_warning").is_none())));
    assert!(events.last().unwrap().diagnostic.as_ref().unwrap()["sequence_warnings"].is_null());
}

#[tokio::test]
async fn cancellation_while_connecting_drops_the_send_and_yields_cancelled() {
    for protocol in PROTOCOLS {
        let transport = Arc::new(PendingTransport::new());
        let cancel = gritt_provider::CancellationToken::new();
        let context = gritt_provider::AdapterContext {
            profile: profile(protocol),
            session_id: gritt_core::session::SessionId("session-test".into()),
            transport: transport.clone(),
            keys: Arc::new(gritt_provider::adapter::StaticKey(
                gritt_core::secret::Secret::new(TEST_KEY),
            )),
            capabilities: Arc::new(gritt_provider::NoCapabilities),
            cancel: cancel.clone(),
        };
        let adapter = adapter_for(context);
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            canceller.cancel();
        });
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            adapter.send(prompt(protocol, false)),
        )
        .await
        .expect("send must return once cancelled")
        .unwrap();
        let events = collect(stream).await;
        assert_eq!(kinds(&events), vec!["cancelled"], "{protocol:?}");
        assert_eq!(
            transport.attempts.load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        // A reset token lets the same adapter serve the next turn.
        cancel.reset();
        assert!(!cancel.is_cancelled());
    }
}

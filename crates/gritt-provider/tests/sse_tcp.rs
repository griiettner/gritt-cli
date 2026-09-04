//! Serves a streaming fixture from a local TCP listener through the real
//! `reqwest` transport. The server withholds the second half of the body
//! until the client has consumed the first text delta, so the test can only
//! pass if parsing is incremental.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use futures::StreamExt;
use gritt_core::event::EventKind;
use gritt_core::provider::{Protocol, ProviderAdapter, ProviderProfile};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::SessionId;
use gritt_provider::chat_completions::ChatCompletionsAdapter;
use gritt_provider::{
    AdapterContext, CancellationToken, NoCapabilities, ReqwestTransport, StaticKey,
};

const FIRST: &str = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"first\"},\"finish_reason\":null}]}\n\n";
const SECOND: &str = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" second\"},\"finish_reason\":\"stop\"}]}\n\ndata: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";

#[tokio::test]
async fn incremental_sse_over_a_real_http_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (seen_tx, seen_rx) = mpsc::channel::<String>();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut content_length = 0usize;
        let mut authorization = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                content_length = value.trim().parse().unwrap();
            }
            if lower.starts_with("authorization:") {
                authorization = line.trim().to_owned();
            }
            if line == "\r\n" {
                break;
            }
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).unwrap();
        seen_tx.send(authorization).unwrap();
        let mut stream = stream;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.write_all(FIRST.as_bytes()).unwrap();
        stream.flush().unwrap();
        // Wait for the client to prove it parsed the first event.
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        stream.write_all(SECOND.as_bytes()).unwrap();
        stream.flush().unwrap();
        String::from_utf8(body).unwrap()
    });

    let profile = ProviderProfile {
        name: "local".into(),
        protocol: Protocol::ChatCompletions,
        base_url: format!("http://127.0.0.1:{port}/v1"),
        key: SecretRef::for_profile("local", "LOCAL_KEY"),
        aliases: Default::default(),
    };
    let context = AdapterContext {
        profile,
        session_id: SessionId("tcp".into()),
        transport: Arc::new(ReqwestTransport::new().unwrap()),
        keys: Arc::new(StaticKey(Secret::new(TEST_KEY))),
        capabilities: Arc::new(NoCapabilities),
        cancel: CancellationToken::new(),
    };
    let adapter = ChatCompletionsAdapter::new(context);
    let mut stream = adapter
        .send(prompt(Protocol::ChatCompletions, false))
        .await
        .unwrap();

    let mut events = Vec::new();
    let mut released = false;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("stream stalled: parsing is not incremental")
    {
        let event = event.unwrap();
        if !released {
            if let EventKind::TextDelta { text } = &event.kind {
                assert_eq!(text, "first");
                release_tx.send(()).unwrap();
                released = true;
            }
        }
        events.push(event);
    }
    assert!(released);
    assert_eq!(text_of(&events), "first second");
    assert_eq!(kinds(&events).last().unwrap(), "completed:EndTurn");
    let body = server.join().unwrap();
    assert!(body.contains("\"stream\":true"));
    let authorization = seen_rx.recv().unwrap();
    assert_eq!(authorization, format!("authorization: Bearer {TEST_KEY}"));
}

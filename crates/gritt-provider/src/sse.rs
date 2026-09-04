//! Incremental `text/event-stream` parser. Bytes are fed as they arrive;
//! the parser never needs the whole body and handles chunk boundaries
//! anywhere, including inside a UTF-8 sequence or a `\r\n` pair.

use bytes::Bytes;
use futures::stream::{Stream, StreamExt};
use gritt_core::Result;

use crate::transport::ByteStream;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

impl SseEvent {
    /// The `[DONE]` sentinel OpenAI-compatible streams end with.
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.data).ok()
    }
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
    saw_cr: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one chunk and returns every event completed by it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for &byte in chunk {
            // A line ends at `\n`, `\r`, or `\r\n`. A `\n` right after a
            // `\r` belongs to the previous line ending.
            if byte == b'\n' && self.saw_cr {
                self.saw_cr = false;
                continue;
            }
            self.saw_cr = byte == b'\r';
            if byte == b'\n' || byte == b'\r' {
                let line = std::mem::take(&mut self.buffer);
                if let Some(event) = self.line(&line) {
                    events.push(event);
                }
            } else {
                self.buffer.push(byte);
            }
        }
        events
    }

    /// Flushes a trailing event that had no terminating blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.line(&line);
        }
        self.dispatch()
    }

    fn line(&mut self, line: &[u8]) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        let line = String::from_utf8_lossy(line);
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            "id" => self.id = Some(value.to_owned()),
            // `retry` and unknown fields are ignored per the specification.
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.event.is_none() {
            return None;
        }
        let event = SseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
            id: self.id.clone(),
        };
        Some(event)
    }
}

/// Turns a byte stream into an event stream without buffering the body.
pub fn sse_stream(body: ByteStream) -> impl Stream<Item = Result<SseEvent>> + Send {
    struct State {
        body: ByteStream,
        parser: SseParser,
        pending: std::collections::VecDeque<SseEvent>,
        finished: bool,
    }
    let state = State {
        body,
        parser: SseParser::new(),
        pending: Default::default(),
        finished: false,
    };
    futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                return Some((Ok(event), state));
            }
            if state.finished {
                return None;
            }
            match state.body.next().await {
                Some(Ok(chunk)) => state.pending.extend(state.parser.feed(&chunk)),
                Some(Err(error)) => {
                    state.finished = true;
                    return Some((Err(error), state));
                }
                None => {
                    state.finished = true;
                    state.pending.extend(state.parser.finish());
                }
            }
        }
    })
}

/// Splits a fixture into a chunked byte stream. Test helper.
pub fn chunked(body: &[u8], chunk_size: usize) -> ByteStream {
    let chunks: Vec<Result<Bytes>> = body
        .chunks(chunk_size.max(1))
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
        .collect();
    Box::pin(futures::stream::iter(chunks))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "event: message_start\ndata: {\"a\":1}\n\n: keep-alive\n\ndata: line one\ndata: line two\nid: 7\n\r\ndata: [DONE]\n\n";

    fn parse_all(body: &[u8], chunk_size: usize) -> Vec<SseEvent> {
        let mut parser = SseParser::new();
        let mut events = Vec::new();
        for chunk in body.chunks(chunk_size) {
            events.extend(parser.feed(chunk));
        }
        events.extend(parser.finish());
        events
    }

    #[test]
    fn parses_events_comments_multiline_data_and_done() {
        let events = parse_all(SAMPLE.as_bytes(), 1024);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[1].data, "line one\nline two");
        assert_eq!(events[1].id.as_deref(), Some("7"));
        assert!(events[2].is_done());
    }

    #[test]
    fn every_chunk_boundary_yields_the_same_events() {
        let reference = parse_all(SAMPLE.as_bytes(), 1024);
        for size in 1..SAMPLE.len() {
            assert_eq!(
                parse_all(SAMPLE.as_bytes(), size),
                reference,
                "chunk {size}"
            );
        }
    }

    #[test]
    fn crlf_and_utf8_split_across_chunks() {
        let body = "data: caf\u{e9}\r\ndata: end\r\n\r\n";
        let events = parse_all(body.as_bytes(), 2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "caf\u{e9}\nend");
    }

    #[test]
    fn trailing_event_without_blank_line_is_flushed() {
        let events = parse_all(b"data: tail", 4);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "tail");
    }

    #[tokio::test]
    async fn stream_preserves_order() {
        let stream = sse_stream(chunked(SAMPLE.as_bytes(), 5));
        let events: Vec<SseEvent> = stream.map(|event| event.unwrap()).collect().await;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert!(events[2].is_done());
    }
}

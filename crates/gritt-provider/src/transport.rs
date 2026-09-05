//! HTTP transport seam. Adapters build [`HttpRequest`]s and read a byte
//! stream back; the real implementation wraps `reqwest`, and tests use an
//! in-memory fixture transport that replays recorded bodies in small chunks.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;

use bytes::Bytes;
use futures::stream::{self, Stream, StreamExt};
use gritt_core::secret::Secret;
use gritt_core::session::BoxFuture;
use gritt_core::{Error, Result};

pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    /// Used by MCP's Streamable HTTP transport to end a session.
    Delete,
}

/// A header value. Secrets keep their redaction through Debug.
#[derive(Clone)]
pub enum HeaderValue {
    Plain(String),
    Secret(Secret),
}

impl HeaderValue {
    /// The only place a header secret is read. Never pass the result to a
    /// formatter.
    pub fn expose(&self) -> &str {
        match self {
            HeaderValue::Plain(value) => value,
            HeaderValue::Secret(secret) => secret.expose(),
        }
    }
}

impl std::fmt::Debug for HeaderValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderValue::Plain(value) => f.write_str(value),
            HeaderValue::Secret(_) => f.write_str("[redacted]"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, HeaderValue)>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn delete(url: impl Into<String>) -> Self {
        Self {
            method: Method::Delete,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: vec![(
                "content-type".into(),
                HeaderValue::Plain("application/json".into()),
            )],
            body: Some(body.to_string().into_bytes()),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers
            .push((name.to_ascii_lowercase(), HeaderValue::Plain(value.into())));
        self
    }

    pub fn secret_header(mut self, name: &str, value: Secret) -> Self {
        self.headers
            .push((name.to_ascii_lowercase(), HeaderValue::Secret(value)));
        self
    }

    /// Parses the JSON body, for tests and request inspection.
    pub fn body_json(&self) -> Option<serde_json::Value> {
        self.body
            .as_deref()
            .and_then(|body| serde_json::from_slice(body).ok())
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    /// Response headers with lower-cased names. Protocols that carry state
    /// in a header, such as MCP's `Mcp-Session-Id`, read it from here.
    pub headers: Vec<(String, String)>,
    pub body: ByteStream,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The first value of `name`, which is matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Collects the whole body. Only for non-streaming responses such as
    /// model lists and error bodies.
    pub async fn bytes(self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut body = self.body;
        while let Some(chunk) = body.next().await {
            out.extend_from_slice(&chunk?);
        }
        Ok(out)
    }
}

pub trait HttpTransport: Send + Sync {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse>>;
}

/// The production transport.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self> {
        // `reqwest` is built without a TLS crypto provider so the workspace
        // never pulls `aws-lc-rs`; `ring` is installed once here. A second
        // install attempt only reports that one already exists.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .user_agent(concat!("gritt/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| Error::provider(None, format!("cannot build HTTP client: {error}")))?;
        Ok(Self { client })
    }
}

impl HttpTransport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse>> {
        Box::pin(async move {
            let mut builder = match request.method {
                Method::Get => self.client.get(&request.url),
                Method::Post => self.client.post(&request.url),
                Method::Delete => self.client.delete(&request.url),
            };
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.expose());
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let response = builder
                .send()
                .await
                .map_err(|error| Error::provider(None, redact_reqwest_error(&error)))?;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
                })
                .collect();
            let body = response.bytes_stream().map(|chunk| {
                chunk.map_err(|error| Error::provider(None, redact_reqwest_error(&error)))
            });
            Ok(HttpResponse {
                status,
                content_type,
                headers,
                body: Box::pin(body),
            })
        })
    }
}

/// `reqwest` errors can embed the full URL. The URL never carries a key on
/// the supported protocols, but the message is trimmed to its kind anyway.
fn redact_reqwest_error(error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_request() {
        "request failed"
    } else if error.is_body() || error.is_decode() {
        "body read failed"
    } else {
        "transport error"
    };
    match error.url() {
        Some(url) => format!("{kind} for {}{}", url.host_str().unwrap_or("?"), url.path()),
        None => kind.to_string(),
    }
}

/// One recorded response for the fixture transport.
#[derive(Debug, Clone)]
pub struct FixtureResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl FixtureResponse {
    pub fn sse(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            content_type: Some("text/event-stream".into()),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn json(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: Some("application/json".into()),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Adds a response header, for protocols that read one back.
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_ascii_lowercase(), value.into()));
        self
    }
}

/// Replays queued responses and records every request. Bodies are split
/// into `chunk_size` byte pieces so parsers prove they handle arbitrary
/// boundaries.
pub struct FixtureTransport {
    responses: Mutex<VecDeque<FixtureResponse>>,
    requests: Mutex<Vec<HttpRequest>>,
    chunk_size: usize,
}

impl FixtureTransport {
    pub fn new(responses: impl IntoIterator<Item = FixtureResponse>, chunk_size: usize) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            chunk_size: chunk_size.max(1),
        }
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().expect("fixture requests").clone()
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("fixture requests").len()
    }
}

impl HttpTransport for FixtureTransport {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse>> {
        self.requests
            .lock()
            .expect("fixture requests")
            .push(request);
        let next = self
            .responses
            .lock()
            .expect("fixture responses")
            .pop_front();
        Box::pin(async move {
            let Some(response) = next else {
                return Err(Error::provider(
                    None,
                    "fixture transport has no response queued",
                ));
            };
            let chunks: Vec<Result<Bytes>> = response
                .body
                .chunks(self.chunk_size)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                .collect();
            Ok(HttpResponse {
                status: response.status,
                content_type: response.content_type,
                headers: response.headers,
                body: Box::pin(stream::iter(chunks)),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_headers_are_redacted_in_debug() {
        let request = HttpRequest::get("https://example.test/models")
            .secret_header("authorization", Secret::new("Bearer sk-live"))
            .header("x-title", "Gritt");
        let debug = format!("{request:?}");
        assert!(!debug.contains("sk-live"));
        assert!(debug.contains("[redacted]"));
        assert!(debug.contains("Gritt"));
    }

    #[tokio::test]
    async fn fixture_transport_chunks_and_records() {
        let transport = FixtureTransport::new([FixtureResponse::sse("abcdefg")], 3);
        let response = transport
            .send(HttpRequest::get("https://example.test"))
            .await
            .unwrap();
        let mut body = response.body;
        let mut sizes = Vec::new();
        while let Some(chunk) = body.next().await {
            sizes.push(chunk.unwrap().len());
        }
        assert_eq!(sizes, vec![3, 3, 1]);
        assert_eq!(transport.request_count(), 1);
    }
}

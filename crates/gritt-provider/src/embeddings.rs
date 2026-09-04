//! Opt-in embedding and reranking clients. Both exist only when the
//! environment enables them; when disabled no client is built and no
//! request is ever issued. Endpoints are `{base_url}/v1/embeddings` and
//! `{base_url}/v1/rerank` on the configured OpenAI-compatible gateway.

use std::sync::Arc;

use gritt_core::embeddings::{EmbeddingConfig, MemoryGateway, RerankConfig};
use gritt_core::secret::Secret;
use gritt_core::{Error, Result};

use crate::adapter::{provider_error, KeyProvider};
use crate::transport::{HttpRequest, HttpTransport};

const GATEWAY_PROFILE: &str = "memory-gateway";

struct Gateway {
    transport: Arc<dyn HttpTransport>,
    keys: Arc<dyn KeyProvider>,
    config: MemoryGateway,
    model: String,
}

impl Gateway {
    fn url(&self, path: &str) -> Result<String> {
        let base = self.config.base_url.as_deref().ok_or_else(|| {
            Error::config(format!(
                "{} is enabled but {} is not set",
                self.model,
                gritt_core::embeddings::BASE_URL_VAR
            ))
        })?;
        Ok(format!("{}/v1/{path}", base.trim_end_matches('/')))
    }

    async fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let key = self.keys.key(GATEWAY_PROFILE, &self.config.key)?;
        let request = HttpRequest::post_json(self.url(path)?, &body).secret_header(
            "authorization",
            Secret::new(format!("Bearer {}", key.expose())),
        );
        let response = self.transport.send(request).await?;
        let status = response.status;
        let bytes = response.bytes().await?;
        if !(200..300).contains(&status) {
            return Err(provider_error(status, &bytes, &[key]));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| Error::provider(Some(status), format!("invalid JSON: {error}")))
    }
}

pub struct EmbeddingClient {
    gateway: Gateway,
}

impl EmbeddingClient {
    /// `None` when embeddings are disabled. The transport factory runs only
    /// when a client is actually built.
    pub fn from_config(
        config: Option<&EmbeddingConfig>,
        keys: Arc<dyn KeyProvider>,
        transport: impl FnOnce() -> Arc<dyn HttpTransport>,
    ) -> Option<Self> {
        let config = config?;
        let model = config.model.clone()?;
        Some(Self {
            gateway: Gateway {
                transport: transport(),
                keys,
                config: config.gateway.clone(),
                model,
            },
        })
    }

    pub fn model(&self) -> &str {
        &self.gateway.model
    }

    pub async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let body = serde_json::json!({ "model": self.gateway.model, "input": inputs });
        let value = self.gateway.post("embeddings", body).await?;
        let data = value
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| Error::provider(None, "embeddings response has no `data`"))?;
        let mut rows: Vec<(usize, Vec<f32>)> = data
            .iter()
            .enumerate()
            .map(|(position, item)| {
                let index = item
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(position);
                let vector = item
                    .get("embedding")
                    .and_then(|v| v.as_array())
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|v| v.as_f64())
                            .map(|v| v as f32)
                            .collect()
                    })
                    .unwrap_or_default();
                (index, vector)
            })
            .collect();
        rows.sort_by_key(|(index, _)| *index);
        Ok(rows.into_iter().map(|(_, vector)| vector).collect())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedDocument {
    pub index: usize,
    pub score: f64,
}

pub struct RerankClient {
    gateway: Gateway,
}

impl RerankClient {
    pub fn from_config(
        config: Option<&RerankConfig>,
        keys: Arc<dyn KeyProvider>,
        transport: impl FnOnce() -> Arc<dyn HttpTransport>,
    ) -> Option<Self> {
        let config = config?;
        let model = config.model.clone()?;
        Some(Self {
            gateway: Gateway {
                transport: transport(),
                keys,
                config: config.gateway.clone(),
                model,
            },
        })
    }

    pub fn model(&self) -> &str {
        &self.gateway.model
    }

    /// Reorders candidates. It never expands the query or fetches new
    /// documents.
    pub async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<RankedDocument>> {
        let body = serde_json::json!({
            "model": self.gateway.model,
            "query": query,
            "documents": documents,
        });
        let value = self.gateway.post("rerank", body).await?;
        let results = value
            .get("results")
            .and_then(|r| r.as_array())
            .ok_or_else(|| Error::provider(None, "rerank response has no `results`"))?;
        let mut ranked: Vec<RankedDocument> = results
            .iter()
            .filter_map(|item| {
                Some(RankedDocument {
                    index: item.get("index")?.as_u64()? as usize,
                    score: item.get("relevance_score")?.as_f64()?,
                })
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(ranked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::StaticKey;
    use crate::transport::{FixtureResponse, FixtureTransport};
    use std::collections::HashMap;

    fn keys() -> Arc<dyn KeyProvider> {
        Arc::new(StaticKey(Secret::new("gateway-key")))
    }

    #[test]
    fn disabled_builds_nothing_and_never_touches_the_transport() {
        let empty = HashMap::new();
        let embedding = gritt_core::embeddings::embedding_config(&empty);
        let rerank = gritt_core::embeddings::rerank_config(&empty);
        let built = std::cell::Cell::new(0);
        let factory = || {
            built.set(built.get() + 1);
            Arc::new(FixtureTransport::new([], 8)) as Arc<dyn HttpTransport>
        };
        assert!(EmbeddingClient::from_config(Some(&embedding), keys(), factory).is_none());
        assert!(RerankClient::from_config(Some(&rerank), keys(), factory).is_none());
        assert!(EmbeddingClient::from_config(None, keys(), factory).is_none());
        assert_eq!(built.get(), 0);
    }

    #[tokio::test]
    async fn enabled_clients_use_the_configured_gateway() {
        let env: HashMap<String, String> = [
            ("AGENT_EMBEDDING_PROVIDER", "text-embedding-3-small"),
            ("AGENT_RERANK_PROVIDER", "rerank-3.5"),
            ("AGENT_MEMORY_BASE_URL", "https://gateway.test/api/"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let transport = Arc::new(FixtureTransport::new(
            [
                FixtureResponse::json(
                    200,
                    r#"{"data":[{"index":1,"embedding":[0.5]},{"index":0,"embedding":[0.1,0.2]}]}"#,
                ),
                FixtureResponse::json(
                    200,
                    r#"{"results":[{"index":0,"relevance_score":0.2},{"index":1,"relevance_score":0.9}]}"#,
                ),
            ],
            16,
        ));
        let embedding = gritt_core::embeddings::embedding_config(&env);
        let client =
            EmbeddingClient::from_config(Some(&embedding), keys(), || transport.clone()).unwrap();
        let vectors = client.embed(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(vectors, vec![vec![0.1, 0.2], vec![0.5]]);
        let rerank = gritt_core::embeddings::rerank_config(&env);
        let client =
            RerankClient::from_config(Some(&rerank), keys(), || transport.clone()).unwrap();
        let ranked = client.rerank("q", &["x".into(), "y".into()]).await.unwrap();
        assert_eq!(ranked[0].index, 1);
        let requests = transport.requests();
        assert_eq!(requests[0].url, "https://gateway.test/api/v1/embeddings");
        assert_eq!(requests[1].url, "https://gateway.test/api/v1/rerank");
        assert!(!format!("{:?}", requests[0]).contains("gateway-key"));
    }
}

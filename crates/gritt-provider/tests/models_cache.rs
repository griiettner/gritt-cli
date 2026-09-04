//! Model list cache: daily refresh, stale fallback, and the error kinds.

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use common::*;
use gritt_core::config::ModelListPolicy;
use gritt_core::provider::{ModelListStatus, Protocol};
use gritt_core::secret::Secret;
use gritt_core::ErrorKind;
use gritt_provider::adapter::{CapabilitySource, KeyProvider, StaticKey};
use gritt_provider::models::{load_models, ModelCache, ModelCatalog};
use gritt_provider::{FixtureResponse, FixtureTransport};

fn keys() -> Arc<dyn KeyProvider> {
    Arc::new(StaticKey(Secret::new(TEST_KEY)))
}

#[tokio::test]
async fn refreshes_at_most_daily_and_falls_back_to_the_stale_list() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path());
    let policy = ModelListPolicy::default();
    let keys = keys();
    let now = Utc::now();
    for protocol in [
        Protocol::ChatCompletions,
        Protocol::Responses,
        Protocol::Messages,
    ] {
        let profile = profile(protocol);
        let transport = FixtureTransport::new(
            [FixtureResponse::json(
                200,
                fixture(protocol_dir(protocol), "models.json"),
            )],
            32,
        );
        let first = load_models(&cache, &transport, &*keys, &profile, &policy, now, false)
            .await
            .unwrap();
        assert!(matches!(first.status, ModelListStatus::Fresh { fetched_at } if fetched_at == now));
        assert!(!first.models.is_empty());
        assert!(cache.path(&profile.name).is_file());
        let request = &transport.requests()[0];
        let expected = match protocol {
            Protocol::ChatCompletions => "https://openrouter.ai/api/v1/models",
            Protocol::Responses => "https://api.openai.com/v1/models",
            Protocol::Messages => "https://api.anthropic.com/v1/models",
        };
        assert_eq!(request.url, expected);
        assert!(!format!("{request:?}").contains(TEST_KEY));

        // Within the interval the cache answers and nothing is fetched.
        let later = now + Duration::hours(1);
        let second = load_models(&cache, &transport, &*keys, &profile, &policy, later, false)
            .await
            .unwrap();
        assert_eq!(second.models, first.models);
        assert_eq!(transport.request_count(), 1);

        // After the interval the refresh fails (no response queued) and
        // the cached list comes back marked stale.
        let next_day = now + Duration::hours(25);
        let third = load_models(
            &cache, &transport, &*keys, &profile, &policy, next_day, false,
        )
        .await
        .unwrap();
        assert!(matches!(third.status, ModelListStatus::Stale { fetched_at } if fetched_at == now));
        assert_eq!(third.models, first.models);
        assert_eq!(transport.request_count(), 2);

        let strict = ModelListPolicy {
            stale_fallback: false,
            ..ModelListPolicy::default()
        };
        let error = load_models(
            &cache, &transport, &*keys, &profile, &strict, next_day, false,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::StaleModelList);
    }
}

#[tokio::test]
async fn missing_cache_and_failed_refresh_is_missing_model_list() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path());
    let transport = FixtureTransport::new(
        [FixtureResponse::json(
            500,
            r#"{"error":{"message":"down"}}"#,
        )],
        32,
    );
    let error = load_models(
        &cache,
        &transport,
        &*keys(),
        &profile(Protocol::ChatCompletions),
        &ModelListPolicy::default(),
        Utc::now(),
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MissingModelList);
    assert!(error.message.contains("openrouter"));
    assert!(error.diagnostic.is_some());
}

#[tokio::test]
async fn catalog_reports_capabilities_and_deprecations_from_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path());
    let transport = FixtureTransport::new(
        [FixtureResponse::json(
            200,
            fixture("chat-completions", "models.json"),
        )],
        64,
    );
    let list = load_models(
        &cache,
        &transport,
        &*keys(),
        &profile(Protocol::ChatCompletions),
        &ModelListPolicy::default(),
        Utc::now(),
        true,
    )
    .await
    .unwrap();
    let catalog = ModelCatalog::new();
    catalog.insert(list);
    let nano = catalog
        .capabilities("openrouter", "openai/gpt-5-nano")
        .unwrap();
    assert_eq!(nano.tools, Some(true));
    assert_eq!(nano.vision, Some(true));
    assert_eq!(nano.reasoning, Some(true));
    assert_eq!(nano.context_length, Some(400000));
    let text_only = catalog
        .capabilities("openrouter", "placeholder/text-only")
        .unwrap();
    assert_eq!(text_only.tools, Some(false));
    assert_eq!(text_only.vision, Some(false));
    let legacy = catalog.model("openrouter", "placeholder/legacy").unwrap();
    assert!(legacy.deprecated);
    assert_eq!(legacy.replaced_by.as_deref(), Some("placeholder/text-only"));
    assert!(catalog.capabilities("openrouter", "unknown").is_none());
    assert!(catalog.capabilities("other", "openai/gpt-5-nano").is_none());
}

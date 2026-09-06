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
use gritt_provider::models::{load_models, probe_models, ModelCache, ModelCatalog};
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

#[tokio::test]
async fn a_failed_refresh_is_not_retried_until_the_interval_passes() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path());
    let policy = ModelListPolicy::default();
    let keys = keys();
    let profile = profile(Protocol::ChatCompletions);
    let now = Utc::now();
    let transport = FixtureTransport::new(
        [
            FixtureResponse::json(200, fixture("chat-completions", "models.json")),
            FixtureResponse::json(503, r#"{"error":{"message":"down"}}"#),
        ],
        64,
    );
    load_models(&cache, &transport, &*keys, &profile, &policy, now, false)
        .await
        .unwrap();
    let next_day = now + Duration::hours(25);
    let stale = load_models(
        &cache, &transport, &*keys, &profile, &policy, next_day, false,
    )
    .await
    .unwrap();
    assert!(matches!(stale.status, ModelListStatus::Stale { .. }));
    assert_eq!(transport.request_count(), 2);

    // An hour later the failed attempt is still within the interval: no fetch.
    let soon = next_day + Duration::hours(1);
    let throttled = load_models(&cache, &transport, &*keys, &profile, &policy, soon, false)
        .await
        .unwrap();
    assert!(matches!(throttled.status, ModelListStatus::Stale { fetched_at } if fetched_at == now));
    assert_eq!(transport.request_count(), 2);

    // Forcing bypasses the throttle; with nothing queued it stays stale.
    let forced = load_models(&cache, &transport, &*keys, &profile, &policy, soon, true)
        .await
        .unwrap();
    assert!(matches!(forced.status, ModelListStatus::Stale { .. }));
    assert_eq!(transport.request_count(), 3);

    // After the interval the refresh is attempted again.
    let later = next_day + Duration::hours(25);
    load_models(&cache, &transport, &*keys, &profile, &policy, later, false)
        .await
        .unwrap();
    assert_eq!(transport.request_count(), 4);
}

#[tokio::test]
async fn a_failed_refresh_is_throttled_even_when_stale_fallback_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path());
    let strict = ModelListPolicy {
        stale_fallback: false,
        ..ModelListPolicy::default()
    };
    let keys = keys();
    let profile = profile(Protocol::ChatCompletions);
    let now = Utc::now();
    let transport = FixtureTransport::new(
        [
            FixtureResponse::json(200, fixture("chat-completions", "models.json")),
            FixtureResponse::json(503, r#"{"error":{"message":"down"}}"#),
        ],
        64,
    );
    load_models(&cache, &transport, &*keys, &profile, &strict, now, false)
        .await
        .unwrap();
    let next_day = now + Duration::hours(25);
    let error = load_models(
        &cache, &transport, &*keys, &profile, &strict, next_day, false,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::StaleModelList);
    assert_eq!(transport.request_count(), 2);

    // Inside the interval the failed attempt is honored: no request leaves.
    let soon = next_day + Duration::hours(1);
    let error = load_models(&cache, &transport, &*keys, &profile, &strict, soon, false)
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::StaleModelList);
    assert_eq!(transport.request_count(), 2);
    assert!(error.diagnostic.unwrap()["last_attempt_at"].is_string());

    // After the interval the refresh is attempted again.
    let later = next_day + Duration::hours(25);
    let _ = load_models(&cache, &transport, &*keys, &profile, &strict, later, false).await;
    assert_eq!(transport.request_count(), 3);
}

#[tokio::test]
async fn a_failed_first_fetch_is_not_retried_until_the_interval_passes() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path());
    let policy = ModelListPolicy::default();
    let keys = keys();
    let profile = profile(Protocol::Responses);
    let now = Utc::now();
    let transport = FixtureTransport::new(
        [FixtureResponse::json(
            500,
            r#"{"error":{"message":"down"}}"#,
        )],
        64,
    );
    let error = load_models(&cache, &transport, &*keys, &profile, &policy, now, false)
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MissingModelList);
    assert_eq!(transport.request_count(), 1);

    let soon = now + Duration::hours(1);
    let error = load_models(&cache, &transport, &*keys, &profile, &policy, soon, false)
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MissingModelList);
    assert_eq!(
        transport.request_count(),
        1,
        "throttled attempt must not fetch"
    );

    // Forcing bypasses the throttle.
    let _ = load_models(&cache, &transport, &*keys, &profile, &policy, soon, true).await;
    assert_eq!(transport.request_count(), 2);
}

#[tokio::test]
async fn a_probe_fetches_live_and_reports_the_raw_failure_without_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path());
    let keys = keys();
    let profile = profile(Protocol::ChatCompletions);
    let now = Utc::now();
    let transport = FixtureTransport::new(
        [
            FixtureResponse::json(200, fixture("chat-completions", "models.json")),
            FixtureResponse::json(
                401,
                format!(r#"{{"error":{{"message":"invalid key {TEST_KEY}"}}}}"#),
            ),
        ],
        32,
    );
    let fresh = probe_models(&cache, &transport, &*keys, &profile, now)
        .await
        .unwrap();
    assert!(matches!(fresh.status, ModelListStatus::Fresh { fetched_at } if fetched_at == now));
    assert!(!fresh.models.is_empty());

    // A second probe a minute later fetches again: the interval does not
    // apply to a probe. The failure comes back as the provider's own
    // error, key-redacted, and the cached list survives it.
    let error = probe_models(
        &cache,
        &transport,
        &*keys,
        &profile,
        now + Duration::minutes(1),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Provider);
    assert_eq!(error.diagnostic.as_ref().unwrap()["status"], 401);
    assert!(!error.message.contains(TEST_KEY));
    assert!(!format!("{:?}", error.diagnostic).contains(TEST_KEY));
    assert_eq!(transport.request_count(), 2);
    let cached = cache.read(&profile.name).unwrap().unwrap();
    assert_eq!(cached.fetched_at, Some(now));
    assert_eq!(cached.last_attempt_at, Some(now + Duration::minutes(1)));
    assert_eq!(cached.models, fresh.models);

    // With nothing queued the failure is a transport error with no status.
    let error = probe_models(&cache, &transport, &*keys, &profile, now)
        .await
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Provider);
    assert!(error.diagnostic.is_none());
}

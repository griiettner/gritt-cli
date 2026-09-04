//! Live provider checks. Gated by `GRITT_LIVE_TESTS=1` plus the profile's
//! key variable; never required for a pass and skipped silently otherwise.

mod common;

use std::sync::Arc;

use common::*;
use gritt_core::provider::{Protocol, ProviderProfile};
use gritt_core::session::SessionId;
use gritt_provider::{
    adapter_for, AdapterContext, CancellationToken, EnvKeys, NoCapabilities, ReqwestTransport,
};

fn live_profile(protocol: Protocol) -> Option<ProviderProfile> {
    if std::env::var("GRITT_LIVE_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    let profile = profile(protocol);
    std::env::var(&profile.key.env_var_name)
        .ok()
        .filter(|v| !v.is_empty())?;
    Some(profile)
}

async fn run(protocol: Protocol) {
    let Some(profile) = live_profile(protocol) else {
        eprintln!("skipping live {protocol:?}: GRITT_LIVE_TESTS or the key variable is unset");
        return;
    };
    let context = AdapterContext {
        profile,
        session_id: SessionId("live".into()),
        transport: Arc::new(ReqwestTransport::new().unwrap()),
        keys: Arc::new(EnvKeys),
        capabilities: Arc::new(NoCapabilities),
        cancel: CancellationToken::new(),
    };
    let adapter = adapter_for(context);
    let mut request = prompt(protocol, false);
    request.options.max_tokens = Some(32);
    let events = collect(adapter.send(request).await.unwrap()).await;
    assert!(!text_of(&events).is_empty(), "{protocol:?}: no text");
    assert!(kinds(&events).last().unwrap().starts_with("completed"));
}

#[tokio::test]
async fn live_chat_completions() {
    run(Protocol::ChatCompletions).await;
}

#[tokio::test]
async fn live_responses() {
    run(Protocol::Responses).await;
}

#[tokio::test]
async fn live_messages() {
    run(Protocol::Messages).await;
}

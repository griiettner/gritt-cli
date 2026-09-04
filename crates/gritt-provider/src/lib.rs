//! Provider adapters for Gritt (ADR-007, ADR-008).
//!
//! One [`gritt_core::provider::ProviderAdapter`] implementation per wire
//! protocol: [`chat_completions`] serves OpenRouter, OpenAI in Chat
//! Completions mode, and generic endpoints; [`responses`] is OpenAI
//! Responses; [`messages`] is Anthropic Messages. Each has its own
//! normalizer and tool-schema generator and shares only the event model.
//! [`models`] fetches and caches model lists, [`alias`] resolves names and
//! deprecations, and [`embeddings`] holds the opt-in gateway clients.
//! Nothing above an adapter learns which provider served a request.

pub mod adapter;
pub mod alias;
pub mod cancel;
pub mod chat_completions;
pub mod embeddings;
pub mod messages;
pub mod models;
pub mod responses;
pub mod sse;
pub mod transport;

use std::sync::Arc;

use gritt_core::provider::{Protocol, ProviderAdapter};

pub use adapter::{
    AdapterContext, CapabilitySource, EnvKeys, KeyProvider, NoCapabilities, StaticKey,
};
pub use cancel::CancellationToken;
pub use models::{ModelCache, ModelCatalog};
pub use transport::{FixtureResponse, FixtureTransport, HttpTransport, ReqwestTransport};

/// Builds the adapter for the context's profile protocol.
pub fn adapter_for(context: AdapterContext) -> Arc<dyn ProviderAdapter> {
    match context.profile.protocol {
        Protocol::ChatCompletions => {
            Arc::new(chat_completions::ChatCompletionsAdapter::new(context))
        }
        Protocol::Responses => Arc::new(responses::ResponsesAdapter::new(context)),
        Protocol::Messages => Arc::new(messages::MessagesAdapter::new(context)),
    }
}

//! Provider adapters for Gritt.
//!
//! Adapters implement [`gritt_core::provider::ProviderAdapter`] per wire
//! protocol. This crate depends on `gritt-core` only. HTTP, SSE parsing,
//! normalizers, and the model list cache land in TKT-0010.

pub use gritt_core;

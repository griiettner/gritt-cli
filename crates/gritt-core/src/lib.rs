//! Provider-neutral contracts for Gritt.
//!
//! Every type here is shared by the provider adapters, the terminal harness,
//! the connectors, and the binary. Nothing in this crate performs I/O: no
//! filesystem, network, terminal, or async runtime dependency. A type that
//! needs one of those belongs one crate up (ADR-006).
//!
//! Provider-specific data never appears as a typed field. It travels only as
//! optional diagnostic metadata on an [`event::Event`] (ADR-007).

pub mod config;
pub mod connector;
pub mod embeddings;
pub mod error;
pub mod event;
pub mod policy;
pub mod provider;
pub mod secret;
pub mod session;
pub mod telemetry;
pub mod tool;

pub use error::{Error, ErrorKind, Result};

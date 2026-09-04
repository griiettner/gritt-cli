//! External agent connectors for Gritt.
//!
//! Connectors implement [`gritt_core::connector::Connector`] for the native
//! path and for installed agents such as Codex and Claude Code. Process
//! supervision, PTY fallback, and event normalization land in TKT-0012.

pub use gritt_core;

//! Terminal harness for Gritt (ADR-009): the embedded session store, the
//! permission engine, workspace-bounded native tools, the native agent
//! loop, local telemetry, and the print, REPL, and full-screen modes.
//!
//! Print mode is the fallback every feature degrades to. The full-screen
//! mode uses Ratatui 0.30 with its Crossterm 0.29 backend.

// The native turn's nested futures need this depth for Send trait checking.
#![recursion_limit = "256"]

pub mod agent;
pub mod connector_session;
pub mod control;
pub mod draft;
pub mod driver;
pub mod mcp;
pub mod modes;
pub mod native_connector;
pub mod policy;
pub mod setup;
pub mod store;
pub mod telemetry;
pub mod tools;
pub mod tui;

pub use gritt_provider::CancellationToken;

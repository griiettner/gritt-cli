//! Terminal harness for Gritt (ADR-009): the embedded session store, the
//! permission engine, workspace-bounded native tools, the native agent
//! loop, local telemetry, and the print, REPL, and full-screen modes.
//!
//! Print mode is the fallback every feature degrades to. The full-screen
//! mode uses Ratatui 0.30 with its Crossterm 0.29 backend.

pub mod agent;
pub mod modes;
pub mod policy;
pub mod store;
pub mod telemetry;
pub mod tools;
pub mod tui;

pub use gritt_provider::CancellationToken;

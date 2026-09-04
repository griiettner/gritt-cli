//! Full-screen mode (ADR-009) on Ratatui 0.30 with the Crossterm 0.29
//! backend. `app` holds the state and key reducer, testable without a
//! terminal; `render` draws it; `run` owns the terminal and the event loop.

pub mod app;
pub mod render;
pub mod run;

pub use app::{Action, App, Entry, EntryKind, View};
pub use run::run_tui;

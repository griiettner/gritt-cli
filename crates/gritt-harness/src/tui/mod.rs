//! Full-screen mode (ADR-009) on Ratatui 0.30 with the Crossterm 0.29
//! backend.
//!
//! The split is deliberate: `app` holds every piece of state and the key
//! reducer and is testable without a terminal, `render` draws it, and
//! `run` owns the terminal and the event loop. `theme`, `composer`,
//! `command`, `picker`, and `sidebar` are the pieces `app` composes;
//! `fixture` builds the reviewable prototype state.

pub mod app;
pub mod command;
pub mod composer;
pub mod fixture;
pub mod picker;
pub mod render;
pub mod run;
pub mod sidebar;
pub mod theme;

pub use app::{Action, App, Entry, EntryKind, Focus, Layout, Metrics, Overlay, PickerKind, View};
pub use command::Command;
pub use composer::Composer;
pub use fixture::FixtureScreen;
pub use picker::{ListStatus, Picker, PickerRow};
pub use run::{run_fixture, run_tui};
pub use sidebar::{SidebarModel, SidebarPlacement};
pub use theme::{Theme, ThemeMode};

//! Project-local agent CLI for the Gritt repository.
//!
//! The binary is `gritt-agent`. It replaced the Node maintenance scripts for
//! local memory, ticket metadata, skill adapters, chain scaffolding, Codex
//! trust, and Cursor migration. Product code lives in a separate Cargo
//! workspace; this crate is repository tooling only.

pub mod codex;
pub mod delegate;
pub mod error;
pub mod frontmatter;
pub mod fsx;
pub mod memory;
pub mod mcp;
pub mod migrate;
pub mod repo;
pub mod skill;
pub mod ticket;

pub use error::{CliError, Result};

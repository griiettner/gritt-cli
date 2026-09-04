//! Project-local agent CLI for the Gritt repository.
//!
//! The binary is `gritt-agent`. It replaces the Node maintenance scripts for
//! local memory, ticket metadata, and skill adapters. Product code lives in a
//! separate Cargo workspace; this crate is repository tooling only.

pub mod error;
pub mod frontmatter;
pub mod fsx;
pub mod memory;
pub mod repo;
pub mod skill;
pub mod ticket;

pub use error::{CliError, Result};

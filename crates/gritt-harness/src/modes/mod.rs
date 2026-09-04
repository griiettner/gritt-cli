//! Print and REPL modes. Print mode is the fallback every feature degrades
//! to; REPL adds history and continuation on top of it.

pub mod print;
pub mod repl;

pub use print::{PrintUi, PrintUiOptions};
pub use repl::{run_repl, ReplCommand};

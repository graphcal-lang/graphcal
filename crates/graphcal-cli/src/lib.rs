//! Library surface for the `graphcal` CLI.
//!
//! The binary (`main.rs`) is the imperative shell: it parses arguments, reads
//! and writes files, and prints to stdout/stderr. The reusable functional core
//! lives here so it can be tested without spawning the process.
#![allow(
    clippy::disallowed_methods,
    reason = "CLI adapters are an imperative shell where reviewed graceful-degradation fallbacks are intentional"
)]

pub mod format;

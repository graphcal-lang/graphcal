//! Typed, bounded Graphcal project generators shared by property tests and fuzzers.
#![allow(
    clippy::expect_used,
    reason = "trusted generator constants and test-only invariant checks may panic explicitly"
)]

#[cfg(not(target_family = "wasm"))]
pub mod bytes;
#[cfg(not(target_family = "wasm"))]
pub mod project;

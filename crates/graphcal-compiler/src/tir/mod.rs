//! Graphcal TIR: typed intermediate representation and dimension checking.

pub mod dim_check;
#[expect(clippy::module_inception, reason = "preserved from pre-merge crate structure")]
pub mod tir;

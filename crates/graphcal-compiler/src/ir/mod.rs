//! Graphcal IR: name resolution and intermediate representation lowering.

pub mod fn_check;
#[expect(
    clippy::module_inception,
    reason = "preserved from pre-merge crate structure"
)]
pub mod ir;
pub mod resolve;

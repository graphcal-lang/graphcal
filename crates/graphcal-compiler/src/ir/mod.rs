//! Graphcal IR: declaration collection and intermediate representation lowering.

pub mod imported_binding;
pub mod instance;
pub mod lower;
pub(crate) mod override_reconciliation;
pub(crate) mod required_bindability;
pub mod resolve;
pub mod static_interface;

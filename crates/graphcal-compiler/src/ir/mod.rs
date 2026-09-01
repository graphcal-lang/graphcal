//! Graphcal IR: declaration collection and intermediate representation lowering.

pub(crate) mod extern_fns;
pub mod imported_binding;
pub(crate) mod include;
pub mod instance;
pub mod lower;
pub(crate) mod override_reconciliation;
pub(crate) mod registry_build;
pub(crate) mod required_bindability;
pub mod resolve;
pub mod static_dependencies;
#[cfg(test)]
mod static_external_surface_formal_conformance;
pub mod static_interface;

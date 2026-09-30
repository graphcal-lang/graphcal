//! Graphcal IR: declaration collection and intermediate representation lowering.

pub mod decl_table;
pub mod entry;
pub(crate) mod extern_fns;
pub mod extern_function;
mod freeze;
pub mod imported_binding;
pub mod include;
pub mod instance;
pub mod lower;
pub mod model;
pub mod module_definitions;
pub mod module_interface;
mod node_definition;
pub(crate) mod override_reconciliation;
pub mod prelude_definitions;
pub(crate) mod required_bindability;
pub mod resolve;
pub mod static_definitions;
pub mod static_dependencies;
#[cfg(test)]
mod static_external_surface_formal_conformance;
pub mod static_substitution;

//! Pure whole-project compilation from loaded syntax to [`CheckedProject`].
//!
//! This module owns module interfaces, import/include elaboration, canonical
//! template checking, and project-wide semantic validation. Runtime planning
//! and evaluation consume its checked result through [`crate::prepare::PreparedProject`].

mod binding_values;
mod checked_project;
mod checking;
mod entry_interface;
mod generic_leakage;
mod hir_project;
mod imports;
mod including_module;
mod lowering;
mod model;
mod module_resolve_errors;
mod pipeline;

mod session;
mod template;

pub use checked_project::CheckedProject;
pub(crate) use checked_project::CheckedProjectRuntimeParts;
pub(crate) use checked_project::CompiledFile;
pub(crate) use entry_interface::CheckedEntryInterface;
pub use hir_project::HirProject;
pub(crate) use model::IncludeDebugNameMap;
pub use session::{ProjectCompiler, check_project};
#[cfg(test)]
pub(crate) use session::{compile_to_tir, compile_to_tir_from_project};

//! Pure whole-project compilation from loaded syntax to [`CheckedProject`].
//!
//! This module owns module interfaces, import/include elaboration, canonical
//! template checking, and project-wide semantic validation. Runtime planning
//! and evaluation consume its checked result through [`crate::eval::PreparedProject`].

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use miette::NamedSource;

#[cfg(test)]
use graphcal_compiler::graphcal_error::GraphcalError;

mod binding_values;
mod checked_project;
mod checking;
mod entry_interface;
mod execution_check;
mod generic_leakage;
mod hir_project;
mod imports;
mod lowering;
mod model;
mod module_resolve_errors;
mod pipeline;

mod session;
mod template;

pub use checked_project::CheckedProject;
pub(crate) use checked_project::CheckedProjectRuntimeParts;
pub(crate) use entry_interface::CheckedEntryInterface;
/// Test-only sealing of a single checked TIR without checked modules.
#[cfg(test)]
pub(crate) fn seal_checked_program_with_cancellation(
    tir: graphcal_compiler::tir::typed::CheckedTir,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<crate::checked_program::CheckedProgram, GraphcalError> {
    execution_check::seal_checked_program(
        tir,
        &crate::checked_program::ExecutionFacts::default(),
        src,
        cancellation,
    )
}

#[cfg(test)]
pub(crate) fn resolve_struct_field_constraints(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    const_values: &crate::constant_pools::RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<
    HashMap<
        graphcal_compiler::tir::typed::StructFieldConstraintKey,
        crate::domain_constraint::ResolvedDomainConstraint,
    >,
    GraphcalError,
> {
    execution_check::resolve_struct_field_constraints(tir, const_values, src)
}
pub use hir_project::HirProject;
pub(crate) use model::{CompiledFile, IncludeDebugNameMap};
pub use session::{ProjectCompiler, check_project};
#[cfg(test)]
pub(crate) use session::{compile_to_tir, compile_to_tir_project};

//! Pure whole-project compilation from loaded syntax to [`CheckedProject`].
//!
//! This module owns module interfaces, import/include elaboration, canonical
//! template checking, and project-wide semantic validation. Runtime planning
//! and evaluation consume its checked result through [`crate::eval::PreparedProject`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::desugar::desugared_ast::ModulePath;
use graphcal_compiler::ir::imported_binding::{HirImportedBinding, ImportedBinding};
use graphcal_compiler::ir::resolve::{DeclCategory, ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::resolve_types::ExternalDeclSurface;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{IndexBindingTarget, Registry, RegistryBuilder};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::dimension::DimName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::module_name::{IncludeInstanceScope, ModuleAliasName};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::StructTypeName;

use crate::eval::types::CompileError;
pub(in crate::project_compiler) use crate::import_surface::{
    ImportItemPresence, decl_has_external_role, declarations_expose_import_item,
    declarations_have_import_item, extract_external_decl_surface, file_import_item_presence,
    import_item_not_found_error_from_declarations,
};

mod checking;
mod entry_interface;
mod execution_check;
mod generic_leakage;
mod hir_project;
mod imports;
mod lowering;
mod model;
mod pipeline;
mod qualified_refs;
mod recursion;
mod registry_merge;
mod session;
mod template;

pub(crate) use entry_interface::CheckedEntryInterface;
#[cfg(test)]
pub(crate) fn check_execution_facts_with_cancellation(
    tir: &graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<crate::execution_facts::CheckedExecutionFacts, GraphcalError> {
    execution_check::check_execution_facts_with_inherited(
        tir,
        &crate::execution_facts::CheckedExecutionFacts::empty(),
        src,
        cancellation,
    )
}

#[cfg(test)]
pub(crate) fn resolve_struct_field_constraints(
    tir: &graphcal_compiler::tir::typed::TIR,
    const_values: &crate::execution_facts::RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<
    HashMap<
        graphcal_compiler::tir::typed::StructFieldConstraintKey,
        crate::domain_check::ResolvedDomainConstraint,
    >,
    GraphcalError,
> {
    execution_check::resolve_struct_field_constraints(tir, const_values, src)
}
pub use hir_project::HirProject;
pub(crate) use model::{CompiledFile, IncludeDebugNameMap};
use model::{
    DepToImporter, FrontendRegistryImport, HirFile, ImportAlias, ImportContext,
    IncludeInstanceRequest, IndexBindings, LoweringModuleInterface, ModuleArtifact,
    ModuleArtifactStore, ProjectModuleBinding, ProjectSemanticContext, ProjectedStaticAlias,
    RuntimeUnitBoundary, UnitProjectionAlias,
};
use qualified_refs::rewrite_qualified_refs_in_compilation_body;
pub(crate) use session::CheckedProjectRuntimeParts;
pub use session::{
    CheckedProject, ProjectCompiler, check_project, check_project_with_host_fns,
    check_project_with_host_fns_and_cancellation, check_project_with_host_metadata,
    check_project_with_host_metadata_and_cancellation,
};
#[cfg(test)]
pub(crate) use session::{compile_to_tir, compile_to_tir_project};
use template::{ElaboratedModuleTemplate, ModuleTemplateRef, ModuleTemplateStore};

/// Derive the source-facing module alias from a module path leaf.
fn derive_module_name_from_import_path(import_path: &ModulePath) -> ModuleAliasName {
    ModuleAliasName::from_atom(import_path.leaf().name.clone())
}

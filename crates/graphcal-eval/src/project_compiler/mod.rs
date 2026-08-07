//! Pure whole-project compilation from loaded syntax to [`CheckedProject`].
//!
//! This module owns module interfaces, import/include elaboration, canonical
//! template checking, and project-wide semantic validation. Runtime planning
//! and evaluation consume its checked result through [`crate::eval::PreparedProject`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::desugar::desugared_ast::{Expr, ModulePath};
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
    ImportItemPresence, decl_has_external_role, extract_external_decl_surface,
    file_exposes_import_item, file_has_import_item, file_import_item_presence,
};

mod checking;
mod entry_interface;
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
pub use hir_project::HirProject;
pub(crate) use model::{CompiledFile, IncludeDebugNameMap};
use model::{
    DepToImporter, DynamicUnitBoundary, FrontendRegistryImport, HirFile, ImportAlias,
    ImportContext, IncludeInstanceRequest, IndexBindings, LoweringModuleInterface, ModuleArtifact,
    ProjectModuleBinding, ProjectSemanticContext,
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

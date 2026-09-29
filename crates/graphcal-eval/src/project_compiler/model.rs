//! Internal data model shared by project-checking passes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::desugar::desugared_ast::Expr;
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::ir::static_substitution::StaticSubstitution;
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::IndexBindingTarget;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::resolved_name::ResolvedIndexName;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::dimension::UnitName;
use graphcal_compiler::syntax::module_name::IncludeInstanceId;
use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopeSegment};
use graphcal_compiler::syntax::span::Span;

/// One include's Static bindings, resolved once at the include site.
///
/// Every bound template port is its canonical identity and every target is
/// the canonical importer-side definition it names; no consumer resolves an
/// authored name again.
#[derive(Debug, Default)]
pub(super) struct IncludeStaticBindings {
    pub(super) substitution: StaticSubstitution,
    /// Authored spelling and source site of each bound index port, for
    /// diagnostics of its binding contract.
    pub(super) index_sites: HashMap<ResolvedIndexName, IndexBindingSite>,
}

/// Diagnostic provenance of one index port binding.
#[derive(Debug)]
pub(super) struct IndexBindingSite {
    pub(super) authored: IndexBindingTarget,
    pub(super) span: Span,
}

/// Presentation aliases for private selective-include scopes.
pub type IncludeDebugNameMap = HashMap<IncludeInstanceId, ModuleAliasName>;

/// A selective import/include alias with explicit source and local roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImportAlias {
    pub(super) original: DeclName,
    pub(super) local: DeclName,
}

/// One fully resolved physical file before static checking.
#[derive(Debug)]
pub(super) struct HirFile {
    pub(super) source: NamedSource<Arc<String>>,
    pub(super) root: graphcal_compiler::ir::lower::HirDag,
    /// Inline DAG bodies carry their own canonical keys; no parallel tuple key
    /// can disagree with the body identity.
    pub(super) inline_dags: Vec<graphcal_compiler::ir::lower::HirDag>,
    pub(super) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    pub(super) output_surface: HashSet<ScopedName>,
    pub(super) include_debug_names: IncludeDebugNameMap,
    pub(super) module_map: HashMap<ModuleAliasName, ProjectModuleBinding>,
}

/// Checked compile-time artifact made available to downstream modules.
pub(super) struct ModuleArtifact {
    pub(super) declared_types_by_dag:
        HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
    pub(super) override_dependencies: graphcal_compiler::tir::dim_check::OverrideDependencySummary,
    /// The module's own bodies, frozen once and shared by every importer.
    pub(super) dag_store: Arc<graphcal_compiler::tir::typed::DagStore>,
    pub(super) extern_functions: HashMap<
        graphcal_compiler::plugin_identity::ExternFnKey,
        graphcal_compiler::ir::lower::ExternFunctionEntry,
    >,
}

/// Checked module artifacts indexed both by physical file and canonical DAG owner.
///
/// The owner index keeps imported type/value lookup O(1) without erasing the
/// physical-file grouping needed when dependency TIRs are merged.
#[derive(Default)]
pub(super) struct ModuleArtifactStore {
    by_file: HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
    owner_to_file: HashMap<graphcal_compiler::dag_id::DagId, graphcal_compiler::dag_id::DagId>,
}

impl ModuleArtifactStore {
    pub(super) fn insert(
        &mut self,
        file: graphcal_compiler::dag_id::DagId,
        artifact: ModuleArtifact,
    ) -> Result<(), ModuleArtifactOwnerConflict> {
        if let Some(owner) = artifact
            .declared_types_by_dag
            .keys()
            .find(|owner| self.owner_to_file.contains_key(*owner))
        {
            return Err(ModuleArtifactOwnerConflict {
                owner: owner.clone(),
            });
        }
        self.owner_to_file.extend(
            artifact
                .declared_types_by_dag
                .keys()
                .cloned()
                .map(|owner| (owner, file.clone())),
        );
        self.by_file.insert(file, artifact);
        Ok(())
    }

    pub(super) fn for_owner(
        &self,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Option<&ModuleArtifact> {
        self.owner_to_file
            .get(owner)
            .and_then(|file| self.by_file.get(file))
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &ModuleArtifact> {
        self.by_file.values()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("canonical DAG owner `{owner}` was emitted by more than one module artifact")]
pub(super) struct ModuleArtifactOwnerConflict {
    owner: graphcal_compiler::dag_id::DagId,
}

/// Result of checking one file in project context.
pub struct CompiledFile {
    pub(crate) tir: graphcal_compiler::tir::typed::TIR,
    pub(crate) checked_execution_facts: crate::execution_facts::CheckedExecutionFacts,
    pub(crate) entry_interface: super::CheckedEntryInterface,
    pub(crate) declared_types: HashMap<ScopedName, DeclaredType>,
    pub(crate) imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    pub(crate) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    pub(crate) output_surface: HashSet<ScopedName>,
    pub(crate) include_debug_names: IncludeDebugNameMap,
}

/// One typed dynamic-unit projection requested by a selective include.
pub(super) struct UnitProjectionAlias {
    pub(super) source: UnitName,
    pub(super) alias: UnitName,
}

/// Typed request for one concrete file-root or inline-DAG instance.
pub(super) struct IncludeInstanceRequest<'a> {
    /// Reusable file-root or inline-DAG template module.
    pub(super) template: crate::loader::LoadedModule<'a>,
    pub(super) instance_scope: ScopeSegment,
    pub(super) debug_scope: ModuleAliasName,
    pub(super) bindings: HashMap<DeclName, Expr>,
    pub(super) static_bindings: IncludeStaticBindings,
    pub(super) selective_names: Option<Vec<ImportAlias>>,
    pub(super) unit_projection_aliases: Vec<UnitProjectionAlias>,
    pub(super) runtime_unit_names: HashSet<UnitName>,
    pub(super) assertion_aliases: HashMap<DeclName, DeclName>,
    pub(super) surface_outputs: Vec<ScopedName>,
    pub(super) requested_plots: HashMap<DeclName, graphcal_compiler::ir::lower::RequestedPlot>,
    pub(super) include_span: Span,
    pub(super) import_item_attributes:
        HashMap<DeclName, Vec<graphcal_compiler::desugar::desugared_ast::Attribute>>,
    /// Namespace-agnostic surface atoms selected with `{ pub name }`.
    pub(super) pub_reexport_items: HashSet<graphcal_compiler::syntax::names::NameAtom>,
}

/// Canonical routing metadata for one module alias.
#[derive(Debug, Clone)]
pub(super) struct ProjectModuleBinding {
    pub(super) target: graphcal_compiler::dag_id::DagId,
    pub(super) span: Span,
    pub(super) role: graphcal_compiler::resolve::scope::ModuleAliasRole,
}

impl ProjectModuleBinding {
    pub(super) const fn span(&self) -> Span {
        self.span
    }
}

/// Mutable state accumulated while processing one body's imports.
pub(super) struct ImportContext<'a> {
    pub(super) imported_names: ImportedValueNames,
    pub(super) imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    pub(super) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    pub(super) module_map: HashMap<ModuleAliasName, ProjectModuleBinding>,
    pub(super) include_instances: Vec<IncludeInstanceRequest<'a>>,
}

//! Internal data model shared by project-checking passes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::desugar::desugared_ast::Expr;
use graphcal_compiler::ir::imported_binding::HirImportedBinding;
use graphcal_compiler::ir::resolve::{DeclCategory, ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::resolve_types::ExternalDeclSurface;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{IndexBindingTarget, Registry};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::dimension::{DimName, UnitName};
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::module_name::{IncludeInstanceScope, ModuleAliasName};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::StructTypeName;

use super::template::{ModuleTemplateRef, ModuleTemplateStore};

/// Dependency-side name to importer-side name.
pub(super) type DepToImporter<T> = HashMap<T, T>;

/// Dependency index port to a declared or structural importer-side axis.
pub(super) type IndexBindings = HashMap<IndexName, IndexBindingTarget>;

/// Presentation aliases for private selective-include scopes.
pub type IncludeDebugNameMap = HashMap<ModuleAliasName, ModuleAliasName>;

/// A selective import/include alias with explicit source and local roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImportAlias {
    pub(super) original: DeclName,
    pub(super) local: DeclName,
}

/// Frontend interface available only while lowering downstream modules.
///
/// The derived dynamic-unit set is constructed atomically with the registry
/// and export surface, so consumers cannot disagree about which exported units
/// are runtime-dependent. It contains no checked type, value, or runtime fact.
pub(super) struct LoweringModuleInterface {
    frontend_registry: Registry,
    external_surface: ExternalDeclSurface,
    exported_dynamic_units: HashSet<UnitName>,
}

impl LoweringModuleInterface {
    pub(super) fn new(frontend_registry: Registry, external_surface: ExternalDeclSurface) -> Self {
        let exported_dynamic_units = frontend_registry
            .units
            .all_units()
            .filter(|(unit, info)| {
                !unit.is_qualified()
                    && info.scale.is_dynamic()
                    && external_surface.is_unit_explicit_export(unit.name().atom())
            })
            .map(|(unit, _)| unit.name().clone())
            .collect();
        Self {
            frontend_registry,
            external_surface,
            exported_dynamic_units,
        }
    }

    pub(super) const fn frontend_registry(&self) -> &Registry {
        &self.frontend_registry
    }

    pub(super) const fn external_surface(&self) -> &ExternalDeclSurface {
        &self.external_surface
    }

    pub(super) fn is_exported_dynamic_unit(&self, name: &UnitName) -> bool {
        self.exported_dynamic_units.contains(name)
    }

    pub(super) const fn exported_dynamic_units(&self) -> &HashSet<UnitName> {
        &self.exported_dynamic_units
    }
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
    pub(super) checked_execution_facts: crate::execution_facts::CheckedExecutionFacts,
    pub(super) const_values_by_dag:
        HashMap<graphcal_compiler::dag_id::DagId, HashMap<DeclName, RuntimeValue>>,
    pub(super) declared_types_by_dag:
        HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
    pub(super) override_dependencies: graphcal_compiler::tir::dim_check::OverrideDependencySummary,
    pub(super) dag_tirs: graphcal_compiler::tir::typed::DagRegistry,
    pub(super) extern_functions: HashMap<
        graphcal_compiler::syntax::plugin::ExternFnKey,
        graphcal_compiler::ir::lower::ExternFunctionEntry,
    >,
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

/// Project-wide semantic services shared by every module lowering pass.
pub(super) struct ProjectSemanticContext<'project> {
    pub(super) project: &'project crate::loader::LoadedProject,
    pub(super) module_resolver: &'project graphcal_compiler::syntax::module_resolve::ModuleResolver,
    pub(super) module_templates: &'project mut ModuleTemplateStore,
}

/// Typed request for one concrete file-root or inline-DAG instance.
pub(super) struct IncludeInstanceRequest {
    pub(super) template: ModuleTemplateRef,
    pub(super) instance_scope: IncludeInstanceScope,
    pub(super) debug_scope: ModuleAliasName,
    pub(super) bindings: HashMap<DeclName, Expr>,
    pub(super) index_bindings: IndexBindings,
    pub(super) index_binding_spans: HashMap<IndexName, Span>,
    pub(super) type_bindings: DepToImporter<StructTypeName>,
    pub(super) dim_bindings: DepToImporter<DimName>,
    pub(super) selective_names: Option<Vec<ImportAlias>>,
    pub(super) assertion_aliases: HashMap<DeclName, DeclName>,
    pub(super) surface_outputs: Vec<ScopedName>,
    pub(super) requested_plots: HashMap<DeclName, graphcal_compiler::ir::lower::RequestedPlot>,
    pub(super) include_span: Span,
    pub(super) import_item_attributes:
        HashMap<DeclName, Vec<graphcal_compiler::desugar::desugared_ast::Attribute>>,
    pub(super) pub_reexport_items: HashSet<DeclName>,
}

/// Canonical routing metadata for one module alias.
#[derive(Debug, Clone)]
pub(super) struct ProjectModuleBinding {
    pub(super) target: graphcal_compiler::dag_id::DagId,
    pub(super) span: Span,
    pub(super) role: graphcal_compiler::syntax::module_resolve::ModuleAliasRole,
}

impl ProjectModuleBinding {
    pub(super) const fn span(&self) -> Span {
        self.span
    }
}

/// Mutable state accumulated while processing one body's imports.
pub(super) struct ImportContext<'a> {
    pub(super) imported_names: ImportedValueNames,
    pub(super) imported_bindings: HashMap<ScopedName, HirImportedBinding>,
    pub(super) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    pub(super) imported_type_system_names: HashMap<
        graphcal_compiler::dag_id::DagId,
        graphcal_compiler::ir::lower::SelectedDeclarations,
    >,
    pub(super) module_map: HashMap<ModuleAliasName, ProjectModuleBinding>,
    pub(super) frontend_registry_imports: Vec<FrontendRegistryImport<'a>>,
    pub(super) include_instances: Vec<IncludeInstanceRequest>,
}

/// Whether registry composition is a pure import or concrete instance boundary.
#[derive(Debug, Clone, Copy)]
pub(super) enum DynamicUnitBoundary {
    PureImport,
    ConcreteInstance,
}

impl DynamicUnitBoundary {
    pub(super) const fn includes_dynamic_units(self) -> bool {
        matches!(self, Self::ConcreteInstance)
    }
}

/// Frontend registry surface queued for a module import.
pub(super) struct FrontendRegistryImport<'a> {
    pub(super) registry: &'a Registry,
    pub(super) external_surface: &'a ExternalDeclSurface,
    pub(super) unit_alias: ModuleAliasName,
    pub(super) dynamic_unit_boundary: DynamicUnitBoundary,
    pub(super) import_span: Span,
}

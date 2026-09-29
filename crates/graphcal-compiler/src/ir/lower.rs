//! Per-DAG HIR construction from a desugared syntax tree.
//!
//! `lower()` combines declaration collection (`resolve`), registry
//! construction (dimensions, units, indexes, structs), and function
//! registration into one [`HirDag`]. Reference resolution happens at
//! [`UnfrozenIR::freeze`], which lowers every assembled declaration body to
//! HIR — a frozen DAG carries no syntax-AST expression.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::declaration_category::DeclCategory;
use crate::desugar::desugared_ast::{DeclKind, Expr, File, TypeExpr};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::ir::instance::InstanceRecord;
use crate::ir::resolve::{CollectedFile, ImportedValueNames, resolve_with_imported_values};
use crate::plot_visibility::PlotVisibility;
use crate::registry::error::GraphcalError;
use crate::registry::prelude::load_prelude;
use crate::registry::resolve_types::ExternalDeclSurface;
use crate::registry::resolve_types::ParsedExpectedFail;
use crate::registry::types::{Registry, RegistryBuilder, SemanticRegistry};
use crate::resolved_name::{ResolvedDeclName, ResolvedUnitName};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{UnitName, UnitRef};
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;

use super::entry::{self, BodyPhase, Syntax};
#[cfg(test)]
use super::extern_fns::resolve_extern_struct_return;
pub use super::extern_fns::{ExternFunctionEntry, ExternStructResult};
pub use super::include::{
    IncludeOverrideReconciliations, SemanticInstanceInput, specialize_type_definition,
    substitute_dim_expr_names, substitute_type_expr_indexes, substitute_type_expr_nominal_names,
};
use super::registry_build::register_file_declarations;
pub use super::registry_build::{
    SelectedDeclarations, SelectedDimension, register_selected_declarations,
};

// ---------------------------------------------------------------------------
// Entry types for IR declarations
// ---------------------------------------------------------------------------

/// One plot declaration's expressions lowered to HIR, in source order.
#[derive(Debug, Clone, Default)]
pub struct LoweredPlotBody {
    /// Encoding channel expressions (`x: ...`, `y: ...`).
    pub encodings: Vec<(crate::syntax::ast::EncodingChannel, crate::hir::CheckedExpr)>,
    /// Mark property expressions (`stroke_width: ...`).
    pub mark_properties: Vec<LoweredPlotField>,
    /// Plot-level property expressions (`title: ...`).
    pub properties: Vec<LoweredPlotField>,
}

/// Typed classification of a plot, mark, or composition property name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoweredPlotProperty {
    Mark(crate::plot_props::MarkProperty),
    Plot(crate::plot_props::PlotProperty),
    Composition(crate::plot_props::CompositionProperty),
    Unknown(crate::syntax::ast::PlotPropertyName),
}

impl LoweredPlotProperty {
    pub(super) fn mark(name: crate::syntax::ast::PlotPropertyName) -> Self {
        crate::plot_props::MarkProperty::from_name(name.as_str())
            .map(Self::Mark)
            .unwrap_or(Self::Unknown(name))
    }

    pub(super) fn plot(name: crate::syntax::ast::PlotPropertyName) -> Self {
        crate::plot_props::PlotProperty::from_name(name.as_str())
            .map(Self::Plot)
            .unwrap_or(Self::Unknown(name))
    }

    pub(super) fn composition(name: crate::syntax::ast::PlotPropertyName) -> Self {
        crate::plot_props::CompositionProperty::from_name(name.as_str())
            .map(Self::Composition)
            .unwrap_or(Self::Unknown(name))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Mark(property) => property.name(),
            Self::Plot(property) => property.name(),
            Self::Composition(property) => property.name(),
            Self::Unknown(name) => name.as_str(),
        }
    }
}

/// A typed plot/figure/layer field expression lowered to HIR.
#[derive(Debug, Clone)]
pub struct LoweredPlotField {
    pub property: LoweredPlotProperty,
    /// Span of the property name in the source, for validation diagnostics.
    pub(crate) name_span: crate::syntax::span::Span,
    pub value: crate::hir::CheckedExpr,
}

/// Unresolved expected-fail metadata with the scope that authored it.
///
/// Include assembly may rename the assertion key, but the attribute's index
/// paths and spans remain owned by the file where the attribute was written.
#[derive(Debug, Clone)]
pub(crate) struct ParsedExpectedFailMetadata {
    pub(crate) expected: ParsedExpectedFail,
    pub(crate) resolution_owner: crate::dag_id::DagId,
    pub(crate) attribute_span: Span,
}

/// Frozen phase: every body is strictly lowered HIR with canonical references.
#[derive(Debug, Clone, Copy)]
pub enum Lowered {}

impl BodyPhase for Lowered {
    type Expr = crate::hir::CheckedExpr;
    type TypeAnnotation = crate::hir::TypeAnnotation;
    type NodeDefinition = crate::hir::node_definition::NodeDefinition;
    type AssertBody = crate::hir::CheckedAssertBody;
    type PlotBody = LoweredPlotBody;
    type CompositionFields = Vec<LoweredPlotField>;
    type UnitIdentity = ResolvedUnitName;
}

/// A const declaration with type annotation and lowered body.
pub type ConstEntry = entry::ConstEntry<Lowered>;
/// A param declaration with type annotation and an atomic lowered default.
pub type ParamEntry = entry::ParamEntry<Lowered>;
/// A node declaration with type annotation and lowered body.
pub type NodeEntry = entry::NodeEntry<Lowered>;
/// An assert declaration with lowered body.
pub type AssertEntry = entry::AssertEntry<Lowered>;
/// A plot declaration with lowered body.
pub type PlotEntry = entry::PlotEntry<Lowered>;
/// A figure declaration with lowered fields.
pub type FigureEntry = entry::FigureEntry<Lowered>;
/// A layer declaration with lowered fields.
pub type LayerEntry = entry::LayerEntry<Lowered>;
/// A validated dynamic unit scale lowered to HIR.
pub type DynamicUnitScaleEntry = entry::DynamicUnitScaleEntry<Lowered>;

/// A plot alias brought into this DAG by an include brace list (#847).
///
/// The plot itself is evaluated in its owning instance; this entry only
/// makes the alias known to the DAG so figures/layers can reference it and
/// duplicate-name checks see it.
#[derive(Debug, Clone)]
pub struct IncludedPlotEntry {
    /// The local alias the plot is visible under.
    pub name: ScopedName,
}

/// A plot requested by an include brace list item (#847).
#[derive(Debug, Clone)]
pub struct RequestedPlot {
    /// The local alias the plot enters the root namespace under.
    pub alias: DeclName,
    /// Composition-only when the include item carried `#[hidden]`.
    pub visibility: PlotVisibility,
}

/// Intermediate Representation produced by [`lower`].
///
/// Contains everything downstream stages need:
/// - A `Registry` with dimensions, units, indexes, structs, and functions
/// - Declarations (consts, params, nodes) with their expressions
/// - Dependency graphs for const and runtime evaluation ordering
/// - Source-order tracking for deterministic output
#[derive(Debug)]
pub struct HirDag {
    /// Canonical identity carried by the body itself, so storage and consumers
    /// cannot pair this HIR with a different DAG key.
    pub(super) dag_id: crate::dag_id::DagId,
    /// Registry capabilities valid from HIR onward. Syntax-backed nominal
    /// definitions are unrepresentable; `nominal_types` is the sole authority.
    pub registry: SemanticRegistry,
    /// Local nominal definitions with all signatures lowered to canonical HIR.
    pub(super) nominal_types: crate::hir::NominalTypeRegistry,
    /// Const declarations in source order.
    pub(crate) consts: Vec<ConstEntry>,
    /// Param declarations in source order.
    pub(crate) params: Vec<ParamEntry>,
    /// Node declarations in source order.
    pub(crate) nodes: Vec<NodeEntry>,
    /// Assert declarations in source order.
    pub(crate) asserts: Vec<AssertEntry>,
    /// Plot declarations in source order.
    pub(crate) plots: Vec<PlotEntry>,
    /// Figure declarations in source order.
    pub(crate) figures: Vec<FigureEntry>,
    /// Layer declarations in source order.
    pub(crate) layers: Vec<LayerEntry>,
    /// Plot aliases from include brace lists (#847).
    pub(crate) included_plots: Vec<IncludedPlotEntry>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// Runtime-interface-relevant declarations authored directly in this DAG,
    /// excluding declarations merged from includes.
    pub(super) source_declarations: Vec<crate::hir::SourceDeclaration>,
    /// Typed Static ports authored directly in this reusable DAG.
    pub(crate) static_ports: Vec<crate::hir::StaticPort>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub(crate) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Expected-fail metadata keyed by assertion name, retaining its authored scope and source.
    pub(crate) expected_fail: HashMap<ScopedName, ParsedExpectedFailMetadata>,
    /// Strictly lowered, source-qualified dynamic unit scale definitions.
    pub(crate) dynamic_unit_scales: Vec<DynamicUnitScaleEntry>,
    /// Imported declarations keyed by their source-visible lexical binding.
    ///
    /// HIR retains only canonical targets. Checked types and optional values
    /// are attached atomically when this IR becomes TIR.
    pub(crate) imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    /// Resolved extern function signatures declared by `import plugin`
    /// blocks, keyed by canonical plugin identity plus function name.
    pub(crate) extern_functions: HashMap<crate::plugin_identity::ExternFnKey, ExternFunctionEntry>,
    /// Explicit exports and annotation-free `param` input ports, kept in
    /// distinct roles for downstream boundary checks.
    pub external_surface: ExternalDeclSurface,
    /// Semantic instance edges with importer-context value bindings.
    pub(crate) semantic_instances: Vec<crate::ir::instance::HirInstanceRecord>,
}

impl HirDag {
    /// Canonical identity of this HIR body.
    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        &self.dag_id
    }

    /// Nominal definitions canonically owned by this DAG.
    #[must_use]
    pub const fn nominal_types(&self) -> &crate::hir::NominalTypeRegistry {
        &self.nominal_types
    }

    /// Borrow canonical lexical import targets resolved during HIR lowering.
    #[must_use]
    pub const fn imported_bindings(&self) -> &HashMap<ScopedName, ResolvedDeclName> {
        &self.imported_bindings
    }

    /// Runtime unit definitions after concrete include ownership has been
    /// assigned.
    #[must_use]
    pub fn dynamic_unit_scales(&self) -> &[DynamicUnitScaleEntry] {
        &self.dynamic_unit_scales
    }

    /// Declarations authored directly in this DAG that define its runtime
    /// parameter, output, and required-index interface.
    #[must_use]
    pub fn source_declarations(&self) -> &[crate::hir::SourceDeclaration] {
        &self.source_declarations
    }

    /// Typed Static interface authored directly in this DAG.
    #[must_use]
    pub fn static_ports(&self) -> &[crate::hir::StaticPort] {
        &self.static_ports
    }

    /// Extern signatures declared by this DAG's own `import plugin` blocks,
    /// keyed by canonical plugin identity plus function name.
    #[must_use]
    pub const fn extern_functions(
        &self,
    ) -> &HashMap<crate::plugin_identity::ExternFnKey, ExternFunctionEntry> {
        &self.extern_functions
    }
}

/// Lower an AST into a [`HirDag`].
///
/// This combines:
/// 1. Name resolution (`resolve`) — checks duplicates, extracts deps
/// 2. Registry construction — registers dimensions, units, indexes, structs from declarations
/// 3. Function registration — registers user-defined functions into the registry
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails
/// (e.g., unknown dimension in a type annotation, duplicate names, etc.).
pub fn lower(ast: &File, src: &NamedSource<Arc<String>>) -> Result<HirDag, GraphcalError> {
    let dag_id = crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(src.name()))
        .map_err(|error| {
            GraphcalError::internal_error(
                format!("invalid source name `{}`: {error}", src.name()),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let (builder, unresolved) = lower_to_builder_with_imported_bindings(
        ast,
        src,
        &ImportedValueNames::default(),
        HashMap::new(),
        &dag_id,
        None,
    )?;
    let resolver = single_module_resolver(ast, &dag_id, src)?;
    let registry = builder.build();
    unresolved.freeze(registry, &dag_id, &resolver, src)
}

#[cfg(test)]
pub(crate) fn lower_with_frontend_registry_for_test(
    ast: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(HirDag, Registry), GraphcalError> {
    let dag_id = crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(src.name()))
        .map_err(|error| {
            GraphcalError::internal_error(
                format!("invalid source name `{}`: {error}", src.name()),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let (builder, unresolved) = lower_to_builder_with_imported_bindings(
        ast,
        src,
        &ImportedValueNames::default(),
        HashMap::new(),
        &dag_id,
        None,
    )?;
    let resolver = single_module_resolver(ast, &dag_id, src)?;
    let registry = builder.build();
    let hir = unresolved.freeze(registry.clone(), &dag_id, &resolver, src)?;
    Ok((hir, registry))
}

/// Build a resolver covering only this file's own module.
///
/// Single-file lowering has no project loader, so imported modules are not
/// resolvable; bodies that reference them fail at the freeze boundary just
/// as they previously failed during type resolution.
fn single_module_resolver(
    ast: &File,
    dag_id: &crate::dag_id::DagId,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::resolve::ModuleResolver, GraphcalError> {
    let mut tables = crate::resolve::builder::SymbolTables::default();
    tables
        .add_file(dag_id.clone(), &ast.declarations)
        .and_then(|()| {
            tables
                .scopes(&crate::resolve::builder::NoModuleTargets)?
                .freeze()
        })
        .map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })
}

fn collect_static_ports(ast: &File, owner: &crate::dag_id::DagId) -> Vec<crate::hir::StaticPort> {
    ast.declarations
        .iter()
        .filter_map(|declaration| {
            let interface = crate::static_interface::static_interface(&declaration.kind)?;
            let identity = match &declaration.kind {
                DeclKind::Type(type_decl) => crate::hir::StaticPortIdentity::Type(
                    crate::resolved_name::ResolvedStructTypeName::from_def(
                        owner.clone(),
                        type_decl.name.value.clone(),
                    ),
                ),
                DeclKind::BaseDimension(dimension) => crate::hir::StaticPortIdentity::Dimension(
                    crate::resolved_name::ResolvedDimName::from_def(
                        owner.clone(),
                        dimension.name.value.clone(),
                    ),
                ),
                DeclKind::Dimension(dimension) => crate::hir::StaticPortIdentity::Dimension(
                    crate::resolved_name::ResolvedDimName::from_def(
                        owner.clone(),
                        dimension.name.value.clone(),
                    ),
                ),
                DeclKind::Index(index) => crate::hir::StaticPortIdentity::Index(
                    crate::resolved_name::ResolvedIndexName::from_def(
                        owner.clone(),
                        index.name.value.clone(),
                    ),
                ),
                _ => return None,
            };
            Some(crate::hir::StaticPort {
                identity,
                role: interface.role(),
                span: declaration.span,
            })
        })
        .collect()
}

fn collect_source_declarations(ast: &File) -> Vec<crate::hir::SourceDeclaration> {
    ast.declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Param(param) => Some(crate::hir::SourceDeclaration::Parameter {
                name: param.name.value.clone(),
                span: declaration.span,
            }),
            DeclKind::Node(node) => Some(crate::hir::SourceDeclaration::Node {
                name: node.name.value.clone(),
                span: declaration.span,
            }),
            DeclKind::Index(index) => Some(crate::hir::SourceDeclaration::Index {
                name: index.name.value.clone(),
                span: declaration.span,
            }),
            _ => None,
        })
        .collect()
}

/// Hook that merges imported type-system declarations into the registry builder.
///
/// Invoked after the prelude is loaded but before the file's own
/// declarations are registered, so local declarations (e.g. a `unit`
/// definition referencing an imported unit) resolve against the imported
/// entries.
pub type RegistrySeed<'a> = &'a mut dyn FnMut(&mut RegistryBuilder) -> Result<(), GraphcalError>;

/// Lower an AST with imported value bindings, returning a `RegistryBuilder`
/// that can be further mutated before freezing.
///
/// Imported lexical names are added to the resolution scope without injecting
/// parallel AST expressions. Canonical targets remain attached to those names
/// in `imported_bindings`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_to_builder_with_imported_bindings(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    lower_to_builder_with_imported_bindings_and_cancellation(
        ast,
        src,
        imported_names,
        imported_bindings,
        dag_id,
        registry_seed,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Lower an AST with imported bindings and cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid source or cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_to_builder_with_imported_bindings_and_cancellation(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    cancellation.checkpoint()?;
    let resolved = resolve_with_imported_values(ast, src, imported_names, dag_id)?;
    let (builder, mut unfrozen) = build_ir_from_resolved(
        ast,
        src,
        resolved,
        imported_bindings,
        dag_id,
        None,
        registry_seed,
        cancellation,
    )?;

    // Plot aliases from include brace lists become known to this DAG so
    // figures/layers can reference them (#847).
    unfrozen.included_plots = imported_names
        .plot_names
        .iter()
        .map(|(name, _span)| IncludedPlotEntry { name: name.clone() })
        .collect();

    Ok((builder, unfrozen))
}

/// Lower a `dag { ... }` body as if it were a standalone file.
///
/// The dag body is a virtual [`File`] whose registry is seeded with the
/// enclosing file's frozen registry. Cross-scope values must be passed through
/// params or explicit imports; every imported lexical name maps to its
/// canonical [`ResolvedDeclName`] target.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or type-system construction
/// fails for the dag body.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_dag_module_to_builder_with_imported_bindings(
    dag_body: &File,
    parent_registry: Option<&Registry>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    lower_dag_module_to_builder_with_imported_bindings_and_cancellation(
        dag_body,
        parent_registry,
        imported_names,
        imported_bindings,
        src,
        dag_id,
        registry_seed,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Lower an inline DAG module with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid source or cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "dag-module lowering threads imported bindings, registry state, and cancellation"
)]
pub fn lower_dag_module_to_builder_with_imported_bindings_and_cancellation(
    dag_body: &File,
    parent_registry: Option<&Registry>,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    cancellation.checkpoint()?;
    let resolved = resolve_with_imported_values(dag_body, src, imported_names, dag_id)?;

    build_ir_from_resolved(
        dag_body,
        src,
        resolved,
        imported_bindings,
        dag_id,
        parent_registry,
        registry_seed,
        cancellation,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "dag-body lowering threads imported bindings plus parent registry"
)]
#[cfg(test)]
pub(crate) fn lower_dag_body_to_ir(
    dag_name: &crate::syntax::decl_name::DeclName,
    stripped_body: &[crate::desugar::desugared_ast::Declaration],
    parent_registry: &Registry,
    resolver: &crate::resolve::ModuleResolver,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
) -> Result<HirDag, GraphcalError> {
    let virtual_file = File {
        declarations: stripped_body.to_vec(),
    };
    let dag_dag_id = parent_dag_id.inline_dag_child(dag_name.clone());
    let (builder, unfrozen) = lower_dag_module_to_builder_with_imported_bindings(
        &virtual_file,
        Some(parent_registry),
        imported_names,
        imported_bindings,
        src,
        &dag_dag_id,
        None,
    )?;
    let registry = builder.build();
    unfrozen.freeze(registry, &dag_dag_id, resolver, src)
}

/// Result of `preprocess_dag_body_self_imports`: imported names, canonical
/// bindings, and the body with self-import declarations stripped.
pub struct DagBodySelfImports {
    pub names: ImportedValueNames,
    pub bindings: HashMap<ScopedName, ResolvedDeclName>,
    pub stripped_body: Vec<crate::desugar::desugared_ast::Declaration>,
}

/// Shared implementation for local and imported-binding lowering.
///
/// Builds the registry, augments runtime deps for dynamic units, and
/// constructs the `UnfrozenIR` from the collected declaration entries.
#[expect(
    clippy::too_many_arguments,
    reason = "IR construction threads imported bindings and registry state"
)]
fn build_ir_from_resolved(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    resolved: CollectedFile,
    imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    dag_id: &crate::dag_id::DagId,
    parent_registry: Option<&Registry>,
    registry_seed: Option<RegistrySeed<'_>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    cancellation.checkpoint()?;
    // Build registry (prelude + user-declared dimensions/units/indexes/structs).
    // When a parent registry is provided (inline-dag bodies), its entries are
    // merged in before registering the virtual file's own declarations so that
    // type annotations and dynamic-unit dep augmentation see the enclosing
    // file's type system.
    let mut builder = RegistryBuilder::new();
    load_prelude(&mut builder).map_err(|error| {
        GraphcalError::internal_error(
            format!("prelude failed to load: {error}"),
            src,
            DiagnosticAnchor::Builtin,
        )
    })?;
    if let Some(parent) = parent_registry {
        builder.merge_from_registry(parent);
    }
    // Imported type-system declarations merge before the file's own so that
    // local declarations (e.g. `const unit halfmile: Length = 0.5 u.mile;`) resolve
    // against them.
    if let Some(seed) = registry_seed {
        seed(&mut builder)?;
    }
    let dynamic_unit_scales = register_file_declarations(ast, &mut builder, src, dag_id)?;
    cancellation.checkpoint()?;

    let unfrozen = UnfrozenIR {
        consts: resolved.consts,
        params: resolved.params,
        nodes: resolved.nodes,
        asserts: resolved.asserts,
        plots: resolved.plots,
        figures: resolved.figures,
        layers: resolved.layers,
        included_plots: Vec::new(),
        source_order: resolved
            .source_order
            .into_iter()
            .map(|(name, cat)| (ScopedName::from(name), cat))
            .collect(),
        source_declarations: collect_source_declarations(ast),
        static_ports: collect_static_ports(ast, dag_id),
        assumes_map: resolved
            .assumes_map
            .into_iter()
            .map(|(k, v)| {
                (
                    ScopedName::from(k),
                    v.into_iter().map(ScopedName::from).collect(),
                )
            })
            .collect(),
        expected_fail: resolved
            .expected_fail
            .into_iter()
            .map(|(name, collected)| {
                (
                    ScopedName::from(name),
                    ParsedExpectedFailMetadata {
                        expected: collected.expected,
                        resolution_owner: dag_id.clone(),
                        attribute_span: collected.attribute_span,
                    },
                )
            })
            .collect(),
        dynamic_unit_scales,
        unit_bindings: HashMap::new(),
        imported_bindings,
        external_surface: resolved.external_surface,
        plugin_imports: ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                DeclKind::PluginImport(plugin) => Some(plugin.clone()),
                _ => None,
            })
            .collect(),
        semantic_instances: Vec::new(),
    };

    Ok((builder, unfrozen))
}

/// Value declaration metadata needed to create a selective include alias.
#[derive(Debug, Clone)]
pub struct IncludeAliasDeclaration {
    /// Producer-owned type annotation before importer-side Static substitution.
    pub type_ann: TypeExpr,
    /// Whether the alias must remain in the compile-time constant category.
    pub is_const: bool,
}

/// An IR without a frozen registry, awaiting a call to [`freeze`](Self::freeze).
#[derive(Debug, Clone)]
pub struct UnfrozenIR {
    pub(super) consts: Vec<entry::ConstEntry<Syntax>>,
    pub(super) params: Vec<entry::ParamEntry<Syntax>>,
    pub(super) nodes: Vec<entry::NodeEntry<Syntax>>,
    pub(super) asserts: Vec<entry::AssertEntry<Syntax>>,
    pub(super) plots: Vec<entry::PlotEntry<Syntax>>,
    pub(super) figures: Vec<entry::FigureEntry<Syntax>>,
    pub(super) layers: Vec<entry::LayerEntry<Syntax>>,
    /// Plot aliases from include brace lists (#847).
    pub(super) included_plots: Vec<IncludedPlotEntry>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// Direct source declarations are immutable provenance. Include merging
    /// extends `source_order` but never this entry-interface subset.
    pub(super) source_declarations: Vec<crate::hir::SourceDeclaration>,
    /// Static interface provenance is authored only by this DAG template.
    pub(super) static_ports: Vec<crate::hir::StaticPort>,
    // Key-lookup only, order irrelevant.
    pub(super) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    // Key-lookup only, order irrelevant. Each value retains authored scope/source.
    pub(super) expected_fail: HashMap<ScopedName, ParsedExpectedFailMetadata>,
    // Dynamic unit scales declared by this source body.
    pub(super) dynamic_unit_scales: Vec<entry::DynamicUnitScaleEntry<Syntax>>,
    // Source-visible projected units mapped to concrete instance identities.
    pub(super) unit_bindings: HashMap<UnitRef, ResolvedUnitName>,
    // Lexical binding lookup only; each value carries one canonical target.
    pub(super) imported_bindings: HashMap<ScopedName, ResolvedDeclName>,
    // Explicit exports and named `param` input ports used by downstream
    // import/include boundary checks.
    pub(super) external_surface: ExternalDeclSurface,
    /// Plugin-import declarations, awaiting signature resolution against the
    /// frozen registry in [`UnfrozenIR::freeze`].
    pub(super) plugin_imports: Vec<crate::desugar::desugared_ast::PluginImportDecl>,
    /// Semantic instance edges awaiting importer-context HIR lowering.
    pub(super) semantic_instances: Vec<UnfrozenSemanticInstance>,
}

/// One semantic include edge before importer-context value expressions become HIR.
#[derive(Debug, Clone)]
pub struct UnfrozenSemanticInstance {
    pub(crate) instance: InstanceRecord,
    pub(crate) debug_scope: crate::syntax::module_name::ModuleAliasName,
    pub(crate) value_bindings: HashMap<ResolvedDeclName, Expr>,
    pub(crate) runtime_unit_names: HashSet<UnitName>,
    pub(crate) output_projections: Vec<crate::ir::instance::InstanceValueProjection>,
    pub(crate) assertion_projections: Vec<crate::ir::instance::InstanceAssertionProjection>,
    pub(crate) plot_projections: Vec<crate::ir::instance::InstancePlotProjection>,
    pub(crate) override_reconciliations: HashMap<
        ResolvedDeclName,
        Vec<crate::ir::override_reconciliation::PendingOverrideReconciliation>,
    >,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types;
    use crate::syntax::names::{NameAtom, NamePath};
    use crate::syntax::parser::Parser;
    use crate::syntax::type_name::ConstructorName;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<HirDag, GraphcalError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = crate::desugar::desugared_ast::File::from(raw_file);
        let file = desugared;
        lower(&file, &make_src(source))
    }

    #[test]
    fn lower_rocket() {
        let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.consts.len(), 1); // G0
        assert_eq!(ir.params.len(), 3); // dry_mass, fuel_mass, isp
        assert_eq!(ir.nodes.len(), 3); // v_exhaust, mass_ratio, delta_v
        assert!(
            ir.registry
                .dimensions
                .get_dimension(&crate::syntax::dimension::DimRef::local(
                    crate::syntax::dimension::DimName::expect_valid("Length")
                ))
                .is_some()
        );
        assert!(
            ir.registry
                .units
                .get_unit(&crate::syntax::dimension::UnitRef::local(
                    crate::syntax::dimension::UnitName::expect_valid("km"),
                ))
                .is_some()
        );
    }

    #[test]
    fn lower_constants() {
        let source = include_str!("../../../../tests/fixtures/valid/constants.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.consts.len(), 4);
        assert_eq!(ir.params.len(), 1);
        assert_eq!(ir.nodes.len(), 2);
    }

    #[test]
    fn lower_indexed() {
        let source = include_str!("../../../../tests/fixtures/valid/indexed.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(
            ir.registry
                .indexes
                .get_index(&crate::syntax::index_name::IndexName::expect_valid(
                    "Maneuver"
                ))
                .is_some()
        );
    }

    #[test]
    fn hir_retains_the_direct_runtime_interface_in_source_order() {
        let hir = parse_and_lower(
            "const node ignored: Dimensionless = 1.0;\n\
             pub(bind) index Phase;\n\
             param input: Dimensionless = 1.0;\n\
             node private: Dimensionless = @input;\n\
             pub node output: Dimensionless = @private;\n",
        )
        .unwrap();

        assert!(matches!(
            hir.source_declarations(),
            [
                crate::hir::SourceDeclaration::Index { name, .. },
                crate::hir::SourceDeclaration::Parameter { name: input, .. },
                crate::hir::SourceDeclaration::Node { name: private, .. },
                crate::hir::SourceDeclaration::Node { name: output, .. },
            ] if name.as_str() == "Phase"
                && input.as_str() == "input"
                && private.as_str() == "private"
                && output.as_str() == "output"
        ));
    }

    #[test]
    fn nominal_signatures_are_canonical_hir() {
        let hir = parse_and_lower(
            "type Marker { Marker }\n\
             type Box<T: Type = Marker> { Box(value: T) }\n",
        )
        .unwrap();
        let identity = crate::resolved_name::ResolvedStructTypeName::from_def(
            hir.dag_id().clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Box"),
        );
        let marker = crate::resolved_name::ResolvedStructTypeName::from_def(
            hir.dag_id().clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Marker"),
        );
        assert!(
            hir.nominal_types().get(&identity).is_some(),
            "canonical nominal definitions must cross into HIR"
        );
        let definition = hir.nominal_types().get(&identity).unwrap();
        let [parameter] = definition.generic_params() else {
            panic!("Box should retain exactly one generic parameter");
        };
        let Some(crate::hir::GenericArg::Type(default)) = parameter.default() else {
            panic!("Box#T should have a HIR type default");
        };
        assert!(matches!(
            &default.kind,
            crate::hir::ValueTypeKind::Struct(name) if name.value == marker
        ));
        let [constructor] = definition.union_members().unwrap() else {
            panic!("Box should retain exactly one constructor");
        };
        let [field] = constructor.fields() else {
            panic!("Box should retain exactly one field");
        };
        assert!(matches!(
            &field.type_annotation().decl_type,
            crate::hir::DeclType::Value(crate::hir::ValueType {
                kind: crate::hir::ValueTypeKind::GenericTypeParam(field_param),
                ..
            }) if &field_param.value == parameter.id()
        ));
        assert_eq!(definition.source().name(), "test.gcl");
    }

    #[test]
    fn lower_hohmann() {
        // hohmann.gcl uses DAG+include. The full project pipeline accepts
        // it (see the CLI tests), but single-file IR lowering rejects it at
        // the freeze boundary: include expansion is a higher-phase concern,
        // so `@transfer` (the include's projected node) cannot resolve.
        let source = include_str!("../../../../tests/fixtures/valid/hohmann.gcl");
        let err = parse_and_lower(source).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
    }

    #[test]
    fn lower_duplicate_name_error() {
        let err = parse_and_lower("param x: Dimensionless = 1.0;\nnode x: Dimensionless = 2.0;")
            .unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn duplicate_constructor_field_declarations_are_rejected() {
        let err =
            parse_and_lower("pub type Pair { Pair(value: Length, other: Bool, value: Time) }")
                .unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::DuplicateConstructorField {
                type_name,
                constructor,
                field,
                ..
            } if type_name.as_str() == "Pair"
                && constructor.as_str() == "Pair"
                && field.as_str() == "value"
        ));
    }

    #[test]
    fn same_field_name_in_distinct_constructors_is_valid() {
        parse_and_lower("pub type Choice { Left(value: Length), Right(value: Time) }").unwrap();
    }

    #[test]
    fn checked_union_member_api_rejects_duplicate_fields() {
        let field = types::StructField::new(
            crate::syntax::type_name::FieldName::expect_valid("value"),
            crate::desugar::desugared_ast::TypeExpr {
                kind: crate::desugar::desugared_ast::TypeExprKind::Dimensionless,
                constraints: Vec::new(),
                span: Span::new(0, 0),
            },
        );
        let error = types::UnionMemberDef::try_new(
            ConstructorName::expect_valid("Pair"),
            vec![field.clone(), field],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            types::TypeDefError::DuplicateConstructorField {
                first_index: 0,
                duplicate_index: 1,
                ..
            }
        ));
    }

    #[test]
    fn unknown_unit_dimension_reports_referenced_dimension() {
        let err = parse_and_lower("unit foo: Blah = 1.0 m;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnknownDimension { name, .. } if name.to_string() == "Blah"
        ));
    }

    #[test]
    fn unknown_derived_dimension_term_reports_referenced_dimension() {
        let err = parse_and_lower("dim Foo = Bar * Baz;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnknownDimension { name, .. } if name.to_string() == "Bar"
        ));
    }

    #[test]
    fn extern_struct_results_merge_only_for_the_same_record_type() {
        let declarations = |second_result: &str| {
            format!(
                "type Bounds {{ Bounds(lo: Length, hi: Length), }}\n\
                 type Range {{ Range(lo: Length, hi: Length), }}\n\
                 import plugin \"graphcal:demo\" as a {{ fn bounds(x: Length) -> Bounds; }}\n\
                 import plugin \"graphcal:demo\" as b {{ fn bounds(y: Length) -> {second_result}; }}\n"
            )
        };

        let merged = parse_and_lower(&declarations("Bounds")).unwrap();
        assert_eq!(merged.extern_functions().len(), 1);

        let err = parse_and_lower(&declarations("Range")).unwrap_err();
        assert!(
            matches!(
                &err,
                GraphcalError::InvalidExternSignature { message, .. }
                    if message.contains("different result type")
            ),
            "{err:?}"
        );
    }

    #[test]
    fn unknown_qualified_extern_dimension_preserves_its_path() {
        let path = NamePath::qualified(
            crate::syntax::non_empty::NonEmpty::singleton(NameAtom::parse("missing").unwrap()),
            NameAtom::parse("Dimension").unwrap(),
        );
        let registry = RegistryBuilder::new().build();
        let owner =
            crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl"))
                .unwrap();
        let resolver = crate::resolve::ModuleResolver::default();
        let source = make_src("missing.Dimension");

        let error = resolve_extern_struct_return(
            &path,
            Span::new(0, source.inner().len()),
            &registry,
            &owner,
            &resolver,
            &source,
        )
        .unwrap_err();

        assert!(
            matches!(
                &error,
                GraphcalError::UnknownDimension { name, .. }
                    if name.qualifier().iter().map(NameAtom::as_str).eq(["missing"])
                        && name.leaf().as_str() == "Dimension"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn static_unit_definition_must_match_declared_dimension() {
        let err = parse_and_lower("const unit wrong: Length = 1.0 h;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnitDefinitionDimensionMismatch {
                name,
                declared,
                definition,
                ..
            } if name.as_str() == "wrong"
                && declared == "Length"
                && definition == "Time"
        ));
    }

    #[test]
    fn dynamic_unit_definition_must_match_declared_dimension() {
        let err = parse_and_lower(
            "param factor: Dimensionless = 2.0;\nunit wrong: Length = (@factor) h;",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnitDefinitionDimensionMismatch { name, .. }
                if name.as_str() == "wrong"
        ));
    }

    #[test]
    fn compound_unit_definition_with_matching_dimension_is_accepted() {
        parse_and_lower("const unit cruise: Velocity = 0.5 km/h;").unwrap();
    }

    #[test]
    fn failed_unit_definition_does_not_enter_registry() {
        let source = "const unit wrong: Length = 1.0 h;";
        let src = make_src(source);
        let raw_file = Parser::new(source).parse_file().unwrap();
        let file = crate::desugar::desugared_ast::File::from(raw_file);
        let mut builder = RegistryBuilder::new();
        load_prelude(&mut builder).unwrap();

        let err = register_file_declarations(
            &file,
            &mut builder,
            &src,
            &crate::dag_id::DagId::root_in_package("test", "main"),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            GraphcalError::UnitDefinitionDimensionMismatch { .. }
        ));
        assert!(
            builder
                .get_unit(&crate::syntax::dimension::UnitRef::local(
                    crate::syntax::dimension::UnitName::expect_valid("wrong"),
                ))
                .is_none()
        );
    }

    #[test]
    fn lower_source_order_preserved() {
        let ir = parse_and_lower(
            "param b: Dimensionless = 2.0;\nparam a: Dimensionless = 1.0;\nnode z: Dimensionless = @a + @b;",
        )
        .unwrap();
        let names: Vec<String> = ir.source_order.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(names, vec!["b", "a", "z"]);
    }
}

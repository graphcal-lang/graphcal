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

use crate::desugar::desugared_ast::{
    AssertBody, DeclKind, Expr, FigureDecl, File, LayerDecl, PlotDecl, TypeExpr,
};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::ir::imported_binding::HirImportedBinding;
use crate::ir::instance::InstanceRecord;
use crate::ir::resolve::{
    CollectedFile, DeclCategory, ImportedValueNames, ParsedExpectedFail,
    resolve_with_imported_values,
};
use crate::registry::error::GraphcalError;
use crate::registry::prelude::load_prelude;
use crate::registry::resolve_types::ExternalDeclSurface;
use crate::registry::types::{Registry, RegistryBuilder, SemanticRegistry};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{ResolvedUnitName, UnitRef};
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::{Span, Spanned};

#[cfg(test)]
use super::extern_fns::resolve_extern_struct_return;
pub use super::extern_fns::{ExternFunctionEntry, ExternStructResult};
pub use super::include::{
    specialize_type_definition, substitute_dim_expr_names, substitute_type_expr_indexes,
    substitute_type_expr_nominal_names,
};
pub use super::registry_build::{SelectedDeclarations, register_selected_declarations};
use super::registry_build::{
    extract_type_annotations, register_file_declarations, registry_build_error,
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

/// Where a merged declaration body's spans index into (#868).
///
/// A declaration written in the file being compiled spans into that file's
/// own [`NamedSource`], which every lowering/type stage already threads as its
/// ambient `src` — those entries carry `BodySource::own`. An instantiated
/// `include` merges a dependency's declaration bodies into the importer's IR
/// (`merge_dependency`); those bodies keep the *dependency's* byte offsets, so
/// they carry `BodySource::dependency` naming the dependency file. Rendering
/// a diagnostic for such a body against the importer's source produces an
/// out-of-bounds (or simply wrong) label; `BodySource::resolve` hands back
/// the correct source to anchor against.
#[derive(Debug, Clone, Default)]
pub struct BodySource(Option<NamedSource<Arc<String>>>);

impl BodySource {
    /// The declaration belongs to the file being compiled; its span indexes
    /// into the ambient `src` threaded through the pipeline.
    #[must_use]
    pub(super) const fn own() -> Self {
        Self(None)
    }

    /// The declaration was merged from a dependency body whose spans index
    /// into `src`.
    #[must_use]
    pub(super) const fn dependency(src: NamedSource<Arc<String>>) -> Self {
        Self(Some(src))
    }

    /// Resolve the source the span should render against, falling back to the
    /// ambient `default` source for declarations native to the compiled file.
    #[must_use]
    pub(crate) fn resolve<'a>(
        &'a self,
        default: &'a NamedSource<Arc<String>>,
    ) -> &'a NamedSource<Arc<String>> {
        self.0.as_ref().unwrap_or(default)
    }

    /// Carry an already-merged provenance forward, or attribute a still-native
    /// body to `dep_src` as it crosses one merge boundary (#868).
    ///
    /// A dependency's own declarations carry [`BodySource::own`] until they are
    /// merged, at which point their spans become foreign to the importer and
    /// must name `dep_src`. A body already tagged with a deeper dependency
    /// source (a transitively-merged include) keeps that attribution.
    #[must_use]
    pub(super) fn or_dependency(self, dep_src: &NamedSource<Arc<String>>) -> Self {
        match self.0 {
            Some(_) => self,
            None => Self::dependency(dep_src.clone()),
        }
    }
}

/// Unresolved expected-fail metadata with the scope and source that authored it.
///
/// Include assembly may rename the assertion key, but the attribute's index
/// paths and spans remain owned by the file where the attribute was written.
#[derive(Debug, Clone)]
pub(crate) struct ParsedExpectedFailMetadata {
    pub(crate) expected: ParsedExpectedFail,
    pub(crate) resolution_owner: crate::dag_id::DagId,
    pub(crate) src: BodySource,
    pub(crate) attribute_span: Span,
}

/// A const declaration with type annotation and lowered body.
#[derive(Debug, Clone)]
pub struct ConstEntry {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: crate::dag_id::DagId,
    pub type_ann: crate::hir::TypeAnnotation,
    pub(crate) expr: crate::hir::CheckedExpr,
    pub span: Span,
    /// Source of the type annotation and declaration span.
    pub(crate) type_src: BodySource,
    /// Source of the body expression and its spans.
    pub(crate) body_src: BodySource,
}

/// A lowered parameter default and the source that owns its spans.
#[derive(Debug, Clone)]
pub struct ParamDefault {
    pub expr: crate::hir::CheckedExpr,
    /// An include binding is importer-owned even when the signature is
    /// producer-owned.
    pub(crate) src: BodySource,
}

/// A param declaration with type annotation and an atomic lowered default.
#[derive(Debug, Clone)]
pub struct ParamEntry {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: crate::dag_id::DagId,
    pub type_ann: crate::hir::TypeAnnotation,
    pub default: Option<ParamDefault>,
    pub span: Span,
    /// Source of the type annotation and declaration span.
    pub(crate) type_src: BodySource,
    /// Include overrides whose nominal dependencies must be checked after
    /// canonical type inference.
    pub(crate) override_reconciliations:
        Vec<crate::ir::override_reconciliation::PendingOverrideReconciliation>,
}

/// A node declaration with type annotation and lowered body.
#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: crate::dag_id::DagId,
    pub type_ann: crate::hir::TypeAnnotation,
    pub expr: crate::hir::CheckedExpr,
    pub span: Span,
    /// Source of the type annotation and declaration span.
    pub(crate) type_src: BodySource,
    /// Source of the body expression and its spans.
    pub(crate) body_src: BodySource,
}

/// An assert declaration with lowered body.
#[derive(Debug, Clone)]
pub struct AssertEntry {
    pub name: ScopedName,
    /// Canonical semantic owner, independent of the source-facing scoped name.
    pub(crate) declaration_owner: crate::dag_id::DagId,
    pub body: crate::hir::CheckedAssertBody,
    pub span: Span,
    /// Source of the assertion body and its spans.
    pub(crate) body_src: BodySource,
}

/// A const declaration awaiting body lowering at [`UnfrozenIR::freeze`].
///
/// Pre-freeze bodies stay syntactic so include instantiation can rewrite
/// reference paths (prefixing, index/type rebinding) before resolution.
#[derive(Debug, Clone)]
pub struct UnfrozenConstEntry {
    pub(super) name: ScopedName,
    pub(super) declaration_owner: crate::dag_id::DagId,
    pub(super) type_ann: TypeExpr,
    /// Module scope for the declaration signature (type annotation and domain bounds).
    pub(super) type_resolution_owner: crate::dag_id::DagId,
    pub(super) expr: Expr,
    /// Module scope for the declaration body expression.
    pub(super) body_resolution_owner: crate::dag_id::DagId,
    pub(super) span: Span,
    /// Source of the type annotation and declaration span.
    pub(super) type_src: BodySource,
    /// Source of the body expression and its spans.
    pub(super) body_src: BodySource,
}

/// A syntactic parameter default awaiting lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub(super) struct UnfrozenParamDefault {
    pub(super) expr: Expr,
    /// Module scope used to resolve the default expression.
    pub(super) resolution_owner: crate::dag_id::DagId,
    pub(super) src: BodySource,
}

/// A param declaration awaiting default lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenParamEntry {
    pub(super) name: ScopedName,
    pub(super) declaration_owner: crate::dag_id::DagId,
    pub(super) type_ann: TypeExpr,
    /// Module scope for the parameter signature (type annotation and domain bounds).
    pub(super) type_resolution_owner: crate::dag_id::DagId,
    pub(super) default: Option<UnfrozenParamDefault>,
    pub(super) span: Span,
    /// Source of the type annotation and declaration span.
    pub(super) type_src: BodySource,
    /// Include overrides awaiting canonical typed dependency checking.
    pub(super) override_reconciliations:
        Vec<crate::ir::override_reconciliation::PendingOverrideReconciliation>,
}

/// A node declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenNodeEntry {
    pub(super) name: ScopedName,
    pub(super) declaration_owner: crate::dag_id::DagId,
    pub(super) type_ann: TypeExpr,
    /// Module scope for the declaration signature (type annotation and domain bounds).
    pub(super) type_resolution_owner: crate::dag_id::DagId,
    pub(super) expr: Expr,
    /// Module scope for the declaration body expression.
    pub(super) body_resolution_owner: crate::dag_id::DagId,
    pub(super) span: Span,
    /// Source of the type annotation and declaration span.
    pub(super) type_src: BodySource,
    /// Source of the body expression and its spans.
    pub(super) body_src: BodySource,
}

/// An assert declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenAssertEntry {
    pub(super) name: ScopedName,
    pub(super) declaration_owner: crate::dag_id::DagId,
    pub(super) body: AssertBody,
    /// Module scope for the assertion body expression(s).
    pub(super) body_resolution_owner: crate::dag_id::DagId,
    pub(super) span: Span,
    /// Source of the assertion body and its spans.
    pub(super) body_src: BodySource,
}

/// A plot declaration with lowered body.
#[derive(Debug, Clone)]
pub struct PlotEntry {
    pub name: ScopedName,
    /// Mark shape rendered for this plot.
    pub mark_type: crate::syntax::ast::MarkType,
    /// Strictly lowered semantic body.
    pub body: LoweredPlotBody,
    /// Source of every expression in the plot body.
    pub(crate) body_src: BodySource,
    /// Whether this plot renders standalone when its file is the entry
    /// point. `true` unless the declaration carries `#[hidden]` (#847).
    pub displayed: bool,
}

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

/// A validated dynamic unit scale lowered to HIR.
#[derive(Debug, Clone)]
pub struct DynamicUnitScaleEntry {
    /// Canonical declaration identity of the unit being defined.
    pub unit: ResolvedUnitName,
    /// Source spelling under which the unit is registered in this IR.
    pub spelling: UnitRef,
    /// Strictly lowered scalar expression.
    pub expr: crate::hir::CheckedExpr,
    /// Dimension declared on the unit definition.
    pub declared_dimension: Dimension,
    /// Dimension proved from the RHS base-unit expression.
    pub base_unit_dimension: Dimension,
    /// Span of the scalar expression.
    pub span: Span,
    /// Source whose bytes are indexed by `expr` and `span`.
    pub src: NamedSource<Arc<String>>,
}

/// A dynamic unit scale awaiting strict HIR lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub(super) struct UnfrozenDynamicUnitScaleEntry {
    pub(super) spelling: UnitRef,
    pub(super) expr: Expr,
    pub(super) unit_owner: crate::dag_id::DagId,
    pub(super) body_resolution_owner: crate::dag_id::DagId,
    pub(super) declared_dimension: Dimension,
    pub(super) base_unit_dimension: Dimension,
    pub(super) span: Span,
    pub(super) src: BodySource,
}

/// A plot requested by an include brace list item (#847).
#[derive(Debug, Clone)]
pub struct RequestedPlot {
    /// The local alias the plot enters the root namespace under.
    pub alias: DeclName,
    /// Whether the include item carried `#[hidden]` (composition-only).
    pub hidden: bool,
}

/// A figure declaration with lowered fields.
#[derive(Debug, Clone)]
pub struct FigureEntry {
    pub name: ScopedName,
    /// Plots composed by this figure, in source order.
    pub plot_names: Vec<Spanned<ScopedName>>,
    /// Strictly lowered field expressions.
    pub fields: Vec<LoweredPlotField>,
    /// Source of every field expression.
    pub(crate) body_src: BodySource,
}

/// A layer declaration with lowered fields.
#[derive(Debug, Clone)]
pub struct LayerEntry {
    pub name: ScopedName,
    /// Plots composed by this layer, in source order.
    pub plot_names: Vec<Spanned<ScopedName>>,
    /// Strictly lowered field expressions.
    pub fields: Vec<LoweredPlotField>,
    /// Source of every field expression.
    pub(crate) body_src: BodySource,
}

/// A plot declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenPlotEntry {
    pub(super) name: ScopedName,
    pub(super) decl: PlotDecl,
    /// Module scope for plot field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
    /// Source of every plot expression.
    pub(super) body_src: BodySource,
    /// Whether this plot renders standalone (no `#[hidden]`).
    pub(super) displayed: bool,
}

/// A figure declaration awaiting field lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenFigureEntry {
    pub(super) name: ScopedName,
    pub(super) decl: FigureDecl,
    /// Module scope for figure field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
    /// Source of every figure field expression.
    pub(super) body_src: BodySource,
}

/// A layer declaration awaiting field lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenLayerEntry {
    pub(super) name: ScopedName,
    pub(super) decl: LayerDecl,
    /// Module scope for layer field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
    /// Source of every layer field expression.
    pub(super) body_src: BodySource,
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
    pub(crate) imported_bindings: HashMap<ScopedName, HirImportedBinding>,
    /// Resolved extern function signatures declared by `import plugin`
    /// blocks, keyed by canonical plugin identity plus function name.
    pub(crate) extern_functions: HashMap<crate::syntax::plugin::ExternFnKey, ExternFunctionEntry>,
    /// Explicit exports and annotation-free `param` input ports, kept in
    /// distinct roles for downstream boundary checks.
    pub external_surface: ExternalDeclSurface,
    /// Explicit typed template-instance graph assembled by include elaboration.
    pub(crate) instances: Vec<InstanceRecord>,
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
    pub const fn imported_bindings(&self) -> &HashMap<ScopedName, HirImportedBinding> {
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
    let registry = builder
        .try_build()
        .map_err(|error| registry_build_error(&error, src))?;
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
    let registry = builder
        .try_build()
        .map_err(|error| registry_build_error(&error, src))?;
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
) -> Result<crate::syntax::module_resolve::ModuleResolver, GraphcalError> {
    fn add_module_with_dags(
        target: &mut crate::syntax::module_resolve::ModuleResolver,
        owner: &crate::dag_id::DagId,
        declarations: &[crate::desugar::desugared_ast::Declaration],
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        target
            .add_module(owner.clone(), declarations)
            .map_err(|error| {
                GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
            })?;
        for decl in declarations {
            if let crate::desugar::desugared_ast::DeclKind::Dag(dag) = &decl.kind {
                add_module_with_dags(
                    target,
                    &owner.child(dag.name.value.as_str()),
                    &dag.body,
                    src,
                )?;
            }
        }
        Ok(())
    }

    let mut resolver = crate::syntax::module_resolve::ModuleResolver::default();
    add_module_with_dags(&mut resolver, dag_id, &ast.declarations, src)?;
    Ok(resolver)
}

fn collect_static_ports(ast: &File, owner: &crate::dag_id::DagId) -> Vec<crate::hir::StaticPort> {
    ast.declarations
        .iter()
        .filter_map(|declaration| {
            let interface = crate::static_interface::static_interface(&declaration.kind)?;
            let identity = match &declaration.kind {
                DeclKind::Type(type_decl) => crate::hir::StaticPortIdentity::Type(
                    crate::syntax::type_name::ResolvedStructTypeName::from_def(
                        owner.clone(),
                        type_decl.name.value.clone(),
                    ),
                ),
                DeclKind::BaseDimension(dimension) => crate::hir::StaticPortIdentity::Dimension(
                    crate::syntax::dimension::ResolvedDimName::from_def(
                        owner.clone(),
                        dimension.name.value.clone(),
                    ),
                ),
                DeclKind::Dimension(dimension) => crate::hir::StaticPortIdentity::Dimension(
                    crate::syntax::dimension::ResolvedDimName::from_def(
                        owner.clone(),
                        dimension.name.value.clone(),
                    ),
                ),
                DeclKind::Index(index) => crate::hir::StaticPortIdentity::Index(
                    crate::syntax::index_name::ResolvedIndexName::from_def(
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
    imported_bindings: HashMap<ScopedName, HirImportedBinding>,
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
    imported_bindings: HashMap<ScopedName, HirImportedBinding>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    cancellation.checkpoint()?;
    let resolved = resolve_with_imported_values(ast, src, imported_names)?;
    let type_anns = extract_type_annotations(ast);
    let (builder, mut unfrozen) = build_ir_from_resolved(
        ast,
        src,
        resolved,
        type_anns,
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
/// params or explicit imports; every imported lexical name carries one
/// [`HirImportedBinding`] with its canonical target.
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
    imported_bindings: HashMap<ScopedName, HirImportedBinding>,
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
    imported_bindings: HashMap<ScopedName, HirImportedBinding>,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    cancellation.checkpoint()?;
    let resolved = resolve_with_imported_values(dag_body, src, imported_names)?;
    let type_anns = extract_type_annotations(dag_body);

    build_ir_from_resolved(
        dag_body,
        src,
        resolved,
        type_anns,
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
    dag_name: &str,
    stripped_body: &[crate::desugar::desugared_ast::Declaration],
    parent_registry: &Registry,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    imported_names: &ImportedValueNames,
    imported_bindings: HashMap<ScopedName, HirImportedBinding>,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
) -> Result<HirDag, GraphcalError> {
    let virtual_file = File {
        declarations: stripped_body.to_vec(),
    };
    let dag_dag_id = parent_dag_id.child(dag_name);
    let (builder, unfrozen) = lower_dag_module_to_builder_with_imported_bindings(
        &virtual_file,
        Some(parent_registry),
        imported_names,
        imported_bindings,
        src,
        &dag_dag_id,
        None,
    )?;
    let registry = builder
        .try_build()
        .map_err(|err| registry_build_error(&err, src))?;
    unfrozen.freeze(registry, &dag_dag_id, resolver, src)
}

/// Result of `preprocess_dag_body_self_imports`: imported names, canonical
/// bindings, and the body with self-import declarations stripped.
pub struct DagBodySelfImports {
    pub names: ImportedValueNames,
    pub bindings: HashMap<ScopedName, HirImportedBinding>,
    pub stripped_body: Vec<crate::desugar::desugared_ast::Declaration>,
}

/// Remove and return the type annotation for `name`, or raise an internal error
/// if it was dropped during resolution. The parser and resolver jointly
/// guarantee that every top-level const/param/node ends up in `type_anns`;
/// a missing entry is a compiler invariant violation.
fn take_type_ann(
    type_anns: &mut HashMap<DeclName, TypeExpr>,
    name: &DeclName,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<TypeExpr, GraphcalError> {
    type_anns
        .remove(name)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("missing type annotation for `{name}`"),
            src: src.clone(),
            span: span.into(),
        })
}

/// Shared implementation for local and imported-binding lowering.
///
/// Builds the registry, augments runtime deps for dynamic units, pairs resolved
/// declarations with type annotations, and constructs the `UnfrozenIR`.
#[expect(
    clippy::too_many_lines,
    reason = "single linear pipeline — splitting would obscure the flow"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "IR construction threads imported bindings and registry state"
)]
fn build_ir_from_resolved(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    resolved: CollectedFile,
    mut type_anns: HashMap<DeclName, TypeExpr>,
    imported_bindings: HashMap<ScopedName, HirImportedBinding>,
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

    // Pair resolved declarations with type annotations.
    let consts = resolved
        .consts
        .into_iter()
        .map(|entry| {
            cancellation.checkpoint()?;
            let decl_name = entry.name;
            let type_ann = take_type_ann(&mut type_anns, &decl_name, entry.span, src)?;
            Ok(UnfrozenConstEntry {
                name: ScopedName::from(decl_name),
                declaration_owner: dag_id.clone(),
                type_ann,
                type_resolution_owner: dag_id.clone(),
                expr: entry.expr,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
                type_src: BodySource::own(),
                body_src: BodySource::own(),
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    cancellation.checkpoint()?;
    let params = resolved
        .params
        .into_iter()
        .map(|entry| {
            cancellation.checkpoint()?;
            let decl_name = entry.name;
            let type_ann = take_type_ann(&mut type_anns, &decl_name, entry.span, src)?;
            let default = entry.default_expr.map(|expr| UnfrozenParamDefault {
                expr,
                resolution_owner: dag_id.clone(),
                src: BodySource::own(),
            });
            Ok(UnfrozenParamEntry {
                name: ScopedName::from(decl_name),
                declaration_owner: dag_id.clone(),
                type_ann,
                type_resolution_owner: dag_id.clone(),
                default,
                span: entry.span,
                type_src: BodySource::own(),
                override_reconciliations: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    cancellation.checkpoint()?;
    let nodes = resolved
        .nodes
        .into_iter()
        .map(|entry| {
            cancellation.checkpoint()?;
            let decl_name = entry.name;
            let type_ann = take_type_ann(&mut type_anns, &decl_name, entry.span, src)?;
            Ok(UnfrozenNodeEntry {
                name: ScopedName::from(decl_name),
                declaration_owner: dag_id.clone(),
                type_ann,
                type_resolution_owner: dag_id.clone(),
                expr: entry.expr,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
                type_src: BodySource::own(),
                body_src: BodySource::own(),
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;

    let unfrozen = UnfrozenIR {
        consts,
        params,
        nodes,
        asserts: resolved
            .asserts
            .into_iter()
            .map(|entry| UnfrozenAssertEntry {
                name: ScopedName::from(entry.name),
                declaration_owner: dag_id.clone(),
                body: entry.body,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
                body_src: BodySource::own(),
            })
            .collect(),
        plots: resolved
            .plots
            .into_iter()
            .map(|entry| {
                let displayed = !resolved.hidden_plots.contains(entry.name.as_str());
                UnfrozenPlotEntry {
                    name: ScopedName::from(entry.name),
                    decl: entry.decl,
                    body_resolution_owner: dag_id.clone(),
                    span: entry.span,
                    body_src: BodySource::own(),
                    displayed,
                }
            })
            .collect(),
        figures: resolved
            .figures
            .into_iter()
            .map(|entry| UnfrozenFigureEntry {
                name: ScopedName::from(entry.name),
                decl: entry.decl,
                body_resolution_owner: dag_id.clone(),
                body_src: BodySource::own(),
            })
            .collect(),
        layers: resolved
            .layers
            .into_iter()
            .map(|entry| UnfrozenLayerEntry {
                name: ScopedName::from(entry.name),
                decl: entry.decl,
                body_resolution_owner: dag_id.clone(),
                body_src: BodySource::own(),
            })
            .collect(),
        included_plots: Vec::new(),
        source_order: resolved
            .source_order
            .into_iter()
            .map(|(name, cat)| (ScopedName::from(name), cat))
            .collect(),
        source_declarations: collect_source_declarations(ast),
        static_ports: collect_static_ports(ast, dag_id),
        assert_names: resolved
            .assert_names
            .into_iter()
            .map(ScopedName::from)
            .collect(),
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
                        src: BodySource::own(),
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
        instances: Vec::new(),
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
    pub(super) consts: Vec<UnfrozenConstEntry>,
    pub(super) params: Vec<UnfrozenParamEntry>,
    pub(super) nodes: Vec<UnfrozenNodeEntry>,
    pub(super) asserts: Vec<UnfrozenAssertEntry>,
    pub(super) plots: Vec<UnfrozenPlotEntry>,
    pub(super) figures: Vec<UnfrozenFigureEntry>,
    pub(super) layers: Vec<UnfrozenLayerEntry>,
    /// Plot aliases from include brace lists (#847).
    pub(super) included_plots: Vec<IncludedPlotEntry>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// Direct source declarations are immutable provenance. Include merging
    /// extends `source_order` but never this entry-interface subset.
    pub(super) source_declarations: Vec<crate::hir::SourceDeclaration>,
    /// Static interface provenance is not extended by include merging.
    pub(super) static_ports: Vec<crate::hir::StaticPort>,
    pub(super) assert_names: HashSet<ScopedName>,
    // Key-lookup only, order irrelevant.
    pub(super) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    // Key-lookup only, order irrelevant. Each value retains authored scope/source.
    pub(super) expected_fail: HashMap<ScopedName, ParsedExpectedFailMetadata>,
    // Dynamic unit scales declared by this body or merged includes.
    pub(super) dynamic_unit_scales: Vec<UnfrozenDynamicUnitScaleEntry>,
    // Source-visible projected units mapped to concrete instance identities.
    pub(super) unit_bindings: HashMap<UnitRef, ResolvedUnitName>,
    // Lexical binding lookup only; each value carries one canonical target.
    pub(super) imported_bindings: HashMap<ScopedName, HirImportedBinding>,
    // Explicit exports and named `param` input ports used by downstream
    // import/include boundary checks.
    pub(super) external_surface: ExternalDeclSurface,
    /// Plugin-import declarations, awaiting signature resolution against the
    /// frozen registry in [`UnfrozenIR::freeze`].
    pub(super) plugin_imports: Vec<crate::desugar::desugared_ast::PluginImportDecl>,
    /// Explicit typed template-instance graph assembled by include elaboration.
    pub(super) instances: Vec<InstanceRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types;
    use crate::syntax::decl_name::ResolvedDeclName;
    use crate::syntax::module_name::ModuleAliasName;
    use crate::syntax::names::{NameAtom, NamePath};
    use crate::syntax::parser::Parser;
    use crate::syntax::type_name::ConstructorName;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<HirDag, GraphcalError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
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
        assert!(ir.registry.dimensions.get_dimension("Length").is_some());
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
        assert!(ir.registry.indexes.get_index("Maneuver").is_some());
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
        let identity = crate::syntax::type_name::ResolvedStructTypeName::from_def(
            hir.dag_id().clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Box"),
        );
        let marker = crate::syntax::type_name::ResolvedStructTypeName::from_def(
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
            crate::hir::TypeExprKind::Struct(name) if name.value == marker
        ));
        let [constructor] = definition.union_members().unwrap() else {
            panic!("Box should retain exactly one constructor");
        };
        let [field] = constructor.fields() else {
            panic!("Box should retain exactly one field");
        };
        assert!(matches!(
            &field.type_annotation().type_expr.kind,
            crate::hir::TypeExprKind::GenericTypeParam(field_param)
                if &field_param.value == parameter.id()
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
    fn unknown_qualified_extern_dimension_preserves_its_path() {
        let path = NamePath::qualified_path(
            [NameAtom::parse("missing").unwrap()],
            NameAtom::parse("Dimension").unwrap(),
        );
        let registry = RegistryBuilder::new().try_build().unwrap();
        let owner =
            crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new("test.gcl"))
                .unwrap();
        let resolver = crate::syntax::module_resolve::ModuleResolver::default();
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
                    if name.owner().is_some_and(|owner| owner.segments()
                        .iter().map(NameAtom::as_str).eq(["missing"]))
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
        let file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
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

    #[test]
    fn merge_dependency_scopes_qualified_import_binding_without_changing_target() {
        let dep_source = "node out: Dimensionless = 2.0;";
        let dep_src = make_src(dep_source);
        let raw_file = Parser::new(dep_source).parse_file().unwrap();
        let dep_file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let qualified = ScopedName::qualified(
            ModuleAliasName::expect_valid("mission"),
            DeclName::expect_valid("C"),
        );
        let canonical = ResolvedDeclName::from_def(
            crate::dag_id::DagId::root_in_package("producer", "config"),
            DeclName::expect_valid("C"),
        );
        let imported_names = ImportedValueNames {
            const_names: vec![(qualified.clone(), Span::new(0, 0))],
            ..ImportedValueNames::default()
        };
        let (_dep_builder, dep_unfrozen) = lower_to_builder_with_imported_bindings(
            &dep_file,
            &dep_src,
            &imported_names,
            HashMap::from([(
                qualified.clone(),
                HirImportedBinding::new(canonical.clone()),
            )]),
            &crate::dag_id::DagId::root_in_package("test", "dep"),
            None,
        )
        .unwrap();

        let importer_source = "node anchor: Dimensionless = 1.0;";
        let importer_src = make_src(importer_source);
        let raw_importer = Parser::new(importer_source).parse_file().unwrap();
        let importer_file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_importer);
        let (_importer_builder, mut unfrozen) = lower_to_builder_with_imported_bindings(
            &importer_file,
            &importer_src,
            &ImportedValueNames::default(),
            HashMap::new(),
            &crate::dag_id::DagId::root_in_package("test", "main"),
            None,
        )
        .unwrap();

        let dep_names: HashSet<DeclName> = dep_unfrozen
            .source_order
            .iter()
            .map(|(n, _)| n.member().clone())
            .collect();
        let instance = ModuleAliasName::expect_valid("inst");
        unfrozen
            .merge_dependency(
                dep_unfrozen,
                &instance,
                &HashMap::new(),
                &dep_names,
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                &crate::dag_id::DagId::root_in_package("test", "dep"),
                &crate::dag_id::DagId::root_in_package("test", "main"),
                Span::new(0, 0),
                &importer_src,
                &dep_src,
            )
            .unwrap();

        let lexical = qualified.within_scope(&instance);
        let binding = &unfrozen.imported_bindings[&lexical];
        assert_eq!(binding.target(), &canonical);
        assert!(!unfrozen.imported_bindings.contains_key(&qualified));
    }
}

//! Intermediate Representation (IR) — the result of lowering an AST.
//!
//! `lower()` combines declaration collection (`resolve`), registry
//! construction (dimensions, units, indexes, structs), and function
//! registration into a single `IR` value. Reference resolution happens at
//! [`UnfrozenIR::freeze`], which lowers every assembled declaration body to
//! HIR — the frozen `IR` carries no syntax-AST expression.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::desugar::desugared_ast::{
    AssertBody, DeclKind, DimExpr, Expr, ExprKind, FigureDecl, File, IndexDeclKind, LayerDecl,
    PlotDecl, TypeExpr,
};
use crate::dimension::{Dimension, Rational};
use crate::ir::imported_binding::ImportedBinding;
use crate::ir::resolve::{
    DeclCategory, ExpectedFail, ImportedValueNames, ParsedExpectedFail, ResolvedFile,
    resolve_with_imported_values,
};
use crate::ir::resolve::{ImportedNames, resolve_with_imports};
use crate::registry::dimension_registry::DimensionResolveError;
use crate::registry::error::GraphcalError;
use crate::registry::format::format_unit_expr_with_config;
use crate::registry::prelude::load_prelude;
use crate::registry::resolve_types::ExternalDeclSurface;
use crate::registry::types::{
    self, PositiveFiniteScale, PositiveFiniteScaleError, Registry, RegistryBuilder, UnitScale,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::DimName;
use crate::syntax::index_name::IndexName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{ConstructorName, GenericParamName, StructTypeName};
use crate::syntax::visitor::{ExprVisitor, ExprVisitorMut};

// ---------------------------------------------------------------------------
// Entry types for IR declarations
// ---------------------------------------------------------------------------

/// One plot declaration's expressions lowered to HIR, in source order.
#[derive(Debug, Clone, Default)]
pub struct LoweredPlotBody {
    /// Encoding channel expressions (`x: ...`, `y: ...`).
    pub encodings: Vec<(crate::syntax::ast::EncodingChannel, crate::hir::Expr)>,
    /// Mark property expressions (`stroke_width: ...`).
    pub mark_properties: Vec<LoweredPlotField>,
    /// Plot-level property expressions (`title: ...`).
    pub properties: Vec<LoweredPlotField>,
}

/// A named plot/figure/layer field expression lowered to HIR.
#[derive(Debug, Clone)]
pub struct LoweredPlotField {
    pub name: crate::syntax::ast::PlotPropertyName,
    /// Span of the property name in the source, for validation diagnostics.
    pub(crate) name_span: crate::syntax::span::Span,
    pub value: crate::hir::Expr,
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
    const fn own() -> Self {
        Self(None)
    }

    /// The declaration was merged from a dependency body whose spans index
    /// into `src`.
    #[must_use]
    const fn dependency(src: NamedSource<Arc<String>>) -> Self {
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
    fn or_dependency(self, dep_src: &NamedSource<Arc<String>>) -> Self {
        match self.0 {
            Some(_) => self,
            None => Self::dependency(dep_src.clone()),
        }
    }
}

/// A const declaration with type annotation and lowered body.
#[derive(Debug, Clone)]
pub struct ConstEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope used to resolve this declaration's type annotation and
    /// domain-bound expressions. Merged include outputs keep the producer's
    /// scope so consumers do not need imports for names they never wrote.
    pub(crate) type_resolution_owner: crate::dag_id::DagId,
    pub(crate) expr: crate::hir::Expr,
    pub span: Span,
    /// Provenance of this declaration's `span` (#868). `None` means the span
    /// indexes into the IR's own file source; `Some` carries the source of a
    /// dependency body merged in by an instantiated include, so diagnostics
    /// anchored on the span render against the right file.
    pub(crate) src: BodySource,
}

/// A param declaration with type annotation and lowered default.
#[derive(Debug, Clone)]
pub struct ParamEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope used to resolve this parameter's type annotation and
    /// domain-bound expressions.
    pub(crate) type_resolution_owner: crate::dag_id::DagId,
    pub default_expr: Option<crate::hir::Expr>,
    pub span: Span,
    /// Source provenance of `span`; see [`ConstEntry::src`] (#868).
    pub(crate) src: BodySource,
}

/// A node declaration with type annotation and lowered body.
#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope used to resolve this node's type annotation and
    /// domain-bound expressions.
    pub(crate) type_resolution_owner: crate::dag_id::DagId,
    pub expr: crate::hir::Expr,
    pub span: Span,
    /// Source provenance of `span`; see [`ConstEntry::src`] (#868).
    pub(crate) src: BodySource,
}

/// An assert declaration with lowered body.
#[derive(Debug, Clone)]
pub struct AssertEntry {
    pub name: ScopedName,
    pub body: crate::hir::AssertBody,
    pub span: Span,
    /// Source provenance of `span`; see [`ConstEntry::src`] (#868).
    pub(crate) src: BodySource,
}

/// A const declaration awaiting body lowering at [`UnfrozenIR::freeze`].
///
/// Pre-freeze bodies stay syntactic so include instantiation can rewrite
/// reference paths (prefixing, index/type rebinding) before resolution.
#[derive(Debug, Clone)]
pub struct UnfrozenConstEntry {
    name: ScopedName,
    type_ann: TypeExpr,
    /// Module scope for the declaration signature (type annotation and domain bounds).
    type_resolution_owner: crate::dag_id::DagId,
    expr: Expr,
    /// Module scope for the declaration body expression.
    body_resolution_owner: crate::dag_id::DagId,
    span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    src: BodySource,
}

/// A param declaration awaiting default lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenParamEntry {
    name: ScopedName,
    type_ann: TypeExpr,
    /// Module scope for the parameter signature (type annotation and domain bounds).
    type_resolution_owner: crate::dag_id::DagId,
    default_expr: Option<Expr>,
    /// Module scope for the default expression when present. Include-time
    /// param bindings are importer-written expressions, so this can differ
    /// from `type_resolution_owner`.
    default_resolution_owner: crate::dag_id::DagId,
    span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    src: BodySource,
}

/// A node declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenNodeEntry {
    name: ScopedName,
    type_ann: TypeExpr,
    /// Module scope for the declaration signature (type annotation and domain bounds).
    type_resolution_owner: crate::dag_id::DagId,
    expr: Expr,
    /// Module scope for the declaration body expression.
    body_resolution_owner: crate::dag_id::DagId,
    span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    src: BodySource,
}

/// An assert declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenAssertEntry {
    name: ScopedName,
    body: AssertBody,
    /// Module scope for the assertion body expression(s).
    body_resolution_owner: crate::dag_id::DagId,
    span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    src: BodySource,
}

/// A plot declaration with lowered body.
#[derive(Debug, Clone)]
pub struct PlotEntry {
    pub name: ScopedName,
    /// Mark shape rendered for this plot.
    pub mark_type: crate::syntax::ast::MarkType,
    /// Lowered body, or `None` when an expression failed to lower. Plots
    /// are best-effort at evaluation time: an incomplete body is skipped by
    /// the runtime instead of failing the compile.
    pub body: Option<LoweredPlotBody>,
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
    /// Lowered field expressions; fields that failed to lower are omitted
    /// (best-effort, matching plots).
    pub fields: Vec<LoweredPlotField>,
}

/// A layer declaration with lowered fields.
#[derive(Debug, Clone)]
pub struct LayerEntry {
    pub name: ScopedName,
    /// Plots composed by this layer, in source order.
    pub plot_names: Vec<Spanned<ScopedName>>,
    /// Lowered field expressions; fields that failed to lower are omitted
    /// (best-effort, matching plots).
    pub fields: Vec<LoweredPlotField>,
}

/// A plot declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenPlotEntry {
    name: ScopedName,
    decl: PlotDecl,
    /// Module scope for plot field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
    /// Whether this plot renders standalone (no `#[hidden]`).
    displayed: bool,
}

/// A figure declaration awaiting field lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenFigureEntry {
    name: ScopedName,
    decl: FigureDecl,
    /// Module scope for figure field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
}

/// A layer declaration awaiting field lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenLayerEntry {
    name: ScopedName,
    decl: LayerDecl,
    /// Module scope for layer field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
}

/// Intermediate Representation produced by [`lower`].
///
/// Contains everything downstream stages need:
/// - A `Registry` with dimensions, units, indexes, structs, and functions
/// - Declarations (consts, params, nodes) with their expressions
/// - Dependency graphs for const and runtime evaluation ordering
/// - Source-order tracking for deterministic output
#[derive(Debug)]
pub struct IR {
    /// The type/unit/dimension/index/struct/function registry.
    pub registry: Registry,
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
    /// Mapping from assert name to the list of declarations that assume it.
    pub(crate) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub(crate) expected_fail: HashMap<ScopedName, ParsedExpectedFail>,
    /// Imported values keyed by their source-visible lexical binding.
    ///
    /// Each record atomically retains the canonical declaration target, its
    /// declared type, and an optional pre-evaluated value. Include merges may
    /// prefix the lexical key, but never rewrite the canonical target.
    pub(crate) imported_bindings: HashMap<ScopedName, ImportedBinding>,
    /// Resolved extern function signatures declared by `import plugin`
    /// blocks, keyed by canonical plugin identity plus function name.
    pub(crate) extern_functions: HashMap<crate::syntax::plugin::ExternFnKey, ExternFunctionEntry>,
    /// Explicit exports and annotation-free `param` input ports, kept in
    /// distinct roles for downstream boundary checks.
    pub external_surface: ExternalDeclSurface,
}

/// Lower an AST into an [`IR`].
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
pub fn lower(ast: &File, src: &NamedSource<Arc<String>>) -> Result<IR, GraphcalError> {
    let dag_id = crate::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(src.name()))
        .map_err(|e| GraphcalError::EvalError {
            message: format!("invalid source name `{}`: {e}", src.name()),
            src: src.clone(),
            span: crate::syntax::span::Span::new(0, 0).into(),
        })?;
    lower_with_imports(ast, src, &ImportedNames::default(), &dag_id)
}

/// Lower an AST with imported declarations into an [`IR`].
///
/// Same as [`lower`] but accepts imported names from other files.
/// The registry is frozen (via `try_build()`) before returning.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails.
fn lower_with_imports(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
    dag_id: &crate::dag_id::DagId,
) -> Result<IR, GraphcalError> {
    let (builder, resolved_ir) = lower_to_builder(ast, src, imported, dag_id)?;
    let resolver = single_module_resolver(ast, dag_id, src)?;
    let registry = builder
        .try_build()
        .map_err(|err| registry_build_error(&err, src))?;
    resolved_ir.freeze(registry, dag_id, &resolver, src)
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
            .map_err(|err| GraphcalError::EvalError {
                message: err.to_string(),
                src: src.clone(),
                span: Span::new(0, 0).into(),
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

/// Lower an AST with imported declarations, returning a `RegistryBuilder`
/// that can be further mutated (e.g., to register imported type-system
/// declarations) before freezing.
///
/// Call [`UnfrozenIR::freeze`] with the final [`Registry`] to produce an [`IR`].
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails.
fn lower_to_builder(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
    dag_id: &crate::dag_id::DagId,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Declaration collection
    let resolved = resolve_with_imports(ast, src, imported)?;

    // Step 2: Extract type annotations from AST + imported declarations.
    let mut type_anns = extract_type_annotations(ast);
    for (name, type_ann, _, _) in &imported.consts {
        type_anns.insert(name.clone(), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.params {
        type_anns.insert(name.clone(), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.nodes {
        type_anns.insert(name.clone(), type_ann.clone());
    }

    // Step 3: Build registry, augment deps, and construct IR
    build_ir_from_resolved(
        ast,
        src,
        resolved,
        type_anns,
        HashMap::new(),
        dag_id,
        None,
        None,
        &crate::cancellation::CancellationToken::unbounded(),
    )
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
/// Unlike `lower_to_builder`, this uses `resolve_with_imported_values`, which
/// adds only imported lexical names to the scope. Canonical targets, declared
/// types, and optional pre-evaluated values stay atomic in `imported_bindings`.
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
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
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
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
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
/// params or explicit imports; every imported lexical name carries one atomic
/// [`ImportedBinding`] with its canonical target and declared type.
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
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
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
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
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
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
) -> Result<IR, GraphcalError> {
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
    pub bindings: HashMap<ScopedName, ImportedBinding>,
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
    resolved: ResolvedFile,
    mut type_anns: HashMap<DeclName, TypeExpr>,
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
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
    load_prelude(&mut builder).map_err(|e| GraphcalError::EvalError {
        message: format!("internal: prelude failed to load: {e}"),
        src: src.clone(),
        span: Span::new(0, 0).into(),
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
    register_file_declarations(ast, &mut builder, src, dag_id)?;
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
                type_ann,
                type_resolution_owner: dag_id.clone(),
                expr: entry.expr,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
                src: BodySource::own(),
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
            Ok(UnfrozenParamEntry {
                name: ScopedName::from(decl_name),
                type_ann,
                type_resolution_owner: dag_id.clone(),
                default_expr: entry.default_expr,
                default_resolution_owner: dag_id.clone(),
                span: entry.span,
                src: BodySource::own(),
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
                type_ann,
                type_resolution_owner: dag_id.clone(),
                expr: entry.expr,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
                src: BodySource::own(),
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
                body: entry.body,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
                src: BodySource::own(),
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
            })
            .collect(),
        layers: resolved
            .layers
            .into_iter()
            .map(|entry| UnfrozenLayerEntry {
                name: ScopedName::from(entry.name),
                decl: entry.decl,
                body_resolution_owner: dag_id.clone(),
            })
            .collect(),
        included_plots: Vec::new(),
        source_order: resolved
            .source_order
            .into_iter()
            .map(|(name, cat)| (ScopedName::from(name), cat))
            .collect(),
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
            .map(|(k, v)| (ScopedName::from(k), v))
            .collect(),
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
    };

    Ok((builder, unfrozen))
}

/// An IR without a frozen registry, awaiting a call to [`freeze`](Self::freeze).
pub struct UnfrozenIR {
    consts: Vec<UnfrozenConstEntry>,
    params: Vec<UnfrozenParamEntry>,
    nodes: Vec<UnfrozenNodeEntry>,
    asserts: Vec<UnfrozenAssertEntry>,
    plots: Vec<UnfrozenPlotEntry>,
    figures: Vec<UnfrozenFigureEntry>,
    layers: Vec<UnfrozenLayerEntry>,
    /// Plot aliases from include brace lists (#847).
    included_plots: Vec<IncludedPlotEntry>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    assert_names: HashSet<ScopedName>,
    // Key-lookup only, order irrelevant.
    assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    // Key-lookup only, order irrelevant.
    expected_fail: HashMap<ScopedName, ParsedExpectedFail>,
    // Lexical binding lookup only; each value atomically carries canonical metadata.
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
    // Explicit exports and named `param` input ports used by downstream
    // import/include boundary checks.
    external_surface: ExternalDeclSurface,
    /// Plugin-import declarations, awaiting signature resolution against the
    /// frozen registry in [`UnfrozenIR::freeze`].
    plugin_imports: Vec<crate::desugar::desugared_ast::PluginImportDecl>,
}

impl UnfrozenIR {
    /// Freeze into a complete [`IR`] by providing a built [`Registry`] and
    /// the resolution context.
    ///
    /// This is the lowering boundary of the pipeline: every declaration body
    /// assembled so far (including merged include instances and applied
    /// overrides) is lowered to HIR here, so the frozen [`IR`] carries no
    /// syntax-AST expression.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] if any body contains a reference that
    /// cannot be resolved.
    pub fn freeze(
        self,
        registry: Registry,
        owner: &crate::dag_id::DagId,
        resolver: &crate::syntax::module_resolve::ModuleResolver,
        src: &NamedSource<Arc<String>>,
    ) -> Result<IR, GraphcalError> {
        self.freeze_with_cancellation(
            registry,
            owner,
            resolver,
            src,
            &crate::cancellation::CancellationToken::unbounded(),
        )
    }

    /// Freeze into IR while observing cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] for unresolved bodies or cancellation.
    #[expect(
        clippy::too_many_lines,
        reason = "single lowering boundary over every declaration kind"
    )]
    pub fn freeze_with_cancellation(
        self,
        registry: Registry,
        owner: &crate::dag_id::DagId,
        resolver: &crate::syntax::module_resolve::ModuleResolver,
        src: &NamedSource<Arc<String>>,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<IR, GraphcalError> {
        cancellation.checkpoint()?;
        // Entries already visible in this IR (including prefixed include
        // instances and dag self-imports) bind their written names to
        // canonical identities for the lowering below.
        let mut decl_bindings = HashMap::new();
        for name in self
            .consts
            .iter()
            .map(|entry| &entry.name)
            .chain(self.params.iter().map(|entry| &entry.name))
            .chain(self.nodes.iter().map(|entry| &entry.name))
        {
            cancellation.checkpoint()?;
            let canonical = crate::hir::diagnostics::resolved_decl_key(owner, name);
            decl_bindings.insert(name.clone(), canonical);
        }
        for (name, binding) in &self.imported_bindings {
            cancellation.checkpoint()?;
            if decl_bindings
                .insert(name.clone(), binding.target().clone())
                .is_some()
            {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "imported lexical binding `{name}` collides with a local declaration"
                    ),
                    src: src.clone(),
                    span: Span::new(0, 0).into(),
                });
            }
        }

        let generic_scope = crate::hir::GenericScope::new();
        let prelude = crate::hir::PreludeTypeScope::graphcal();
        // A merged dependency body keeps the dependency file's byte offsets, so
        // a lowering error must render against that body's own source rather
        // than the importer's `src` (#868); `BodySource::resolve` selects it.
        let lower_in = |expr: &Expr,
                        resolution_owner: &crate::dag_id::DagId,
                        body_src: &NamedSource<Arc<String>>| {
            let expr_ctx = crate::hir::ExprLoweringContext::new(
                resolution_owner,
                resolver,
                &generic_scope,
                &registry.time_zones,
            )
            .with_prelude(&prelude)
            .with_unit_registry(&registry.units)
            .with_decl_bindings(&decl_bindings);
            crate::hir::lower_expr(expr, expr_ctx).map_err(|err| {
                crate::hir::diagnostics::expr_lower_error_to_graphcal(&err, body_src)
            })
        };

        let consts = self
            .consts
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(ConstEntry {
                    name: entry.name.clone(),
                    type_ann: entry.type_ann.clone(),
                    type_resolution_owner: entry.type_resolution_owner.clone(),
                    expr: lower_in(
                        &entry.expr,
                        &entry.body_resolution_owner,
                        entry.src.resolve(src),
                    )?,
                    span: entry.span,
                    src: entry.src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let params = self
            .params
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(ParamEntry {
                    name: entry.name.clone(),
                    type_ann: entry.type_ann.clone(),
                    type_resolution_owner: entry.type_resolution_owner.clone(),
                    default_expr: entry
                        .default_expr
                        .as_ref()
                        .map(|expr| {
                            lower_in(
                                expr,
                                &entry.default_resolution_owner,
                                entry.src.resolve(src),
                            )
                        })
                        .transpose()?,
                    span: entry.span,
                    src: entry.src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let nodes = self
            .nodes
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(NodeEntry {
                    name: entry.name.clone(),
                    type_ann: entry.type_ann.clone(),
                    type_resolution_owner: entry.type_resolution_owner.clone(),
                    expr: lower_in(
                        &entry.expr,
                        &entry.body_resolution_owner,
                        entry.src.resolve(src),
                    )?,
                    span: entry.span,
                    src: entry.src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let asserts = self
            .asserts
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                let body_src = entry.src.resolve(src);
                Ok(AssertEntry {
                    name: entry.name.clone(),
                    body: {
                        let expr_ctx = crate::hir::ExprLoweringContext::new(
                            &entry.body_resolution_owner,
                            resolver,
                            &generic_scope,
                            &registry.time_zones,
                        )
                        .with_prelude(&prelude)
                        .with_unit_registry(&registry.units)
                        .with_decl_bindings(&decl_bindings);
                        crate::hir::lower_assert_body(&entry.body, expr_ctx).map_err(|err| {
                            crate::hir::diagnostics::expr_lower_error_to_graphcal(&err, body_src)
                        })?
                    },
                    span: entry.span,
                    src: entry.src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        // Plots and figure/layer fields are best-effort at evaluation time:
        // an expression that fails to lower leaves the body incomplete (the
        // runtime skips it) instead of failing the compile.
        let lower_optional = |expr: &Expr, resolution_owner: &crate::dag_id::DagId| {
            let expr_ctx = crate::hir::ExprLoweringContext::new(
                resolution_owner,
                resolver,
                &generic_scope,
                &registry.time_zones,
            )
            .with_prelude(&prelude)
            .with_unit_registry(&registry.units)
            .with_decl_bindings(&decl_bindings);
            crate::hir::lower_expr(expr, expr_ctx).ok()
        };
        cancellation.checkpoint()?;
        let plots = self
            .plots
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                let mut body = LoweredPlotBody::default();
                let mut complete = true;
                for encoding in &entry.decl.encodings {
                    match lower_optional(&encoding.value, &entry.body_resolution_owner) {
                        Some(lowered) => body.encodings.push((encoding.channel, lowered)),
                        None => complete = false,
                    }
                }
                for field in &entry.decl.mark.properties {
                    match lower_optional(&field.value, &entry.body_resolution_owner) {
                        Some(lowered) => body.mark_properties.push(LoweredPlotField {
                            name: field.name.value.clone(),
                            name_span: field.name.span,
                            value: lowered,
                        }),
                        None => complete = false,
                    }
                }
                for field in &entry.decl.properties {
                    match lower_optional(&field.value, &entry.body_resolution_owner) {
                        Some(lowered) => body.properties.push(LoweredPlotField {
                            name: field.name.value.clone(),
                            name_span: field.name.span,
                            value: lowered,
                        }),
                        None => complete = false,
                    }
                }
                Ok(PlotEntry {
                    name: entry.name.clone(),
                    mark_type: entry.decl.mark.mark_type,
                    body: complete.then_some(body),
                    displayed: entry.displayed,
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        let lower_fields = |fields: &[crate::desugar::desugared_ast::PlotField],
                            resolution_owner: &crate::dag_id::DagId| {
            fields
                .iter()
                .filter_map(|field| {
                    Some(LoweredPlotField {
                        name: field.name.value.clone(),
                        name_span: field.name.span,
                        value: lower_optional(&field.value, resolution_owner)?,
                    })
                })
                .collect::<Vec<_>>()
        };
        cancellation.checkpoint()?;
        let figures = self
            .figures
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(FigureEntry {
                    name: entry.name.clone(),
                    plot_names: entry.decl.plot_names.clone(),
                    fields: lower_fields(&entry.decl.fields, &entry.body_resolution_owner),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let layers = self
            .layers
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(LayerEntry {
                    name: entry.name.clone(),
                    plot_names: entry.decl.plot_names.clone(),
                    fields: lower_fields(&entry.decl.fields, &entry.body_resolution_owner),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        cancellation.checkpoint()?;

        let extern_functions =
            resolve_plugin_imports(&self.plugin_imports, &registry, owner, resolver, src)?;

        Ok(IR {
            extern_functions,
            registry,
            consts,
            params,
            nodes,
            asserts,
            plots,
            figures,
            layers,
            included_plots: self.included_plots,
            source_order: self.source_order,
            assumes_map: self.assumes_map,
            expected_fail: self.expected_fail,
            imported_bindings: self.imported_bindings,
            external_surface: self.external_surface,
        })
    }

    /// Retarget the resolution scope of declarations already present in this
    /// unfrozen IR.
    ///
    /// Inline DAG include instances are lowered under an instance owner so
    /// merged declaration keys stay unique, but their source bodies must still
    /// resolve type-system names and constructors in the DAG's definition
    /// scope. Call this before merging nested includes so later nested entries
    /// keep their own producer scopes.
    pub fn retarget_existing_resolution_owners(&mut self, owner: &crate::dag_id::DagId) {
        for entry in &mut self.consts {
            entry.type_resolution_owner = owner.clone();
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.params {
            entry.type_resolution_owner = owner.clone();
            entry.default_resolution_owner = owner.clone();
        }
        for entry in &mut self.nodes {
            entry.type_resolution_owner = owner.clone();
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.asserts {
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.plots {
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.figures {
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.layers {
            entry.body_resolution_owner = owner.clone();
        }
    }

    /// Add a const alias: a synthetic const declaration that references another const.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_const_alias(
        &mut self,
        name: ScopedName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.consts.push(UnfrozenConstEntry {
            name: name.clone(),
            type_ann,
            type_resolution_owner,
            expr,
            body_resolution_owner,
            span,
            // Alias bodies are synthesized from the importer's include
            // statement, so their span belongs to the importer's source.
            src: BodySource::own(),
        });
        self.source_order.push((name, DeclCategory::Const));
    }

    /// Add a node alias: a synthetic node declaration that references another node/param.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_node_alias(
        &mut self,
        name: ScopedName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.nodes.push(UnfrozenNodeEntry {
            name: name.clone(),
            type_ann,
            type_resolution_owner,
            expr,
            body_resolution_owner,
            span,
            // Alias bodies are synthesized from the importer's include
            // statement, so their span belongs to the importer's source.
            src: BodySource::own(),
        });
        self.source_order.push((name, DeclCategory::Node));
    }

    /// Scan param defaults for variant literals of overridden `pub(bind)`
    /// indexes (and nominally-tied names of overridden types) whose owning
    /// `param` is not itself re-bound — axiom A8 / diagnostic V005.
    ///
    /// Per axiom §1, only `index` and `type` overrides have nominal
    /// substructure; `dim` and `param` overrides substitute totally and
    /// never trigger A8.
    ///
    /// Other non-bindable declaration kinds (`node`, `const`) are
    /// guarded at library compile time by V004 (A10c), so their bodies
    /// cannot mention overridden-symbol nominals once a library is
    /// accepted. Sink-kind declarations (`assert`, `plot`, `figure`,
    /// `layer`) pick up the A10(b) private-only carve-out; this check
    /// stays focused on `param` for that reason.
    pub fn check_include_reconciles_overrides(
        &self,
        bindings: &HashMap<DeclName, Expr>,
        index_bindings: &HashMap<IndexName, types::IndexBindingTarget>,
        type_bindings: &HashMap<StructTypeName, StructTypeName>,
        importer_src: &NamedSource<Arc<String>>,
        include_span: Span,
    ) -> Result<(), GraphcalError> {
        if index_bindings.is_empty() && type_bindings.is_empty() {
            return Ok(());
        }
        for param in &self.params {
            if bindings.contains_key(param.name.member()) {
                continue;
            }
            let Some(default_expr) = &param.default_expr else {
                continue;
            };
            let mut checker = OverrideReconciliationChecker {
                index_bindings,
                type_bindings,
                orphan_decl: param.name.member().as_str(),
                importer_src,
                include_span,
            };
            checker.visit_expr(default_expr)?;
        }
        Ok(())
    }

    /// Merge an instantiated dependency's IR into this IR.
    ///
    /// All declarations from the dependency are prefixed with `prefix.` and
    /// appended to this IR's declaration lists. Param bindings replace the
    /// dependency's param default expressions. Internal references within the
    /// dependency's expressions are rewritten to use prefixed names.
    ///
    /// `dep_names` is the set of all declaration names in the dependency (before
    /// prefixing), used to determine which references should be rewritten.
    ///
    /// `dep_src` is the dependency body's [`NamedSource`]: merged declarations
    /// keep the dependency file's byte offsets, so each is tagged with it (via
    /// `BodySource::or_dependency`) for diagnostics raised at the importer's
    /// freeze/TIR boundary (#868). For inline-DAG includes the body shares the
    /// importer's source, so `dep_src` equals `importer_src` there.
    #[expect(
        clippy::too_many_lines,
        reason = "single logical operation: prefix and merge all declaration kinds"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "merge_dependency coordinates every binding kind plus prefixing state"
    )]
    pub fn merge_dependency(
        &mut self,
        dep: Self,
        prefix: &ModuleAliasName,
        bindings: &HashMap<DeclName, Expr>,
        dep_names: &HashSet<DeclName>,
        index_bindings: &HashMap<IndexName, types::IndexBindingTarget>,
        type_bindings: &HashMap<StructTypeName, StructTypeName>,
        dim_bindings: &HashMap<DimName, DimName>,
        import_item_attributes: &HashMap<DeclName, Vec<crate::desugar::desugared_ast::Attribute>>,
        requested_plots: &HashMap<DeclName, RequestedPlot>,
        importer_owner: &crate::dag_id::DagId,
        importer_src: &NamedSource<Arc<String>>,
        dep_src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        /// Prefix a `ScopedName` if it is an unqualified member owned by
        /// the dependency.
        ///
        /// Already-qualified names that identify nested declarations owned by
        /// this dependency gain an outer instance segment. Other qualified
        /// names (for example a module import used by the dependency) retain
        /// their original external namespace.
        fn prefix_dep(
            name: &ScopedName,
            prefix: &ModuleAliasName,
            dep_names: &HashSet<DeclName>,
            dep_scoped_names: &HashSet<ScopedName>,
        ) -> ScopedName {
            if dep_scoped_names.contains(name) {
                name.within_scope(prefix)
            } else if !name.is_qualified() && dep_names.contains(name.member()) {
                name.with_prefix(prefix)
            } else {
                name.clone()
            }
        }

        // Include-time type-system bindings rewrite selected producer names
        // to importer-side identifiers. Keep those rewritten bodies/signatures
        // in the importer scope; otherwise preserve producer scope so an
        // include consumer need not import constructors/types it never names.
        let type_system_bindings_present =
            !index_bindings.is_empty() || !type_bindings.is_empty() || !dim_bindings.is_empty();
        let merge_resolution_owner = |producer_owner: crate::dag_id::DagId| {
            if type_system_bindings_present {
                importer_owner.clone()
            } else {
                producer_owner
            }
        };

        // Source-order entries are declarations owned by the dependency,
        // including declarations already nested by an inner include. Imported
        // metadata is different: a qualified key can name an external module
        // used by the dependency and must retain that canonical qualifier.
        let all_dep_scoped_names = dep
            .source_order
            .iter()
            .map(|(name, _)| name.clone())
            .chain(dep.imported_bindings.keys().cloned())
            .collect::<HashSet<_>>();
        let dep_scoped_names = &all_dep_scoped_names;

        let mut all_dep_names = dep_names.clone();
        all_dep_names.extend(dep_scoped_names.iter().map(|name| name.member().clone()));
        let dep_names = &all_dep_names;

        // Merge consts
        for mut entry in dep.consts {
            substitute_indexes(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names, dep_scoped_names);
            substitute_type_expr_indexes(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            let prefixed = entry.name.within_scope(prefix);
            self.consts.push(UnfrozenConstEntry {
                name: prefixed.clone(),
                type_ann: entry.type_ann,
                type_resolution_owner: merge_resolution_owner(entry.type_resolution_owner),
                expr: entry.expr,
                body_resolution_owner: merge_resolution_owner(entry.body_resolution_owner),
                span: entry.span,
                src: entry.src.or_dependency(dep_src),
            });
            self.source_order.push((prefixed, DeclCategory::Const));
        }

        // Merge params — replace defaults with bindings where provided
        for mut entry in dep.params {
            let prefixed = entry.name.within_scope(prefix);
            let default_resolution_owner =
                if let Some(binding_expr) = bindings.get(entry.name.member()) {
                    // Use the binding expression (from the importer's scope, no prefixing needed
                    // for refs that belong to the importer — only dep-internal refs get prefixed).
                    // The declared type (the diagnostic anchor for an annotation
                    // mismatch) still belongs to the dependency, so the entry keeps
                    // dependency provenance below (#868).
                    entry.default_expr = Some(binding_expr.clone());
                    importer_owner.clone()
                } else if let Some(ref mut expr) = entry.default_expr {
                    // Keep default, but substitute indexes and prefix internal refs.
                    substitute_indexes(expr, index_bindings);
                    substitute_type_names_in_expr(expr, type_bindings);
                    prefix_expr_refs(expr, prefix, dep_names, dep_scoped_names);
                    merge_resolution_owner(entry.default_resolution_owner)
                } else {
                    // Required param without binding — stays None, caught later in exec_plan
                    merge_resolution_owner(entry.default_resolution_owner)
                };
            substitute_type_expr_indexes(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            self.params.push(UnfrozenParamEntry {
                name: prefixed.clone(),
                type_ann: entry.type_ann,
                type_resolution_owner: merge_resolution_owner(entry.type_resolution_owner),
                default_expr: entry.default_expr,
                default_resolution_owner,
                span: entry.span,
                src: entry.src.or_dependency(dep_src),
            });
            self.source_order.push((prefixed, DeclCategory::Param));
        }

        // Merge nodes
        for mut entry in dep.nodes {
            substitute_indexes(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names, dep_scoped_names);
            substitute_type_expr_indexes(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            let prefixed = entry.name.within_scope(prefix);
            self.nodes.push(UnfrozenNodeEntry {
                name: prefixed.clone(),
                type_ann: entry.type_ann,
                type_resolution_owner: merge_resolution_owner(entry.type_resolution_owner),
                expr: entry.expr,
                body_resolution_owner: merge_resolution_owner(entry.body_resolution_owner),
                span: entry.span,
                src: entry.src.or_dependency(dep_src),
            });
            self.source_order.push((prefixed, DeclCategory::Node));
        }

        // Merge asserts
        for mut entry in dep.asserts {
            match &mut entry.body {
                crate::desugar::desugared_ast::AssertBody::Expr(e) => {
                    substitute_indexes(e, index_bindings);
                    substitute_type_names_in_expr(e, type_bindings);
                    prefix_expr_refs(e, prefix, dep_names, dep_scoped_names);
                }
                crate::desugar::desugared_ast::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    ..
                } => {
                    substitute_indexes(actual, index_bindings);
                    substitute_type_names_in_expr(actual, type_bindings);
                    prefix_expr_refs(actual, prefix, dep_names, dep_scoped_names);
                    substitute_indexes(expected, index_bindings);
                    substitute_type_names_in_expr(expected, type_bindings);
                    prefix_expr_refs(expected, prefix, dep_names, dep_scoped_names);
                    substitute_indexes(tolerance, index_bindings);
                    substitute_type_names_in_expr(tolerance, type_bindings);
                    prefix_expr_refs(tolerance, prefix, dep_names, dep_scoped_names);
                }
            }
            let prefixed = entry.name.within_scope(prefix);
            self.asserts.push(UnfrozenAssertEntry {
                name: prefixed.clone(),
                body: entry.body,
                body_resolution_owner: merge_resolution_owner(entry.body_resolution_owner),
                span: entry.span,
                src: entry.src.or_dependency(dep_src),
            });
            self.assert_names.insert(prefixed.clone());
            self.source_order.push((prefixed, DeclCategory::Assert));
        }

        // Merge only the plots requested by the include's brace list (#847):
        // display is a consumer-side opt-in, so unrequested dep plots do not
        // travel with the instance. A requested plot enters the root
        // namespace under its local alias, evaluating against this instance's
        // bindings; `#[hidden]` on the include item keeps it composition-only.
        for mut entry in dep.plots {
            let Some(requested) = requested_plots.get(entry.name.member()) else {
                continue;
            };
            for encoding in &mut entry.decl.encodings {
                substitute_indexes(&mut encoding.value, index_bindings);
                substitute_type_names_in_expr(&mut encoding.value, type_bindings);
                prefix_expr_refs(&mut encoding.value, prefix, dep_names, dep_scoped_names);
            }
            for prop in &mut entry.decl.mark.properties {
                substitute_indexes(&mut prop.value, index_bindings);
                substitute_type_names_in_expr(&mut prop.value, type_bindings);
                prefix_expr_refs(&mut prop.value, prefix, dep_names, dep_scoped_names);
            }
            for prop in &mut entry.decl.properties {
                substitute_indexes(&mut prop.value, index_bindings);
                substitute_type_names_in_expr(&mut prop.value, type_bindings);
                prefix_expr_refs(&mut prop.value, prefix, dep_names, dep_scoped_names);
            }
            let local = ScopedName::local(requested.alias.clone());
            self.plots.push(UnfrozenPlotEntry {
                name: local.clone(),
                decl: entry.decl,
                body_resolution_owner: merge_resolution_owner(entry.body_resolution_owner),
                span: entry.span,
                displayed: !requested.hidden,
            });
            self.source_order.push((local, DeclCategory::Plot));
        }

        // Dep figures and layers do not merge: they cannot be requested by a
        // brace list, and display is controlled by the consumer (#847).

        // Merge assumes_map and expected_fail
        for (assert_name, assumers) in dep.assumes_map {
            let prefixed_assert = assert_name.within_scope(prefix);
            let prefixed_assumers: Vec<ScopedName> = assumers
                .iter()
                .map(|assumer| assumer.within_scope(prefix))
                .collect();
            self.assumes_map
                .entry(prefixed_assert)
                .or_default()
                .extend(prefixed_assumers);
        }
        for (assert_name, ef) in dep.expected_fail {
            let prefixed = assert_name.within_scope(prefix);

            // If the expected_fail references overridden indexes, filter or drop.
            if index_bindings.is_empty() {
                self.expected_fail.insert(prefixed, ef);
            } else {
                match ef {
                    ExpectedFail::All => {
                        self.expected_fail.insert(prefixed, ExpectedFail::All);
                    }
                    ExpectedFail::Variants(keys) => {
                        let filtered: Vec<_> = keys
                            .into_iter()
                            .filter(|key| {
                                // Drop keys that reference any overridden index.
                                // `#N` finite-position segments never name an
                                // index, so they cannot reference an overridden one.
                                !key.iter().any(|part| {
                                    part.index_path().is_some_and(|index_path| {
                                        index_bindings.contains_key(&IndexName::from_atom(
                                            index_path.leaf().clone(),
                                        ))
                                    })
                                })
                            })
                            .collect();
                        if !filtered.is_empty() {
                            self.expected_fail
                                .insert(prefixed, ExpectedFail::Variants(filtered));
                        }
                        // If all keys were dropped, don't insert any expected_fail.
                    }
                }
            }
        }

        // Apply import-item expected_fail attributes (from the importing file).
        // Malformed args are surfaced as `ExpectedFailInvalidArg`, matching the
        // behavior for non-imported `#[expected_fail]` attributes.
        for (orig_name, attrs) in import_item_attributes {
            for attr in attrs {
                if attr
                    .name
                    .name
                    .parse::<crate::syntax::attribute::AttributeName>()
                    == Ok(crate::syntax::attribute::AttributeName::ExpectedFail)
                {
                    let prefixed_assert = ScopedName::local(orig_name.clone()).with_prefix(prefix);
                    let ef = crate::ir::resolve::names::parse_expected_fail_args(
                        &attr.args,
                        importer_src,
                    )?;
                    self.expected_fail.insert(prefixed_assert, ef);
                }
            }
        }

        // Hidden imports used by dependency expressions receive the same
        // lexical instance prefix as those expressions. Their canonical target,
        // declared type, and optional value remain one unchanged record.
        for (name, binding) in dep.imported_bindings {
            let lexical = prefix_dep(&name, prefix, dep_names, dep_scoped_names);
            if self
                .imported_bindings
                .insert(lexical.clone(), binding)
                .is_some()
            {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "duplicate imported lexical binding `{lexical}` while merging include instance `{prefix}`"
                    ),
                    src: importer_src.clone(),
                    span: Span::new(0, 0).into(),
                });
            }
        }
        Ok(())
    }
}

/// Visitor that detects V005 / A8 violations in a param default expression.
///
/// Emits [`GraphcalError::IncludeMustReconcileOverride`] on the first
/// occurrence of a variant literal `s.v` where `s` is in
/// `index_bindings`, or of a constructor / as-cast / generic type
/// argument whose type name is in `type_bindings`. The spans reported
/// point at the importer's include statement — the error blames the
/// importer for omitting the required re-binding.
struct OverrideReconciliationChecker<'a> {
    index_bindings: &'a HashMap<IndexName, types::IndexBindingTarget>,
    type_bindings: &'a HashMap<StructTypeName, StructTypeName>,
    orphan_decl: &'a str,
    importer_src: &'a NamedSource<Arc<String>>,
    include_span: Span,
}

impl OverrideReconciliationChecker<'_> {
    fn orphan_error(
        &self,
        overridden_kind: &str,
        overridden: &str,
        detail: String,
    ) -> GraphcalError {
        GraphcalError::IncludeMustReconcileOverride {
            overridden: overridden.to_string(),
            overridden_kind: overridden_kind.to_string(),
            orphan_decl: self.orphan_decl.to_string(),
            detail,
            src: self.importer_src.clone(),
            span: self.include_span.into(),
        }
    }

    fn check_type_expr(&self, type_expr: &TypeExpr) -> Result<(), GraphcalError> {
        use crate::desugar::desugared_ast::TypeExprKind;
        match &type_expr.kind {
            TypeExprKind::DimExpr(dim_expr) => {
                for item in &dim_expr.terms {
                    let name = &item.term.name.value;
                    if let Some(atom) = name.as_bare()
                        && self.type_bindings.contains_key(atom.as_str())
                    {
                        return Err(self.orphan_error(
                            "type",
                            atom.as_str(),
                            format!("type `{name}`"),
                        ));
                    }
                }
                Ok(())
            }
            TypeExprKind::TypeApplication { name, generic_args } => {
                if let Some(atom) = name.value.as_bare()
                    && self.type_bindings.contains_key(atom.as_str())
                {
                    return Err(self.orphan_error(
                        "type",
                        atom.as_str(),
                        format!("type `{}`", name.value),
                    ));
                }
                for arg in generic_args {
                    match arg {
                        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
                            self.check_type_expr(type_expr)?;
                        }
                        crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
                            for ident in ambiguous_generic_arg_idents(ambiguous) {
                                if self.type_bindings.contains_key(ident.name.as_str()) {
                                    return Err(self.orphan_error(
                                        "type",
                                        ident.name.as_str(),
                                        format!("generic argument `{ambiguous}`"),
                                    ));
                                }
                            }
                        }
                        crate::desugar::desugared_ast::GenericArg::Index(_)
                        | crate::desugar::desugared_ast::GenericArg::Nat(_) => {}
                    }
                }
                Ok(())
            }
            TypeExprKind::ComplexApplication { generic_args }
            | TypeExprKind::KeyApplication { generic_args } => {
                for arg in generic_args {
                    match arg {
                        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
                            self.check_type_expr(type_expr)?;
                        }
                        crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
                            for ident in ambiguous_generic_arg_idents(ambiguous) {
                                if self.type_bindings.contains_key(ident.name.as_str()) {
                                    return Err(self.orphan_error(
                                        "type",
                                        ident.name.as_str(),
                                        format!("generic argument `{ambiguous}`"),
                                    ));
                                }
                            }
                        }
                        crate::desugar::desugared_ast::GenericArg::Index(_)
                        | crate::desugar::desugared_ast::GenericArg::Nat(_) => {}
                    }
                }
                Ok(())
            }
            TypeExprKind::DatetimeApplication { type_args } => {
                for arg in type_args {
                    self.check_type_expr(arg)?;
                }
                Ok(())
            }
            TypeExprKind::Indexed { base, .. } => self.check_type_expr(base),
            TypeExprKind::Dimensionless
            | TypeExprKind::Bool
            | TypeExprKind::Int
            | TypeExprKind::Datetime => Ok(()),
        }
    }
}

impl ExprVisitor<crate::syntax::phase::Desugared> for OverrideReconciliationChecker<'_> {
    type Error = GraphcalError;

    fn visit_unresolved_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        let ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) = &expr.kind
        else {
            return Ok(());
        };
        // A two-segment path whose head names a rebound index is a variant
        // literal of that index.
        if let [head, variant] = path.segments()
            && self.index_bindings.contains_key(head.name.as_str())
        {
            return Err(self.orphan_error(
                "index",
                head.name.as_str(),
                format!("`{}.{}`", head.name, variant.name),
            ));
        }
        // A bare path naming a rebound type is a nullary constructor use.
        if let Some(ident) = path.as_bare()
            && self.type_bindings.contains_key(ident.name.as_str())
        {
            let n = ident.name.as_str();
            return Err(self.orphan_error("type", n, format!("constructor `{n}`")));
        }
        Ok(())
    }

    fn visit_single_child(&mut self, expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::IndexAccess { args, .. } = &expr.kind {
            for arg in args {
                if let crate::desugar::desugared_ast::IndexArg::Variant { index, variant } = arg
                    && self
                        .index_bindings
                        .contains_key(index.value.leaf().as_str())
                {
                    return Err(self.orphan_error(
                        "index",
                        index.value.leaf().as_str(),
                        format!("`{}.{}`", index.value, variant.value),
                    ));
                }
            }
        }
        self.visit_expr(inner)
    }

    fn visit_map_entries(
        &mut self,
        _expr: &Expr,
        entries: &[crate::desugar::desugared_ast::MapEntry],
    ) -> Result<(), Self::Error> {
        for entry in entries {
            let key = entry.keys.first();
            if let crate::syntax::ast::MapEntryIndex::Named(index_name) = &key.index.value
                && self.index_bindings.contains_key(index_name.leaf().as_str())
            {
                return Err(self.orphan_error(
                    "index",
                    index_name.leaf().as_str(),
                    format!("`{}.{}`", index_name, key.variant.value),
                ));
            }
            self.visit_expr(&entry.value)?;
        }
        Ok(())
    }

    fn visit_match(
        &mut self,
        _expr: &Expr,
        scrutinee: &Expr,
        arms: &[crate::desugar::desugared_ast::MatchArm],
    ) -> Result<(), Self::Error> {
        self.visit_expr(scrutinee)?;
        for arm in arms {
            match &arm.pattern {
                crate::desugar::desugared_ast::MatchPattern::IndexLabel {
                    index, variant, ..
                } if self
                    .index_bindings
                    .contains_key(index.value.leaf().as_str()) =>
                {
                    return Err(self.orphan_error(
                        "index",
                        index.value.leaf().as_str(),
                        format!("`{}.{}`", index.value, variant.value),
                    ));
                }
                crate::desugar::desugared_ast::MatchPattern::Path { path, .. } => {
                    if let [head, variant] = path.segments()
                        && self.index_bindings.contains_key(head.name.as_str())
                    {
                        return Err(self.orphan_error(
                            "index",
                            head.name.as_str(),
                            format!("`{}.{}`", head.name, variant.name),
                        ));
                    }
                }
                _ => {}
            }
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }

    fn visit_constructor_call(
        &mut self,
        expr: &Expr,
        fields: &[crate::desugar::desugared_ast::FieldInit],
    ) -> Result<(), Self::Error> {
        if let ExprKind::ConstructorCall {
            callee,
            generic_args,
            ..
        } = &expr.kind
        {
            if let Some(constructor) = callee.as_bare() {
                let n = constructor.name.as_str();
                if self.type_bindings.contains_key(n) {
                    return Err(self.orphan_error("type", n, format!("constructor `{n}(...)`")));
                }
            }
            for arg in generic_args {
                if let crate::desugar::desugared_ast::GenericArg::Type(ty) = arg {
                    self.check_type_expr(ty)?;
                }
            }
        }
        for f in fields {
            self.visit_expr(&f.value)?;
        }
        Ok(())
    }

    fn visit_fn_call(&mut self, expr: &Expr, args: &[Expr]) -> Result<(), Self::Error> {
        if let ExprKind::FnCall { generic_args, .. } = &expr.kind {
            for ga in generic_args {
                if let crate::desugar::desugared_ast::GenericArg::Type(ty) = ga {
                    self.check_type_expr(ty)?;
                }
            }
        }
        for arg in args {
            self.visit_expr(arg)?;
        }
        Ok(())
    }
}

/// Visitor that prefixes references to dependency declarations.
///
/// When a `@name` (or bare const `NAME`) refers to a name owned by the
/// dependency being merged, rewrite its typed [`ScopedName`] so the merged-IR
/// key matches the enclosing instance path. Existing qualifiers are preserved
/// for exact nested declarations and left untouched for external module
/// imports. No flat separator strings are constructed here.
struct RefPrefixer<'a> {
    prefix: &'a ModuleAliasName,
    dep_names: &'a HashSet<DeclName>,
    dep_scoped_names: &'a HashSet<ScopedName>,
}

impl RefPrefixer<'_> {
    fn rewrite(&self, scoped: &ScopedName) -> Option<ScopedName> {
        if self.dep_scoped_names.contains(scoped) {
            Some(scoped.within_scope(self.prefix))
        } else if !scoped.is_qualified() && self.dep_names.contains(scoped.member()) {
            Some(scoped.with_prefix(self.prefix))
        } else {
            None
        }
    }
}

impl ExprVisitorMut<crate::syntax::phase::Desugared> for RefPrefixer<'_> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &mut expr.kind
            && let Some(prefixed) = self.rewrite(&ident.value)
        {
            ident.value = prefixed;
        }
        Ok(())
    }

    fn visit_unresolved_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        // A bare reference path owned by the dependency becomes a qualified
        // path under the merge prefix, mirroring the prefixed entry name it
        // resolves to. Already-qualified paths belong to another namespace
        // (a transitive import inside the dep) and keep their qualifier.
        if let ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) =
            &mut expr.kind
            && let Some(ident) = path.as_bare()
            && self.dep_names.contains(ident.name.as_str())
        {
            let leaf = ident.clone();
            let prefix_segment = crate::syntax::ast::Ident {
                name: self.prefix.atom().clone(),
                span: leaf.span,
            };
            *path = crate::syntax::ast::IdentPath::new(crate::syntax::non_empty::NonEmpty::new(
                prefix_segment,
                vec![leaf],
            ));
        }
        Ok(())
    }

    // Function calls don't need rewriting: built-ins (`sqrt`, `sum`, …)
    // are unqualified and never appear in `dep_names`, and there are no
    // user-defined functions in graphcal. The default `visit_fn_call_mut`
    // (which recurses into args) is correct.
}

/// Rewrite `@`-references and const/fn references within an expression to use
/// prefixed names, but only for names that belong to the dependency.
///
/// For example, `GraphRef("dry_mass")` becomes `GraphRef("r.dry_mass")` when
/// `"dry_mass"` is in `dep_names` and `prefix` is `"r"`.
///
/// Built-in names and names from the importer's scope are left unchanged.
fn prefix_expr_refs(
    expr: &mut Expr,
    prefix: &ModuleAliasName,
    dep_names: &HashSet<DeclName>,
    dep_scoped_names: &HashSet<ScopedName>,
) {
    let mut prefixer = RefPrefixer {
        prefix,
        dep_names,
        dep_scoped_names,
    };
    let _ = prefixer.visit_expr_mut(expr);
}

/// Visitor that rewrites index references in expressions according to a
/// resolved include binding map.
///
/// Index positions such as `for i: Axis` can become either another declared
/// axis or a structural `Fin(N)` axis. Label-bearing positions can only be
/// renamed to another declared axis; a required index has no labels of its own,
/// so a valid generic body cannot require such a rewrite for a finite target.
struct IndexSubstituter<'a> {
    bindings: &'a HashMap<IndexName, types::IndexBindingTarget>,
}

impl ExprVisitorMut<crate::syntax::phase::Desugared> for IndexSubstituter<'_> {
    type Error = std::convert::Infallible;

    fn visit_expr_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::KeyForm { axis, .. } = &mut expr.kind {
            substitute_index_expr(axis, self.bindings);
        }
        self.dispatch_mut(expr)
    }

    fn visit_generic_args_mut(
        &mut self,
        args: &mut [crate::desugar::desugared_ast::GenericArg],
    ) -> Result<(), Self::Error> {
        for arg in args {
            substitute_generic_arg_indexes(arg, self.bindings);
            if let crate::desugar::desugared_ast::GenericArg::Type(type_expr) = arg {
                self.visit_type_expr_mut(type_expr)?;
            }
        }
        Ok(())
    }

    fn visit_unresolved_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        // A two-segment path whose head names a rebound index is a variant
        // literal of that index (`Phase.Burn`). Only a declared target can
        // preserve such a label-bearing reference.
        if let ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) =
            &mut expr.kind
            && let [head, _variant] = path.segments.as_mut_slice()
            && let Some(new) = self
                .bindings
                .get(head.name.as_str())
                .and_then(types::IndexBindingTarget::declared_name)
        {
            head.name = new.atom().clone();
        }
        Ok(())
    }

    fn visit_for_comp_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        use crate::desugar::desugared_ast::{ForBindingIndex, NatExpr};

        if let ExprKind::ForComp { bindings, body } = &mut expr.kind {
            for binding in bindings {
                let replacement = match &binding.index {
                    ForBindingIndex::Named(index) => self
                        .bindings
                        .get(index.value.leaf().as_str())
                        .map(|target| match target {
                            types::IndexBindingTarget::Declared(name) => ForBindingIndex::Named(
                                Spanned::new(name.clone().into(), index.span),
                            ),
                            types::IndexBindingTarget::Finite(finite) => ForBindingIndex::Finite {
                                cardinality: NatExpr::Literal(finite.size_u64(), index.span),
                                span: index.span,
                            },
                        }),
                    ForBindingIndex::Finite { .. } => None,
                };
                if let Some(replacement) = replacement {
                    binding.index = replacement;
                }
            }
            self.visit_expr_mut(body)?;
        }
        Ok(())
    }

    fn visit_unfold_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::Unfold {
            axis, init, body, ..
        } = &mut expr.kind
        {
            if let Some(new) = self
                .bindings
                .get(axis.value.leaf().as_str())
                .and_then(types::IndexBindingTarget::declared_name)
            {
                axis.value = new.clone().into();
            }
            self.visit_expr_mut(init)?;
            self.visit_expr_mut(body)?;
        }
        Ok(())
    }

    fn visit_index_access_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        use crate::desugar::desugared_ast::IndexArg;
        if let ExprKind::IndexAccess { expr: inner, args } = &mut expr.kind {
            for arg in args.iter_mut() {
                match arg {
                    IndexArg::Variant { index, .. } => {
                        if let Some(new) = self
                            .bindings
                            .get(index.value.leaf().as_str())
                            .and_then(types::IndexBindingTarget::declared_name)
                        {
                            index.value = new.clone().into();
                        }
                    }
                    IndexArg::Expr(e) => {
                        self.visit_expr_mut(e)?;
                    }
                    IndexArg::Var(_) => {}
                }
            }
            self.visit_expr_mut(inner)?;
        }
        Ok(())
    }

    fn visit_map_literal_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::MapLiteral { entries } = &mut expr.kind {
            for entry in entries.iter_mut() {
                for key in &mut entry.keys {
                    if let crate::syntax::ast::MapEntryIndex::Named(index_name) = &key.index.value
                        && let Some(new) = self
                            .bindings
                            .get(index_name.leaf().as_str())
                            .and_then(types::IndexBindingTarget::declared_name)
                    {
                        key.index.value =
                            crate::syntax::ast::MapEntryIndex::Named(new.clone().into());
                    }
                }
                self.visit_expr_mut(&mut entry.value)?;
            }
        }
        Ok(())
    }

    fn visit_match_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::Match { scrutinee, arms } = &mut expr.kind {
            self.visit_expr_mut(scrutinee)?;
            for arm in arms {
                match &mut arm.pattern {
                    crate::desugar::desugared_ast::MatchPattern::IndexLabel { index, .. } => {
                        if let Some(new) = self
                            .bindings
                            .get(index.value.leaf().as_str())
                            .and_then(types::IndexBindingTarget::declared_name)
                        {
                            index.value = new.clone().into();
                        }
                    }
                    // A two-segment path pattern whose head names a rebound
                    // index is an index-label pattern; rewrite the head.
                    crate::desugar::desugared_ast::MatchPattern::Path { path, .. } => {
                        if let [head, _variant] = path.segments.as_mut_slice()
                            && let Some(new) = self
                                .bindings
                                .get(head.name.as_str())
                                .and_then(types::IndexBindingTarget::declared_name)
                        {
                            head.name = new.atom().clone();
                        }
                    }
                    crate::desugar::desugared_ast::MatchPattern::Constructor { .. } => {}
                }
                self.visit_expr_mut(&mut arm.body)?;
            }
        }
        Ok(())
    }
}

/// Rewrite index names within an expression according to a binding map.
///
/// For example, if `bindings` maps `"Phase"` to `"MyPhase"`, then
/// `VariantLiteral { index: Phase, variant: A }` becomes
/// `VariantLiteral { index: MyPhase, variant: A }`.
///
/// This must be called **before** `prefix_expr_refs` so that index names are
/// correct before ref-prefixing adds the `prefix.` qualifier.
fn substitute_indexes(expr: &mut Expr, bindings: &HashMap<IndexName, types::IndexBindingTarget>) {
    if bindings.is_empty() {
        return;
    }
    let mut sub = IndexSubstituter { bindings };
    let _ = sub.visit_expr_mut(expr);
}

fn ambiguous_generic_arg_idents(
    arg: &crate::desugar::desugared_ast::AmbiguousGenericArg,
) -> Vec<&crate::syntax::ast::Ident> {
    fn collect<'a>(
        arg: &'a crate::desugar::desugared_ast::AmbiguousGenericArg,
        idents: &mut Vec<&'a crate::syntax::ast::Ident>,
    ) {
        match arg {
            crate::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
                idents.push(ident);
            }
            crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
                for operand in operands {
                    collect(operand, idents);
                }
            }
        }
    }

    let mut idents = Vec::new();
    collect(arg, &mut idents);
    idents
}

fn rewrite_ambiguous_generic_arg_names<K>(
    arg: &mut crate::desugar::desugared_ast::AmbiguousGenericArg,
    bindings: &HashMap<K, K>,
) where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    match arg {
        crate::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
            if let Some(new_name) = bindings.get(ident.name.as_str())
                && let Ok(atom) = NameAtom::parse(new_name.as_ref())
            {
                ident.name = atom;
            }
        }
        crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                rewrite_ambiguous_generic_arg_names(operand, bindings);
            }
        }
    }
}

fn rewrite_ambiguous_index_arg_declared_names(
    arg: &mut crate::desugar::desugared_ast::AmbiguousGenericArg,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    match arg {
        crate::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
            if let Some(name) = bindings
                .get(ident.name.as_str())
                .and_then(types::IndexBindingTarget::declared_name)
            {
                ident.name = name.atom().clone();
            }
        }
        crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                rewrite_ambiguous_index_arg_declared_names(operand, bindings);
            }
        }
    }
}

fn index_expr_for_binding_target(
    target: &types::IndexBindingTarget,
    span: Span,
) -> crate::desugar::desugared_ast::IndexExpr {
    use crate::desugar::desugared_ast::{IndexExpr, NatExpr};

    match target {
        types::IndexBindingTarget::Declared(name) => IndexExpr::Name(Spanned::new(
            crate::syntax::names::NamePath::expect_local(name.as_str()),
            span,
        )),
        types::IndexBindingTarget::Finite(finite) => IndexExpr::Finite {
            cardinality: NatExpr::Literal(finite.size_u64(), span),
            span,
        },
    }
}

fn substitute_generic_arg_indexes(
    arg: &mut crate::desugar::desugared_ast::GenericArg,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    use crate::desugar::desugared_ast::{AmbiguousGenericArg, GenericArg};

    let finite_replacement = match arg {
        GenericArg::Ambiguous(AmbiguousGenericArg::Name(ident)) => bindings
            .get(ident.name.as_str())
            .filter(|target| matches!(target, types::IndexBindingTarget::Finite(_)))
            .map(|target| GenericArg::Index(index_expr_for_binding_target(target, ident.span))),
        GenericArg::Type(_)
        | GenericArg::Index(_)
        | GenericArg::Nat(_)
        | GenericArg::Ambiguous(AmbiguousGenericArg::Mul(..)) => None,
    };
    if let Some(replacement) = finite_replacement {
        *arg = replacement;
        return;
    }

    match arg {
        GenericArg::Type(type_expr) => substitute_type_expr_indexes(type_expr, bindings),
        GenericArg::Index(index) => substitute_index_expr(index, bindings),
        GenericArg::Ambiguous(ambiguous) => {
            rewrite_ambiguous_index_arg_declared_names(ambiguous, bindings);
        }
        GenericArg::Nat(_) => {}
    }
}

fn substitute_generic_arg_nominal_names<K>(
    arg: &mut crate::desugar::desugared_ast::GenericArg,
    bindings: &HashMap<K, K>,
) where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    match arg {
        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
            substitute_type_expr_nominal_names(type_expr, bindings);
        }
        crate::desugar::desugared_ast::GenericArg::Index(_)
        | crate::desugar::desugared_ast::GenericArg::Nat(_) => {}
        crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
            rewrite_ambiguous_generic_arg_names(ambiguous, bindings);
        }
    }
}

fn substitute_index_expr(
    index: &mut crate::desugar::desugared_ast::IndexExpr,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    let replacement = match index {
        crate::desugar::desugared_ast::IndexExpr::Name(path) => path
            .value
            .as_bare()
            .and_then(|atom| bindings.get(atom.as_str()))
            .map(|target| index_expr_for_binding_target(target, path.span)),
        crate::desugar::desugared_ast::IndexExpr::Finite { .. }
        | crate::desugar::desugared_ast::IndexExpr::BareNat(_) => None,
    };
    if let Some(replacement) = replacement {
        *index = replacement;
    }
}

/// Rewrite index arguments within a type expression according to a binding map.
///
/// `TypeExpr` is not part of the `Expr` tree, so it needs a separate
/// substitution pass. This rewrites index identifiers in `Indexed` types to a
/// declared target or structural `Fin(N)` target and recurses into
/// `TypeApplication` arguments.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn substitute_type_expr_indexes(
    type_expr: &mut TypeExpr,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    use crate::desugar::desugared_ast::TypeExprKind;

    if bindings.is_empty() {
        return;
    }
    match &mut type_expr.kind {
        TypeExprKind::Indexed { base, indexes } => {
            for index in indexes {
                substitute_index_expr(index, bindings);
            }
            substitute_type_expr_indexes(base, bindings);
        }
        TypeExprKind::TypeApplication { generic_args, .. } => {
            for arg in generic_args {
                substitute_generic_arg_indexes(arg, bindings);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            for arg in generic_args {
                substitute_generic_arg_indexes(arg, bindings);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            for arg in type_args {
                substitute_type_expr_indexes(arg, bindings);
            }
        }
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => {}
    }
}

/// Rewrite bare dimension names within a dimension expression.
///
/// This is the shared syntactic substitution used by include-time dimension
/// bindings. Qualified references retain their producer-owned path; only a bare
/// bindable declaration name can be replaced by an importer-side binding.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn substitute_dim_expr_names<K>(dim_expr: &mut DimExpr, bindings: &HashMap<K, K>)
where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    for item in &mut dim_expr.terms {
        if let Some(atom) = item.term.name.value.as_bare()
            && let Some(new_name) = bindings.get(atom.as_str())
        {
            item.term.name.value = crate::syntax::names::NamePath::expect_local(new_name.as_ref());
        }
    }
}

/// Rewrite nominally-tied names (types or dimensions) within a type expression.
///
/// `TypeExpr` uses `DimExpr` to carry single-identifier type references (the
/// resolver disambiguates them into `StructType` / `Dim` later). Both type and
/// dimension bindings therefore need to walk `DimExpr` terms and rewrite their
/// names. `TypeApplication.name` is rewritten for type bindings (generic
/// parametric types like `Vec3<Length>`), which is harmless for dim bindings
/// because type and dim names can't collide (A6 nominal identity).
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn substitute_type_expr_nominal_names<K>(type_expr: &mut TypeExpr, bindings: &HashMap<K, K>)
where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    use crate::desugar::desugared_ast::TypeExprKind;

    if bindings.is_empty() {
        return;
    }
    match &mut type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => substitute_dim_expr_names(dim_expr, bindings),
        TypeExprKind::Indexed { base, .. } => {
            substitute_type_expr_nominal_names(base, bindings);
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            if let Some(atom) = name.value.as_bare()
                && let Some(new_name) = bindings.get(atom.as_str())
            {
                name.value = crate::syntax::names::NamePath::expect_local(new_name.as_ref());
            }
            for arg in generic_args {
                substitute_generic_arg_nominal_names(arg, bindings);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            for arg in generic_args {
                substitute_generic_arg_nominal_names(arg, bindings);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // The built-in `Datetime` name is fixed; only the type args can
            // carry user-bindable nominal names.
            for arg in type_args {
                substitute_type_expr_nominal_names(arg, bindings);
            }
        }
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
    }
}

/// Rewrite struct-type names within an expression according to a binding map.
///
/// Covers `ConstructorCall.constructor` and both call variants' `generic_args`.
/// Recurses through child expressions so nested constructor calls are also
/// rewritten.
#[expect(
    clippy::too_many_lines,
    reason = "single recursion covering every ExprKind variant"
)]
fn substitute_type_names_in_expr(
    expr: &mut Expr,
    bindings: &HashMap<StructTypeName, StructTypeName>,
) {
    use crate::desugar::desugared_ast::{GenericArg, IndexArg};

    if bindings.is_empty() {
        return;
    }
    match &mut expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::GraphRef(_) => {}

        // A bare reference path naming a rebound type is a nullary
        // constructor use; rewrite it to the importer's constructor name.
        ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
            if let Some(ident) = path.as_bare_mut()
                && let Some(new_name) = bindings.get(ident.name.as_str())
                && let Ok(parsed_name) = NameAtom::parse(new_name.as_ref())
            {
                ident.name = parsed_name;
            }
        }

        ExprKind::InlineDagRef { args, .. } => {
            for binding in args {
                substitute_type_names_in_expr(&mut binding.value, bindings);
            }
        }

        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } => {
            if let Some(constructor) = callee.as_bare_mut()
                && let Some(new_name) = bindings.get(constructor.name.as_str())
                && let Ok(parsed_name) = NameAtom::parse(new_name.as_ref())
            {
                constructor.name = parsed_name;
            }
            for arg in generic_args.iter_mut() {
                if let GenericArg::Type(ty) = arg {
                    substitute_type_expr_nominal_names(ty, bindings);
                }
            }
            for field in fields {
                substitute_type_names_in_expr(&mut field.value, bindings);
            }
        }

        ExprKind::FnCall {
            generic_args, args, ..
        } => {
            for ga in generic_args.iter_mut() {
                if let GenericArg::Type(ty) = ga {
                    substitute_type_expr_nominal_names(ty, bindings);
                }
            }
            for arg in args {
                substitute_type_names_in_expr(arg, bindings);
            }
        }

        ExprKind::BinOp { lhs, rhs, .. } => {
            substitute_type_names_in_expr(lhs, bindings);
            substitute_type_names_in_expr(rhs, bindings);
        }
        ExprKind::UnaryOp { operand, .. } => {
            substitute_type_names_in_expr(operand, bindings);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            substitute_type_names_in_expr(condition, bindings);
            substitute_type_names_in_expr(then_branch, bindings);
            substitute_type_names_in_expr(else_branch, bindings);
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::FieldAccess { expr: inner, .. } => {
            substitute_type_names_in_expr(inner, bindings);
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            substitute_type_names_in_expr(inner, bindings);
            for arg in args {
                if let IndexArg::Expr(e) = arg {
                    substitute_type_names_in_expr(e, bindings);
                }
            }
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                substitute_type_names_in_expr(&mut entry.value, bindings);
            }
        }
        ExprKind::ForComp { body, .. } => {
            substitute_type_names_in_expr(body, bindings);
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            substitute_type_names_in_expr(source, bindings);
            substitute_type_names_in_expr(init, bindings);
            substitute_type_names_in_expr(body, bindings);
        }
        ExprKind::Unfold { init, body, .. } => {
            substitute_type_names_in_expr(init, bindings);
            substitute_type_names_in_expr(body, bindings);
        }
        ExprKind::KeyForm { arg, .. } => {
            substitute_type_names_in_expr(arg, bindings);
        }
        ExprKind::Match { scrutinee, arms } => {
            substitute_type_names_in_expr(scrutinee, bindings);
            for arm in arms {
                substitute_type_names_in_expr(&mut arm.body, bindings);
            }
        }
        // `Sugar` payload is `Infallible` post-desugar — statically
        // unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) => match *s {},
    }
}

/// Register dimensions, units, indexes, and struct types from a file's declarations
/// into the registry.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
fn register_file_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
) -> Result<(), GraphcalError> {
    register_declarations_impl(file, registry, src, None, dag_id)
}

/// Categorized declarations selected from a dependency's registry.
///
/// Each source import marker maps to exactly one typed set. Keeping the
/// categories separate prevents a declaration from drifting between semantic
/// namespaces without changing the importing source.
#[derive(Debug, Default, Clone)]
pub struct SelectedDeclarations {
    /// Bare items are unresolved between declaration and constructor namespaces.
    terms: HashSet<crate::syntax::names::NameAtom>,
    dimensions: HashSet<crate::syntax::dimension::DimName>,
    units: HashSet<crate::syntax::dimension::UnitName>,
    indexes: HashSet<crate::syntax::index_name::IndexName>,
    types: HashSet<crate::syntax::type_name::StructTypeName>,
}

impl SelectedDeclarations {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.dimensions.is_empty()
            && self.units.is_empty()
            && self.indexes.is_empty()
            && self.types.is_empty()
    }

    /// Record one selective import in its declared namespace.
    pub fn insert(
        &mut self,
        namespace: crate::syntax::ast::ImportItemNamespace,
        name: crate::syntax::names::NameAtom,
    ) {
        match namespace {
            crate::syntax::ast::ImportItemNamespace::Term => {
                self.terms.insert(name);
            }
            crate::syntax::ast::ImportItemNamespace::Type => {
                self.types
                    .insert(crate::syntax::type_name::StructTypeName::from_atom(name));
            }
            crate::syntax::ast::ImportItemNamespace::Dimension => {
                self.dimensions
                    .insert(crate::syntax::dimension::DimName::from_atom(name));
            }
            crate::syntax::ast::ImportItemNamespace::Unit => {
                self.units
                    .insert(crate::syntax::dimension::UnitName::from_atom(name));
            }
            crate::syntax::ast::ImportItemNamespace::Index => {
                self.indexes
                    .insert(crate::syntax::index_name::IndexName::from_atom(name));
            }
        }
    }

    /// Dimension names selected with the explicit `dim` marker.
    pub fn dimensions(&self) -> impl Iterator<Item = &crate::syntax::dimension::DimName> {
        self.dimensions.iter()
    }

    /// Clone this selection without declarations that are closed semantic values.
    ///
    /// Project lowering imports selected dimensions and units from the
    /// dependency's already-resolved semantic registry, while the remaining
    /// declaration categories still use source registration.
    #[must_use]
    pub fn without_resolved_dimensions_and_units(&self) -> Self {
        let mut selected = self.clone();
        selected.dimensions.clear();
        selected.units.clear();
        selected
    }

    /// Unit names selected with the explicit `unit` marker.
    pub fn units(&self) -> impl Iterator<Item = &crate::syntax::dimension::UnitName> {
        self.units.iter()
    }
}

/// Register only categorized declarations selected from a file.
///
/// This is the selective counterpart to `register_file_declarations`: each
/// declaration kind is filtered through the import category written by the
/// consumer.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
pub fn register_selected_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    names: &SelectedDeclarations,
    dag_id: &crate::dag_id::DagId,
) -> Result<(), GraphcalError> {
    register_declarations_impl(file, registry, src, Some(names), dag_id)
}

/// Shared implementation for registering type-system declarations.
///
/// Registration is split into phases to allow forward references between
/// declarations of the same kind (e.g., a derived dimension referencing another
/// derived dimension declared later in the file). The phases are:
///
/// 1. Base dimensions, types, union types, named/required-named indexes
/// 2. Derived dimensions (topologically sorted by inter-dependency)
/// 3. Required-coordinate indexes (depend only on dimensions)
/// 4. Units (topologically sorted by inter-dependency)
/// 5. Coordinate indexes (depend on dimensions and units)
///
/// When `filter` is `None`, all declarations are registered.
/// When `filter` is `Some(names)`, every declaration kind is filtered through
/// its own typed import category.
fn register_declarations_impl(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    filter: Option<&SelectedDeclarations>,
    dag_id: &crate::dag_id::DagId,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::{DimDecl, IndexDecl, UnitDecl};

    let should_register_term = |name: &str| filter.is_none_or(|names| names.terms.contains(name));
    let should_register_dimension =
        |name: &str| filter.is_none_or(|names| names.dimensions.contains(name));
    let should_register_unit = |name: &str| filter.is_none_or(|names| names.units.contains(name));
    let should_register_index =
        |name: &str| filter.is_none_or(|names| names.indexes.contains(name));
    let should_register_type = |name: &str| filter.is_none_or(|names| names.types.contains(name));

    // Collect declarations by kind for phased registration.
    let mut derived_dims: Vec<&DimDecl> = Vec::new();
    let mut units: Vec<&UnitDecl> = Vec::new();
    let mut required_coordinate_indexes: Vec<(&IndexDecl, Span)> = Vec::new();
    let mut coordinate_indexes: Vec<(&IndexDecl, Span)> = Vec::new();

    // Phase 1: Register base dimensions, types, union types, named/required-named indexes.
    // Also collect derived dims, units, and dependent indexes for later phases.
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::BaseDimension(d) if should_register_dimension(d.name.value.as_str()) => {
                register_base_dimension_decl(d, registry, dag_id);
            }
            DeclKind::Dimension(d) if should_register_dimension(d.name.value.as_str()) => {
                if d.definition.is_some() {
                    derived_dims.push(d);
                } else {
                    // Required dim (`dim D;`) — no body. Compile as an opaque
                    // base dimension so the library checks out in isolation;
                    // substitution via include-time dim bindings happens in a
                    // later phase (see visibility/bindability axioms plan §C2).
                    register_required_dimension_decl(d, registry, dag_id);
                }
            }
            DeclKind::Unit(u) if should_register_unit(u.name.value.as_str()) => {
                units.push(u);
            }
            DeclKind::Index(idx) if should_register_index(idx.name.value.as_str()) => {
                match &idx.kind {
                    IndexDeclKind::RequiredCoordinate { .. } => {
                        required_coordinate_indexes.push((idx, decl.span));
                    }
                    IndexDeclKind::Range { .. } | IndexDeclKind::Linspace { .. } => {
                        coordinate_indexes.push((idx, decl.span));
                    }
                    IndexDeclKind::Named { .. } | IndexDeclKind::RequiredNamed => {
                        register_index_decl(idx, registry, src, decl.span)?;
                    }
                }
            }
            DeclKind::Type(t) if should_register_type(t.name.value.as_str()) => {
                register_type_decl(t, registry, src)?;
            }
            DeclKind::Dag(d) if should_register_term(d.name.value.as_str()) => {
                registry.register_dag(d.name.value.clone(), d.clone());
            }
            _ => {}
        }
    }

    // Phase 2: Topologically sort and register derived dimensions.
    if !derived_dims.is_empty() {
        let sorted = topo_sort_derived_dims(&derived_dims, src)?;
        for d in sorted {
            register_dimension_decl(d, registry, src)?;
        }
    }

    // Phase 3: Register required-coordinate indexes (depend only on dimensions).
    for (idx, span) in &required_coordinate_indexes {
        register_index_decl(idx, registry, src, *span)?;
    }

    // Phase 4: Topologically sort and register units.
    if !units.is_empty() {
        let sorted = topo_sort_units(&units, src)?;
        for u in sorted {
            register_unit_decl(u, registry, src)?;
        }
    }

    // Phase 5: Register coordinate indexes (depend on dimensions and units).
    for (idx, span) in &coordinate_indexes {
        register_index_decl(idx, registry, src, *span)?;
    }

    register_structural_finite_indexes(file, registry, src)?;

    Ok(())
}

/// Register concrete Fin definitions under typed structural identities appearing
/// in type positions or finite comprehensions.
fn register_structural_finite_indexes(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Param(d) => {
                collect_finite_indexes_from_type_expr(&d.type_ann, registry, src)?;
                if let Some(ref value) = d.value {
                    collect_finite_indexes_from_expr(value, registry, src)?;
                }
            }
            DeclKind::Node(d) | DeclKind::ConstNode(d) => {
                collect_finite_indexes_from_type_expr(&d.type_ann, registry, src)?;
                collect_finite_indexes_from_expr(&d.value, registry, src)?;
            }
            DeclKind::Type(d) => {
                for default in d
                    .generic_params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                {
                    collect_finite_indexes_from_generic_arg(default, registry, src)?;
                }
                if let crate::desugar::desugared_ast::TypeDeclBody::Constructors(members) = &d.body
                {
                    for field in members
                        .iter()
                        .filter_map(|member| member.payload.as_ref())
                        .flatten()
                    {
                        collect_finite_indexes_from_type_expr(&field.type_ann, registry, src)?;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Topologically sort derived dimension declarations by their inter-dependencies.
///
/// Dependencies on dimensions already in the registry (e.g., from preludes or imports)
/// are considered satisfied and do not create graph edges. Only dependencies between
/// the file-local derived dimensions are edges.
fn topo_sort_derived_dims<'a>(
    dims: &[&'a crate::desugar::desugared_ast::DimDecl],
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<&'a crate::desugar::desugared_ast::DimDecl>, GraphcalError> {
    let mut graph = DiGraph::<&str, ()>::new();
    let mut name_to_idx: HashMap<&str, petgraph::graph::NodeIndex> = HashMap::new();
    let mut idx_to_pos: HashMap<petgraph::graph::NodeIndex, usize> = HashMap::new();

    // Add a node for each derived dimension.
    for (pos, d) in dims.iter().enumerate() {
        let name = d.name.value.as_str();
        let idx = graph.add_node(name);
        name_to_idx.insert(name, idx);
        idx_to_pos.insert(idx, pos);
    }

    // Add edges: if dim A references dim B (and B is a *different* file-local dim), add A → B.
    // Self-references (e.g., `dimension Mass = Mass;` aliasing a prelude dimension) are
    // excluded — they resolve against the existing registry during registration.
    for d in dims {
        let self_name = d.name.value.as_str();
        let from = name_to_idx[self_name];
        // Only derived dims reach this sort; required dims are routed
        // directly to the base-dim registry in Phase 1.
        let Some(definition) = &d.definition else {
            continue;
        };
        for item in &definition.terms {
            let Some(dep_name) = item
                .term
                .name
                .value
                .as_bare()
                .map(super::super::syntax::names::NameAtom::as_str)
            else {
                continue;
            };
            if dep_name != self_name
                && let Some(&to) = name_to_idx.get(dep_name)
            {
                graph.add_edge(from, to, ());
            }
        }
    }

    // Topologically sort (reversed, since edges point from dependent → dependency).
    let sorted_indices = toposort(&graph, None).map_err(|cycle| {
        let cycle_name = graph[cycle.node_id()];
        let pos = idx_to_pos[&cycle.node_id()];
        GraphcalError::CyclicDimension {
            name: DimName::expect_valid(cycle_name),
            src: src.clone(),
            span: dims[pos].name.span.into(),
        }
    })?;

    // toposort returns dependencies-last order; reverse for dependencies-first.
    Ok(sorted_indices
        .into_iter()
        .rev()
        .map(|idx| dims[idx_to_pos[&idx]])
        .collect())
}

/// Topologically sort unit declarations by their inter-dependencies.
///
/// A unit depends on other units through its `definition.unit_expr` (e.g., `const unit km: Length = 1000 m;`
/// depends on `m`). Dependencies on units already in the registry are satisfied and
/// do not create graph edges.
fn topo_sort_units<'a>(
    units: &[&'a crate::desugar::desugared_ast::UnitDecl],
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<&'a crate::desugar::desugared_ast::UnitDecl>, GraphcalError> {
    let mut graph = DiGraph::<&str, ()>::new();
    let mut name_to_idx: HashMap<&str, petgraph::graph::NodeIndex> = HashMap::new();
    let mut idx_to_pos: HashMap<petgraph::graph::NodeIndex, usize> = HashMap::new();

    // Add a node for each unit.
    for (pos, u) in units.iter().enumerate() {
        let name = u.name.value.as_str();
        let idx = graph.add_node(name);
        name_to_idx.insert(name, idx);
        idx_to_pos.insert(idx, pos);
    }

    // Add edges: if unit A's definition references unit B (a *different* file-local unit), add A → B.
    for u in units {
        let self_name = u.name.value.as_str();
        let from = name_to_idx[self_name];
        if let Some(def) = &u.definition {
            for item in &def.unit_expr.terms {
                // Module-qualified references can never name a file-local
                // unit, so only bare references create graph edges.
                if item.name.value.is_qualified() {
                    continue;
                }
                let dep_name = item.name.value.name().as_str();
                if dep_name != self_name
                    && let Some(&to) = name_to_idx.get(dep_name)
                {
                    graph.add_edge(from, to, ());
                }
            }
        }
    }

    let sorted_indices = toposort(&graph, None).map_err(|cycle| {
        let pos = idx_to_pos[&cycle.node_id()];
        GraphcalError::CyclicUnit {
            name: units[pos].name.value.clone(),
            src: src.clone(),
            span: units[pos].name.span.into(),
        }
    })?;

    // toposort returns dependencies-last order; reverse for dependencies-first.
    Ok(sorted_indices
        .into_iter()
        .rev()
        .map(|idx| units[idx_to_pos[&idx]])
        .collect())
}

fn register_base_dimension_decl(
    d: &crate::desugar::desugared_ast::BaseDimDecl,
    registry: &mut RegistryBuilder,
    dag_id: &crate::dag_id::DagId,
) {
    let dim_id = crate::dimension::BaseDimId::UserDefined(
        crate::syntax::dimension::ResolvedDimName::from_def(dag_id.clone(), d.name.value.clone()),
    );
    registry.register_base_dimension(d.name.value.clone(), dim_id);
}

fn register_dimension_decl(
    d: &crate::desugar::desugared_ast::DimDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    // Only derived dims reach this function; required dims (`dim D;`)
    // are routed to `register_required_dimension_decl` in Phase 1 and
    // never end up in the topo-sorted derived-dim list.
    let Some(definition) = d.definition.as_ref() else {
        return Ok(());
    };
    let dim = registry
        .resolve_dim_expr_detailed(definition)
        .map_err(|err| dimension_resolve_error(err, src, definition.span))?;
    registry.register_dimension(d.name.value.clone(), dim);
    Ok(())
}

/// Register a required dim (`dim D;`) as an opaque base dimension.
///
/// The library treats the required dim like a base SI dimension while
/// compiling standalone. Later include-time substitution rewires
/// references through the importer's dim bindings.
fn register_required_dimension_decl(
    d: &crate::desugar::desugared_ast::DimDecl,
    registry: &mut RegistryBuilder,
    dag_id: &crate::dag_id::DagId,
) {
    let dim_id = crate::dimension::BaseDimId::UserDefined(
        crate::syntax::dimension::ResolvedDimName::from_def(dag_id.clone(), d.name.value.clone()),
    );
    registry.register_base_dimension(d.name.value.clone(), dim_id);
}

fn registry_build_error(
    err: &types::RegistryBuildError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::InternalError {
        message: format!("registry build failed: {err}"),
        src: src.clone(),
        span: Span::new(0, 0).into(),
    }
}

fn dimension_resolve_error(
    err: DimensionResolveError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    match err {
        DimensionResolveError::UnknownDimension { name } => GraphcalError::UnknownDimension {
            name,
            src: src.clone(),
            span: span.into(),
        },
        DimensionResolveError::Overflow(_) => GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: span.into(),
        },
    }
}

fn eval_error(
    message: impl Into<String>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: message.into(),
        src: src.clone(),
        span: span.into(),
    }
}

fn validate_positive_finite_scale(
    value: f64,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<PositiveFiniteScale, GraphcalError> {
    PositiveFiniteScale::new(value).map_err(|err| {
        let reason = match err {
            PositiveFiniteScaleError::NonFinite => "must be finite",
            PositiveFiniteScaleError::NonPositive => "must be greater than zero",
        };
        eval_error(format!("{context} {reason}, got {value}"), src, span)
    })
}

fn multiply_positive_scales(
    lhs: PositiveFiniteScale,
    rhs: PositiveFiniteScale,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<PositiveFiniteScale, GraphcalError> {
    validate_positive_finite_scale(lhs.get() * rhs.get(), context, src, span)
}

fn register_unit_decl(
    u: &crate::desugar::desugared_ast::UnitDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let dim = registry
        .resolve_dim_expr_detailed(&u.dim_type)
        .map_err(|err| dimension_resolve_error(err, src, u.dim_type.span))?;
    if u.definition.is_some() && registry.is_affine_prone(&dim) {
        return Err(GraphcalError::AffineProneUnitDefinition {
            dim: registry.format_dimension(&dim),
            src: src.clone(),
            span: u.name.span.into(),
        });
    }
    let scale = if let Some(def) = &u.definition {
        if u.constness.is_const() {
            if let Some(graph_ref) = first_graph_ref(&def.scale_expr) {
                return Err(GraphcalError::GraphRefInConstUnit {
                    name: graph_ref.value,
                    src: src.clone(),
                    span: graph_ref.span.into(),
                });
            }
            if let Some(unit_name) = first_non_const_unit_ref(registry, &def.unit_expr) {
                return Err(GraphcalError::NonConstUnitInConst {
                    name: unit_name.value.clone(),
                    src: src.clone(),
                    span: unit_name.span.into(),
                });
            }
        }
        let resolved_definition = resolve_unit_definition(registry, &def.unit_expr, src)?;
        if resolved_definition.dimension != dim {
            return Err(GraphcalError::UnitDefinitionDimensionMismatch {
                name: u.name.value.clone(),
                declared: registry.format_dimension(&dim),
                definition: registry.format_dimension(&resolved_definition.dimension),
                src: src.clone(),
                span: def.unit_expr.span.into(),
            });
        }
        if contains_graph_ref(&def.scale_expr) {
            // Dynamic unit: scale depends on runtime values (e.g., `(@rate) USD`).
            UnitScale::Dynamic {
                scale_expr: def.scale_expr.clone(),
                base_unit_scale: resolved_definition.base_scale,
            }
        } else {
            // Static scale value. A plain `unit` with no `@` still remains a
            // runtime unit for const-context policy; `const unit` is the
            // surface marker that makes it available to `const node`.
            let scale_expr = validate_positive_finite_scale(
                eval_scale_expr(&def.scale_expr, src)?,
                "unit scale expression",
                src,
                def.scale_expr.span,
            )?;
            let scale = multiply_positive_scales(
                scale_expr,
                resolved_definition.base_scale,
                "unit scale",
                src,
                def.span,
            )?;
            UnitScale::Static(scale)
        }
    } else {
        UnitScale::Static(validate_positive_finite_scale(
            1.0,
            "base unit scale",
            src,
            u.name.span,
        )?)
    };
    // If this is a base unit (scale=1, no definition) for a single
    // base dimension, record the unit name as the SI symbol for
    // that dimension. This handles user-defined dimensions like
    // `base unit bit: Information;` → symbol "bit" for Information.
    if u.definition.is_none() {
        // Check if this dimension is a single base dimension
        let mut iter = dim.iter();
        if let Some((id, &exp)) = iter.next()
            && iter.next().is_none()
            && exp == Rational::ONE
        {
            registry.set_base_dim_symbol(id.clone(), u.name.value.to_string());
        }
    }
    registry.register_unit_with_scale(u.name.value.clone(), dim, scale, u.constness);
    Ok(())
}

fn first_graph_ref(expr: &Expr) -> Option<Spanned<ScopedName>> {
    struct FirstGraphRef(Option<Spanned<ScopedName>>);

    impl ExprVisitor<crate::syntax::phase::Desugared> for FirstGraphRef {
        type Error = std::convert::Infallible;

        fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
            if self.0.is_none()
                && let ExprKind::GraphRef(name) = &expr.kind
            {
                self.0 = Some(name.clone());
            }
            Ok(())
        }
    }

    let mut visitor = FirstGraphRef(None);
    let _ = visitor.visit_expr(expr);
    visitor.0
}

fn first_non_const_unit_ref<'a>(
    registry: &RegistryBuilder,
    unit_expr: &'a crate::desugar::desugared_ast::UnitExpr,
) -> Option<&'a Spanned<crate::syntax::dimension::UnitRef>> {
    unit_expr.terms.iter().find_map(|term| {
        registry
            .get_unit(&term.name.value)
            .is_some_and(|info| !info.constness.is_const())
            .then_some(&term.name)
    })
}

/// Dimension and static scale proved for a unit definition's RHS unit expression.
///
/// Keeping these facts together prevents registration from pairing a scale from
/// one physical dimension with an unrelated declared dimension.
struct ResolvedUnitDefinition {
    dimension: Dimension,
    base_scale: PositiveFiniteScale,
}

/// Resolve the unit-expression half of a unit definition before mutating the registry.
///
/// For `unit EUR: Money = (@rate) USD;`, this proves both that `USD` measures
/// `Money` and that its static base scale is 1.0. The base unit itself must be
/// static; only the scalar expression may depend on runtime values.
fn resolve_unit_definition(
    registry: &RegistryBuilder,
    unit_expr: &crate::desugar::desugared_ast::UnitExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedUnitDefinition, GraphcalError> {
    let (dimension, base_scale) = registry
        .resolve_unit_expr(unit_expr)
        .map_err(|err| unit_resolve_to_graphcal(err, src, unit_expr.span))?;
    let base_scale =
        validate_positive_finite_scale(base_scale, "base unit scale", src, unit_expr.span)?;
    Ok(ResolvedUnitDefinition {
        dimension,
        base_scale,
    })
}

/// Convert a typed unit-resolution failure into a spanned diagnostic.
fn unit_resolve_to_graphcal(
    err: crate::registry::types::UnitResolveError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    use crate::registry::types::UnitResolveError;
    match err {
        UnitResolveError::UnknownUnit(name) => GraphcalError::UnknownUnit {
            name,
            src: src.clone(),
            span: span.into(),
        },
        UnitResolveError::DynamicScale(name) => GraphcalError::EvalError {
            message: format!("unit `{name}` has a dynamic scale and cannot be used here"),
            src: src.clone(),
            span: span.into(),
        },
        UnitResolveError::InvalidScale { value, reason } => {
            let reason = match reason {
                PositiveFiniteScaleError::NonFinite => "must be finite",
                PositiveFiniteScaleError::NonPositive => "must be greater than zero",
            };
            GraphcalError::EvalError {
                message: format!("compound unit scale {reason}, got {value}"),
                src: src.clone(),
                span: span.into(),
            }
        }
        UnitResolveError::Overflow(_) => GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: span.into(),
        },
    }
}

/// Check if an expression contains any `@`-references (graph refs).
fn contains_graph_ref(expr: &Expr) -> bool {
    crate::ir::resolve::contains_graph_ref(expr)
}

fn concrete_nat_value(
    expr: &crate::desugar::desugared_ast::NatExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<u64>, GraphcalError> {
    use crate::desugar::desugared_ast::NatExpr;
    match expr {
        NatExpr::Literal(value, _) => Ok(Some(*value)),
        NatExpr::Var(_) => Ok(None),
        NatExpr::Add(operands, span) => {
            operands.iter().try_fold(Some(0_u64), |sum, operand| {
                match (sum, concrete_nat_value(operand, src)?) {
                    (Some(sum), Some(value)) => sum
                        .checked_add(value)
                        .map(Some)
                        .ok_or_else(|| eval_error("Fin cardinality addition overflow", src, *span)),
                    _ => Ok(None),
                }
            })
        }
        NatExpr::Mul(operands, span) => {
            operands.iter().try_fold(Some(1_u64), |product, operand| {
                match (product, concrete_nat_value(operand, src)?) {
                    (Some(product), Some(value)) => {
                        product.checked_mul(value).map(Some).ok_or_else(|| {
                            eval_error("Fin cardinality multiplication overflow", src, *span)
                        })
                    }
                    _ => Ok(None),
                }
            })
        }
    }
}

fn ensure_concrete_finite_index(
    cardinality: u64,
    span: Span,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let index = types::FiniteIndex::try_from_u64(cardinality)
        .map_err(|error| eval_error(error.to_string(), src, span))?;
    registry.ensure_finite_index(index.cardinality());
    Ok(())
}

fn collect_finite_index_expr(
    index: &crate::desugar::desugared_ast::IndexExpr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match index {
        crate::desugar::desugared_ast::IndexExpr::Finite { cardinality, span } => {
            if let Some(value) = concrete_nat_value(cardinality, src)? {
                ensure_concrete_finite_index(value, *span, registry, src)?;
            }
        }
        crate::desugar::desugared_ast::IndexExpr::Name(_)
        | crate::desugar::desugared_ast::IndexExpr::BareNat(_) => {}
    }
    Ok(())
}

fn collect_finite_indexes_from_generic_arg(
    arg: &crate::desugar::desugared_ast::GenericArg,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match arg {
        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
            collect_finite_indexes_from_type_expr(type_expr, registry, src)
        }
        crate::desugar::desugared_ast::GenericArg::Index(index) => {
            collect_finite_index_expr(index, registry, src)
        }
        crate::desugar::desugared_ast::GenericArg::Nat(_)
        | crate::desugar::desugared_ast::GenericArg::Ambiguous(_) => Ok(()),
    }
}

/// Register every concrete `Fin(N)` identity used by a type expression.
fn collect_finite_indexes_from_type_expr(
    type_expr: &crate::desugar::desugared_ast::TypeExpr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if let crate::desugar::desugared_ast::TypeExprKind::Indexed { base, indexes } = &type_expr.kind
    {
        collect_finite_indexes_from_type_expr(base, registry, src)?;
        for index in indexes {
            collect_finite_index_expr(index, registry, src)?;
        }
    }
    match &type_expr.kind {
        crate::desugar::desugared_ast::TypeExprKind::TypeApplication { generic_args, .. } => {
            for arg in generic_args {
                collect_finite_indexes_from_generic_arg(arg, registry, src)?;
            }
        }
        crate::desugar::desugared_ast::TypeExprKind::DatetimeApplication { type_args } => {
            for arg in type_args {
                collect_finite_indexes_from_type_expr(arg, registry, src)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Register concrete `Fin(N)` identities used by comprehensions and tables.
fn collect_finite_indexes_from_expr(
    expr: &crate::desugar::desugared_ast::Expr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::{ExprKind, ForBindingIndex};

    struct FiniteCollector<'a> {
        registry: &'a mut RegistryBuilder,
        src: &'a NamedSource<Arc<String>>,
    }

    impl crate::syntax::visitor::ExprVisitor<crate::syntax::phase::Desugared> for FiniteCollector<'_> {
        type Error = GraphcalError;

        fn visit_expr(
            &mut self,
            expr: &crate::desugar::desugared_ast::Expr,
        ) -> Result<(), GraphcalError> {
            match &expr.kind {
                ExprKind::ForComp { bindings, .. } => {
                    for binding in bindings {
                        if let ForBindingIndex::Finite { cardinality, span } = &binding.index
                            && let Some(value) = concrete_nat_value(cardinality, self.src)?
                        {
                            ensure_concrete_finite_index(value, *span, self.registry, self.src)?;
                        }
                    }
                }
                ExprKind::MapLiteral { entries } => {
                    for entry in entries {
                        for key in &entry.keys {
                            if let crate::syntax::ast::MapEntryIndex::Finite(cardinality) =
                                &key.index.value
                            {
                                ensure_concrete_finite_index(
                                    *cardinality,
                                    key.index.span,
                                    self.registry,
                                    self.src,
                                )?;
                            }
                        }
                    }
                }
                _ => {}
            }
            self.dispatch(expr)
        }
    }

    let mut collector = FiniteCollector { registry, src };
    collector.visit_expr(expr)
}

fn register_index_decl(
    idx: &crate::desugar::desugared_ast::IndexDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<(), GraphcalError> {
    let kind = match &idx.kind {
        crate::desugar::desugared_ast::IndexDeclKind::Named { variants } => {
            types::IndexKind::Named {
                variants: variants.iter().map(|v| v.value.clone()).collect(),
            }
        }
        crate::desugar::desugared_ast::IndexDeclKind::Range {
            start: start_expr,
            end: end_expr,
            step: step_expr,
        } => lower_range_index(
            &idx.name.value,
            start_expr,
            end_expr,
            step_expr,
            registry,
            src,
            decl_span,
        )?,
        crate::desugar::desugared_ast::IndexDeclKind::Linspace {
            start: start_expr,
            end: end_expr,
            points,
        } => lower_linspace_index(
            &idx.name.value,
            start_expr,
            end_expr,
            points,
            registry,
            src,
            decl_span,
        )?,
        crate::desugar::desugared_ast::IndexDeclKind::RequiredNamed => {
            types::IndexKind::RequiredNamed
        }
        crate::desugar::desugared_ast::IndexDeclKind::RequiredCoordinate { dimension } => {
            let dim = registry
                .resolve_dim_expr_detailed(dimension)
                .map_err(|err| dimension_resolve_error(err, src, dimension.span))?;
            types::IndexKind::RequiredCoordinate { dimension: dim }
        }
    };
    registry.register_index(types::IndexDef {
        name: idx.name.value.clone(),
        kind,
    });
    Ok(())
}

fn register_type_decl(
    t: &crate::desugar::desugared_ast::TypeDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    validate_type_generic_params(t, src)?;
    let generic_params: Vec<types::TypeGenericParam> = t
        .generic_params
        .iter()
        .map(|g| types::TypeGenericParam {
            name: g.name.value.clone(),
            constraint: g.constraint.into(),
            default: g.default.clone(),
        })
        .collect();

    let kind = match &t.body {
        crate::desugar::desugared_ast::TypeDeclBody::Required => types::TypeDefKind::Required,
        crate::desugar::desugared_ast::TypeDeclBody::Constructors(type_members) => {
            // Every constructor carries its payload inline; no per-constructor
            // TypeDef is synthesized. The constructor namespace lives on the
            // registry and points back to this type.
            let members = type_members
                .iter()
                .map(|m| {
                    let fields = m.payload.as_ref().map_or_else(Vec::new, |fs| {
                        fs.iter()
                            .map(|f| types::StructField {
                                name: f.name.value.clone(),
                                type_ann: f.type_ann.clone(),
                            })
                            .collect()
                    });
                    types::UnionMemberDef {
                        name: ConstructorName::expect_valid(m.name.value.as_str()),
                        fields,
                    }
                })
                .collect();
            types::TypeDefKind::Union { members }
        }
    };

    registry.register_type(types::TypeDef {
        name: t.name.value.clone(),
        generic_params,
        kind,
    });
    Ok(())
}

fn validate_type_generic_params(
    t: &crate::desugar::desugared_ast::TypeDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let positions = t.generic_params.iter().enumerate().try_fold(
        HashMap::new(),
        |mut positions, (index, param)| match positions.insert(
            param.name.value.atom().clone(),
            (param.name.value.clone(), index, param.name.span),
        ) {
            Some((name, _, _)) => Err(GraphcalError::EvalError {
                message: format!("duplicate generic parameter `{name}`"),
                src: src.clone(),
                span: param.name.span.into(),
            }),
            None => Ok(positions),
        },
    )?;

    let mut first_defaulted: Option<&GenericParamName> = None;
    for (index, param) in t.generic_params.iter().enumerate() {
        match &param.default {
            Some(default) => {
                first_defaulted.get_or_insert(&param.name.value);
                if let Some((referenced, span)) =
                    find_non_earlier_generic_reference(default, index, &positions)
                {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "default for generic parameter `{}` may reference only earlier generic parameters; `{referenced}` is not earlier",
                            param.name.value
                        ),
                        src: src.clone(),
                        span: span.into(),
                    });
                }
            }
            None => {
                if let Some(first_defaulted) = first_defaulted {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` without a default cannot follow defaulted parameter `{first_defaulted}`",
                            param.name.value
                        ),
                        src: src.clone(),
                        span: param.name.span.into(),
                    });
                }
            }
        }
    }
    Ok(())
}

type GenericParamPositions = HashMap<NameAtom, (GenericParamName, usize, Span)>;

fn find_non_earlier_generic_reference(
    arg: &crate::desugar::desugared_ast::GenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        crate::syntax::ast::GenericArg::Type(type_expr) => {
            find_non_earlier_type_reference(type_expr, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Index(index) => {
            find_non_earlier_index_reference(index, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Nat(nat_expr) => {
            find_non_earlier_nat_reference(nat_expr, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
            find_non_earlier_ambiguous_reference(ambiguous, current_index, positions)
        }
    }
}

fn non_earlier_generic_reference(
    name: &NameAtom,
    span: Span,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    positions
        .get(name)
        .filter(|(_, index, _)| *index >= current_index)
        .map(|(name, _, _)| (name.clone(), span))
}

fn find_non_earlier_path_reference(
    path: &Spanned<NamePath>,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    path.value.is_bare().then_some(())?;
    non_earlier_generic_reference(path.value.leaf(), path.span, current_index, positions)
}

fn find_non_earlier_ambiguous_reference(
    arg: &crate::syntax::ast::AmbiguousGenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        crate::syntax::ast::AmbiguousGenericArg::Name(ident) => {
            non_earlier_generic_reference(&ident.name, ident.span, current_index, positions)
        }
        crate::syntax::ast::AmbiguousGenericArg::Mul(operands, _) => {
            operands.iter().find_map(|operand| {
                find_non_earlier_ambiguous_reference(operand, current_index, positions)
            })
        }
    }
}

fn find_non_earlier_nat_reference(
    expr: &crate::syntax::ast::NatExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match expr {
        crate::syntax::ast::NatExpr::Literal(..) => None,
        crate::syntax::ast::NatExpr::Var(ident) => {
            non_earlier_generic_reference(&ident.name, ident.span, current_index, positions)
        }
        crate::syntax::ast::NatExpr::Add(operands, _)
        | crate::syntax::ast::NatExpr::Mul(operands, _) => operands
            .iter()
            .find_map(|operand| find_non_earlier_nat_reference(operand, current_index, positions)),
    }
}

fn find_non_earlier_index_reference(
    index: &crate::syntax::ast::IndexExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match index {
        crate::syntax::ast::IndexExpr::Name(path) => {
            find_non_earlier_path_reference(path, current_index, positions)
        }
        crate::syntax::ast::IndexExpr::Finite { cardinality, .. }
        | crate::syntax::ast::IndexExpr::BareNat(cardinality) => {
            find_non_earlier_nat_reference(cardinality, current_index, positions)
        }
    }
}

fn find_non_earlier_type_reference(
    type_expr: &TypeExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    use crate::desugar::desugared_ast::TypeExprKind;

    match &type_expr.kind {
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => None,
        TypeExprKind::DimExpr(dim_expr) => dim_expr.terms.iter().find_map(|item| {
            find_non_earlier_path_reference(&item.term.name, current_index, positions)
        }),
        TypeExprKind::Indexed { base, indexes } => {
            find_non_earlier_type_reference(base, current_index, positions).or_else(|| {
                indexes.iter().find_map(|index| {
                    find_non_earlier_index_reference(index, current_index, positions)
                })
            })
        }
        TypeExprKind::TypeApplication { generic_args, .. } => generic_args
            .iter()
            .find_map(|arg| find_non_earlier_generic_reference(arg, current_index, positions)),
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => generic_args
            .iter()
            .find_map(|arg| find_non_earlier_generic_reference(arg, current_index, positions)),
        TypeExprKind::DatetimeApplication { type_args } => type_args.iter().find_map(|type_arg| {
            find_non_earlier_type_reference(type_arg, current_index, positions)
        }),
    }
}

/// Evaluate a constant scale expression (e.g. `1000`, `PI / 180`) to `f64`.
///
/// Scale expressions appear in unit definitions and are restricted to numeric
/// literals, built-in constants (`PI`, `E`), and basic arithmetic.
fn eval_scale_expr(expr: &Expr, src: &NamedSource<Arc<String>>) -> Result<f64, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(*n),
        #[expect(clippy::cast_precision_loss, reason = "unit scale constant expression")]
        ExprKind::Integer(n) => Ok(*n as f64),
        ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
            // Route through the typed builtin-constant table instead of
            // string-matching a hand-picked subset: all built-in constants
            // (PI, E, TAU, SQRT2, LN2, LN10) are legal in scale expressions.
            let builtin = path
                .as_bare()
                .and_then(|ident| crate::builtin::BuiltinConst::parse(ident.name.as_str()));
            builtin
                .map(crate::builtin::BuiltinConst::value)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!(
                        "unknown constant `{}` in scale expression; only built-in \
                         constants (PI, E, TAU, SQRT2, LN2, LN10) are supported",
                        path.display_path()
                    ),
                    src: src.clone(),
                    span: path.span().into(),
                })
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            use crate::desugar::desugared_ast::BinOp;
            use crate::syntax::ast::PowerExponent;
            let lhs_value = eval_scale_expr(lhs, src)?;
            if let BinOp::Pow(PowerExponent::Exact(exponent)) = op {
                return exponent
                    .pow_f64(lhs_value)
                    .map_err(|error| GraphcalError::EvalError {
                        message: error.to_string(),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
            }
            let rhs_value = eval_scale_expr(rhs, src)?;
            match op {
                BinOp::Add => Ok(lhs_value + rhs_value),
                BinOp::Sub => Ok(lhs_value - rhs_value),
                BinOp::Mul => Ok(lhs_value * rhs_value),
                BinOp::Div => Ok(lhs_value / rhs_value),
                BinOp::Pow(_) => Ok(lhs_value.powf(rhs_value)),
                _ => Err(GraphcalError::EvalError {
                    message: format!(
                        "unsupported operator `{op:?}` in scale expression; \
                         only `+`, `-`, `*`, `/`, `^` are allowed"
                    ),
                    src: src.clone(),
                    span: expr.span.into(),
                }),
            }
        }
        ExprKind::UnaryOp {
            op: crate::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => Ok(-eval_scale_expr(operand, src)?),
        _ => Err(GraphcalError::EvalError {
            message: "scale expression must be a constant expression \
                      (numbers, PI, E, and arithmetic)"
                .to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
    }
}

/// Evaluate a statically known coordinate expression to its SI value and dimension.
///
/// This is a pure compile-time evaluator: graph references, calls, integer
/// values, and dynamic units are rejected. Arithmetic over quantity literals is
/// accepted so coordinate declarations are not limited to a single literal.
fn eval_coordinate_expr(
    expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(f64, crate::dimension::Dimension), GraphcalError> {
    use crate::desugar::desugared_ast::BinOp;
    use crate::dimension::Dimension;

    let ensure_finite = |value: f64, span: Span| {
        value.is_finite().then_some(value).ok_or_else(|| {
            eval_error(
                format!("coordinate expression must evaluate to a finite quantity, got {value}"),
                src,
                span,
            )
        })
    };

    match &expr.kind {
        ExprKind::Number(value) => Ok((
            ensure_finite(*value, expr.span)?,
            Dimension::dimensionless(),
        )),
        ExprKind::UnitLiteral { value, unit } => {
            let (dimension, scale) = registry
                .resolve_unit_expr(unit)
                .map_err(|error| unit_resolve_to_graphcal(error, src, unit.span))?;
            let scale =
                validate_positive_finite_scale(scale, "coordinate unit scale", src, unit.span)?;
            Ok((ensure_finite(*value * scale.get(), expr.span)?, dimension))
        }
        ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
            let value = path
                .as_bare()
                .and_then(|ident| crate::builtin::BuiltinConst::parse(ident.name.as_str()))
                .map(crate::builtin::BuiltinConst::value)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!(
                        "coordinate expression must be statically evaluable; `{}` is not a built-in constant",
                        path.display_path()
                    ),
                    src: src.clone(),
                    span: path.span().into(),
                })?;
            Ok((value, Dimension::dimensionless()))
        }
        ExprKind::UnaryOp {
            op: crate::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => {
            let (value, dimension) = eval_coordinate_expr(operand, registry, src)?;
            Ok((ensure_finite(-value, expr.span)?, dimension))
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let (lhs_value, lhs_dimension) = eval_coordinate_expr(lhs, registry, src)?;
            let (rhs_value, rhs_dimension) = eval_coordinate_expr(rhs, registry, src)?;
            let overflow = || GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: expr.span.into(),
            };
            let (value, dimension) = match op {
                BinOp::Add | BinOp::Sub => {
                    if lhs_dimension != rhs_dimension {
                        return Err(GraphcalError::EvalError {
                            message: "addition or subtraction in a coordinate expression requires matching dimensions".to_string(),
                            src: src.clone(),
                            span: expr.span.into(),
                        });
                    }
                    let value = if *op == BinOp::Add {
                        lhs_value + rhs_value
                    } else {
                        lhs_value - rhs_value
                    };
                    (value, lhs_dimension)
                }
                BinOp::Mul => (
                    lhs_value * rhs_value,
                    lhs_dimension
                        .checked_mul(&rhs_dimension)
                        .map_err(|_| overflow())?,
                ),
                BinOp::Div => (
                    lhs_value / rhs_value,
                    lhs_dimension
                        .checked_div(&rhs_dimension)
                        .map_err(|_| overflow())?,
                ),
                _ => {
                    return Err(eval_error(
                        "coordinate arguments support only static quantity arithmetic",
                        src,
                        expr.span,
                    ));
                }
            };
            Ok((ensure_finite(value, expr.span)?, dimension))
        }
        _ => Err(eval_error(
            "coordinate arguments must be statically evaluable quantities; Int values and runtime expressions are not supported",
            src,
            expr.span,
        )),
    }
}

fn coordinate_invalid(
    name: &IndexName,
    message: impl Into<String>,
    help: impl Into<String>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::CoordinateIndexInvalid {
        name: name.clone(),
        message: message.into(),
        help: help.into(),
        src: src.clone(),
        span: span.into(),
    }
}

/// One centralized binary64 endpoint comparison for coordinate construction.
fn coordinate_values_equal(actual: f64, expected: f64, scale_hint: f64) -> bool {
    let scale = actual.abs().max(expected.abs()).max(scale_hint.abs());
    // Keep the tolerance relative even for coordinates near zero. A unit-scale
    // floor would accept steps that miss tiny endpoints by a large fraction.
    // The subnormal floor covers a small number of binary64 ULPs at zero.
    let tolerance = (scale * (32.0 * f64::EPSILON)).max(f64::from_bits(32));
    (actual - expected).abs() <= tolerance
}

/// Exact numeric endpoint equality after finite-value validation.
fn coordinate_endpoints_equal(start: f64, end: f64) -> bool {
    matches!(start.partial_cmp(&end), Some(std::cmp::Ordering::Equal))
}

fn checked_range_cardinality(
    name: &IndexName,
    start: f64,
    end: f64,
    step: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<types::IndexCardinality, GraphcalError> {
    if step == 0.0 {
        return Err(coordinate_invalid(
            name,
            "step must be nonzero",
            "use an explicitly positive or negative quantity step",
            src,
            span,
        ));
    }
    if (start < end && step < 0.0) || (start > end && step > 0.0) {
        return Err(coordinate_invalid(
            name,
            format!("step {step} moves away from endpoint {end}"),
            "use a positive step for an ascending range and a negative step for a descending range",
            src,
            span,
        ));
    }
    if coordinate_endpoints_equal(start, end) {
        return types::IndexCardinality::try_from_u64(1).map_err(|error| {
            coordinate_invalid(name, error.to_string(), "reduce the index size", src, span)
        });
    }

    let raw_intervals = (end - start) / step;
    if !raw_intervals.is_finite() || raw_intervals < 0.0 {
        return Err(coordinate_invalid(
            name,
            "range interval count is not a finite non-negative value",
            "choose finite bounds and a finite nonzero step with matching direction",
            src,
            span,
        ));
    }
    let intervals = raw_intervals.round();
    let reconstructed_end = intervals.mul_add(step, start);
    if intervals < 1.0 || !coordinate_values_equal(reconstructed_end, end, intervals * step) {
        return Err(coordinate_invalid(
            name,
            format!("step {step} does not land on endpoint {end}"),
            "change the endpoint or use `linspace(start, end, points: N)` when the point count is authoritative",
            src,
            span,
        ));
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "the practical limit is far below f64's exact integer range"
    )]
    if intervals >= types::MAX_INDEX_CARDINALITY as f64 {
        return Err(coordinate_invalid(
            name,
            format!(
                "range requires {} coordinate points, which exceeds the practical limit of {}",
                intervals + 1.0,
                types::MAX_INDEX_CARDINALITY
            ),
            format!(
                "reduce the index to at most {} points",
                types::MAX_INDEX_CARDINALITY
            ),
            src,
            span,
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the rounded interval count is finite, non-negative, and practically bounded"
    )]
    let count = intervals as u64 + 1;
    types::IndexCardinality::try_from_u64(count).map_err(|error| {
        coordinate_invalid(name, error.to_string(), "reduce the index size", src, span)
    })
}

fn eval_static_nat_expr(
    expr: &crate::desugar::desugared_ast::NatExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<u64, GraphcalError> {
    use crate::desugar::desugared_ast::NatExpr;
    match expr {
        NatExpr::Literal(value, _) => Ok(*value),
        NatExpr::Var(ident) => Err(GraphcalError::EvalError {
            message: format!(
                "linspace point count must be statically known; `{}` is not a constant Nat",
                ident.name
            ),
            src: src.clone(),
            span: ident.span.into(),
        }),
        NatExpr::Add(operands, span) => operands.iter().try_fold(0_u64, |sum, operand| {
            sum.checked_add(eval_static_nat_expr(operand, src)?)
                .ok_or_else(|| eval_error("linspace point-count addition overflow", src, *span))
        }),
        NatExpr::Mul(operands, span) => operands.iter().try_fold(1_u64, |product, operand| {
            product
                .checked_mul(eval_static_nat_expr(operand, src)?)
                .ok_or_else(|| {
                    eval_error("linspace point-count multiplication overflow", src, *span)
                })
        }),
    }
}

fn coordinate_display_unit(
    start_expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(Option<String>, f64), GraphcalError> {
    let unit = match &start_expr.kind {
        ExprKind::UnitLiteral { unit, .. } => unit,
        ExprKind::UnaryOp {
            op: crate::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => return coordinate_display_unit(operand, registry, src),
        _ => return Ok((None, 1.0)),
    };
    match registry.resolve_unit_expr(unit) {
        Ok((_dimension, scale)) => {
            let scale = validate_positive_finite_scale(
                scale,
                "coordinate display unit scale",
                src,
                unit.span,
            )?;
            Ok((Some(format_unit_expr_with_config(unit, true)), scale.get()))
        }
        Err(crate::registry::types::UnitResolveError::Overflow(_)) => {
            Err(GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: unit.span.into(),
            })
        }
        Err(crate::registry::types::UnitResolveError::InvalidScale { value, reason }) => {
            let reason = match reason {
                PositiveFiniteScaleError::NonFinite => "must be finite",
                PositiveFiniteScaleError::NonPositive => "must be greater than zero",
            };
            Err(eval_error(
                format!("coordinate display unit scale {reason}, got {value}"),
                src,
                unit.span,
            ))
        }
        Err(_) => Ok((None, 1.0)),
    }
}

fn validate_coordinate_values(
    name: &IndexName,
    data: &types::CoordinateIndexData,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    let direction = data.end.total_cmp(&data.start);
    let count = data.cardinality.get();
    let first = data.coordinate_value(0);
    (1..count).try_fold(first, |previous, position| {
        let current = data.coordinate_value(position);
        if !current.is_finite() {
            return Err(coordinate_invalid(
                name,
                format!("coordinate at position {position} is not finite"),
                "reduce the cardinality or use bounds and spacing with stable binary64 coordinates",
                src,
                span,
            ));
        }
        let monotonic = match direction {
            std::cmp::Ordering::Less => current < previous,
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Greater => current > previous,
        };
        if !monotonic {
            return Err(coordinate_invalid(
                name,
                format!(
                    "coordinates at positions {} and {position} are duplicate or non-monotonic in binary64",
                    position - 1
                ),
                "reduce the cardinality or increase the spacing between adjacent coordinates",
                src,
                span,
            ));
        }
        Ok(current)
    })?;
    Ok(())
}

/// Lower `range(start, end, step: delta)` with exact endpoint semantics.
fn lower_range_index(
    name: &IndexName,
    start_expr: &Expr,
    end_expr: &Expr,
    step_expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<types::IndexKind, GraphcalError> {
    let (start, start_dimension) = eval_coordinate_expr(start_expr, registry, src)?;
    let (end, end_dimension) = eval_coordinate_expr(end_expr, registry, src)?;
    let (step, step_dimension) = eval_coordinate_expr(step_expr, registry, src)?;
    if start_dimension != end_dimension || start_dimension != step_dimension {
        return Err(GraphcalError::CoordinateIndexDimensionMismatch {
            name: name.clone(),
            message: format!(
                "range start, end, and step have dimensions {}, {}, and {}",
                registry.format_dimension(&start_dimension),
                registry.format_dimension(&end_dimension),
                registry.format_dimension(&step_dimension)
            ),
            src: src.clone(),
            span: decl_span.into(),
        });
    }
    let cardinality = checked_range_cardinality(name, start, end, step, src, decl_span)?;
    let (display_label, display_scale) = coordinate_display_unit(start_expr, registry, src)?;
    let data = types::CoordinateIndexData {
        start,
        end,
        spacing: types::CoordinateSpacing::Step { step },
        cardinality,
        dimension: start_dimension,
        display_label,
        display_scale,
    };
    if cardinality.get() > 1 {
        validate_coordinate_values(name, &data, src, decl_span)?;
    }
    Ok(types::IndexKind::Coordinate(data))
}

/// Lower `linspace(start, end, points: N)` with exact endpoint semantics.
fn lower_linspace_index(
    name: &IndexName,
    start_expr: &Expr,
    end_expr: &Expr,
    points_expr: &crate::desugar::desugared_ast::NatExpr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<types::IndexKind, GraphcalError> {
    let (start, start_dimension) = eval_coordinate_expr(start_expr, registry, src)?;
    let (end, end_dimension) = eval_coordinate_expr(end_expr, registry, src)?;
    if start_dimension != end_dimension {
        return Err(GraphcalError::CoordinateIndexDimensionMismatch {
            name: name.clone(),
            message: format!(
                "linspace start and end have dimensions {} and {}",
                registry.format_dimension(&start_dimension),
                registry.format_dimension(&end_dimension)
            ),
            src: src.clone(),
            span: decl_span.into(),
        });
    }
    let points = eval_static_nat_expr(points_expr, src)?;
    if points == 0 {
        return Err(coordinate_invalid(
            name,
            "linspace requires at least one point",
            "use `points: 1` or greater",
            src,
            points_expr.span(),
        ));
    }
    let cardinality = types::IndexCardinality::try_from_u64(points).map_err(|error| {
        coordinate_invalid(
            name,
            error.to_string(),
            format!(
                "use a point count from 1 through {}",
                types::MAX_INDEX_CARDINALITY
            ),
            src,
            points_expr.span(),
        )
    })?;
    match (cardinality.get(), coordinate_endpoints_equal(start, end)) {
        (1, false) => {
            return Err(coordinate_invalid(
                name,
                "linspace with one point requires identical start and end values",
                "set end equal to start or request at least two points",
                src,
                decl_span,
            ));
        }
        (2.., true) => {
            return Err(coordinate_invalid(
                name,
                "linspace with two or more points requires distinct endpoints",
                "use `points: 1` for a singleton or choose distinct endpoints",
                src,
                decl_span,
            ));
        }
        _ => {}
    }
    let (display_label, display_scale) = coordinate_display_unit(start_expr, registry, src)?;
    let data = types::CoordinateIndexData {
        start,
        end,
        spacing: types::CoordinateSpacing::Linspace,
        cardinality,
        dimension: start_dimension,
        display_label,
        display_scale,
    };
    if cardinality.get() > 1 {
        validate_coordinate_values(name, &data, src, decl_span)?;
    }
    Ok(types::IndexKind::Coordinate(data))
}

/// Extract a map of type annotations from const/param/node declarations,
/// keyed by their typed declaration names.
fn extract_type_annotations(ast: &File) -> HashMap<DeclName, TypeExpr> {
    let mut type_anns = HashMap::new();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::Param(p) => {
                type_anns.insert(p.name.value.clone(), p.type_ann.clone());
            }
            DeclKind::Node(n) => {
                type_anns.insert(n.name.value.clone(), n.type_ann.clone());
            }
            DeclKind::ConstNode(c) => {
                type_anns.insert(c.name.value.clone(), c.type_ann.clone());
            }
            _ => {}
        }
    }
    type_anns
}

// ---------------------------------------------------------------------------
// Extern plugin functions
// ---------------------------------------------------------------------------

/// A resolved extern function declared by an `import plugin` block.
///
/// The signature is stored in structural (base-dimension exponent) form —
/// this is what Phase B of the plugin plan (#25) compares against embedded
/// plugin manifests.
#[derive(Debug, Clone)]
pub struct ExternFunctionEntry {
    /// Canonical plugin identity (the `import plugin "…"` path string).
    pub plugin: crate::syntax::plugin::PluginPath,
    /// The alias the declaring file bound the plugin to (diagnostics only).
    pub alias: crate::syntax::module_name::ModuleAliasName,
    /// The function leaf name.
    pub name: crate::syntax::function_name::FnName,
    /// The resolved dimensional signature.
    pub signature: crate::function_signature::FunctionSignature,
    /// The nominal identity of the record type a struct-returning function
    /// produces. `Some` exactly when the signature's result is
    /// [`crate::function_signature::ValueKind::Struct`]: the manifest side
    /// is structural, but the declaration binds the shape to this type, and
    /// checking/evaluation carry the nominal identity from here.
    pub result_struct: Option<ExternStructResult>,
    /// Span of the function name inside the declaring block.
    pub name_span: Span,
    /// Span of the whole `fn ...;` declaration.
    pub decl_span: Span,
    /// Span of the `import plugin "…"` path string, for diagnostics about
    /// the plugin as a whole (unloadable file, hash mismatch, …).
    pub path_span: Span,
}

/// The record type a struct-returning extern function was declared with.
#[derive(Debug, Clone)]
pub struct ExternStructResult {
    /// Canonical identity of the record type named at the declaration site.
    pub resolved: crate::syntax::type_name::ResolvedStructTypeName,
}

/// Resolve every `import plugin` block's declared signatures against the
/// frozen registry.
///
/// Dimension names resolve in the importing scope (prelude + user dims);
/// dimension variables come from each function's explicit `<...>` binders;
/// record result types resolve through the module resolver in the
/// importing scope.
fn resolve_plugin_imports(
    decls: &[crate::desugar::desugared_ast::PluginImportDecl],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<crate::syntax::plugin::ExternFnKey, ExternFunctionEntry>, GraphcalError> {
    use std::collections::hash_map::Entry;

    let mut map = HashMap::new();
    for decl in decls {
        for function in &decl.functions {
            let (key, entry) =
                resolve_extern_function(decl, function, registry, owner, resolver, src)?;
            match map.entry(key) {
                Entry::Occupied(existing) => {
                    // The same plugin function may be declared under several
                    // aliases, but its signature is a single fact about the
                    // plugin — conflicting declarations are rejected.
                    // Comparison is structural: renaming dimension variables
                    // or parameters does not make a different signature.
                    let existing: &ExternFunctionEntry = existing.get();
                    if !existing.signature.structurally_equivalent(&entry.signature) {
                        return Err(GraphcalError::InvalidExternSignature {
                            message: format!(
                                "function `{}` of plugin \"{}\" is declared elsewhere with a different signature",
                                entry.name, entry.plugin
                            ),
                            src: src.clone(),
                            span: function.span.into(),
                        });
                    }
                    // A struct return is nominal at the declaration site:
                    // two aliases must also agree on WHICH record type the
                    // shared shape produces.
                    let existing_struct = existing.result_struct.as_ref().map(|s| &s.resolved);
                    let entry_struct = entry.result_struct.as_ref().map(|s| &s.resolved);
                    if existing_struct != entry_struct {
                        return Err(GraphcalError::InvalidExternSignature {
                            message: format!(
                                "function `{}` of plugin \"{}\" is declared elsewhere with a different result type",
                                entry.name, entry.plugin
                            ),
                            src: src.clone(),
                            span: function.span.into(),
                        });
                    }
                }
                Entry::Vacant(slot) => {
                    slot.insert(entry);
                }
            }
        }
    }
    Ok(map)
}

/// Resolve one extern-signature type annotation to a [`crate::function_signature::ValueKind`].
fn resolve_extern_value_kind(
    type_ann: &TypeExpr,
    dim_vars: &[crate::syntax::dimension::DimVarName],
    index_vars: &[crate::syntax::index_name::IndexVarName],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::ValueKind, GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::ValueKind;

    if !type_ann.constraints.is_empty() {
        return Err(GraphcalError::InvalidExternSignature {
            message: "domain constraints are not allowed in extern function signatures".to_string(),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    match &type_ann.kind {
        TypeExprKind::Bool => Ok(ValueKind::Bool),
        TypeExprKind::Int => Ok(ValueKind::Int),
        TypeExprKind::Dimensionless => Ok(ValueKind::dimensionless()),
        TypeExprKind::DimExpr(dim_expr) => {
            resolve_extern_dim_monomial(dim_expr, dim_vars, registry, src)
                .map(ValueKind::Quantity)
        }
        TypeExprKind::Indexed { base, indexes } => {
            resolve_extern_array_kind(
                base,
                indexes.as_slice(),
                dim_vars,
                index_vars,
                registry,
                src,
            )
        }
        TypeExprKind::Datetime
        | TypeExprKind::DatetimeApplication { .. }
        | TypeExprKind::ComplexApplication { .. }
        | TypeExprKind::KeyApplication { .. }
        | TypeExprKind::TypeApplication { .. } => Err(GraphcalError::InvalidExternSignature {
            message:
                "extern function signatures support Bool, Int, quantity types, and arrays of quantities over one or more declared index variables"
                    .to_string(),
            src: src.clone(),
            span: type_ann.span.into(),
        }),
    }
}

/// Resolve one `fn` declaration of an `import plugin` block to its keyed
/// [`ExternFunctionEntry`].
fn resolve_extern_function(
    decl: &crate::desugar::desugared_ast::PluginImportDecl,
    function: &crate::desugar::desugared_ast::ExternFnDecl,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<(crate::syntax::plugin::ExternFnKey, ExternFunctionEntry), GraphcalError> {
    // Binder idents share one lexical namespace regardless of
    // constraint: `<D: Dim, D: Index>` is a duplicate declaration.
    let mut seen_binders: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for binder in &function.generics {
        if !seen_binders.insert(binder.name_str()) {
            return Err(GraphcalError::InvalidExternSignature {
                message: format!(
                    "generic binder `{}` is declared more than once",
                    binder.name_str()
                ),
                src: src.clone(),
                span: binder.span().into(),
            });
        }
    }
    let dim_vars: Vec<crate::syntax::dimension::DimVarName> = function
        .generics
        .iter()
        .filter_map(|binder| match binder {
            crate::syntax::ast::ExternGenericBinder::Dim(var) => Some(var.value.clone()),
            crate::syntax::ast::ExternGenericBinder::Index(_) => None,
        })
        .collect();
    let index_vars: Vec<crate::syntax::index_name::IndexVarName> = function
        .generics
        .iter()
        .filter_map(|binder| match binder {
            crate::syntax::ast::ExternGenericBinder::Index(var) => Some(var.value.clone()),
            crate::syntax::ast::ExternGenericBinder::Dim(_) => None,
        })
        .collect();
    let params = function
        .params
        .iter()
        .map(|param| {
            let kind =
                resolve_extern_value_kind(&param.type_ann, &dim_vars, &index_vars, registry, src)?;
            Ok(crate::function_signature::FunctionParam {
                name: param.name.value.clone(),
                kind,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let (result, result_struct) = resolve_extern_result_kind(
        &function.result,
        &dim_vars,
        &index_vars,
        registry,
        owner,
        resolver,
        src,
    )?;
    let signature =
        crate::function_signature::FunctionSignature::try_new(dim_vars, index_vars, params, result)
            .map_err(|err| {
                if let crate::function_signature::SignatureError::DuplicateParamName {
                    name,
                    first,
                    duplicate,
                } = &err
                    && let (Some(first_param), Some(duplicate_param)) =
                        (function.params.get(*first), function.params.get(*duplicate))
                {
                    return GraphcalError::DuplicateExternParameter {
                        name: name.clone(),
                        src: src.clone(),
                        duplicate: duplicate_param.name.span.into(),
                        first: first_param.name.span.into(),
                    };
                }
                GraphcalError::InvalidExternSignature {
                    message: err.to_string(),
                    src: src.clone(),
                    span: function.span.into(),
                }
            })?;
    let key = crate::syntax::plugin::ExternFnKey {
        plugin: decl.path.value.clone(),
        name: function.name.value.clone(),
    };
    let entry = ExternFunctionEntry {
        plugin: decl.path.value.clone(),
        alias: decl.alias.value.clone(),
        name: function.name.value.clone(),
        signature,
        result_struct,
        name_span: function.name.span,
        decl_span: function.span,
        path_span: decl.path.span,
    };
    Ok((key, entry))
}

/// Resolve an extern RESULT type annotation: any parameter kind, or a
/// record struct type in scope.
///
/// A bare name in result position that is neither a declared dimension
/// variable nor a dimension is tried as a record type; the struct branch
/// returns the flattened field shape plus the nominal identity the
/// declaration binds it to.
fn resolve_extern_result_kind(
    type_ann: &TypeExpr,
    dim_vars: &[crate::syntax::dimension::DimVarName],
    index_vars: &[crate::syntax::index_name::IndexVarName],
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<
    (
        crate::function_signature::ValueKind,
        Option<ExternStructResult>,
    ),
    GraphcalError,
> {
    use crate::desugar::desugared_ast::TypeExprKind;

    if type_ann.constraints.is_empty()
        && let TypeExprKind::DimExpr(dim_expr) = &type_ann.kind
        && let [item] = dim_expr.terms.as_slice()
        && item.term.power.is_none()
        && let Some(atom) = item.term.name.value.as_bare()
        && !dim_vars.iter().any(|var| var.as_str() == atom.as_str())
        && registry.dimensions.get_dimension(atom.as_str()).is_none()
    {
        // Not a dimension: the only remaining reading is a record type.
        return resolve_extern_struct_return(&item.term, registry, owner, resolver, src);
    }
    if let TypeExprKind::TypeApplication { .. } = &type_ann.kind {
        return Err(GraphcalError::InvalidExternSignature {
            message: "generic struct returns are not supported in this phase; use a record \
                      type with concrete field types"
                .to_string(),
            src: src.clone(),
            span: type_ann.span.into(),
        });
    }
    resolve_extern_value_kind(type_ann, dim_vars, index_vars, registry, src)
        .map(|kind| (kind, None))
}

/// Resolve a record-type extern result: nominal identity through the
/// module resolver, flattened field shape from the registry's definition.
fn resolve_extern_struct_return(
    term: &crate::desugar::desugared_ast::DimTerm,
    registry: &Registry,
    owner: &crate::dag_id::DagId,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<
    (
        crate::function_signature::ValueKind,
        Option<ExternStructResult>,
    ),
    GraphcalError,
> {
    use crate::function_signature::{StructShape, StructShapeField, ValueKind};

    let span = term.name.span;
    let invalid = |message: String| GraphcalError::InvalidExternSignature {
        message,
        src: src.clone(),
        span: span.into(),
    };
    let path = &term.name.value;
    let Ok(resolved_type) = resolver.resolve_struct_type_path(owner, path) else {
        // Neither a dimension nor a type in scope: report it the way any
        // other unknown dimension-position name is reported.
        return Err(GraphcalError::UnknownDimension {
            name: DimName::from_atom(path.as_bare().cloned().unwrap_or_else(|| {
                crate::syntax::names::NameAtom::new_unchecked_for_parser(path.display_path())
            })),
            src: src.clone(),
            span: span.into(),
        });
    };
    let leaf = resolved_type.to_unowned_def_name();
    let Some(type_def) = registry.types.get_type(leaf.as_str()) else {
        return Err(invalid(format!(
            "record type `{leaf}` is not available in this file's registry; extern struct \
             returns must use a type declared in (or imported into) the declaring file"
        )));
    };
    if !type_def.generic_params.is_empty() {
        return Err(invalid(format!(
            "record type `{leaf}` is generic; generic struct returns are not supported in \
             this phase"
        )));
    }
    let Some(fields) = type_def.record_fields() else {
        return Err(invalid(format!(
            "`{leaf}` is not a record type; extern struct returns need a single constructor \
             named after the type"
        )));
    };

    let shape_fields = fields
        .iter()
        .map(|field| {
            let kind = resolve_extern_struct_field(field, registry, src)?;
            Ok(StructShapeField {
                name: field.name.clone(),
                kind,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let shape = StructShape::try_new(shape_fields).map_err(|err| invalid(err.to_string()))?;
    Ok((
        ValueKind::Struct(shape),
        Some(ExternStructResult {
            resolved: resolved_type,
        }),
    ))
}

/// Resolve one record field to its concrete boundary kind.
fn resolve_extern_struct_field(
    field: &crate::registry::type_def::StructField,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::StructFieldKind, GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::StructFieldKind;

    let unsupported = || GraphcalError::InvalidExternSignature {
        message: format!(
            "field `{}` has a type that cannot cross the plugin boundary; struct-return \
             fields support Bool, Int, and quantity types in this phase",
            field.name
        ),
        src: src.clone(),
        span: field.type_ann.span.into(),
    };
    if !field.type_ann.constraints.is_empty() {
        return Err(unsupported());
    }
    match &field.type_ann.kind {
        TypeExprKind::Bool => Ok(StructFieldKind::Bool),
        TypeExprKind::Int => Ok(StructFieldKind::Int),
        TypeExprKind::Dimensionless => Ok(StructFieldKind::Quantity(
            crate::dimension::Dimension::dimensionless(),
        )),
        TypeExprKind::DimExpr(dim_expr) => {
            // No dimension variables are in scope inside a record's fields;
            // the monomial is therefore concrete by construction.
            let monomial = resolve_extern_dim_monomial(dim_expr, &[], registry, src)?;
            Ok(StructFieldKind::Quantity(monomial.fixed))
        }
        TypeExprKind::Datetime
        | TypeExprKind::DatetimeApplication { .. }
        | TypeExprKind::ComplexApplication { .. }
        | TypeExprKind::KeyApplication { .. }
        | TypeExprKind::Indexed { .. }
        | TypeExprKind::TypeApplication { .. } => Err(unsupported()),
    }
}

/// Resolve an array type annotation (`D[I]`, `Velocity[I, J]`) to
/// [`crate::function_signature::ValueKind::Indexed`].
///
/// Every axis must name one of the signature's `Index` binders. Concrete
/// declared/structural indexes remain declaration-site concerns: an extern
/// function is generic over the axes it receives, and every result axis must
/// come from an input.
fn resolve_extern_array_kind(
    base: &TypeExpr,
    indexes: &[crate::syntax::ast::IndexExpr],
    dim_vars: &[crate::syntax::dimension::DimVarName],
    index_vars: &[crate::syntax::index_name::IndexVarName],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::ValueKind, GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    use crate::function_signature::{DimMonomial, ValueKind};
    use crate::syntax::ast::IndexExpr;

    let resolved_indexes = indexes
        .iter()
        .map(|index_expr| {
            let index_var = match index_expr {
                IndexExpr::Name(name) => name.value.as_bare().and_then(|atom| {
                    index_vars
                        .iter()
                        .find(|var| var.as_str() == atom.as_str())
                        .cloned()
                }),
                IndexExpr::Finite { .. } | IndexExpr::BareNat(_) => None,
            };
            index_var.ok_or_else(|| GraphcalError::InvalidExternSignature {
                message: "extern array axes must name the signature's `Index` binders \
                          (concrete indexes and `Fin(N)` axes cannot appear in the declaration)"
                    .to_string(),
                src: src.clone(),
                span: index_expr.span().into(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let indexes =
        crate::syntax::non_empty::NonEmpty::try_from_vec(resolved_indexes).map_err(|_| {
            GraphcalError::InvalidExternSignature {
                message: "extern arrays must have at least one axis".to_string(),
                src: src.clone(),
                span: type_ann_indexes_span(indexes, base),
            }
        })?;

    if !base.constraints.is_empty() {
        return Err(GraphcalError::InvalidExternSignature {
            message: "domain constraints are not allowed in extern function signatures".to_string(),
            src: src.clone(),
            span: base.span.into(),
        });
    }
    let element = match &base.kind {
        TypeExprKind::Dimensionless => DimMonomial::dimensionless(),
        TypeExprKind::DimExpr(dim_expr) => {
            resolve_extern_dim_monomial(dim_expr, dim_vars, registry, src)?
        }
        _ => {
            return Err(GraphcalError::InvalidExternSignature {
                message: "extern array elements must be quantities in this phase".to_string(),
                src: src.clone(),
                span: base.span.into(),
            });
        }
    };
    Ok(ValueKind::Indexed { element, indexes })
}

/// Span covering an array annotation's index list (falls back to the base
/// type's span when the list is empty, which the parser never produces).
fn type_ann_indexes_span(
    indexes: &[crate::syntax::ast::IndexExpr],
    base: &TypeExpr,
) -> miette::SourceSpan {
    match (indexes.first(), indexes.last()) {
        (Some(first), Some(last)) => first.span().merge(last.span()).into(),
        _ => base.span.into(),
    }
}

/// Resolve a dimension expression over declared dim variables and in-scope
/// dimensions into a [`crate::function_signature::DimMonomial`].
fn resolve_extern_dim_monomial(
    dim_expr: &crate::desugar::desugared_ast::DimExpr,
    dim_vars: &[crate::syntax::dimension::DimVarName],
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::function_signature::DimMonomial, GraphcalError> {
    use crate::function_signature::DimVarPower;
    use crate::syntax::ast::MulDivOp;

    let overflow = |span: Span| GraphcalError::DimensionOverflow {
        src: src.clone(),
        span: span.into(),
    };

    let mut vars: Vec<DimVarPower> = Vec::new();
    let mut fixed = crate::dimension::Dimension::dimensionless();
    for item in &dim_expr.terms {
        let term = &item.term;
        let power = term.power.unwrap_or(Rational::ONE);
        let path = &term.name.value;
        if let Some(atom) = path.as_bare()
            && let Some(var) = dim_vars.iter().find(|v| v.as_str() == atom.as_str())
        {
            let power = match item.op {
                MulDivOp::Mul => power,
                MulDivOp::Div => power.checked_neg().map_err(|_| overflow(term.span))?,
            };
            vars.push(DimVarPower {
                var: var.clone(),
                power,
            });
            continue;
        }
        let Some(leaf) = path.as_bare() else {
            return Err(GraphcalError::InvalidExternSignature {
                message: format!(
                    "module-qualified dimension `{}` is not supported in extern function signatures",
                    path.display_path()
                ),
                src: src.clone(),
                span: term.span.into(),
            });
        };
        let Some(dim) = registry.dimensions.get_dimension(leaf.as_str()) else {
            return Err(GraphcalError::UnknownDimension {
                name: DimName::from_atom(leaf.clone()),
                src: src.clone(),
                span: term.name.span.into(),
            });
        };
        let powered = dim.pow(power).map_err(|_| overflow(term.span))?;
        fixed = match item.op {
            MulDivOp::Mul => fixed.checked_mul(&powered),
            MulDivOp::Div => fixed.checked_div(&powered),
        }
        .map_err(|_| overflow(term.span))?;
    }
    Ok(crate::function_signature::DimMonomial { vars, fixed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::declared_type::DeclaredType;
    use crate::syntax::decl_name::ResolvedDeclName;
    use crate::syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<IR, GraphcalError> {
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
    fn unknown_unit_dimension_reports_referenced_dimension() {
        let err = parse_and_lower("unit foo: Blah = 1.0 m;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnknownDimension { name, .. } if name.as_str() == "Blah"
        ));
    }

    #[test]
    fn unknown_derived_dimension_term_reports_referenced_dimension() {
        let err = parse_and_lower("dim Foo = Bar * Baz;").unwrap_err();
        assert!(matches!(
            err,
            GraphcalError::UnknownDimension { name, .. } if name.as_str() == "Bar"
        ));
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
        let (_dep_builder, mut dep_unfrozen) = lower_to_builder(
            &dep_file,
            &dep_src,
            &ImportedNames {
                consts: vec![],
                params: vec![],
                nodes: vec![],
                asserts: vec![],
            },
            &crate::dag_id::DagId::root_in_package("test", "dep"),
        )
        .unwrap();
        let qualified = ScopedName::qualified(
            ModuleAliasName::expect_valid("mission"),
            DeclName::expect_valid("C"),
        );
        let canonical = ResolvedDeclName::from_def(
            crate::dag_id::DagId::root_in_package("producer", "config"),
            DeclName::expect_valid("C"),
        );
        dep_unfrozen.imported_bindings.insert(
            qualified.clone(),
            ImportedBinding::with_value(
                canonical.clone(),
                DeclaredType::Quantity(crate::dimension::Dimension::dimensionless()),
                crate::registry::runtime_value::RuntimeValue::Quantity(7.0),
            ),
        );

        let importer_source = "node anchor: Dimensionless = 1.0;";
        let importer_src = make_src(importer_source);
        let raw_importer = Parser::new(importer_source).parse_file().unwrap();
        let importer_file = crate::syntax::desugar::desugar_multi_decls_in_file(raw_importer);
        let (_importer_builder, mut unfrozen) = lower_to_builder(
            &importer_file,
            &importer_src,
            &ImportedNames {
                consts: vec![],
                params: vec![],
                nodes: vec![],
                asserts: vec![],
            },
            &crate::dag_id::DagId::root_in_package("test", "main"),
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
                &crate::dag_id::DagId::root_in_package("test", "main"),
                &importer_src,
                &dep_src,
            )
            .unwrap();

        let lexical = qualified.within_scope(&instance);
        let binding = &unfrozen.imported_bindings[&lexical];
        assert_eq!(binding.target(), &canonical);
        assert!(matches!(
            binding.declared_type(),
            DeclaredType::Quantity(dimension) if dimension.is_dimensionless()
        ));
        assert!(matches!(
            binding.value(),
            Some(crate::registry::runtime_value::RuntimeValue::Quantity(value))
                if (*value - 7.0).abs() < f64::EPSILON
        ));
        assert!(!unfrozen.imported_bindings.contains_key(&qualified));
    }
}

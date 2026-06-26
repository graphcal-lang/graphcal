//! Intermediate Representation (IR) — the result of lowering an AST.
//!
//! `lower()` combines declaration collection (`resolve`), registry
//! construction (dimensions, units, indexes, structs), and function
//! registration into a single `IR` value. Reference resolution happens at
//! [`UnfrozenIR::freeze`], which lowers every assembled declaration body to
//! HIR — the frozen `IR` carries no syntax-AST expression.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::desugar::desugared_ast::{
    AssertBody, DeclKind, Expr, ExprKind, FigureDecl, File, IndexDeclKind, LayerDecl, PlotDecl,
    TypeExpr,
};
use crate::ir::resolve::{
    DeclCategory, ExpectedFail, ImportedValueNames, ParsedExpectedFail, ResolvedFile,
    resolve_with_imported_values,
};
use crate::ir::resolve::{ImportedNames, resolve_with_imports};
use crate::registry::declared_type::DeclaredType;
use crate::registry::error::GraphcalError;
use crate::registry::format::format_unit_expr;
use crate::registry::prelude::load_prelude;
use crate::registry::runtime_value::RuntimeValue;
use crate::registry::types::{
    self, PositiveFiniteScale, PositiveFiniteScaleError, Registry, RegistryBuilder, UnitScale,
};
use crate::syntax::dimension::Rational;
use crate::syntax::names::{
    ConstructorName, DeclName, DimName, IndexName, NameAtom, NamePath, ScopedName, StructTypeName,
};
use crate::syntax::span::{Span, Spanned};
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
    pub name: crate::syntax::names::PlotPropertyName,
    /// Span of the property name in the source, for validation diagnostics.
    pub name_span: crate::syntax::span::Span,
    pub value: crate::hir::Expr,
}

/// Where a merged declaration body's spans index into (#868).
///
/// A declaration written in the file being compiled spans into that file's
/// own [`NamedSource`], which every lowering/type stage already threads as its
/// ambient `src` — those entries carry [`BodySource::own`]. An instantiated
/// `include` merges a dependency's declaration bodies into the importer's IR
/// (`merge_dependency`); those bodies keep the *dependency's* byte offsets, so
/// they carry [`BodySource::dependency`] naming the dependency file. Rendering
/// a diagnostic for such a body against the importer's source produces an
/// out-of-bounds (or simply wrong) label; [`BodySource::resolve`] hands back
/// the correct source to anchor against.
#[derive(Debug, Clone, Default)]
pub struct BodySource(Option<NamedSource<Arc<String>>>);

impl BodySource {
    /// The declaration belongs to the file being compiled; its span indexes
    /// into the ambient `src` threaded through the pipeline.
    #[must_use]
    pub const fn own() -> Self {
        Self(None)
    }

    /// The declaration was merged from a dependency body whose spans index
    /// into `src`.
    #[must_use]
    pub const fn dependency(src: NamedSource<Arc<String>>) -> Self {
        Self(Some(src))
    }

    /// Resolve the source the span should render against, falling back to the
    /// ambient `default` source for declarations native to the compiled file.
    #[must_use]
    pub fn resolve<'a>(
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
    pub fn or_dependency(self, dep_src: &NamedSource<Arc<String>>) -> Self {
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
    pub type_resolution_owner: crate::dag_id::DagId,
    pub expr: crate::hir::Expr,
    pub span: Span,
    /// Provenance of this declaration's `span` (#868). `None` means the span
    /// indexes into the IR's own file source; `Some` carries the source of a
    /// dependency body merged in by an instantiated include, so diagnostics
    /// anchored on the span render against the right file.
    pub src: BodySource,
}

/// A param declaration with type annotation and lowered default.
#[derive(Debug, Clone)]
pub struct ParamEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope used to resolve this parameter's type annotation and
    /// domain-bound expressions.
    pub type_resolution_owner: crate::dag_id::DagId,
    pub default_expr: Option<crate::hir::Expr>,
    pub span: Span,
    /// Source provenance of `span`; see [`ConstEntry::src`] (#868).
    pub src: BodySource,
}

/// A node declaration with type annotation and lowered body.
#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope used to resolve this node's type annotation and
    /// domain-bound expressions.
    pub type_resolution_owner: crate::dag_id::DagId,
    pub expr: crate::hir::Expr,
    pub span: Span,
    /// Source provenance of `span`; see [`ConstEntry::src`] (#868).
    pub src: BodySource,
}

/// An assert declaration with lowered body.
#[derive(Debug, Clone)]
pub struct AssertEntry {
    pub name: ScopedName,
    pub body: crate::hir::AssertBody,
    pub span: Span,
    /// Source provenance of `span`; see [`ConstEntry::src`] (#868).
    pub src: BodySource,
}

/// A const declaration awaiting body lowering at [`UnfrozenIR::freeze`].
///
/// Pre-freeze bodies stay syntactic so include instantiation can rewrite
/// reference paths (prefixing, index/type rebinding) before resolution.
#[derive(Debug, Clone)]
pub struct UnfrozenConstEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope for the declaration signature (type annotation and domain bounds).
    pub type_resolution_owner: crate::dag_id::DagId,
    pub expr: Expr,
    /// Module scope for the declaration body expression.
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    pub src: BodySource,
}

/// A param declaration awaiting default lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenParamEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope for the parameter signature (type annotation and domain bounds).
    pub type_resolution_owner: crate::dag_id::DagId,
    pub default_expr: Option<Expr>,
    /// Module scope for the default expression when present. Include-time
    /// param bindings are importer-written expressions, so this can differ
    /// from `type_resolution_owner`.
    pub default_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    pub src: BodySource,
}

/// A node declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenNodeEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    /// Module scope for the declaration signature (type annotation and domain bounds).
    pub type_resolution_owner: crate::dag_id::DagId,
    pub expr: Expr,
    /// Module scope for the declaration body expression.
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    pub src: BodySource,
}

/// An assert declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenAssertEntry {
    pub name: ScopedName,
    pub body: AssertBody,
    /// Module scope for the assertion body expression(s).
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
    /// Source provenance of `span`; see [`BodySource`] (#868).
    pub src: BodySource,
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
    pub span: Span,
    /// Whether this plot is `pub` (exported across the file boundary,
    /// requestable by include brace lists). Says nothing about display.
    pub is_pub: bool,
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
    /// The include item's span.
    pub span: Span,
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
    pub span: Span,
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
    pub span: Span,
}

/// A plot declaration awaiting body lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenPlotEntry {
    pub name: ScopedName,
    pub decl: PlotDecl,
    /// Module scope for plot field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
    /// Whether this plot is `pub` (visible in standalone output).
    pub is_pub: bool,
    /// Whether this plot renders standalone (no `#[hidden]`).
    pub displayed: bool,
}

/// A figure declaration awaiting field lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenFigureEntry {
    pub name: ScopedName,
    pub decl: FigureDecl,
    /// Module scope for figure field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
}

/// A layer declaration awaiting field lowering at [`UnfrozenIR::freeze`].
#[derive(Debug, Clone)]
pub struct UnfrozenLayerEntry {
    pub name: ScopedName,
    pub decl: LayerDecl,
    /// Module scope for layer field expressions.
    pub body_resolution_owner: crate::dag_id::DagId,
    pub span: Span,
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
    pub consts: Vec<ConstEntry>,
    /// Param declarations in source order.
    pub params: Vec<ParamEntry>,
    /// Node declarations in source order.
    pub nodes: Vec<NodeEntry>,
    /// Assert declarations in source order.
    pub asserts: Vec<AssertEntry>,
    /// Plot declarations in source order.
    pub plots: Vec<PlotEntry>,
    /// Figure declarations in source order.
    pub figures: Vec<FigureEntry>,
    /// Layer declarations in source order.
    pub layers: Vec<LayerEntry>,
    /// Plot aliases from include brace lists (#847).
    pub included_plots: Vec<IncludedPlotEntry>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// Set of all assert names.
    pub assert_names: HashSet<ScopedName>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub expected_fail: HashMap<ScopedName, ParsedExpectedFail>,
    /// Pre-evaluated values imported from dependency files.
    /// These are injected directly into the execution plan rather than compiled.
    /// Each entry carries the runtime value and its declared type (for `dim_check`).
    pub imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    /// Declared types for imported names that are not backed by a pre-evaluated
    /// value at this compilation boundary.
    ///
    /// Inline DAG bodies use this for `import parent.{const}`: the body needs
    /// the imported name's type during dim-checking, while the concrete value is
    /// supplied later by the caller or by the dependency that owns the DAG.
    pub imported_decl_types: HashMap<ScopedName, DeclaredType>,
    /// Source bindings for imported values whose runtime value is supplied
    /// outside this IR.
    pub imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    /// Names of declarations marked `pub` (or `pub(bind)`) in the file.
    ///
    /// Carried through from the resolver so downstream stages — most
    /// notably `preprocess_dag_body_self_imports` — can enforce
    /// visibility on `import <self>.{...}` items: a dag inside a file
    /// can only reach the file's `pub`-marked top-level declarations,
    /// matching the rules for cross-file imports. Implicit visibility
    /// (params are visible by default) is already baked in.
    pub pub_names: HashSet<DeclName>,
}

/// Runtime source of an imported value visible inside a DAG body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedValueSource {
    /// DAG that owns the original declaration.
    pub dag_id: crate::dag_id::DagId,
    /// Original declaration name in the owning DAG.
    pub source_name: DeclName,
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
/// The registry is frozen (via `build()`) before returning.
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
    resolved_ir.freeze(builder.build(), dag_id, &resolver, src)
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
pub(crate) fn lower_to_builder(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
    dag_id: &crate::dag_id::DagId,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Declaration collection
    let resolved = resolve_with_imports(ast, src, imported)?;

    // Step 2: Extract type annotations from AST + imported declarations.
    // Imported lists still carry flat-string names (a wider typing pass is
    // tracked separately); wrap them at the boundary so the map stays
    // DeclName-keyed.
    let mut type_anns = extract_type_annotations(ast);
    for (name, type_ann, _, _) in &imported.consts {
        type_anns.insert(DeclName::new(name.clone()), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.params {
        type_anns.insert(DeclName::new(name.clone()), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.nodes {
        type_anns.insert(DeclName::new(name.clone()), type_ann.clone());
    }

    // Step 3: Build registry, augment deps, and construct IR
    build_ir_from_resolved(
        ast,
        src,
        resolved,
        type_anns,
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        dag_id,
        None,
        None,
    )
}

/// Hook that merges imported type-system declarations into the registry builder.
///
/// Invoked after the prelude is loaded but before the file's own
/// declarations are registered, so local declarations (e.g. a `unit`
/// definition referencing an imported unit) resolve against the imported
/// entries.
pub type RegistrySeed<'a> = &'a mut dyn FnMut(&mut RegistryBuilder) -> Result<(), GraphcalError>;

/// Lower an AST with pre-evaluated imported values, returning a `RegistryBuilder`
/// that can be further mutated before freezing.
///
/// Unlike `lower_to_builder`, this uses `resolve_with_imported_values` which
/// only adds imported names to the scope (not their expressions). The actual
/// imported values are stored in `UnfrozenIR::imported_values` and injected
/// into the execution plan at runtime.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_to_builder_with_imported_values(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    let imported_decl_types = imported_values
        .iter()
        .map(|(name, (_value, ty))| (name.clone(), ty.clone()))
        .collect();
    lower_to_builder_with_imported_value_decls(
        ast,
        src,
        imported_names,
        imported_values,
        imported_decl_types,
        HashMap::new(),
        dag_id,
        registry_seed,
    )
}

/// Lower an AST with imported value names plus declared types for imports whose
/// runtime values will be supplied later.
///
/// This is used for inline DAG bodies that import a parent const. The resolver
/// needs the local imported name in scope, dim-checking needs its declared type,
/// and evaluation gets the concrete value from `imported_value_sources`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or registry construction fails.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "lowering threads imported value metadata plus the registry seed hook"
)]
pub fn lower_to_builder_with_imported_value_decls(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Declaration collection with imported value names in scope
    let resolved = resolve_with_imported_values(ast, src, imported_names)?;

    // Step 2: Extract type annotations from local declarations only
    let type_anns = extract_type_annotations(ast);

    // Step 3: Build registry, augment deps, and construct IR
    let (builder, mut unfrozen) = build_ir_from_resolved(
        ast,
        src,
        resolved,
        type_anns,
        imported_values,
        imported_decl_types,
        imported_value_sources,
        dag_id,
        None,
        registry_seed,
    )?;

    // Plot aliases from include brace lists become known to this DAG so
    // figures/layers can reference them (#847).
    unfrozen.included_plots = imported_names
        .plot_names
        .iter()
        .map(|(name, span)| IncludedPlotEntry {
            name: name.clone(),
            span: *span,
        })
        .collect();

    Ok((builder, unfrozen))
}

/// Lower a `dag { ... }` body as if it were a standalone file.
///
/// The dag body is a virtual [`File`] whose registry is seeded with the
/// enclosing file's frozen registry (dimensions, units, types, indexes, and
/// sibling dags) so that reference resolution and type checking behave exactly as
/// they would for a top-level declaration. Per Concept 9, the dag body cannot
/// implicitly reference the enclosing file's `const`/`param`/`node` values
/// — cross-scope values must be either passed in via the dag's own params or
/// brought into scope explicitly via `import <self>.{...}`.
///
/// The caller is responsible for pre-processing dag-body `import` declarations
/// (resolving self-imports to local names, classifying items against the
/// parent's value/type-system surface, recording source bindings) and passing
/// in:
///
/// - `stripped_body`: the dag body with self-import declarations removed.
///   Cross-file imports inside dag bodies (if any) are still left for the
///   downstream resolver to handle through the regular import machinery.
/// - `imported_names`: the resolver scope contribution from preprocessed
///   self-imports.
/// - `imported_decl_types`: per-name declared types for those self-imports.
/// - `imported_value_sources`: per-name source bindings for those
///   self-imports — recording that the value comes from the parent DAG at
///   runtime.
///
/// The returned `IR` has a `dag_id` formed by appending `dag_name` to
/// `parent_dag_id`, so nested-scope diagnostics have a stable source location.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if declaration collection or type-system construction
/// fails for the dag body.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "dag-module lowering threads pre-processed import metadata + optional parent registry"
)]
pub fn lower_dag_module_to_builder_with_imported_value_decls(
    dag_body: &File,
    parent_registry: Option<&Registry>,
    imported_names: &ImportedValueNames,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
    registry_seed: Option<RegistrySeed<'_>>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    let resolved = resolve_with_imported_values(dag_body, src, imported_names)?;
    let type_anns = extract_type_annotations(dag_body);

    build_ir_from_resolved(
        dag_body,
        src,
        resolved,
        type_anns,
        imported_values,
        imported_decl_types,
        imported_value_sources,
        dag_id,
        parent_registry,
        registry_seed,
    )
}

#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "dag-body lowering threads pre-processed import metadata + parent registry"
)]
pub fn lower_dag_body_to_ir(
    dag_name: &str,
    stripped_body: &[crate::desugar::desugared_ast::Declaration],
    parent_registry: &Registry,
    resolver: &crate::syntax::module_resolve::ModuleResolver,
    imported_names: &ImportedValueNames,
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::dag_id::DagId,
) -> Result<IR, GraphcalError> {
    let virtual_file = File {
        declarations: stripped_body.to_vec(),
    };
    let dag_dag_id = parent_dag_id.child(dag_name);
    let (builder, unfrozen) = lower_dag_module_to_builder_with_imported_value_decls(
        &virtual_file,
        Some(parent_registry),
        imported_names,
        HashMap::new(),
        imported_decl_types,
        imported_value_sources,
        src,
        &dag_dag_id,
        None,
    )?;
    unfrozen.freeze(builder.build(), &dag_dag_id, resolver, src)
}

/// Result of `preprocess_dag_body_self_imports`: imported names, declared
/// types, source bindings, and the body with self-import declarations stripped.
pub struct DagBodySelfImports {
    pub names: ImportedValueNames,
    pub decl_types: HashMap<ScopedName, DeclaredType>,
    pub value_sources: HashMap<ScopedName, ImportedValueSource>,
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

fn scoped_name_to_name_path(
    name: &ScopedName,
    src: &NamedSource<Arc<String>>,
) -> Result<NamePath, GraphcalError> {
    let span = Span::new(0, 0);
    let qualifier = name
        .qualifier()
        .iter()
        .map(|segment| {
            NameAtom::parse(segment.as_ref()).map_err(|err| GraphcalError::InternalError {
                message: format!("invalid scoped-name segment `{segment}`: {err}"),
                src: src.clone(),
                span: span.into(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let leaf = NameAtom::parse(name.member()).map_err(|err| GraphcalError::InternalError {
        message: format!("invalid scoped-name member `{}`: {err}", name.member()),
        src: src.clone(),
        span: span.into(),
    })?;
    Ok(if qualifier.is_empty() {
        NamePath::local(leaf)
    } else {
        NamePath::qualified_path(qualifier, leaf)
    })
}

/// Shared implementation for `lower_to_builder` and `lower_to_builder_with_imported_values`.
///
/// Builds the registry, augments runtime deps for dynamic units, pairs resolved
/// declarations with type annotations, and constructs the `UnfrozenIR`.
#[expect(
    clippy::too_many_lines,
    reason = "single linear pipeline — splitting would obscure the flow"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "IR construction threads imported value type/source metadata"
)]
fn build_ir_from_resolved(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    resolved: ResolvedFile,
    mut type_anns: HashMap<DeclName, TypeExpr>,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    dag_id: &crate::dag_id::DagId,
    parent_registry: Option<&Registry>,
    registry_seed: Option<RegistrySeed<'_>>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
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

    // Pair resolved declarations with type annotations. The resolved entries
    // still carry flat-string names (a wider typing pass is tracked separately);
    // wrap each into a `DeclName` once so both `take_type_ann` and the
    // `ScopedName::from` lift see the typed form.
    let consts = resolved
        .consts
        .into_iter()
        .map(|entry| {
            let decl_name = DeclName::new(entry.name);
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
    let params = resolved
        .params
        .into_iter()
        .map(|entry| {
            let decl_name = DeclName::new(entry.name);
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
    let nodes = resolved
        .nodes
        .into_iter()
        .map(|entry| {
            let decl_name = DeclName::new(entry.name);
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
                name: ScopedName::local(entry.name),
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
                let is_pub = resolved.pub_names.contains(entry.name.as_str());
                let displayed = !resolved.hidden_plots.contains(entry.name.as_str());
                UnfrozenPlotEntry {
                    name: ScopedName::local(entry.name),
                    decl: entry.decl,
                    body_resolution_owner: dag_id.clone(),
                    span: entry.span,
                    is_pub,
                    displayed,
                }
            })
            .collect(),
        figures: resolved
            .figures
            .into_iter()
            .map(|entry| UnfrozenFigureEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
            })
            .collect(),
        layers: resolved
            .layers
            .into_iter()
            .map(|entry| UnfrozenLayerEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                body_resolution_owner: dag_id.clone(),
                span: entry.span,
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
        imported_values,
        imported_decl_types,
        imported_value_sources,
        pub_names: resolved.pub_names,
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
    pub included_plots: Vec<IncludedPlotEntry>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    assert_names: HashSet<ScopedName>,
    // Key-lookup only, order irrelevant.
    assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    // Key-lookup only, order irrelevant.
    expected_fail: HashMap<ScopedName, ParsedExpectedFail>,
    // Key-lookup only, order irrelevant.
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    // Key-lookup only, order irrelevant.
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    // Key-lookup only, order irrelevant.
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    // Names of declarations marked `pub`/`pub(bind)` (plus implicit-pub
    // params). Used by `preprocess_dag_body_self_imports` to enforce
    // visibility on dag-body `import <self>.{...}` items.
    pub_names: HashSet<DeclName>,
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
    #[expect(
        clippy::too_many_lines,
        reason = "single lowering boundary over every declaration kind"
    )]
    pub fn freeze(
        self,
        registry: Registry,
        owner: &crate::dag_id::DagId,
        resolver: &crate::syntax::module_resolve::ModuleResolver,
        src: &NamedSource<Arc<String>>,
    ) -> Result<IR, GraphcalError> {
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
            let canonical =
                crate::hir::diagnostics::resolved_decl_key(owner, name).ok_or_else(|| {
                    GraphcalError::InternalError {
                        message: format!("could not build canonical declaration key for `{name}`"),
                        src: src.clone(),
                        span: Span::new(0, 0).into(),
                    }
                })?;
            decl_bindings.insert(name.clone(), canonical);
        }
        for name in self.imported_values.keys() {
            let path = scoped_name_to_name_path(name, src)?;
            let canonical = resolver
                .resolve_decl_path(owner, &path)
                .unwrap_or_else(|_| {
                    crate::hir::diagnostics::resolved_decl_key(owner, name).unwrap_or_else(|| {
                        crate::syntax::names::ResolvedName::from_def(
                            owner.clone(),
                            DeclName::new(name.member()),
                        )
                    })
                });
            decl_bindings.insert(name.clone(), canonical);
        }
        for (name, source) in &self.imported_value_sources {
            decl_bindings.insert(
                name.clone(),
                crate::syntax::names::ResolvedName::from_def(
                    source.dag_id.clone(),
                    source.source_name.clone(),
                ),
            );
        }

        let generic_scope = crate::hir::GenericScope::new();
        let prelude = crate::hir::PreludeTypeScope::graphcal();
        // A merged dependency body keeps the dependency file's byte offsets, so
        // a lowering error must render against that body's own source rather
        // than the importer's `src` (#868); `BodySource::resolve` selects it.
        let lower_in = |expr: &Expr,
                        resolution_owner: &crate::dag_id::DagId,
                        body_src: &NamedSource<Arc<String>>| {
            let expr_ctx =
                crate::hir::ExprLoweringContext::new(resolution_owner, resolver, &generic_scope)
                    .with_prelude(&prelude)
                    .with_decl_bindings(&decl_bindings);
            crate::hir::lower_expr(expr, expr_ctx).map_err(|err| {
                crate::hir::diagnostics::expr_lower_error_to_graphcal(&err, body_src)
            })
        };

        let consts = self
            .consts
            .iter()
            .map(|entry| {
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
        let params = self
            .params
            .iter()
            .map(|entry| {
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
        let nodes = self
            .nodes
            .iter()
            .map(|entry| {
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
        let asserts = self
            .asserts
            .iter()
            .map(|entry| {
                let body_src = entry.src.resolve(src);
                Ok(AssertEntry {
                    name: entry.name.clone(),
                    body: {
                        let expr_ctx = crate::hir::ExprLoweringContext::new(
                            &entry.body_resolution_owner,
                            resolver,
                            &generic_scope,
                        )
                        .with_prelude(&prelude)
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
            let expr_ctx =
                crate::hir::ExprLoweringContext::new(resolution_owner, resolver, &generic_scope)
                    .with_prelude(&prelude)
                    .with_decl_bindings(&decl_bindings);
            crate::hir::lower_expr(expr, expr_ctx).ok()
        };
        let plots = self
            .plots
            .iter()
            .map(|entry| {
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
                PlotEntry {
                    name: entry.name.clone(),
                    mark_type: entry.decl.mark.mark_type,
                    body: complete.then_some(body),
                    span: entry.span,
                    is_pub: entry.is_pub,
                    displayed: entry.displayed,
                }
            })
            .collect();
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
        let figures = self
            .figures
            .iter()
            .map(|entry| FigureEntry {
                name: entry.name.clone(),
                plot_names: entry.decl.plot_names.clone(),
                fields: lower_fields(&entry.decl.fields, &entry.body_resolution_owner),
                span: entry.span,
            })
            .collect();
        let layers = self
            .layers
            .iter()
            .map(|entry| LayerEntry {
                name: entry.name.clone(),
                plot_names: entry.decl.plot_names.clone(),
                fields: lower_fields(&entry.decl.fields, &entry.body_resolution_owner),
                span: entry.span,
            })
            .collect();

        Ok(IR {
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
            assert_names: self.assert_names,
            assumes_map: self.assumes_map,
            expected_fail: self.expected_fail,
            imported_values: self.imported_values,
            imported_decl_types: self.imported_decl_types,
            imported_value_sources: self.imported_value_sources,
            pub_names: self.pub_names,
        })
    }

    /// Replace a param's default expression with an override.
    ///
    /// Returns `false` when no param entry with that leaf name exists.
    pub fn override_param_default(&mut self, name: &str, expr: Expr) -> bool {
        match self
            .params
            .iter_mut()
            .find(|entry| entry.name.member() == name)
        {
            Some(entry) => {
                entry.default_expr = Some(expr);
                true
            }
            None => false,
        }
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
        index_bindings: &HashMap<IndexName, IndexName>,
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
                orphan_decl: param.name.member(),
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
    /// [`BodySource::or_dependency`]) for diagnostics raised at the importer's
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
        prefix: &str,
        bindings: &HashMap<DeclName, Expr>,
        dep_names: &HashSet<DeclName>,
        index_bindings: &HashMap<IndexName, IndexName>,
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
        /// Mirrors [`RefPrefixer::rewrite`]: already-qualified names (e.g. a
        /// transitively-imported `module.x` inside the dep) belong to another
        /// namespace and must keep their qualifier — `with_prefix` would
        /// silently replace it, diverging from the merged expressions whose
        /// qualified refs are left untouched.
        fn prefix_dep(d: &ScopedName, prefix: &str, dep_names: &HashSet<DeclName>) -> ScopedName {
            if !d.is_qualified() && dep_names.contains(d.member()) {
                d.with_prefix(prefix)
            } else {
                d.clone()
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

        let mut all_dep_names = dep_names.clone();
        all_dep_names.extend(
            dep.imported_values
                .keys()
                .map(|name| DeclName::new(name.member())),
        );
        all_dep_names.extend(
            dep.imported_decl_types
                .keys()
                .map(|name| DeclName::new(name.member())),
        );
        all_dep_names.extend(
            dep.imported_value_sources
                .keys()
                .map(|name| DeclName::new(name.member())),
        );
        let dep_names = &all_dep_names;

        // Merge consts
        for mut entry in dep.consts {
            substitute_index_names(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names);
            substitute_type_expr_index_names(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            let prefixed = entry.name.with_prefix(prefix);
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
            let prefixed = entry.name.with_prefix(prefix);
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
                    // Keep default, but substitute index names and prefix internal refs
                    substitute_index_names(expr, index_bindings);
                    substitute_type_names_in_expr(expr, type_bindings);
                    prefix_expr_refs(expr, prefix, dep_names);
                    merge_resolution_owner(entry.default_resolution_owner)
                } else {
                    // Required param without binding — stays None, caught later in exec_plan
                    merge_resolution_owner(entry.default_resolution_owner)
                };
            substitute_type_expr_index_names(&mut entry.type_ann, index_bindings);
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
            substitute_index_names(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names);
            substitute_type_expr_index_names(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            let prefixed = entry.name.with_prefix(prefix);
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
                    substitute_index_names(e, index_bindings);
                    substitute_type_names_in_expr(e, type_bindings);
                    prefix_expr_refs(e, prefix, dep_names);
                }
                crate::desugar::desugared_ast::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    ..
                } => {
                    substitute_index_names(actual, index_bindings);
                    substitute_type_names_in_expr(actual, type_bindings);
                    prefix_expr_refs(actual, prefix, dep_names);
                    substitute_index_names(expected, index_bindings);
                    substitute_type_names_in_expr(expected, type_bindings);
                    prefix_expr_refs(expected, prefix, dep_names);
                    substitute_index_names(tolerance, index_bindings);
                    substitute_type_names_in_expr(tolerance, type_bindings);
                    prefix_expr_refs(tolerance, prefix, dep_names);
                }
            }
            let prefixed = entry.name.with_prefix(prefix);
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
                substitute_index_names(&mut encoding.value, index_bindings);
                substitute_type_names_in_expr(&mut encoding.value, type_bindings);
                prefix_expr_refs(&mut encoding.value, prefix, dep_names);
            }
            for prop in &mut entry.decl.mark.properties {
                substitute_index_names(&mut prop.value, index_bindings);
                substitute_type_names_in_expr(&mut prop.value, type_bindings);
                prefix_expr_refs(&mut prop.value, prefix, dep_names);
            }
            for prop in &mut entry.decl.properties {
                substitute_index_names(&mut prop.value, index_bindings);
                substitute_type_names_in_expr(&mut prop.value, type_bindings);
                prefix_expr_refs(&mut prop.value, prefix, dep_names);
            }
            let local = ScopedName::local(requested.alias.as_str());
            self.plots.push(UnfrozenPlotEntry {
                name: local.clone(),
                decl: entry.decl,
                body_resolution_owner: merge_resolution_owner(entry.body_resolution_owner),
                span: entry.span,
                // The alias is root-local; re-export requires its own `pub`
                // include item, resolved at the import surface.
                is_pub: false,
                displayed: !requested.hidden,
            });
            self.source_order.push((local, DeclCategory::Plot));
        }

        // Dep figures and layers do not merge: they cannot be requested by a
        // brace list, and display is controlled by the consumer (#847).

        // Merge assumes_map and expected_fail
        for (assert_name, assumers) in dep.assumes_map {
            let prefixed_assert = assert_name.with_prefix(prefix);
            let prefixed_assumers: Vec<ScopedName> =
                assumers.iter().map(|a| a.with_prefix(prefix)).collect();
            self.assumes_map
                .entry(prefixed_assert)
                .or_default()
                .extend(prefixed_assumers);
        }
        for (assert_name, ef) in dep.expected_fail {
            let prefixed = assert_name.with_prefix(prefix);

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
                                // `#N` range segments never name an index, so
                                // they cannot reference an overridden one.
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
                    let prefixed_assert = ScopedName::local(orig_name.as_str()).with_prefix(prefix);
                    let ef = crate::ir::resolve::names::parse_expected_fail_args(
                        &attr.args,
                        importer_src,
                    )?;
                    self.expected_fail.insert(prefixed_assert, ef);
                }
            }
        }

        // Propagate the dep's imported-value metadata. Hidden imports used by
        // the dep's expressions are instance-scoped together with the merged
        // expressions, preventing two DAG include instances from sharing an
        // unqualified synthetic name.
        for (name, value) in dep.imported_values {
            self.imported_values
                .entry(prefix_dep(&name, prefix, dep_names))
                .or_insert(value);
        }
        for (name, dt) in dep.imported_decl_types {
            self.imported_decl_types
                .entry(prefix_dep(&name, prefix, dep_names))
                .or_insert(dt);
        }
        for (name, source) in dep.imported_value_sources {
            self.imported_value_sources
                .entry(prefix_dep(&name, prefix, dep_names))
                .or_insert(source);
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
    index_bindings: &'a HashMap<IndexName, IndexName>,
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
            TypeExprKind::TypeApplication { name, type_args } => {
                if let Some(atom) = name.value.as_bare()
                    && self.type_bindings.contains_key(atom.as_str())
                {
                    return Err(self.orphan_error(
                        "type",
                        atom.as_str(),
                        format!("type `{}`", name.value),
                    ));
                }
                for arg in type_args {
                    self.check_type_expr(arg)?;
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
        if let ExprKind::FnCall { type_args, .. } = &expr.kind {
            for ga in type_args {
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
/// dependency being merged, rewrite the typed [`ScopedName`] payload via
/// [`ScopedName::with_prefix`] so the merged-IR key matches the prefixed
/// declaration name. No flat separator strings are constructed here — the
/// local/qualified distinction lives in the structured qualifier path.
struct RefPrefixer<'a> {
    prefix: &'a str,
    prefix_atom: NameAtom,
    dep_names: &'a HashSet<DeclName>,
}

impl RefPrefixer<'_> {
    fn rewrite(&self, scoped: &ScopedName) -> Option<ScopedName> {
        // Only rewrite refs that are local to the dep (i.e. unqualified
        // members owned by the dependency). Already-qualified refs (e.g.
        // a transitively-imported `@module.x` inside the dep) belong to
        // some other namespace and are left untouched.
        if !scoped.is_qualified() && self.dep_names.contains(scoped.member()) {
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
                name: self.prefix_atom.clone(),
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
pub(crate) fn prefix_expr_refs(expr: &mut Expr, prefix: &str, dep_names: &HashSet<DeclName>) {
    let Ok(prefix_atom) = NameAtom::parse(prefix) else {
        // The prefix comes from a validated include alias; a non-identifier
        // prefix cannot name any reference, so there is nothing to rewrite.
        return;
    };
    let mut prefixer = RefPrefixer {
        prefix,
        prefix_atom,
        dep_names,
    };
    let _ = prefixer.visit_expr_mut(expr);
}

/// Visitor that rewrites index names in expressions according to a binding map.
///
/// Overrides the per-variant handler methods for nodes that carry index name
/// fields (`VariantLiteral`, `ForComp`, `IndexAccess`, `MapLiteral`,
/// `TableLiteral`, `Match`) to rewrite those names before recursing into
/// child expressions.
struct IndexSubstituter<'a> {
    bindings: &'a HashMap<IndexName, IndexName>,
}

impl ExprVisitorMut<crate::syntax::phase::Desugared> for IndexSubstituter<'_> {
    type Error = std::convert::Infallible;

    fn visit_unresolved_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        // A two-segment path whose head names a rebound index is a variant
        // literal of that index (`Phase.Burn`); rewrite the head segment so
        // the literal points at the importer's index.
        if let ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) =
            &mut expr.kind
            && let [head, _variant] = path.segments.as_mut_slice()
            && let Some(new) = self.bindings.get(head.name.as_str())
            && let Ok(new_atom) = NameAtom::parse(new.as_str())
        {
            head.name = new_atom;
        }
        Ok(())
    }

    fn visit_for_comp_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::ForComp { bindings, body } = &mut expr.kind {
            for b in bindings {
                if let crate::desugar::desugared_ast::ForBindingIndex::Named(ref mut spanned_idx) =
                    b.index
                    && let Some(new) = self.bindings.get(spanned_idx.value.leaf().as_str())
                {
                    spanned_idx.value = new.clone().into();
                }
            }
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
                        if let Some(new) = self.bindings.get(index.value.leaf().as_str()) {
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
                        && let Some(new) = self.bindings.get(index_name.leaf().as_str())
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
                        if let Some(new) = self.bindings.get(index.value.leaf().as_str()) {
                            index.value = new.clone().into();
                        }
                    }
                    // A two-segment path pattern whose head names a rebound
                    // index is an index-label pattern; rewrite the head.
                    crate::desugar::desugared_ast::MatchPattern::Path { path, .. } => {
                        if let [head, _variant] = path.segments.as_mut_slice()
                            && let Some(new) = self.bindings.get(head.name.as_str())
                            && let Ok(new_atom) = NameAtom::parse(new.as_str())
                        {
                            head.name = new_atom;
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
pub(crate) fn substitute_index_names(expr: &mut Expr, bindings: &HashMap<IndexName, IndexName>) {
    if bindings.is_empty() {
        return;
    }
    let mut sub = IndexSubstituter { bindings };
    let _ = sub.visit_expr_mut(expr);
}

/// Rewrite index names within a type expression according to a binding map.
///
/// `TypeExpr` is not part of the `Expr` tree, so it needs a separate
/// substitution pass. This rewrites index identifiers in `Indexed` types
/// (e.g., `Dimensionless[Phase]` → `Dimensionless[MyPhase]`) and recurses
/// into `TypeApplication` arguments.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn substitute_type_expr_index_names(
    type_expr: &mut TypeExpr,
    bindings: &HashMap<IndexName, IndexName>,
) {
    use crate::desugar::desugared_ast::TypeExprKind;

    if bindings.is_empty() {
        return;
    }
    match &mut type_expr.kind {
        TypeExprKind::Indexed { base, indexes } => {
            for idx_expr in indexes.iter_mut() {
                if let crate::desugar::desugared_ast::IndexExpr::Name(path) = idx_expr
                    && let Some(atom) = path.value.as_bare()
                    && let Some(new_name) = bindings.get(atom.as_str())
                {
                    path.value = crate::syntax::names::NamePath::from(new_name.as_str());
                }
            }
            substitute_type_expr_index_names(base, bindings);
        }
        TypeExprKind::TypeApplication { type_args, .. }
        | TypeExprKind::DatetimeApplication { type_args } => {
            for arg in type_args {
                substitute_type_expr_index_names(arg, bindings);
            }
        }
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => {}
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
        TypeExprKind::DimExpr(dim_expr) => {
            for item in &mut dim_expr.terms {
                if let Some(atom) = item.term.name.value.as_bare()
                    && let Some(new_name) = bindings.get(atom.as_str())
                {
                    item.term.name.value = crate::syntax::names::NamePath::from(new_name.as_ref());
                }
            }
        }
        TypeExprKind::Indexed { base, .. } => {
            substitute_type_expr_nominal_names(base, bindings);
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            if let Some(atom) = name.value.as_bare()
                && let Some(new_name) = bindings.get(atom.as_str())
            {
                name.value = crate::syntax::names::NamePath::from(new_name.as_ref());
            }
            for arg in type_args {
                substitute_type_expr_nominal_names(arg, bindings);
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
/// Covers `ConstructorCall.constructor`, `ConstructorCall.generic_args`,
/// and `FnCall.type_args`. Recurses through child expressions so nested
/// constructor calls are also rewritten.
#[expect(
    clippy::too_many_lines,
    reason = "single recursion covering every ExprKind variant"
)]
pub(crate) fn substitute_type_names_in_expr(
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
            type_args, args, ..
        } => {
            for ga in type_args.iter_mut() {
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
pub(crate) fn register_file_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
) -> Result<(), GraphcalError> {
    register_declarations_impl(file, registry, src, None, dag_id)
}

/// Names selected from a dependency's type-system registry.
///
/// The sets span several namespaces by design (dims, units, indexes, and
/// types share the selective-import surface), so entries are kept as the
/// namespace-agnostic [`NameAtom`] rather than coerced into one name type.
#[derive(Debug, Default, Clone)]
pub struct SelectedDeclarations {
    /// Names imported from the default compile-time namespace.
    pub default: HashSet<crate::syntax::names::NameAtom>,
    /// Names imported from the explicit `type` namespace.
    pub types: HashSet<crate::syntax::names::NameAtom>,
}

impl SelectedDeclarations {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.default.is_empty() && self.types.is_empty()
    }

    pub fn insert_default(&mut self, name: impl Into<crate::syntax::names::NameAtom>) {
        self.default.insert(name.into());
    }

    pub fn insert_type(&mut self, name: impl Into<crate::syntax::names::NameAtom>) {
        self.types.insert(name.into());
    }
}

/// Register only the named type-system declarations (dimensions, units, indexes, types)
/// from a file into the registry.
///
/// This is the selective counterpart to `register_file_declarations`: instead of
/// registering everything, it filters default-namespace declarations and type
/// declarations independently.
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
/// 3. Required-range indexes (depend only on dimensions)
/// 4. Units (topologically sorted by inter-dependency)
/// 5. Range indexes (depend on dimensions and units)
///
/// When `filter` is `None`, all declarations are registered.
/// When `filter` is `Some(names)`, default-namespace declarations and type
/// declarations are filtered independently.
fn register_declarations_impl(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    filter: Option<&SelectedDeclarations>,
    dag_id: &crate::dag_id::DagId,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::{DimDecl, IndexDecl, UnitDecl};

    let should_register_default =
        |name: &str| filter.is_none_or(|names| names.default.contains(name));
    let should_register_type = |name: &str| filter.is_none_or(|names| names.types.contains(name));

    // Collect declarations by kind for phased registration.
    let mut derived_dims: Vec<&DimDecl> = Vec::new();
    let mut units: Vec<&UnitDecl> = Vec::new();
    let mut required_range_indexes: Vec<(&IndexDecl, Span)> = Vec::new();
    let mut range_indexes: Vec<(&IndexDecl, Span)> = Vec::new();

    // Phase 1: Register base dimensions, types, union types, named/required-named indexes.
    // Also collect derived dims, units, and dependent indexes for later phases.
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::BaseDimension(d) if should_register_default(d.name.value.as_str()) => {
                register_base_dimension_decl(d, registry, dag_id);
            }
            DeclKind::Dimension(d) if should_register_default(d.name.value.as_str()) => {
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
            DeclKind::Unit(u) if should_register_default(u.name.value.as_str()) => {
                units.push(u);
            }
            DeclKind::Index(idx) if should_register_default(idx.name.value.as_str()) => {
                match &idx.kind {
                    IndexDeclKind::RequiredRange { .. } => {
                        required_range_indexes.push((idx, decl.span));
                    }
                    IndexDeclKind::Range { .. } => {
                        range_indexes.push((idx, decl.span));
                    }
                    IndexDeclKind::Named { .. } | IndexDeclKind::RequiredNamed => {
                        register_index_decl(idx, registry, src, decl.span)?;
                    }
                }
            }
            DeclKind::Type(t) if should_register_type(t.name.value.as_str()) => {
                register_type_decl(t, registry);
            }
            DeclKind::Dag(d) if should_register_default(d.name.value.as_str()) => {
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

    // Phase 3: Register required-range indexes (depend only on dimensions).
    for (idx, span) in &required_range_indexes {
        register_index_decl(idx, registry, src, *span)?;
    }

    // Phase 4: Topologically sort and register units.
    if !units.is_empty() {
        let sorted = topo_sort_units(&units, src)?;
        for u in sorted {
            register_unit_decl(u, registry, src)?;
        }
    }

    // Phase 5: Register range indexes (depend on dimensions and units).
    for (idx, span) in &range_indexes {
        register_index_decl(idx, registry, src, *span)?;
    }

    // Phase 6: Register synthetic nat range indexes for any integer literals
    // appearing in type position (e.g., `param A: Length[3, 4]`) or
    // for-range expressions (e.g., `for i: range(3) { ... }`).
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Param(d) => {
                collect_nat_ranges_from_type_expr(&d.type_ann, registry, src)?;
                if let Some(ref value) = d.value {
                    collect_nat_ranges_from_expr(value, registry, src)?;
                }
            }
            DeclKind::Node(d) => {
                collect_nat_ranges_from_type_expr(&d.type_ann, registry, src)?;
                collect_nat_ranges_from_expr(&d.value, registry, src)?;
            }
            DeclKind::ConstNode(d) => {
                collect_nat_ranges_from_type_expr(&d.type_ann, registry, src)?;
                collect_nat_ranges_from_expr(&d.value, registry, src)?;
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
            name: DimName::new(cycle_name),
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
    let dim_id = crate::syntax::dimension::BaseDimId::UserDefined {
        dag: dag_id.clone(),
        name: d.name.value.to_string(),
    };
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
        .resolve_dim_expr(definition)
        .map_err(|_| GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: d.name.span.into(),
        })?
        .ok_or_else(|| GraphcalError::UnknownDimension {
            name: d.name.value.clone(),
            src: src.clone(),
            span: d.name.span.into(),
        })?;
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
    let dim_id = crate::syntax::dimension::BaseDimId::UserDefined {
        dag: dag_id.clone(),
        name: d.name.value.to_string(),
    };
    registry.register_base_dimension(d.name.value.clone(), dim_id);
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
        .resolve_dim_expr(&u.dim_type)
        .map_err(|_| GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: u.name.span.into(),
        })?
        .ok_or_else(|| GraphcalError::UnknownDimension {
            name: DimName::new(u.name.value.as_str()),
            src: src.clone(),
            span: u.name.span.into(),
        })?;
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
        if contains_graph_ref(&def.scale_expr) {
            // Dynamic unit: scale depends on runtime values (e.g., `(@rate) USD`).
            // Resolve the base unit's dimension and static scale factor.
            let base_scale = resolve_base_unit_static_scale(registry, &def.unit_expr, src)?;
            UnitScale::Dynamic {
                scale_expr: def.scale_expr.clone(),
                base_unit_scale: base_scale,
            }
        } else {
            // Static scale value. A plain `unit` with no `@` still remains a
            // runtime unit for const-context policy; `const unit` is the
            // surface marker that makes it available to `const node`.
            let (_unit_dim, base_scale) = registry
                .resolve_unit_expr(&def.unit_expr)
                .map_err(|err| unit_resolve_to_graphcal(err, src, def.span))?;
            let scale_expr = validate_positive_finite_scale(
                eval_scale_expr(&def.scale_expr, src)?,
                "unit scale expression",
                src,
                def.scale_expr.span,
            )?;
            let base_scale = validate_positive_finite_scale(
                base_scale,
                "base unit scale",
                src,
                def.unit_expr.span,
            )?;
            let scale =
                multiply_positive_scales(scale_expr, base_scale, "unit scale", src, def.span)?;
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
) -> Option<&'a Spanned<crate::syntax::names::UnitRef>> {
    unit_expr.terms.iter().find_map(|term| {
        registry
            .get_unit(&term.name.value)
            .is_some_and(|info| !info.constness.is_const())
            .then_some(&term.name)
    })
}

/// Resolve the static scale factor of the base unit expression in a unit definition.
///
/// For `unit EUR: Money = (@rate) USD;`, the base unit expr is `USD` with scale 1.0.
/// The base unit itself must be static (not dynamic).
fn resolve_base_unit_static_scale(
    registry: &RegistryBuilder,
    unit_expr: &crate::desugar::desugared_ast::UnitExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<PositiveFiniteScale, GraphcalError> {
    let (_dim, base_scale) = registry
        .resolve_unit_expr(unit_expr)
        .map_err(|err| unit_resolve_to_graphcal(err, src, unit_expr.span))?;
    validate_positive_finite_scale(base_scale, "base unit scale", src, unit_expr.span)
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

/// Convert an AST-level `u64` nat literal to the `usize` size the registry
/// stores, raising a graceful runtime error if the value doesn't fit in
/// `usize` on the current target (e.g., a > 4G literal on a 32-bit build).
fn nat_size_to_usize(
    n: u64,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<NonZeroUsize, GraphcalError> {
    let size = usize::try_from(n).map_err(|_| GraphcalError::EvalError {
        message: format!("nat range size {n} does not fit in usize on this target"),
        src: src.clone(),
        span: span.into(),
    })?;
    NonZeroUsize::new(size).ok_or_else(|| {
        eval_error(
            "range(0) is not allowed; indexes must contain at least one element",
            src,
            span,
        )
    })
}

/// Recursively scan a type expression for nat literals in index position
/// and register the corresponding synthetic nat range indexes in the registry.
fn collect_nat_ranges_from_type_expr(
    type_expr: &crate::desugar::desugared_ast::TypeExpr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if let crate::desugar::desugared_ast::TypeExprKind::Indexed { base, indexes } = &type_expr.kind
    {
        collect_nat_ranges_from_type_expr(base, registry, src)?;
        for idx in indexes {
            match idx {
                crate::desugar::desugared_ast::IndexExpr::NatExpr(nat_expr) => {
                    collect_nat_range_literals_from_nat_expr(nat_expr, registry, src)?;
                }
                crate::desugar::desugared_ast::IndexExpr::Name(_) => {}
            }
        }
    }
    if let crate::desugar::desugared_ast::TypeExprKind::TypeApplication { type_args, .. }
    | crate::desugar::desugared_ast::TypeExprKind::DatetimeApplication { type_args } =
        &type_expr.kind
    {
        for arg in type_args {
            collect_nat_ranges_from_type_expr(arg, registry, src)?;
        }
    }
    Ok(())
}

/// Collect nat range literal values from a `NatExpr` tree.
///
/// Only literal-only expressions can be registered at compile time;
/// expressions containing variables are resolved at call sites.
fn collect_nat_range_literals_from_nat_expr(
    expr: &crate::desugar::desugared_ast::NatExpr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::NatExpr;
    match expr {
        NatExpr::Literal(n, span) => {
            let size = nat_size_to_usize(*n, *span, src)?;
            registry.ensure_nat_range_index(size);
        }
        NatExpr::Var(_) => {}
        NatExpr::Add(lhs, rhs, _) | NatExpr::Mul(lhs, rhs, _) => {
            collect_nat_range_literals_from_nat_expr(lhs, registry, src)?;
            collect_nat_range_literals_from_nat_expr(rhs, registry, src)?;
        }
    }
    Ok(())
}

/// Recursively scan an expression for `for i: range(N)` and register
/// nat range indexes for concrete nat literals.
fn collect_nat_ranges_from_expr(
    expr: &crate::desugar::desugared_ast::Expr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::{ExprKind, ForBindingIndex};

    // Use the visitor trait to walk all sub-expressions
    struct NatRangeCollector<'a> {
        registry: &'a mut RegistryBuilder,
        src: &'a NamedSource<Arc<String>>,
    }

    impl crate::syntax::visitor::ExprVisitor<crate::syntax::phase::Desugared>
        for NatRangeCollector<'_>
    {
        type Error = GraphcalError;

        fn visit_expr(
            &mut self,
            expr: &crate::desugar::desugared_ast::Expr,
        ) -> Result<(), GraphcalError> {
            match &expr.kind {
                ExprKind::ForComp { bindings, .. } => {
                    for binding in bindings {
                        if let ForBindingIndex::Range { arg, .. } = &binding.index {
                            collect_nat_range_literals_from_nat_expr(arg, self.registry, self.src)?;
                        }
                    }
                }
                ExprKind::MapLiteral { entries } => {
                    for entry in entries {
                        for key in &entry.keys {
                            if let crate::syntax::ast::MapEntryIndex::NatRange(n) = &key.index.value
                            {
                                let size = nat_size_to_usize(*n, key.index.span, self.src)?;
                                self.registry.ensure_nat_range_index(size);
                            }
                        }
                    }
                }
                _ => {}
            }
            self.dispatch(expr)
        }
    }

    let mut collector = NatRangeCollector { registry, src };
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
        crate::desugar::desugared_ast::IndexDeclKind::RequiredNamed => {
            types::IndexKind::RequiredNamed
        }
        crate::desugar::desugared_ast::IndexDeclKind::RequiredRange { dimension } => {
            let dim = registry
                .resolve_dim_expr(dimension)
                .map_err(|_| GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: dimension.span.into(),
                })?
                .ok_or_else(|| GraphcalError::UnknownDimension {
                    name: crate::syntax::names::DimName::new(idx.name.value.as_str()),
                    src: src.clone(),
                    span: dimension.span.into(),
                })?;
            types::IndexKind::RequiredRange { dimension: dim }
        }
    };
    registry.register_index(types::IndexDef {
        name: idx.name.value.clone(),
        kind,
    });
    Ok(())
}

fn register_type_decl(t: &crate::desugar::desugared_ast::TypeDecl, registry: &mut RegistryBuilder) {
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
                        name: ConstructorName::new(m.name.value.as_str()),
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
                .and_then(|ident| crate::hir::BuiltinConst::parse(ident.name.as_str()));
            builtin
                .map(crate::hir::BuiltinConst::value)
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
            let l = eval_scale_expr(lhs, src)?;
            let r = eval_scale_expr(rhs, src)?;
            match op {
                BinOp::Add => Ok(l + r),
                BinOp::Sub => Ok(l - r),
                BinOp::Mul => Ok(l * r),
                BinOp::Div => Ok(l / r),
                BinOp::Pow => Ok(l.powf(r)),
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

/// Evaluate a range expression (e.g. `0.0 s`) to get its SI value and dimension.
///
/// Range expressions are syntactically restricted to numeric literals and
/// unit-annotated literals, so we evaluate them directly against the
/// `RegistryBuilder` instead of going through the full `eval_expr` pipeline.
///
/// Returns `(si_value, dimension)`.
fn eval_range_expr(
    expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(f64, crate::syntax::dimension::Dimension), GraphcalError> {
    use crate::syntax::dimension::Dimension;

    let ensure_finite = |value: f64, span: Span| {
        if value.is_finite() {
            Ok(value)
        } else {
            Err(eval_error(
                format!("range expression must be finite, got {value}"),
                src,
                span,
            ))
        }
    };

    match &expr.kind {
        ExprKind::Number(n) => Ok((ensure_finite(*n, expr.span)?, Dimension::dimensionless())),
        ExprKind::UnitLiteral { value, unit } => {
            let (dim, scale) = registry
                .resolve_unit_expr(unit)
                .map_err(|err| unit_resolve_to_graphcal(err, src, unit.span))?;
            let scale = validate_positive_finite_scale(scale, "range unit scale", src, unit.span)?;
            Ok((ensure_finite(*value * scale.get(), expr.span)?, dim))
        }
        ExprKind::UnaryOp {
            op: crate::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => {
            let (val, dim) = eval_range_expr(operand, registry, src)?;
            Ok((ensure_finite(-val, expr.span)?, dim))
        }
        _ => Err(GraphcalError::EvalError {
            message: "range expression must be a numeric or unit literal".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
    }
}

fn checked_range_step_count(
    name: &IndexName,
    start: f64,
    end: f64,
    step: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<NonZeroUsize, GraphcalError> {
    let raw_steps = (end - start) / step;
    if !raw_steps.is_finite() {
        return Err(GraphcalError::RangeIndexInvalid {
            name: name.clone(),
            message: "range cardinality is not finite".to_string(),
            src: src.clone(),
            span: span.into(),
        });
    }

    let nearest = raw_steps.round();
    let tolerance = f64::EPSILON.mul_add(raw_steps.abs().max(1.0) * 16.0, 1e-12);
    let whole_steps = if (raw_steps - nearest).abs() <= tolerance {
        nearest
    } else {
        raw_steps.floor()
    };
    if whole_steps < 0.0 {
        return Err(GraphcalError::RangeIndexInvalid {
            name: name.clone(),
            message: "range cardinality is negative".to_string(),
            src: src.clone(),
            span: span.into(),
        });
    }

    let count = whole_steps + 1.0;
    #[expect(
        clippy::cast_precision_loss,
        reason = "usize upper bound check for f64 range count"
    )]
    let max_count = usize::MAX as f64;
    if count >= max_count {
        return Err(GraphcalError::RangeIndexInvalid {
            name: name.clone(),
            message: format!("range has too many steps ({count})"),
            src: src.clone(),
            span: span.into(),
        });
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range count is finite, non-negative, and bounded by usize::MAX"
    )]
    let count = count as usize;
    NonZeroUsize::new(count).ok_or_else(|| GraphcalError::RangeIndexInvalid {
        name: name.clone(),
        message: "range must contain at least one step".to_string(),
        src: src.clone(),
        span: span.into(),
    })
}

/// Lower a range index declaration, evaluating start/end/step and validating dimensions.
fn lower_range_index(
    name: &crate::syntax::names::IndexName,
    start_expr: &Expr,
    end_expr: &Expr,
    step_expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: crate::syntax::span::Span,
) -> Result<types::IndexKind, GraphcalError> {
    let (start_val, start_dim) = eval_range_expr(start_expr, registry, src)?;
    let (end_val, end_dim) = eval_range_expr(end_expr, registry, src)?;
    let (step_val, step_dim) = eval_range_expr(step_expr, registry, src)?;

    // All three must have the same dimension
    if start_dim != end_dim || start_dim != step_dim {
        return Err(GraphcalError::RangeIndexDimensionMismatch {
            name: name.clone(),
            start_dim: format!("Dimension({})", registry.format_dimension(&start_dim)),
            end_dim: format!("Dimension({})", registry.format_dimension(&end_dim)),
            step_dim: format!("Dimension({})", registry.format_dimension(&step_dim)),
            src: src.clone(),
            span: decl_span.into(),
        });
    }

    for (label, value) in [("start", start_val), ("end", end_val), ("step", step_val)] {
        if !value.is_finite() {
            return Err(GraphcalError::RangeIndexInvalid {
                name: name.clone(),
                message: format!("{label} ({value}) must be finite"),
                src: src.clone(),
                span: decl_span.into(),
            });
        }
    }

    // Validate: start <= end
    if start_val > end_val {
        return Err(GraphcalError::RangeIndexInvalid {
            name: name.clone(),
            message: format!("start ({start_val}) must be <= end ({end_val})"),
            src: src.clone(),
            span: decl_span.into(),
        });
    }

    // Validate: step > 0
    if step_val <= 0.0 {
        return Err(GraphcalError::RangeIndexInvalid {
            name: name.clone(),
            message: format!("step ({step_val}) must be > 0"),
            src: src.clone(),
            span: decl_span.into(),
        });
    }

    let step_count = checked_range_step_count(name, start_val, end_val, step_val, src, decl_span)?;

    // Extract display unit from the start expression's unit annotation.
    let (display_label, display_scale) = match &start_expr.kind {
        ExprKind::UnitLiteral { unit, .. } => {
            // Unknown/dynamic units have no static display scale; the
            // expression itself is validated elsewhere.
            match registry.resolve_unit_expr(unit) {
                Ok((_dim, scale)) => {
                    let scale = validate_positive_finite_scale(
                        scale,
                        "range display unit scale",
                        src,
                        unit.span,
                    )?;
                    (Some(format_unit_expr(unit)), scale.get())
                }
                Err(crate::registry::types::UnitResolveError::Overflow(_)) => {
                    return Err(GraphcalError::DimensionOverflow {
                        src: src.clone(),
                        span: unit.span.into(),
                    });
                }
                Err(_) => (None, 1.0),
            }
        }
        _ => (None, 1.0),
    };

    Ok(types::IndexKind::Range(types::RangeIndexData {
        start: start_val,
        end: end_val,
        step: step_val,
        step_count,
        dimension: start_dim,
        display_label,
        display_scale,
    }))
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

#[cfg(test)]
mod tests {
    use super::*;
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
                .get_unit(&crate::syntax::names::UnitRef::local("km"))
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
    fn lower_source_order_preserved() {
        let ir = parse_and_lower(
            "param b: Dimensionless = 2.0;\nparam a: Dimensionless = 1.0;\nnode z: Dimensionless = @a + @b;",
        )
        .unwrap();
        let names: Vec<String> = ir.source_order.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(names, vec!["b", "a", "z"]);
    }

    #[test]
    fn merge_dependency_keeps_qualified_imported_value_keys() {
        // Regression: `prefix_dep` used to re-key a dep's *qualified*
        // imported value (e.g. `mission.C` from `import lib as mission;`)
        // with the include-instance prefix, dropping the qualifier — while
        // the merged expressions kept referencing `@mission.C` (RefPrefixer
        // skips qualified refs), so the value map and the expressions
        // diverged.
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
        // Simulate the loader having pre-evaluated `import lib as mission;`
        // inside the dep: the imported value is keyed by a qualified name.
        let qualified = ScopedName::qualified("mission", "C");
        dep_unfrozen.imported_values.insert(
            qualified.clone(),
            (
                RuntimeValue::Scalar(7.0),
                DeclaredType::Scalar(crate::syntax::dimension::Dimension::dimensionless()),
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
            .map(|(n, _)| DeclName::new(n.member()))
            .collect();
        unfrozen
            .merge_dependency(
                dep_unfrozen,
                "inst",
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

        assert!(
            unfrozen.imported_values.contains_key(&qualified),
            "qualified imported value must keep its qualifier"
        );
        assert!(
            !unfrozen
                .imported_values
                .contains_key(&ScopedName::qualified("inst", "C")),
            "imported value must not be re-keyed with the instance prefix"
        );
    }
}

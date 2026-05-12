//! Intermediate Representation (IR) — the result of lowering an AST.
//!
//! `lower()` combines name resolution (`resolve`), registry construction
//! (dimensions, units, indexes, structs), and function registration into a
//! single `IR` value that downstream stages can consume without reaching
//! back to the raw AST.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::desugar::resolved_ast::{
    AssertBody, DeclKind, Expr, ExprKind, FigureDecl, File, IndexDeclKind, LayerDecl, PlotDecl,
    TypeExpr,
};
use crate::ir::resolve::{
    DeclCategory, ExpectedFail, ImportedValueNames, ResolvedFile, resolve_with_imported_values,
};
use crate::ir::resolve::{ImportedNames, resolve_with_imports};
use crate::registry::declared_type::DeclaredType;
use crate::registry::error::GraphcalError;
use crate::registry::format::format_unit_expr;
use crate::registry::prelude::load_prelude;
use crate::registry::runtime_value::RuntimeValue;
use crate::registry::types::{self, Registry, RegistryBuilder, UnitScale};
use crate::syntax::dimension::Rational;
use crate::syntax::names::{DeclName, DimName, IndexName, ScopedName, StructTypeName};
use crate::syntax::span::Span;
use crate::syntax::visitor::{ExprVisitor, ExprVisitorMut};

// ---------------------------------------------------------------------------
// Entry types for IR declarations
// ---------------------------------------------------------------------------

/// A const declaration with type annotation.
#[derive(Debug, Clone)]
pub struct ConstEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    pub expr: Expr,
    pub span: Span,
}

/// A param declaration with type annotation.
#[derive(Debug, Clone)]
pub struct ParamEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    pub default_expr: Option<Expr>,
    pub span: Span,
}

/// A node declaration with type annotation.
#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub name: ScopedName,
    pub type_ann: TypeExpr,
    pub expr: Expr,
    pub span: Span,
}

/// An assert declaration.
#[derive(Debug, Clone)]
pub struct AssertEntry {
    pub name: ScopedName,
    pub body: AssertBody,
    pub span: Span,
}

/// A plot declaration.
#[derive(Debug, Clone)]
pub struct PlotEntry {
    pub name: ScopedName,
    pub decl: PlotDecl,
    pub span: Span,
    /// Whether this plot is `pub` (visible in standalone output).
    pub is_pub: bool,
}

/// A figure declaration.
#[derive(Debug, Clone)]
pub struct FigureEntry {
    pub name: ScopedName,
    pub decl: FigureDecl,
    pub span: Span,
}

/// A layer declaration.
#[derive(Debug, Clone)]
pub struct LayerEntry {
    pub name: ScopedName,
    pub decl: LayerDecl,
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
    /// For each param/node, the set of `@`-references (runtime deps).
    /// Outer map is keyed by declaration name (key-lookup only, order irrelevant).
    /// Inner set uses `BTreeSet` for deterministic iteration when building the DAG.
    pub runtime_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
    /// For each const, the set of const-references (const deps).
    /// Outer map is keyed by declaration name (key-lookup only, order irrelevant).
    /// Inner set uses `BTreeSet` for deterministic iteration when building the DAG.
    pub const_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// Set of all assert names.
    pub assert_names: HashSet<ScopedName>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub expected_fail: HashMap<ScopedName, ExpectedFail>,
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
    pub dag_id: crate::syntax::dag_id::DagId,
    /// Original declaration name in the owning DAG.
    pub source_name: DeclName,
}

/// Convert a resolver dep map (whose value sets are `HashSet`) into the
/// IR shape that uses `BTreeSet` for deterministic iteration order. Keys
/// and dep entries are already typed [`ScopedName`]s end-to-end.
fn wrap_dep_map(
    map: HashMap<ScopedName, HashSet<ScopedName>>,
) -> HashMap<ScopedName, BTreeSet<ScopedName>> {
    map.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

/// Lower an AST into an [`IR`].
///
/// This combines:
/// 1. Name resolution (`resolve`) — checks duplicates, casing, extracts deps
/// 2. Registry construction — registers dimensions, units, indexes, structs from declarations
/// 3. Function registration — registers user-defined functions into the registry
///
/// # Errors
///
/// Returns a [`GraphcalError`] if name resolution or registry construction fails
/// (e.g., unknown dimension in a type annotation, duplicate names, etc.).
pub fn lower(ast: &File, src: &NamedSource<Arc<String>>) -> Result<IR, GraphcalError> {
    let dag_id = crate::syntax::dag_id::DagId::from_relative_path(std::path::Path::new(src.name()));
    lower_with_imports(ast, src, &ImportedNames::default(), &dag_id)
}

/// Lower an AST with imported declarations into an [`IR`].
///
/// Same as [`lower`] but accepts imported names from other files.
/// The registry is frozen (via `build()`) before returning.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if name resolution or registry construction fails.
fn lower_with_imports(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
    dag_id: &crate::syntax::dag_id::DagId,
) -> Result<IR, GraphcalError> {
    let (builder, resolved_ir) = lower_to_builder(ast, src, imported, dag_id)?;
    Ok(resolved_ir.freeze(builder.build()))
}

/// Lower an AST with imported declarations, returning a `RegistryBuilder`
/// that can be further mutated (e.g., to register imported type-system
/// declarations) before freezing.
///
/// Call [`UnfrozenIR::freeze`] with the final [`Registry`] to produce an [`IR`].
///
/// # Errors
///
/// Returns a [`GraphcalError`] if name resolution or registry construction fails.
pub(crate) fn lower_to_builder(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
    dag_id: &crate::syntax::dag_id::DagId,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Name resolution
    let resolved = resolve_with_imports(ast, src, imported)?;

    // Step 2: Extract type annotations from AST + imported declarations
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
        HashMap::new(),
        HashMap::new(),
        dag_id,
        None,
    )
}

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
/// Returns a [`GraphcalError`] if name resolution or registry construction fails.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_to_builder_with_imported_values(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    dag_id: &crate::syntax::dag_id::DagId,
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
/// Returns a [`GraphcalError`] if name resolution or registry construction fails.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn lower_to_builder_with_imported_value_decls(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    dag_id: &crate::syntax::dag_id::DagId,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Name resolution with imported value names in scope
    let resolved = resolve_with_imported_values(ast, src, imported_names)?;

    // Step 2: Extract type annotations from local declarations only
    let type_anns = extract_type_annotations(ast);

    // Step 3: Build registry, augment deps, and construct IR
    build_ir_from_resolved(
        ast,
        src,
        resolved,
        type_anns,
        imported_values,
        imported_decl_types,
        imported_value_sources,
        dag_id,
        None,
    )
}

/// Lower a `dag { ... }` body as if it were a standalone file.
///
/// The dag body is a virtual [`File`] whose registry is seeded with the
/// enclosing file's frozen registry (dimensions, units, types, indexes, and
/// sibling dags) so that name resolution and type checking behave exactly as
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
/// Returns a [`GraphcalError`] if name resolution or type-system construction
/// fails for the dag body.
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
    stripped_body: &[crate::desugar::resolved_ast::Declaration],
    parent_registry: &Registry,
    imported_names: &ImportedValueNames,
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &crate::syntax::dag_id::DagId,
) -> Result<IR, GraphcalError> {
    let virtual_file = File {
        declarations: stripped_body.to_vec(),
    };
    let dag_dag_id = parent_dag_id.child(dag_name);

    let resolved = resolve_with_imported_values(&virtual_file, src, imported_names)?;
    let type_anns = extract_type_annotations(&virtual_file);

    let (builder, unfrozen) = build_ir_from_resolved(
        &virtual_file,
        src,
        resolved,
        type_anns,
        HashMap::new(),
        imported_decl_types,
        imported_value_sources,
        &dag_dag_id,
        Some(parent_registry),
    )?;
    Ok(unfrozen.freeze(builder.build()))
}

/// Result of `preprocess_dag_body_self_imports`: imported names, declared
/// types, source bindings, and the body with self-import declarations stripped.
pub struct DagBodySelfImports {
    pub names: ImportedValueNames,
    pub decl_types: HashMap<ScopedName, DeclaredType>,
    pub value_sources: HashMap<ScopedName, ImportedValueSource>,
    pub stripped_body: Vec<crate::desugar::resolved_ast::Declaration>,
}

/// Collect every type-system declaration name from a frozen [`Registry`].
///
/// Used to build the `parent_type_system_names` argument for
/// `preprocess_dag_body_self_imports` when the caller has a frozen
/// registry (e.g. inside `compile_inline_dag_bodies` orchestrating
/// dag-body lowering for a file's own inline DAGs).
#[must_use]
pub fn type_system_names_from_registry(registry: &Registry) -> HashSet<String> {
    let mut out = HashSet::new();
    for (name, _) in registry.dimensions.all_dimensions() {
        out.insert(name.to_string());
    }
    for (name, _, _) in registry.units.all_units() {
        out.insert(name.to_string());
    }
    for ty in registry.types.all_types() {
        out.insert(ty.name.to_string());
    }
    for idx in registry.indexes.all_indexes() {
        out.insert(idx.name.to_string());
    }
    for (name, _) in registry.dags.all_dags() {
        out.insert(name.clone());
    }
    out
}

/// Collect every type-system declaration name (dim/unit/type/union
/// member/index/dag) declared in `file`.
///
/// Used to build the `parent_type_system_names` argument for
/// `preprocess_dag_body_self_imports` when the caller has the parent
/// file's AST handy but no frozen [`Registry`].
#[must_use]
pub fn collect_type_system_names(file: &crate::desugar::resolved_ast::File) -> HashSet<String> {
    let mut out = HashSet::new();
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::BaseDimension(d) => {
                out.insert(d.name.value.to_string());
            }
            DeclKind::Dimension(d) => {
                out.insert(d.name.value.to_string());
            }
            DeclKind::Unit(u) => {
                out.insert(u.name.value.to_string());
            }
            DeclKind::Type(t) => {
                out.insert(t.name.value.to_string());
            }
            DeclKind::UnionType(u) => {
                out.insert(u.name.value.to_string());
                for m in &u.members {
                    out.insert(m.name.value.to_string());
                }
            }
            DeclKind::Index(i) => {
                out.insert(i.name.value.to_string());
            }
            DeclKind::Dag(d) => {
                out.insert(d.name.value.to_string());
            }
            _ => {}
        }
    }
    out
}

/// Remove and return the type annotation for `name`, or raise an internal error
/// if it was dropped during resolution. The parser and resolver jointly
/// guarantee that every top-level const/param/node ends up in `type_anns`;
/// a missing entry is a compiler invariant violation.
fn take_type_ann(
    type_anns: &mut HashMap<String, TypeExpr>,
    name: &str,
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
    mut resolved: ResolvedFile,
    mut type_anns: HashMap<String, TypeExpr>,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_decl_types: HashMap<ScopedName, DeclaredType>,
    imported_value_sources: HashMap<ScopedName, ImportedValueSource>,
    dag_id: &crate::syntax::dag_id::DagId,
    parent_registry: Option<&Registry>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Build registry (prelude + user-declared dimensions/units/indexes/structs).
    // When a parent registry is provided (inline-dag bodies), its entries are
    // merged in before registering the virtual file's own declarations so that
    // type annotations and dynamic-unit dep augmentation see the enclosing
    // file's type system.
    let mut builder = RegistryBuilder::new();
    load_prelude(&mut builder);
    if let Some(parent) = parent_registry {
        builder.merge_from_registry(parent);
    }
    register_file_declarations(ast, &mut builder, src, dag_id)?;

    // Augment runtime deps with transitive dependencies through dynamic units.
    let dynamic_unit_deps = build_dynamic_unit_deps(&builder);
    augment_runtime_deps_for_dynamic_units(
        &mut resolved.runtime_deps,
        &dynamic_unit_deps,
        &resolved.params,
        &resolved.nodes,
    );

    // Pair resolved declarations with type annotations.
    let consts = resolved
        .consts
        .into_iter()
        .map(|entry| {
            let type_ann = take_type_ann(&mut type_anns, &entry.name, entry.span, src)?;
            Ok(ConstEntry {
                name: ScopedName::local(entry.name),
                type_ann,
                expr: entry.expr,
                span: entry.span,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let params = resolved
        .params
        .into_iter()
        .map(|entry| {
            let type_ann = take_type_ann(&mut type_anns, &entry.name, entry.span, src)?;
            Ok(ParamEntry {
                name: ScopedName::local(entry.name),
                type_ann,
                default_expr: entry.default_expr,
                span: entry.span,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let nodes = resolved
        .nodes
        .into_iter()
        .map(|entry| {
            let type_ann = take_type_ann(&mut type_anns, &entry.name, entry.span, src)?;
            Ok(NodeEntry {
                name: ScopedName::local(entry.name),
                type_ann,
                expr: entry.expr,
                span: entry.span,
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
            .map(|entry| AssertEntry {
                name: ScopedName::local(entry.name),
                body: entry.body,
                span: entry.span,
            })
            .collect(),
        plots: resolved
            .plots
            .into_iter()
            .map(|entry| {
                let is_pub = resolved.pub_names.contains(entry.name.as_str());
                PlotEntry {
                    name: ScopedName::local(entry.name),
                    decl: entry.decl,
                    span: entry.span,
                    is_pub,
                }
            })
            .collect(),
        figures: resolved
            .figures
            .into_iter()
            .map(|entry| FigureEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                span: entry.span,
            })
            .collect(),
        layers: resolved
            .layers
            .into_iter()
            .map(|entry| LayerEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                span: entry.span,
            })
            .collect(),
        runtime_deps: wrap_dep_map(resolved.runtime_deps),
        const_deps: wrap_dep_map(resolved.const_deps),
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
                    ScopedName::local(k),
                    v.into_iter().map(ScopedName::local).collect(),
                )
            })
            .collect(),
        expected_fail: resolved
            .expected_fail
            .into_iter()
            .map(|(k, v)| (ScopedName::local(k), v))
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
    consts: Vec<ConstEntry>,
    params: Vec<ParamEntry>,
    nodes: Vec<NodeEntry>,
    asserts: Vec<AssertEntry>,
    plots: Vec<PlotEntry>,
    figures: Vec<FigureEntry>,
    layers: Vec<LayerEntry>,
    runtime_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
    const_deps: HashMap<ScopedName, BTreeSet<ScopedName>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    assert_names: HashSet<ScopedName>,
    // Key-lookup only, order irrelevant.
    assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    // Key-lookup only, order irrelevant.
    expected_fail: HashMap<ScopedName, ExpectedFail>,
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
    /// Freeze into a complete [`IR`] by providing a built [`Registry`].
    #[must_use]
    pub fn freeze(self, registry: Registry) -> IR {
        IR {
            registry,
            consts: self.consts,
            params: self.params,
            nodes: self.nodes,
            asserts: self.asserts,
            plots: self.plots,
            figures: self.figures,
            layers: self.layers,
            runtime_deps: self.runtime_deps,
            const_deps: self.const_deps,
            source_order: self.source_order,
            assert_names: self.assert_names,
            assumes_map: self.assumes_map,
            expected_fail: self.expected_fail,
            imported_values: self.imported_values,
            imported_decl_types: self.imported_decl_types,
            imported_value_sources: self.imported_value_sources,
            pub_names: self.pub_names,
        }
    }

    /// Add a const alias: a synthetic const declaration that references another const.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix::delta_v`.
    pub fn add_const_alias(
        &mut self,
        name: ScopedName,
        type_ann: TypeExpr,
        expr: Expr,
        span: Span,
        target: ScopedName,
    ) {
        let mut deps = BTreeSet::new();
        deps.insert(target);
        self.const_deps.insert(name.clone(), deps);
        self.consts.push(ConstEntry {
            name: name.clone(),
            type_ann,
            expr,
            span,
        });
        self.source_order.push((name, DeclCategory::Const));
    }

    /// Add a node alias: a synthetic node declaration that references another node/param.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix::delta_v`.
    pub fn add_node_alias(
        &mut self,
        name: ScopedName,
        type_ann: TypeExpr,
        expr: Expr,
        span: Span,
        target: ScopedName,
    ) {
        let mut deps = BTreeSet::new();
        deps.insert(target);
        self.runtime_deps.insert(name.clone(), deps);
        self.nodes.push(NodeEntry {
            name: name.clone(),
            type_ann,
            expr,
            span,
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
        bindings: &HashMap<String, Expr>,
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
    /// All declarations from the dependency are prefixed with `prefix::` and
    /// appended to this IR's declaration lists. Param bindings replace the
    /// dependency's param default expressions. Internal references within the
    /// dependency's expressions are rewritten to use prefixed names.
    ///
    /// `dep_names` is the set of all declaration names in the dependency (before
    /// prefixing), used to determine which references should be rewritten.
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
        bindings: &HashMap<String, Expr>,
        dep_names: &HashSet<String>,
        index_bindings: &HashMap<IndexName, IndexName>,
        type_bindings: &HashMap<StructTypeName, StructTypeName>,
        dim_bindings: &HashMap<DimName, DimName>,
        import_item_attributes: &HashMap<String, Vec<crate::desugar::resolved_ast::Attribute>>,
        importer_src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        /// Prefix a `ScopedName` dep if its member is in `dep_names`.
        fn prefix_dep(d: &ScopedName, prefix: &str, dep_names: &HashSet<String>) -> ScopedName {
            if dep_names.contains(d.member()) {
                d.with_prefix(prefix)
            } else {
                d.clone()
            }
        }

        // Merge consts
        for mut entry in dep.consts {
            substitute_index_names(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names);
            substitute_type_expr_index_names(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            let prefixed = entry.name.with_prefix(prefix);
            // Prefix const deps
            if let Some(deps) = dep.const_deps.get(&entry.name) {
                let prefixed_deps = deps
                    .iter()
                    .map(|d| prefix_dep(d, prefix, dep_names))
                    .collect();
                self.const_deps.insert(prefixed.clone(), prefixed_deps);
            }
            self.consts.push(ConstEntry {
                name: prefixed.clone(),
                type_ann: entry.type_ann,
                expr: entry.expr,
                span: entry.span,
            });
            self.source_order.push((prefixed, DeclCategory::Const));
        }

        // Merge params — replace defaults with bindings where provided
        for mut entry in dep.params {
            let prefixed = entry.name.with_prefix(prefix);
            if let Some(binding_expr) = bindings.get(entry.name.member()) {
                // Use the binding expression (from the importer's scope, no prefixing needed
                // for refs that belong to the importer — only dep-internal refs get prefixed)
                entry.default_expr = Some(binding_expr.clone());
            } else if let Some(ref mut expr) = entry.default_expr {
                // Keep default, but substitute index names and prefix internal refs
                substitute_index_names(expr, index_bindings);
                substitute_type_names_in_expr(expr, type_bindings);
                prefix_expr_refs(expr, prefix, dep_names);
            } else {
                // Required param without binding — stays None, caught later in exec_plan
            }
            substitute_type_expr_index_names(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            // Rebuild runtime deps for the (possibly rewritten) expression
            let mut graph_refs = BTreeSet::new();
            if let Some(orig_deps) = dep.runtime_deps.get(&entry.name) {
                if bindings.contains_key(entry.name.member()) {
                    // Binding expression — deps are already in the importer's namespace.
                    // We'll recompute deps from the binding expression below.
                } else {
                    // Default expression — prefix dep-internal deps
                    for d in orig_deps {
                        graph_refs.insert(prefix_dep(d, prefix, dep_names));
                    }
                }
            }
            if let Some(binding_expr) = bindings.get(entry.name.member()) {
                // Collect graph refs from the binding expression
                collect_graph_refs_from_expr(binding_expr, &mut graph_refs);
            }
            self.runtime_deps.insert(prefixed.clone(), graph_refs);
            self.params.push(ParamEntry {
                name: prefixed.clone(),
                type_ann: entry.type_ann,
                default_expr: entry.default_expr,
                span: entry.span,
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
            if let Some(deps) = dep.runtime_deps.get(&entry.name) {
                let prefixed_deps = deps
                    .iter()
                    .map(|d| prefix_dep(d, prefix, dep_names))
                    .collect();
                self.runtime_deps.insert(prefixed.clone(), prefixed_deps);
            }
            self.nodes.push(NodeEntry {
                name: prefixed.clone(),
                type_ann: entry.type_ann,
                expr: entry.expr,
                span: entry.span,
            });
            self.source_order.push((prefixed, DeclCategory::Node));
        }

        // Merge asserts
        for mut entry in dep.asserts {
            match &mut entry.body {
                crate::desugar::resolved_ast::AssertBody::Expr(e) => {
                    substitute_index_names(e, index_bindings);
                    substitute_type_names_in_expr(e, type_bindings);
                    prefix_expr_refs(e, prefix, dep_names);
                }
                crate::desugar::resolved_ast::AssertBody::Tolerance {
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
            self.asserts.push(AssertEntry {
                name: prefixed.clone(),
                body: entry.body,
                span: entry.span,
            });
            self.assert_names.insert(prefixed.clone());
            self.source_order.push((prefixed, DeclCategory::Assert));
        }

        // Merge plots
        for mut entry in dep.plots {
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
            let prefixed = entry.name.with_prefix(prefix);
            self.plots.push(PlotEntry {
                name: prefixed.clone(),
                decl: entry.decl,
                span: entry.span,
                is_pub: entry.is_pub,
            });
            self.source_order.push((prefixed, DeclCategory::Plot));
        }

        // Merge figures
        for mut entry in dep.figures {
            for field in &mut entry.decl.fields {
                substitute_index_names(&mut field.value, index_bindings);
                substitute_type_names_in_expr(&mut field.value, type_bindings);
                prefix_expr_refs(&mut field.value, prefix, dep_names);
            }
            // Prefix plot names referenced by the figure
            for plot_name in &mut entry.decl.plot_names {
                if dep_names.contains(plot_name.value.as_str()) {
                    plot_name.value = crate::syntax::names::DeclName::new(format!(
                        "{prefix}::{}",
                        plot_name.value
                    ));
                }
            }
            let prefixed = entry.name.with_prefix(prefix);
            self.figures.push(FigureEntry {
                name: prefixed.clone(),
                decl: entry.decl,
                span: entry.span,
            });
            self.source_order.push((prefixed, DeclCategory::Figure));
        }

        // Merge layers
        for mut entry in dep.layers {
            for field in &mut entry.decl.fields {
                substitute_index_names(&mut field.value, index_bindings);
                substitute_type_names_in_expr(&mut field.value, type_bindings);
                prefix_expr_refs(&mut field.value, prefix, dep_names);
            }
            // Prefix plot names referenced by the layer
            for plot_name in &mut entry.decl.plot_names {
                if dep_names.contains(plot_name.value.as_str()) {
                    plot_name.value = crate::syntax::names::DeclName::new(format!(
                        "{prefix}::{}",
                        plot_name.value
                    ));
                }
            }
            let prefixed = entry.name.with_prefix(prefix);
            self.layers.push(LayerEntry {
                name: prefixed.clone(),
                decl: entry.decl,
                span: entry.span,
            });
            self.source_order.push((prefixed, DeclCategory::Layer));
        }

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
                                !key.iter()
                                    .any(|(idx, _)| index_bindings.contains_key(idx.as_str()))
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
                if attr.name.name == "expected_fail" {
                    let prefixed_assert = ScopedName::Local(orig_name.clone()).with_prefix(prefix);
                    let ef = crate::ir::resolve::names::parse_expected_fail_args(
                        &attr.args,
                        importer_src,
                    )?;
                    self.expected_fail.insert(prefixed_assert, ef);
                }
            }
        }

        // Propagate the dep's imported-value metadata. An inline DAG body
        // whose `import <self>.{...}` resolves to a different file leaves
        // its parent-file value bindings on `dep.imported_values` /
        // `dep.imported_value_sources`; merging the dag into the importer
        // requires those entries to ride along so eval can resolve the
        // local alias (e.g., `radius` in `prefix::result = @radius * ...`).
        // Keys keep their original `ScopedName` (they were not in
        // `dep_names` and therefore not prefixed in expressions).
        for (name, value) in dep.imported_values {
            self.imported_values.entry(name).or_insert(value);
        }
        for (name, dt) in dep.imported_decl_types {
            self.imported_decl_types.entry(name).or_insert(dt);
        }
        for (name, source) in dep.imported_value_sources {
            self.imported_value_sources.entry(name).or_insert(source);
        }
        Ok(())
    }
}

/// Visitor that detects V005 / A8 violations in a param default expression.
///
/// Emits [`GraphcalError::IncludeMustReconcileOverride`] on the first
/// occurrence of a variant literal `s::v` where `s` is in
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
        use crate::desugar::resolved_ast::TypeExprKind;
        match &type_expr.kind {
            TypeExprKind::DimExpr(dim_expr) => {
                for item in &dim_expr.terms {
                    let name = item.term.name.name.as_str();
                    if self.type_bindings.contains_key(name) {
                        return Err(self.orphan_error("type", name, format!("type `{name}`")));
                    }
                }
                Ok(())
            }
            TypeExprKind::TypeApplication { name, type_args } => {
                let n = name.name.as_str();
                if self.type_bindings.contains_key(n) {
                    return Err(self.orphan_error("type", n, format!("type `{n}`")));
                }
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

impl ExprVisitor<crate::syntax::phase::Resolved> for OverrideReconciliationChecker<'_> {
    type Error = GraphcalError;

    fn visit_leaf(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::VariantLiteral { index, variant } = &expr.kind
            && self.index_bindings.contains_key(index.value.as_str())
        {
            return Err(self.orphan_error(
                "index",
                index.value.as_ref(),
                format!(
                    "`{}`",
                    crate::syntax::names::fmt_qualified_variant(&index.value, &variant.value)
                ),
            ));
        }
        Ok(())
    }

    fn visit_single_child(&mut self, expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        match &expr.kind {
            ExprKind::IndexAccess { args, .. } => {
                for arg in args {
                    if let crate::desugar::resolved_ast::IndexArg::Variant { index, variant } = arg
                        && self.index_bindings.contains_key(index.value.as_str())
                    {
                        return Err(self.orphan_error(
                            "index",
                            index.value.as_ref(),
                            format!(
                                "`{}`",
                                crate::syntax::names::fmt_qualified_variant(
                                    &index.value,
                                    &variant.value
                                )
                            ),
                        ));
                    }
                }
            }
            ExprKind::AsCast { target_type, .. } => {
                self.check_type_expr(target_type)?;
            }
            _ => {}
        }
        self.visit_expr(inner)
    }

    fn visit_map_entries(
        &mut self,
        _expr: &Expr,
        entries: &[crate::desugar::resolved_ast::MapEntry],
    ) -> Result<(), Self::Error> {
        for entry in entries {
            if let Some(key) = entry.keys.first()
                && self.index_bindings.contains_key(key.index.value.as_str())
            {
                return Err(self.orphan_error(
                    "index",
                    key.index.value.as_ref(),
                    format!(
                        "`{}`",
                        crate::syntax::names::fmt_qualified_variant(
                            &key.index.value,
                            &key.variant.value,
                        )
                    ),
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
        arms: &[crate::desugar::resolved_ast::MatchArm],
    ) -> Result<(), Self::Error> {
        self.visit_expr(scrutinee)?;
        for arm in arms {
            if let Some(qi) = &arm.pattern.qualified_index
                && self.index_bindings.contains_key(qi.value.as_str())
            {
                return Err(self.orphan_error(
                    "index",
                    qi.value.as_ref(),
                    format!(
                        "`{}`",
                        crate::syntax::names::fmt_qualified_variant(
                            &qi.value,
                            &arm.pattern.variant_name.value,
                        )
                    ),
                ));
            }
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }

    fn visit_struct_construction(
        &mut self,
        expr: &Expr,
        fields: &[crate::desugar::resolved_ast::FieldInit],
    ) -> Result<(), Self::Error> {
        if let ExprKind::StructConstruction {
            type_name,
            type_args,
            ..
        } = &expr.kind
        {
            let n = type_name.value.as_str();
            if self.type_bindings.contains_key(n) {
                return Err(self.orphan_error("type", n, format!("constructor `{n} {{ ... }}`")));
            }
            for ty in type_args {
                self.check_type_expr(ty)?;
            }
        }
        for f in fields {
            if let Some(v) = &f.value {
                self.visit_expr(v)?;
            }
        }
        Ok(())
    }

    fn visit_fn_call(&mut self, expr: &Expr, args: &[Expr]) -> Result<(), Self::Error> {
        if let ExprKind::FnCall { type_args, .. } = &expr.kind {
            for ga in type_args {
                if let crate::desugar::resolved_ast::GenericArg::Type(ty) = ga {
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
/// declaration name. No flat `::` strings are constructed here — the
/// `Local`/`Qualified` distinction lives in the variant.
struct RefPrefixer<'a> {
    prefix: &'a str,
    dep_names: &'a HashSet<String>,
}

impl RefPrefixer<'_> {
    fn rewrite(&self, scoped: &ScopedName) -> Option<ScopedName> {
        // Only rewrite refs that are local to the dep (i.e. unqualified
        // members owned by the dependency). Already-qualified refs (e.g.
        // a transitively-imported `@module.x` inside the dep) belong to
        // some other namespace and are left untouched.
        if let ScopedName::Local(name) = scoped
            && self.dep_names.contains(name.as_str())
        {
            Some(scoped.with_prefix(self.prefix))
        } else {
            None
        }
    }
}

impl ExprVisitorMut<crate::syntax::phase::Resolved> for RefPrefixer<'_> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &mut expr.kind
            && let Some(prefixed) = self.rewrite(&ident.value)
        {
            ident.value = prefixed;
        }
        Ok(())
    }

    fn visit_const_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::ConstRef(ident) = &mut expr.kind
            && let Some(prefixed) = self.rewrite(&ident.value)
        {
            ident.value = prefixed;
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
/// For example, `GraphRef("dry_mass")` becomes `GraphRef("r::dry_mass")` when
/// `"dry_mass"` is in `dep_names` and `prefix` is `"r"`.
///
/// Built-in names and names from the importer's scope are left unchanged.
pub(crate) fn prefix_expr_refs(expr: &mut Expr, prefix: &str, dep_names: &HashSet<String>) {
    let mut prefixer = RefPrefixer { prefix, dep_names };
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

impl ExprVisitorMut<crate::syntax::phase::Resolved> for IndexSubstituter<'_> {
    type Error = std::convert::Infallible;

    fn visit_variant_literal_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::VariantLiteral { index, .. } = &mut expr.kind
            && let Some(new) = self.bindings.get(index.value.as_str())
        {
            index.value = new.clone();
        }
        Ok(())
    }

    fn visit_for_comp_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::ForComp { bindings, body } = &mut expr.kind {
            for b in bindings {
                if let crate::desugar::resolved_ast::ForBindingIndex::Named(ref mut spanned_idx) =
                    b.index
                    && let Some(new) = self.bindings.get(spanned_idx.value.as_str())
                {
                    spanned_idx.value = new.clone();
                }
            }
            self.visit_expr_mut(body)?;
        }
        Ok(())
    }

    fn visit_index_access_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        use crate::desugar::resolved_ast::IndexArg;
        if let ExprKind::IndexAccess { expr: inner, args } = &mut expr.kind {
            for arg in args.iter_mut() {
                match arg {
                    IndexArg::Variant { index, .. } => {
                        if let Some(new) = self.bindings.get(index.value.as_str()) {
                            index.value = new.clone();
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
                    if let Some(new) = self.bindings.get(key.index.value.as_str()) {
                        key.index.value = new.clone();
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
                if let Some(ref mut idx) = arm.pattern.qualified_index
                    && let Some(new) = self.bindings.get(idx.value.as_str())
                {
                    idx.value = new.clone();
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
/// correct before ref-prefixing adds the `prefix::` qualifier.
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
    use crate::desugar::resolved_ast::TypeExprKind;

    if bindings.is_empty() {
        return;
    }
    match &mut type_expr.kind {
        TypeExprKind::Indexed { base, indexes } => {
            for idx_expr in indexes.iter_mut() {
                if let crate::desugar::resolved_ast::IndexExpr::Name(ident) = idx_expr
                    && let Some(new_name) = bindings.get(ident.name.as_str())
                {
                    ident.name = new_name.as_str().to_string();
                }
            }
            substitute_type_expr_index_names(base, bindings);
        }
        TypeExprKind::TypeApplication { type_args, .. } => {
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
    use crate::desugar::resolved_ast::TypeExprKind;

    if bindings.is_empty() {
        return;
    }
    match &mut type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => {
            for item in &mut dim_expr.terms {
                if let Some(new_name) = bindings.get(item.term.name.name.as_str()) {
                    item.term.name.name = new_name.as_ref().to_string();
                }
            }
        }
        TypeExprKind::Indexed { base, .. } => {
            substitute_type_expr_nominal_names(base, bindings);
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            if let Some(new_name) = bindings.get(name.name.as_str()) {
                name.name = new_name.as_ref().to_string();
            }
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
/// Covers `StructConstruction.type_name`, `StructConstruction.type_args`,
/// `AsCast.target_type`, and `FnCall.type_args`. Recurses through child
/// expressions so nested struct constructions are also rewritten.
#[expect(
    clippy::too_many_lines,
    reason = "single recursion covering every ExprKind variant"
)]
pub(crate) fn substitute_type_names_in_expr(
    expr: &mut Expr,
    bindings: &HashMap<StructTypeName, StructTypeName>,
) {
    use crate::desugar::resolved_ast::{GenericArg, IndexArg};

    if bindings.is_empty() {
        return;
    }
    match &mut expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::VariantLiteral { .. } => {}

        ExprKind::InlineDagRef { args, .. } => {
            for binding in args {
                substitute_type_names_in_expr(&mut binding.value, bindings);
            }
        }

        ExprKind::StructConstruction {
            type_name,
            type_args,
            fields,
        } => {
            if let Some(new_name) = bindings.get(type_name.value.as_str()) {
                type_name.value = new_name.clone();
            }
            for ty in type_args.iter_mut() {
                substitute_type_expr_nominal_names(ty, bindings);
            }
            for field in fields {
                if let Some(val) = &mut field.value {
                    substitute_type_names_in_expr(val, bindings);
                }
            }
        }

        ExprKind::AsCast {
            expr: inner,
            target_type,
        } => {
            substitute_type_expr_nominal_names(target_type, bindings);
            substitute_type_names_in_expr(inner, bindings);
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
        // TupleMatch is desugared to If/BinOp(Eq) chains before this pass runs.
        #[expect(
            clippy::unreachable,
            reason = "invariant: desugared before IR lowering"
        )]
        ExprKind::TupleMatch { .. } => {
            unreachable!("TupleMatch should be desugared before substitute_type_names_in_expr")
        }
        // `Sugar` and `UnresolvedRef` payloads are `Infallible` in `Resolved`
        // — both arms are statically unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar/UnresolvedRef(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) | ExprKind::UnresolvedRef(s) => match *s {},
    }
}

/// Collect all `@`-referenced names from an expression (non-recursive into child scopes).
///
/// This is a simpler version of `resolve::collect_graph_refs` that operates on
/// arbitrary expressions without requiring a known-names set. Used for building
/// runtime deps from binding expressions.
fn collect_graph_refs_from_expr(expr: &Expr, refs: &mut BTreeSet<ScopedName>) {
    crate::ir::resolve::collect_scoped_graph_refs(expr, refs);
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
    dag_id: &crate::syntax::dag_id::DagId,
) -> Result<(), GraphcalError> {
    register_declarations_impl(file, registry, src, None, dag_id)
}

/// Register only the named type-system declarations (dimensions, units, indexes, types)
/// from a file into the registry.
///
/// This is the selective counterpart to `register_file_declarations`: instead of
/// registering everything, it only registers declarations whose names are in `names`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn register_selected_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    names: &std::collections::HashSet<String>,
    dag_id: &crate::syntax::dag_id::DagId,
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
/// When `filter` is `Some(names)`, only declarations whose names are in `names` are registered.
fn register_declarations_impl(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    filter: Option<&std::collections::HashSet<String>>,
    dag_id: &crate::syntax::dag_id::DagId,
) -> Result<(), GraphcalError> {
    use crate::desugar::resolved_ast::{DimDecl, IndexDecl, UnitDecl};

    let should_register = |name: &str| filter.is_none_or(|names| names.contains(name));

    // Collect declarations by kind for phased registration.
    let mut derived_dims: Vec<&DimDecl> = Vec::new();
    let mut units: Vec<&UnitDecl> = Vec::new();
    let mut required_range_indexes: Vec<(&IndexDecl, Span)> = Vec::new();
    let mut range_indexes: Vec<(&IndexDecl, Span)> = Vec::new();

    // Phase 1: Register base dimensions, types, union types, named/required-named indexes.
    // Also collect derived dims, units, and dependent indexes for later phases.
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::BaseDimension(d) if should_register(d.name.value.as_str()) => {
                register_base_dimension_decl(d, registry, dag_id);
            }
            DeclKind::Dimension(d) if should_register(d.name.value.as_str()) => {
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
            DeclKind::Unit(u) if should_register(u.name.value.as_str()) => {
                units.push(u);
            }
            DeclKind::Index(idx) if should_register(idx.name.value.as_str()) => match &idx.kind {
                IndexDeclKind::RequiredRange { .. } => {
                    required_range_indexes.push((idx, decl.span));
                }
                IndexDeclKind::Range { .. } => {
                    range_indexes.push((idx, decl.span));
                }
                IndexDeclKind::Named { .. } | IndexDeclKind::RequiredNamed => {
                    register_index_decl(idx, registry, src, decl.span)?;
                }
            },
            DeclKind::Type(t) if should_register(t.name.value.as_str()) => {
                register_type_decl(t, registry);
            }
            DeclKind::UnionType(t) if should_register(t.name.value.as_str()) => {
                register_union_type_decl(t, registry);
            }
            DeclKind::Dag(d) if should_register(d.name.value.as_str()) => {
                registry.register_dag(d.name.value.to_string(), d.clone());
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
                collect_nat_ranges_from_type_expr(&d.type_ann, registry);
                if let Some(ref value) = d.value {
                    collect_nat_ranges_from_expr(value, registry);
                }
            }
            DeclKind::Node(d) => {
                collect_nat_ranges_from_type_expr(&d.type_ann, registry);
                collect_nat_ranges_from_expr(&d.value, registry);
            }
            DeclKind::ConstNode(d) => {
                collect_nat_ranges_from_type_expr(&d.type_ann, registry);
                collect_nat_ranges_from_expr(&d.value, registry);
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
    dims: &[&'a crate::desugar::resolved_ast::DimDecl],
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<&'a crate::desugar::resolved_ast::DimDecl>, GraphcalError> {
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
            let dep_name = item.term.name.name.as_str();
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
/// A unit depends on other units through its `definition.unit_expr` (e.g., `unit km: Length = 1000 m;`
/// depends on `m`). Dependencies on units already in the registry are satisfied and
/// do not create graph edges.
fn topo_sort_units<'a>(
    units: &[&'a crate::desugar::resolved_ast::UnitDecl],
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<&'a crate::desugar::resolved_ast::UnitDecl>, GraphcalError> {
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
                let dep_name = item.name.value.as_str();
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
    d: &crate::desugar::resolved_ast::BaseDimDecl,
    registry: &mut RegistryBuilder,
    dag_id: &crate::syntax::dag_id::DagId,
) {
    let dim_id = crate::syntax::dimension::BaseDimId::UserDefined {
        dag: dag_id.clone(),
        name: d.name.value.to_string(),
    };
    registry.register_base_dimension(d.name.value.clone(), dim_id);
}

fn register_dimension_decl(
    d: &crate::desugar::resolved_ast::DimDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    // Only derived dims reach this function; required dims (`dim D;`)
    // are routed to `register_required_dimension_decl` in Phase 1 and
    // never end up in the topo-sorted derived-dim list.
    let Some(definition) = d.definition.as_ref() else {
        return Ok(());
    };
    let dim =
        registry
            .resolve_dim_expr(definition)
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
    d: &crate::desugar::resolved_ast::DimDecl,
    registry: &mut RegistryBuilder,
    dag_id: &crate::syntax::dag_id::DagId,
) {
    let dim_id = crate::syntax::dimension::BaseDimId::UserDefined {
        dag: dag_id.clone(),
        name: d.name.value.to_string(),
    };
    registry.register_base_dimension(d.name.value.clone(), dim_id);
}

fn register_unit_decl(
    u: &crate::desugar::resolved_ast::UnitDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let dim =
        registry
            .resolve_dim_expr(&u.dim_type)
            .ok_or_else(|| GraphcalError::UnknownDimension {
                name: DimName::new(u.name.value.as_str()),
                src: src.clone(),
                span: u.name.span.into(),
            })?;
    let scale = if let Some(def) = &u.definition {
        if contains_graph_ref(&def.scale_expr) {
            // Dynamic unit: scale depends on runtime values (e.g., `(@rate) USD`).
            // Resolve the base unit's dimension and static scale factor.
            let base_scale = resolve_base_unit_static_scale(registry, &def.unit_expr, src)?;
            UnitScale::Dynamic {
                scale_expr: def.scale_expr.clone(),
                base_unit_scale: base_scale,
            }
        } else {
            // Static unit: scale is a compile-time constant.
            let (_unit_dim, base_scale) =
                registry.resolve_unit_expr(&def.unit_expr).ok_or_else(|| {
                    GraphcalError::UnknownUnit {
                        name: u.name.value.clone(),
                        src: src.clone(),
                        span: def.span.into(),
                    }
                })?;
            UnitScale::Static(eval_scale_expr(&def.scale_expr, src)? * base_scale)
        }
    } else {
        UnitScale::Static(1.0)
    };
    // If this is a base unit (scale=1, no definition) for a single
    // base dimension, record the unit name as the SI symbol for
    // that dimension. This handles user-defined dimensions like
    // `unit bit: Information;` → symbol "bit" for Information.
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
    registry.register_unit_dynamic(u.name.value.clone(), dim, scale);
    Ok(())
}

/// Resolve the static scale factor of the base unit expression in a unit definition.
///
/// For `unit EUR: Money = (@rate) USD;`, the base unit expr is `USD` with scale 1.0.
/// The base unit itself must be static (not dynamic).
fn resolve_base_unit_static_scale(
    registry: &RegistryBuilder,
    unit_expr: &crate::desugar::resolved_ast::UnitExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<f64, GraphcalError> {
    let (_dim, base_scale) =
        registry
            .resolve_unit_expr(unit_expr)
            .ok_or_else(|| GraphcalError::UnknownUnit {
                name: format_unit_expr(unit_expr).into(),
                src: src.clone(),
                span: unit_expr.span.into(),
            })?;
    Ok(base_scale)
}

/// Check if an expression contains any `@`-references (graph refs).
fn contains_graph_ref(expr: &Expr) -> bool {
    crate::ir::resolve::contains_graph_ref(expr)
}

/// Build a map of dynamic unit name → set of `@`-reference names from the registry.
///
/// For each dynamic unit, extracts the graph refs from its `scale_expr`.
/// Returns an empty map if no dynamic units exist.
fn build_dynamic_unit_deps(registry: &RegistryBuilder) -> HashMap<String, HashSet<String>> {
    let mut dynamic_deps: HashMap<String, HashSet<String>> = HashMap::new();

    for (name, _dim, scale) in registry.all_units() {
        if let UnitScale::Dynamic { scale_expr, .. } = scale {
            let mut refs = HashSet::new();
            crate::ir::resolve::collect_graph_ref_names(scale_expr, &mut refs);
            if !refs.is_empty() {
                dynamic_deps.insert(name.to_string(), refs);
            }
        }
    }

    dynamic_deps
}

/// Visitor that collects all unit names referenced by `UnitLiteral` and `Convert` nodes.
struct UnitNameCollector {
    unit_names: HashSet<String>,
}

impl ExprVisitor<crate::syntax::phase::Resolved> for UnitNameCollector {
    type Error = std::convert::Infallible;

    fn visit_leaf(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::UnitLiteral { unit, .. } = &expr.kind {
            for term in &unit.terms {
                self.unit_names.insert(term.name.value.to_string());
            }
        }
        Ok(())
    }

    fn visit_single_child(&mut self, expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        // Collect unit names from Convert targets
        if let ExprKind::Convert { target, .. } = &expr.kind {
            for term in &target.terms {
                self.unit_names.insert(term.name.value.to_string());
            }
        }
        // Continue recursion into the inner expression
        self.visit_expr(inner)
    }
}

/// Augment `runtime_deps` with transitive dependencies through dynamic units.
///
/// When a param/node expression references a dynamic unit (via `UnitLiteral` or
/// `Convert`), the `@`-references in that unit's scale expression become implicit
/// dependencies of the param/node. This ensures correct topological ordering:
/// the params referenced by dynamic unit scales are evaluated before any
/// node/param that uses the dynamic unit.
fn augment_runtime_deps_for_dynamic_units(
    runtime_deps: &mut HashMap<ScopedName, HashSet<ScopedName>>,
    dynamic_unit_deps: &HashMap<String, HashSet<String>>,
    params: &[crate::registry::resolve_types::ResolvedParamEntry],
    nodes: &[crate::registry::resolve_types::ResolvedNodeEntry],
) {
    if dynamic_unit_deps.is_empty() {
        return;
    }

    // For each param with a default expression, check for dynamic unit references.
    // Resolved param/node names are bare locals at this stage of the resolver.
    for param in params {
        if let Some(expr) = &param.default_expr {
            let extra_deps = collect_dynamic_unit_deps_from_expr(expr, dynamic_unit_deps);
            if !extra_deps.is_empty() {
                runtime_deps
                    .entry(ScopedName::local(param.name.as_str()))
                    .or_default()
                    .extend(extra_deps.into_iter().map(ScopedName::local));
            }
        }
    }

    // For each node, check for dynamic unit references
    for node in nodes {
        let extra_deps = collect_dynamic_unit_deps_from_expr(&node.expr, dynamic_unit_deps);
        if !extra_deps.is_empty() {
            runtime_deps
                .entry(ScopedName::local(node.name.as_str()))
                .or_default()
                .extend(extra_deps.into_iter().map(ScopedName::local));
        }
    }
}

/// Collect transitive `@`-dependencies from dynamic units referenced in an expression.
fn collect_dynamic_unit_deps_from_expr(
    expr: &Expr,
    dynamic_unit_deps: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let mut collector = UnitNameCollector {
        unit_names: HashSet::new(),
    };
    let _ = collector.visit_expr(expr);

    let mut extra_deps = HashSet::new();
    for unit_name in &collector.unit_names {
        if let Some(deps) = dynamic_unit_deps.get(unit_name) {
            extra_deps.extend(deps.iter().cloned());
        }
    }
    extra_deps
}

/// Recursively scan a type expression for nat literals in index position
/// and register the corresponding synthetic nat range indexes in the registry.
fn collect_nat_ranges_from_type_expr(
    type_expr: &crate::desugar::resolved_ast::TypeExpr,
    registry: &mut RegistryBuilder,
) {
    if let crate::desugar::resolved_ast::TypeExprKind::Indexed { base, indexes } = &type_expr.kind {
        collect_nat_ranges_from_type_expr(base, registry);
        for idx in indexes {
            match idx {
                crate::desugar::resolved_ast::IndexExpr::NatLiteral(n, _) => {
                    registry.ensure_nat_range_index(*n);
                }
                crate::desugar::resolved_ast::IndexExpr::NatExpr(nat_expr) => {
                    collect_nat_range_literals_from_nat_expr(nat_expr, registry);
                }
                crate::desugar::resolved_ast::IndexExpr::Name(_) => {}
            }
        }
    }
    if let crate::desugar::resolved_ast::TypeExprKind::TypeApplication { type_args, .. } =
        &type_expr.kind
    {
        for arg in type_args {
            collect_nat_ranges_from_type_expr(arg, registry);
        }
    }
}

/// Collect nat range literal values from a `NatExpr` tree.
///
/// Only literal-only expressions can be registered at compile time;
/// expressions containing variables are resolved at call sites.
fn collect_nat_range_literals_from_nat_expr(
    expr: &crate::desugar::resolved_ast::NatExpr,
    registry: &mut RegistryBuilder,
) {
    use crate::desugar::resolved_ast::NatExpr;
    match expr {
        NatExpr::Literal(n, _) => {
            registry.ensure_nat_range_index(*n);
        }
        NatExpr::Var(_) => {}
        NatExpr::Add(lhs, rhs, _) | NatExpr::Mul(lhs, rhs, _) => {
            collect_nat_range_literals_from_nat_expr(lhs, registry);
            collect_nat_range_literals_from_nat_expr(rhs, registry);
        }
    }
}

/// Recursively scan an expression for `for i: range(N)` and register
/// nat range indexes for concrete nat literals.
fn collect_nat_ranges_from_expr(
    expr: &crate::desugar::resolved_ast::Expr,
    registry: &mut RegistryBuilder,
) {
    use crate::desugar::resolved_ast::{ExprKind, ForBindingIndex};

    // Use the visitor trait to walk all sub-expressions
    struct NatRangeCollector<'a> {
        registry: &'a mut RegistryBuilder,
    }

    impl crate::syntax::visitor::ExprVisitor<crate::syntax::phase::Resolved> for NatRangeCollector<'_> {
        type Error = GraphcalError;

        fn visit_expr(
            &mut self,
            expr: &crate::desugar::resolved_ast::Expr,
        ) -> Result<(), GraphcalError> {
            match &expr.kind {
                ExprKind::ForComp { bindings, .. } => {
                    for binding in bindings {
                        if let ForBindingIndex::Range { arg, .. } = &binding.index {
                            collect_nat_range_literals_from_nat_expr(arg, self.registry);
                        }
                    }
                }
                ExprKind::MapLiteral { entries } => {
                    // After TableLiteral desugaring, NatRange axes survive as
                    // synthetic `__nat_range_N` index names in entry keys.
                    // Recover and register N here so the registry knows
                    // about every range used by the file.
                    for entry in entries {
                        for key in &entry.keys {
                            if let Some(n) = crate::registry::types::parse_nat_range_index_name(
                                key.index.value.as_str(),
                            ) {
                                self.registry.ensure_nat_range_index(n);
                            }
                        }
                    }
                }
                _ => {}
            }
            self.dispatch(expr)
        }
    }

    let mut collector = NatRangeCollector { registry };
    let _ = collector.visit_expr(expr);
}

fn register_index_decl(
    idx: &crate::desugar::resolved_ast::IndexDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<(), GraphcalError> {
    let kind = match &idx.kind {
        crate::desugar::resolved_ast::IndexDeclKind::Named { variants } => {
            types::IndexKind::Named {
                variants: variants.iter().map(|v| v.value.clone()).collect(),
            }
        }
        crate::desugar::resolved_ast::IndexDeclKind::Range {
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
        crate::desugar::resolved_ast::IndexDeclKind::RequiredNamed => {
            types::IndexKind::RequiredNamed
        }
        crate::desugar::resolved_ast::IndexDeclKind::RequiredRange { dimension } => {
            let dim = registry.resolve_dim_expr(dimension).ok_or_else(|| {
                GraphcalError::UnknownDimension {
                    name: crate::syntax::names::DimName::new(idx.name.value.as_str()),
                    src: src.clone(),
                    span: dimension.span.into(),
                }
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

fn register_type_decl(t: &crate::desugar::resolved_ast::TypeDecl, registry: &mut RegistryBuilder) {
    let generic_params: Vec<types::TypeGenericParam> = t
        .generic_params
        .iter()
        .map(|g| types::TypeGenericParam {
            name: g.name.value.clone(),
            constraint: g.constraint.into(),
            default: g.default.clone(),
        })
        .collect();

    // Required types (`type T;` with no body) are treated like opaque
    // unit types at the library level; include-time substitution rewires
    // references through the importer's type bindings (see plan §C1).
    let kind = match &t.fields {
        None => types::TypeDefKind::Unit,
        Some(fields) if fields.is_empty() => types::TypeDefKind::Unit,
        Some(fields) => {
            let fields = fields
                .iter()
                .map(|f| types::StructField {
                    name: f.name.value.clone(),
                    type_ann: f.type_ann.clone(),
                })
                .collect();
            types::TypeDefKind::Record { fields }
        }
    };

    registry.register_type(types::TypeDef {
        name: t.name.value.clone(),
        generic_params,
        kind,
    });
}

fn register_union_type_decl(
    t: &crate::desugar::resolved_ast::UnionTypeDecl,
    registry: &mut RegistryBuilder,
) {
    let generic_params: Vec<types::TypeGenericParam> = t
        .generic_params
        .iter()
        .map(|g| types::TypeGenericParam {
            name: g.name.value.clone(),
            constraint: g.constraint.into(),
            default: g.default.clone(),
        })
        .collect();

    let members = t
        .members
        .iter()
        .map(|m| types::UnionMemberDef {
            name: m.name.value.clone(),
            type_args: m.type_args.clone(),
        })
        .collect();

    registry.register_type(types::TypeDef {
        name: t.name.value.clone(),
        generic_params,
        kind: types::TypeDefKind::Union { members },
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
        ExprKind::ConstRef(ident) => match ident.value.member() {
            "PI" if !ident.value.is_qualified() => Ok(std::f64::consts::PI),
            "E" if !ident.value.is_qualified() => Ok(std::f64::consts::E),
            _ => Err(GraphcalError::EvalError {
                message: format!(
                    "unknown constant `{}` in scale expression; only `PI` and `E` are supported",
                    ident.value
                ),
                src: src.clone(),
                span: ident.span.into(),
            }),
        },
        ExprKind::BinOp { op, lhs, rhs } => {
            use crate::desugar::resolved_ast::BinOp;
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
            op: crate::desugar::resolved_ast::UnaryOp::Neg,
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

    match &expr.kind {
        ExprKind::Number(n) => Ok((*n, Dimension::dimensionless())),
        ExprKind::UnitLiteral { value, unit } => {
            let (dim, scale) =
                registry
                    .resolve_unit_expr(unit)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: "unknown unit in range expression".to_string(),
                        src: src.clone(),
                        span: unit.span.into(),
                    })?;
            Ok((*value * scale, dim))
        }
        ExprKind::UnaryOp {
            op: crate::desugar::resolved_ast::UnaryOp::Neg,
            operand,
        } => {
            let (val, dim) = eval_range_expr(operand, registry, src)?;
            Ok((-val, dim))
        }
        _ => Err(GraphcalError::EvalError {
            message: "range expression must be a numeric or unit literal".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
    }
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

    // Extract display unit from the start expression's unit annotation.
    let (display_label, display_scale) = match &start_expr.kind {
        ExprKind::UnitLiteral { unit, .. } => {
            if let Some((_dim, scale)) = registry.resolve_unit_expr(unit) {
                (Some(format_unit_expr(unit)), scale)
            } else {
                (None, 1.0)
            }
        }
        _ => (None, 1.0),
    };

    Ok(types::IndexKind::Range(types::RangeIndexData {
        start: start_val,
        end: end_val,
        step: step_val,
        dimension: start_dim,
        display_label,
        display_scale,
    }))
}

/// Extract a map of type annotations from const/param/node declarations.
fn extract_type_annotations(ast: &File) -> HashMap<String, TypeExpr> {
    let mut type_anns = HashMap::new();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::Param(p) => {
                type_anns.insert(p.name.value.to_string(), p.type_ann.clone());
            }
            DeclKind::Node(n) => {
                type_anns.insert(n.name.value.to_string(), n.type_ann.clone());
            }
            DeclKind::ConstNode(c) => {
                type_anns.insert(c.name.value.to_string(), c.type_ann.clone());
            }
            _ => {}
        }
    }
    type_anns
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]
    use super::*;
    use crate::syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<IR, GraphcalError> {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let mut desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        crate::syntax::ast::desugar_tuple_matches(&mut desugared);
        let file = crate::syntax::name_resolve::resolve_name_refs(desugared);
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
        assert!(ir.registry.units.get_unit("km").is_some());
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
        // it (see the CLI tests), but single-file IR lowering rejects it:
        // include expansion is a higher-phase concern, so `@transfer`
        // (the include's projected node) remains unresolved at this layer.
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
    fn lower_deps_extracted() {
        let ir = parse_and_lower(
            "param a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;\nnode c: Dimensionless = @a + @b;",
        )
        .unwrap();
        let c_deps = &ir.runtime_deps[&ScopedName::local("c")];
        assert!(c_deps.contains(&ScopedName::local("a")));
        assert!(c_deps.contains(&ScopedName::local("b")));
        assert_eq!(c_deps.len(), 2);
    }
}

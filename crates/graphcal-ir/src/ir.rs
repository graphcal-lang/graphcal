//! Intermediate Representation (IR) — the result of lowering an AST.
//!
//! `lower()` combines name resolution (`resolve`), registry construction
//! (dimensions, units, indexes, structs), and function registration into a
//! single `IR` value that downstream stages can consume without reaching
//! back to the raw AST.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{
    AssertBody, DeclKind, Expr, ExprKind, FigureDecl, File, FnDecl, LayerDecl, PlotDecl, TypeExpr,
};
use graphcal_syntax::dimension::Rational;
use graphcal_syntax::names::{DeclName, DimName, FnName};
use graphcal_syntax::span::Span;
use graphcal_syntax::visitor::{ExprVisitor, ExprVisitorMut};

use crate::resolve::{
    DeclCategory, ExpectedFail, ImportedValueNames, ResolvedFile, resolve_with_imported_values,
};
use crate::resolve::{ImportedNames, resolve_with_imports};
use graphcal_registry::declared_type::DeclaredType;
use graphcal_registry::error::GraphcalError;
use graphcal_registry::format::format_unit_expr;
use graphcal_registry::prelude::load_prelude;
use graphcal_registry::registry::{self, Registry, RegistryBuilder};
use graphcal_registry::resolve_types::ScopedName;
use graphcal_registry::runtime_value::RuntimeValue;

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
    pub hidden: bool,
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

/// A function declaration.
#[derive(Debug, Clone)]
pub struct FunctionEntry {
    pub name: ScopedName,
    pub decl: FnDecl,
    pub span: Span,
}

/// Intermediate Representation produced by [`lower`].
///
/// Contains everything downstream stages need:
/// - A `Registry` with dimensions, units, indexes, structs, and functions
/// - Declarations (consts, params, nodes) with their expressions
/// - Dependency graphs for const and runtime evaluation ordering
/// - Source-order tracking for deterministic output
/// - User-defined function declarations
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
    pub runtime_deps: HashMap<ScopedName, HashSet<ScopedName>>,
    /// For each const, the set of const-references (const deps).
    pub const_deps: HashMap<ScopedName, HashSet<ScopedName>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// User-defined function declarations.
    pub functions: Vec<FunctionEntry>,
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
}

/// Convert a `String`-keyed dep map from the resolver to a `ScopedName`-keyed map.
fn wrap_dep_map(map: HashMap<String, HashSet<String>>) -> HashMap<ScopedName, HashSet<ScopedName>> {
    map.into_iter()
        .map(|(k, v)| {
            (
                ScopedName::local(k),
                v.into_iter().map(ScopedName::local).collect(),
            )
        })
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
    lower_with_imports(ast, src, &ImportedNames::default())
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
) -> Result<IR, GraphcalError> {
    let (builder, resolved_ir) = lower_to_builder(ast, src, imported)?;
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
#[expect(
    clippy::too_many_lines,
    reason = "will be decomposed in a later refactor phase"
)]
pub fn lower_to_builder(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Name resolution
    let resolved = resolve_with_imports(ast, src, imported)?;

    // Step 2: Build registry (prelude + user-declared dimensions/units/indexes/structs)
    let mut builder = RegistryBuilder::new();
    load_prelude(&mut builder);
    register_file_declarations(ast, &mut builder, src)?;

    // Step 3: Register user-defined functions
    register_functions(&resolved, &mut builder, src)?;

    // Step 4: Extract type annotations from the AST and pair with resolved declarations.
    // Build a map from declaration name to TypeExpr.
    let mut type_anns: HashMap<String, TypeExpr> = HashMap::new();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::Const(c) => {
                type_anns.insert(c.name.value.to_string(), c.type_ann.clone());
            }
            DeclKind::Param(p) => {
                type_anns.insert(p.name.value.to_string(), p.type_ann.clone());
            }
            DeclKind::Node(n) => {
                type_anns.insert(n.name.value.to_string(), n.type_ann.clone());
            }
            _ => {}
        }
    }
    // Also extract type annotations from imported declarations.
    for (name, type_ann, _, _) in &imported.consts {
        type_anns.insert(name.clone(), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.params {
        type_anns.insert(name.clone(), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.nodes {
        type_anns.insert(name.clone(), type_ann.clone());
    }

    let consts = resolved
        .consts
        .into_iter()
        .map(|entry| {
            let type_ann =
                type_anns
                    .remove(&entry.name)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("internal: missing type annotation for `{}`", entry.name),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
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
            let type_ann =
                type_anns
                    .remove(&entry.name)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("internal: missing type annotation for `{}`", entry.name),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
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
            let type_ann =
                type_anns
                    .remove(&entry.name)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("internal: missing type annotation for `{}`", entry.name),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
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
            .map(|entry| PlotEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                span: entry.span,
                hidden: entry.hidden,
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
            .map(|(name, cat)| (ScopedName::local(name), cat))
            .collect(),
        functions: resolved
            .functions
            .into_iter()
            .map(|entry| FunctionEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                span: entry.span,
            })
            .collect(),
        assert_names: resolved
            .assert_names
            .into_iter()
            .map(ScopedName::local)
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
        imported_values: HashMap::new(),
    };

    Ok((builder, unfrozen))
}

/// Lower an AST with pre-evaluated imported values, returning a `RegistryBuilder`
/// that can be further mutated before freezing.
///
/// Unlike [`lower_to_builder`], this uses [`resolve_with_imported_values`] which
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
#[expect(
    clippy::too_many_lines,
    reason = "will be decomposed in a later refactor phase"
)]
pub fn lower_to_builder_with_imported_values(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Name resolution with imported value names in scope
    let resolved = resolve_with_imported_values(ast, src, imported_names)?;

    // Step 2: Build registry (prelude + user-declared dimensions/units/indexes/structs)
    let mut builder = RegistryBuilder::new();
    load_prelude(&mut builder);
    register_file_declarations(ast, &mut builder, src)?;

    // Step 3: Register user-defined functions (including imported functions)
    register_functions(&resolved, &mut builder, src)?;

    // Step 4: Extract type annotations from local declarations only.
    let mut type_anns: HashMap<String, TypeExpr> = HashMap::new();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::Const(c) => {
                type_anns.insert(c.name.value.to_string(), c.type_ann.clone());
            }
            DeclKind::Param(p) => {
                type_anns.insert(p.name.value.to_string(), p.type_ann.clone());
            }
            DeclKind::Node(n) => {
                type_anns.insert(n.name.value.to_string(), n.type_ann.clone());
            }
            _ => {}
        }
    }

    let consts = resolved
        .consts
        .into_iter()
        .map(|entry| {
            let type_ann =
                type_anns
                    .remove(&entry.name)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("internal: missing type annotation for `{}`", entry.name),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
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
            let type_ann =
                type_anns
                    .remove(&entry.name)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("internal: missing type annotation for `{}`", entry.name),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
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
            let type_ann =
                type_anns
                    .remove(&entry.name)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: format!("internal: missing type annotation for `{}`", entry.name),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
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
            .map(|entry| PlotEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                span: entry.span,
                hidden: entry.hidden,
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
            .map(|(name, cat)| (ScopedName::local(name), cat))
            .collect(),
        functions: resolved
            .functions
            .into_iter()
            .map(|entry| FunctionEntry {
                name: ScopedName::local(entry.name),
                decl: entry.decl,
                span: entry.span,
            })
            .collect(),
        assert_names: resolved
            .assert_names
            .into_iter()
            .map(ScopedName::local)
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
    runtime_deps: HashMap<ScopedName, HashSet<ScopedName>>,
    const_deps: HashMap<ScopedName, HashSet<ScopedName>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(ScopedName, DeclCategory)>,
    /// User-defined function declarations.
    pub functions: Vec<FunctionEntry>,
    assert_names: HashSet<ScopedName>,
    assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    expected_fail: HashMap<ScopedName, ExpectedFail>,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
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
            functions: self.functions,
            assert_names: self.assert_names,
            assumes_map: self.assumes_map,
            expected_fail: self.expected_fail,
            imported_values: self.imported_values,
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
        let mut deps = HashSet::new();
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
        let mut deps = HashSet::new();
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
    pub fn merge_dependency(
        &mut self,
        dep: Self,
        prefix: &str,
        bindings: &HashMap<String, Expr>,
        dep_names: &HashSet<String>,
    ) {
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
            prefix_expr_refs(&mut entry.expr, prefix, dep_names);
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
                // Keep default, but prefix internal refs
                prefix_expr_refs(expr, prefix, dep_names);
            } else {
                // Required param without binding — stays None, caught later in exec_plan
            }
            // Rebuild runtime deps for the (possibly rewritten) expression
            let mut graph_refs = HashSet::new();
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
            prefix_expr_refs(&mut entry.expr, prefix, dep_names);
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
                graphcal_syntax::ast::AssertBody::Expr(e) => {
                    prefix_expr_refs(e, prefix, dep_names);
                }
                graphcal_syntax::ast::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    ..
                } => {
                    prefix_expr_refs(actual, prefix, dep_names);
                    prefix_expr_refs(expected, prefix, dep_names);
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
                prefix_expr_refs(&mut encoding.value, prefix, dep_names);
            }
            for prop in &mut entry.decl.mark.properties {
                prefix_expr_refs(&mut prop.value, prefix, dep_names);
            }
            for prop in &mut entry.decl.properties {
                prefix_expr_refs(&mut prop.value, prefix, dep_names);
            }
            let prefixed = entry.name.with_prefix(prefix);
            self.plots.push(PlotEntry {
                name: prefixed.clone(),
                decl: entry.decl,
                span: entry.span,
                hidden: entry.hidden,
            });
            self.source_order.push((prefixed, DeclCategory::Plot));
        }

        // Merge figures
        for mut entry in dep.figures {
            for field in &mut entry.decl.fields {
                prefix_expr_refs(&mut field.value, prefix, dep_names);
            }
            // Prefix plot names referenced by the figure
            for plot_name in &mut entry.decl.plot_names {
                if dep_names.contains(plot_name.value.as_str()) {
                    plot_name.value = graphcal_syntax::names::DeclName::new(format!(
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
                prefix_expr_refs(&mut field.value, prefix, dep_names);
            }
            // Prefix plot names referenced by the layer
            for plot_name in &mut entry.decl.plot_names {
                if dep_names.contains(plot_name.value.as_str()) {
                    plot_name.value = graphcal_syntax::names::DeclName::new(format!(
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

        // Merge functions
        for mut entry in dep.functions {
            match &mut entry.decl.body {
                graphcal_syntax::ast::FnBody::Short(e) => {
                    prefix_expr_refs(e, prefix, dep_names);
                }
                graphcal_syntax::ast::FnBody::Block { stmts, expr } => {
                    for stmt in stmts {
                        prefix_expr_refs(&mut stmt.value, prefix, dep_names);
                    }
                    prefix_expr_refs(expr, prefix, dep_names);
                }
            }
            let prefixed = entry.name.with_prefix(prefix);
            self.functions.push(FunctionEntry {
                name: prefixed,
                decl: entry.decl,
                span: entry.span,
            });
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
            self.expected_fail.insert(prefixed, ef);
        }
    }
}

/// Visitor that prefixes references to dependency declarations.
struct RefPrefixer<'a> {
    prefix: &'a str,
    dep_names: &'a HashSet<String>,
}

impl ExprVisitorMut for RefPrefixer<'_> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &mut expr.kind
            && self.dep_names.contains(ident.value.as_str())
        {
            ident.value = DeclName::new(format!("{}::{}", self.prefix, ident.value));
        }
        Ok(())
    }

    fn visit_const_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::ConstRef(ident) = &mut expr.kind
            && self.dep_names.contains(ident.value.as_str())
        {
            ident.value = DeclName::new(format!("{}::{}", self.prefix, ident.value));
        }
        Ok(())
    }

    fn visit_fn_call_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::FnCall { name, args } = &mut expr.kind {
            if self.dep_names.contains(name.value.as_str()) {
                name.value = FnName::new(format!("{}::{}", self.prefix, name.value));
            }
            for arg in args {
                self.visit_expr_mut(arg)?;
            }
        }
        Ok(())
    }

    // Qualified refs and leaf nodes don't need rewriting (handled by default no-ops)
}

/// Rewrite `@`-references and const/fn references within an expression to use
/// prefixed names, but only for names that belong to the dependency.
///
/// For example, `GraphRef("dry_mass")` becomes `GraphRef("r::dry_mass")` when
/// `"dry_mass"` is in `dep_names` and `prefix` is `"r"`.
///
/// Built-in names and names from the importer's scope are left unchanged.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn prefix_expr_refs(expr: &mut Expr, prefix: &str, dep_names: &HashSet<String>) {
    let mut prefixer = RefPrefixer { prefix, dep_names };
    let _ = prefixer.visit_expr_mut(expr);
}

/// Visitor that collects graph references from expressions.
struct GraphRefCollector<'a> {
    refs: &'a mut HashSet<ScopedName>,
}

impl ExprVisitor for GraphRefCollector<'_> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind {
            self.refs.insert(ScopedName::local(ident.value.as_str()));
        }
        Ok(())
    }

    fn visit_qualified_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedGraphRef { module, name } = &expr.kind {
            self.refs
                .insert(ScopedName::qualified(&module.name, name.value.as_str()));
        }
        Ok(())
    }
}

/// Collect all `@`-referenced names from an expression (non-recursive into child scopes).
///
/// This is a simpler version of `resolve::collect_graph_refs` that operates on
/// arbitrary expressions without requiring a known-names set. Used for building
/// runtime deps from binding expressions.
fn collect_graph_refs_from_expr(expr: &Expr, refs: &mut HashSet<ScopedName>) {
    let mut collector = GraphRefCollector { refs };
    let _ = collector.visit_expr(expr);
}

/// Register dimensions, units, indexes, and struct types from a file's declarations
/// into the registry.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
pub fn register_file_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let file_path = std::path::PathBuf::from(src.name());
    register_declarations_impl(file, registry, src, None, &file_path)
}

/// Register only the named type-system declarations (dimensions, units, indexes, types)
/// from a file into the registry.
///
/// This is the selective counterpart to [`register_file_declarations`]: instead of
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
) -> Result<(), GraphcalError> {
    let file_path = std::path::PathBuf::from(src.name());
    register_declarations_impl(file, registry, src, Some(names), &file_path)
}

/// Shared implementation for registering type-system declarations.
///
/// When `filter` is `None`, all declarations are registered.
/// When `filter` is `Some(names)`, only declarations whose names are in `names` are registered.
fn register_declarations_impl(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    filter: Option<&std::collections::HashSet<String>>,
    file_path: &std::path::Path,
) -> Result<(), GraphcalError> {
    let should_register = |name: &str| filter.is_none_or(|names| names.contains(name));

    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(d) if should_register(d.name.value.as_str()) => {
                register_dimension_decl(d, registry, src, file_path)?;
            }
            DeclKind::Unit(u) if should_register(u.name.value.as_str()) => {
                register_unit_decl(u, registry, src)?;
            }
            DeclKind::Index(idx) if should_register(idx.name.value.as_str()) => {
                register_index_decl(idx, registry, src, decl.span)?;
            }
            DeclKind::Type(t) if should_register(t.name.value.as_str()) => {
                register_type_decl(t, &decl.attributes, registry);
            }
            _ => {}
        }
    }
    Ok(())
}

fn register_dimension_decl(
    d: &graphcal_syntax::ast::DimDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    file_path: &std::path::Path,
) -> Result<(), GraphcalError> {
    if let Some(def) = &d.definition {
        // Derived dimension — resolve the expression
        let dim =
            registry
                .resolve_dim_expr(def)
                .ok_or_else(|| GraphcalError::UnknownDimension {
                    name: d.name.value.clone(),
                    src: src.clone(),
                    span: d.name.span.into(),
                })?;
        registry.register_dimension(d.name.value.clone(), dim);
    } else {
        // Base dimension — register a new orthogonal axis
        let dim_id = graphcal_syntax::dimension::BaseDimId::UserDefined {
            file: file_path.to_path_buf(),
            name: d.name.value.to_string(),
        };
        registry.register_base_dimension(d.name.value.clone(), dim_id);
    }
    Ok(())
}

fn register_unit_decl(
    u: &graphcal_syntax::ast::UnitDecl,
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
        let (_unit_dim, base_scale) =
            registry.resolve_unit_expr(&def.unit_expr).ok_or_else(|| {
                GraphcalError::UnknownUnit {
                    name: u.name.value.clone(),
                    src: src.clone(),
                    span: def.span.into(),
                }
            })?;
        eval_scale_expr(&def.scale_expr, src)? * base_scale
    } else {
        1.0
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
    registry.register_unit(u.name.value.clone(), dim, scale);
    Ok(())
}

fn register_index_decl(
    idx: &graphcal_syntax::ast::IndexDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<(), GraphcalError> {
    let kind = match &idx.kind {
        graphcal_syntax::ast::IndexDeclKind::Named { variants } => registry::IndexKind::Named {
            variants: variants.iter().map(|v| v.value.clone()).collect(),
        },
        graphcal_syntax::ast::IndexDeclKind::Range {
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
    };
    registry.register_index(registry::IndexDef {
        name: idx.name.value.clone(),
        kind,
    });
    Ok(())
}

fn register_type_decl(
    t: &graphcal_syntax::ast::TypeDecl,
    attributes: &[graphcal_syntax::ast::Attribute],
    registry: &mut RegistryBuilder,
) {
    let generic_params: Vec<registry::TypeGenericParam> = t
        .generic_params
        .iter()
        .map(|g| registry::TypeGenericParam {
            name: g.name.value.clone(),
            constraint: g.constraint.into(),
            default: g.default.clone(),
        })
        .collect();
    let mut variants = Vec::new();
    for variant in &t.variants {
        let mut fields = Vec::new();
        for field in &variant.fields {
            fields.push(registry::StructField {
                name: field.name.value.clone(),
                type_ann: field.type_ann.clone(),
            });
        }
        variants.push(registry::VariantDef {
            name: variant.name.value.clone(),
            fields,
        });
    }

    // Extract derives from attributes (validated by resolver)
    let derives: Vec<graphcal_syntax::ast::DeriveOp> = attributes
        .iter()
        .filter(|a| a.name.name == "derive")
        .flat_map(|a| a.args.iter())
        .filter_map(|arg| {
            arg.as_single_ident()
                .and_then(|ident| match ident.name.as_str() {
                    "Add" => Some(graphcal_syntax::ast::DeriveOp::Add),
                    "Sub" => Some(graphcal_syntax::ast::DeriveOp::Sub),
                    "Neg" => Some(graphcal_syntax::ast::DeriveOp::Neg),
                    _ => None,
                })
        })
        .collect();

    registry.register_type(registry::TypeDef {
        name: t.name.value.clone(),
        generic_params,
        derives,
        variants,
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
        ExprKind::ConstRef(ident) => match ident.value.as_str() {
            "PI" => Ok(std::f64::consts::PI),
            "E" => Ok(std::f64::consts::E),
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
            use graphcal_syntax::ast::BinOp;
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
            op: graphcal_syntax::ast::UnaryOp::Neg,
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
) -> Result<(f64, graphcal_syntax::dimension::Dimension), GraphcalError> {
    use graphcal_syntax::dimension::Dimension;

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
            op: graphcal_syntax::ast::UnaryOp::Neg,
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
    name: &graphcal_syntax::names::IndexName,
    start_expr: &Expr,
    end_expr: &Expr,
    step_expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: graphcal_syntax::span::Span,
) -> Result<registry::IndexKind, GraphcalError> {
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

    Ok(registry::IndexKind::Range {
        start: start_val,
        end: end_val,
        step: step_val,
        dimension: start_dim,
        display_label,
        display_scale,
    })
}

/// Register user-defined functions from a [`ResolvedFile`] into the registry builder.
fn register_functions(
    resolved: &ResolvedFile,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for entry in &resolved.functions {
        registry.register_function(registry::FnDef {
            name: FnName::new(&entry.name),
            generic_params: entry
                .decl
                .generic_params
                .iter()
                .map(|g| {
                    let constraint = match g.constraint {
                        graphcal_syntax::ast::GenericConstraint::Dim => {
                            registry::FnGenericConstraint::Dim
                        }
                        graphcal_syntax::ast::GenericConstraint::Index => {
                            registry::FnGenericConstraint::Index
                        }
                        graphcal_syntax::ast::GenericConstraint::Type => {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "internal: `Type` constraint is not valid on function generic parameter `{}`",
                                    g.name.value
                                ),
                                src: src.clone(),
                                span: g.name.span.into(),
                            });
                        }
                    };
                    Ok(registry::FnGenericParam {
                        name: g.name.value.clone(),
                        constraint,
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()?,
            params: entry
                .decl
                .params
                .iter()
                .map(|p| registry::FnParamDef {
                    name: p.name.name.clone(),
                    type_expr: p.type_ann.clone(),
                })
                .collect(),
            return_type_expr: entry.decl.return_type.clone(),
            body: entry.decl.body.clone(),
            span: entry.span,
        });
    }
    Ok(())
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
    use graphcal_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<IR, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        lower(&file, &make_src(source))
    }

    #[test]
    fn lower_rocket() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.consts.len(), 1); // G0
        assert_eq!(ir.params.len(), 3); // dry_mass, fuel_mass, isp
        assert_eq!(ir.nodes.len(), 3); // v_exhaust, mass_ratio, delta_v
        assert!(ir.registry.dimensions.get_dimension("Length").is_some());
        assert!(ir.registry.units.get_unit("km").is_some());
    }

    #[test]
    fn lower_constants() {
        let source = include_str!("../../../tests/fixtures/constants.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.consts.len(), 4);
        assert_eq!(ir.params.len(), 1);
        assert_eq!(ir.nodes.len(), 2);
    }

    #[test]
    fn lower_functions() {
        let source = include_str!("../../../tests/fixtures/functions.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(!ir.functions.is_empty());
        // Functions should be registered in the registry
        assert!(
            ir.registry
                .functions
                .get_function("orbital_velocity")
                .is_some()
        );
    }

    #[test]
    fn lower_indexed() {
        let source = include_str!("../../../tests/fixtures/indexed.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(ir.registry.indexes.get_index("Maneuver").is_some());
    }

    #[test]
    fn lower_hohmann() {
        let source = include_str!("../../../tests/fixtures/hohmann.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(ir.registry.types.get_type("TransferResult").is_some());
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

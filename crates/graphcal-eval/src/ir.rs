//! Intermediate Representation (IR) — the result of lowering an AST.
//!
//! `lower()` combines name resolution (`resolve`), registry construction
//! (dimensions, units, indexes, structs), and function registration into a
//! single `IR` value that downstream stages can consume without reaching
//! back to the raw AST.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{AssertBody, DeclKind, Expr, ExprKind, File, FnDecl, TypeExpr};
use graphcal_syntax::dimension::Rational;
use graphcal_syntax::names::{DeclName, DimName, FnName};
use graphcal_syntax::span::Span;

use crate::dim_check::DeclaredType;
use crate::error::GraphcalError;
use crate::eval::format_unit_expr;
use crate::eval_expr::RuntimeValue;
use crate::prelude::load_prelude;
use crate::registry::{self, Registry, RegistryBuilder};
use crate::resolve::{
    DeclCategory, ExpectedFail, ImportedValueNames, ResolvedFile, resolve_with_imported_values,
};
#[cfg(test)]
use crate::resolve::{ImportedNames, resolve_with_imports};

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
    /// Const declarations in source order: (name, `type_ann`, expr, span).
    pub consts: Vec<(String, TypeExpr, Expr, Span)>,
    /// Param declarations in source order: (name, `type_ann`, expr, span).
    pub params: Vec<(String, TypeExpr, Expr, Span)>,
    /// Node declarations in source order: (name, `type_ann`, expr, span).
    pub nodes: Vec<(String, TypeExpr, Expr, Span)>,
    /// Assert declarations in source order: (name, body, span).
    pub asserts: Vec<(String, AssertBody, Span)>,
    /// For each param/node, the set of `@`-references (runtime deps).
    pub runtime_deps: HashMap<String, HashSet<String>>,
    /// For each const, the set of const-references (const deps).
    pub const_deps: HashMap<String, HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, FnDecl, Span)>,
    /// Set of all assert names.
    pub assert_names: HashSet<String>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<String, Vec<String>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub expected_fail: HashMap<String, ExpectedFail>,
    /// Pre-evaluated values imported from dependency files.
    /// These are injected directly into the execution plan rather than compiled.
    /// Each entry carries the runtime value and its declared type (for `dim_check`).
    pub imported_values: HashMap<crate::resolve::ScopedName, (RuntimeValue, DeclaredType)>,
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
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let params = resolved
        .params
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let nodes = resolved
        .nodes
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;

    let unfrozen = UnfrozenIR {
        consts,
        params,
        nodes,
        asserts: resolved.asserts,
        runtime_deps: resolved.runtime_deps,
        const_deps: resolved.const_deps,
        source_order: resolved.source_order,
        functions: resolved.functions,
        assert_names: resolved.assert_names,
        assumes_map: resolved.assumes_map,
        expected_fail: resolved.expected_fail,
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
pub fn lower_to_builder_with_imported_values(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported_names: &ImportedValueNames,
    imported_values: HashMap<crate::resolve::ScopedName, (RuntimeValue, DeclaredType)>,
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
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let params = resolved
        .params
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let nodes = resolved
        .nodes
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;

    let unfrozen = UnfrozenIR {
        consts,
        params,
        nodes,
        asserts: resolved.asserts,
        runtime_deps: resolved.runtime_deps,
        const_deps: resolved.const_deps,
        source_order: resolved.source_order,
        functions: resolved.functions,
        assert_names: resolved.assert_names,
        assumes_map: resolved.assumes_map,
        expected_fail: resolved.expected_fail,
        imported_values,
    };

    Ok((builder, unfrozen))
}

/// An IR without a frozen registry, awaiting a call to [`freeze`](Self::freeze).
pub struct UnfrozenIR {
    consts: Vec<(String, TypeExpr, Expr, Span)>,
    params: Vec<(String, TypeExpr, Expr, Span)>,
    nodes: Vec<(String, TypeExpr, Expr, Span)>,
    asserts: Vec<(String, graphcal_syntax::ast::AssertBody, Span)>,
    runtime_deps: HashMap<String, HashSet<String>>,
    const_deps: HashMap<String, HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, graphcal_syntax::ast::FnDecl, Span)>,
    assert_names: HashSet<String>,
    assumes_map: HashMap<String, Vec<String>>,
    expected_fail: HashMap<String, ExpectedFail>,
    imported_values: HashMap<crate::resolve::ScopedName, (RuntimeValue, DeclaredType)>,
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
        name: String,
        type_ann: TypeExpr,
        expr: Expr,
        span: Span,
        target: String,
    ) {
        let mut deps = HashSet::new();
        deps.insert(target);
        self.const_deps.insert(name.clone(), deps);
        self.consts.push((name.clone(), type_ann, expr, span));
        self.source_order.push((name, DeclCategory::Const));
    }

    /// Add a node alias: a synthetic node declaration that references another node/param.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix::delta_v`.
    pub fn add_node_alias(
        &mut self,
        name: String,
        type_ann: TypeExpr,
        expr: Expr,
        span: Span,
        target: String,
    ) {
        let mut deps = HashSet::new();
        deps.insert(target);
        self.runtime_deps.insert(name.clone(), deps);
        self.nodes.push((name.clone(), type_ann, expr, span));
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
        // Merge consts
        for (name, type_ann, mut expr, span) in dep.consts {
            prefix_expr_refs(&mut expr, prefix, dep_names);
            let prefixed = format!("{prefix}::{name}");
            self.consts.push((prefixed.clone(), type_ann, expr, span));
            // Prefix const deps
            if let Some(deps) = dep.const_deps.get(&name) {
                let prefixed_deps = deps
                    .iter()
                    .map(|d| {
                        if dep_names.contains(d) {
                            format!("{prefix}::{d}")
                        } else {
                            d.clone()
                        }
                    })
                    .collect();
                self.const_deps.insert(prefixed.clone(), prefixed_deps);
            }
            self.source_order.push((prefixed, DeclCategory::Const));
        }

        // Merge params — replace defaults with bindings where provided
        for (name, type_ann, mut expr, span) in dep.params {
            let prefixed = format!("{prefix}::{name}");
            if let Some(binding_expr) = bindings.get(&name) {
                // Use the binding expression (from the importer's scope, no prefixing needed
                // for refs that belong to the importer — only dep-internal refs get prefixed)
                expr = binding_expr.clone();
            } else {
                // Keep default, but prefix internal refs
                prefix_expr_refs(&mut expr, prefix, dep_names);
            }
            self.params.push((prefixed.clone(), type_ann, expr, span));
            // Rebuild runtime deps for the (possibly rewritten) expression
            let mut graph_refs = HashSet::new();
            if let Some(orig_deps) = dep.runtime_deps.get(&name) {
                if bindings.contains_key(&name) {
                    // Binding expression — deps are already in the importer's namespace.
                    // We'll recompute deps from the binding expression below.
                } else {
                    // Default expression — prefix dep-internal deps
                    for d in orig_deps {
                        if dep_names.contains(d) {
                            graph_refs.insert(format!("{prefix}::{d}"));
                        } else {
                            graph_refs.insert(d.clone());
                        }
                    }
                }
            }
            if let Some(binding_expr) = bindings.get(&name) {
                // Collect graph refs from the binding expression
                collect_graph_refs_from_expr(binding_expr, &mut graph_refs);
            }
            self.runtime_deps.insert(prefixed.clone(), graph_refs);
            self.source_order.push((prefixed, DeclCategory::Param));
        }

        // Merge nodes
        for (name, type_ann, mut expr, span) in dep.nodes {
            prefix_expr_refs(&mut expr, prefix, dep_names);
            let prefixed = format!("{prefix}::{name}");
            self.nodes.push((prefixed.clone(), type_ann, expr, span));
            if let Some(deps) = dep.runtime_deps.get(&name) {
                let prefixed_deps = deps
                    .iter()
                    .map(|d| {
                        if dep_names.contains(d) {
                            format!("{prefix}::{d}")
                        } else {
                            d.clone()
                        }
                    })
                    .collect();
                self.runtime_deps.insert(prefixed.clone(), prefixed_deps);
            }
            self.source_order.push((prefixed, DeclCategory::Node));
        }

        // Merge asserts
        for (name, mut body, span) in dep.asserts {
            match &mut body {
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
            let prefixed = format!("{prefix}::{name}");
            self.asserts.push((prefixed.clone(), body, span));
            self.assert_names.insert(prefixed.clone());
            self.source_order.push((prefixed, DeclCategory::Assert));
        }

        // Merge functions
        for (name, mut fn_decl, span) in dep.functions {
            match &mut fn_decl.body {
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
            let prefixed = format!("{prefix}::{name}");
            self.functions.push((prefixed, fn_decl, span));
        }

        // Merge assumes_map and expected_fail
        for (assert_name, assumers) in dep.assumes_map {
            let prefixed_assert = format!("{prefix}::{assert_name}");
            let prefixed_assumers: Vec<String> =
                assumers.iter().map(|a| format!("{prefix}::{a}")).collect();
            self.assumes_map
                .entry(prefixed_assert)
                .or_default()
                .extend(prefixed_assumers);
        }
        for (assert_name, ef) in dep.expected_fail {
            let prefixed = format!("{prefix}::{assert_name}");
            self.expected_fail.insert(prefixed, ef);
        }
    }
}

/// Rewrite `@`-references and const/fn references within an expression to use
/// prefixed names, but only for names that belong to the dependency.
///
/// For example, `GraphRef("dry_mass")` becomes `GraphRef("r::dry_mass")` when
/// `"dry_mass"` is in `dep_names` and `prefix` is `"r"`.
///
/// Built-in names and names from the importer's scope are left unchanged.
pub fn prefix_expr_refs(expr: &mut Expr, prefix: &str, dep_names: &HashSet<String>) {
    match &mut expr.kind {
        ExprKind::GraphRef(ident) | ExprKind::ConstRef(ident) => {
            if dep_names.contains(ident.value.as_str()) {
                ident.value = DeclName::new(format!("{prefix}::{}", ident.value));
            }
        }
        ExprKind::FnCall { name, args } => {
            if dep_names.contains(name.value.as_str()) {
                name.value = FnName::new(format!("{prefix}::{}", name.value));
            }
            for arg in args {
                prefix_expr_refs(arg, prefix, dep_names);
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            prefix_expr_refs(lhs, prefix, dep_names);
            prefix_expr_refs(rhs, prefix, dep_names);
        }
        ExprKind::UnaryOp { operand, .. } => {
            prefix_expr_refs(operand, prefix, dep_names);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            prefix_expr_refs(condition, prefix, dep_names);
            prefix_expr_refs(then_branch, prefix, dep_names);
            prefix_expr_refs(else_branch, prefix, dep_names);
        }
        ExprKind::Convert { expr, .. }
        | ExprKind::DisplayTimezone { expr, .. }
        | ExprKind::AsCast { expr, .. }
        | ExprKind::FieldAccess { expr, .. }
        | ExprKind::IndexAccess { expr, .. } => {
            prefix_expr_refs(expr, prefix, dep_names);
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                prefix_expr_refs(&mut stmt.value, prefix, dep_names);
            }
            prefix_expr_refs(expr, prefix, dep_names);
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &mut field.value {
                    prefix_expr_refs(val, prefix, dep_names);
                }
            }
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                prefix_expr_refs(&mut entry.value, prefix, dep_names);
            }
        }
        ExprKind::ForComp { body, .. } => {
            prefix_expr_refs(body, prefix, dep_names);
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            prefix_expr_refs(source, prefix, dep_names);
            prefix_expr_refs(init, prefix, dep_names);
            prefix_expr_refs(body, prefix, dep_names);
        }
        ExprKind::Unfold { init, body, .. } => {
            prefix_expr_refs(init, prefix, dep_names);
            prefix_expr_refs(body, prefix, dep_names);
        }
        ExprKind::Match { scrutinee, arms } => {
            prefix_expr_refs(scrutinee, prefix, dep_names);
            for arm in arms {
                prefix_expr_refs(&mut arm.body, prefix, dep_names);
            }
        }
        // Qualified refs (rewritten before merging) and leaf nodes need no rewriting.
        ExprKind::QualifiedGraphRef { .. }
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::QualifiedFnCall { .. }
        | ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => {}
    }
}

/// Collect all `@`-referenced names from an expression (non-recursive into child scopes).
///
/// This is a simpler version of `resolve::collect_graph_refs` that operates on
/// arbitrary expressions without requiring a known-names set. Used for building
/// runtime deps from binding expressions.
fn collect_graph_refs_from_expr(expr: &Expr, refs: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::GraphRef(ident) => {
            refs.insert(ident.value.to_string());
        }
        ExprKind::QualifiedGraphRef { module, name } => {
            refs.insert(format!("{}::{}", module.name, name.value));
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_graph_refs_from_expr(lhs, refs);
            collect_graph_refs_from_expr(rhs, refs);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_graph_refs_from_expr(operand, refs);
        }
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
            for arg in args {
                collect_graph_refs_from_expr(arg, refs);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_graph_refs_from_expr(condition, refs);
            collect_graph_refs_from_expr(then_branch, refs);
            collect_graph_refs_from_expr(else_branch, refs);
        }
        ExprKind::Convert { expr, .. }
        | ExprKind::DisplayTimezone { expr, .. }
        | ExprKind::AsCast { expr, .. }
        | ExprKind::FieldAccess { expr, .. }
        | ExprKind::IndexAccess { expr, .. } => {
            collect_graph_refs_from_expr(expr, refs);
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                collect_graph_refs_from_expr(&stmt.value, refs);
            }
            collect_graph_refs_from_expr(expr, refs);
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    collect_graph_refs_from_expr(val, refs);
                }
            }
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                collect_graph_refs_from_expr(&entry.value, refs);
            }
        }
        ExprKind::ForComp { body, .. } => {
            collect_graph_refs_from_expr(body, refs);
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_graph_refs_from_expr(source, refs);
            collect_graph_refs_from_expr(init, refs);
            collect_graph_refs_from_expr(body, refs);
        }
        ExprKind::Unfold { init, body, .. } => {
            collect_graph_refs_from_expr(init, refs);
            collect_graph_refs_from_expr(body, refs);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_graph_refs_from_expr(scrutinee, refs);
            for arm in arms {
                collect_graph_refs_from_expr(&arm.body, refs);
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => {}
    }
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
                register_type_decl(t, registry);
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
        def.scale * base_scale
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

fn register_type_decl(t: &graphcal_syntax::ast::TypeDecl, registry: &mut RegistryBuilder) {
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
    registry.register_type(registry::TypeDef {
        name: t.name.value.clone(),
        generic_params,
        derives: t.derives.iter().map(|d| d.value).collect(),
        variants,
    });
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
    for (name, fn_decl, span) in &resolved.functions {
        registry.register_function(registry::FnDef {
            name: FnName::new(name),
            generic_params: fn_decl
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
            params: fn_decl
                .params
                .iter()
                .map(|p| registry::FnParamDef {
                    name: p.name.name.clone(),
                    type_expr: p.type_ann.clone(),
                })
                .collect(),
            return_type_expr: fn_decl.return_type.clone(),
            body: fn_decl.body.clone(),
            span: *span,
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
        let names: Vec<&str> = ir.source_order.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["b", "a", "z"]);
    }

    #[test]
    fn lower_deps_extracted() {
        let ir = parse_and_lower(
            "param a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;\nnode c: Dimensionless = @a + @b;",
        )
        .unwrap();
        let c_deps = &ir.runtime_deps["c"];
        assert!(c_deps.contains("a"));
        assert!(c_deps.contains("b"));
        assert_eq!(c_deps.len(), 2);
    }
}

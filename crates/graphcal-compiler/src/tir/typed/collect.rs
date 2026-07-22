use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::hir;
use crate::hir::diagnostics::resolved_decl_key;
use crate::ir::resolve::{ExpectedFail, ParsedExpectedFail};
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::syntax::decl_name::{DeclName, ResolvedDeclName};
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;
use crate::syntax::type_name::ResolvedConstructorName;

use super::{
    DagSemanticBody, ModuleTypeContext, ResolvedCollectionRefs, ResolvedConstructorRefs,
    ResolvedConstructorTarget, ResolvedDagDependencies, ResolvedDomainBound, ResolvedExpressions,
    ResolvedIndex, ResolvedInlineDagCall, ResolvedInlineDagRefs, ResolvedTypeExpr, internal_error,
    module_resolve_error,
};

pub(super) fn augment_runtime_deps_for_dynamic_units(semantic: &mut DagSemanticBody) {
    if semantic.dynamic_unit_scales.is_empty() {
        return;
    }
    let scale_deps: HashMap<crate::syntax::dimension::UnitRef, BTreeSet<ResolvedDeclName>> =
        semantic
            .dynamic_unit_scales
            .iter()
            .map(|(name, expr)| {
                (
                    name.clone(),
                    hir::collect_expr_dependencies(expr).graph_refs,
                )
            })
            .collect();

    let DagSemanticBody {
        expressions,
        dependencies,
        ..
    } = semantic;
    for (key, expr) in expressions.param_defaults.iter().chain(&expressions.nodes) {
        let mut unit_names = std::collections::HashSet::new();
        collect_unit_names_from_hir(expr, &mut unit_names);
        let extra: BTreeSet<ResolvedDeclName> = unit_names
            .iter()
            .filter_map(|unit| scale_deps.get(unit))
            .flatten()
            .cloned()
            .collect();
        if !extra.is_empty() {
            dependencies
                .runtime_deps
                .entry(key.clone())
                .or_default()
                .extend(extra);
        }
    }
}

/// Collect every unit name mentioned by `UnitLiteral` / `Convert` nodes.
fn collect_unit_names_from_hir(
    expr: &hir::Expr,
    names: &mut std::collections::HashSet<crate::syntax::dimension::UnitRef>,
) {
    // Recursion choke point: recurses once per tree level.
    crate::stack::with_stack_growth(|| match &expr.kind {
        hir::ExprKind::UnitLiteral { unit, .. } => {
            for term in &unit.terms {
                names.insert(term.name.value.clone());
            }
        }
        hir::ExprKind::Convert {
            expr: inner,
            target,
        } => {
            for term in &target.terms {
                names.insert(term.name.value.clone());
            }
            collect_unit_names_from_hir(inner, names);
        }
        hir::ExprKind::Error
        | hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::ConstRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::VariantLiteral(_) => {}
        hir::ExprKind::BinOp { lhs, rhs, .. } => {
            collect_unit_names_from_hir(lhs, names);
            collect_unit_names_from_hir(rhs, names);
        }
        hir::ExprKind::UnaryOp { operand, .. }
        | hir::ExprKind::DisplayTimezone { expr: operand, .. }
        | hir::ExprKind::FieldAccess { expr: operand, .. } => {
            collect_unit_names_from_hir(operand, names);
        }
        hir::ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_unit_names_from_hir(arg, names);
            }
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_unit_names_from_hir(condition, names);
            collect_unit_names_from_hir(then_branch, names);
            collect_unit_names_from_hir(else_branch, names);
        }
        hir::ExprKind::ConstructorCall { fields, .. } => {
            for field in fields {
                collect_unit_names_from_hir(&field.value, names);
            }
        }
        hir::ExprKind::MapLiteral { entries } => {
            for entry in entries {
                collect_unit_names_from_hir(&entry.value, names);
            }
        }
        hir::ExprKind::ForComp { body, .. } => collect_unit_names_from_hir(body, names),
        hir::ExprKind::IndexAccess { expr: inner, args } => {
            collect_unit_names_from_hir(inner, names);
            for arg in args {
                if let hir::expr::IndexArg::Expr(arg_expr) = arg {
                    collect_unit_names_from_hir(arg_expr, names);
                }
            }
        }
        hir::ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_unit_names_from_hir(source, names);
            collect_unit_names_from_hir(init, names);
            collect_unit_names_from_hir(body, names);
        }
        hir::ExprKind::Unfold { init, body, .. } => {
            collect_unit_names_from_hir(init, names);
            collect_unit_names_from_hir(body, names);
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_unit_names_from_hir(scrutinee, names);
            for arm in arms {
                collect_unit_names_from_hir(&arm.body, names);
            }
        }
        hir::ExprKind::InlineDagRef { args, .. } => {
            for arg in args {
                collect_unit_names_from_hir(&arg.value, names);
            }
        }
    });
}

pub(super) fn collect_resolved_dag_dependencies(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    exprs: &ResolvedExpressions,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDagDependencies, GraphcalError> {
    let mut resolved = ResolvedDagDependencies::default();

    for entry in consts {
        let body_src = entry.src.resolve(src);
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                body_src,
                entry.span,
            )
        })?;
        let hir_expr = exprs.consts.get(&key).ok_or_else(|| {
            internal_error(
                format!(
                    "missing HIR expression for const declaration `{}`",
                    entry.name
                ),
                body_src,
                entry.span,
            )
        })?;
        let mut deps = hir::collect_expr_dependencies(hir_expr);
        for graph_ref in &deps.graph_refs {
            // `@const_name` in a const body is a const dependency. Non-const
            // `@` targets are rejected with a spanned diagnostic by
            // `check_hir_body_policies`.
            let kind = ctx
                .resolver
                .decl_symbol_kind(graph_ref)
                .map_err(|err| module_resolve_error(&err, body_src, entry.span))?;
            if kind.is_const() {
                deps.const_refs.insert(graph_ref.clone());
            }
        }
        resolved.const_deps.insert(key, deps.const_refs);
    }

    for entry in params {
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                entry.src.resolve(src),
                entry.span,
            )
        })?;
        let deps = exprs.param_defaults.get(&key).map_or_else(
            hir::ExprDependencies::default,
            hir::collect_expr_dependencies,
        );
        resolved.runtime_deps.insert(key, deps.graph_refs);
    }

    for entry in nodes {
        let body_src = entry.src.resolve(src);
        let key = resolved_decl_key(ctx.owner, &entry.name).ok_or_else(|| {
            internal_error(
                format!(
                    "could not build canonical declaration key for `{}`",
                    entry.name
                ),
                body_src,
                entry.span,
            )
        })?;
        let hir_expr = exprs.nodes.get(&key).ok_or_else(|| {
            internal_error(
                format!(
                    "missing HIR expression for node declaration `{}`",
                    entry.name
                ),
                body_src,
                entry.span,
            )
        })?;
        let mut deps = hir::collect_expr_dependencies(hir_expr);
        // Unfold self-references access the previous step, not the node's
        // own value: drop the self-edge whenever every self-reference lies
        // inside an unfold subtree (covers nested forms like
        // `if c { unfold(…) } else { unfold(…) }`, not just a top-level
        // unfold). A self-reference outside any unfold stays and is
        // reported as a genuine cycle.
        if !hir::has_ref_outside_unfold(hir_expr, &key) {
            deps.graph_refs.remove(&key);
        }
        resolved.runtime_deps.insert(key, deps.graph_refs);
    }

    Ok(resolved)
}

pub(super) fn collect_resolved_collection_refs(
    exprs: &ResolvedExpressions,
    domain_bounds: &HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>,
    resolved_decl_types: &HashMap<ScopedName, ResolvedTypeExpr>,
    imported_values: &HashMap<
        ScopedName,
        (
            crate::registry::runtime_value::RuntimeValue,
            crate::registry::declared_type::DeclaredType,
        ),
    >,
    imported_decl_types: &HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedCollectionRefs, GraphcalError> {
    let mut refs = ResolvedCollectionRefs::default();

    for resolved_type in resolved_decl_types.values() {
        collect_resolved_collection_indexes_from_type(resolved_type, ctx, src, &mut refs)?;
    }
    for declared in imported_decl_types
        .values()
        .chain(imported_values.values().map(|(_value, declared)| declared))
    {
        collect_resolved_collection_indexes_from_declared_type(declared, ctx, src, &mut refs)?;
    }

    for hir_expr in exprs
        .consts
        .values()
        .chain(exprs.param_defaults.values())
        .chain(exprs.nodes.values())
        .chain(domain_bounds.values().flatten().map(|bound| &bound.value))
    {
        collect_resolved_collection_refs_from_expr(hir_expr, ctx, src, &mut refs)?;
    }
    for body in exprs.asserts.values() {
        collect_resolved_collection_refs_from_assert_body(body, ctx, src, &mut refs)?;
    }

    Ok(refs)
}

fn record_resolved_collection_index(
    index: &ResolvedIndexName,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    span: Span,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    if refs.index_defs.contains_key(index) {
        return Ok(());
    }
    let def = ctx.types.get_index(index).cloned().ok_or_else(|| {
        internal_error(
            format!("semantic collection metadata references unknown index `{index}`"),
            src,
            span,
        )
    })?;
    refs.index_defs.insert(index.clone(), def);
    Ok(())
}

fn collect_resolved_collection_indexes_from_declared_type(
    declared_type: &crate::registry::declared_type::DeclaredType,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    match declared_type {
        crate::registry::declared_type::DeclaredType::IndexArg(index) => {
            record_declared_collection_index(index, ctx, src, refs)
        }
        crate::registry::declared_type::DeclaredType::Indexed { element, index } => {
            record_declared_collection_index(index, ctx, src, refs)?;
            collect_resolved_collection_indexes_from_declared_type(element, ctx, src, refs)
        }
        crate::registry::declared_type::DeclaredType::Struct(_name, type_args) => {
            for arg in type_args {
                collect_resolved_collection_indexes_from_declared_type(arg, ctx, src, refs)?;
            }
            Ok(())
        }
        crate::registry::declared_type::DeclaredType::Scalar(_)
        | crate::registry::declared_type::DeclaredType::Bool
        | crate::registry::declared_type::DeclaredType::Int
        | crate::registry::declared_type::DeclaredType::Datetime(_) => Ok(()),
    }
}

fn record_declared_collection_index(
    index: &IndexTypeRef,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    index.declared_resolved().map_or(Ok(()), |resolved| {
        record_resolved_collection_index(resolved, ctx, src, Span::new(0, 0), refs)
    })
}

fn collect_resolved_collection_indexes_from_type(
    resolved_type: &ResolvedTypeExpr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    match resolved_type {
        ResolvedTypeExpr::IndexArg(ResolvedIndex::Concrete(index, span)) => {
            record_resolved_collection_index(index, ctx, src, *span, refs)
        }
        ResolvedTypeExpr::Indexed { base, indexes } => {
            collect_resolved_collection_indexes_from_type(base, ctx, src, refs)?;
            for index in indexes {
                if let ResolvedIndex::Concrete(resolved, span) = index {
                    record_resolved_collection_index(resolved, ctx, src, *span, refs)?;
                }
            }
            Ok(())
        }
        ResolvedTypeExpr::GenericStruct { type_args, .. } => {
            for arg in type_args {
                collect_resolved_collection_indexes_from_type(arg, ctx, src, refs)?;
            }
            Ok(())
        }
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::IndexArg(_)
        | ResolvedTypeExpr::Scalar(_)
        | ResolvedTypeExpr::Struct(_, _)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericTypeParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => Ok(()),
    }
}

pub(super) fn collect_resolved_collection_refs_from_expr(
    expr: &hir::Expr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    // Recursion choke point: recurses once per tree level (unbounded for
    // left-nested operator chains).
    crate::stack::with_stack_growth(|| {
        collect_resolved_collection_refs_from_expr_inner(expr, ctx, src, refs)
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "expression traversal mirrors HIR variants"
)]
fn collect_resolved_collection_refs_from_expr_inner(
    expr: &hir::Expr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        hir::ExprKind::Error
        | hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::ConstRef(_)
        | hir::ExprKind::UnitLiteral { .. } => Ok(()),
        hir::ExprKind::VariantLiteral(variant) => record_resolved_collection_index(
            variant.variant.index(),
            ctx,
            src,
            variant.path_span(),
            refs,
        ),
        hir::ExprKind::BinOp { lhs, rhs, .. } => {
            collect_resolved_collection_refs_from_expr(lhs, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(rhs, ctx, src, refs)
        }
        hir::ExprKind::UnaryOp { operand, .. } => {
            collect_resolved_collection_refs_from_expr(operand, ctx, src, refs)
        }
        hir::ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_resolved_collection_refs_from_expr(arg, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_resolved_collection_refs_from_expr(condition, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(then_branch, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(else_branch, ctx, src, refs)
        }
        hir::ExprKind::Convert { expr, .. }
        | hir::ExprKind::DisplayTimezone { expr, .. }
        | hir::ExprKind::FieldAccess { expr, .. } => {
            collect_resolved_collection_refs_from_expr(expr, ctx, src, refs)
        }
        hir::ExprKind::ConstructorCall { fields, .. } => {
            for field in fields {
                collect_resolved_collection_refs_from_expr(&field.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::MapLiteral { entries } => {
            for entry in entries {
                for key in &entry.keys {
                    match key {
                        hir::expr::MapEntryKey::IndexVariant(variant) => {
                            record_resolved_collection_index(
                                variant.variant.index(),
                                ctx,
                                src,
                                variant.variant_span,
                                refs,
                            )?;
                        }
                        hir::expr::MapEntryKey::NatRangeVariant { .. } => {}
                    }
                }
                collect_resolved_collection_refs_from_expr(&entry.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::ForComp { bindings, body } => {
            for binding in bindings {
                match &binding.index {
                    hir::expr::ForBindingIndex::Named(index) => {
                        record_resolved_collection_index(&index.value, ctx, src, index.span, refs)?;
                    }
                    hir::expr::ForBindingIndex::Range { .. } => {}
                }
            }
            collect_resolved_collection_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::IndexAccess { expr, args } => {
            collect_resolved_collection_refs_from_expr(expr, ctx, src, refs)?;
            for arg in args {
                match arg {
                    hir::expr::IndexArg::Variant(variant) => {
                        record_resolved_collection_index(
                            variant.variant.index(),
                            ctx,
                            src,
                            variant.path_span(),
                            refs,
                        )?;
                    }
                    hir::expr::IndexArg::Expr(expr) => {
                        collect_resolved_collection_refs_from_expr(expr, ctx, src, refs)?;
                    }
                    hir::expr::IndexArg::Var(_) => {}
                }
            }
            Ok(())
        }
        hir::ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_resolved_collection_refs_from_expr(source, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Unfold { init, body, .. } => {
            collect_resolved_collection_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_resolved_collection_refs_from_expr(scrutinee, ctx, src, refs)?;
            for arm in arms {
                if let hir::expr::MatchPattern::IndexLabel { variant, span: _ } = &arm.pattern {
                    record_resolved_collection_index(
                        variant.variant.index(),
                        ctx,
                        src,
                        variant.path_span(),
                        refs,
                    )?;
                }
                collect_resolved_collection_refs_from_expr(&arm.body, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::InlineDagRef { args, .. } => {
            for arg in args {
                collect_resolved_collection_refs_from_expr(&arg.value, ctx, src, refs)?;
            }
            Ok(())
        }
    }
}

fn collect_resolved_collection_refs_from_assert_body(
    body: &hir::AssertBody,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedCollectionRefs,
) -> Result<(), GraphcalError> {
    match body {
        hir::AssertBody::Expr(expr) => {
            collect_resolved_collection_refs_from_expr(expr, ctx, src, refs)
        }
        hir::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative: _,
        } => {
            collect_resolved_collection_refs_from_expr(actual, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(expected, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(tolerance, ctx, src, refs)
        }
    }
}

pub(super) fn collect_resolved_constructor_refs(
    exprs: &ResolvedExpressions,
    domain_bounds: &HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedConstructorRefs, GraphcalError> {
    let mut refs = ResolvedConstructorRefs::default();

    for hir_expr in exprs
        .consts
        .values()
        .chain(exprs.param_defaults.values())
        .chain(exprs.nodes.values())
        .chain(domain_bounds.values().flatten().map(|bound| &bound.value))
    {
        collect_resolved_constructor_refs_from_expr(hir_expr, ctx, src, &mut refs)?;
    }
    for body in exprs.asserts.values() {
        collect_resolved_constructor_refs_from_assert_body(body, ctx, src, &mut refs)?;
    }

    Ok(refs)
}

fn record_resolved_constructor_target(
    constructor: &ResolvedConstructorName,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    span: Span,
    refs: &mut ResolvedConstructorRefs,
) -> Result<ResolvedConstructorTarget, GraphcalError> {
    if let Some(target) = refs.constructor_defs.get(constructor) {
        return Ok(target.clone());
    }

    let def = ctx.types.lookup_constructor(constructor).ok_or_else(|| {
        internal_error(
            format!("semantic constructor metadata references unknown constructor `{constructor}`"),
            src,
            span,
        )
    })?;
    let target = ResolvedConstructorTarget {
        owning_type: def.owning_type.clone(),
        type_def: def.type_def.clone(),
        variant: def.variant.clone(),
    };
    refs.constructor_defs
        .insert(constructor.clone(), target.clone());
    Ok(target)
}

pub(super) fn collect_resolved_constructor_refs_from_expr(
    expr: &hir::Expr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedConstructorRefs,
) -> Result<(), GraphcalError> {
    // Recursion choke point: recurses once per tree level (unbounded for
    // left-nested operator chains).
    crate::stack::with_stack_growth(|| {
        collect_resolved_constructor_refs_from_expr_inner(expr, ctx, src, refs)
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "expression traversal mirrors HIR variants"
)]
fn collect_resolved_constructor_refs_from_expr_inner(
    expr: &hir::Expr,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedConstructorRefs,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        hir::ExprKind::Error
        | hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::UnitLiteral { .. }
        | hir::ExprKind::VariantLiteral(_) => Ok(()),
        hir::ExprKind::ConstRef(target) => {
            if let hir::ConstRef::Constructor(constructor) = &target.value {
                record_resolved_constructor_target(constructor, ctx, src, target.span, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::BinOp { lhs, rhs, .. } => {
            collect_resolved_constructor_refs_from_expr(lhs, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(rhs, ctx, src, refs)
        }
        hir::ExprKind::UnaryOp { operand, .. } => {
            collect_resolved_constructor_refs_from_expr(operand, ctx, src, refs)
        }
        hir::ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_resolved_constructor_refs_from_expr(arg, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_resolved_constructor_refs_from_expr(condition, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(then_branch, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(else_branch, ctx, src, refs)
        }
        hir::ExprKind::Convert { expr, .. }
        | hir::ExprKind::DisplayTimezone { expr, .. }
        | hir::ExprKind::FieldAccess { expr, .. } => {
            collect_resolved_constructor_refs_from_expr(expr, ctx, src, refs)
        }
        hir::ExprKind::ConstructorCall { callee, fields, .. } => {
            record_resolved_constructor_target(&callee.value, ctx, src, callee.span, refs)?;
            for field in fields {
                collect_resolved_constructor_refs_from_expr(&field.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::MapLiteral { entries } => {
            for entry in entries {
                collect_resolved_constructor_refs_from_expr(&entry.value, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::ForComp { body, .. } => {
            collect_resolved_constructor_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::IndexAccess { expr, args } => {
            collect_resolved_constructor_refs_from_expr(expr, ctx, src, refs)?;
            for arg in args {
                if let hir::expr::IndexArg::Expr(expr) = arg {
                    collect_resolved_constructor_refs_from_expr(expr, ctx, src, refs)?;
                }
            }
            Ok(())
        }
        hir::ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_resolved_constructor_refs_from_expr(source, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Unfold { init, body, .. } => {
            collect_resolved_constructor_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_resolved_constructor_refs_from_expr(scrutinee, ctx, src, refs)?;
            for arm in arms {
                if let hir::expr::MatchPattern::Constructor { constructor, .. } = &arm.pattern {
                    record_resolved_constructor_target(
                        &constructor.value,
                        ctx,
                        src,
                        constructor.span,
                        refs,
                    )?;
                }
                collect_resolved_constructor_refs_from_expr(&arm.body, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::InlineDagRef { args, .. } => {
            for arg in args {
                collect_resolved_constructor_refs_from_expr(&arg.value, ctx, src, refs)?;
            }
            Ok(())
        }
    }
}

fn collect_resolved_constructor_refs_from_assert_body(
    body: &hir::AssertBody,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
    refs: &mut ResolvedConstructorRefs,
) -> Result<(), GraphcalError> {
    match body {
        hir::AssertBody::Expr(expr) => {
            collect_resolved_constructor_refs_from_expr(expr, ctx, src, refs)
        }
        hir::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative: _,
        } => {
            collect_resolved_constructor_refs_from_expr(actual, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(expected, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(tolerance, ctx, src, refs)
        }
    }
}

pub(super) fn collect_resolved_inline_dag_refs(
    exprs: &ResolvedExpressions,
) -> ResolvedInlineDagRefs {
    let mut refs = ResolvedInlineDagRefs::default();

    for hir_expr in exprs
        .consts
        .values()
        .chain(exprs.param_defaults.values())
        .chain(exprs.nodes.values())
    {
        collect_resolved_inline_dag_refs_from_expr(hir_expr, &mut refs);
    }
    for body in exprs.asserts.values() {
        collect_resolved_inline_dag_refs_from_assert_body(body, &mut refs);
    }

    refs
}

fn collect_resolved_inline_dag_refs_from_expr(expr: &hir::Expr, refs: &mut ResolvedInlineDagRefs) {
    // Recursion choke point: recurses once per tree level (unbounded for
    // left-nested operator chains).
    crate::stack::with_stack_growth(|| {
        collect_resolved_inline_dag_refs_from_expr_inner(expr, refs);
    });
}

fn collect_resolved_inline_dag_refs_from_expr_inner(
    expr: &hir::Expr,
    refs: &mut ResolvedInlineDagRefs,
) {
    match &expr.kind {
        hir::ExprKind::Error
        | hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::ConstRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::UnitLiteral { .. }
        | hir::ExprKind::VariantLiteral(_) => {}
        hir::ExprKind::BinOp { lhs, rhs, .. } => {
            collect_resolved_inline_dag_refs_from_expr(lhs, refs);
            collect_resolved_inline_dag_refs_from_expr(rhs, refs);
        }
        hir::ExprKind::UnaryOp { operand, .. }
        | hir::ExprKind::Convert { expr: operand, .. }
        | hir::ExprKind::DisplayTimezone { expr: operand, .. }
        | hir::ExprKind::FieldAccess { expr: operand, .. } => {
            collect_resolved_inline_dag_refs_from_expr(operand, refs);
        }
        hir::ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_resolved_inline_dag_refs_from_expr(arg, refs);
            }
        }
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_resolved_inline_dag_refs_from_expr(condition, refs);
            collect_resolved_inline_dag_refs_from_expr(then_branch, refs);
            collect_resolved_inline_dag_refs_from_expr(else_branch, refs);
        }
        hir::ExprKind::ConstructorCall { fields, .. } => {
            for field in fields {
                collect_resolved_inline_dag_refs_from_expr(&field.value, refs);
            }
        }
        hir::ExprKind::MapLiteral { entries } => {
            for entry in entries {
                collect_resolved_inline_dag_refs_from_expr(&entry.value, refs);
            }
        }
        hir::ExprKind::ForComp { body, .. } => {
            collect_resolved_inline_dag_refs_from_expr(body, refs);
        }
        hir::ExprKind::IndexAccess { expr, args } => {
            collect_resolved_inline_dag_refs_from_expr(expr, refs);
            for arg in args {
                if let hir::expr::IndexArg::Expr(expr) = arg {
                    collect_resolved_inline_dag_refs_from_expr(expr, refs);
                }
            }
        }
        hir::ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_resolved_inline_dag_refs_from_expr(source, refs);
            collect_resolved_inline_dag_refs_from_expr(init, refs);
            collect_resolved_inline_dag_refs_from_expr(body, refs);
        }
        hir::ExprKind::Unfold { init, body, .. } => {
            collect_resolved_inline_dag_refs_from_expr(init, refs);
            collect_resolved_inline_dag_refs_from_expr(body, refs);
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_resolved_inline_dag_refs_from_expr(scrutinee, refs);
            for arm in arms {
                collect_resolved_inline_dag_refs_from_expr(&arm.body, refs);
            }
        }
        hir::ExprKind::InlineDagRef {
            target,
            args,
            output,
        } => {
            let arg_targets = args
                .iter()
                .map(|arg| (arg.target.span, arg.target.value.clone()))
                .collect();
            refs.calls.insert(
                expr.span,
                ResolvedInlineDagCall {
                    target: target.value.clone(),
                    arg_targets,
                    output: output.clone(),
                },
            );
            for arg in args {
                collect_resolved_inline_dag_refs_from_expr(&arg.value, refs);
            }
        }
    }
}

fn collect_resolved_inline_dag_refs_from_assert_body(
    body: &hir::AssertBody,
    refs: &mut ResolvedInlineDagRefs,
) {
    match body {
        hir::AssertBody::Expr(expr) => collect_resolved_inline_dag_refs_from_expr(expr, refs),
        hir::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative: _,
        } => {
            collect_resolved_inline_dag_refs_from_expr(actual, refs);
            collect_resolved_inline_dag_refs_from_expr(expected, refs);
            collect_resolved_inline_dag_refs_from_expr(tolerance, refs);
        }
    }
}

pub(super) fn collect_hir_decl_bindings(
    owner: &crate::dag_id::DagId,
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ResolvedDeclName>, GraphcalError> {
    let mut bindings = HashMap::new();

    for name in consts
        .iter()
        .map(|entry| &entry.name)
        .chain(params.iter().map(|entry| &entry.name))
        .chain(nodes.iter().map(|entry| &entry.name))
    {
        let resolved = resolved_decl_key(owner, name).ok_or_else(|| {
            internal_error(
                format!("could not build canonical declaration key for `{name}`"),
                src,
                Span::new(0, 0),
            )
        })?;
        bindings.insert(name.clone(), resolved);
    }

    for (name, source) in imported_value_sources {
        bindings.insert(
            name.clone(),
            ResolvedDeclName::from_def(source.dag_id.clone(), source.source_name.clone()),
        );
    }

    Ok(bindings)
}

#[expect(
    clippy::too_many_arguments,
    reason = "collects local and imported declaration binding sources for a completed DAG"
)]
pub(super) fn collect_resolved_decl_bindings(
    ctx: ModuleTypeContext<'_>,
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    imported_values: &HashMap<
        ScopedName,
        (
            crate::registry::runtime_value::RuntimeValue,
            crate::registry::declared_type::DeclaredType,
        ),
    >,
    imported_decl_types: &HashMap<ScopedName, crate::registry::declared_type::DeclaredType>,
    imported_value_sources: &HashMap<ScopedName, crate::ir::lower::ImportedValueSource>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ResolvedDeclName>, GraphcalError> {
    let mut bindings = collect_hir_decl_bindings(
        ctx.owner,
        consts,
        params,
        nodes,
        imported_value_sources,
        src,
    )?;

    for name in imported_values
        .keys()
        .chain(imported_decl_types.keys())
        .chain(imported_value_sources.keys())
    {
        if bindings.contains_key(name) {
            continue;
        }
        let path = scoped_name_to_name_path(name).ok_or_else(|| {
            internal_error(
                format!("could not convert visible declaration `{name}` to a name path"),
                src,
                Span::new(0, 0),
            )
        })?;
        let resolved = match ctx.resolver.resolve_decl_path(ctx.owner, &path) {
            Ok(resolved) => resolved,
            Err(err) => {
                if let Some(source) = imported_value_sources.get(name) {
                    ResolvedDeclName::from_def(source.dag_id.clone(), source.source_name.clone())
                } else if imported_values.contains_key(name)
                    || imported_decl_types.contains_key(name)
                {
                    // Instantiated inline-DAG includes can carry hidden imported
                    // values from the included DAG body into the importer. Those
                    // aliases are not import declarations in the importer's module
                    // scope, but they are explicit IR inputs, so bind them as
                    // synthetic declarations owned by the current DAG — only when
                    // that synthetic declaration actually exists in the resolver.
                    resolve_existing_synthetic_child_decl(ctx, name)
                        .or_else(|| resolved_decl_key(ctx.owner, name))
                        .ok_or_else(|| {
                            internal_error(
                                format!(
                                    "visible declaration `{name}` is absent from module resolver: {err}"
                                ),
                                src,
                                Span::new(0, 0),
                            )
                        })?
                } else {
                    return Err(module_resolve_error(&err, src, Span::new(0, 0)));
                }
            }
        };
        bindings.insert(name.clone(), resolved);
    }

    Ok(bindings)
}

fn resolve_existing_synthetic_child_decl(
    ctx: ModuleTypeContext<'_>,
    name: &ScopedName,
) -> Option<ResolvedDeclName> {
    let mut qualifier = name.qualifier().iter();
    let first = qualifier.next()?;
    let synthetic_owner = qualifier.fold(ctx.owner.child(first.as_ref()), |owner, segment| {
        owner.child(segment.as_ref())
    });
    let decl_name = DeclName::expect_valid(name.member());
    ctx.resolver
        .modules()
        .get(&synthetic_owner)
        .and_then(|module| module.decls().contains_key(&decl_name).then_some(()))
        .map(|()| ResolvedDeclName::from_def(synthetic_owner, decl_name))
}

fn scoped_name_to_name_path(name: &ScopedName) -> Option<NamePath> {
    let qualifier = name
        .qualifier()
        .iter()
        .map(|segment| NameAtom::parse(segment.as_ref()).ok())
        .collect::<Option<Vec<_>>>()?;
    let leaf = NameAtom::parse(name.member()).ok()?;
    Some(if qualifier.is_empty() {
        NamePath::local(leaf)
    } else {
        NamePath::qualified_path(qualifier, leaf)
    })
}

pub(super) fn resolve_expected_fail_keys(
    expected_fail: HashMap<ScopedName, ParsedExpectedFail>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ExpectedFail>, GraphcalError> {
    expected_fail
        .into_iter()
        .map(|(assert_name, expected)| {
            let resolved = match expected {
                ExpectedFail::All => ExpectedFail::All,
                ExpectedFail::Variants(keys) => {
                    let resolved_keys = keys
                        .into_iter()
                        .map(|key| {
                            key.into_iter()
                                .map(|part| match part {
                                    crate::registry::resolve_types::ExpectedFailKeyPart::Named {
                                        index,
                                        variant,
                                        span,
                                    } => {
                                        let resolved = ctx
                                            .resolver
                                            .resolve_index_variant_parts(ctx.owner, &index, &variant)
                                            .map_err(|err| module_resolve_error(&err, src, span))?;
                                        Ok(crate::registry::resolve_types::ExpectedFailKeyPart::resolved(
                                            resolved, span,
                                        ))
                                    }
                                    crate::registry::resolve_types::ExpectedFailKeyPart::RangeStep {
                                        step,
                                        span,
                                    } => Ok(
                                        crate::registry::resolve_types::ExpectedFailKeyPart::RangeStep {
                                            step,
                                            span,
                                        },
                                    ),
                                })
                                .collect::<Result<_, GraphcalError>>()
                        })
                        .collect::<Result<_, GraphcalError>>()?;
                    ExpectedFail::Variants(resolved_keys)
                }
            };
            Ok((assert_name, resolved))
        })
        .collect()
}

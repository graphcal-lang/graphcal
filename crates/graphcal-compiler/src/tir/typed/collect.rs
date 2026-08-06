use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::hir;
use crate::ir::resolve::{ExpectedFail, ParsedExpectedFail};
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::ResolvedConstructorName;

use super::{
    DagTIR, ModuleTypeContext, ResolvedCollectionRefs, ResolvedConstructorRefs,
    ResolvedConstructorTarget, ResolvedDagDependencies, ResolvedDomainBound, ResolvedIndex,
    ResolvedTypeExpr, internal_error, module_resolve_error,
};

pub(super) fn augment_runtime_deps_for_dynamic_units(dag: &mut DagTIR) {
    if dag.semantic.dynamic_unit_scales.is_empty() {
        return;
    }
    let scale_deps: HashMap<
        crate::syntax::dimension::ResolvedUnitName,
        BTreeSet<ResolvedDeclName>,
    > = dag
        .semantic
        .dynamic_unit_scales
        .iter()
        .map(|(name, entry)| {
            (
                name.clone(),
                hir::collect_expr_dependencies(&entry.expr).graph_refs,
            )
        })
        .collect();
    let runtime_units = dag
        .params
        .iter()
        .filter_map(|entry| {
            entry.default_expr.as_ref().map(|expr| {
                (
                    dag.resolved_decl_key_for_local(&entry.name),
                    collect_unit_names(expr),
                )
            })
        })
        .chain(dag.nodes.iter().map(|entry| {
            (
                dag.resolved_decl_key_for_local(&entry.name),
                collect_unit_names(&entry.expr),
            )
        }))
        .collect::<Vec<_>>();

    for (key, unit_names) in runtime_units {
        let extra: BTreeSet<ResolvedDeclName> = unit_names
            .iter()
            .filter_map(|unit| scale_deps.get(unit))
            .flatten()
            .cloned()
            .collect();
        if !extra.is_empty() {
            dag.semantic
                .dependencies
                .runtime_deps
                .entry(key)
                .or_default()
                .extend(extra);
        }
    }
}

fn collect_unit_names(
    expr: &hir::Expr,
) -> std::collections::HashSet<crate::syntax::dimension::ResolvedUnitName> {
    let mut names = std::collections::HashSet::new();
    collect_unit_names_from_hir(expr, &mut names);
    names
}

/// Collect every unit name mentioned by `QuantityLiteral` / `Convert` nodes.
fn collect_unit_names_from_hir(
    expr: &hir::Expr,
    names: &mut std::collections::HashSet<crate::syntax::dimension::ResolvedUnitName>,
) {
    // Recursion choke point: recurses once per tree level.
    crate::stack::with_stack_growth(|| match &expr.kind {
        hir::ExprKind::QuantityLiteral { unit, .. } => {
            for term in &unit.terms {
                names.insert(term.name.value.resolved().clone());
            }
        }
        hir::ExprKind::Convert {
            expr: inner,
            target,
        } => {
            for term in &target.terms {
                names.insert(term.name.value.resolved().clone());
            }
            collect_unit_names_from_hir(inner, names);
        }
        hir::ExprKind::Error { .. }
        | hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::OffsetDateTimeLiteral(_)
        | hir::ExprKind::CivilDateTimeLiteral(_)
        | hir::ExprKind::ZonedDateTimeLiteral(_)
        | hir::ExprKind::IanaTimeZoneLiteral(_)
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
        hir::ExprKind::KeyForm { arg, .. } => {
            collect_unit_names_from_hir(arg, names);
        }
        hir::ExprKind::Match { scrutinee, arms } => {
            collect_unit_names_from_hir(scrutinee, names);
            for arm in arms {
                collect_unit_names_from_hir(&arm.body, names);
            }
        }
        hir::ExprKind::DagCall { args, .. } => {
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
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDagDependencies, GraphcalError> {
    let mut resolved = ResolvedDagDependencies::default();

    for entry in consts {
        let body_src = entry.body_src.resolve(src);
        let key = ResolvedDeclName::from_def(
            entry.declaration_owner.clone(),
            entry.name.member().clone(),
        );
        let mut deps = hir::collect_expr_dependencies(&entry.expr);
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
        let key = ResolvedDeclName::from_def(
            entry.declaration_owner.clone(),
            entry.name.member().clone(),
        );
        let deps = entry.default_expr.as_ref().map_or_else(
            hir::ExprDependencies::default,
            hir::collect_expr_dependencies,
        );
        resolved.runtime_deps.insert(key, deps.graph_refs);
    }

    for entry in nodes {
        let key = ResolvedDeclName::from_def(
            entry.declaration_owner.clone(),
            entry.name.member().clone(),
        );
        let deps = hir::collect_expr_dependencies(&entry.expr);
        resolved.runtime_deps.insert(key, deps.graph_refs);
    }

    Ok(resolved)
}

#[expect(
    clippy::too_many_arguments,
    reason = "whole-DAG reference collection visits every authoritative body category and its resolution context"
)]
pub(super) fn collect_resolved_collection_refs(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    asserts: &[crate::ir::lower::AssertEntry],
    domain_bounds: &HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>,
    resolved_decl_types: &HashMap<ScopedName, ResolvedTypeExpr>,
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedCollectionRefs, GraphcalError> {
    let mut refs = ResolvedCollectionRefs::default();

    for resolved_type in resolved_decl_types.values() {
        collect_resolved_collection_indexes_from_type(resolved_type, ctx, src, &mut refs)?;
    }
    for binding in imported_bindings.values() {
        collect_resolved_collection_indexes_from_declared_type(
            binding.declared_type(),
            ctx,
            src,
            &mut refs,
        )?;
    }

    for entry in consts {
        collect_resolved_collection_refs_from_expr(
            &entry.expr,
            ctx,
            entry.body_src.resolve(src),
            &mut refs,
        )?;
    }
    for entry in params {
        if let Some(expr) = &entry.default_expr {
            let default_src = entry.default_src.as_ref().ok_or_else(|| {
                internal_error(
                    format!("parameter `{}` is missing default provenance", entry.name),
                    src,
                    entry.span,
                )
            })?;
            collect_resolved_collection_refs_from_expr(
                expr,
                ctx,
                default_src.resolve(src),
                &mut refs,
            )?;
        }
    }
    for entry in nodes {
        collect_resolved_collection_refs_from_expr(
            &entry.expr,
            ctx,
            entry.body_src.resolve(src),
            &mut refs,
        )?;
    }
    for bounds in domain_bounds.values() {
        for bound in bounds {
            collect_resolved_collection_refs_from_expr(&bound.value, ctx, &bound.src, &mut refs)?;
        }
    }
    for entry in asserts {
        collect_resolved_collection_refs_from_assert_body(
            &entry.body,
            ctx,
            entry.body_src.resolve(src),
            &mut refs,
        )?;
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
    let def = ctx.types.get_index_handle(index).cloned().ok_or_else(|| {
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
        crate::registry::declared_type::DeclaredType::IndexArg(index)
        | crate::registry::declared_type::DeclaredType::Key(index) => {
            record_declared_collection_index(index, ctx, src, refs)
        }
        crate::registry::declared_type::DeclaredType::Indexed { element, index } => {
            record_declared_collection_index(index, ctx, src, refs)?;
            collect_resolved_collection_indexes_from_declared_type(element, ctx, src, refs)
        }
        crate::registry::declared_type::DeclaredType::Struct(_name, generic_args) => {
            for arg in generic_args {
                match arg {
                    crate::registry::declared_type::DeclaredGenericArg::Index(index) => {
                        record_declared_collection_index(index, ctx, src, refs)?;
                    }
                    crate::registry::declared_type::DeclaredGenericArg::Type(type_expr) => {
                        collect_resolved_collection_indexes_from_declared_type(
                            type_expr, ctx, src, refs,
                        )?;
                    }
                    crate::registry::declared_type::DeclaredGenericArg::Dim(_)
                    | crate::registry::declared_type::DeclaredGenericArg::Nat(_) => {}
                }
            }
            Ok(())
        }
        crate::registry::declared_type::DeclaredType::Quantity(_)
        | crate::registry::declared_type::DeclaredType::Complex(_)
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
        ResolvedTypeExpr::IndexArg(ResolvedIndex::Concrete(index, span))
        | ResolvedTypeExpr::Key {
            index: ResolvedIndex::Concrete(index, span),
            ..
        } => record_resolved_collection_index(index, ctx, src, *span, refs),
        ResolvedTypeExpr::Indexed { base, indexes } => {
            collect_resolved_collection_indexes_from_type(base, ctx, src, refs)?;
            for index in indexes {
                if let ResolvedIndex::Concrete(resolved, span) = index {
                    record_resolved_collection_index(resolved, ctx, src, *span, refs)?;
                }
            }
            Ok(())
        }
        ResolvedTypeExpr::GenericStruct { generic_args, .. } => {
            for arg in generic_args {
                match arg {
                    crate::tir::typed::ResolvedGenericArg::Index(ResolvedIndex::Concrete(
                        index,
                        span,
                    )) => record_resolved_collection_index(index, ctx, src, *span, refs)?,
                    crate::tir::typed::ResolvedGenericArg::Type(type_expr) => {
                        collect_resolved_collection_indexes_from_type(type_expr, ctx, src, refs)?;
                    }
                    crate::tir::typed::ResolvedGenericArg::Dim(_)
                    | crate::tir::typed::ResolvedGenericArg::Index(_)
                    | crate::tir::typed::ResolvedGenericArg::Nat(_, _) => {}
                }
            }
            Ok(())
        }
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Complex { .. }
        | ResolvedTypeExpr::Key { .. }
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::IndexArg(_)
        | ResolvedTypeExpr::Quantity(_)
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
        hir::ExprKind::Error { children } => {
            for child in children {
                collect_resolved_collection_refs_from_expr(child, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::OffsetDateTimeLiteral(_)
        | hir::ExprKind::CivilDateTimeLiteral(_)
        | hir::ExprKind::ZonedDateTimeLiteral(_)
        | hir::ExprKind::IanaTimeZoneLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::ConstRef(_)
        | hir::ExprKind::QuantityLiteral { .. } => Ok(()),
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
                        hir::expr::MapEntryKey::FinitePosition { .. } => {}
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
                    hir::expr::ForBindingIndex::Finite { .. } => {}
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
        hir::ExprKind::Unfold {
            recurrence,
            init,
            body,
        } => {
            let axis = &recurrence.axis;
            record_resolved_collection_index(&axis.value, ctx, src, axis.span, refs)?;
            collect_resolved_collection_refs_from_expr(init, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(body, ctx, src, refs)
        }
        hir::ExprKind::KeyForm { axis, arg, .. } => {
            if let hir::expr::ForBindingIndex::Named(axis) = axis {
                record_resolved_collection_index(&axis.value, ctx, src, axis.span, refs)?;
            }
            collect_resolved_collection_refs_from_expr(arg, ctx, src, refs)
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
        hir::ExprKind::DagCall { args, .. } => {
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
        } => {
            collect_resolved_collection_refs_from_expr(actual, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(expected, ctx, src, refs)?;
            collect_resolved_collection_refs_from_expr(tolerance, ctx, src, refs)
        }
    }
}

pub(super) fn collect_resolved_constructor_refs(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    asserts: &[crate::ir::lower::AssertEntry],
    domain_bounds: &HashMap<ResolvedDeclName, Vec<ResolvedDomainBound>>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedConstructorRefs, GraphcalError> {
    let mut refs = ResolvedConstructorRefs::default();

    for entry in consts {
        collect_resolved_constructor_refs_from_expr(
            &entry.expr,
            ctx,
            entry.body_src.resolve(src),
            &mut refs,
        )?;
    }
    for entry in params {
        if let Some(expr) = &entry.default_expr {
            let default_src = entry.default_src.as_ref().ok_or_else(|| {
                internal_error(
                    format!("parameter `{}` is missing default provenance", entry.name),
                    src,
                    entry.span,
                )
            })?;
            collect_resolved_constructor_refs_from_expr(
                expr,
                ctx,
                default_src.resolve(src),
                &mut refs,
            )?;
        }
    }
    for entry in nodes {
        collect_resolved_constructor_refs_from_expr(
            &entry.expr,
            ctx,
            entry.body_src.resolve(src),
            &mut refs,
        )?;
    }
    for bounds in domain_bounds.values() {
        for bound in bounds {
            collect_resolved_constructor_refs_from_expr(&bound.value, ctx, &bound.src, &mut refs)?;
        }
    }
    for entry in asserts {
        collect_resolved_constructor_refs_from_assert_body(
            &entry.body,
            ctx,
            entry.body_src.resolve(src),
            &mut refs,
        )?;
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
        hir::ExprKind::Error { children } => {
            for child in children {
                collect_resolved_constructor_refs_from_expr(child, ctx, src, refs)?;
            }
            Ok(())
        }
        hir::ExprKind::Number(_)
        | hir::ExprKind::Integer(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::OffsetDateTimeLiteral(_)
        | hir::ExprKind::CivilDateTimeLiteral(_)
        | hir::ExprKind::ZonedDateTimeLiteral(_)
        | hir::ExprKind::IanaTimeZoneLiteral(_)
        | hir::ExprKind::TypeSystemRef(_)
        | hir::ExprKind::GraphRef(_)
        | hir::ExprKind::LocalRef(_)
        | hir::ExprKind::QuantityLiteral { .. }
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
        hir::ExprKind::KeyForm { arg, .. } => {
            collect_resolved_constructor_refs_from_expr(arg, ctx, src, refs)
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
        hir::ExprKind::DagCall { args, .. } => {
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
        } => {
            collect_resolved_constructor_refs_from_expr(actual, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(expected, ctx, src, refs)?;
            collect_resolved_constructor_refs_from_expr(tolerance, ctx, src, refs)
        }
    }
}

pub(super) fn collect_hir_decl_bindings(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
) -> HashMap<ScopedName, ResolvedDeclName> {
    let mut bindings = HashMap::new();

    for (name, declaration_owner) in consts
        .iter()
        .map(|entry| (&entry.name, &entry.declaration_owner))
        .chain(
            params
                .iter()
                .map(|entry| (&entry.name, &entry.declaration_owner)),
        )
        .chain(
            nodes
                .iter()
                .map(|entry| (&entry.name, &entry.declaration_owner)),
        )
    {
        bindings.insert(
            name.clone(),
            ResolvedDeclName::from_def(declaration_owner.clone(), name.member().clone()),
        );
    }

    bindings.extend(
        imported_bindings
            .iter()
            .map(|(name, binding)| (name.clone(), binding.target().clone())),
    );
    bindings
}

pub(super) fn collect_resolved_decl_bindings(
    consts: &[crate::ir::lower::ConstEntry],
    params: &[crate::ir::lower::ParamEntry],
    nodes: &[crate::ir::lower::NodeEntry],
    imported_bindings: &HashMap<ScopedName, crate::ir::imported_binding::ImportedBinding>,
) -> HashMap<ScopedName, ResolvedDeclName> {
    collect_hir_decl_bindings(consts, params, nodes, imported_bindings)
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
                                    crate::registry::resolve_types::ExpectedFailKeyPart::FinitePosition {
                                        position,
                                        span,
                                    } => Ok(
                                        crate::registry::resolve_types::ExpectedFailKeyPart::FinitePosition {
                                            position,
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

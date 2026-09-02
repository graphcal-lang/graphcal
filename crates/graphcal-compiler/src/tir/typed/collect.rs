use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir;
use crate::ir::lower::ParsedExpectedFailMetadata;
use crate::ir::resolve::ExpectedFail;
use crate::registry::error::GraphcalError;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::ResolvedConstructorName;

use super::{
    DagTIR, ModuleTypeContext, ResolvedConstructorRefs, ResolvedConstructorTarget,
    ResolvedDagDependencies, ResolvedDomainBound, ResolvedExpectedFailMetadata, internal_error,
    module_resolve_error,
};

pub(super) fn augment_runtime_deps_for_dynamic_units(
    dag: &mut DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if dag.semantic.dynamic_unit_scales.is_empty() {
        return Ok(());
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
        .filter_map(|entry| entry.default.as_ref().map(|default| (entry, default)))
        .map(|(entry, default)| {
            let entry_src = default.src.resolve(src);
            Ok((
                dag.require_bound_decl_identity(
                    &entry.name,
                    entry_src,
                    DiagnosticAnchor::Source(entry.span),
                )?,
                collect_unit_names(&default.expr),
            ))
        })
        .chain(dag.nodes.iter().map(|entry| {
            let entry_src = entry.body_src.resolve(src);
            Ok((
                dag.require_bound_decl_identity(
                    &entry.name,
                    entry_src,
                    DiagnosticAnchor::Source(entry.span),
                )?,
                collect_unit_names(&entry.expr),
            ))
        }))
        .collect::<Result<Vec<_>, GraphcalError>>()?;

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
    Ok(())
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
        let deps = entry
            .default
            .as_ref()
            .map_or_else(hir::ExprDependencies::default, |default| {
                hir::collect_expr_dependencies(&default.expr)
            });
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
        if let Some(default) = &entry.default {
            collect_resolved_constructor_refs_from_expr(
                &default.expr,
                ctx,
                default.src.resolve(src),
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
    expected_fail: HashMap<ScopedName, ParsedExpectedFailMetadata>,
    ctx: ModuleTypeContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ResolvedExpectedFailMetadata>, GraphcalError> {
    expected_fail
        .into_iter()
        .map(|(assert_name, metadata)| {
            let ParsedExpectedFailMetadata {
                expected,
                resolution_owner,
                src: metadata_src,
                attribute_span,
            } = metadata;
            let metadata_src = metadata_src.resolve(src).clone();
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
                                            .resolve_index_variant_parts(
                                                &resolution_owner,
                                                &index,
                                                &variant,
                                            )
                                            .map_err(|err| {
                                                module_resolve_error(&err, &metadata_src, span)
                                            })?;
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
            Ok((
                assert_name,
                ResolvedExpectedFailMetadata {
                    expected: resolved,
                    src: metadata_src,
                    attribute_span,
                },
            ))
        })
        .collect()
}

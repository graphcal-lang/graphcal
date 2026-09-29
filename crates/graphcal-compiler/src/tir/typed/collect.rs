use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir;
use crate::registry::error::GraphcalError;
use crate::resolved_name::{ResolvedConstructorName, ResolvedDeclName};
use crate::syntax::span::Span;

use super::{
    DagTIR, ModuleTypeContext, ResolvedConstructorRefs, ResolvedConstructorTarget,
    ResolvedDagDependencies, internal_error, module_resolve_error,
};

pub(super) fn augment_runtime_deps_for_dynamic_units(
    dag: &mut DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if dag.semantic.dynamic_unit_scales.is_empty() {
        return Ok(());
    }
    let scale_deps: HashMap<crate::resolved_name::ResolvedUnitName, BTreeSet<ResolvedDeclName>> =
        dag.semantic
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
            Ok((
                dag.require_bound_decl_identity(
                    &entry.name,
                    src,
                    DiagnosticAnchor::Source(entry.span),
                )?,
                collect_unit_names(default),
            ))
        })
        .chain(dag.nodes.iter().map(|entry| {
            Ok((
                dag.require_bound_decl_identity(
                    &entry.name,
                    src,
                    DiagnosticAnchor::Source(entry.span),
                )?,
                entry
                    .definition
                    .formula()
                    .map_or_else(Default::default, |expression| {
                        collect_unit_names(expression)
                    }),
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
) -> std::collections::HashSet<crate::resolved_name::ResolvedUnitName> {
    let mut names = std::collections::HashSet::new();
    collect_unit_names_from_hir(expr, &mut names);
    names
}

/// Only literal scales are computational prerequisites. Conversion targets
/// select display computations after the frame's SI values exist, including
/// self/forward references; an unselected display target schedules no work.
fn collect_unit_names_from_hir(
    expr: &hir::Expr,
    names: &mut std::collections::HashSet<crate::resolved_name::ResolvedUnitName>,
) {
    hir::visit_expr(expr, &mut |node| {
        let unit = match node.kind() {
            hir::ExprKind::QuantityLiteral { unit, .. } => Some(unit),
            _ => None,
        };
        if let Some(unit) = unit {
            names.extend(
                unit.terms
                    .iter()
                    .map(|term| term.name.value.resolved().clone()),
            );
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
        let key =
            ResolvedDeclName::from_def(entry.declaration_owner.clone(), entry.name.leaf().clone());
        let mut deps = hir::collect_expr_dependencies(&entry.expr);
        for graph_ref in &deps.graph_refs {
            // `@const_name` in a const body is a const dependency. Non-const
            // `@` targets are rejected with a spanned diagnostic by
            // `check_hir_body_policies`.
            let kind = ctx
                .resolver
                .symbol(graph_ref)
                .map(|symbol| *symbol.kind())
                .ok_or_else(|| {
                    module_resolve_error(
                        &crate::resolve::error::ModuleResolveError::UnknownName {
                            owner: graph_ref.owner().clone(),
                            category: crate::resolve::error::NameCategory::Table(
                                crate::resolve::category::SymbolTable::Decl,
                            ),
                            name: graph_ref.atom().clone(),
                        },
                        src,
                        entry.span,
                    )
                })?;
            if kind.is_const() {
                deps.const_refs.insert(graph_ref.clone());
            }
        }
        resolved.const_deps.insert(key, deps.const_refs);
    }

    for entry in params {
        let key =
            ResolvedDeclName::from_def(entry.declaration_owner.clone(), entry.name.leaf().clone());
        let deps = entry
            .default
            .as_ref()
            .map_or_else(hir::ExprDependencies::default, |default| {
                hir::collect_expr_dependencies(default)
            });
        resolved.runtime_deps.insert(key, deps.graph_refs);
    }

    for entry in nodes {
        let key =
            ResolvedDeclName::from_def(entry.declaration_owner.clone(), entry.name.leaf().clone());
        let dependencies = match &entry.definition {
            crate::node_definition::NodeDefinition::Formula(expression) => {
                hir::collect_expr_dependencies(expression).graph_refs
            }
            crate::node_definition::NodeDefinition::Todo(dependencies) => dependencies
                .value
                .iter()
                .map(|reference| reference.value.clone())
                .collect(),
        };
        resolved.runtime_deps.insert(key, dependencies);
    }

    Ok(resolved)
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
    let mut result = Ok(());
    hir::visit_expr(expr, &mut |node| {
        if result.is_err() {
            return;
        }
        result = (|| {
            match node.kind() {
                hir::ExprKind::ConstructorCall { callee, .. } => {
                    record_resolved_constructor_target(&callee.value, ctx, src, callee.span, refs)?;
                }
                hir::ExprKind::ConstRef(target) => {
                    if let hir::ConstRef::Constructor(constructor) = &target.value {
                        record_resolved_constructor_target(
                            constructor,
                            ctx,
                            src,
                            target.span,
                            refs,
                        )?;
                    }
                }
                hir::ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        if let hir::expr::MatchPattern::Constructor { constructor, .. } =
                            &arm.pattern
                        {
                            record_resolved_constructor_target(
                                &constructor.value,
                                ctx,
                                src,
                                constructor.span,
                                refs,
                            )?;
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        })();
    });
    result
}

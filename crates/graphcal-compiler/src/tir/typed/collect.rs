use std::collections::{BTreeSet, HashMap, HashSet};

use crate::ir::instance::frame::InstanceFrame;
use crate::resolved_name::{ResolvedConstructorName, ResolvedDeclName, ResolvedStructTypeName};
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::syntax::span::Span;

use super::model::{DagTIR, ResolvedDagDependencies};
use super::module_type_context::ModuleTypeContext;
use super::type_expr::{internal_error, module_resolve_error};

pub(super) fn augment_runtime_deps_for_dynamic_units(dag: &mut DagTIR) {
    if dag.semantic.dynamic_unit_scales.is_empty() {
        return;
    }
    let scale_deps: HashMap<crate::resolved_name::ResolvedUnitName, BTreeSet<ResolvedDeclName>> =
        dag.semantic
            .dynamic_unit_scales
            .iter()
            .map(|(name, entry)| {
                (
                    name.clone(),
                    crate::hir::expr::collect_expr_dependencies(&entry.expr)
                        .graph_refs
                        .iter()
                        .map(|reference| dag.frame.resolve(reference))
                        .collect(),
                )
            })
            .collect();
    let runtime_units = dag
        .params()
        .filter_map(|entry| {
            entry
                .default
                .as_ref()
                .map(|default| (entry.identity(), collect_unit_names(&dag.frame, default)))
        })
        .chain(dag.nodes().map(|entry| {
            (
                entry.identity(),
                entry
                    .definition
                    .formula()
                    .map_or_else(Default::default, |expression| {
                        collect_unit_names(&dag.frame, expression)
                    }),
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

/// The runtime units whose scales the quantity literals of `expr` read, as
/// the DAG running it in `frame` names them.
fn collect_unit_names(
    frame: &InstanceFrame,
    expr: &crate::hir::expr::Expr,
) -> std::collections::HashSet<crate::resolved_name::ResolvedUnitName> {
    let mut names = std::collections::HashSet::new();
    collect_unit_names_from_hir(frame, expr, &mut names);
    names
}

/// Only literal scales are computational prerequisites. Conversion targets
/// select display computations after the frame's SI values exist, including
/// self/forward references; an unselected display target schedules no work.
fn collect_unit_names_from_hir(
    frame: &InstanceFrame,
    expr: &crate::hir::expr::Expr,
    names: &mut std::collections::HashSet<crate::resolved_name::ResolvedUnitName>,
) {
    crate::hir::expr::visit_expr(expr, &mut |node| {
        let unit = match node.kind() {
            crate::hir::expr::ExprKind::QuantityLiteral { unit, .. } => Some(unit),
            _ => None,
        };
        if let Some(unit) = unit {
            names.extend(
                unit.terms
                    .iter()
                    .map(|term| frame.resolve_unit(&term.name.value)),
            );
        }
    });
}

/// The declarations `references` name in `frame`.
fn resolve_all<'a>(
    frame: &InstanceFrame,
    references: impl IntoIterator<Item = &'a crate::hir::expr::LocalDecl>,
) -> BTreeSet<ResolvedDeclName> {
    references
        .into_iter()
        .map(|reference| frame.resolve(reference))
        .collect()
}

pub(super) fn collect_resolved_dag_dependencies(
    decls: &crate::ir::decl_table::DeclTable<super::model::Typed>,
    frame: &InstanceFrame,
    ctx: ModuleTypeContext<'_>,
    src: SourceId,
) -> Result<ResolvedDagDependencies, SemanticError> {
    let mut resolved = ResolvedDagDependencies::default();

    for entry in decls.consts() {
        let key = entry.identity();
        let deps = crate::hir::expr::collect_expr_dependencies(&entry.expr);
        let mut const_refs = resolve_all(frame, &deps.const_refs);
        for graph_ref in &resolve_all(frame, &deps.graph_refs) {
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
                const_refs.insert(graph_ref.clone());
            }
        }
        resolved.const_deps.insert(key, const_refs);
    }

    for entry in decls.params() {
        let key = entry.identity();
        let deps = entry
            .default
            .as_ref()
            .map_or_else(crate::hir::expr::ExprDependencies::default, |default| {
                crate::hir::expr::collect_expr_dependencies(default)
            });
        resolved
            .runtime_deps
            .insert(key, resolve_all(frame, &deps.graph_refs));
    }

    for entry in decls.nodes() {
        let key = entry.identity();
        let dependencies = match &entry.definition {
            crate::node_definition::NodeDefinition::Formula(expression) => resolve_all(
                frame,
                &crate::hir::expr::collect_expr_dependencies(expression).graph_refs,
            ),
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

fn record_constructed_type(
    constructor: &ResolvedConstructorName,
    ctx: ModuleTypeContext<'_>,
    src: SourceId,
    span: Span,
    constructed_types: &mut HashSet<ResolvedStructTypeName>,
) -> Result<(), SemanticError> {
    let resolved = ctx.types.lookup_constructor(constructor).ok_or_else(|| {
        internal_error(
            format!("semantic constructor metadata references unknown constructor `{constructor}`"),
            src,
            span,
        )
    })?;
    constructed_types.insert(resolved.owning_type().clone());
    Ok(())
}

/// Collect the owning types of every constructor a body calls, names, or matches.
pub(super) fn collect_constructed_types_from_expr(
    expr: &crate::hir::expr::Expr,
    ctx: ModuleTypeContext<'_>,
    src: SourceId,
    constructed_types: &mut HashSet<ResolvedStructTypeName>,
) -> Result<(), SemanticError> {
    let mut result = Ok(());
    crate::hir::expr::visit_expr(expr, &mut |node| {
        if result.is_err() {
            return;
        }
        result = (|| {
            match node.kind() {
                crate::hir::expr::ExprKind::ConstructorCall { callee, .. } => {
                    record_constructed_type(
                        &callee.value,
                        ctx,
                        src,
                        callee.span,
                        constructed_types,
                    )?;
                }
                crate::hir::expr::ExprKind::ConstRef(target) => {
                    if let crate::hir::expr::ConstRef::Constructor(constructor) = &target.value {
                        record_constructed_type(
                            constructor,
                            ctx,
                            src,
                            target.span,
                            constructed_types,
                        )?;
                    }
                }
                crate::hir::expr::ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        if let crate::hir::expr::MatchPattern::Constructor { constructor, .. } =
                            &arm.pattern
                        {
                            record_constructed_type(
                                &constructor.value,
                                ctx,
                                src,
                                constructor.span,
                                constructed_types,
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

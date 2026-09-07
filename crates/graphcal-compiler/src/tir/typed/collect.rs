use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use crate::assertion_expectation::ExpectedFail;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir;
use crate::ir::lower::ParsedExpectedFailMetadata;
use crate::registry::error::GraphcalError;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::ResolvedConstructorName;

use super::{
    DagTIR, ModuleTypeContext, ResolvedConstructorRefs, ResolvedConstructorTarget,
    ResolvedDagDependencies, ResolvedExpectedFailMetadata, internal_error, module_resolve_error,
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

/// Only literal scales are computational prerequisites. Conversion targets
/// select display computations after the frame's SI values exist, including
/// self/forward references; an unselected display target schedules no work.
fn collect_unit_names_from_hir(
    expr: &hir::Expr,
    names: &mut std::collections::HashSet<crate::syntax::dimension::ResolvedUnitName>,
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
                                    crate::assertion_expectation::ExpectedFailKeyPart::Named {
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
                                        Ok(crate::assertion_expectation::ExpectedFailKeyPart::resolved(
                                            resolved, span,
                                        ))
                                    }
                                    crate::assertion_expectation::ExpectedFailKeyPart::FinitePosition {
                                        position,
                                        span,
                                    } => Ok(
                                        crate::assertion_expectation::ExpectedFailKeyPart::FinitePosition {
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

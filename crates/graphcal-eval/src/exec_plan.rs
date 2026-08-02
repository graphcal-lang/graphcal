//! Execution plan — the result of compiling a TIR.
//!
//! Contains evaluated const values, topologically sorted runtime declarations,
//! and their expressions, ready for evaluation.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::hir::AssertBody;
use graphcal_compiler::registry::declared_type::StructTypeRef;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::{Span, Spanned};
use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName};
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::{
    ResolvedDomainBound as EvaluatedDomainBound, ResolvedDomainBounds as EvaluatedDomainBounds,
    ResolvedDomainConstraint,
};
use crate::eval_expr::{
    EvalContext, HirLocalValueMap, RuntimeValue, RuntimeValueMap, eval_hir_expr,
};
use graphcal_compiler::registry::builtins::{builtin_constants, builtin_functions};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::{
    DagTIR, ResolvedDagDependencies, StructFieldConstraintKey, TIR,
};

/// An assert body entry for execution.
#[derive(Debug, Clone)]
pub struct AssertBodyEntry {
    pub(crate) name: ScopedName,
    pub(crate) body: AssertBody,
    pub(crate) span: Span,
}

/// A plot body entry for execution.
#[derive(Debug, Clone)]
pub struct PlotBodyEntry {
    pub(crate) name: ScopedName,
    /// Mark shape rendered for this plot.
    pub(crate) mark_type: graphcal_compiler::syntax::ast::MarkType,
    /// Whether this plot renders standalone (no `#[hidden]`; #847).
    pub(crate) displayed: bool,
}

/// A figure body entry for execution.
#[derive(Debug, Clone)]
pub struct FigureBodyEntry {
    pub(crate) name: ScopedName,
    /// Plots composed by this figure, in source order.
    pub(crate) plot_names: Vec<Spanned<ScopedName>>,
}

/// A layer body entry for execution.
#[derive(Debug, Clone)]
pub struct LayerBodyEntry {
    pub(crate) name: ScopedName,
    /// Plots composed by this layer, in source order.
    pub(crate) plot_names: Vec<Spanned<ScopedName>>,
}

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    /// Evaluated const values (in base SI units).
    /// Key-lookup only, order irrelevant.
    pub(crate) const_values: RuntimeValueMap,
    /// Pre-evaluated values imported from dependency files.
    /// These are injected directly into the evaluation environment.
    /// Iterated once during env setup; feeds into `HashMap` (key-lookup only).
    pub(crate) imported_values: RuntimeValueMap,
    /// Topologically sorted names for runtime evaluation (params + nodes).
    pub(crate) topo_order: Vec<RuntimeDeclKey>,
    /// Assert bodies in source order.
    pub(crate) assert_bodies: Vec<AssertBodyEntry>,
    /// Plot declarations in source order.
    pub(crate) plot_bodies: Vec<PlotBodyEntry>,
    /// Figure declarations in source order.
    pub(crate) figure_bodies: Vec<FigureBodyEntry>,
    /// Layer declarations in source order.
    pub(crate) layer_bodies: Vec<LayerBodyEntry>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Key-lookup only, order irrelevant.
    pub(crate) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Key-lookup only, order irrelevant.
    pub(crate) expected_fail: HashMap<ScopedName, graphcal_compiler::ir::resolve::ExpectedFail>,
    /// Resolved domain constraints for runtime validation, keyed by declaration name.
    /// Key-lookup only, order irrelevant.
    pub(crate) domain_constraints: HashMap<RuntimeDeclKey, ResolvedDomainConstraint>,
    /// Resolved domain constraints for struct/union member fields, keyed by
    /// owner-qualified struct/constructor/field identity. Looked up at every
    /// `ExprKind::ConstructorCall` evaluation to validate field values.
    pub(crate) struct_field_constraints:
        HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
}

/// Compile a TIR into an execution plan.
///
/// This performs:
/// 1. Topological sort of const declarations + compile-time evaluation
/// 2. Topological sort of runtime declarations (params + nodes) into evaluation order
///
/// # Errors
///
/// Returns a [`GraphcalError`] if there is a cyclic dependency or if
/// const evaluation fails.
#[cfg(test)]
pub fn compile(tir: &TIR, src: &NamedSource<Arc<String>>) -> Result<ExecPlan, GraphcalError> {
    compile_with_cancellation(
        tir,
        src,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile a TIR into an execution plan with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid plan or cancellation.
pub fn compile_with_cancellation(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    cancellation.checkpoint()?;
    let const_values = eval_consts_from_tir_with_cancellation(tir, src, cancellation)?;
    cancellation.checkpoint()?;
    let topo_order = build_runtime_dag(tir, src, cancellation)?;

    let root = tir.root();
    let assert_bodies: Vec<AssertBodyEntry> = root
        .asserts
        .iter()
        .map(|entry| {
            let key = root.resolved_decl_key_for_local(&entry.name);
            let body = root
                .semantic
                .expressions
                .asserts
                .get(&key)
                .cloned()
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!("semantic HIR body missing for assertion `{}`", entry.name),
                    src: src.clone(),
                    span: entry.span.into(),
                })?;
            Ok(AssertBodyEntry {
                name: entry.name.clone(),
                body,
                span: entry.span,
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;

    let plot_bodies: Vec<PlotBodyEntry> = tir
        .root()
        .plots
        .iter()
        .map(|entry| PlotBodyEntry {
            name: entry.name.clone(),
            mark_type: entry.mark_type,
            displayed: entry.displayed,
        })
        .collect();

    let figure_bodies: Vec<FigureBodyEntry> = tir
        .root()
        .figures
        .iter()
        .map(|entry| FigureBodyEntry {
            name: entry.name.clone(),
            plot_names: entry.plot_names.clone(),
        })
        .collect();

    let layer_bodies: Vec<LayerBodyEntry> = tir
        .root()
        .layers
        .iter()
        .map(|entry| LayerBodyEntry {
            name: entry.name.clone(),
            plot_names: entry.plot_names.clone(),
        })
        .collect();

    // Resolve domain constraints from type annotations.
    cancellation.checkpoint()?;
    let domain_constraints =
        resolve_domain_constraints_with_cancellation(tir, &const_values, src, cancellation)?;
    // Resolve domain constraints declared on struct/union member fields.
    cancellation.checkpoint()?;
    let struct_field_constraints =
        resolve_struct_field_constraints_with_cancellation(tir, &const_values, src, cancellation)?;

    // Validate struct field constraints against const struct values. Const
    // evaluation runs before field constraints are resolved (the constraint
    // bound exprs themselves need const values), so the violation check is
    // deferred to here. Top-level struct-typed consts that violate any field
    // constraint produce a compile-time `DomainViolation`.
    cancellation.checkpoint()?;
    check_const_struct_field_constraints_at_compile_time(
        tir,
        &const_values,
        &struct_field_constraints,
        src,
    )?;

    Ok(ExecPlan {
        const_values,
        imported_values: tir
            .root()
            .imported_values
            .iter()
            .map(|(k, (v, _dt))| (RuntimeDeclKey::for_visible_name(tir.root(), k), v.clone()))
            .collect(),
        topo_order,
        assert_bodies,
        plot_bodies,
        figure_bodies,
        layer_bodies,
        assumes_map: tir.root().assumes_map.clone(),
        expected_fail: tir.root().expected_fail.clone(),
        domain_constraints,
        struct_field_constraints,
    })
}

type ResolvedDeclKey = graphcal_compiler::syntax::decl_name::ResolvedDeclName;

fn visible_values_with_imports(
    dag: &DagTIR,
    local_const_values: &RuntimeValueMap,
) -> RuntimeValueMap {
    let mut values: RuntimeValueMap = dag
        .imported_values
        .iter()
        .map(|(name, (value, _))| (RuntimeDeclKey::for_visible_name(dag, name), value.clone()))
        .collect();
    values.extend(
        local_const_values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    values
}

/// Topologically sort and evaluate const declarations from a TIR.
pub fn eval_consts_from_tir_with_cancellation(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<RuntimeValueMap, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let dag = tir.root();

    if dag.consts.is_empty() {
        return Ok(HashMap::new());
    }

    let sorted_names = const_eval_order(dag, src, cancellation)?;

    let empty_hir_locals = HirLocalValueMap::root();
    let mut visible_values = visible_values_with_imports(dag, &HashMap::new());
    let mut local_const_values: RuntimeValueMap = HashMap::new();

    for name in sorted_names {
        cancellation.checkpoint()?;
        let key = RuntimeDeclKey::for_local_decl(dag, &name);
        let ctx = EvalContext {
            cancellation: cancellation.clone(),
            builtin_consts,
            builtin_fns,
            registry: &tir.registry,
            src,
            tir,
            current_dag: Some(tir.root()),
            root_values: Some(&visible_values),
            struct_field_constraints: None,
            host_fns: None,
        };
        let hir_expr = dag
            .semantic
            .expressions
            .consts
            .get(key.as_resolved())
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("semantic TIR missing HIR const expression for `{name}`"),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })?;
        let value = eval_hir_expr(hir_expr, &visible_values, &empty_hir_locals, &ctx)?;
        visible_values.insert(key.clone(), value.clone());
        local_const_values.insert(key, value);
    }

    Ok(local_const_values)
}

fn const_eval_order(
    dag: &DagTIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    const_eval_order_resolved(dag, &dag.semantic.dependencies, src, cancellation)
}

fn const_eval_order_resolved(
    dag: &DagTIR,
    deps: &ResolvedDagDependencies,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    cancellation.checkpoint()?;
    let mut graph = DiGraph::<ResolvedDeclKey, ()>::new();
    let mut index_map: HashMap<ResolvedDeclKey, petgraph::graph::NodeIndex> = HashMap::new();
    let mut local_name_by_key: HashMap<ResolvedDeclKey, ScopedName> = HashMap::new();
    let mut span_by_key: HashMap<ResolvedDeclKey, Span> = HashMap::new();

    // Sort consts by name for canonical tie-breaking among incomparable nodes.
    let mut sorted_consts: Vec<&_> = dag.consts.iter().collect();
    sorted_consts.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in &sorted_consts {
        cancellation.checkpoint()?;
        let key = dag.resolved_decl_key_for_local(&entry.name);
        let idx = graph.add_node(key.clone());
        index_map.insert(key.clone(), idx);
        local_name_by_key.insert(key.clone(), entry.name.clone());
        span_by_key.insert(key, entry.span);
    }

    for (name, dep_set) in &deps.const_deps {
        cancellation.checkpoint()?;
        let Some(&dependent_idx) = index_map.get(name) else {
            continue;
        };
        for dep in dep_set {
            if let Some(&dep_idx) = index_map.get(dep) {
                graph.add_edge(dep_idx, dependent_idx, ());
            }
        }
    }

    let sorted = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = span_by_key
            .get(cycle_node)
            .copied()
            .unwrap_or_else(|| Span::new(0, 0));
        let name = local_name_by_key
            .get(cycle_node)
            .map_or_else(|| cycle_node.to_string(), std::string::ToString::to_string);
        GraphcalError::CyclicDependency {
            name,
            src: src.clone(),
            span: span.into(),
        }
    })?;

    Ok(sorted
        .into_iter()
        .filter_map(|idx| local_name_by_key.get(&graph[idx]).cloned())
        .collect())
}

/// Build a topologically sorted runtime DAG from params and nodes in a TIR.
fn build_runtime_dag(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<RuntimeDeclKey>, GraphcalError> {
    // Merge params and nodes, then sort by name for canonical tie-breaking
    // among incomparable nodes in the topological sort.
    enum DeclRef<'a> {
        Param(&'a graphcal_compiler::ir::lower::ParamEntry),
        Node(&'a graphcal_compiler::ir::lower::NodeEntry),
    }

    impl DeclRef<'_> {
        const fn name(&self) -> &ScopedName {
            match self {
                Self::Param(e) => &e.name,
                Self::Node(e) => &e.name,
            }
        }

        const fn span(&self) -> Span {
            match self {
                Self::Param(e) => e.span,
                Self::Node(e) => e.span,
            }
        }
    }

    cancellation.checkpoint()?;
    let dag = tir.root();
    let mut decl_spans: Vec<(ScopedName, Span)> = Vec::new();

    let mut all_decls: Vec<DeclRef<'_>> = dag
        .params
        .iter()
        .map(DeclRef::Param)
        .chain(dag.nodes.iter().map(DeclRef::Node))
        .collect();
    all_decls.sort_by(|a, b| a.name().cmp(b.name()));

    for decl in &all_decls {
        cancellation.checkpoint()?;
        let name = decl.name().clone();
        decl_spans.push((name.clone(), decl.span()));
        match decl {
            DeclRef::Param(entry) if entry.default_expr.is_none() => {
                return Err(GraphcalError::RequiredParamNotProvided {
                    name: name.to_string(),
                    src: src.clone(),
                    span: entry.span.into(),
                });
            }
            DeclRef::Param(_) | DeclRef::Node(_) => {}
        }
    }

    Ok(runtime_eval_order(dag, &decl_spans, src, cancellation)?
        .into_iter()
        .map(|name| RuntimeDeclKey::for_local_decl(dag, &name))
        .collect())
}

fn runtime_eval_order(
    dag: &DagTIR,
    decl_spans: &[(ScopedName, Span)],
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    runtime_eval_order_resolved(
        dag,
        decl_spans,
        &dag.semantic.dependencies,
        src,
        cancellation,
    )
}

fn runtime_eval_order_resolved(
    dag: &DagTIR,
    decl_spans: &[(ScopedName, Span)],
    deps: &ResolvedDagDependencies,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<ScopedName>, GraphcalError> {
    cancellation.checkpoint()?;
    let mut graph = DiGraph::<ResolvedDeclKey, ()>::new();
    let mut index_map: HashMap<ResolvedDeclKey, petgraph::graph::NodeIndex> = HashMap::new();
    let mut local_name_by_key: HashMap<ResolvedDeclKey, ScopedName> = HashMap::new();
    let mut span_by_key: HashMap<ResolvedDeclKey, Span> = HashMap::new();

    for (name, span) in decl_spans {
        cancellation.checkpoint()?;
        let key = dag.resolved_decl_key_for_local(name);
        let idx = graph.add_node(key.clone());
        index_map.insert(key.clone(), idx);
        local_name_by_key.insert(key.clone(), name.clone());
        span_by_key.insert(key, *span);
    }

    for (name, dep_set) in &deps.runtime_deps {
        cancellation.checkpoint()?;
        let Some(&dependent_idx) = index_map.get(name) else {
            continue;
        };
        for dep in dep_set {
            if let Some(&dep_idx) = index_map.get(dep) {
                graph.add_edge(dep_idx, dependent_idx, ());
            }
        }
    }

    let topo_indices = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = span_by_key
            .get(cycle_node)
            .copied()
            .unwrap_or_else(|| Span::new(0, 0));
        let name = local_name_by_key
            .get(cycle_node)
            .map_or_else(|| cycle_node.to_string(), std::string::ToString::to_string);
        GraphcalError::CyclicDependency {
            name,
            src: src.clone(),
            span: span.into(),
        }
    })?;

    Ok(topo_indices
        .into_iter()
        .filter_map(|idx| local_name_by_key.get(&graph[idx]).cloned())
        .collect())
}

/// Resolve domain constraints from type annotations on consts, params, and nodes.
///
/// Evaluates each compile-time bound expression using const values and builtins,
/// selects the target's quantity, integer, or datetime representation, and
/// checks `min <= max`. Bound types and target compatibility are validated
/// earlier in `dim_check::check_dimensions_tir`.
///
/// For const declarations, the resolved constraint is also checked against the
/// already-evaluated const value at compile time, raising `DomainViolation`
/// if the value is out of bounds.
pub fn resolve_domain_constraints_with_cancellation(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let visible_const_values = visible_values_with_imports(tir.root(), const_values);

    let ctx = EvalContext {
        cancellation: cancellation.clone(),
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        tir,
        current_dag: Some(tir.root()),
        root_values: Some(&visible_const_values),
        struct_field_constraints: None,
        // Compile-time contexts carry no host registry; dimension checking
        // rejects extern calls in const and unit-scale positions.
        host_fns: None,
    };

    let mut constraints = HashMap::new();

    // Iterate over consts, params, and nodes with stored HIR domain bounds.
    // The `is_const` flag selects whether an immediate compile-time
    // value-vs-constraint check runs below; params/nodes defer that check to
    // `eval/runtime.rs`.
    let decl_iter = tir
        .root()
        .consts
        .iter()
        .map(|e| (&e.name, e.span, true))
        .chain(tir.root().params.iter().map(|e| (&e.name, e.span, false)))
        .chain(tir.root().nodes.iter().map(|e| (&e.name, e.span, false)));

    for (name, decl_span, is_const) in decl_iter {
        cancellation.checkpoint()?;
        let key = tir.root().resolved_decl_key_for_local(name);
        let domain_bounds = tir.root().semantic.domain_bounds.get(&key);
        let Some(domain_bounds) = domain_bounds else {
            continue;
        };

        let constraint_src = domain_bounds.first().map_or(src, |bound| &bound.src);
        let target = resolve_constraint_target(
            &name.to_string(),
            tir.root().resolved_decl_types.get(name).map(strip_indexed),
            decl_span,
            constraint_src,
        )?;
        let bound_ctx = ctx.with_src(constraint_src);
        let resolved_constraint = resolve_constraint_from_bounds(
            domain_bounds,
            &name.to_string(),
            target,
            &visible_const_values,
            &bound_ctx,
            constraint_src,
        )?;

        // For const declarations, validate the (already-known) value at compile time.
        let key = RuntimeDeclKey::for_local_decl(tir.root(), name);
        if is_const
            && let Some(value) = const_values.get(&key)
            && let Err(violation) =
                crate::domain_check::check_domain_constraint(value, &resolved_constraint)
        {
            return Err(GraphcalError::DomainViolation {
                name: name.to_string(),
                value: format_runtime_value(value),
                violation: violation.message,
                src: src.clone(),
                span: decl_span.into(),
            });
        }

        constraints.insert(key, resolved_constraint);
    }

    Ok(constraints)
}

#[derive(Debug, Clone, Copy)]
enum ConstraintTarget {
    Quantity,
    Int,
    Datetime(graphcal_compiler::registry::time_scale::TimeScale),
}

/// Evaluate a declaration's or field's stored HIR domain bounds into the
/// representation selected by its constrained value family.
fn resolve_constraint_from_bounds(
    bounds: &[graphcal_compiler::tir::typed::ResolvedDomainBound],
    display_name: &str,
    target: ConstraintTarget,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDomainConstraint, GraphcalError> {
    match target {
        ConstraintTarget::Quantity => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            |value, bound| match value {
                RuntimeValue::Quantity(value) => Ok(*value),
                RuntimeValue::Int(value) => exact_domain_int_bound(*value, src, bound.value.span),
                other => Err(domain_bound_value_error(
                    display_name,
                    bound,
                    "a quantity",
                    other,
                    src,
                )),
            },
            |expr, value| format_quantity_bound_display(expr, *value),
        )
        .map(ResolvedDomainConstraint::quantity),
        ConstraintTarget::Int => evaluate_domain_bounds(
            bounds,
            display_name,
            values,
            ctx,
            src,
            |value, bound| match value {
                RuntimeValue::Int(value) => Ok(*value),
                other => Err(domain_bound_value_error(
                    display_name,
                    bound,
                    "Int",
                    other,
                    src,
                )),
            },
            |_expr, value| value.to_string(),
        )
        .map(ResolvedDomainConstraint::int),
        ConstraintTarget::Datetime(scale) => {
            let evaluated = evaluate_domain_bounds(
                bounds,
                display_name,
                values,
                ctx,
                src,
                |value, bound| match value {
                    RuntimeValue::Datetime(epoch) if epoch.time_scale == scale.to_hifitime() => {
                        Ok(*epoch)
                    }
                    other => Err(domain_bound_value_error(
                        display_name,
                        bound,
                        &format!("Datetime<{scale}>"),
                        other,
                        src,
                    )),
                },
                |_expr, epoch| epoch.to_string(),
            )?;
            ResolvedDomainConstraint::datetime(scale, evaluated).map_err(|error| {
                GraphcalError::InternalError {
                    message: format!(
                        "datetime domain bounds on `{display_name}` violated their checked scale invariant: {error}"
                    ),
                    src: src.clone(),
                    span: bounds
                        .first()
                        .map_or_else(|| Span::new(0, 0), |bound| bound.span)
                        .into(),
                }
            })
        }
    }
}

fn evaluate_domain_bounds<T: PartialOrd>(
    bounds: &[graphcal_compiler::tir::typed::ResolvedDomainBound],
    display_name: &str,
    values: &RuntimeValueMap,
    ctx: &EvalContext<'_>,
    src: &NamedSource<Arc<String>>,
    convert: impl Fn(
        &RuntimeValue,
        &graphcal_compiler::tir::typed::ResolvedDomainBound,
    ) -> Result<T, GraphcalError>,
    format_display: impl Fn(&graphcal_compiler::hir::Expr, &T) -> String,
) -> Result<EvaluatedDomainBounds<T>, GraphcalError> {
    let Some(first) = bounds.first() else {
        return Err(GraphcalError::InternalError {
            message: format!("domain constraint on `{display_name}` has no bounds"),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        });
    };
    let empty_locals = HirLocalValueMap::root();
    let evaluated = bounds
        .iter()
        .map(|bound| {
            let runtime_value = eval_hir_expr(&bound.value, values, &empty_locals, ctx)?;
            let value = convert(&runtime_value, bound)?;
            let display = format_display(&bound.value, &value);
            Ok((bound.kind, EvaluatedDomainBound::new(value, display)))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let constraint_span = bounds
        .iter()
        .skip(1)
        .fold(first.span, |span, bound| span.merge(bound.span));
    let (min, max) =
        evaluated
            .into_iter()
            .fold((None, None), |(min, max), (kind, bound)| match kind {
                graphcal_compiler::syntax::ast::DomainBoundKind::Min => (Some(bound), max),
                graphcal_compiler::syntax::ast::DomainBoundKind::Max => (min, Some(bound)),
            });
    if let (Some(min), Some(max)) = (&min, &max)
        && min.value() > max.value()
    {
        return Err(GraphcalError::DomainMinExceedsMax {
            name: display_name.to_string(),
            min: min.display().to_string(),
            max: max.display().to_string(),
            src: src.clone(),
            span: constraint_span.into(),
        });
    }
    Ok(EvaluatedDomainBounds::new(min, max))
}

fn domain_bound_value_error(
    display_name: &str,
    bound: &graphcal_compiler::tir::typed::ResolvedDomainBound,
    expected: &str,
    actual: &RuntimeValue,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: format!(
            "{} domain bound on `{display_name}` must evaluate to {expected}, got {}",
            bound.kind,
            actual.kind()
        ),
        src: src.clone(),
        span: bound.value.span.into(),
    }
}

/// Resolve domain constraints declared on struct/union member fields.
///
/// Field bounds are stored as HIR in each DAG's semantic type defs
/// (`ResolvedTypeDefs.field_bounds`); this evaluates each constrained field's
/// typed `min`/`max` bounds, validates `min ≤ max`, and stores the result keyed
/// by the owning struct type, constructor, and field name —
/// under both the owner-qualified identity and the root-owned display leaf so
/// runtime lookups for boundary-created synthetic owners still hit.
///
/// Bound types and target compatibility are validated earlier by the compiler's
/// field-domain checks. This pass focuses on the runtime-relevant pieces: bound
/// evaluation, `min ≤ max`, and storage.
#[cfg(test)]
pub fn resolve_struct_field_constraints(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    resolve_struct_field_constraints_with_cancellation(
        tir,
        const_values,
        src,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

pub fn resolve_struct_field_constraints_with_cancellation(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>, GraphcalError> {
    cancellation.checkpoint()?;
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let visible_const_values = visible_values_with_imports(tir.root(), const_values);

    let ctx = EvalContext {
        cancellation: cancellation.clone(),
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        tir,
        current_dag: Some(tir.root()),
        root_values: Some(&visible_const_values),
        struct_field_constraints: None,
        // Compile-time contexts carry no host registry; dimension checking
        // rejects extern calls in const and unit-scale positions.
        host_fns: None,
    };

    let mut constraints = HashMap::new();
    let mut seen: std::collections::HashSet<
        &graphcal_compiler::tir::typed::ResolvedStructFieldTypeKey,
    > = std::collections::HashSet::new();

    for (id, dag) in &tir.dags {
        cancellation.checkpoint()?;
        if id != &tir.root_dag_id && id.parent().as_ref() != Some(&tir.root_dag_id) {
            continue;
        }
        for (key, bounds) in &dag.semantic.type_defs.field_bounds {
            if !seen.insert(key) {
                continue;
            }
            // Display name uses the constructor's leaf while semantic identity
            // remains the owning union type.
            let display_name = format!("{}.{}", key.constructor, key.field);
            let bound_span = bounds
                .first()
                .map_or_else(|| Span::new(0, 0), |bound| bound.span);
            let constraint_src = bounds.first().map_or(src, |bound| &bound.src);
            let target = resolve_constraint_target(
                &display_name,
                dag.semantic.type_defs.field_type(key).map(strip_indexed),
                bound_span,
                constraint_src,
            )?;
            let bound_ctx = ctx.with_src(constraint_src);
            let constraint = resolve_constraint_from_bounds(
                bounds,
                &display_name,
                target,
                &visible_const_values,
                &bound_ctx,
                constraint_src,
            )?;
            // Store under both the owner-qualified identity and the
            // root-owned display leaf so runtime lookups for
            // boundary-created synthetic owners still hit.
            constraints.insert(
                StructFieldConstraintKey::new(
                    StructTypeRef::from_resolved(key.owning_type.clone()),
                    key.constructor.clone(),
                    key.field.clone(),
                ),
                constraint.clone(),
            );
            constraints.insert(
                StructFieldConstraintKey::new(
                    StructTypeRef::with_owner(
                        tir.root_dag_id.clone(),
                        key.owning_type.to_unowned_def_name(),
                    ),
                    key.constructor.clone(),
                    key.field.clone(),
                ),
                constraint,
            );
        }
    }

    Ok(constraints)
}

/// Walk every top-level const value and validate it against resolved
/// struct-field constraints. Used both inside [`compile`] and from the
/// `check`-only path in `eval/project/lowering.rs` so that struct-field
/// violations on const nodes surface under `graphcal check`, not only
/// during full evaluation.
///
/// # Errors
///
/// Returns the first [`GraphcalError::DomainViolation`] encountered.
pub fn check_const_struct_field_constraints_at_compile_time(
    tir: &TIR,
    const_values: &RuntimeValueMap,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for entry in &tir.root().consts {
        let key = RuntimeDeclKey::for_local_decl(tir.root(), &entry.name);
        if let Some(value) = const_values.get(&key) {
            let owning_type = tir
                .root()
                .resolved_decl_types
                .get(&entry.name)
                .and_then(struct_type_ref_from_resolved_type);
            check_const_struct_field_constraints(
                value,
                entry.name.member().as_str(),
                entry.span,
                owning_type.as_ref(),
                field_constraints,
                src,
            )?;
        }
    }
    Ok(())
}

fn struct_type_ref_from_resolved_type(
    resolved: &graphcal_compiler::tir::typed::ResolvedTypeExpr,
) -> Option<StructTypeRef> {
    match strip_indexed(resolved) {
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Struct(name, _)
        | graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericStruct { name, .. } => {
            Some(StructTypeRef::from_resolved(name.clone()))
        }
        _ => None,
    }
}

fn find_struct_field_constraint<'a>(
    field_constraints: &'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    owning_type: Option<&StructTypeRef>,
    constructor: &ConstructorName,
    field: &FieldName,
) -> Option<&'a ResolvedDomainConstraint> {
    owning_type.and_then(|owning_type| {
        field_constraints.get(&StructFieldConstraintKey::new(
            owning_type.clone(),
            constructor.clone(),
            field.clone(),
        ))
    })
}

/// Recursively validate a const value against resolved struct-field
/// constraints. For `RuntimeValue::Struct`, looks up each field's
/// owner-qualified struct/constructor/field constraint and emits
/// `DomainViolation` on the first violation. Indexed values recurse
/// element-wise; nested structs recurse field-wise. Other variants short-circuit
/// to `Ok(())`.
fn check_const_struct_field_constraints(
    value: &RuntimeValue,
    decl_name: &str,
    decl_span: Span,
    owning_type: Option<&StructTypeRef>,
    field_constraints: &HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match value {
        RuntimeValue::Struct { type_name, fields } => {
            let runtime_owning_type = StructTypeRef::from_resolved(type_name.resolved().clone());
            let effective_owning_type = owning_type.or(Some(&runtime_owning_type));
            let constructor = ConstructorName::from_atom(type_name.name().atom().clone());
            for (field_name, field_value) in fields {
                if let Some(constraint) = find_struct_field_constraint(
                    field_constraints,
                    effective_owning_type,
                    &constructor,
                    field_name,
                ) && let Err(violation) =
                    crate::domain_check::check_domain_constraint(field_value, constraint)
                {
                    return Err(GraphcalError::DomainViolation {
                        name: format!("{decl_name}.{field_name}"),
                        value: format_runtime_value(field_value),
                        violation: violation.message,
                        src: src.clone(),
                        span: decl_span.into(),
                    });
                }
                // Recurse for nested struct fields. The nested runtime value
                // carries its canonical owner when module-aware constructor
                // evaluation created it, so the recursive call can recover the
                // owner even without a field-declared type side channel.
                check_const_struct_field_constraints(
                    field_value,
                    &format!("{decl_name}.{field_name}"),
                    decl_span,
                    None,
                    field_constraints,
                    src,
                )?;
            }
            Ok(())
        }
        RuntimeValue::Indexed { entries, .. } => {
            for (variant, entry) in entries {
                check_const_struct_field_constraints(
                    entry,
                    &format!("{decl_name}.{variant}"),
                    decl_span,
                    owning_type,
                    field_constraints,
                    src,
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Format a runtime value for inclusion in a `DomainViolation` error message.
fn format_runtime_value(rv: &RuntimeValue) -> String {
    match rv {
        RuntimeValue::Quantity(v) => graphcal_compiler::registry::format::format_number(*v),
        RuntimeValue::Int(i) => format!("{i}"),
        RuntimeValue::Datetime(epoch) => epoch.to_string(),
        RuntimeValue::Indexed { entries, .. } => {
            // Show the first violating entry's value if recoverable; otherwise summary.
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{k}: {}", format_runtime_value(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        other => format!("{other:?}"),
    }
}

/// Strip `Indexed` wrapper to get the base resolved type.
fn strip_indexed(
    resolved: &graphcal_compiler::tir::typed::ResolvedTypeExpr,
) -> &graphcal_compiler::tir::typed::ResolvedTypeExpr {
    match resolved {
        graphcal_compiler::tir::typed::ResolvedTypeExpr::Indexed { base, .. } => {
            strip_indexed(base)
        }
        other => other,
    }
}

/// Resolve the typed constraint family selected by a declaration or field type.
fn resolve_constraint_target(
    name: &str,
    base_resolved: Option<&graphcal_compiler::tir::typed::ResolvedTypeExpr>,
    decl_span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<ConstraintTarget, GraphcalError> {
    use graphcal_compiler::tir::typed::ResolvedTypeExpr;

    let Some(resolved) = base_resolved else {
        return Err(GraphcalError::InternalError {
            message: format!("domain constraint target `{name}` has no resolved type"),
            src: src.clone(),
            span: decl_span.into(),
        });
    };
    match resolved {
        ResolvedTypeExpr::Quantity(_)
        | ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => Ok(ConstraintTarget::Quantity),
        ResolvedTypeExpr::Int => Ok(ConstraintTarget::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(ConstraintTarget::Datetime(*scale)),
        ResolvedTypeExpr::Bool => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Bool".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Complex { .. } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Complex".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Key { .. } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Key".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::IndexArg(index) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("index {}", index.format_for_diagnostic()),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Struct(struct_name, _)
        | ResolvedTypeExpr::GenericStruct {
            name: struct_name, ..
        } => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("struct `{}`", struct_name.as_str()),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::GenericTypeParam(param, _) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("generic Type parameter `{param}`"),
            src: src.clone(),
            span: decl_span.into(),
        }),
        ResolvedTypeExpr::Indexed { .. } => Err(GraphcalError::InternalError {
            message: format!("domain constraint target `{name}` was not reduced to its base type"),
            src: src.clone(),
            span: decl_span.into(),
        }),
    }
}

/// Format a bound expression for display (e.g., `"100 kg"`, `"0.01 N"`).
///
/// For simple expressions (numbers, unit literals, unary negation), the
/// original syntactic form is preserved. For complex expressions, the
/// pre-evaluated SI value is displayed as a fallback — no re-evaluation needed.
fn exact_domain_int_bound(
    value: i64,
    src: &NamedSource<Arc<String>>,
    span: graphcal_compiler::syntax::span::Span,
) -> Result<f64, GraphcalError> {
    const MAX_EXACT_F64_INT: u64 = 1_u64 << f64::MANTISSA_DIGITS;
    if value.unsigned_abs() > MAX_EXACT_F64_INT {
        return Err(GraphcalError::EvalError {
            message: format!(
                "domain bound integer {value} is too large for exact quantity comparison"
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer magnitude is checked to be exactly representable before casting"
    )]
    {
        Ok(value as f64)
    }
}

fn format_quantity_bound_display(expr: &graphcal_compiler::hir::Expr, si_value: f64) -> String {
    use graphcal_compiler::hir::ExprKind;
    match &expr.kind {
        ExprKind::Number(n) => graphcal_compiler::registry::format::format_number(*n),
        ExprKind::Integer(n) => format!("{n}"),
        ExprKind::UnitLiteral { value, unit } => {
            let unit_str = graphcal_compiler::registry::format::format_unit_terms_with_config(
                unit.terms
                    .iter()
                    .map(|item| (item.op, item.name.value.to_string(), item.power)),
                true,
            );
            let val_str = graphcal_compiler::registry::format::format_number(*value);
            format!("{val_str} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_compiler::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_quantity_bound_display(operand, -si_value))
        }
        // Fallback: display the already-evaluated SI value.
        _ => graphcal_compiler::registry::format::format_number(si_value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};
    use graphcal_compiler::syntax::module_resolve::ModuleResolver;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::{
        ModuleTypeRegistry, ResolvedDagDependencies, type_resolve_with_modules,
    };

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn compile_source(source: &str) -> Result<ExecPlan, GraphcalError> {
        let (tir, src) = tir_from_source(source);
        compile(&tir, &src)
    }

    fn tir_from_source(
        source: &str,
    ) -> (graphcal_compiler::tir::typed::TIR, NamedSource<Arc<String>>) {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = desugared;
        let src = make_src(source);
        let ir = lower(&file, &src).unwrap();
        let dag_id = graphcal_compiler::dag_id::DagId::from_virtual_relative_path(
            std::path::Path::new("test.gcl"),
        )
        .unwrap();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(dag_id.clone(), &file.declarations)
            .unwrap();
        let mut module_types = ModuleTypeRegistry::default();
        module_types.insert_graphcal_prelude().unwrap();
        module_types.insert_registry(&dag_id, &ir.registry, src.clone());
        let tir = type_resolve_with_modules(ir, dag_id, &src, &resolver, &module_types).unwrap();
        (tir, src)
    }

    fn quantity(rv: &RuntimeValue) -> f64 {
        match rv {
            RuntimeValue::Quantity(v) => *v,
            other => panic!("expected quantity, got {other:?}"),
        }
    }

    fn test_dag_id() -> graphcal_compiler::dag_id::DagId {
        graphcal_compiler::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(
            "test.gcl",
        ))
        .unwrap()
    }

    fn resolved_key(name: &str) -> RuntimeDeclKey {
        RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
            test_dag_id(),
            DeclName::expect_valid(name),
        ))
    }

    #[test]
    fn compile_simple_const() {
        let plan = compile_source("const node g0: Dimensionless = 9.80665;").unwrap();
        assert!((quantity(&plan.const_values[&resolved_key("g0")]) - 9.80665).abs() < f64::EPSILON);
        assert!(plan.topo_order.is_empty());
    }

    #[test]
    fn compile_const_chain() {
        let plan = compile_source(
            "const node g0: Dimensionless = 9.80665;\nconst node two_g0: Dimensionless = 2.0 * @g0;",
        )
        .unwrap();
        assert!((quantity(&plan.const_values[&resolved_key("two_g0")]) - 19.6133).abs() < 1e-10);
    }

    #[test]
    fn compile_runtime_dag() {
        let plan = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let x_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "x")
            .unwrap();
        let y_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "y")
            .unwrap();
        let z_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "z")
            .unwrap();
        assert!(x_pos < y_pos);
        assert!(y_pos < z_pos);
    }

    #[test]
    fn compile_const_cycle() {
        let err = compile_source(
            "const node a: Dimensionless = @b + 1.0;\nconst node b: Dimensionless = @a + 1.0;",
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    #[test]
    fn compile_runtime_cycle() {
        let err =
            compile_source("node a: Dimensionless = @b + 1.0;\nnode b: Dimensionless = @a + 1.0;")
                .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    #[test]
    fn compile_uses_semantic_const_deps() {
        use std::collections::BTreeSet;

        let (mut tir, src) = tir_from_source(
            "const node a: Dimensionless = 1.0;\n\
             const node b: Dimensionless = @a + 1.0;",
        );
        let dag_id = tir.root_dag_id.clone();
        let a = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("a"));
        let b = ResolvedDeclName::from_def(dag_id, DeclName::expect_valid("b"));
        let mut resolved = ResolvedDagDependencies::default();
        resolved.const_deps.insert(a.clone(), BTreeSet::new());
        resolved.const_deps.insert(b, BTreeSet::from([a]));
        tir.root_mut().semantic.dependencies = resolved;

        let plan = compile(&tir, &src).unwrap();
        assert!(
            (quantity(
                &plan.const_values[&RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id.clone(),
                    DeclName::expect_valid("b")
                ))]
            ) - 2.0)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn compile_uses_semantic_runtime_deps() {
        use std::collections::BTreeSet;

        let (mut tir, src) = tir_from_source(
            "node a: Dimensionless = 1.0;\n\
             node b: Dimensionless = @a + 1.0;",
        );
        let dag_id = tir.root_dag_id.clone();
        let a = ResolvedDeclName::from_def(dag_id.clone(), DeclName::expect_valid("a"));
        let b = ResolvedDeclName::from_def(dag_id, DeclName::expect_valid("b"));
        let mut resolved = ResolvedDagDependencies::default();
        resolved.runtime_deps.insert(a.clone(), BTreeSet::new());
        resolved.runtime_deps.insert(b, BTreeSet::from([a]));
        tir.root_mut().semantic.dependencies = resolved;

        let plan = compile(&tir, &src).unwrap();
        let a_pos = plan
            .topo_order
            .iter()
            .position(|name| {
                name == &RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id.clone(),
                    DeclName::expect_valid("a"),
                ))
            })
            .unwrap();
        let b_pos = plan
            .topo_order
            .iter()
            .position(|name| {
                name == &RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id.clone(),
                    DeclName::expect_valid("b"),
                ))
            })
            .unwrap();
        assert!(a_pos < b_pos);
    }

    // -----------------------------------------------------------------------
    // Domain constraints on const nodes (#441)
    // -----------------------------------------------------------------------

    #[test]
    fn const_domain_value_within_bounds_passes() {
        compile_source("const node MAX_M: Mass(min: 1.0 kg, max: 100.0 kg) = 50.0 kg;").unwrap();
    }

    #[test]
    fn const_domain_value_below_min_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_value_above_max_rejected() {
        let err = compile_source("const node X: Mass(max: 10.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_min_exceeds_max_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg, max: 50.0 kg) = 75.0 kg;")
            .unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainMinExceedsMax { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_invalid_target_rejected() {
        // `Bool` is not a valid constraint target; this should now fire on consts too.
        let err = compile_source("const node FLAG: Bool(min: 0.0) = true;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::InvalidDomainTarget { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_int_value_within_bounds() {
        compile_source("const node N: Int(min: 1, max: 100) = 5;").unwrap();
    }

    #[test]
    fn const_domain_int_value_out_of_bounds_rejected() {
        let err = compile_source("const node N: Int(min: 1, max: 10) = 100;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_int_preserves_full_range_bounds() {
        compile_source(
            "const node MIN_I: Int(\
             min: -9223372036854775807 - 1, \
             max: 9223372036854775807) = -9223372036854775807 - 1;",
        )
        .unwrap();
    }

    #[test]
    fn const_datetime_domain_bounds_are_inclusive() {
        compile_source(
            r#"
const node START: Datetime<TT> = epoch<TT>("2024-01-01T00:00:00");
const node EVENT: Datetime<TT>(
    min: @START,
    max: epoch<TT>("2024-12-31T23:59:59"),
) = @START;
"#,
        )
        .unwrap();
    }

    #[test]
    fn const_datetime_domain_violation_is_rejected() {
        let error = compile_source(
            r#"
const node EVENT: Datetime(
    min: datetime("2024-01-01T00:00:00Z"),
    max: datetime("2024-12-31T23:59:59Z"),
) = datetime("2025-01-01T00:00:00Z");
"#,
        )
        .unwrap_err();
        assert!(matches!(error, GraphcalError::DomainViolation { .. }));
    }

    #[test]
    fn datetime_domain_min_exceeds_max_is_rejected() {
        let error = compile_source(
            r#"
const node EVENT: Datetime<TT>(
    min: epoch<TT>("2025-01-01T00:00:00"),
    max: epoch<TT>("2024-01-01T00:00:00"),
) = epoch<TT>("2024-06-01T00:00:00");
"#,
        )
        .unwrap_err();
        assert!(matches!(error, GraphcalError::DomainMinExceedsMax { .. }));
    }
}

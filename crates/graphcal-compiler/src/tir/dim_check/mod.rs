use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::Expr;
use crate::syntax::dimension::Dimension;
use crate::syntax::names::{IndexName, ScopedName, StructTypeName};

use crate::registry::builtins::builtin_functions;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::Registry;
use crate::tir::typed::NatLinearForm;

pub(crate) use helpers::format_inferred_type;
use helpers::{expect_scalar, format_declared_type, is_bool_type, types_match};
use infer::{infer_type, infer_type_with_owner};

mod builtins;
mod helpers;
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref,
    clippy::doc_markdown,
    reason = "inference functions pass compilation context through many parameters; \
              large match on ExprKind variants is inherently long"
)]
mod infer;
#[cfg(test)]
mod tests;

pub use crate::registry::declared_type::DeclaredType;

/// The inferred type of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredType {
    Scalar(Dimension),
    Bool,
    Int,
    /// A bounded natural number `Fin(N)`: the type of loop variables over `range(N)`.
    ///
    /// A value of type `Fin(N)` satisfies `0 <= value < N`. This enables compile-time
    /// bounds checking: `v[i]` is valid when `i : Fin(N)` and `v : T[M]` with `N <= M`.
    ///
    /// `Fin(N)` is not a user-declarable type — it only arises as the type of loop
    /// variables in `for i: range(N) { ... }`.
    Fin(NatLinearForm),
    /// A datetime instant in a specific time scale.
    Datetime(TimeScale),
    /// A label of a named index (e.g., `Maneuver.Departure` has type `Label(Maneuver)`).
    Label(IndexName),
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(StructTypeName, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: IndexName,
    },
}

impl InferredType {
    /// Returns `true` if this type is `Int` or `Fin(N)` (integer-like).
    #[must_use]
    pub const fn is_int_like(&self) -> bool {
        matches!(self, Self::Int | Self::Fin(_))
    }
}

/// Check that a declaration's expression type matches its declared type annotation.
#[expect(clippy::too_many_arguments, reason = "passes compilation context")]
fn check_decl_expr_type(
    expr: &Expr,
    name: &crate::syntax::names::ScopedName,
    type_ann_span: &crate::syntax::span::Span,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    empty_locals: &HashMap<String, InferredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let declared = declared_types
        .get(name)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("no declared type recorded for `{name}`"),
            src: src.clone(),
            span: (*type_ann_span).into(),
        })?;
    let inferred = infer_type_with_owner(
        expr,
        Some(name.member()),
        declared_types,
        empty_locals,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if !types_match(declared, &inferred, registry) {
        return Err(GraphcalError::DimensionMismatchInAnnotation {
            declared: format_declared_type(declared, registry),
            inferred: format_inferred_type(&inferred, registry),
            src: src.clone(),
            span: (*type_ann_span).into(),
        });
    }
    Ok(())
}

/// Check dimension consistency of an assert body.
///
/// For expression asserts, verifies the body is boolean. For tolerance asserts,
/// verifies actual/expected have matching dimensions and tolerance is compatible.
#[expect(clippy::too_many_arguments, reason = "passes compilation context")]
fn check_assert_body(
    body: &crate::desugar::desugared_ast::AssertBody,
    span: crate::syntax::span::Span,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    empty_locals: &HashMap<String, InferredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match body {
        crate::desugar::desugared_ast::AssertBody::Expr(body_expr) => {
            let inferred = infer_type(
                body_expr,
                declared_types,
                empty_locals,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            if !is_bool_type(&inferred) {
                return Err(GraphcalError::AssertBodyNotBool {
                    found: format_inferred_type(&inferred, registry),
                    src: src.clone(),
                    span: span.into(),
                });
            }
        }
        crate::desugar::desugared_ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative,
        } => {
            let actual_type = infer_type(
                actual,
                declared_types,
                empty_locals,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            let expected_type = infer_type(
                expected,
                declared_types,
                empty_locals,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            let tolerance_type = infer_type(
                tolerance,
                declared_types,
                empty_locals,
                tir,
                registry,
                builtin_fns,
                src,
            )?;

            // actual and expected must have the same dimension
            let actual_dim = expect_scalar(&actual_type, registry, src, actual.span)?;
            let expected_dim = expect_scalar(&expected_type, registry, src, expected.span)?;
            if actual_dim != expected_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&actual_dim),
                    found: registry.dimensions.format_dimension(&expected_dim),
                    help: "actual and expected in tolerance assertion must have the same dimension"
                        .to_string(),
                    src: src.clone(),
                    span: expected.span.into(),
                });
            }

            // tolerance: same dimension (absolute) or dimensionless/Int (relative %)
            let tolerance_ok = if *is_relative {
                tolerance_type.is_int_like()
                    || matches!(&tolerance_type, InferredType::Scalar(d) if d.is_dimensionless())
            } else {
                let tolerance_dim = expect_scalar(&tolerance_type, registry, src, tolerance.span)?;
                tolerance_dim == actual_dim
            };
            if !tolerance_ok {
                let (expected_str, help_str) = if *is_relative {
                    (
                        "Dimensionless".to_string(),
                        "relative tolerance (%) must be dimensionless".to_string(),
                    )
                } else {
                    (
                        registry.dimensions.format_dimension(&actual_dim),
                        "absolute tolerance must have the same dimension as actual/expected"
                            .to_string(),
                    )
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: expected_str,
                    found: format_inferred_type(&tolerance_type, registry),
                    help: help_str,
                    src: src.clone(),
                    span: tolerance.span.into(),
                });
            }
        }
    }
    Ok(())
}

/// Check dimensions for all declarations in a file.
///
/// For each const/param/node, infers the dimension of the RHS expression
/// and verifies it matches the declared type annotation. Uses
/// `tir.build_declared_types()` (derived from `resolved_decl_types`) to validate
/// that every RHS expression matches its declared type annotation.
///
/// This is a pure validation step — returns `()` on success.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if dimensions are inconsistent.
pub fn check_dimensions_tir(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    detect_decl_cycles(tir, src)?;
    detect_cross_dag_cycles(tir, src)?;
    let builtin_fns = builtin_functions();

    // Dim-check the file's own DAGs (root + inline children) against the
    // file's shared registry. Dep DAGs merged in by `merge_dep_dag_tirs`
    // were already dim-checked in their own file's pipeline, against
    // their own registry — re-checking them here against the importer's
    // registry would fail on types renamed by include bindings.
    for (id, dag) in &tir.dags {
        if id == &tir.root_dag_id || id.parent().as_ref() == Some(&tir.root_dag_id) {
            check_dimensions_dag(dag, tir, &tir.registry, builtin_fns, src)?;
        }
    }

    Ok(())
}

/// Dim-check a single [`DagTIR`] against the file's shared registry and
/// the full flat dag map.
fn check_dimensions_dag(
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &crate::registry::types::Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let declared_types = dag.build_declared_types(src)?;
    let empty_locals: HashMap<String, InferredType> = HashMap::new();

    for entry in &dag.consts {
        check_decl_expr_type(
            &entry.expr,
            &entry.name,
            &entry.type_ann.span,
            &declared_types,
            &empty_locals,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
    }
    for entry in &dag.nodes {
        check_decl_expr_type(
            &entry.expr,
            &entry.name,
            &entry.type_ann.span,
            &declared_types,
            &empty_locals,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
    }
    for entry in &dag.params {
        let Some(ref value_expr) = entry.default_expr else {
            continue;
        };
        check_decl_expr_type(
            value_expr,
            &entry.name,
            &entry.type_ann.span,
            &declared_types,
            &empty_locals,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
    }

    for entry in &dag.asserts {
        check_assert_body(
            &entry.body,
            entry.span,
            &declared_types,
            &empty_locals,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
    }

    check_domain_constraint_targets_dag(dag, src)?;
    check_domain_constraint_dimensions_dag(
        dag,
        &declared_types,
        &empty_locals,
        tir,
        registry,
        builtin_fns,
        src,
    )?;

    Ok(())
}

/// What a domain bound expression must infer to for a given target type.
enum ExpectedBound {
    /// Bound must be `Scalar(d)`. `Int` is also accepted when `d` is dimensionless.
    Scalar(Dimension),
    /// Bound must be unitless: `Int`, or `Scalar` with the dimensionless dimension.
    Int,
}

/// Check that domain constraint bound expressions have the correct type.
///
/// For each param/node with `(min: ..., max: ...)` constraints whose target type
/// is `Scalar(d)`, `Dimensionless`, or `Int`, infers the type of each bound
/// expression using the regular type checker and verifies it matches:
/// - `Scalar(d)` target: bound must be `Scalar(d)` (or `Int` if `d` is dimensionless).
/// - `Dimensionless` target: bound must be `Scalar(dimensionless)` or `Int`.
/// - `Int` target: bound must be `Int` or `Scalar(dimensionless)` — units forbidden.
///
/// Other targets (e.g., `Bool`) are skipped here and handled by
/// `validate_constraint_target` in `exec_plan` (which raises `InvalidDomainTarget`).
fn check_domain_constraint_dimensions_dag(
    dag: &crate::tir::typed::DagTIR,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    empty_locals: &HashMap<String, InferredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let decl_iter = dag
        .consts
        .iter()
        .map(|e| (&e.name, &e.type_ann))
        .chain(dag.params.iter().map(|e| (&e.name, &e.type_ann)))
        .chain(dag.nodes.iter().map(|e| (&e.name, &e.type_ann)));

    for (name, type_ann) in decl_iter {
        let bounds = extract_domain_bounds(type_ann);
        if bounds.is_empty() {
            continue;
        }

        let resolved = dag.resolved_decl_types.get(name);
        let base_resolved = resolved.map(strip_indexed);
        let expected = match base_resolved {
            Some(crate::tir::typed::ResolvedTypeExpr::Scalar(dim)) => {
                ExpectedBound::Scalar(dim.clone())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Dimensionless) => {
                ExpectedBound::Scalar(Dimension::dimensionless())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Int) => ExpectedBound::Int,
            _ => continue,
        };

        for bound in bounds {
            let inferred = infer_type(
                &bound.value,
                declared_types,
                empty_locals,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            check_one_bound(name, bound, &inferred, &expected, registry, src)?;
        }
    }

    Ok(())
}

fn check_one_bound(
    name: &crate::syntax::names::ScopedName,
    bound: &crate::desugar::desugared_ast::DomainBound,
    inferred: &InferredType,
    expected: &ExpectedBound,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match expected {
        ExpectedBound::Scalar(target_dim) => {
            let ok = match inferred {
                InferredType::Scalar(d) => d == target_dim,
                InferredType::Int => target_dim.is_dimensionless(),
                _ => false,
            };
            if ok {
                return Ok(());
            }
            let bound_dim_str = match inferred {
                InferredType::Scalar(d) => registry.dimensions.format_dimension(d),
                other => format_inferred_type(other, registry),
            };
            Err(GraphcalError::DomainDimensionMismatch {
                name: name.to_string(),
                type_dim: registry.dimensions.format_dimension(target_dim),
                bound_name: bound.kind.to_string(),
                bound_dim: bound_dim_str,
                src: src.clone(),
                span: bound.span.into(),
            })
        }
        ExpectedBound::Int => {
            let ok = match inferred {
                InferredType::Int => true,
                InferredType::Scalar(d) => d.is_dimensionless(),
                _ => false,
            };
            if ok {
                return Ok(());
            }
            Err(GraphcalError::IntDomainBoundNotUnitless {
                name: name.to_string(),
                bound_name: bound.kind.to_string(),
                bound_type: format_inferred_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
    }
}

/// Reject domain constraints on base types that don't accept them.
///
/// Bool, Datetime, Label, and struct/generic types cannot carry numeric
/// `(min: …, max: …)` bounds. The check is a pure function of the resolved
/// declaration type — independent of any bound expression's value — so it
/// belongs in compile-time validation rather than runtime resolution.
fn check_domain_constraint_targets_dag(
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let decl_iter = dag
        .consts
        .iter()
        .map(|e| (&e.name, &e.type_ann, e.span))
        .chain(dag.params.iter().map(|e| (&e.name, &e.type_ann, e.span)))
        .chain(dag.nodes.iter().map(|e| (&e.name, &e.type_ann, e.span)));

    for (name, type_ann, decl_span) in decl_iter {
        if extract_domain_bounds(type_ann).is_empty() {
            continue;
        }
        let Some(resolved) = dag.resolved_decl_types.get(name) else {
            continue;
        };
        let type_kind = match strip_indexed(resolved) {
            crate::tir::typed::ResolvedTypeExpr::Bool => "Bool".to_string(),
            crate::tir::typed::ResolvedTypeExpr::Datetime(_) => "Datetime".to_string(),
            crate::tir::typed::ResolvedTypeExpr::Label(idx, _) => format!("Label({idx})"),
            crate::tir::typed::ResolvedTypeExpr::Struct(struct_name, _)
            | crate::tir::typed::ResolvedTypeExpr::GenericStruct {
                name: struct_name, ..
            } => format!("struct `{struct_name}`"),
            crate::tir::typed::ResolvedTypeExpr::Scalar(_)
            | crate::tir::typed::ResolvedTypeExpr::Dimensionless
            | crate::tir::typed::ResolvedTypeExpr::Int
            | crate::tir::typed::ResolvedTypeExpr::GenericDimParam(_, _)
            | crate::tir::typed::ResolvedTypeExpr::GenericDimExpr { .. }
            | crate::tir::typed::ResolvedTypeExpr::Indexed { .. } => continue,
        };
        return Err(GraphcalError::InvalidDomainTarget {
            type_kind,
            src: src.clone(),
            span: decl_span.into(),
        });
    }
    Ok(())
}

/// Extract `DomainBound`s from a `TypeExpr`, handling indexed types.
///
/// For `Velocity(min: 0)[Maneuver]`, the constraints are on the base `Velocity`,
/// not on the outer `Indexed` wrapper.
fn extract_domain_bounds(
    type_ann: &crate::desugar::desugared_ast::TypeExpr,
) -> &[crate::desugar::desugared_ast::DomainBound] {
    if !type_ann.constraints.is_empty() {
        return &type_ann.constraints;
    }
    if let crate::desugar::desugared_ast::TypeExprKind::Indexed { base, .. } = &type_ann.kind {
        return &base.constraints;
    }
    &[]
}

/// Strip `Indexed` wrappers to get the base resolved type.
fn strip_indexed(
    resolved: &crate::tir::typed::ResolvedTypeExpr,
) -> &crate::tir::typed::ResolvedTypeExpr {
    match resolved {
        crate::tir::typed::ResolvedTypeExpr::Indexed { base, .. } => strip_indexed(base),
        other => other,
    }
}

/// Check that an override expression has the correct dimension for the given param.
///
/// # Errors
///
/// Returns a [`GraphcalError::DimensionMismatch`] if the expression's inferred
/// dimension does not match the declared type of the param.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn check_override_dimension(
    expr: &Expr,
    param_name: &str,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, InferredType> = HashMap::new();

    // Override targets are addressed by their bare param name, which is always
    // a top-level local in the file being overridden.
    let param_key = ScopedName::local(param_name);
    let declared =
        declared_types
            .get(&param_key)
            .ok_or_else(|| GraphcalError::OverrideUnknownParam {
                name: crate::syntax::names::DeclName::new(param_name.to_string()),
            })?;
    let inferred = infer_type(
        expr,
        declared_types,
        &empty_locals,
        tir,
        registry,
        builtin_fns,
        src,
    )?;

    if !types_match(declared, &inferred, registry) {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_declared_type(declared, registry),
            found: format_inferred_type(&inferred, registry),
            help: format!(
                "override for `{param_name}` must have dimension {}",
                format_declared_type(declared, registry)
            ),
            src: src.clone(),
            span: expr.span.into(),
        });
    }
    Ok(())
}

/// Detect cycles in the cross-dag inline-call graph.
///
/// A dag `A` that transitively inline-calls itself — directly or through a
/// chain `A → B → … → A` — would recurse unboundedly at evaluation time. We
/// reject such programs at compile time with
/// [`GraphcalError::CyclicDependency`] pointing at one dag involved in the
/// cycle (chosen deterministically by the DFS entry order).
///
/// Per the issue thread, a dag — not a file — is the semantic unit of
/// cycle detection, so the same check applies whether the cycle is within
/// a single file or spans multiple files.
enum DagCycleFrame {
    Enter(crate::syntax::dag_id::DagId),
    Leave(crate::syntax::dag_id::DagId),
}

struct DagTargetCollector<'a, 'b> {
    /// Caller TIR — used to translate user-typed call paths to canonical
    /// [`DagId`](crate::syntax::dag_id::DagId) keys via
    /// [`crate::tir::typed::TIR::resolve_call_path`].
    tir: &'a crate::tir::typed::TIR,
    out: &'b mut std::collections::BTreeSet<crate::syntax::dag_id::DagId>,
}

impl crate::syntax::visitor::ExprVisitor<crate::syntax::phase::Desugared>
    for DagTargetCollector<'_, '_>
{
    type Error = std::convert::Infallible;

    fn visit_inline_dag_ref(
        &mut self,
        expr: &crate::desugar::desugared_ast::Expr,
        args: &[crate::desugar::desugared_ast::ParamBinding],
    ) -> Result<(), Self::Error> {
        if let crate::desugar::desugared_ast::ExprKind::InlineDagRef { path, .. } = &expr.kind
            && let Some(id) = self.tir.resolve_call_path(path)
        {
            self.out.insert(id);
        }
        for b in args {
            self.visit_expr(&b.value)?;
        }
        Ok(())
    }
}

/// Collect inline dag call targets from a compiled DAG's body expressions.
///
/// Walks every const/param/node RHS expression and records the canonical
/// [`DagId`](crate::syntax::dag_id::DagId) for each `@dag(args).out` /
/// `@mod.dag(args).out` reference. The translation from user-typed
/// `ModulePath` to canonical id goes through
/// [`crate::tir::typed::TIR::resolve_call_path`] so cross-file qualified
/// calls use the importer's `module_aliases` map.
fn collect_dag_call_targets_from_dag(
    tir: &crate::tir::typed::TIR,
    dag: &crate::tir::typed::DagTIR,
    out: &mut std::collections::BTreeSet<crate::syntax::dag_id::DagId>,
) {
    use crate::syntax::visitor::ExprVisitor;

    let mut collector = DagTargetCollector { tir, out };
    for entry in &dag.consts {
        let _ = collector.visit_expr(&entry.expr);
    }
    for entry in &dag.nodes {
        let _ = collector.visit_expr(&entry.expr);
    }
    for entry in &dag.params {
        if let Some(expr) = &entry.default_expr {
            let _ = collector.visit_expr(expr);
        }
    }
}

/// Detect cycles in same-file declaration dependencies.
///
/// A graph cycle is a topological property of source — knowable without
/// evaluating any value. This check rejects cyclic params/nodes (`runtime_deps`)
/// and cyclic consts (`const_deps`) at compile time so the diagnostic appears
/// under `graphcal check`, not only at evaluation. Mirrors the toposort-based
/// cycle detection in `graphcal-eval`'s `exec_plan::eval_consts_from_tir` and
/// `build_runtime_dag`, which now act as defense-in-depth backstops.
fn detect_decl_cycles(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use std::collections::BTreeSet;

    use petgraph::algo::toposort;
    use petgraph::graph::DiGraph;

    use crate::syntax::names::ScopedName;

    fn check<'a>(
        names_with_spans: impl Iterator<Item = (&'a ScopedName, crate::syntax::span::Span)>,
        deps: &HashMap<ScopedName, BTreeSet<ScopedName>>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        let mut graph = DiGraph::<String, ()>::new();
        let mut index_map: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut spans: HashMap<String, crate::syntax::span::Span> = HashMap::new();
        for (name, span) in names_with_spans {
            let key = name.to_string();
            let idx = graph.add_node(key.clone());
            index_map.insert(key.clone(), idx);
            spans.insert(key, span);
        }
        if index_map.is_empty() {
            return Ok(());
        }
        for (name, dep_set) in deps {
            let Some(&to) = index_map.get(name.to_string().as_str()) else {
                continue;
            };
            for dep in dep_set {
                if let Some(&from) = index_map.get(dep.to_string().as_str()) {
                    graph.add_edge(from, to, ());
                }
            }
        }
        toposort(&graph, None).map(|_| ()).map_err(|cycle| {
            let cycle_node = &graph[cycle.node_id()];
            let span = spans
                .get(cycle_node)
                .copied()
                .unwrap_or_else(|| crate::syntax::span::Span::new(0, 0));
            GraphcalError::CyclicDependency {
                name: crate::syntax::names::DeclName::new(cycle_node.clone()),
                src: src.clone(),
                span: span.into(),
            }
        })
    }

    for dag in tir.dags.values() {
        check(
            dag.consts.iter().map(|e| (&e.name, e.span)),
            &dag.const_deps,
            src,
        )?;
        check(
            dag.params
                .iter()
                .map(|e| (&e.name, e.span))
                .chain(dag.nodes.iter().map(|e| (&e.name, e.span))),
            &dag.runtime_deps,
            src,
        )?;
    }
    Ok(())
}

fn detect_cross_dag_cycles(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use std::collections::{BTreeMap, BTreeSet, HashSet};

    use crate::syntax::dag_id::DagId;

    let mut edges: BTreeMap<DagId, BTreeSet<DagId>> = BTreeMap::new();
    let mut spans: HashMap<DagId, crate::syntax::span::Span> = HashMap::new();
    for (key, dag_tir) in &tir.dags {
        let mut targets = BTreeSet::new();
        collect_dag_call_targets_from_dag(tir, dag_tir, &mut targets);
        edges.insert(key.clone(), targets);
        // Best-effort span: for inline children of this file the parent's
        // registry entry has the AST span; cross-file merged dags fall
        // back to a zero span (no AST in the importer).
        let parent = key.parent();
        let span = if parent.as_ref() == Some(&tir.root_dag_id) {
            tir.registry
                .dags
                .get(key.name())
                .map_or_else(|| crate::syntax::span::Span::new(0, 0), |d| d.name.span)
        } else {
            crate::syntax::span::Span::new(0, 0)
        };
        spans.insert(key.clone(), span);
    }

    let mut visited: HashSet<DagId> = HashSet::new();
    let mut on_stack: HashSet<DagId> = HashSet::new();

    for start in edges.keys() {
        if visited.contains(start) {
            continue;
        }
        let mut work: Vec<DagCycleFrame> = vec![DagCycleFrame::Enter(start.clone())];
        while let Some(frame) = work.pop() {
            match frame {
                DagCycleFrame::Enter(key) => {
                    if visited.contains(&key) {
                        continue;
                    }
                    if on_stack.contains(&key) {
                        let span = spans
                            .get(&key)
                            .copied()
                            .unwrap_or_else(|| crate::syntax::span::Span::new(0, 0));
                        return Err(GraphcalError::CyclicDependency {
                            name: crate::syntax::names::DeclName::new(key.to_string()),
                            src: src.clone(),
                            span: span.into(),
                        });
                    }
                    on_stack.insert(key.clone());
                    work.push(DagCycleFrame::Leave(key.clone()));
                    if let Some(targets) = edges.get(&key) {
                        for t in targets {
                            if edges.contains_key(t) {
                                work.push(DagCycleFrame::Enter(t.clone()));
                            }
                        }
                    }
                }
                DagCycleFrame::Leave(key) => {
                    on_stack.remove(&key);
                    visited.insert(key);
                }
            }
        }
    }

    Ok(())
}

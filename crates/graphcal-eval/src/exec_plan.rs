//! Execution plan — the result of compiling a TIR.
//!
//! Contains evaluated const values, topologically sorted runtime declarations,
//! and their expressions, ready for evaluation.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{AssertBody, Expr};
use graphcal_syntax::span::Span;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::error::GraphcalError;
use crate::eval_expr::{RuntimeValue, eval_expr};
use crate::tir::{ResolvedDomainConstraint, TIR};

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    /// Evaluated const values (in base SI units).
    pub const_values: HashMap<String, RuntimeValue>,
    /// Pre-evaluated values imported from dependency files.
    /// These are injected directly into the evaluation environment.
    pub imported_values: HashMap<crate::resolve::ScopedName, RuntimeValue>,
    /// Topologically sorted names for runtime evaluation (params + nodes).
    pub topo_order: Vec<String>,
    /// Runtime expressions keyed by declaration name (params + nodes).
    pub expressions: HashMap<String, Expr>,
    /// Assert bodies in source order: (name, body, span).
    pub assert_bodies: Vec<(String, AssertBody, Span)>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<String, Vec<String>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub expected_fail: HashMap<String, crate::resolve::ExpectedFail>,
    /// Resolved domain constraints for runtime validation, keyed by declaration name.
    pub domain_constraints: HashMap<String, ResolvedDomainConstraint>,
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
pub fn compile(tir: &TIR, src: &NamedSource<Arc<String>>) -> Result<ExecPlan, GraphcalError> {
    let const_values = eval_consts_from_tir(tir, src)?;
    let (topo_order, expressions) = build_runtime_dag(tir, src)?;

    let assert_bodies: Vec<(String, AssertBody, Span)> = tir
        .asserts
        .iter()
        .map(|(name, body, span)| (name.clone(), body.clone(), *span))
        .collect();

    // Resolve domain constraints from type annotations.
    let domain_constraints = resolve_domain_constraints(tir, &const_values, src)?;

    Ok(ExecPlan {
        const_values,
        imported_values: tir
            .imported_values
            .iter()
            .map(|(k, (v, _dt))| (k.clone(), v.clone()))
            .collect(),
        topo_order,
        expressions,
        assert_bodies,
        assumes_map: tir.assumes_map.clone(),
        expected_fail: tir.expected_fail.clone(),
        domain_constraints,
    })
}

/// Topologically sort and evaluate const declarations from a TIR.
fn eval_consts_from_tir(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<String, RuntimeValue>, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    if tir.consts.is_empty() {
        return Ok(HashMap::new());
    }

    let mut graph = DiGraph::<String, ()>::new();
    let mut index_map: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

    for (name, _, _, _) in &tir.consts {
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
    }

    for (name, _, _, _) in &tir.consts {
        if let Some(deps) = tir.const_deps.get(name) {
            let from = index_map[name];
            for dep in deps {
                let to = index_map[dep];
                graph.add_edge(to, from, ());
            }
        }
    }

    let sorted = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = tir
            .consts
            .iter()
            .find(|(n, _, _, _)| n == cycle_node)
            .map_or_else(|| Span::new(0, 0), |(_, _, _, s)| *s);
        GraphcalError::CyclicDependency {
            name: cycle_node.clone().into(),
            src: src.clone(),
            span: span.into(),
        }
    })?;

    let const_exprs: HashMap<&str, &Expr> = tir
        .consts
        .iter()
        .map(|(name, _, expr, _)| (name.as_str(), expr))
        .collect();

    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();
    let mut values: HashMap<String, RuntimeValue> = HashMap::new();

    for idx in sorted {
        let name = &graph[idx];
        let expr = const_exprs[name.as_str()];
        let val = eval_expr(
            expr,
            &values,
            &empty_locals,
            &builtin_consts,
            &builtin_fns,
            &tir.registry,
            src,
        )?;
        values.insert(name.clone(), val);
    }

    Ok(values)
}

/// Build a topologically sorted runtime DAG from params and nodes in a TIR.
fn build_runtime_dag(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(Vec<String>, HashMap<String, Expr>), GraphcalError> {
    let mut graph = DiGraph::<String, ()>::new();
    let mut index_map: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
    let mut expressions: HashMap<String, Expr> = HashMap::new();

    for (name, _, expr_opt, span) in &tir.params {
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
        match expr_opt {
            Some(expr) => {
                expressions.insert(name.clone(), expr.clone());
            }
            None => {
                return Err(GraphcalError::RequiredParamNotProvided {
                    name: name.clone(),
                    src: src.clone(),
                    span: (*span).into(),
                });
            }
        }
    }
    for (name, _, expr, _) in &tir.nodes {
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
        expressions.insert(name.clone(), expr.clone());
    }

    for (name, deps) in &tir.runtime_deps {
        if let Some(&to_idx) = index_map.get(name) {
            for dep in deps {
                if let Some(&from_idx) = index_map.get(dep) {
                    graph.add_edge(from_idx, to_idx, ());
                }
            }
        }
    }

    let topo_indices = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = tir
            .nodes
            .iter()
            .map(|(n, _, _, s)| (n, *s))
            .chain(tir.params.iter().map(|(n, _, _, s)| (n, *s)))
            .find(|(n, _)| *n == cycle_node)
            .map_or_else(|| Span::new(0, 0), |(_, s)| s);
        GraphcalError::CyclicDependency {
            name: cycle_node.clone().into(),
            src: src.clone(),
            span: span.into(),
        }
    })?;

    let topo_order: Vec<String> = topo_indices
        .into_iter()
        .map(|idx| graph[idx].clone())
        .collect();

    Ok((topo_order, expressions))
}

/// Resolve domain constraints from type annotations on params and nodes.
///
/// Evaluates each constraint bound expression using const values and builtins,
/// validates the dimension matches the declared type, and checks min <= max.
#[expect(
    clippy::too_many_lines,
    reason = "linear iteration over domain bounds with validation"
)]
fn resolve_domain_constraints(
    tir: &TIR,
    const_values: &HashMap<String, RuntimeValue>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<String, ResolvedDomainConstraint>, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let mut constraints = HashMap::new();

    // Iterate over params and nodes that have non-empty constraints in their type annotations.
    let decl_iter = tir
        .params
        .iter()
        .map(|(name, type_ann, _, span)| (name, type_ann, *span))
        .chain(
            tir.nodes
                .iter()
                .map(|(name, type_ann, _, span)| (name, type_ann, *span)),
        );

    for (name, type_ann, decl_span) in decl_iter {
        // Get constraints from the type annotation (could be on base type if indexed).
        let domain_bounds = extract_domain_bounds(type_ann);
        if domain_bounds.is_empty() {
            continue;
        }

        // Validate that the base type supports constraints.
        let resolved = tir.resolved_decl_types.get(name);
        let base_resolved = resolved.map(strip_indexed);
        validate_constraint_target(name, base_resolved, decl_span, src)?;

        // Get the expected dimension for bound validation.
        let expected_dim = base_resolved.and_then(|r| match r {
            crate::tir::ResolvedTypeExpr::Scalar(dim) => Some(dim.clone()),
            crate::tir::ResolvedTypeExpr::Dimensionless => {
                Some(graphcal_syntax::dimension::Dimension::dimensionless())
            }
            _ => None,
        });

        let mut min_val: Option<f64> = None;
        let mut max_val: Option<f64> = None;
        let mut min_display: Option<String> = None;
        let mut max_display: Option<String> = None;
        let mut constraint_span = domain_bounds[0].span;

        for bound in domain_bounds {
            // Evaluate the bound expression.
            let rv = eval_expr(
                &bound.value,
                const_values,
                &empty_locals,
                &builtin_consts,
                &builtin_fns,
                &tir.registry,
                src,
            )?;

            let si_value = match &rv {
                RuntimeValue::Scalar(v) => *v,
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "domain bound integers are small"
                )]
                RuntimeValue::Int(i) => *i as f64,
                _ => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "domain constraint `{}` must evaluate to a scalar value",
                            bound.kind,
                        ),
                        src: src.clone(),
                        span: bound.value.span.into(),
                    });
                }
            };

            // Validate dimension of the bound against the type's dimension.
            if let (Some(expected), RuntimeValue::Scalar(_)) = (&expected_dim, &rv) {
                // Infer the dimension of the bound expression from its unit annotation.
                let bound_dim = infer_bound_dimension(&bound.value, &tir.registry);
                if let Some(bd) = &bound_dim
                    && bd != expected
                {
                    return Err(GraphcalError::DomainDimensionMismatch {
                        name: name.clone(),
                        type_dim: tir.registry.dimensions.format_dimension(expected),
                        bound_name: bound.kind.to_string(),
                        bound_dim: tir.registry.dimensions.format_dimension(bd),
                        src: src.clone(),
                        span: bound.span.into(),
                    });
                }
            }

            // Extract display text from the source.
            let display_text = format_bound_display(&bound.value, &tir.registry);

            constraint_span = constraint_span.merge(bound.span);

            match bound.kind {
                graphcal_syntax::ast::DomainBoundKind::Min => {
                    min_val = Some(si_value);
                    min_display = Some(display_text);
                }
                graphcal_syntax::ast::DomainBoundKind::Max => {
                    max_val = Some(si_value);
                    max_display = Some(display_text);
                }
            }
        }

        // Validate min <= max.
        if let (Some(min), Some(max)) = (min_val, max_val)
            && min > max
        {
            return Err(GraphcalError::DomainMinExceedsMax {
                name: name.clone(),
                min: min_display.unwrap_or_else(|| format!("{min}")),
                max: max_display.unwrap_or_else(|| format!("{max}")),
                src: src.clone(),
                span: constraint_span.into(),
            });
        }

        constraints.insert(
            name.clone(),
            ResolvedDomainConstraint {
                min: min_val,
                max: max_val,
                min_display,
                max_display,
                span: constraint_span,
            },
        );
    }

    Ok(constraints)
}

/// Extract `DomainBound`s from a `TypeExpr`, handling indexed types.
///
/// For `Velocity(min: 0)[Maneuver]`, the constraints are on the base `Velocity`,
/// not on the outer `Indexed` wrapper.
fn extract_domain_bounds(
    type_ann: &graphcal_syntax::ast::TypeExpr,
) -> &[graphcal_syntax::ast::DomainBound] {
    if !type_ann.constraints.is_empty() {
        return &type_ann.constraints;
    }
    // For indexed types, check the base type's constraints.
    if let graphcal_syntax::ast::TypeExprKind::Indexed { base, .. } = &type_ann.kind {
        return &base.constraints;
    }
    &[]
}

/// Strip `Indexed` wrapper to get the base resolved type.
fn strip_indexed(resolved: &crate::tir::ResolvedTypeExpr) -> &crate::tir::ResolvedTypeExpr {
    match resolved {
        crate::tir::ResolvedTypeExpr::Indexed { base, .. } => strip_indexed(base),
        other => other,
    }
}

/// Validate that the resolved type supports domain constraints.
fn validate_constraint_target(
    _name: &str,
    base_resolved: Option<&crate::tir::ResolvedTypeExpr>,
    decl_span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let Some(resolved) = base_resolved else {
        return Ok(()); // No resolved type — skip validation (will be caught elsewhere)
    };
    match resolved {
        crate::tir::ResolvedTypeExpr::Scalar(_)
        | crate::tir::ResolvedTypeExpr::Dimensionless
        | crate::tir::ResolvedTypeExpr::Int => Ok(()),
        crate::tir::ResolvedTypeExpr::Bool => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Bool".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        crate::tir::ResolvedTypeExpr::Datetime(_) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: "Datetime".to_string(),
            src: src.clone(),
            span: decl_span.into(),
        }),
        crate::tir::ResolvedTypeExpr::Label(idx, _) => Err(GraphcalError::InvalidDomainTarget {
            type_kind: format!("Label({idx})"),
            src: src.clone(),
            span: decl_span.into(),
        }),
        crate::tir::ResolvedTypeExpr::Struct(name_s, _)
        | crate::tir::ResolvedTypeExpr::GenericStruct { name: name_s, .. } => {
            Err(GraphcalError::InvalidDomainTarget {
                type_kind: format!("struct `{name_s}`"),
                src: src.clone(),
                span: decl_span.into(),
            })
        }
        crate::tir::ResolvedTypeExpr::GenericDimParam(_, _)
        | crate::tir::ResolvedTypeExpr::GenericDimExpr { .. } => {
            // Generic types in function signatures — constraints don't apply here
            Ok(())
        }
        crate::tir::ResolvedTypeExpr::Indexed { .. } => {
            // Already stripped, shouldn't reach here
            Ok(())
        }
    }
}

/// Infer the dimension of a bound expression from its unit annotation.
///
/// Only handles simple cases: unit literals and negated unit literals.
/// For bare numbers (no unit), returns `None` (dimensionless is assumed).
fn infer_bound_dimension(
    expr: &graphcal_syntax::ast::Expr,
    registry: &crate::registry::Registry,
) -> Option<graphcal_syntax::dimension::Dimension> {
    use graphcal_syntax::ast::ExprKind;
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => {
            registry.units.resolve_unit_expr(unit).map(|(dim, _)| dim)
        }
        ExprKind::UnaryOp { operand, .. } => infer_bound_dimension(operand, registry),
        ExprKind::BinOp { lhs, rhs, .. } => {
            // Try lhs first, fall back to rhs
            infer_bound_dimension(lhs, registry).or_else(|| infer_bound_dimension(rhs, registry))
        }
        _ => None, // Number, Integer, and other expressions are dimensionless
    }
}

/// Format a bound expression for display (e.g., `"100 kg"`, `"0.01 N"`).
fn format_bound_display(
    expr: &graphcal_syntax::ast::Expr,
    registry: &crate::registry::Registry,
) -> String {
    use graphcal_syntax::ast::ExprKind;
    match &expr.kind {
        ExprKind::Number(n) => crate::eval::format_number(*n),
        ExprKind::Integer(n) => format!("{n}"),
        ExprKind::UnitLiteral { value, unit } => {
            let unit_str = crate::eval::format_unit_expr(unit);
            let val_str = crate::eval::format_number(*value);
            format!("{val_str} {unit_str}")
        }
        ExprKind::UnaryOp {
            op: graphcal_syntax::ast::UnaryOp::Neg,
            operand,
        } => {
            format!("-{}", format_bound_display(operand, registry))
        }
        _ => {
            // Fallback: try to evaluate and display raw SI value
            let builtin_consts = builtin_constants();
            let builtin_fns = builtin_functions();
            let empty: HashMap<String, RuntimeValue> = HashMap::new();
            let src = NamedSource::new("", Arc::new(String::new()));
            eval_expr(
                expr,
                &empty,
                &empty,
                &builtin_consts,
                &builtin_fns,
                registry,
                &src,
            )
            .map_or_else(
                |_| "?".to_string(),
                |rv| match rv {
                    RuntimeValue::Scalar(v) => crate::eval::format_number(v),
                    RuntimeValue::Int(i) => format!("{i}"),
                    _ => "?".to_string(),
                },
            )
        }
    }
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
    use crate::ir::lower;
    use crate::tir::type_resolve;
    use graphcal_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn compile_source(source: &str) -> Result<ExecPlan, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        let src = make_src(source);
        let ir = lower(&file, &src).unwrap();
        let tir = type_resolve(ir, &src).unwrap();
        compile(&tir, &src)
    }

    fn scalar(rv: &RuntimeValue) -> f64 {
        match rv {
            RuntimeValue::Scalar(v) => *v,
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    #[test]
    fn compile_simple_const() {
        let plan = compile_source("const G0: Dimensionless = 9.80665;").unwrap();
        assert!((scalar(&plan.const_values["G0"]) - 9.80665).abs() < f64::EPSILON);
        assert!(plan.topo_order.is_empty());
    }

    #[test]
    fn compile_const_chain() {
        let plan = compile_source(
            "const G0: Dimensionless = 9.80665;\nconst TWO_G0: Dimensionless = 2.0 * G0;",
        )
        .unwrap();
        assert!((scalar(&plan.const_values["TWO_G0"]) - 19.6133).abs() < 1e-10);
    }

    #[test]
    fn compile_runtime_dag() {
        let plan = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let x_pos = plan.topo_order.iter().position(|n| n == "x").unwrap();
        let y_pos = plan.topo_order.iter().position(|n| n == "y").unwrap();
        let z_pos = plan.topo_order.iter().position(|n| n == "z").unwrap();
        assert!(x_pos < y_pos);
        assert!(y_pos < z_pos);
    }

    #[test]
    fn compile_const_cycle() {
        let err =
            compile_source("const A: Dimensionless = B + 1.0;\nconst B: Dimensionless = A + 1.0;")
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
}

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use graphcal_syntax::ast::{Expr, ExprKind, FnBody};

use crate::error::GraphcalError;
use crate::registry::{FnDef, FunctionRegistry};
use graphcal_syntax::names::FnName;

/// Check that user-defined functions do not form recursive call cycles.
///
/// Builds a directed call graph among user-defined functions and checks for
/// cycles using topological sort. Direct recursion (f calls f) and mutual
/// recursion (f calls g, g calls f) are both detected.
pub fn check_no_recursion(
    functions: &FunctionRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    // Collect all user-defined function names
    let fn_names: Vec<&FnDef> = functions.all_functions().collect();

    if fn_names.is_empty() {
        return Ok(());
    }

    let mut graph = DiGraph::<FnName, ()>::new();
    let mut index_map: HashMap<FnName, petgraph::graph::NodeIndex> = HashMap::new();

    // Add a node for each function
    for fn_def in &fn_names {
        let idx = graph.add_node(fn_def.name.clone());
        index_map.insert(fn_def.name.clone(), idx);
    }

    // Add edges: if function A calls function B, add edge A -> B
    for fn_def in &fn_names {
        let from = index_map[&fn_def.name];
        let callees = collect_fn_calls(&fn_def.body, &index_map);
        for callee_name in callees {
            let to = index_map[&callee_name];
            graph.add_edge(from, to, ());
        }
    }

    // Build a span lookup from the collected function definitions.
    let fn_spans: HashMap<&FnName, graphcal_syntax::span::Span> =
        fn_names.iter().map(|d| (&d.name, d.span)).collect();

    // Check for cycles
    toposort(&graph, None).map_err(|cycle| {
        let cycle_name = &graph[cycle.node_id()];
        let span = fn_spans
            .get(cycle_name)
            .copied()
            .unwrap_or_else(|| graphcal_syntax::span::Span::new(0, 0));
        GraphcalError::RecursiveFunction {
            name: cycle_name.clone(),
            src: src.clone(),
            span: span.into(),
        }
    })?;

    Ok(())
}

/// Check that user-defined functions do not form recursive call cycles, using TIR.
///
/// Delegates to [`check_no_recursion`] using the function registry from the TIR.
pub fn check_no_recursion_tir(
    tir: &crate::tir::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    check_no_recursion(&tir.registry.functions, src)
}

/// Collect names of user-defined functions called from a function body.
fn collect_fn_calls(
    body: &FnBody,
    user_fns: &HashMap<FnName, petgraph::graph::NodeIndex>,
) -> Vec<FnName> {
    let mut calls = Vec::new();
    match body {
        FnBody::Short(expr) => collect_fn_calls_in_expr(expr, user_fns, &mut calls),
        FnBody::Block { stmts, expr } => {
            for stmt in stmts {
                collect_fn_calls_in_expr(&stmt.value, user_fns, &mut calls);
            }
            collect_fn_calls_in_expr(expr, user_fns, &mut calls);
        }
    }
    calls
}

fn collect_fn_calls_in_expr(
    expr: &Expr,
    user_fns: &HashMap<FnName, petgraph::graph::NodeIndex>,
    calls: &mut Vec<FnName>,
) {
    match &expr.kind {
        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            if user_fns.contains_key(name.value.as_str()) {
                calls.push(name.value.clone());
            }
            for arg in args {
                collect_fn_calls_in_expr(arg, user_fns, calls);
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_fn_calls_in_expr(lhs, user_fns, calls);
            collect_fn_calls_in_expr(rhs, user_fns, calls);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_fn_calls_in_expr(operand, user_fns, calls);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_fn_calls_in_expr(condition, user_fns, calls);
            collect_fn_calls_in_expr(then_branch, user_fns, calls);
            collect_fn_calls_in_expr(else_branch, user_fns, calls);
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => {
            collect_fn_calls_in_expr(inner, user_fns, calls);
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                collect_fn_calls_in_expr(&stmt.value, user_fns, calls);
            }
            collect_fn_calls_in_expr(expr, user_fns, calls);
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            collect_fn_calls_in_expr(expr, user_fns, calls);
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    collect_fn_calls_in_expr(val, user_fns, calls);
                }
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::GraphRef(_)
        | ExprKind::QualifiedGraphRef { .. }
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => {}
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                collect_fn_calls_in_expr(&entry.value, user_fns, calls);
            }
        }
        ExprKind::ForComp { body, .. } => {
            collect_fn_calls_in_expr(body, user_fns, calls);
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_fn_calls_in_expr(source, user_fns, calls);
            collect_fn_calls_in_expr(init, user_fns, calls);
            collect_fn_calls_in_expr(body, user_fns, calls);
        }
        ExprKind::Unfold { init, body, .. } => {
            collect_fn_calls_in_expr(init, user_fns, calls);
            collect_fn_calls_in_expr(body, user_fns, calls);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_fn_calls_in_expr(scrutinee, user_fns, calls);
            for arm in arms {
                collect_fn_calls_in_expr(&arm.body, user_fns, calls);
            }
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
    use crate::prelude::load_prelude;
    use crate::registry::RegistryBuilder;
    use graphcal_syntax::parser::Parser;

    fn check_recursion(source: &str) -> Result<(), GraphcalError> {
        let src = NamedSource::new("test", Arc::new(source.to_string()));
        let file = Parser::new(source).parse_file().unwrap();
        let resolved = crate::resolve::resolve(&file, &src).unwrap();
        let mut builder = RegistryBuilder::new();
        load_prelude(&mut builder);
        for (name, fn_decl, span) in &resolved.functions {
            builder.register_function(FnDef {
                name: FnName::new(name),
                generic_params: fn_decl
                    .generic_params
                    .iter()
                    .map(|g| crate::registry::FnGenericParam {
                        name: g.name.value.clone(),
                        constraint: match g.constraint {
                            graphcal_syntax::ast::GenericConstraint::Dim => {
                                crate::registry::FnGenericConstraint::Dim
                            }
                            graphcal_syntax::ast::GenericConstraint::Index => {
                                crate::registry::FnGenericConstraint::Index
                            }
                            graphcal_syntax::ast::GenericConstraint::Type => {
                                unreachable!(
                                    "`Type` constraint is not valid on function generic parameters"
                                )
                            }
                        },
                    })
                    .collect(),
                params: fn_decl
                    .params
                    .iter()
                    .map(|p| crate::registry::FnParamDef {
                        name: p.name.name.clone(),
                        type_expr: p.type_ann.clone(),
                    })
                    .collect(),
                return_type_expr: fn_decl.return_type.clone(),
                body: fn_decl.body.clone(),
                span: *span,
            });
        }
        let registry = builder.build();
        check_no_recursion(&registry.functions, &src)
    }

    #[test]
    fn no_recursion_ok() {
        let source = r"
            fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
            fn quadruple(x: Dimensionless) -> Dimensionless = double(double(x));
        ";
        check_recursion(source).unwrap();
    }

    #[test]
    fn direct_recursion_error() {
        let source = r"
            fn bad(x: Dimensionless) -> Dimensionless = bad(x);
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn mutual_recursion_error() {
        let source = r"
            fn ping(x: Dimensionless) -> Dimensionless = pong(x);
            fn pong(x: Dimensionless) -> Dimensionless = ping(x);
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn deep_recursion_chain() {
        let source = r"
            fn a(x: Dimensionless) -> Dimensionless = b(x);
            fn b(x: Dimensionless) -> Dimensionless = c(x);
            fn c(x: Dimensionless) -> Dimensionless = a(x);
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_unary_op() {
        let source = r"
            fn bad(x: Dimensionless) -> Dimensionless = -bad(x);
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_if_else() {
        let source = r"
            fn bad(x: Dimensionless) -> Dimensionless =
                if x > 0.0 { bad(x - 1.0) } else { 0.0 };
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_convert() {
        // Recursion hidden inside a Convert expression
        let source = r"
            fn bad(x: Length) -> Length = bad(x) -> m;
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_block_in_short_body() {
        // Recursion in a block expression (inside FnBody::Short)
        let source = r"
            fn bad(x: Dimensionless) -> Dimensionless = {
                let a = bad(x);
                a
            };
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_block_body() {
        // Recursion via FnBody::Block (multi-statement fn body with block stmts)
        let source = r"
            fn bad(x: Dimensionless) -> Dimensionless {
                let a = x + 1.0;
                bad(a)
            }
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_field_access() {
        // collect_fn_calls_in_expr on FieldAccess traverses the inner expression
        let source = r"
            type Pair { x: Dimensionless, y: Dimensionless }
            fn helper(v: Dimensionless) -> Pair = Pair { x: v, y: v };
            fn bad(v: Dimensionless) -> Dimensionless = helper(bad(v)).x;
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_index_access() {
        // collect_fn_calls_in_expr on IndexAccess traverses the inner expression
        let source = r"
            index Phase = { Coast, Burn }
            fn helper(v: Dimensionless) -> Dimensionless = v;
            fn bad(v: Dimensionless) -> Dimensionless = helper(bad(v));
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_map_literal() {
        // Recursion hidden inside a map literal value expression
        let source = r"
            index Phase = { Coast, Burn }
            fn helper(x: Dimensionless) -> Dimensionless[Phase] = {
                Phase::Coast: helper2(x),
                Phase::Burn: x,
            };
            fn helper2(x: Dimensionless) -> Dimensionless = helper(x)[Phase::Coast];
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_for_comp() {
        // Recursion hidden inside a for comprehension body
        let source = r"
            index Phase = { Coast, Burn }
            fn helper(x: Dimensionless) -> Dimensionless[Phase] =
                for p: Phase { helper2(x) };
            fn helper2(x: Dimensionless) -> Dimensionless = helper(x)[Phase::Coast];
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_scan() {
        // Recursion hidden inside a scan body
        let source = r"
            index Phase = { Coast, Burn }
            fn helper(x: Dimensionless) -> Dimensionless[Phase] =
                scan({ Phase::Coast: x, Phase::Burn: x }, 0.0, |acc, val| helper2(acc));
            fn helper2(x: Dimensionless) -> Dimensionless = helper(x)[Phase::Coast];
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn recursion_through_struct_construction() {
        // collect_fn_calls_in_expr on StructConstruction traverses field values
        let source = r"
            type Pair { x: Dimensionless, y: Dimensionless }
            fn helper(v: Dimensionless) -> Pair = Pair { x: bad(v), y: v };
            fn bad(v: Dimensionless) -> Dimensionless = helper(v).x;
        ";
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, GraphcalError::RecursiveFunction { .. }));
    }

    #[test]
    fn no_functions_ok() {
        let source = r"
            param x: Dimensionless = 1.0;
        ";
        check_recursion(source).unwrap();
    }
}

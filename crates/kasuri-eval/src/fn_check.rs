use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use kasuri_syntax::ast::{Expr, ExprKind, FnBody};

use crate::error::KasuriError;
use crate::registry::{FnDef, Registry};

/// Check that user-defined functions do not form recursive call cycles.
///
/// Builds a directed call graph among user-defined functions and checks for
/// cycles using topological sort. Direct recursion (f calls f) and mutual
/// recursion (f calls g, g calls f) are both detected.
pub fn check_no_recursion(
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), KasuriError> {
    // Collect all user-defined function names
    let fn_names: Vec<&FnDef> = registry.all_functions().collect();

    if fn_names.is_empty() {
        return Ok(());
    }

    let mut graph = DiGraph::<String, ()>::new();
    let mut index_map: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

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

    // Check for cycles
    toposort(&graph, None).map_err(|cycle| {
        let cycle_name = &graph[cycle.node_id()];
        let fn_def = registry.get_function(cycle_name).expect("function exists");
        KasuriError::RecursiveFunction {
            name: cycle_name.clone(),
            src: src.clone(),
            span: fn_def.span.into(),
        }
    })?;

    Ok(())
}

/// Collect names of user-defined functions called from a function body.
fn collect_fn_calls(
    body: &FnBody,
    user_fns: &HashMap<String, petgraph::graph::NodeIndex>,
) -> Vec<String> {
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
    user_fns: &HashMap<String, petgraph::graph::NodeIndex>,
    calls: &mut Vec<String>,
) {
    match &expr.kind {
        ExprKind::FnCall { name, args } => {
            if user_fns.contains_key(&name.name) {
                calls.push(name.name.clone());
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
        ExprKind::Convert { expr: inner, .. } => {
            collect_fn_calls_in_expr(inner, user_fns, calls);
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                collect_fn_calls_in_expr(&stmt.value, user_fns, calls);
            }
            collect_fn_calls_in_expr(expr, user_fns, calls);
        }
        ExprKind::FieldAccess { expr, .. } => {
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
        | ExprKind::Bool(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_) => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::prelude::load_prelude;
    use kasuri_syntax::parser::Parser;

    fn check_recursion(source: &str) -> Result<(), KasuriError> {
        let src = NamedSource::new("test", Arc::new(source.to_string()));
        let file = Parser::new(source).parse_file().unwrap();
        let resolved = crate::resolve::resolve(&file, &src).unwrap();
        let mut registry = Registry::new();
        load_prelude(&mut registry);
        for (name, fn_decl, span) in &resolved.functions {
            registry.register_function(FnDef {
                name: name.clone(),
                generic_params: fn_decl
                    .generic_params
                    .iter()
                    .map(|g| g.name.name.clone())
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
        check_no_recursion(&registry, &src)
    }

    #[test]
    fn no_recursion_ok() {
        let source = r#"
            fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
            fn quadruple(x: Dimensionless) -> Dimensionless = double(double(x));
        "#;
        check_recursion(source).unwrap();
    }

    #[test]
    fn direct_recursion_error() {
        let source = r#"
            fn bad(x: Dimensionless) -> Dimensionless = bad(x);
        "#;
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, KasuriError::RecursiveFunction { .. }));
    }

    #[test]
    fn mutual_recursion_error() {
        let source = r#"
            fn ping(x: Dimensionless) -> Dimensionless = pong(x);
            fn pong(x: Dimensionless) -> Dimensionless = ping(x);
        "#;
        let err = check_recursion(source).unwrap_err();
        assert!(matches!(err, KasuriError::RecursiveFunction { .. }));
    }
}

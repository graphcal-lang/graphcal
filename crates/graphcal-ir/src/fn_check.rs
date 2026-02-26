use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use graphcal_syntax::ast::{Expr, ExprKind, FnBody};
use graphcal_syntax::visitor::ExprVisitor;

use graphcal_registry::error::GraphcalError;
use graphcal_registry::registry::{FnDef, FunctionRegistry};
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

/// Visitor that collects user-defined function calls.
struct FnCallCollector<'a> {
    user_fns: &'a HashMap<FnName, petgraph::graph::NodeIndex>,
    calls: Vec<FnName>,
}

impl ExprVisitor for FnCallCollector<'_> {
    type Error = std::convert::Infallible;

    fn visit_fn_call(&mut self, expr: &Expr, args: &[Expr]) -> Result<(), Self::Error> {
        if let ExprKind::FnCall { name, .. } = &expr.kind
            && self.user_fns.contains_key(name.value.as_str())
        {
            self.calls.push(name.value.clone());
        }
        for arg in args {
            self.visit_expr(arg)?;
        }
        Ok(())
    }

    fn visit_qualified_fn_call(&mut self, expr: &Expr, args: &[Expr]) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedFnCall { name, .. } = &expr.kind
            && self.user_fns.contains_key(name.value.as_str())
        {
            self.calls.push(name.value.clone());
        }
        for arg in args {
            self.visit_expr(arg)?;
        }
        Ok(())
    }
}

/// Collect names of user-defined functions called from a function body.
fn collect_fn_calls(
    body: &FnBody,
    user_fns: &HashMap<FnName, petgraph::graph::NodeIndex>,
) -> Vec<FnName> {
    let mut collector = FnCallCollector {
        user_fns,
        calls: Vec::new(),
    };
    match body {
        FnBody::Short(expr) => {
            let _ = collector.visit_expr(expr);
        }
        FnBody::Block { stmts, expr } => {
            for stmt in stmts {
                let _ = collector.visit_expr(&stmt.value);
            }
            let _ = collector.visit_expr(expr);
        }
    }
    collector.calls
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
    use graphcal_registry::prelude::load_prelude;
    use graphcal_registry::registry::RegistryBuilder;
    use graphcal_syntax::parser::Parser;

    fn check_recursion(source: &str) -> Result<(), GraphcalError> {
        let src = NamedSource::new("test", Arc::new(source.to_string()));
        let file = Parser::new(source).parse_file().unwrap();
        let resolved = crate::resolve::resolve(&file, &src).unwrap();
        let mut builder = RegistryBuilder::new();
        load_prelude(&mut builder);
        for entry in &resolved.functions {
            builder.register_function(FnDef {
                name: FnName::new(&entry.name),
                generic_params: entry
                    .decl
                    .generic_params
                    .iter()
                    .map(|g| graphcal_registry::registry::FnGenericParam {
                        name: g.name.value.clone(),
                        constraint: match g.constraint {
                            graphcal_syntax::ast::GenericConstraint::Dim => {
                                graphcal_registry::registry::FnGenericConstraint::Dim
                            }
                            graphcal_syntax::ast::GenericConstraint::Index => {
                                graphcal_registry::registry::FnGenericConstraint::Index
                            }
                            graphcal_syntax::ast::GenericConstraint::Type => {
                                unreachable!(
                                    "`Type` constraint is not valid on function generic parameters"
                                )
                            }
                        },
                    })
                    .collect(),
                params: entry
                    .decl
                    .params
                    .iter()
                    .map(|p| graphcal_registry::registry::FnParamDef {
                        name: p.name.name.clone(),
                        type_expr: p.type_ann.clone(),
                    })
                    .collect(),
                return_type_expr: entry.decl.return_type.clone(),
                body: entry.decl.body.clone(),
                span: entry.span,
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

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
use crate::tir::TIR;

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

    for (name, _, expr, _) in &tir.params {
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
        expressions.insert(name.clone(), expr.clone());
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
            .chain(tir.params.iter())
            .find(|(n, _, _, _)| n == cycle_node)
            .map_or_else(|| Span::new(0, 0), |(_, _, _, s)| *s);
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

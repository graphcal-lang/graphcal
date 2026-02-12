use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use kasuri_syntax::ast::Expr;
use kasuri_syntax::span::Span;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::error::KasuriError;
use crate::eval_expr::eval_expr;
use crate::resolve::ResolvedFile;

/// Topologically sort const declarations and evaluate them at compile time.
/// Returns a map of `const_name` -> `f64` value.
///
/// # Errors
///
/// Returns a [`KasuriError`] if there is a cyclic dependency among constants
/// or if evaluation of a const expression fails.
pub fn eval_consts(
    resolved: &ResolvedFile,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<String, f64>, KasuriError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    if resolved.consts.is_empty() {
        return Ok(HashMap::new());
    }

    // Build a dependency graph among consts
    let mut graph = DiGraph::<String, ()>::new();
    let mut index_map: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

    for (name, _, _) in &resolved.consts {
        let idx = graph.add_node(name.clone());
        index_map.insert(name.clone(), idx);
    }

    for (name, _, _) in &resolved.consts {
        if let Some(deps) = resolved.const_deps.get(name) {
            let from = index_map[name];
            for dep in deps {
                let to = index_map[dep];
                graph.add_edge(to, from, ()); // dep -> name (dep must come first)
            }
        }
    }

    // Topological sort — detect cycles
    let sorted = toposort(&graph, None).map_err(|cycle| {
        let cycle_node = &graph[cycle.node_id()];
        let span = resolved
            .consts
            .iter()
            .find(|(n, _, _)| n == cycle_node)
            .map_or_else(|| Span::new(0, 0), |(_, _, s)| *s);
        KasuriError::CyclicDependency {
            name: cycle_node.clone(),
            src: src.clone(),
            span: span.into(),
        }
    })?;

    // Evaluate in topological order
    let const_exprs: HashMap<&str, &Expr> = resolved
        .consts
        .iter()
        .map(|(name, expr, _)| (name.as_str(), expr))
        .collect();

    let mut values: HashMap<String, f64> = HashMap::new();

    for idx in sorted {
        let name = &graph[idx];
        let expr = const_exprs[name.as_str()];
        let val = eval_expr(expr, &values, &builtin_consts, &builtin_fns, src)?;
        values.insert(name.clone(), val);
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::resolve::resolve;
    use kasuri_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn parse_resolve_eval_consts(source: &str) -> Result<HashMap<String, f64>, KasuriError> {
        let file = Parser::new(source).parse_file().unwrap();
        let src = make_src(source);
        let resolved = resolve(&file, &src)?;
        eval_consts(&resolved, &src)
    }

    #[test]
    fn eval_simple_const() {
        let values = parse_resolve_eval_consts("const G0 = 9.80665;").unwrap();
        assert!((values["G0"] - 9.80665).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_const_chain() {
        let values =
            parse_resolve_eval_consts("const G0 = 9.80665;\nconst TWO_G0 = 2.0 * G0;").unwrap();
        assert!((values["TWO_G0"] - 19.6133).abs() < 1e-10);
    }

    #[test]
    fn eval_const_with_builtin_const() {
        let values = parse_resolve_eval_consts("const HALF_PI = PI / 2.0;").unwrap();
        assert!((values["HALF_PI"] - std::f64::consts::FRAC_PI_2).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_const_with_builtin_fn() {
        let values = parse_resolve_eval_consts("const SQRT2 = sqrt(2.0);").unwrap();
        assert!((values["SQRT2"] - std::f64::consts::SQRT_2).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_const_cycle_detected() {
        let err = parse_resolve_eval_consts("const A = B + 1.0;\nconst B = A + 1.0;").unwrap_err();
        assert!(matches!(err, KasuriError::CyclicDependency { .. }));
    }

    #[test]
    fn eval_no_consts() {
        let values = parse_resolve_eval_consts("param x = 1.0;\nnode y = @x + 2.0;").unwrap();
        assert!(values.is_empty());
    }
}

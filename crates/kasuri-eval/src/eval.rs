use std::collections::HashMap;
use std::sync::Arc;

use miette::{Diagnostic, NamedSource};
use thiserror::Error;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::const_eval::eval_consts;
use crate::dag::{RuntimeGraph, build_dag};
use crate::error::KasuriError;
use crate::eval_expr::eval_expr;
use crate::resolve::{DeclCategory, ResolvedFile, resolve};
use kasuri_syntax::parser::ParseError;

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclType {
    Const,
    Param,
    Node,
}

/// The result of evaluating a `.ksr` file.
#[derive(Debug)]
pub struct EvalResult {
    /// Const values in source order.
    pub consts: Vec<(String, f64)>,
    /// Param values in source order.
    pub params: Vec<(String, f64)>,
    /// Node values in source order.
    pub nodes: Vec<(String, f64)>,
    /// All values in source order with their declaration type.
    pub all: Vec<(String, f64, DeclType)>,
}

/// Full pipeline: parse -> resolve -> const eval -> DAG build -> runtime eval.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing or evaluation fails.
pub fn compile_and_eval(source: &str) -> Result<EvalResult, CompileError> {
    compile_and_eval_named(source, "input")
}

/// Full pipeline with a custom source name (used for file paths in diagnostics).
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing or evaluation fails.
pub fn compile_and_eval_named(source: &str, name: &str) -> Result<EvalResult, CompileError> {
    let src = NamedSource::new(name, Arc::new(source.to_string()));
    let file = kasuri_syntax::parser::Parser::with_name(source, name).parse_file()?;
    let resolved = resolve(&file, &src)?;
    let const_values = eval_consts(&resolved, &src)?;
    let dag = build_dag(&resolved, &src)?;
    let result = evaluate(&resolved, &dag, &const_values, &src)?;
    Ok(result)
}

/// Evaluate the runtime DAG given resolved const values.
fn evaluate(
    resolved: &ResolvedFile,
    dag: &RuntimeGraph,
    const_values: &HashMap<String, f64>,
    src: &NamedSource<Arc<String>>,
) -> Result<EvalResult, KasuriError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    let mut values: HashMap<String, f64> = HashMap::new();

    // Insert const values into the lookup table
    for (name, val) in const_values {
        values.insert(name.clone(), *val);
    }

    // Evaluate in topological order (params first, then nodes that depend on them)
    for idx in &dag.topo_order {
        let name = &dag.graph[*idx];
        if values.contains_key(name) {
            continue;
        }
        let expr = &dag.expressions[name];
        let val = eval_expr(expr, &values, &builtin_consts, &builtin_fns, src)?;
        values.insert(name.clone(), val);
    }

    // Collect results in source order
    let consts = resolved
        .consts
        .iter()
        .map(|(name, _, _)| (name.clone(), const_values[name]))
        .collect();
    let params = resolved
        .params
        .iter()
        .map(|(name, _, _)| (name.clone(), values[name]))
        .collect();
    let nodes = resolved
        .nodes
        .iter()
        .map(|(name, _, _)| (name.clone(), values[name]))
        .collect();

    // Build the `all` list in source order
    let all = resolved
        .source_order
        .iter()
        .map(|(name, cat)| {
            let val = match cat {
                DeclCategory::Const => const_values[name],
                DeclCategory::Param | DeclCategory::Node => values[name],
            };
            let decl_type = match cat {
                DeclCategory::Const => DeclType::Const,
                DeclCategory::Param => DeclType::Param,
                DeclCategory::Node => DeclType::Node,
            };
            (name.clone(), val, decl_type)
        })
        .collect();

    Ok(EvalResult {
        consts,
        params,
        nodes,
        all,
    })
}

/// Top-level compile error that wraps both parse and eval errors.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(#[from] ParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Eval(#[from] KasuriError),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn find_value(result: &EvalResult, name: &str) -> f64 {
        result
            .consts
            .iter()
            .chain(result.params.iter())
            .chain(result.nodes.iter())
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
    }

    #[test]
    #[expect(clippy::suboptimal_flops)] // Clearer to express expected math directly
    fn eval_rocket_milestone() {
        let source = include_str!("../../../tests/fixtures/rocket.ksr");
        let result = compile_and_eval(source).unwrap();

        assert!((find_value(&result, "dry_mass") - 1200.0).abs() < f64::EPSILON);
        assert!((find_value(&result, "fuel_mass") - 2800.0).abs() < f64::EPSILON);
        assert!((find_value(&result, "isp") - 320.0).abs() < f64::EPSILON);
        assert!((find_value(&result, "G0") - 9.80665).abs() < 1e-10);

        let v_exhaust = find_value(&result, "v_exhaust");
        assert!(
            (v_exhaust - 320.0 * 9.80665).abs() < 0.001,
            "v_exhaust = {v_exhaust}"
        );

        let mass_ratio = find_value(&result, "mass_ratio");
        assert!(
            (mass_ratio - (4000.0 / 1200.0)).abs() < 1e-6,
            "mass_ratio = {mass_ratio}"
        );

        let delta_v = find_value(&result, "delta_v");
        let expected_delta_v = 320.0 * 9.80665 * (4000.0_f64 / 1200.0).ln();
        assert!(
            (delta_v - expected_delta_v).abs() < 0.001,
            "delta_v = {delta_v}, expected = {expected_delta_v}"
        );
    }

    #[test]
    #[expect(clippy::suboptimal_flops)] // Clearer to express expected math directly
    fn eval_constants_ksr() {
        let source = include_str!("../../../tests/fixtures/constants.ksr");
        let result = compile_and_eval(source).unwrap();

        assert!((find_value(&result, "G0") - 9.80665).abs() < f64::EPSILON);
        assert!((find_value(&result, "TWO_G0") - 19.6133).abs() < 1e-10);
        assert!(
            (find_value(&result, "HALF_PI") - std::f64::consts::FRAC_PI_2).abs() < f64::EPSILON
        );
        assert!((find_value(&result, "SQRT2") - std::f64::consts::SQRT_2).abs() < f64::EPSILON);

        let circumference = find_value(&result, "circumference");
        let expected = 2.0 * std::f64::consts::PI * 100.0;
        assert!(
            (circumference - expected).abs() < 1e-10,
            "circumference = {circumference}"
        );

        let area = find_value(&result, "area");
        let expected_area = std::f64::consts::PI * 100.0_f64.powf(2.0);
        assert!((area - expected_area).abs() < 1e-10, "area = {area}");
    }

    #[test]
    fn eval_if_else_true_branch() {
        let result =
            compile_and_eval("param x = 5.0;\nnode y = if @x > 0.0 { @x } else { 0.0 };").unwrap();
        assert!((find_value(&result, "y") - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_if_else_false_branch() {
        let result =
            compile_and_eval("param x = -3.0;\nnode y = if @x > 0.0 { @x } else { 0.0 };").unwrap();
        assert!((find_value(&result, "y") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_and() {
        let result = compile_and_eval(
            "param a = 1.0;\nparam b = 0.0;\nnode c = if @a > 0.0 && @b > 0.0 { 1.0 } else { 0.0 };",
        )
        .unwrap();
        assert!((find_value(&result, "c") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_or() {
        let result = compile_and_eval(
            "param a = 1.0;\nparam b = 0.0;\nnode c = if @a > 0.0 || @b > 0.0 { 1.0 } else { 0.0 };",
        )
        .unwrap();
        assert!((find_value(&result, "c") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_unary_neg() {
        let result = compile_and_eval("param x = 5.0;\nnode y = -@x;").unwrap();
        assert!((find_value(&result, "y") - (-5.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_power() {
        let result = compile_and_eval("param x = 3.0;\nnode y = @x ^ 2.0;").unwrap();
        assert!((find_value(&result, "y") - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_result_source_order() {
        let result = compile_and_eval(
            "param b = 2.0;\nparam a = 1.0;\nnode z = @a + @b;\nnode y = @z * 2.0;",
        )
        .unwrap();
        assert_eq!(result.params[0].0, "b");
        assert_eq!(result.params[1].0, "a");
        assert_eq!(result.nodes[0].0, "z");
        assert_eq!(result.nodes[1].0, "y");
    }

    #[test]
    fn eval_result_all_field_source_order() {
        let source = include_str!("../../../tests/fixtures/rocket.ksr");
        let result = compile_and_eval(source).unwrap();
        let names: Vec<&str> = result.all.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "dry_mass",
                "fuel_mass",
                "isp",
                "G0",
                "v_exhaust",
                "mass_ratio",
                "delta_v"
            ]
        );
        assert_eq!(result.all[0].2, DeclType::Param);
        assert_eq!(result.all[3].2, DeclType::Const);
        assert_eq!(result.all[4].2, DeclType::Node);
    }
}

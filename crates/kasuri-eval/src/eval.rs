use std::collections::HashMap;
use std::sync::Arc;

use miette::{Diagnostic, NamedSource};
use thiserror::Error;

use indexmap::IndexMap;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::const_eval::eval_consts;
use crate::dag::{RuntimeGraph, build_dag};
use crate::dim_check::{DeclaredType, check_dimensions};
use crate::error::KasuriError;
use crate::eval_expr::{RuntimeValue, eval_expr};
use crate::prelude::load_prelude;
use crate::registry::{self, Registry};
use crate::resolve::{DeclCategory, ResolvedFile, resolve};
use kasuri_syntax::ast::{DeclKind, ExprKind};
use kasuri_syntax::dimension::Dimension;
use kasuri_syntax::parser::ParseError;

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclType {
    Const,
    Param,
    Node,
}

/// Display unit metadata: the unit name(s) and scale factor for pretty-printing.
#[derive(Debug, Clone)]
pub struct DisplayUnit {
    /// Human-readable unit string (e.g., "km", "m/s^2", "km/hour")
    pub label: String,
    /// Scale factor from SI to this display unit: `display_value = si_value / scale`
    pub scale: f64,
}

/// A runtime value: either a scalar with dimension and display info, or a struct.
#[derive(Debug, Clone)]
pub enum Value {
    Scalar {
        /// The value in base SI units.
        si_value: f64,
        /// The dimension of this value.
        dimension: Dimension,
        /// Optional display unit for pretty-printing.
        display_unit: Option<DisplayUnit>,
    },
    Struct {
        /// The struct type name.
        type_name: String,
        /// Fields in definition order.
        fields: IndexMap<String, Self>,
    },
}

impl Value {
    /// Get the SI value. Panics on struct values.
    #[must_use]
    pub fn si_value(&self) -> f64 {
        match self {
            Self::Scalar { si_value, .. } => *si_value,
            Self::Struct { type_name, .. } => {
                panic!("called si_value() on struct `{type_name}`")
            }
        }
    }

    /// Get the dimension. Panics on struct values.
    #[must_use]
    pub fn dimension(&self) -> Dimension {
        match self {
            Self::Scalar { dimension, .. } => *dimension,
            Self::Struct { type_name, .. } => {
                panic!("called dimension() on struct `{type_name}`")
            }
        }
    }

    /// Get the value formatted for display: in display units if available, otherwise SI.
    #[must_use]
    pub fn display_value(&self) -> f64 {
        match self {
            Self::Scalar {
                si_value,
                display_unit,
                ..
            } => display_unit
                .as_ref()
                .map_or(*si_value, |du| *si_value / du.scale),
            Self::Struct { type_name, .. } => {
                panic!("called display_value() on struct `{type_name}`")
            }
        }
    }

    /// Get the unit label for display, or `None` for dimensionless/no-unit values.
    #[must_use]
    pub fn display_label(&self) -> Option<&str> {
        match self {
            Self::Scalar { display_unit, .. } => display_unit.as_ref().map(|du| du.label.as_str()),
            Self::Struct { .. } => None,
        }
    }
}

/// The result of evaluating a `.ksr` file.
#[derive(Debug)]
pub struct EvalResult {
    /// Const values in source order.
    pub consts: Vec<(String, Value)>,
    /// Param values in source order.
    pub params: Vec<(String, Value)>,
    /// Node values in source order.
    pub nodes: Vec<(String, Value)>,
    /// All values in source order with their declaration type.
    pub all: Vec<(String, Value, DeclType)>,
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

    // Build registry: prelude + user-declared dimensions/units
    let mut registry = Registry::new();
    load_prelude(&mut registry);
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(d) => {
                let dim = if let Some(def) = &d.definition {
                    registry
                        .resolve_dim_expr(def)
                        .ok_or_else(|| KasuriError::UnknownDimension {
                            name: d.name.name.clone(),
                            src: src.clone(),
                            span: d.name.span.into(),
                        })?
                } else {
                    // Base dimension — should already be in the prelude.
                    // If not, this is a user-defined base dimension (not supported yet).
                    continue;
                };
                registry.register_dimension(&d.name.name, dim);
            }
            DeclKind::Unit(u) => {
                let dim = registry.resolve_dim_expr(&u.dim_type).ok_or_else(|| {
                    KasuriError::UnknownDimension {
                        name: u.name.name.clone(),
                        src: src.clone(),
                        span: u.name.span.into(),
                    }
                })?;
                let scale = if let Some(def) = &u.definition {
                    let (_unit_dim, base_scale) = registry
                        .resolve_unit_expr(&def.unit_expr)
                        .ok_or_else(|| KasuriError::UnknownUnit {
                            name: u.name.name.clone(),
                            src: src.clone(),
                            span: def.span.into(),
                        })?;
                    def.scale * base_scale
                } else {
                    1.0
                };
                registry.register_unit(&u.name.name, dim, scale);
            }
            DeclKind::Type(t) => {
                let mut fields = Vec::new();
                for field in &t.fields {
                    let dim = registry.resolve_type_expr(&field.type_ann).ok_or_else(|| {
                        KasuriError::UnknownDimension {
                            name: field.name.name.clone(),
                            src: src.clone(),
                            span: field.name.span.into(),
                        }
                    })?;
                    fields.push(registry::StructField {
                        name: field.name.name.clone(),
                        dimension: dim,
                    });
                }
                registry.register_struct(registry::StructDef {
                    name: t.name.name.clone(),
                    fields,
                });
            }
            _ => {}
        }
    }

    // Register user-defined functions
    for (name, fn_decl, span) in &resolved.functions {
        registry.register_function(registry::FnDef {
            name: name.clone(),
            generic_params: fn_decl
                .generic_params
                .iter()
                .map(|g| g.name.name.clone())
                .collect(),
            params: fn_decl
                .params
                .iter()
                .map(|p| registry::FnParamDef {
                    name: p.name.name.clone(),
                    type_expr: p.type_ann.clone(),
                })
                .collect(),
            return_type_expr: fn_decl.return_type.clone(),
            body: fn_decl.body.clone(),
            span: *span,
        });
    }

    // Check for recursive function calls
    crate::fn_check::check_no_recursion(&registry, &src)?;

    // Dimension check
    let declared_types = check_dimensions(&file, &registry, &src)?;

    let const_values = eval_consts(&resolved, &registry, &src)?;
    let dag = build_dag(&resolved, &src)?;
    let result = evaluate(
        &resolved,
        &dag,
        &const_values,
        &registry,
        &declared_types,
        &src,
    )?;
    Ok(result)
}

/// Convert a `RuntimeValue` to a `Value` using declared type info and display unit extraction.
fn runtime_to_value(
    rv: &RuntimeValue,
    declared_type: Option<&DeclaredType>,
    display_unit: Option<DisplayUnit>,
    registry: &Registry,
) -> Value {
    match rv {
        RuntimeValue::Scalar(si_value) => {
            let dimension = match declared_type {
                Some(DeclaredType::Scalar(d)) => *d,
                _ => Dimension::DIMENSIONLESS,
            };
            Value::Scalar {
                si_value: *si_value,
                dimension,
                display_unit,
            }
        }
        RuntimeValue::Struct { type_name, fields } => {
            let struct_def = registry.get_struct(type_name);
            let converted_fields = fields
                .iter()
                .map(|(field_name, field_rv)| {
                    let field_declared = struct_def.and_then(|sd| {
                        sd.fields
                            .iter()
                            .find(|f| f.name == *field_name)
                            .map(|f| DeclaredType::Scalar(f.dimension))
                    });
                    let val = runtime_to_value(field_rv, field_declared.as_ref(), None, registry);
                    (field_name.clone(), val)
                })
                .collect();
            Value::Struct {
                type_name: type_name.clone(),
                fields: converted_fields,
            }
        }
    }
}

/// Evaluate the runtime DAG given resolved const values.
fn evaluate(
    resolved: &ResolvedFile,
    dag: &RuntimeGraph,
    const_values: &HashMap<String, RuntimeValue>,
    registry: &Registry,
    declared_types: &HashMap<String, DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> Result<EvalResult, KasuriError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let mut values: HashMap<String, RuntimeValue> = HashMap::new();

    // Insert const values into the lookup table
    for (name, val) in const_values {
        values.insert(name.clone(), val.clone());
    }

    // Evaluate in topological order (params first, then nodes that depend on them)
    for idx in &dag.topo_order {
        let name = &dag.graph[*idx];
        if values.contains_key(name) {
            continue;
        }
        let expr = &dag.expressions[name];
        let val = eval_expr(
            expr,
            &values,
            &empty_locals,
            &builtin_consts,
            &builtin_fns,
            registry,
            src,
        )?;
        values.insert(name.clone(), val);
    }

    // Build a map from name -> expression for display unit extraction
    let expr_map: HashMap<&str, &kasuri_syntax::ast::Expr> = resolved
        .consts
        .iter()
        .chain(resolved.params.iter())
        .chain(resolved.nodes.iter())
        .map(|(name, expr, _)| (name.as_str(), expr))
        .collect();

    // Helper to build a Value for a given declaration name
    let make_value = |name: &str, rv: &RuntimeValue| -> Value {
        let display_unit = expr_map
            .get(name)
            .and_then(|expr| extract_display_unit(expr, registry));
        runtime_to_value(rv, declared_types.get(name), display_unit, registry)
    };

    // Collect results in source order
    let consts = resolved
        .consts
        .iter()
        .map(|(name, _, _)| {
            let val = make_value(name, &const_values[name]);
            (name.clone(), val)
        })
        .collect();
    let params = resolved
        .params
        .iter()
        .map(|(name, _, _)| {
            let val = make_value(name, &values[name]);
            (name.clone(), val)
        })
        .collect();
    let nodes = resolved
        .nodes
        .iter()
        .map(|(name, _, _)| {
            let val = make_value(name, &values[name]);
            (name.clone(), val)
        })
        .collect();

    // Build the `all` list in source order
    let all = resolved
        .source_order
        .iter()
        .map(|(name, cat)| {
            let rv = match cat {
                DeclCategory::Const => &const_values[name],
                DeclCategory::Param | DeclCategory::Node => &values[name],
            };
            let val = make_value(name, rv);
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

/// Extract display unit from an expression.
///
/// - `ExprKind::Convert { target, .. }` -> use the target unit
/// - `ExprKind::UnitLiteral { unit, .. }` -> use the literal's unit
/// - Anything else -> `None` (display in SI)
fn extract_display_unit(
    expr: &kasuri_syntax::ast::Expr,
    registry: &Registry,
) -> Option<DisplayUnit> {
    match &expr.kind {
        ExprKind::Convert { target, .. } => {
            let (_dim, scale) = registry.resolve_unit_expr(target)?;
            Some(DisplayUnit {
                label: format_unit_expr(target),
                scale,
            })
        }
        ExprKind::UnitLiteral { unit, .. } => {
            let (_dim, scale) = registry.resolve_unit_expr(unit)?;
            Some(DisplayUnit {
                label: format_unit_expr(unit),
                scale,
            })
        }
        _ => None,
    }
}

/// Format a `UnitExpr` as a human-readable label.
/// E.g., `m`, `km/hour`, `kg * m / s^2`
fn format_unit_expr(expr: &kasuri_syntax::ast::UnitExpr) -> String {
    use kasuri_syntax::ast::MulDivOp;

    let mut numerator = Vec::new();
    let mut denominator = Vec::new();

    for item in &expr.terms {
        let mut part = item.name.name.clone();
        if let Some(pow) = item.power
            && pow != 1
        {
            part = format!("{part}^{pow}");
        }
        match item.op {
            MulDivOp::Mul => numerator.push(part),
            MulDivOp::Div => denominator.push(part),
        }
    }

    if denominator.is_empty() {
        numerator.join(" * ")
    } else if numerator.len() == 1 && denominator.len() == 1 {
        format!("{}/{}", numerator[0], denominator[0])
    } else {
        let num = numerator.join(" * ");
        let den = denominator.join(" * ");
        format!("{num}/{den}")
    }
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

    /// Find the SI value of a named scalar declaration.
    fn find_value(result: &EvalResult, name: &str) -> f64 {
        result
            .consts
            .iter()
            .chain(result.params.iter())
            .chain(result.nodes.iter())
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .si_value()
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
            compile_and_eval("param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };").unwrap();
        assert!((find_value(&result, "y") - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_if_else_false_branch() {
        let result =
            compile_and_eval("param x: Dimensionless = -3.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };").unwrap();
        assert!((find_value(&result, "y") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_and() {
        let result = compile_and_eval(
            "param a: Dimensionless = 1.0;\nparam b: Dimensionless = 0.0;\nnode c: Dimensionless = if @a > 0.0 && @b > 0.0 { 1.0 } else { 0.0 };",
        )
        .unwrap();
        assert!((find_value(&result, "c") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_or() {
        let result = compile_and_eval(
            "param a: Dimensionless = 1.0;\nparam b: Dimensionless = 0.0;\nnode c: Dimensionless = if @a > 0.0 || @b > 0.0 { 1.0 } else { 0.0 };",
        )
        .unwrap();
        assert!((find_value(&result, "c") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_unary_neg() {
        let result =
            compile_and_eval("param x: Dimensionless = 5.0;\nnode y: Dimensionless = -@x;")
                .unwrap();
        assert!((find_value(&result, "y") - (-5.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_power() {
        let result =
            compile_and_eval("param x: Dimensionless = 3.0;\nnode y: Dimensionless = @x ^ 2.0;")
                .unwrap();
        assert!((find_value(&result, "y") - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_result_source_order() {
        let result = compile_and_eval(
            "param b: Dimensionless = 2.0;\nparam a: Dimensionless = 1.0;\nnode z: Dimensionless = @a + @b;\nnode y: Dimensionless = @z * 2.0;",
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

    #[test]
    fn eval_orbital_milestone() {
        let source = include_str!("../../../tests/fixtures/orbital.ksr");
        let result = compile_and_eval(source).unwrap();

        // alt = 400 km -> SI: 400_000.0 m
        assert!(
            (find_value(&result, "alt") - 400_000.0).abs() < f64::EPSILON,
            "alt = {}",
            find_value(&result, "alt")
        );
        // period = 90 min -> SI: 5400.0 s
        assert!(
            (find_value(&result, "period") - 5400.0).abs() < f64::EPSILON,
            "period = {}",
            find_value(&result, "period")
        );
        // R_EARTH = 6371 km -> SI: 6_371_000.0 m
        assert!(
            (find_value(&result, "R_EARTH") - 6_371_000.0).abs() < f64::EPSILON,
            "R_EARTH = {}",
            find_value(&result, "R_EARTH")
        );

        // circumference = 2 * PI * (6_371_000 + 400_000)
        let expected_circumference = 2.0 * std::f64::consts::PI * 6_771_000.0;
        assert!(
            (find_value(&result, "circumference") - expected_circumference).abs() < 0.01,
            "circumference = {}",
            find_value(&result, "circumference")
        );

        // speed = circumference / period
        let expected_speed = expected_circumference / 5400.0;
        assert!(
            (find_value(&result, "speed") - expected_speed).abs() < 0.01,
            "speed = {}",
            find_value(&result, "speed")
        );

        // speed_kmh = speed (same SI value, only display unit changes)
        assert!(
            (find_value(&result, "speed_kmh") - expected_speed).abs() < 0.01,
            "speed_kmh SI = {}",
            find_value(&result, "speed_kmh")
        );

        // Check display units
        let speed_kmh = result.nodes.iter().find(|(n, _)| n == "speed_kmh").unwrap();
        assert_eq!(speed_kmh.1.display_label(), Some("km/hour"));
        let display_kmh = speed_kmh.1.display_value();
        let expected_kmh = expected_speed / (1000.0 / 3600.0);
        assert!(
            (display_kmh - expected_kmh).abs() < 0.01,
            "speed_kmh display = {display_kmh}"
        );
    }

    #[test]
    fn eval_hohmann_milestone() {
        let source = include_str!("../../../tests/fixtures/hohmann.ksr");
        let result = compile_and_eval(source).unwrap();

        // transfer is a struct — check its fields via total_dv and tof_hours nodes
        let total_dv = find_value(&result, "total_dv");
        // LEO-to-GEO Hohmann total delta-v should be ~3935 m/s
        assert!(
            total_dv > 3900.0 && total_dv < 4000.0,
            "total_dv = {total_dv}"
        );

        let tof_hours = find_value(&result, "tof_hours");
        // Transfer time ~5.26 hours -> SI ~18924 seconds
        assert!(
            tof_hours > 18000.0 && tof_hours < 20000.0,
            "tof_hours SI = {tof_hours}"
        );

        // Check that tof_hours has display unit "hour"
        let tof_entry = result.nodes.iter().find(|(n, _)| n == "tof_hours").unwrap();
        assert_eq!(tof_entry.1.display_label(), Some("hour"));
        let tof_display = tof_entry.1.display_value();
        assert!(
            tof_display > 5.0 && tof_display < 6.0,
            "tof display = {tof_display} hours"
        );

        // Check that transfer node is a struct
        let transfer_entry = result.nodes.iter().find(|(n, _)| n == "transfer").unwrap();
        match &transfer_entry.1 {
            Value::Struct { type_name, fields } => {
                assert_eq!(type_name, "TransferResult");
                assert_eq!(fields.len(), 4);
                assert!(fields.contains_key("dv1"));
                assert!(fields.contains_key("dv2"));
                assert!(fields.contains_key("total_dv"));
                assert!(fields.contains_key("tof"));
            }
            Value::Scalar { .. } => panic!("expected struct for transfer"),
        }
    }

    #[test]
    fn eval_functions_milestone() {
        let source = include_str!("../../../tests/fixtures/functions.ksr");
        let result = compile_and_eval(source).unwrap();

        // v_parking: orbital velocity at LEO (R_EARTH + 200 km)
        // sqrt(GM_EARTH / (R_EARTH + 200 km)) = sqrt(3.986004418e14 / 6571000)
        let v_parking = find_value(&result, "v_parking");
        assert!(
            v_parking > 7700.0 && v_parking < 7800.0,
            "v_parking = {v_parking}"
        );

        // v_check should equal v_parking (same computation via fn-calling-fn)
        let v_check = find_value(&result, "v_check");
        assert!(
            (v_check - v_parking).abs() < 1e-6,
            "v_check = {v_check}, v_parking = {v_parking}"
        );

        // midpoint_alt: lerp(200 km, 35786 km, 0.5) = 17993 km -> 17993000 m SI
        let midpoint = find_value(&result, "midpoint_alt");
        assert!(
            (midpoint - 17_993_000.0).abs() < 1.0,
            "midpoint_alt = {midpoint}"
        );

        // transfer: Hohmann LEO-to-GEO, total_dv ~3935 m/s
        let transfer_entry = result.nodes.iter().find(|(n, _)| n == "transfer").unwrap();
        match &transfer_entry.1 {
            Value::Struct { type_name, fields } => {
                assert_eq!(type_name, "TransferResult");
                assert_eq!(fields.len(), 3);
                let total_dv = fields["total_dv"].si_value();
                assert!(
                    total_dv > 3900.0 && total_dv < 4000.0,
                    "total_dv = {total_dv}"
                );
            }
            Value::Scalar { .. } => panic!("expected struct for transfer"),
        }
    }
}

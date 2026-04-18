#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]
use super::*;
use crate::error::GraphcalError;
use graphcal_io::RealFileSystem;

const FS: RealFileSystem = RealFileSystem;

/// Find the SI value of a named scalar declaration.
fn find_value(result: &EvalResult, name: &str) -> f64 {
    // Check consts first (they are not wrapped in Result)
    if let Some((_, val)) = result.consts.iter().find(|(n, _)| n.as_str() == name) {
        return val.si_value().unwrap();
    }
    // Check params and nodes (wrapped in Result)
    result
        .params
        .iter()
        .chain(result.nodes.iter())
        .find(|(n, _)| n.as_str() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .si_value()
        .unwrap()
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "clearer to express expected math directly"
)]
fn eval_rocket_milestone() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "dry_mass") - 1200.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "fuel_mass") - 2800.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "isp") - 320.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "g0") - 9.80665).abs() < 1e-10);

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
#[expect(
    clippy::suboptimal_flops,
    reason = "clearer to express expected math directly"
)]
fn eval_constants_ksr() {
    let source = include_str!("../../../../tests/fixtures/constants.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "g0") - 9.80665).abs() < f64::EPSILON);
    assert!((find_value(&result, "two_g0") - 19.6133).abs() < 1e-10);
    assert!((find_value(&result, "half_pi") - std::f64::consts::FRAC_PI_2).abs() < f64::EPSILON);
    assert!((find_value(&result, "sqrt2") - std::f64::consts::SQRT_2).abs() < f64::EPSILON);

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
    let result = compile_and_eval(
        "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 5.0).abs() < f64::EPSILON);
}

#[test]
fn eval_if_else_false_branch() {
    let result = compile_and_eval(
        "param x: Dimensionless = -3.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };",
    )
    .unwrap();
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
        compile_and_eval("param x: Dimensionless = 5.0;\nnode y: Dimensionless = -@x;").unwrap();
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
    assert_eq!(result.params[0].0.as_str(), "b");
    assert_eq!(result.params[1].0.as_str(), "a");
    assert_eq!(result.nodes[0].0.as_str(), "z");
    assert_eq!(result.nodes[1].0.as_str(), "y");
}

#[test]
fn eval_result_all_field_source_order() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let result = compile_and_eval(source).unwrap();
    let names: Vec<&str> = result.all.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "dry_mass",
            "fuel_mass",
            "isp",
            "g0",
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
    let source = include_str!("../../../../tests/fixtures/orbital.gcl");
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
        (find_value(&result, "r_earth") - 6_371_000.0).abs() < f64::EPSILON,
        "R_EARTH = {}",
        find_value(&result, "r_earth")
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
    let speed_kmh = result
        .nodes
        .iter()
        .find(|(n, _)| n.as_str() == "speed_kmh")
        .unwrap();
    let speed_kmh_val = speed_kmh.1.as_ref().unwrap();
    assert_eq!(
        speed_kmh_val.display_label(&result.base_dim_symbols),
        Some("km/hour".to_string())
    );
    let display_kmh = speed_kmh_val.display_value().unwrap();
    let expected_kmh = expected_speed / (1000.0 / 3600.0);
    assert!(
        (display_kmh - expected_kmh).abs() < 0.01,
        "speed_kmh display = {display_kmh}"
    );
}

#[test]
fn eval_hohmann_milestone() {
    let source = include_str!("../../../../tests/fixtures/hohmann.gcl");
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
    let tof_entry = result
        .nodes
        .iter()
        .find(|(n, _)| n.as_str() == "tof_hours")
        .unwrap();
    let tof_val = tof_entry.1.as_ref().unwrap();
    assert_eq!(
        tof_val.display_label(&result.base_dim_symbols),
        Some("hour".to_string())
    );
    let tof_display = tof_val.display_value().unwrap();
    assert!(
        tof_display > 5.0 && tof_display < 6.0,
        "tof display = {tof_display} hours"
    );

    // Check that transfer node is a struct
    let transfer_entry = result
        .nodes
        .iter()
        .find(|(n, _)| n.as_str() == "transfer")
        .unwrap();
    match transfer_entry.1.as_ref().unwrap() {
        Value::Struct {
            type_name, fields, ..
        } => {
            assert_eq!(type_name.as_str(), "TransferResult");
            assert_eq!(fields.len(), 4);
            assert!(fields.contains_key("dv1"));
            assert!(fields.contains_key("dv2"));
            assert!(fields.contains_key("total_dv"));
            assert!(fields.contains_key("tof"));
        }
        _ => panic!("expected struct for transfer"),
    }
}

#[test]
fn eval_generics_milestone() {
    let source = include_str!("../../../../tests/fixtures/generics.gcl");
    let result = compile_and_eval(source).unwrap();

    // x_pos: field access on Vec3<Length, Eci>, should be 6878 km = 6878000 m
    let x_pos = find_value(&result, "x_pos");
    assert!((x_pos - 6_878_000.0).abs() < 1.0, "x_pos = {x_pos}");

    // y_vel: field access on Vec3<Velocity, Eci>, should be 7.67 km/s = 7670 m/s
    let y_vel = find_value(&result, "y_vel");
    assert!((y_vel - 7670.0).abs() < 1.0, "y_vel = {y_vel}");

    // pos3_eci_x: explicit type args, 100 km = 100000 m
    let pos3_eci_x = find_value(&result, "pos3_eci_x");
    assert!(
        (pos3_eci_x - 100_000.0).abs() < 1.0,
        "pos3_eci_x = {pos3_eci_x}"
    );

    // pos3_default_y: default type param (F = Unframed), 20 km = 20000 m
    let pos3_default_y = find_value(&result, "pos3_default_y");
    assert!(
        (pos3_default_y - 20_000.0).abs() < 1.0,
        "pos3_default_y = {pos3_default_y}"
    );

    // pos_body_x: as cast (phantom only), same value as pos_eci.x = 6878 km = 6878000 m
    let pos_body_x = find_value(&result, "pos_body_x");
    assert!(
        (pos_body_x - 6_878_000.0).abs() < 1.0,
        "pos_body_x = {pos_body_x}"
    );

    // total_dv: non-generic struct still works, 100 + 200 = 300 m/s
    let total_dv = find_value(&result, "total_dv");
    assert!((total_dv - 300.0).abs() < 0.01, "total_dv = {total_dv}");
}

/// Helper: find a named value and return it (for indexed value tests).
fn find_entry(result: &EvalResult, name: &str) -> Value {
    result
        .all
        .iter()
        .find(|(n, _, _)| n.as_str() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .clone()
}

/// Helper: extract indexed entries as `Vec<(variant, si_value)>`.
fn indexed_si_values(value: &Value) -> Vec<(&str, f64)> {
    match value {
        Value::Indexed { entries, .. } => entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.si_value().unwrap()))
            .collect(),
        _ => panic!("expected indexed value, got {value:?}"),
    }
}

#[test]
fn eval_indexed_milestone() {
    let source = include_str!("../../../../tests/fixtures/indexed.gcl");
    let result = compile_and_eval(source).unwrap();

    // delta_v param: 2460, 120, 1830 m/s (SI)
    let dv = find_entry(&result, "delta_v");
    let dv_vals = indexed_si_values(&dv);
    assert_eq!(dv_vals.len(), 3);
    assert!(
        (dv_vals[0].1 - 2460.0).abs() < 0.01,
        "Departure = {}",
        dv_vals[0].1
    );
    assert!(
        (dv_vals[1].1 - 120.0).abs() < 0.01,
        "Correction = {}",
        dv_vals[1].1
    );
    assert!(
        (dv_vals[2].1 - 1830.0).abs() < 0.01,
        "Insertion = {}",
        dv_vals[2].1
    );

    // double_dv: doubled values
    let ddv = find_entry(&result, "double_dv");
    let double_dv_vals = indexed_si_values(&ddv);
    assert!((double_dv_vals[0].1 - 4920.0).abs() < 0.01);
    assert!((double_dv_vals[1].1 - 240.0).abs() < 0.01);
    assert!((double_dv_vals[2].1 - 3660.0).abs() < 0.01);

    // total_dv: 2460 + 120 + 1830 = 4410 m/s
    assert!((find_value(&result, "total_dv") - 4410.0).abs() < 0.01);

    // max_dv: 2460
    assert!((find_value(&result, "max_dv") - 2460.0).abs() < 0.01);

    // min_dv: 120
    assert!((find_value(&result, "min_dv") - 120.0).abs() < 0.01);

    // mean_dv: 4410 / 3 = 1470
    assert!((find_value(&result, "mean_dv") - 1470.0).abs() < 0.01);

    // n_maneuvers: 3
    assert!((find_value(&result, "n_maneuvers") - 3.0).abs() < f64::EPSILON);

    // departure_dv: 2460
    assert!((find_value(&result, "departure_dv") - 2460.0).abs() < 0.01);

    // cumulative_dv: scan cumulative [2460, 2460+120=2580, 2580+1830=4410]
    let cumulative = find_entry(&result, "cumulative_dv");
    let cumulative_vals = indexed_si_values(&cumulative);
    assert!((cumulative_vals[0].1 - 2460.0).abs() < 0.01);
    assert!((cumulative_vals[1].1 - 2580.0).abs() < 0.01);
    assert!((cumulative_vals[2].1 - 4410.0).abs() < 0.01);

    // total_check (generic function): same as total_dv
    assert!((find_value(&result, "total_check") - 4410.0).abs() < 0.01);
}

#[test]
fn eval_table_literal_nat_range_1d() {
    let source = r"
param v: Dimensionless[3] = table[3] {
    1.0;
    2.0;
    3.0;
};
node total: Dimensionless = sum(for i: range(3) { @v[i] });
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "total") - 6.0).abs() < f64::EPSILON);
}

#[test]
fn eval_table_literal_nat_range_2d() {
    let source = r"
param m: Dimensionless[2, 3] = table[2, 3] {
    1.0, 2.0, 3.0;
    4.0, 5.0, 6.0;
};
node row_sums: Dimensionless[2] = for i: range(2) {
    sum(for j: range(3) { @m[i, j] })
};
node total: Dimensionless = sum(for i: range(2) { @row_sums[i] });
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "total") - 21.0).abs() < f64::EPSILON);
}

#[test]
fn eval_table_literal() {
    let source = include_str!("../../../../tests/fixtures/table_literal.gcl");
    let result = compile_and_eval(source).unwrap();

    // 1D table: delta_v should match delta_v_map
    let dv = find_entry(&result, "delta_v");
    let dv_map = find_entry(&result, "delta_v_map");
    let dv_vals = indexed_si_values(&dv);
    let dv_map_vals = indexed_si_values(&dv_map);
    assert_eq!(dv_vals.len(), dv_map_vals.len());
    for (a, b) in dv_vals.iter().zip(dv_map_vals.iter()) {
        assert!((a.1 - b.1).abs() < f64::EPSILON, "{} != {}", a.1, b.1);
    }

    // Derived nodes work: total_dv = 2460 + 120 + 1830 = 4410 m/s
    assert!((find_value(&result, "total_dv") - 4410.0).abs() < 0.01);

    // Access specific 2D entry: launch_departure_mass = 5000 kg
    assert!((find_value(&result, "launch_departure_mass") - 5000.0).abs() < 0.01);

    // 3D table: access specific entries
    assert!((find_value(&result, "nominal_launch_departure") - 5000.0).abs() < 0.01);
    assert!((find_value(&result, "contingency_arrival_insertion") - 3800.0).abs() < 0.01);
}

// --- Comparison and boolean operator tests ---

#[test]
fn eval_comparison_eq() {
    let result = compile_and_eval(
        "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x == 5.0 { 1.0 } else { 0.0 };",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_comparison_neq() {
    let result = compile_and_eval(
        "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x != 3.0 { 1.0 } else { 0.0 };",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_comparison_lt() {
    let result = compile_and_eval(
        "param x: Dimensionless = 3.0;\nnode y: Dimensionless = if @x < 5.0 { 1.0 } else { 0.0 };",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_comparison_lte() {
    let result = compile_and_eval(
        "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x <= 5.0 { 1.0 } else { 0.0 };",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_comparison_gt() {
    let result = compile_and_eval(
        "param x: Dimensionless = 10.0;\nnode y: Dimensionless = if @x > 5.0 { 1.0 } else { 0.0 };",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_comparison_gte() {
    let result = compile_and_eval(
        "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x >= 5.0 { 1.0 } else { 0.0 };",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_boolean_not() {
    let result = compile_and_eval(
        "param x: Dimensionless = 0.0;\nnode y: Dimensionless = if !(@x > 0.0) { 1.0 } else { 0.0 };",
    ).unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_boolean_and_short_circuit() {
    // When first operand is false, second should not matter
    let result = compile_and_eval(
        "param x: Dimensionless = 0.0;\nnode y: Dimensionless = if @x > 0.0 && @x < 10.0 { 1.0 } else { 0.0 };",
    ).unwrap();
    assert!((find_value(&result, "y") - 0.0).abs() < f64::EPSILON);
}

#[test]
fn eval_boolean_or_short_circuit() {
    // When first operand is true, second should not matter
    let result = compile_and_eval(
        "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x > 0.0 || @x < -10.0 { 1.0 } else { 0.0 };",
    ).unwrap();
    assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_nested_if_else() {
    let result = compile_and_eval(
        "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x > 10.0 { 3.0 } else { if @x > 0.0 { 2.0 } else { 1.0 } };",
    ).unwrap();
    assert!((find_value(&result, "y") - 2.0).abs() < f64::EPSILON);
}

#[test]
fn eval_unary_neg_dimensioned() {
    let result = compile_and_eval("param x: Length = 100.0 m;\nnode y: Length = -@x;").unwrap();
    assert!((find_value(&result, "y") - (-100.0)).abs() < f64::EPSILON);
}

// --- Override tests ---

fn parse_expr(s: &str) -> graphcal_compiler::syntax::ast::Expr {
    graphcal_compiler::syntax::parser::Parser::new(s)
        .parse_single_expr()
        .unwrap()
}

#[test]
fn override_param_changes_result() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    // Default isp=320 s, override to 450 s => higher delta_v
    let default = compile_and_eval_named(source, "test").unwrap();
    let default_dv = find_value(&default, "delta_v");

    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("isp"), parse_expr("450.0 s"));
    let overridden = compile_and_eval_with_overrides(source, "test", &overrides, true).unwrap();
    let new_dv = find_value(&overridden, "delta_v");

    assert!(new_dv > default_dv, "higher isp should give higher delta_v");
}

#[test]
fn override_with_wrong_dimension_errors() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    // isp expects Time, not Mass
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("isp"), parse_expr("450.0 kg"));
    let result = compile_and_eval_with_overrides(source, "test", &overrides, true);
    assert!(result.is_err());
}

#[test]
fn override_node_errors() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("delta_v"), parse_expr("100.0 m/s"));
    let result = compile_and_eval_with_overrides(source, "test", &overrides, true);
    match result {
        Err(CompileError::Eval(GraphcalError::OverrideNotAParam { name, actual_kind })) => {
            assert_eq!(name.as_str(), "delta_v");
            assert_eq!(actual_kind.to_string(), "node");
        }
        other => panic!("expected OverrideNotAParam, got {other:?}"),
    }
}

#[test]
fn override_const_errors() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("g0"), parse_expr("10.0 m/s^2"));
    let result = compile_and_eval_with_overrides(source, "test", &overrides, true);
    match result {
        Err(CompileError::Eval(GraphcalError::OverrideNotAParam { name, actual_kind })) => {
            assert_eq!(name.as_str(), "g0");
            assert_eq!(actual_kind.to_string(), "const");
        }
        other => panic!("expected OverrideNotAParam, got {other:?}"),
    }
}

#[test]
fn override_unknown_param_errors() {
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("nonexistent"), parse_expr("100"));
    let result = compile_and_eval_with_overrides(source, "test", &overrides, true);
    match result {
        Err(CompileError::Eval(GraphcalError::OverrideUnknownParam { name })) => {
            assert_eq!(name.as_str(), "nonexistent");
        }
        other => panic!("expected OverrideUnknownParam, got {other:?}"),
    }
}

#[test]
fn required_param_without_override_errors() {
    let source = "param x: Dimensionless;\nnode y: Dimensionless = @x + 1.0;";
    let result = compile_and_eval_with_overrides(source, "test", &HashMap::new(), true);
    match result {
        Err(CompileError::Eval(GraphcalError::RequiredParamNotProvided { name, .. })) => {
            assert_eq!(name, "x");
        }
        other => panic!("expected RequiredParamNotProvided, got {other:?}"),
    }
}

#[test]
fn required_param_with_override_succeeds() {
    let source = "param x: Dimensionless;\nnode y: Dimensionless = @x + 1.0;";
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("x"), parse_expr("42.0"));
    let result = compile_and_eval_with_overrides(source, "test", &overrides, true).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 43.0).abs() < f64::EPSILON, "y = {y}, expected 43.0");
}

#[test]
fn project_multi_file_rocket() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/rocket_split/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let delta_v = find_value(&result, "delta_v");
    let expected_delta_v = 320.0 * 9.80665 * (4000.0_f64 / 1200.0).ln();
    assert!(
        (delta_v - expected_delta_v).abs() < 0.001,
        "delta_v = {delta_v}, expected = {expected_delta_v}"
    );
}

#[test]
fn project_import_alias() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/alias/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 43.0).abs() < f64::EPSILON, "y = {y}, expected 43.0");
}

#[test]
fn project_import_alias_conflict_resolution() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/alias_conflict/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let sum = find_value(&result, "sum");
    assert!(
        (sum - 3.0).abs() < f64::EPSILON,
        "sum = {sum}, expected 3.0"
    );
}

// --- Module import tests ---

#[test]
fn project_module_import_const() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/module_import/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let g = find_value(&result, "g");
    assert!((g - 9.80665).abs() < 1e-6, "g = {g}, expected 9.80665");
}

#[test]
fn project_module_import_const_alias() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/module_import_alias/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let g = find_value(&result, "g");
    assert!((g - 9.80665).abs() < 1e-6, "g = {g}, expected 9.80665");
}

#[test]
fn project_module_import_graph_ref() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/module_import_graph_ref/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let total = find_value(&result, "total_mass");
    assert!(
        (total - 4000.0).abs() < f64::EPSILON,
        "total_mass = {total}, expected 4000.0"
    );
}

#[test]
fn project_module_import_mixed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/module_import_mixed/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let delta_v = find_value(&result, "delta_v");
    let expected = 320.0 * 9.80665 * (4000.0_f64 / 1200.0).ln();
    assert!(
        (delta_v - expected).abs() < 0.001,
        "delta_v = {delta_v}, expected = {expected}"
    );
}

// --- Runtime arithmetic error tests ---

/// Helper: assert that a specific node in the result has a `NodeError::EvalFailed`
/// whose message contains `needle`.
fn assert_node_error(source: &str, node_name: &str, needle: &str) {
    let result = compile_and_eval(source).unwrap();
    let (_, node_result, _) = result
        .all
        .iter()
        .find(|(n, _, _)| n.as_str() == node_name)
        .unwrap_or_else(|| panic!("node `{node_name}` not found"));
    match node_result {
        Err(NodeError::EvalFailed { message }) => {
            assert!(
                message.contains(needle),
                "expected error containing {needle:?}, got {message:?}"
            );
        }
        Err(other) => panic!("expected EvalFailed containing {needle:?}, got {other:?}"),
        Ok(val) => panic!("expected error for `{node_name}`, got value {val:?}"),
    }
}

#[test]
fn eval_division_by_zero() {
    assert_node_error(
        "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x / 0.0;",
        "y",
        "division by zero",
    );
}

#[test]
fn eval_zero_divided_by_zero() {
    assert_node_error(
        "param x: Dimensionless = 0.0;\nnode y: Dimensionless = @x / 0.0;",
        "y",
        "division by zero",
    );
}

#[test]
fn eval_sqrt_negative() {
    assert_node_error("node y: Dimensionless = sqrt(-1.0);", "y", "NaN");
}

#[test]
fn eval_ln_zero() {
    assert_node_error("node y: Dimensionless = ln(0.0);", "y", "infinite");
}

#[test]
fn eval_ln_negative() {
    assert_node_error("node y: Dimensionless = ln(-1.0);", "y", "NaN");
}

#[test]
fn eval_exp_overflow() {
    assert_node_error("node y: Dimensionless = exp(1000.0);", "y", "infinite");
}

#[test]
fn eval_power_negative_base_frac_exp() {
    assert_node_error("node y: Dimensionless = (-1.0) ^ 0.5;", "y", "NaN");
}

#[test]
fn eval_valid_division_ok() {
    let result =
        compile_and_eval("param x: Dimensionless = 10.0;\nnode y: Dimensionless = @x / 2.0;")
            .unwrap();
    assert!((find_value(&result, "y") - 5.0).abs() < f64::EPSILON);
}

#[test]
fn eval_valid_sqrt_ok() {
    let result = compile_and_eval("node y: Dimensionless = sqrt(4.0);").unwrap();
    assert!((find_value(&result, "y") - 2.0).abs() < f64::EPSILON);
}

// --- Error containment tests ---

#[test]
fn eval_error_does_not_block_independent_nodes() {
    let result = compile_and_eval(
        "param x: Dimensionless = 1.0;\n\
         node bad: Dimensionless = @x / 0.0;\n\
         node good: Dimensionless = @x + 1.0;",
    )
    .unwrap();
    // bad should have an error
    assert!(
        result
            .nodes
            .iter()
            .find(|(n, _)| n.as_str() == "bad")
            .unwrap()
            .1
            .is_err()
    );
    // good should succeed because it does not depend on bad
    assert!((find_value(&result, "good") - 2.0).abs() < f64::EPSILON);
}

#[test]
fn eval_error_propagates_to_dependents() {
    let result = compile_and_eval(
        "param x: Dimensionless = 1.0;\n\
         node bad: Dimensionless = @x / 0.0;\n\
         node downstream: Dimensionless = @bad + 1.0;",
    )
    .unwrap();
    // bad fails with EvalFailed
    let bad_result = &result
        .nodes
        .iter()
        .find(|(n, _)| n.as_str() == "bad")
        .unwrap()
        .1;
    assert!(matches!(bad_result, Err(NodeError::EvalFailed { .. })));
    // downstream fails with DependencyFailed
    let ds_result = &result
        .nodes
        .iter()
        .find(|(n, _)| n.as_str() == "downstream")
        .unwrap()
        .1;
    assert!(matches!(ds_result, Err(NodeError::DependencyFailed { .. })));
}

#[test]
fn eval_has_errors_true_when_node_fails() {
    let result =
        compile_and_eval("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x / 0.0;")
            .unwrap();
    assert!(result.has_errors());
}

#[test]
fn eval_has_errors_false_when_all_ok() {
    let result =
        compile_and_eval("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;")
            .unwrap();
    assert!(!result.has_errors());
}

// --- Integer type tests ---

/// Helper: find a named Int value.
fn find_int_value(result: &EvalResult, name: &str) -> i64 {
    let val = result
        .all
        .iter()
        .find(|(n, _, _)| n.as_str() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
    match val {
        Value::Int(i) => *i,
        other => panic!("expected Int for `{name}`, got {other:?}"),
    }
}

/// Helper: find a named Bool value.
fn find_bool_value(result: &EvalResult, name: &str) -> bool {
    let val = result
        .all
        .iter()
        .find(|(n, _, _)| n.as_str() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
    match val {
        Value::Bool(b) => *b,
        other => panic!("expected Bool for `{name}`, got {other:?}"),
    }
}

#[test]
fn eval_integers_milestone() {
    let source = include_str!("../../../../tests/fixtures/integers.gcl");
    let result = compile_and_eval(source).unwrap();

    assert_eq!(find_int_value(&result, "a"), 10);
    assert_eq!(find_int_value(&result, "b"), 3);
    assert_eq!(find_int_value(&result, "sum"), 13);
    assert_eq!(find_int_value(&result, "diff"), 7);
    assert_eq!(find_int_value(&result, "prod"), 30);
    assert_eq!(find_int_value(&result, "quot"), 3); // truncating division
    assert_eq!(find_int_value(&result, "rem"), 1);
    assert_eq!(find_int_value(&result, "power"), 9);
    assert_eq!(find_int_value(&result, "neg_a"), -10);

    assert!(find_bool_value(&result, "a_gt_b"));
    assert!(!find_bool_value(&result, "a_eq_b"));
    assert!(!find_bool_value(&result, "a_le_b"));

    assert_eq!(find_int_value(&result, "seven"), 7);
    assert_eq!(find_int_value(&result, "clamped"), 7); // 10 > 7, so clamp to 7

    // to_float(10) = 10.0
    assert!((find_value(&result, "a_float") - 10.0).abs() < f64::EPSILON);
    // to_int(3.7) = 3 (truncating)
    assert_eq!(find_int_value(&result, "back_to_int"), 3);
}

#[test]
fn eval_int_division_by_zero() {
    assert_node_error(
        "param x: Int = 10;\nnode y: Int = @x / 0;",
        "y",
        "integer division by zero",
    );
}

#[test]
fn eval_int_modulo_by_zero() {
    assert_node_error(
        "param x: Int = 10;\nnode y: Int = @x % 0;",
        "y",
        "integer modulo by zero",
    );
}

#[test]
fn eval_int_negative_exponent() {
    // `-1` is parsed as UnaryOp::Neg(Integer(1)), not a literal, so dim_check
    // rejects it as a non-literal exponent before the evaluator sees it.
    let err = compile_and_eval("param x: Int = 2;\nnode y: Int = @x ^ -1;");
    assert!(err.is_err());
}

#[test]
fn eval_int_mixed_type_error() {
    // Int + Scalar should be a type error
    let err = compile_and_eval("param x: Int = 10;\nnode y: Dimensionless = @x + 1.0;");
    assert!(err.is_err());
}

#[test]
fn eval_int_with_unit_parse_error() {
    // `10 km` should be a parse error
    let err = compile_and_eval("param x: Length = 10 km;");
    assert!(err.is_err());
}

// --- Instantiated import tests ---

#[test]
fn project_instantiated_import_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // dry_mass overridden to 800 kg, fuel_mass default 2800 kg, isp default 320 s
    // delta_v = 320 * 9.80665 * ln((800 + 2800) / 800) = 3138.128 * ln(4.5)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}

#[test]
fn project_instantiated_import_multi() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import_multi/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // stage_1: dry_mass=800, fuel_mass=2800 (default), isp=320
    // stage_1::delta_v = 320 * 9.80665 * ln(3600/800)
    let dv_stage1 = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    // stage_2: dry_mass=500, fuel_mass=2800 (default), isp=450
    // stage_2::delta_v = 450 * 9.80665 * ln(3300/500)
    let dv_stage2 = 450.0 * 9.80665 * (3300.0_f64 / 500.0).ln();
    let total_dv = find_value(&result, "total_dv");
    let expected = dv_stage1 + dv_stage2;
    assert!(
        (total_dv - expected).abs() < 0.01,
        "total_dv = {total_dv}, expected = {expected}"
    );
}

#[test]
fn project_instantiated_import_partial() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import_partial/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // dry_mass overridden to 800, fuel_mass and isp keep defaults (2800 kg, 320 s)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}

#[test]
fn project_instantiated_import_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import_module/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // dry_mass overridden to 800, fuel_mass default 2800, isp default 320
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let dv = find_value(&result, "dv");
    assert!(
        (dv - expected_delta_v).abs() < 0.01,
        "dv = {dv}, expected = {expected_delta_v}"
    );
    let mr = find_value(&result, "mr");
    assert!((mr - 4.5).abs() < 1e-6, "mr = {mr}, expected = 4.5");
}

#[test]
fn project_instantiated_import_graph_ref() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import_graph_ref/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // my_mass = 800 kg, passed as dry_mass binding via @my_mass
    // delta_v = 320 * 9.80665 * ln(3600/800)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}

// ---- Bare module path eval tests ----

#[test]
fn project_bare_import_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/bare_import_selective/src/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let total_mass = find_value(&result, "total_mass");
    assert!(
        (total_mass - 4000.0).abs() < f64::EPSILON,
        "total_mass = {total_mass}, expected 4000.0"
    );
}

#[test]
fn project_bare_import_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/bare_import_module/src/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let g = find_value(&result, "g");
    assert!((g - 9.80665).abs() < 1e-10, "g = {g}, expected 9.80665");
}

#[test]
fn project_bare_import_nested() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/bare_import_nested/src/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let dv = find_value(&result, "dv");
    assert!((dv - 2460.0).abs() < 0.01, "dv = {dv}, expected 2460.0");
}

#[test]
fn project_bare_import_instantiated() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/bare_import_instantiated/src/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // dry_mass = 800 kg, fuel_mass = 3200 kg, isp = 320 s
    // delta_v = 320 * 9.80665 * ln(4000/800)
    let expected_dv = 320.0 * 9.80665 * (4000.0_f64 / 800.0).ln();
    let dv = find_value(&result, "dv");
    assert!(
        (dv - expected_dv).abs() < 0.01,
        "dv = {dv}, expected = {expected_dv}"
    );
}

#[test]
fn project_bare_import_mixed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/bare_import_mixed/src/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let v_exhaust = find_value(&result, "v_exhaust");
    let expected = 320.0 * 9.80665;
    assert!(
        (v_exhaust - expected).abs() < 0.01,
        "v_exhaust = {v_exhaust}, expected = {expected}"
    );
}

#[test]
fn project_bare_import_custom_src() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/bare_import_custom_src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 43.0).abs() < f64::EPSILON, "y = {y}, expected 43.0");
}

mod prop {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn division_of_finite_nonzero_is_finite(
            a in proptest::num::f64::NORMAL,
            b in proptest::num::f64::NORMAL,
        ) {
            prop_assume!(b != 0.0 && a.is_finite() && b.is_finite());
            let source = format!(
                "param x: Dimensionless = {a:e};\nparam y: Dimensionless = {b:e};\nnode z: Dimensionless = @x / @y;"
            );
            let r = compile_and_eval(&source).unwrap();
            let z_result = &r.all.iter()
                .find(|(n, _, _)| n.as_str() == "z")
                .unwrap().1;
            match z_result {
                Ok(val) => {
                    let z = val.si_value().unwrap();
                    prop_assert!(z.is_finite(), "division produced non-finite: {z}");
                }
                Err(NodeError::EvalFailed { message }) => {
                    // Overflow to infinity is correctly caught
                    prop_assert!(
                        message.contains("overflow") || message.contains("infinite"),
                        "unexpected error: {message}"
                    );
                }
                Err(e) => prop_assert!(false, "unexpected error type: {e:?}"),
            }
        }

        #[test]
        fn sqrt_of_positive_is_finite(a in 0.0f64..1e150) {
            let source = format!(
                "param x: Dimensionless = {a:e};\nnode y: Dimensionless = sqrt(@x);"
            );
            let result = compile_and_eval(&source).unwrap();
            let y = find_value(&result, "y");
            prop_assert!(y.is_finite(), "sqrt produced non-finite: {y}");
        }

        #[test]
        fn exp_of_small_is_finite(a in -700.0f64..700.0) {
            let source = format!(
                "param x: Dimensionless = {a:e};\nnode y: Dimensionless = exp(@x);"
            );
            let result = compile_and_eval(&source).unwrap();
            let y = find_value(&result, "y");
            prop_assert!(y.is_finite(), "exp produced non-finite: {y}");
        }
    }
}

// --- Strict param defaults tests ---

#[test]
fn cli_strict_partial_override_errors() {
    // When overrides are provided but not all params are overridden, should error
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("isp"), parse_expr("450.0 s"));
    // allow_defaults = false → should error because dry_mass and fuel_mass not overridden
    let result = compile_and_eval_with_overrides(source, "test", &overrides, false);
    match result {
        Err(CompileError::Eval(GraphcalError::DefaultParamNotProvided { name, .. })) => {
            // Should error on one of the non-overridden params
            assert!(
                name == "dry_mass" || name == "fuel_mass",
                "expected dry_mass or fuel_mass, got {name}"
            );
        }
        other => panic!("expected DefaultParamNotProvided, got {other:?}"),
    }
}

#[test]
fn cli_strict_all_overrides_succeeds() {
    // When ALL params are overridden, should succeed even with allow_defaults=false
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("dry_mass"), parse_expr("800.0 kg"));
    overrides.insert(DeclName::new("fuel_mass"), parse_expr("3200.0 kg"));
    overrides.insert(DeclName::new("isp"), parse_expr("450.0 s"));
    let result = compile_and_eval_with_overrides(source, "test", &overrides, false);
    assert!(
        result.is_ok(),
        "all params overridden should succeed: {result:?}"
    );
}

#[test]
fn cli_strict_no_overrides_succeeds() {
    // When NO overrides at all, defaults are used freely (no trigger)
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let result = compile_and_eval_with_overrides(source, "test", &HashMap::new(), false);
    assert!(
        result.is_ok(),
        "no overrides should use defaults freely: {result:?}"
    );
}

#[test]
fn cli_strict_allow_defaults_opt_out() {
    // When allow_defaults=true, partial overrides are fine
    let source = include_str!("../../../../tests/fixtures/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::new("isp"), parse_expr("450.0 s"));
    let result = compile_and_eval_with_overrides(source, "test", &overrides, true);
    assert!(
        result.is_ok(),
        "allow_defaults should permit partial overrides: {result:?}"
    );
}

#[test]
fn import_strict_partial_binding_errors() {
    // Parameterized import without #[allow_defaults] and partial binding → error
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import_strict_error/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS);
    match result {
        Err(CompileError::Eval(GraphcalError::DefaultParamNotProvided { name, .. })) => {
            // Should error on one of the unbound default params (fuel_mass or isp)
            assert!(
                name == "fuel_mass" || name == "isp",
                "expected fuel_mass or isp, got {name}"
            );
        }
        other => panic!("expected DefaultParamNotProvided, got {other:?}"),
    }
}

#[test]
fn import_with_allow_defaults_succeeds() {
    // Parameterized import WITH #[allow_defaults] and partial binding → success
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS);
    assert!(
        result.is_ok(),
        "import with #[allow_defaults] should succeed: {result:?}"
    );
}

#[test]
fn allow_defaults_on_non_import_errors() {
    // #[allow_defaults] on a node declaration → InvalidAttributeTarget
    let source = "#[allow_defaults]\nnode x: Dimensionless = 1.0;";
    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(GraphcalError::InvalidAttributeTarget {
            attr_name, kind, ..
        })) => {
            assert_eq!(attr_name, "allow_defaults");
            assert_eq!(kind, "node");
        }
        other => panic!("expected InvalidAttributeTarget, got {other:?}"),
    }
}

#[test]
fn allow_defaults_on_param_errors() {
    // #[allow_defaults] on a param declaration → InvalidAttributeTarget
    let source = "#[allow_defaults]\nparam x: Dimensionless = 1.0;";
    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(GraphcalError::InvalidAttributeTarget {
            attr_name, kind, ..
        })) => {
            assert_eq!(attr_name, "allow_defaults");
            assert_eq!(kind, "param");
        }
        other => panic!("expected InvalidAttributeTarget, got {other:?}"),
    }
}

// --- Required param (no default) import tests ---

#[test]
fn project_required_param_import() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/required_param_import/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // radius = 6371 km, circumference = 2 * PI * radius
    let expected = 2.0 * std::f64::consts::PI * 6_371_000.0; // in metres (SI)
    let circumference = find_value(&result, "circumference");
    assert!(
        (circumference - expected).abs() < 1.0,
        "circumference = {circumference}, expected = {expected}"
    );
}

// --- Injectable index tests ---

#[test]
fn project_injectable_index_kind_mismatch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/injectable_index_kind_mismatch/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS);
    match result {
        Err(CompileError::Eval(GraphcalError::IndexKindMismatch {
            dep_index,
            bound_index,
            ..
        })) => {
            assert_eq!(dep_index, "Phase");
            assert_eq!(bound_index, "TimeStep");
        }
        other => panic!("expected IndexKindMismatch, got {other:?}"),
    }
}

#[test]
fn project_injectable_index_strict_default_not_provided() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/injectable_index_strict/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS);
    match result {
        Err(CompileError::Eval(GraphcalError::DefaultIndexNotProvided { name, .. })) => {
            assert_eq!(name, "Phase");
        }
        other => panic!("expected DefaultIndexNotProvided, got {other:?}"),
    }
}

#[test]
fn project_injectable_index_basic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/injectable_index_basic/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // total = sum(10.0 + 20.0) = 30.0
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 30.0).abs() < 1e-10,
        "result = {result_val}, expected 30.0"
    );
}

#[test]
fn project_instantiated_import_type_binding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/instantiated_import_type_binding/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // origin_size = 1.0 m (the lib's `Widget { size: 1.0 m }` rewritten to
    // `MyWidget { size: 1.0 m }` after type substitution)
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 1.0).abs() < 1e-10,
        "result = {result_val}, expected 1.0"
    );
}

#[test]
fn project_injectable_index_expected_fail() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/injectable_index_expected_fail/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // The within_limit assertion should pass overall because Overdrive is marked expected_fail.
    let assert_result = result
        .assertions
        .iter()
        .find(|(name, _, _)| name.as_str().contains("within_limit"))
        .expect("within_limit assertion not found");
    assert!(
        matches!(assert_result.1, AssertResult::Pass),
        "expected Pass, got {:?}",
        assert_result.1
    );
}

// ---- Inline DAG tests (Phase 5) ----

#[test]
fn inline_dag_basic_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/inline_dag_basic/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let val = find_value(&result, "final_result");
    assert!((val - 20.0).abs() < 1e-10, "expected 20.0, got {val}");
}

#[test]
fn inline_dag_import_parent_scope() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/inline_dag_import_parent/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    // Orbital velocity at 400 km: sqrt(GM / (R + h))
    // GM = 3.986004418e14, R = 6371000, h = 400000
    let expected = (3.986_004_418e14_f64 / (6_371_000.0 + 400_000.0)).sqrt();
    let val = find_value(&result, "result");
    assert!(
        (val - expected).abs() < 0.01,
        "expected {expected}, got {val}"
    );
}

#[test]
fn inline_dag_namespace_multi_instantiation() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/inline_dag_namespace/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();
    let doubled = find_value(&result, "doubled_result");
    let tripled = find_value(&result, "tripled_result");
    assert!(
        (doubled - 20.0).abs() < 1e-10,
        "expected 20.0, got {doubled}"
    );
    assert!(
        (tripled - 30.0).abs() < 1e-10,
        "expected 30.0, got {tripled}"
    );
}

#[test]
fn inline_dag_recursive_error() {
    // Direct recursion: dag includes itself.
    let source = "
dag recursive {
    param x: Dimensionless;
    include recursive(x: 1.0) { result };
    node result: Dimensionless = @x;
}
include recursive(x: 1.0) { result };
";
    let result = compile_and_eval(source);
    assert!(result.is_err(), "recursive DAG should fail");
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("recursive DAG instantiation"),
        "error should mention recursive DAG: {err_msg}"
    );
}

#[test]
fn inline_dag_from_source() {
    // Test inline DAG from in-memory source.
    let source = "
pub dim Velocity = Length / Time;

dag add_velocities {
    param a: Velocity;
    param b: Velocity;
    node sum: Velocity = @a + @b;
}

param v1: Velocity = 10.0 m/s;
param v2: Velocity = 5.0 m/s;
include add_velocities(a: @v1, b: @v2) { sum as total };
node result: Velocity = @total;
";
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!((val - 15.0).abs() < 1e-10, "expected 15.0, got {val}");
}

// ---- Cross-file DAG tests (Phase 6.10) ----

#[test]
fn cross_file_dag_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/cross_file_dag/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();

    // double_speed doubles the input: 200.0 * 2.0 = 400.0
    let doubled = find_value(&result, "final_result");
    assert!(
        (doubled - 400.0).abs() < 1e-10,
        "expected 400.0, got {doubled}"
    );

    // mach = 200.0 / 343.0 (SPEED_OF_SOUND imported via `import ..` in the DAG)
    let mach = find_value(&result, "final_mach");
    let expected_mach = 200.0 / 343.0;
    assert!(
        (mach - expected_mach).abs() < 1e-10,
        "expected {expected_mach}, got {mach}"
    );
}

// ---- Bare module path DAG reference tests ----

#[test]
fn bare_module_dag_ref() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/multi/bare_dag_ref/src/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, true, &FS).unwrap();

    // double DAG: 21.0 * 2.0 = 42.0
    let answer = find_value(&result, "answer");
    assert!((answer - 42.0).abs() < 1e-10, "expected 42.0, got {answer}");
}

//! Property-based and edge-case tests to find critical bugs in graphcal.
//!
//! These tests focus on areas that matter most to engineering users:
//! - Correctness of numeric evaluation (arithmetic, conversions)
//! - Display/formatting of output values
//! - Coordinate-index cardinality and endpoint precision
//! - Unit conversion accuracy
#![cfg(test)]
#![expect(
    clippy::cast_precision_loss,
    reason = "test code intentionally tests precision edge cases"
)]
#![expect(
    clippy::needless_raw_string_hashes,
    reason = "raw strings used for readability of graphcal source"
)]

use graphcal_compiler::registry::time_zone::IanaTimeZoneId;
use graphcal_eval::eval::{EvalResult, NodeError, Value, compile_and_eval};
use proptest::prelude::*;

// ============================================================================
// Helpers
// ============================================================================

/// Find the SI value of a named quantity declaration.
fn find_value(result: &EvalResult, name: &str) -> f64 {
    if let Some((_, val)) = result.consts.iter().find(|(n, _)| n.to_string() == name) {
        return val.as_ref().unwrap().si_value().unwrap();
    }
    result
        .params
        .iter()
        .chain(result.nodes.iter())
        .find(|(n, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .si_value()
        .unwrap()
}

/// Find the Int value of a named declaration.
fn find_int_value(result: &EvalResult, name: &str) -> i64 {
    let val = result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
    match val {
        Value::Int(i) => *i,
        other => panic!("expected Int for `{name}`, got {other:?}"),
    }
}

/// Find a named entry as a Value.
fn find_entry(result: &EvalResult, name: &str) -> Value {
    result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .clone()
}

/// Check if a named node has an error.
fn has_node_error(result: &EvalResult, name: &str) -> bool {
    result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
        .is_some_and(|(_, r, _)| r.is_err())
}

/// Get the node error message for a named node.
fn get_node_error_message(result: &EvalResult, name: &str) -> Option<String> {
    result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
        .and_then(|(_, r, _)| match r {
            Err(NodeError::EvalFailed { message }) => Some(message.clone()),
            _ => None,
        })
}

// ============================================================================
// Exact to_int() conversion and range checking (#85, #398)
//
// `to_int(x)` rejects non-integer and out-of-range values instead of silently
// selecting a rounding policy or saturating via `f as i64`.
// ============================================================================

#[test]
fn to_int_rejects_non_integer_values() {
    for value in [3.7, -3.7] {
        let source = format!("node x: Int = to_int({value});");
        let result = compile_and_eval(&source).unwrap();
        let message = get_node_error_message(&result, "x").expect("x should fail");
        assert!(
            message.contains("is not integer-valued"),
            "unexpected error for {value}: {message}"
        );
        assert!(
            message.contains("apply trunc(), floor(), ceil(), or round() explicitly"),
            "missing explicit-rounding guidance for {value}: {message}"
        );
    }
}

#[test]
fn to_int_accepts_an_explicit_truncation_policy() {
    let source = "node positive: Int = to_int(trunc(3.7));\n\
                  node negative: Int = to_int(trunc(-3.7));";
    let result = compile_and_eval(source).unwrap();
    assert_eq!(find_int_value(&result, "positive"), 3);
    assert_eq!(find_int_value(&result, "negative"), -3);
}

#[test]
fn to_int_of_large_positive_float_should_error_or_be_exact() {
    // 1e18 fits in i64 (max is ~9.2e18), so it should work
    let source = "node x: Int = to_int(1e18);";
    let result = compile_and_eval(source).unwrap();
    let x = find_int_value(&result, "x");
    // 1e18 as i64 = 1000000000000000000
    assert_eq!(x, 1_000_000_000_000_000_000i64);
}

#[test]
fn to_int_of_too_large_positive_float_should_error() {
    // 1e20 exceeds i64::MAX (~9.2e18). This should produce an error.
    let source = "node x: Int = to_int(1e20);";
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "x"),
        "to_int(1e20) should produce an error because 1e20 > i64::MAX"
    );
}

#[test]
fn to_int_of_too_large_negative_float_should_error() {
    // -1e20 is below i64::MIN (~-9.2e18). This should produce an error.
    let source = "node x: Int = to_int(-1e20);";
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "x"),
        "to_int(-1e20) should produce an error because -1e20 < i64::MIN"
    );
}

#[test]
fn to_int_of_value_just_above_max_should_error() {
    // 9.3e18 > i64::MAX (~9.2e18), should produce an error.
    let source = "node x: Int = to_int(9.3e18);";
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "x"),
        "to_int(9.3e18) should error because it exceeds i64 range"
    );
}

#[test]
fn to_int_of_value_just_below_min_should_error() {
    // -9.3e18 < i64::MIN (~-9.2e18), should produce an error.
    let source = "node x: Int = to_int(-9.3e18);";
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "x"),
        "to_int(-9.3e18) should error because it exceeds i64 range"
    );
}

// Property test: to_int should round-trip for values that fit in i64
proptest! {
    #[test]
    fn to_int_roundtrip_for_small_values(i in -1_000_000_000i64..1_000_000_000i64) {
        let f = i as f64;
        let source = format!("node x: Int = to_int({f:e});");
        let result = compile_and_eval(&source).unwrap();
        let x = find_int_value(&result, "x");
        prop_assert_eq!(x, i, "to_int({}) should be {} but got {}", f, i, x);
    }
}

// ============================================================================
// Coordinate-index construction and binary64 endpoint behavior
// ============================================================================

#[test]
fn range_index_rejects_step_that_misses_endpoint() {
    let source = r#"
pub index TimeIdx = range(0.0 s, 1.0 s, step: 0.3 s);
param x0: Dimensionless = 1.0;
node x: Dimensionless[TimeIdx] = for t: TimeIdx { @x0 };
"#;
    let error = compile_and_eval(source).unwrap_err();
    assert!(
        error.to_string().contains("does not land on endpoint"),
        "unexpected error: {error}"
    );
}

#[test]
fn range_rejects_tiny_endpoint_misses_relative_to_the_coordinate_scale() {
    compile_and_eval("index Tiny = range(0.0, 2e-16, step: 1e-16);")
        .expect("an exactly divisible tiny range should be valid");

    for source in [
        "index Tiny = range(0.0, 1e-16, step: 1.0);",
        "index Tiny = range(0.0, 1.5e-16, step: 1e-16);",
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains("does not land on endpoint"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn linspace_uses_exact_point_count_and_endpoints() {
    let source = r#"
pub index TimeIdx = linspace(0.0 s, 1.0 s, points: 4);
node x: Time[TimeIdx] = for t: TimeIdx { coord(t) };
"#;
    let result = compile_and_eval(source).unwrap();
    let x = find_entry(&result, "x");
    let Value::Indexed { entries, .. } = &x else {
        panic!("expected indexed value")
    };
    let values = entries
        .iter()
        .map(|(_, value)| match value {
            Value::Quantity { si_value, .. } => *si_value,
            other => panic!("expected quantity, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(values.len(), 4);
    assert_eq!(values[0].to_bits(), 0.0_f64.to_bits());
    assert_eq!(values[3].to_bits(), 1.0_f64.to_bits());
}

#[test]
fn range_preserves_declared_endpoint_after_binary64_interval_validation() {
    let source = r#"
index Tenths = range(0.0 s, 0.3 s, step: 0.1 s);
node coordinates: Time[Tenths] = for t: Tenths { coord(t) };
"#;
    let result = compile_and_eval(source).unwrap();
    let Value::Indexed { entries, .. } = find_entry(&result, "coordinates") else {
        panic!("expected indexed value")
    };
    let last = entries.last().expect("range is nonempty").1;
    let Value::Quantity { si_value, .. } = last else {
        panic!("expected quantity")
    };
    assert_eq!(si_value.to_bits(), 0.3_f64.to_bits());
}

#[test]
fn coordinate_key_values_retain_the_axis_display_unit() {
    let source = r#"
index Hour = range(0.0 h, 2.0 h, step: 1.0 h);
node signal: Time[Hour] = for t: Hour { coord(t) };
node peak: Key<Hour> = argmax(@signal);
node repeated: Key<Hour>[Fin(2)] = for i: Fin(2) { @peak };
"#;
    let result = compile_and_eval(source).unwrap();

    let peak = find_entry(&result, "peak");
    let Value::Quantity {
        si_value,
        display_unit: Some(display_unit),
        ..
    } = &peak
    else {
        panic!("expected a coordinate-shaped key with its axis display unit, got {peak:?}")
    };
    assert_eq!(si_value.to_bits(), 7200.0_f64.to_bits());
    assert_eq!(display_unit.label, "h");
    assert_eq!(display_unit.scale().to_bits(), 3600.0_f64.to_bits());
    assert_eq!(
        peak.format_display(Some(&result.base_dim_symbols)).unwrap(),
        "2 [h]"
    );

    let Value::Indexed { entries, .. } = find_entry(&result, "repeated") else {
        panic!("expected indexed coordinate keys")
    };
    assert_eq!(entries.len(), 2);
    assert!(entries.values().all(|entry| {
        entry
            .format_display(Some(&result.base_dim_symbols))
            .as_deref()
            == Ok("2 [h]")
    }));
}

#[test]
fn coordinate_display_names_remain_unique_when_rounded_labels_collide() {
    let source = r#"
index Tiny = range(1.0, 1.0000002, step: 0.0000001);
node values: Dimensionless[Tiny] = for t: Tiny { coord(t) };
"#;
    let Value::Indexed {
        entry_display_names: Some(display_names),
        ..
    } = find_entry(&compile_and_eval(source).unwrap(), "values")
    else {
        panic!("expected coordinate-indexed value with display names")
    };
    assert_eq!(display_names.len(), 3);
    assert_eq!(
        display_names
            .values()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
}

#[test]
fn descending_range_and_linspace_are_monotone_with_exact_endpoints() {
    let source = r#"
pub index ByStep = range(1.0 s, -1.0 s, step: -0.5 s);
pub index ByCount = linspace(1.0 s, -1.0 s, points: 5);
node stepped: Time[ByStep] = for t: ByStep { coord(t) };
node spaced: Time[ByCount] = for t: ByCount { coord(t) };
"#;
    let result = compile_and_eval(source).unwrap();
    for name in ["stepped", "spaced"] {
        let value = find_entry(&result, name);
        let Value::Indexed { entries, .. } = value else {
            panic!("expected indexed value")
        };
        let coordinates = entries
            .iter()
            .map(|(_, value)| match value {
                Value::Quantity { si_value, .. } => *si_value,
                other => panic!("expected quantity, got {other:?}"),
            })
            .collect::<Vec<_>>();
        let expected = [1.0_f64, 0.5, 0.0, -0.5, -1.0];
        assert_eq!(coordinates.len(), expected.len());
        assert!(
            coordinates
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual.to_bits() == expected.to_bits()),
            "unexpected coordinates: {coordinates:?}"
        );
    }
}

#[test]
fn coordinate_constructors_enforce_direction_and_count_rules() {
    for (source, expected) in [
        (
            "index I = range(0.0, 1.0, step: 0.0);",
            "step must be nonzero",
        ),
        (
            "index I = range(0.0, 1.0, step: -0.5);",
            "moves away from endpoint",
        ),
        (
            "index I = linspace(0.0, 1.0, points: 1);",
            "one point requires identical start and end",
        ),
        (
            "index I = linspace(0.0, 1.0, points: 0);",
            "at least one point",
        ),
        (
            "index I = linspace(0.0, 1.0, points: 1000001);",
            "exceeds the practical limit",
        ),
        (
            "index I = range(0.0, 1000000.0, step: 1.0);",
            "exceeds the practical limit",
        ),
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error for `{source}`: {error}"
        );
    }

    compile_and_eval("index I = linspace(2.0, 2.0, points: 1);").unwrap();
}

#[test]
fn coordinate_constructors_require_static_finite_quantities() {
    for source in [
        "param end: Dimensionless = 1.0; index I = range(0.0, @end, step: 0.1);",
        "param count: Int = 3; index I = linspace(0.0, 1.0, points: @count);",
        "index I = range(0, 2, step: 1);",
    ] {
        assert!(
            compile_and_eval(source).is_err(),
            "invalid coordinate constructor was accepted: {source}"
        );
    }
}

#[test]
fn range_index_step_count_exact_division() {
    // range(0.0 s, 1.0 s, step: 0.25 s) = 5 steps: 0.0, 0.25, 0.5, 0.75, 1.0
    let source = r#"
pub index TimeIdx = range(0.0 s, 1.0 s, step: 0.25 s);
param x0: Dimensionless = 1.0;
node x: Dimensionless[TimeIdx] = for t: TimeIdx { @x0 };
"#;
    let result = compile_and_eval(source).unwrap();
    let x = find_entry(&result, "x");
    match &x {
        Value::Indexed { entries, .. } => {
            assert_eq!(entries.len(), 5);
        }
        _ => panic!("expected indexed value"),
    }
}

#[test]
fn range_index_floating_point_step_accumulation() {
    // range(0.0 s, 1.0 s, step: 0.1 s) should give 11 steps
    // This is tricky because 0.1 is not exactly representable in f64
    // (1.0 - 0.0) / 0.1 = 9.999...96 or 10.000...04 depending on rounding
    let source = r#"
pub index TimeIdx = range(0.0 s, 1.0 s, step: 0.1 s);
param x0: Dimensionless = 1.0;
node x: Dimensionless[TimeIdx] = for t: TimeIdx { @x0 };
"#;
    let result = compile_and_eval(source).unwrap();
    let x = find_entry(&result, "x");
    match &x {
        Value::Indexed { entries, .. } => {
            assert_eq!(
                entries.len(),
                11,
                "range(0, 1, step: 0.1) should have 11 steps (0.0, 0.1, ..., 1.0) but has {}",
                entries.len()
            );
        }
        _ => panic!("expected indexed value"),
    }
}

#[test]
fn for_comp_returning_range_loop_variable_yields_quantities() {
    // Regression: the loop variable of a `for` over a range index is bound to
    // an internal CoordinateLabel value. Extracting its coordinate used to hit
    // `unreachable!("CoordinateLabel should not appear in final values")` when
    // converting the result to a public value. It must surface as a quantity.
    let source = r#"
index Step = range(0.0 s, 2.0 s, step: 1.0 s);
node t: Time[Step] = for i: Step { coord(i) };
"#;
    let result = compile_and_eval(source).unwrap();
    let t = find_entry(&result, "t");
    match &t {
        Value::Indexed { entries, .. } => {
            let values: Vec<f64> = entries
                .iter()
                .map(|(_, v)| match v {
                    Value::Quantity { si_value, .. } => *si_value,
                    other => panic!("expected quantity entry, got {other:?}"),
                })
                .collect();
            assert_eq!(values, vec![0.0, 1.0, 2.0]);
        }
        _ => panic!("expected indexed value"),
    }
}

// ============================================================================
// BUG 3: Quantity power edge cases
//
// Exact integer/rational exponent metadata must reach evaluation so negative
// bases use integer powers and real odd roots instead of a rounded `powf`
// exponent.
// ============================================================================

#[test]
fn power_zero_to_zero_is_one() {
    // IEEE 754 defines 0.0^0.0 = 1.0. Verify graphcal follows this.
    let source = "node x: Dimensionless = 0.0 ^ 0.0;";
    let result = compile_and_eval(source).unwrap();
    // This should either be 1.0 or an error, not NaN
    if !has_node_error(&result, "x") {
        let x = find_value(&result, "x");
        assert!(
            (x - 1.0).abs() < f64::EPSILON,
            "0.0 ^ 0.0 should be 1.0 but got {x}"
        );
    }
    // If it errors, that's also a valid design choice
}

#[test]
fn negative_base_integer_exponent_should_work() {
    let source = "node x: Dimensionless = (-2.0) ^ 3;";
    let result = compile_and_eval(source).unwrap();
    assert!(
        !has_node_error(&result, "x"),
        "(-2.0) ^ 3 should evaluate to -8.0, not produce an error. \
         Error: {:?}",
        get_node_error_message(&result, "x")
    );
    if !has_node_error(&result, "x") {
        let x = find_value(&result, "x");
        assert!(
            (x - (-8.0)).abs() < f64::EPSILON,
            "(-2.0) ^ 3 should be -8.0 but got {x}"
        );
    }
}

#[test]
fn negative_base_even_integer_exponent_should_work() {
    let source = "node x: Dimensionless = (-3.0) ^ 2;";
    let result = compile_and_eval(source).unwrap();
    assert!(
        !has_node_error(&result, "x"),
        "(-3.0) ^ 2 should evaluate to 9.0, not produce an error. \
         Error: {:?}",
        get_node_error_message(&result, "x")
    );
    if !has_node_error(&result, "x") {
        let x = find_value(&result, "x");
        assert!(
            (x - 9.0).abs() < f64::EPSILON,
            "(-3.0) ^ 2 should be 9.0 but got {x}"
        );
    }
}

#[test]
fn negative_base_fractional_exponent_should_error() {
    let source = "node x: Dimensionless = (-2.0) ^ (1/2);";
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "x"),
        "(-2.0) ^ (1/2) should produce an error (imaginary result)"
    );
}

#[test]
fn negative_base_exact_odd_roots_are_real() {
    let source = "\
node cube_root: Dimensionless = (-8.0) ^ (1/3);
node two_fifths: Dimensionless = (-32.0) ^ (2/5);
node reciprocal_cube_root: Dimensionless = (-8.0) ^ (-1/3);";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "cube_root") + 2.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "two_fifths") - 4.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "reciprocal_cube_root") + 0.5).abs() < f64::EPSILON);
}

// ============================================================================
// BUG 4: Display value for very small scale factors
//
// display_value = si_value / scale. If scale is very small (e.g., a
// micro-unit), the display value can overflow to infinity.
// ============================================================================

#[test]
fn display_value_with_very_large_conversion() {
    // Convert a very large SI value to a very small unit
    // This is an extreme case but could happen with unit mismatches
    let source = r#"
param x: Length = 1e300 m;
node y: Length = @x -> m;
"#;
    let result = compile_and_eval(source).unwrap();
    let y = find_entry(&result, "y");
    match &y {
        Value::Quantity {
            si_value,
            display_unit,
            ..
        } => {
            assert!(si_value.is_finite(), "SI value should be finite");
            if let Some(du) = display_unit {
                let display = si_value / du.scale();
                assert!(
                    display.is_finite(),
                    "display value should be finite: {si_value} / {} = {display}",
                    du.scale()
                );
            }
        }
        _ => panic!("expected quantity"),
    }
}

// ============================================================================
// BUG 5: Integer arithmetic edge cases
//
// Test i64 boundary values to verify checked arithmetic catches all overflows.
// ============================================================================

#[test]
fn int_max_plus_one_overflows() {
    let source = &format!("param x: Int = {};\nnode y: Int = @x + 1;", i64::MAX);
    let result = compile_and_eval(source).unwrap();
    assert!(has_node_error(&result, "y"), "i64::MAX + 1 should overflow");
}

#[test]
fn int_min_minus_one_overflows() {
    // i64::MIN is -9223372036854775808, but the parser might not handle it as
    // a literal. Let's use a computed approach.
    let source = &format!(
        "param x: Int = {};\nnode y: Int = @x - 1;",
        i64::MIN + 1 // -9223372036854775807
    );
    let result = compile_and_eval(source).unwrap();
    let y = find_int_value(&result, "y");
    assert_eq!(y, i64::MIN, "MIN+1 - 1 = MIN");

    // Now test actual overflow
    let source2 = &format!("param x: Int = {};\nnode y: Int = @x - 1;", i64::MIN + 1);
    let result2 = compile_and_eval(source2).unwrap();
    // This should be fine: (MIN+1) - 1 = MIN
    assert!(!has_node_error(&result2, "y"));

    // The real overflow test: -9223372036854775807 - 2 should overflow
    let source3 = &format!("param x: Int = {};\nnode y: Int = @x - 2;", i64::MIN + 1);
    let result3 = compile_and_eval(source3).unwrap();
    assert!(
        has_node_error(&result3, "y"),
        "i64::MIN+1 - 2 should overflow"
    );
}

#[test]
fn int_multiplication_overflow() {
    let source = &format!(
        "param x: Int = {};\nnode y: Int = @x * 2;",
        i64::MAX / 2 + 1
    );
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "y"),
        "(MAX/2 + 1) * 2 should overflow"
    );
}

#[test]
fn int_negation_of_min_overflows() {
    // -i64::MIN overflows because |i64::MIN| > i64::MAX
    // i64::MIN = -9223372036854775808, but we can't write that as a literal
    // directly. Use (MIN+1) - 1 approach via subtraction.
    let source = &format!(
        "param x: Int = {};\nnode y: Int = @x - 1;\nnode z: Int = -@y;",
        i64::MIN + 1
    );
    let result = compile_and_eval(source).unwrap();
    // y = MIN, z = -MIN should overflow
    assert!(
        has_node_error(&result, "z"),
        "negation of i64::MIN should overflow"
    );
}

// ============================================================================
// BUG 6: Floating point comparison with exact equality
//
// The DSL uses exact f64 equality (==, !=). For engineering users, this is
// dangerous because computed values rarely equal expected values exactly.
// While this may be a design choice, we should at least verify the behavior
// is consistent and documented.
// ============================================================================

#[test]
fn float_equality_is_exact() {
    // 0.1 + 0.2 != 0.3 in IEEE 754
    let source = r#"
node x: Dimensionless = 0.1 + 0.2;
node eq: Bool = @x == 0.3;
"#;
    let result = compile_and_eval(source).unwrap();
    let val = result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == "eq")
        .unwrap()
        .1
        .as_ref()
        .unwrap();
    match val {
        Value::Bool(b) => {
            // This test documents the behavior: 0.1 + 0.2 != 0.3 exactly
            // For engineering users, this is a footgun
            assert!(
                !b,
                "0.1 + 0.2 == 0.3 should be false due to floating point representation. \
                 If this passes, the language has approximate comparison, which would be good."
            );
        }
        _ => panic!("expected Bool"),
    }
}

// ============================================================================
// BUG 7: Division of -0.0
//
// IEEE 754 has -0.0. Does the division check `r == 0.0` catch -0.0?
// In Rust, `-0.0 == 0.0` is true, so the check should work. But the
// result of `1.0 / -0.0` without the check would be -infinity.
// ============================================================================

#[test]
fn division_by_negative_zero() {
    // -0.0 should be caught as division by zero
    // We can produce -0.0 as: -1.0 * 0.0
    let source = r#"
param neg_zero: Dimensionless = -1.0 * 0.0;
node x: Dimensionless = 1.0 / @neg_zero;
"#;
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "x"),
        "1.0 / (-0.0) should be caught as division by zero"
    );
}

// ============================================================================
// BUG 8: to_float precision loss for large integers
//
// to_float uses `i as f64` which loses precision for i64 values > 2^53.
// For engineering, this could silently corrupt large integer values.
// ============================================================================

#[test]
fn to_float_large_integer_precision() {
    // 2^53 = 9007199254740992 is the last integer exactly representable as f64
    // 2^53 + 1 = 9007199254740993 should lose precision
    let val = (1i64 << 53) + 1;
    let source = format!("param x: Int = {val};\nnode y: Dimensionless = to_float(@x);");
    let result = compile_and_eval(&source).unwrap();
    let y = find_value(&result, "y");
    // BUG: y will be 9007199254740992.0, not 9007199254740993.0
    // The precision loss is silent — no error, no warning
    let expected = val as f64; // This itself loses precision
    assert!(
        (y - expected).abs() < f64::EPSILON,
        "to_float({val}) = {y}, expected {expected}. Note: both lose precision!"
    );
    // The real test: does the language warn about or prevent this?
    // Currently it doesn't, which is a design concern for safety-critical code.
}

// ============================================================================
// BUG 9: Aggregation functions on non-finite collections/results
//
// Aggregation functions (sum, min, max, mean) should reject non-finite
// elements or results instead of silently producing NaN/inf.
// ============================================================================

#[test]
fn sum_of_indexed_values() {
    let source = r#"
pub index Maneuver = { Alpha, Beta, Charlie };
param x: Dimensionless[Maneuver] = {
    Maneuver#Alpha: 1.0,
    Maneuver#Beta: 2.0,
    Maneuver#Charlie: 3.0,
};
node total: Dimensionless = sum(@x);
"#;
    let result = compile_and_eval(source).unwrap();
    let total = find_value(&result, "total");
    assert!(
        (total - 6.0).abs() < f64::EPSILON,
        "sum(1, 2, 3) = {total}, expected 6.0"
    );
}

#[test]
fn mean_of_indexed_values() {
    let source = r#"
pub index Maneuver = { Alpha, Beta, Charlie };
param x: Dimensionless[Maneuver] = {
    Maneuver#Alpha: 1.0,
    Maneuver#Beta: 2.0,
    Maneuver#Charlie: 3.0,
};
node avg: Dimensionless = mean(@x);
"#;
    let result = compile_and_eval(source).unwrap();
    let avg = find_value(&result, "avg");
    assert!(
        (avg - 2.0).abs() < f64::EPSILON,
        "mean(1, 2, 3) = {avg}, expected 2.0"
    );
}

#[test]
fn sum_of_indexed_values_that_overflows_errors() {
    let source = r#"
pub index Maneuver = { Alpha, Beta };
param x: Dimensionless[Maneuver] = {
    Maneuver#Alpha: 1e308,
    Maneuver#Beta: 1e308,
};
node total: Dimensionless = sum(@x);
"#;
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "total"),
        "sum producing infinity should be reported as a node error: {result:?}"
    );
}

// ============================================================================
// BUG 10: Unit scale factor computation with compound units
//
// Test that compound unit scale factors are computed correctly,
// especially for units with powers.
// ============================================================================

#[test]
fn unit_scale_km_squared() {
    // 1 km^2 = 1e6 m^2
    // A value of 5.0 km^2 should be 5e6 m^2 internally
    let source = r#"
const unit km2: Area = 1e6 m^2;
param x: Area = 5.0 km2;
"#;
    let result = compile_and_eval(source).unwrap();
    let x = find_value(&result, "x");
    assert!((x - 5e6).abs() < 1.0, "5 km^2 should be 5e6 m^2, got {x}");
}

#[test]
fn compound_unit_conversion_km_per_h_squared() {
    // km/h in SI is 1000/3600 m/s
    // (km/h)^2 in SI would be (1000/3600)^2 m^2/s^2
    // But graphcal doesn't support squared compound unit expressions directly.
    // Test basic compound units instead.
    let source = r#"
param v: Velocity = 36.0 km/h;
node v_si: Velocity = @v;
"#;
    let result = compile_and_eval(source).unwrap();
    let v = find_value(&result, "v_si");
    // 36 km/h = 36 * 1000/3600 m/s = 10 m/s
    assert!(
        (v - 10.0).abs() < 1e-10,
        "36 km/h should be 10 m/s, got {v}"
    );
}

// ============================================================================
// Property-based tests: stress-test arithmetic with random values
// ============================================================================

proptest! {
    /// Integer addition should be commutative
    #[test]
    fn int_add_commutative(a in -1000i64..1000, b in -1000i64..1000) {
        let source = format!(
            "param a: Int = {a};\nparam b: Int = {b};\n\
             node lhs: Int = @a + @b;\nnode rhs: Int = @b + @a;"
        );
        let result = compile_and_eval(&source).unwrap();
        let lhs = find_int_value(&result, "lhs");
        let rhs = find_int_value(&result, "rhs");
        prop_assert_eq!(lhs, rhs);
    }

    /// Integer multiplication should be commutative
    #[test]
    fn int_mul_commutative(a in -1000i64..1000, b in -1000i64..1000) {
        let source = format!(
            "param a: Int = {a};\nparam b: Int = {b};\n\
             node lhs: Int = @a * @b;\nnode rhs: Int = @b * @a;"
        );
        let result = compile_and_eval(&source).unwrap();
        let lhs = find_int_value(&result, "lhs");
        let rhs = find_int_value(&result, "rhs");
        prop_assert_eq!(lhs, rhs);
    }

    /// Quantity addition should be commutative
    #[test]
    fn float_add_commutative(
        a in proptest::num::f64::NORMAL,
        b in proptest::num::f64::NORMAL,
    ) {
        prop_assume!(a.is_finite() && b.is_finite());
        let source = format!(
            "param a: Dimensionless = {a:e};\nparam b: Dimensionless = {b:e};\n\
             node lhs: Dimensionless = @a + @b;\nnode rhs: Dimensionless = @b + @a;"
        );
        let result = compile_and_eval(&source).unwrap();
        if !has_node_error(&result, "lhs") && !has_node_error(&result, "rhs") {
            let lhs = find_value(&result, "lhs");
            let rhs = find_value(&result, "rhs");
            prop_assert_eq!(lhs.to_bits(), rhs.to_bits());
        }
    }

    /// Quantity multiplication should be commutative
    #[test]
    fn float_mul_commutative(
        a in proptest::num::f64::NORMAL,
        b in proptest::num::f64::NORMAL,
    ) {
        prop_assume!(a.is_finite() && b.is_finite());
        let source = format!(
            "param a: Dimensionless = {a:e};\nparam b: Dimensionless = {b:e};\n\
             node lhs: Dimensionless = @a * @b;\nnode rhs: Dimensionless = @b * @a;"
        );
        let result = compile_and_eval(&source).unwrap();
        if !has_node_error(&result, "lhs") && !has_node_error(&result, "rhs") {
            let lhs = find_value(&result, "lhs");
            let rhs = find_value(&result, "rhs");
            prop_assert_eq!(lhs.to_bits(), rhs.to_bits());
        }
    }

    /// Integer division truncates toward zero (Rust semantics)
    #[test]
    fn int_division_truncates_toward_zero(a in -1000i64..1000, b in 1i64..1000) {
        let source = format!(
            "param a: Int = {a};\nparam b: Int = {b};\nnode q: Int = @a / @b;"
        );
        let result = compile_and_eval(&source).unwrap();
        let q = find_int_value(&result, "q");
        let expected = a / b; // Rust truncates toward zero
        prop_assert_eq!(q, expected);
    }

    /// Integer modulo satisfies: a == (a / b) * b + (a % b)
    #[test]
    fn int_euclidean_identity(a in -1000i64..1000, b in 1i64..1000) {
        let source = format!(
            "param a: Int = {a};\nparam b: Int = {b};\n\
             node q: Int = @a / @b;\nnode r: Int = @a % @b;"
        );
        let result = compile_and_eval(&source).unwrap();
        let q = find_int_value(&result, "q");
        let r = find_int_value(&result, "r");
        prop_assert_eq!(a, q * b + r);
    }

    /// Unit conversion round-trip: x km -> m -> km should be identity
    #[test]
    fn unit_roundtrip_km(v in 0.001f64..1e10) {
        let source = format!(
            "param x: Length = {v:e} km;\n\
             node y: Length = @x -> km;"
        );
        let result = compile_and_eval(&source).unwrap();
        // SI value of y should equal SI value of x
        let x_si = find_value(&result, "x");
        let y_si = find_value(&result, "y");
        let rel_err = if x_si.abs() > 0.0 { ((y_si - x_si) / x_si).abs() } else { (y_si - x_si).abs() };
        prop_assert!(rel_err < 1e-10,
            "round-trip conversion should preserve value: x_si={x_si}, y_si={y_si}, rel_err={rel_err}");
    }
}

/// `to_int` should error for values outside i64 range (proptest version)
#[test]
fn to_int_rejects_out_of_range() {
    use proptest::test_runner::{Config, TestRunner};

    let mut runner = TestRunner::new(Config::default());
    runner
        .run(
            &prop_oneof![(9.3e18f64..1e300f64), (-1e300f64..-9.3e18f64),],
            |f| {
                let source = format!("node x: Int = to_int({f:e});");
                let result = compile_and_eval(&source).unwrap();
                prop_assert!(
                    has_node_error(&result, "x"),
                    "to_int({:e}) should error because value is outside i64 range",
                    f,
                );
                Ok(())
            },
        )
        .unwrap();
}

// ============================================================================
// BUG 5 (reported): Trailing commas in function call arguments
//
// Trailing commas ARE allowed in index map literals and struct literals,
// but were rejected in function call arguments. This is now fixed.
// ============================================================================

#[test]
fn fn_call_trailing_comma() {
    let source = r#"
node result: Dimensionless = greatest(1.0, 2.0,);
"#;
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!(
        (val - 2.0).abs() < f64::EPSILON,
        "greatest(1.0, 2.0,) should be 2.0 but got {val}"
    );
}

#[test]
fn fn_call_single_arg_trailing_comma() {
    let source = r#"
node result: Dimensionless = abs(-42.0,);
"#;
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!(
        (val - 42.0).abs() < f64::EPSILON,
        "abs(-42.0,) should be 42.0 but got {val}"
    );
}

// ============================================================================
// BUG 9 (reported): Multi-dimensional indexed assertions
//
// `assert` only accepted `Bool` or `Bool[SingleIndex]`. Now it accepts
// arbitrarily nested `Bool[I1][I2]...`.
// ============================================================================

#[test]
fn assert_multi_dimensional_indexed() {
    let source = r#"
pub index Row = { RowA, RowB };
pub index Col = { ColA, ColB };

param val: Dimensionless[Row, Col] = {
    (Row#RowA, Col#ColA): 5.0, (Row#RowA, Col#ColB): 3.0,
    (Row#RowB, Col#ColA): 7.0, (Row#RowB, Col#ColB): 1.0,
};

assert all_positive = for r: Row, c: Col {
    @val[r, c] > 0.0
};
"#;
    let result = compile_and_eval(source).unwrap();
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, r, _)| matches!(r, graphcal_eval::eval::AssertResult::Pass)),
        "all values are positive, assertion should pass"
    );
}

#[test]
fn assert_three_dimensional_indexed() {
    let source = r#"
pub index Layer = { Layer1, Layer2 };
pub index Band = { Band1, Band2 };
pub index Channel = { Ch1, Ch2 };

param val: Dimensionless[Layer, Band, Channel] = {
    (Layer#Layer1, Band#Band1, Channel#Ch1): 1.0, (Layer#Layer1, Band#Band1, Channel#Ch2): 2.0,
    (Layer#Layer1, Band#Band2, Channel#Ch1): 3.0, (Layer#Layer1, Band#Band2, Channel#Ch2): 4.0,
    (Layer#Layer2, Band#Band1, Channel#Ch1): 5.0, (Layer#Layer2, Band#Band1, Channel#Ch2): 6.0,
    (Layer#Layer2, Band#Band2, Channel#Ch1): 7.0, (Layer#Layer2, Band#Band2, Channel#Ch2): 8.0,
};

assert all_positive = for l: Layer, b: Band, c: Channel {
    @val[l, b, c] > 0.0
};
"#;
    let result = compile_and_eval(source).unwrap();
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, r, _)| matches!(r, graphcal_eval::eval::AssertResult::Pass)),
        "all values are positive, 3D assertion should pass"
    );
}

// ============================================================================
// #648 B1/N5: display metadata follows value reads.
//
// A value converted at its construction site must render the same way at
// every access site: dag projections, struct field reads, index entry reads,
// const reads, and runtime-selected if/match branches.
// ============================================================================

/// Extract the display unit label of a quantity declaration, if any.
fn display_label(result: &EvalResult, name: &str) -> Option<String> {
    match find_entry(result, name) {
        Value::Quantity { display_unit, .. } => display_unit.map(|du| du.label),
        other => panic!("expected quantity for `{name}`, got {other:?}"),
    }
}

fn assert_quantity_leaves_use_display_unit(
    value: &Value,
    expected_si_value: f64,
    expected_display_label: &str,
    context: &str,
) -> usize {
    match value {
        Value::Quantity {
            si_value,
            display_unit,
            ..
        } => {
            assert_eq!(
                si_value.to_bits(),
                expected_si_value.to_bits(),
                "quantity leaf in `{context}` must retain its SI value"
            );
            assert_eq!(
                display_unit.as_ref().map(|unit| unit.label.as_str()),
                Some(expected_display_label),
                "quantity leaf in `{context}` must retain its display unit"
            );
            1
        }
        Value::Indexed { entries, .. } => entries
            .values()
            .map(|entry| {
                assert_quantity_leaves_use_display_unit(
                    entry,
                    expected_si_value,
                    expected_display_label,
                    context,
                )
            })
            .sum(),
        other => panic!("expected quantity or indexed value for `{context}`, got {other:?}"),
    }
}

#[test]
fn display_unit_propagates_through_reads() {
    let source = include_str!("../../../tests/fixtures/valid/convert_display_propagation.gcl");
    let result = compile_and_eval(source).unwrap();

    for name in [
        "projected",
        "field_read",
        "entry_read",
        "const_read",
        "branch_read",
        "arm_read",
    ] {
        assert_eq!(
            display_label(&result, name).as_deref(),
            Some("km"),
            "`{name}` must render in the unit written at its construction site"
        );
    }

    // Timezone display follows reads the same way.
    match find_entry(&result, "tz_read") {
        Value::Datetime { display_tz, .. } => {
            assert_eq!(
                display_tz.as_ref().map(IanaTimeZoneId::as_str),
                Some("Asia/Tokyo")
            );
        }
        other => panic!("expected datetime, got {other:?}"),
    }

    // Whole-map reads keep per-entry displays.
    match find_entry(&result, "whole_read") {
        Value::Indexed { entries, .. } => {
            for (variant, entry) in &entries {
                match entry {
                    Value::Quantity { display_unit, .. } => assert_eq!(
                        display_unit.as_ref().map(|du| du.label.as_str()),
                        Some("km"),
                        "entry `{variant}` must keep its display unit"
                    ),
                    other => panic!("expected quantity entry, got {other:?}"),
                }
            }
        }
        other => panic!("expected indexed, got {other:?}"),
    }
}

#[test]
fn display_unit_is_preserved_in_multi_axis_and_nested_for_bodies() {
    let source = r#"
index A = { X, Y };
index B = { P, Q };

node single: Length[A] = for a: A {
    1500.0 m -> km
};
node multi: Length[A, B] = for a: A, b: B {
    1500.0 m -> km
};
node nested: Length[A, B] = for a: A {
    for b: B {
        1500.0 m -> km
    }
};
node wrapped: Length[A, B] = (for a: A, b: B {
    1500.0 m
}) -> km;
"#;
    let result = compile_and_eval(source).unwrap();

    for (name, expected_leaf_count) in [("single", 2), ("multi", 4), ("nested", 4), ("wrapped", 4)]
    {
        let value = find_entry(&result, name);
        assert_eq!(
            assert_quantity_leaves_use_display_unit(&value, 1500.0, "km", name),
            expected_leaf_count,
            "`{name}` must contain the expected number of quantity leaves"
        );
    }
}

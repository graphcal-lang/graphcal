//! Property-based and edge-case tests to find critical bugs in graphcal.
//!
//! These tests focus on areas that matter most to engineering users:
//! - Correctness of numeric evaluation (arithmetic, conversions)
//! - Display/formatting of output values
//! - Range index step count precision
//! - Unit conversion accuracy
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(
    clippy::cast_precision_loss,
    reason = "test code intentionally tests precision edge cases"
)]
#![allow(
    clippy::needless_raw_string_hashes,
    reason = "raw strings used for readability of graphcal source"
)]

use graphcal_eval::eval::{EvalResult, NodeError, Value, compile_and_eval};
use proptest::prelude::*;

// ============================================================================
// Helpers
// ============================================================================

/// Find the SI value of a named scalar declaration.
fn find_value(result: &EvalResult, name: &str) -> f64 {
    if let Some((_, val)) = result.consts.iter().find(|(n, _)| n.as_str() == name) {
        return val.si_value();
    }
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
}

/// Find the Int value of a named declaration.
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

/// Find a named entry as a Value.
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

/// Check if a named node has an error.
fn has_node_error(result: &EvalResult, name: &str) -> bool {
    result
        .all
        .iter()
        .find(|(n, _, _)| n.as_str() == name)
        .is_some_and(|(_, r, _)| r.is_err())
}

/// Get the node error message for a named node.
fn get_node_error_message(result: &EvalResult, name: &str) -> Option<String> {
    result
        .all
        .iter()
        .find(|(n, _, _)| n.as_str() == name)
        .and_then(|(_, r, _)| match r {
            Err(NodeError::EvalFailed { message }) => Some(message.clone()),
            _ => None,
        })
}

// ============================================================================
// to_int() range checking (#85, fixed)
//
// `to_int(x)` now rejects values outside i64 range with a clear error,
// instead of silently saturating via `f as i64`.
// ============================================================================

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
// BUG 2: format_step_number for range index labels
//
// `format_step_number` uses `value as i64` for values where
// `value.fract() == 0.0 && value.abs() < 1e15`. However:
// - Negative zero: (-0.0).fract() == 0.0, and (-0.0 as i64) == 0, which is
//   technically correct but inconsistent (displays as "0" not "-0")
// - Values between 1e15 and i64::MAX: the condition `value.abs() < 1e15`
//   prevents overflow, but the boundary is conservative
// - NaN: NaN.fract() returns NaN, which is != 0.0, so it falls through to
//   the float format path — but NaN shouldn't reach this function normally
// ============================================================================

// These are tested indirectly through range index display. The format_step_number
// function is private, so we test it through the public API.

#[test]
fn range_index_step_count_precision() {
    // A range where (end - start) / step doesn't divide evenly in floating point
    // range(0.0 s, 1.0 s, step: 0.3 s) should give steps at 0.0, 0.3, 0.6, 0.9
    // That's (1.0 - 0.0) / 0.3 = 3.333... → round to 3, + 1 = 4 steps
    let source = r#"
index TimeIdx = range(0.0 s, 1.0 s, step: 0.3 s);
param x0: Dimensionless = 1.0;
node x: Dimensionless[TimeIdx] = for t: TimeIdx { @x0 };
"#;
    let result = compile_and_eval(source).unwrap();
    let x = find_entry(&result, "x");
    match &x {
        Value::Indexed { entries, .. } => {
            // Expected: 4 entries (0.0, 0.3, 0.6, 0.9)
            assert_eq!(
                entries.len(),
                4,
                "range(0, 1, step: 0.3) should have 4 steps but has {}",
                entries.len()
            );
        }
        _ => panic!("expected indexed value"),
    }
}

#[test]
fn range_index_step_count_exact_division() {
    // range(0.0 s, 1.0 s, step: 0.25 s) = 5 steps: 0.0, 0.25, 0.5, 0.75, 1.0
    let source = r#"
index TimeIdx = range(0.0 s, 1.0 s, step: 0.25 s);
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
index TimeIdx = range(0.0 s, 1.0 s, step: 0.1 s);
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

// ============================================================================
// BUG 3: Float power edge cases
//
// The evaluator uses `l.powf(r)` for float power, with a post-check for
// finite results. But some edge cases slip through:
// - 0.0 ^ 0.0: mathematically debatable, IEEE 754 says 1.0, but for
//   engineering this may be surprising
// - (-2.0) ^ 3.0: this should be -8.0, but powf(-2.0, 3.0) returns NaN
//   because powf uses exp(3.0 * ln(-2.0)) which involves ln of negative
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
    // (-2.0) ^ 3.0 should be -8.0 in engineering contexts.
    // But f64::powf(-2.0, 3.0) returns NaN because it uses exp(3*ln(-2)).
    // This is a critical bug for engineering: users expect integer-like
    // exponents on negative bases to work correctly.
    let source = "node x: Dimensionless = (-2.0) ^ 3.0;";
    let result = compile_and_eval(source).unwrap();
    // BUG: powf(-2.0, 3.0) = NaN, which is caught as an error.
    // But the user expects -8.0 since 3.0 is effectively an integer exponent.
    assert!(
        !has_node_error(&result, "x"),
        "(-2.0) ^ 3.0 should evaluate to -8.0, not produce an error. \
         Error: {:?}",
        get_node_error_message(&result, "x")
    );
    if !has_node_error(&result, "x") {
        let x = find_value(&result, "x");
        assert!(
            (x - (-8.0)).abs() < f64::EPSILON,
            "(-2.0) ^ 3.0 should be -8.0 but got {x}"
        );
    }
}

#[test]
fn negative_base_even_integer_exponent_should_work() {
    // (-3.0) ^ 2.0 should be 9.0
    // powf(-3.0, 2.0) = NaN on most platforms
    let source = "node x: Dimensionless = (-3.0) ^ 2.0;";
    let result = compile_and_eval(source).unwrap();
    // BUG: Same as above - powf returns NaN for negative bases
    assert!(
        !has_node_error(&result, "x"),
        "(-3.0) ^ 2.0 should evaluate to 9.0, not produce an error. \
         Error: {:?}",
        get_node_error_message(&result, "x")
    );
    if !has_node_error(&result, "x") {
        let x = find_value(&result, "x");
        assert!(
            (x - 9.0).abs() < f64::EPSILON,
            "(-3.0) ^ 2.0 should be 9.0 but got {x}"
        );
    }
}

#[test]
fn negative_base_fractional_exponent_should_error() {
    // (-2.0) ^ 0.5 should definitely error (imaginary result)
    let source = "node x: Dimensionless = (-2.0) ^ 0.5;";
    let result = compile_and_eval(source).unwrap();
    assert!(
        has_node_error(&result, "x"),
        "(-2.0) ^ 0.5 should produce an error (imaginary result)"
    );
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
        Value::Scalar {
            si_value,
            display_unit,
            ..
        } => {
            assert!(si_value.is_finite(), "SI value should be finite");
            if let Some(du) = display_unit {
                let display = si_value / du.scale;
                assert!(
                    display.is_finite(),
                    "display value should be finite: {si_value} / {} = {display}",
                    du.scale
                );
            }
        }
        _ => panic!("expected scalar"),
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
        .find(|(n, _, _)| n.as_str() == "eq")
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
// BUG 9: Aggregation functions on NaN-containing collections
//
// If one entry in an indexed collection somehow has a special value,
// aggregation functions (sum, min, max, mean) might produce unexpected results.
// NaN should propagate correctly.
// ============================================================================

#[test]
fn sum_of_indexed_values() {
    let source = r#"
index Maneuver = { Alpha, Beta, Charlie }
param x: Dimensionless[Maneuver] = {
    Maneuver::Alpha: 1.0,
    Maneuver::Beta: 2.0,
    Maneuver::Charlie: 3.0,
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
index Maneuver = { Alpha, Beta, Charlie }
param x: Dimensionless[Maneuver] = {
    Maneuver::Alpha: 1.0,
    Maneuver::Beta: 2.0,
    Maneuver::Charlie: 3.0,
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
dimension Area = Length^2;
unit km2: Area = 1e6 m^2;
param x: Area = 5.0 km2;
"#;
    let result = compile_and_eval(source).unwrap();
    let x = find_value(&result, "x");
    assert!((x - 5e6).abs() < 1.0, "5 km^2 should be 5e6 m^2, got {x}");
}

#[test]
fn compound_unit_conversion_km_per_hour_squared() {
    // km/hour in SI is 1000/3600 m/s
    // (km/hour)^2 in SI would be (1000/3600)^2 m^2/s^2
    // But graphcal doesn't support squared compound unit expressions directly.
    // Test basic compound units instead.
    let source = r#"
param v: Velocity = 36.0 km/hour;
node v_si: Velocity = @v;
"#;
    let result = compile_and_eval(source).unwrap();
    let v = find_value(&result, "v_si");
    // 36 km/hour = 36 * 1000/3600 m/s = 10 m/s
    assert!(
        (v - 10.0).abs() < 1e-10,
        "36 km/hour should be 10 m/s, got {v}"
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

    /// Float addition should be commutative
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

    /// Float multiplication should be commutative
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
fn add(a: Dimensionless, b: Dimensionless) -> Dimensionless = a + b;
node result: Dimensionless = add(1.0, 2.0,);
"#;
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!(
        (val - 3.0).abs() < f64::EPSILON,
        "add(1.0, 2.0,) should be 3.0 but got {val}"
    );
}

#[test]
fn fn_call_single_arg_trailing_comma() {
    let source = r#"
fn identity(x: Dimensionless) -> Dimensionless = x;
node result: Dimensionless = identity(42.0,);
"#;
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!(
        (val - 42.0).abs() < f64::EPSILON,
        "identity(42.0,) should be 42.0 but got {val}"
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
index Row = { RowA, RowB }
index Col = { ColA, ColB }

param val: Dimensionless[Row, Col] = {
    (Row::RowA, Col::ColA): 5.0, (Row::RowA, Col::ColB): 3.0,
    (Row::RowB, Col::ColA): 7.0, (Row::RowB, Col::ColB): 1.0,
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
index Layer = { Layer1, Layer2 }
index Band = { Band1, Band2 }
index Channel = { Ch1, Ch2 }

param val: Dimensionless[Layer, Band, Channel] = {
    (Layer::Layer1, Band::Band1, Channel::Ch1): 1.0, (Layer::Layer1, Band::Band1, Channel::Ch2): 2.0,
    (Layer::Layer1, Band::Band2, Channel::Ch1): 3.0, (Layer::Layer1, Band::Band2, Channel::Ch2): 4.0,
    (Layer::Layer2, Band::Band1, Channel::Ch1): 5.0, (Layer::Layer2, Band::Band1, Channel::Ch2): 6.0,
    (Layer::Layer2, Band::Band2, Channel::Ch1): 7.0, (Layer::Layer2, Band::Band2, Channel::Ch2): 8.0,
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

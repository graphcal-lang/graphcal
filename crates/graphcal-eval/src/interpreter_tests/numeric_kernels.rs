//! Regression tests for the numeric kernels: integer powers, residual
//! checks, range-safe products, reductions, and loud underflow.

use graphcal_compiler::node_unavailable::NodeUnavailable;

use super::{compile_and_eval, find_bool_value, find_value};
use crate::eval::EvalEntry;

/// Assert that `name` failed to evaluate with a message containing `needle`.
fn assert_eval_failed(source: &str, name: &str, needle: &str) {
    let result = compile_and_eval(source).unwrap();
    let EvalEntry { outcome, .. } = result
        .entries
        .iter()
        .find(|EvalEntry { name: n, .. }| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"));
    match outcome {
        Err(NodeUnavailable::EvalFailed { message }) => assert!(
            message.contains(needle),
            "expected an error containing {needle:?}, got {message:?}"
        ),
        other => panic!("expected `{name}` to fail with {needle:?}, got {other:?}"),
    }
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "the reference is deliberately `powf`, the kernel every power uses"
)]
fn integer_power_does_not_depend_on_how_the_exponent_is_written() {
    let result = compile_and_eval(
        r"
param x: Dimensionless = 0.7015463661686019;
param n: Int = 3;
node lit: Dimensionless = @x ^ 3;
node via_int: Dimensionless = @x ^ @n;
node same: Bool = @x ^ 3 == @x ^ @n;
",
    )
    .unwrap();
    assert!(find_bool_value(&result, "same"));
    assert_eq!(
        find_value(&result, "lit").to_bits(),
        0.701_546_366_168_601_9_f64.powf(3.0).to_bits()
    );
    assert_eq!(
        find_value(&result, "lit").to_bits(),
        find_value(&result, "via_int").to_bits()
    );
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "the reference is deliberately `powf`, the kernel every power uses"
)]
fn integer_literal_powers_do_not_underflow_falsely() {
    let result = compile_and_eval(
        r"
node subnormal: Dimensionless = 2.0 ^ -1070;
node tiny: Dimensionless = 10.0 ^ -310;
node inverse_square: Dimensionless = 1.0e160 ^ -2;
",
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(find_value(&result, "subnormal"), 2.0_f64.powf(-1070.0));
    assert_eq!(find_value(&result, "tiny"), 10.0_f64.powf(-310.0));
    assert_eq!(find_value(&result, "inverse_square"), 1.0e160_f64.powf(-2.0));
}

#[test]
fn integer_literal_power_still_reports_a_genuine_underflow() {
    assert_eval_failed(
        "node gone: Dimensionless = 2.0 ^ -1080;",
        "gone",
        "power operation underflowed to zero",
    );
}

#[test]
fn int_power_of_a_unit_magnitude_base_accepts_a_huge_exponent() {
    let result = compile_and_eval(
        r"
param minus: Int = -1;
param identity: Int = 1;
param nothing: Int = 0;
node one: Int = @identity ^ 5000000000;
node minus_one: Int = @minus ^ 5000000001;
node plus_one: Int = @minus ^ 5000000000;
node zero: Int = @nothing ^ 5000000000;
",
    )
    .unwrap();
    assert_eq!(super::find_int_value(&result, "one"), 1);
    assert_eq!(super::find_int_value(&result, "minus_one"), -1);
    assert_eq!(super::find_int_value(&result, "plus_one"), 1);
    assert_eq!(super::find_int_value(&result, "zero"), 0);
    assert_eval_failed(
        "param two: Int = 2;\nnode huge: Int = @two ^ 5000000000;",
        "huge",
        "integer arithmetic overflow",
    );
}

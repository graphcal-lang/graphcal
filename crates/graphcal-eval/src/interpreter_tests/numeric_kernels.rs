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
    assert_eq!(
        find_value(&result, "inverse_square"),
        1.0e160_f64.powf(-2.0)
    );
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

#[test]
fn complex_square_and_cross_do_not_overflow_before_the_cancellation() {
    let result = compile_and_eval(
        r"
node w: Complex<Dimensionless> = complex(1.4e154, 6e153);
node z: Complex<Dimensionless> = @w * @w;
node z_re: Dimensionless = re(@z);
node z_im: Dimensionless = im(@z);
node a: Dimensionless[Fin(3)] = table[Fin(3)] { 0.0; 1.4e154; 6e153; };
node b: Dimensionless[Fin(3)] = table[Fin(3)] { 0.0; 6e153; 1.4e154; };
node c: Dimensionless[Fin(3)] = cross(@a, @b);
node c_x: Dimensionless = @c[0];
",
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert!((find_value(&result, "z_re") / 1.6e308 - 1.0).abs() < 1.0e-15);
    assert!((find_value(&result, "z_im") / 1.68e308 - 1.0).abs() < 1.0e-15);
    assert!((find_value(&result, "c_x") / 1.6e308 - 1.0).abs() < 1.0e-15);
}

#[test]
fn solve_near_the_edge_of_the_binary64_range_passes_its_residual_check() {
    let result = compile_and_eval(
        r"
node m: Dimensionless[Fin(1), Fin(1)] = table[Fin(1), Fin(1)] { 1e308; };
node b: Dimensionless[Fin(1)] = table[Fin(1)] { 1e308; };
node x: Dimensionless[Fin(1)] = solve(@m, @b);
node x_0: Dimensionless = @x[0];
",
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(find_value(&result, "x_0"), 1.0);
}

#[test]
fn sum_and_product_do_not_depend_on_the_axis_order() {
    let result = compile_and_eval(
        r"
node up: Dimensionless[Fin(3)] = table[Fin(3)] { 1e308; 1e308; -1e308; };
node mixed: Dimensionless[Fin(3)] = table[Fin(3)] { 1e308; -1e308; 1e308; };
node sum_up: Dimensionless = sum(@up);
node sum_mixed: Dimensionless = sum(@mixed);
node small_first: Dimensionless[Fin(4)] = table[Fin(4)] { 1e-200; 1e-200; 1e200; 1e200; };
node large_first: Dimensionless[Fin(4)] = table[Fin(4)] { 1e200; 1e200; 1e-200; 1e-200; };
node product_small_first: Dimensionless = product(@small_first);
node product_large_first: Dimensionless = product(@large_first);
",
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(find_value(&result, "sum_up"), 1.0e308);
    assert_eq!(find_value(&result, "sum_mixed"), 1.0e308);
    for name in ["product_small_first", "product_large_first"] {
        assert!((find_value(&result, name) - 1.0).abs() <= 4.0 * f64::EPSILON);
    }
}

#[test]
fn a_quantity_literal_that_underflows_in_its_unit_scale_is_an_error() {
    assert_eval_failed(
        "const unit tiny: Length = 1.0e-300 m;\nnode a: Length = 1.0e-30 tiny;",
        "a",
        "quantity literal value underflowed to zero",
    );
    let result = compile_and_eval(
        "const unit tiny: Length = 1.0e-300 m;\nnode zero: Length = 0.0 tiny;\nnode small: Length = 1.0e-20 tiny;",
    )
    .unwrap();
    assert_eq!(find_value(&result, "zero"), 0.0);
    assert!(find_value(&result, "small") > 0.0);
}

#[test]
fn complex_multiplication_scaling_and_division_report_underflow() {
    let prelude = "param tiny: Dimensionless = 1.0e-200;\n";
    for (declaration, operation) in [
        (
            "node z: Complex<Dimensionless> = complex(@tiny, 0.0) * complex(@tiny, 0.0);",
            "complex multiplication",
        ),
        (
            "node z: Complex<Dimensionless> = complex(@tiny, @tiny) * @tiny;",
            "complex scalar multiplication",
        ),
        (
            "node z: Complex<Dimensionless> = complex(@tiny, @tiny) / 1.0e200;",
            "complex scalar division",
        ),
        (
            "node z: Complex<Dimensionless> = complex(@tiny, @tiny) / complex(1.0e200, 0.0);",
            "complex division",
        ),
    ] {
        assert_eval_failed(
            &format!("{prelude}{declaration}"),
            "z",
            &format!("{operation} underflowed a nonzero complex component to zero"),
        );
    }
}

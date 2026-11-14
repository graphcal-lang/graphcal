//! End-to-end evaluation: examples, assertions, datetimes, arithmetic errors, Int, and plugins.

use super::*;

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "clearer to express expected math directly"
)]
fn eval_constants_ksr() {
    let source = include_str!("../../../../tests/fixtures/valid/constants.gcl");
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
fn inline_dag_call_with_failing_assert_fails_calling_node() {
    // #812: inline invocation of an assert-carrying dag evaluates the dag's
    // asserts; a failure fails the calling expression (fault-isolated).
    let result = compile_and_eval(
        "dag checked {\n\
             param v: Dimensionless;\n\
             pub node out: Dimensionless = @v * 2.0;\n\
             assert v_positive = @v > 0.0;\n\
         }\n\
         node y: Dimensionless = @checked(v: -3.0)::out;\n\
         node independent: Dimensionless = 1.0;",
    )
    .unwrap();
    let node_result = |name: &str| {
        result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .clone()
    };
    match node_result("y") {
        Err(NodeUnavailable::EvalFailed { message }) => {
            assert_eq!(
                message,
                "assertion `v_positive` failed in inline call of dag `checked` \
                 (assertion evaluated to false)"
            );
        }
        other => panic!("expected eval failure for `y`, got {other:?}"),
    }
    assert!(
        node_result("independent").is_ok(),
        "independent node must not be affected"
    );
}

#[test]
fn inline_dag_call_with_passing_assert_succeeds() {
    let result = compile_and_eval(
        "dag checked {\n\
             param v: Dimensionless;\n\
             pub node out: Dimensionless = @v * 2.0;\n\
             assert v_positive = @v > 0.0;\n\
         }\n\
         node y: Dimensionless = @checked(v: 3.0)::out;",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 6.0).abs() < f64::EPSILON);
    // The dag's assert is internal to the instantiation — no spurious
    // top-level assertion report.
    assert_eq!(result.assertions.as_slice(), []);
}

#[test]
fn inline_dag_call_respects_expected_fail() {
    // An #[expected_fail] assert that fails inside the inline instantiation
    // is an expected failure → Pass → no error. One that passes unexpectedly
    // inverts to a failure → the calling node errors.
    let source_template = |v: &str| {
        format!(
            "dag checked {{\n\
                 param v: Dimensionless;\n\
                 pub node out: Dimensionless = @v * 2.0;\n\
                 #[expected_fail]\n\
                 assert is_neg = @v < 0.0;\n\
             }}\n\
             node y: Dimensionless = @checked(v: {v})::out;"
        )
    };

    let result = compile_and_eval(&source_template("3.0")).unwrap();
    assert!(
        result.nodes().next().unwrap().1.is_ok(),
        "expected failure occurred → no error: {:?}",
        result.nodes().next().unwrap().1
    );

    let result = compile_and_eval(&source_template("-3.0")).unwrap();
    match &result.nodes().next().unwrap().1 {
        Err(NodeUnavailable::EvalFailed { message }) => {
            assert!(
                message.contains("assertion passed but was marked #[expected_fail]"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected eval failure for unexpected pass, got {other:?}"),
    }
}

#[test]
fn assert_literal_negative_zero_tolerance_is_rejected() {
    let error = compile_and_eval(
        "param measured: Length = 1.0 m;\n\
         assert exact = @measured ~= 1.0 m +/- -0.0 m;",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError { error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Attribute(AttributeError::NegativeTolerance { value }), .. }), .. }) if value == 0.0 && value.is_sign_negative()
    ));
}

#[test]
fn invalid_datetime_timezone_is_rejected_in_every_expression_owner() {
    let cases = [
        (
            "param",
            r#"param bad: Datetime = datetime("2024-11-05T10:00", "Not/A_Timezone");"#,
        ),
        (
            "node",
            r#"node bad: Datetime = datetime("2024-11-05T10:00", "Not/A_Timezone");"#,
        ),
        (
            "const node",
            r#"const node bad: Datetime = datetime("2024-11-05T10:00", "Not/A_Timezone");"#,
        ),
        (
            "nested",
            r#"node bad: Datetime = to_utc(datetime("2024-11-05T10:00", "Not/A_Timezone"));"#,
        ),
    ];

    for (context, source) in cases {
        let error = compile_to_tir(source, "test.gcl").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown timezone `Not/A_Timezone`"),
            "{context} did not reject the timezone during checking: {error}"
        );
    }
}

#[test]
fn datetime_constructors_reject_conflicting_or_missing_interpretation_sources() {
    let cases = [
        r#"node bad: Datetime = datetime("2024-11-05T12:00:00");"#,
        r#"node bad: Datetime = datetime("2024-11-05T12:00:00 TT");"#,
        r#"node bad: Datetime = datetime("2024-11-05T12:00:00Z", "UTC");"#,
        r#"node bad: Datetime<TT> = epoch<TT>("2024-11-05T12:00:00Z");"#,
        r#"node bad: Datetime<TT> = epoch<TT>("2024-11-05T12:00:00 TT");"#,
    ];
    for source in cases {
        let error = compile_to_tir(source, "test.gcl").unwrap_err();
        assert!(
            error.to_string().contains("invalid datetime literal"),
            "constructor contract was not checked: {error}"
        );
    }
}

#[test]
fn dst_gaps_and_folds_are_rejected_during_checking() {
    let gap = compile_to_tir(
        r#"const node gap: Datetime = datetime("2024-03-10T02:30:00", "America/New_York");"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        gap,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::NonexistentCivilDateTime { .. }),
                ..
            }),
            ..
        })
    ));

    let fold = compile_to_tir(
        r#"
node fold: Datetime = if true {
    datetime("2024-11-03T01:30:00", "America/New_York")
} else {
    datetime("2024-11-03T01:30:00Z")
};
"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        fold,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(DimensionError::RepeatedCivilDateTime { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn positional_epoch_scale_is_rejected_during_checking() {
    let error = compile_to_tir(
        r#"node bad: Datetime<TT> = epoch("2024-11-05T12:00:00", TT);"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("epoch requires exactly one static time-scale argument"),
        "unexpected error: {error}"
    );
}

#[test]
fn epoch_constructor_rejects_embedded_scale_suffix_during_checking() {
    let err = compile_to_tir(
        r#"node bad: Datetime<TT> = epoch<TT>("2000-01-01T12:00:00 TT");"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("invalid datetime literal"),
        "unexpected error: {err}"
    );
}

#[test]
fn eval_boolean_and_eagerly_evaluates_the_right_operand() {
    assert_node_error("node y: Bool = false && sqrt(-1.0) > 0.0;", "y", "NaN");
}

#[test]
fn eval_boolean_or_eagerly_evaluates_the_right_operand() {
    assert_node_error("node y: Bool = true || sqrt(-1.0) > 0.0;", "y", "NaN");
}

/// Helper: assert that a specific node in the result has a `NodeUnavailable::EvalFailed`
/// whose message contains `needle`.
fn assert_node_error(source: &str, node_name: &str, needle: &str) {
    let result = compile_and_eval(source).unwrap();
    let EvalEntry {
        outcome: node_result,
        ..
    } = result
        .entries
        .iter()
        .find(|EvalEntry { name: n, .. }| n.to_string() == node_name)
        .unwrap_or_else(|| panic!("node `{node_name}` not found"));
    match node_result {
        Err(NodeUnavailable::EvalFailed { message }) => {
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
fn eval_nonzero_multiplication_underflow_is_an_error() {
    assert_node_error(
        "param tiny: Length = 1.0e-300 m;\nnode area: Area = @tiny * @tiny;",
        "area",
        "arithmetic operation underflowed to zero",
    );
}

#[test]
fn constant_quantity_literal_underflow_is_a_compile_error() {
    for source in [
        "const unit tiny: Length = 1.0e-300 m;\nconst node c: Length = 1.0e-30 tiny;",
        "const unit tiny: Length = 1.0e-300 m;\nindex R = range(0.0 m, 1.0e-30 tiny, step: 1.0e-30 tiny);\nnode n: Int = count(for r: R { coord(r) });",
    ] {
        let error = compile_and_eval(source).unwrap_err();
        let rendered = format!("{error} {error:?}");
        assert!(rendered.contains("Underflow"), "{source}: {rendered}");
    }
}

#[test]
fn eval_int_division_by_zero() {
    assert_node_error(
        "param x: Int = 10;\nnode y: Int = div_trunc(@x, 0);",
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
fn eval_int_with_unit_parse_error() {
    // `10 km` should be a parse error
    let err = compile_and_eval("param x: Length = 10 km;");
    assert!(err.is_err());
}

mod prop {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn division_of_finite_nonzero_is_finite_or_reports_range_loss(
            a in proptest::num::f64::NORMAL,
            b in proptest::num::f64::NORMAL,
        ) {
            prop_assume!(b != 0.0 && a.is_finite() && b.is_finite());
            let source = format!(
                "param x: Dimensionless = {a:e};\nparam y: Dimensionless = {b:e};\nnode z: Dimensionless = @x / @y;"
            );
            let r = compile_and_eval(&source).unwrap();
            let z_result = &r.entries.iter()
                .find(|EvalEntry { name: n, .. }| n.to_string() == "z")
                .unwrap().outcome;
            match z_result {
                Ok(val) => {
                    let z = val.si_value().unwrap().get();
                    prop_assert!(z.is_finite(), "division produced non-finite: {z}");
                }
                Err(NodeUnavailable::EvalFailed { message }) => {
                    // Complete underflow and overflow are both surfaced rather
                    // than silently becoming zero or infinity.
                    prop_assert!(
                        message.contains("underflow")
                            || message.contains("overflow")
                            || message.contains("infinite"),
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

#[test]
fn dag_local_required_types_enable_reusable_linear_algebra() {
    let source = include_str!("../../../../tests/fixtures/valid/linear_algebra_reusable_dag.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "displacement_magnitude::result") - 13.0).abs() < 1e-12);
    assert!((find_value(&result, "duration_magnitude::result") - 13.0).abs() < 1e-12);
    assert!(result.assertions.iter().all(|assertion| matches!(
        assertion.outcome,
        graphcal_eval::eval::types::AssertResult::Pass
    )));
}

#[test]
fn eval_extern_plugin_fixture() {
    // Extern functions (#943 Phase A): dim-variable polymorphism, rational
    // result powers, and cross-variable monomial results all evaluate through
    // the demo host registry.
    let source = include_str!("../../../../tests/fixtures/valid/plugin_extern.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "v_mid") - 150.0).abs() < 1e-9);
    assert!((find_value(&result, "pace") - (1.0 / 150.0)).abs() < 1e-12);
    assert!((find_value(&result, "scale") - 6.0).abs() < 1e-12);
    assert!((find_value(&result, "dv_share_total") - 1.0).abs() < 1e-12);
    // Struct results rebuild as ordinary record values: field access works.
    assert!((find_value(&result, "dv_spread") - 1500.0).abs() < 1e-9);
    assert!(
        result.assertions.iter().all(|assertion| matches!(
            assertion.outcome,
            graphcal_eval::eval::types::AssertResult::Pass
        )),
        "all fixture assertions must pass: {:?}",
        result.assertions
    );
}

#[test]
fn assumes_targets_resolve_through_semantic_instances() {
    let source = "dag producer {\n\
                      pub node x: Dimensionless = 1.0;\n\
                      pub assert okay = @x == 2.0;\n\
                      #[assumes(okay)]\n\
                      pub node y: Dimensionless = @x;\n\
                  }\n\
                  include producer()::{ okay as check, y as y_out };\n\
                  #[assumes(check)]\n\
                  node dependent: Dimensionless = 1.0;\n";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    let mut assumers = result
        .assumes_map
        .get(&scoped_name("check"))
        .expect("the projected assertion keeps its assumers")
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assumers.sort();
    assert_eq!(assumers, ["dependent", "y_out"]);
}

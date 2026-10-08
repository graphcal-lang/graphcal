//! Interpreter tests over single-file programs, checked with the compiler
//! alone ([`crate::test_tir`]) and evaluated through the root's execution
//! plan. Programs that need project loading, imports, includes, or inline
//! DAGs are tested in `graphcal-project`.

use graphcal_compiler::display::source_display_names::SourceDisplayNames;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::tir::typed::CheckedTir;

use crate::eval::bindings::RuntimeParameterBindings;
use crate::eval::{
    AssertResult, CompositionProperty, EvalResult, MarkProperty, NodeUnavailable, PlotFieldValue,
    PlotProperty, PropertyValue, Value,
};
use crate::host_fns::HostFunctionRegistry;

/// Check a single-file program.
fn compile_to_tir(source: &str, _name: &str) -> Result<CheckedTir, SemanticError> {
    crate::test_tir::checked_tir_from_source(source).map(|(tir, _, _)| tir)
}

/// Check and evaluate a single-file program with its parameter defaults.
fn compile_and_eval(source: &str) -> Result<EvalResult, SemanticError> {
    let (tir, src, sources) = crate::test_tir::checked_tir_from_source(source)?;
    let prepared = crate::exec_plan::compile(&tir, src, &sources)?;
    graphcal_compiler::outcome::without_cancellation(|cancellation| {
        crate::eval::runtime::evaluate_plan_with_values_and_bindings_and_cancellation(
            prepared.plan(),
            &RuntimeParameterBindings::new(),
            src,
            &sources,
            &HostFunctionRegistry::new(),
            &SourceDisplayNames::default(),
            cancellation,
        )
    })
    .map(|evaluation| evaluation.into_result_and_presentations().0)
}

/// Find the SI value of a named quantity declaration.
fn find_value(result: &EvalResult, name: &str) -> f64 {
    find_entry(result, name).si_value().unwrap().get()
}

fn find_entry(result: &EvalResult, name: &str) -> Value {
    result
        .entries
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
        .unwrap_or_else(|| panic!("value `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
        .clone()
}

/// Extract indexed entries as `Vec<(variant, si_value)>`.
fn indexed_si_values(value: &Value) -> Vec<(&str, f64)> {
    match value {
        Value::Indexed { entries, .. } => entries
            .iter()
            .map(|(k, v)| {
                (
                    k.as_named()
                        .expect("helper is only used with named indexes")
                        .as_str(),
                    v.si_value().unwrap().get(),
                )
            })
            .collect(),
        _ => panic!("expected indexed value, got {value:?}"),
    }
}

fn find_int_value(result: &EvalResult, name: &str) -> i64 {
    match find_entry(result, name) {
        Value::Int(i) => i,
        other => panic!("expected Int for `{name}`, got {other:?}"),
    }
}

fn find_bool_value(result: &EvalResult, name: &str) -> bool {
    match find_entry(result, name) {
        Value::Bool(b) => b,
        other => panic!("expected Bool for `{name}`, got {other:?}"),
    }
}

#[test]
fn generic_nat_services_cannot_cross_type_owners_with_the_same_parameter_name() {
    use graphcal_compiler::resolved_name::ResolvedStructTypeName;
    use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName, StructTypeName};
    let source = "type A<N: Nat> { A(value: Dimensionless(min: sum(for p: Fin(N) { 1.0 }))), } type B<N: Nat> { B(value: Dimensionless), }";
    let tir = compile_to_tir(source, "nat-scopes.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("nat-scopes.gcl", std::sync::Arc::new(source.to_string()));
    let type_id = |name| {
        ResolvedStructTypeName::for_test(
            tir.root_dag_id().clone(),
            StructTypeName::expect_valid(name),
        )
    };
    let nominal_a = tir.nominal_type_body(&type_id("A")).unwrap();
    let a = nominal_a.definition().generic_params()[0].id().clone();
    let b = tir
        .nominal_type_body(&type_id("B"))
        .unwrap()
        .definition()
        .generic_params()[0]
        .id()
        .clone();
    assert_eq!(a.name, b.name);
    assert_ne!(a, b);
    let (constrained, bounds) = nominal_a.constrained_fields().next().unwrap();
    assert_eq!(
        constrained.field().member().constructor().name(),
        ConstructorName::expect_valid("A")
    );
    assert_eq!(
        constrained.field().field().name(),
        &FieldName::expect_valid("value")
    );
    let bound = bounds.map(graphcal_compiler::syntax::non_empty::NonEmpty::first);
    let context = crate::eval_expr::EvalSession::provisional_constants(
        &tir,
        src,
        &sources,
        graphcal_compiler::cancellation::CancellationToken::unbounded(),
    );
    let own = std::collections::HashMap::from([(a, 3)]);
    let foreign = std::collections::HashMap::from([(b, 3)]);
    let values = crate::constant_pools::RuntimeValueMap::new();
    let tree = graphcal_compiler::tir::dim_check::body_specialization::specialize_bound_expression(
        &tir, bound, &own,
    )
    .unwrap();
    let value = crate::eval_expr::eval_root(&tree, &values, &context).unwrap();
    let crate::runtime_value::RuntimeValue::Quantity(value) = value else {
        panic!("expected a quantity bound, got {value:?}");
    };
    assert_eq!(value.get().to_bits(), 3.0_f64.to_bits());
    assert!(
        graphcal_compiler::tir::dim_check::body_specialization::specialize_bound_expression(
            &tir, bound, &foreign,
        )
        .is_err()
    );
}

#[test]
fn numeric_regressions_retain_small_final_values() {
    let result = compile_and_eval(
        r"
node matrix: Dimensionless[Fin(4), Fin(4)] = table[Fin(4), Fin(4)] {
    1.0e-200, 0.0, 0.0, 0.0;
    0.0, 1.0e-200, 0.0, 0.0;
    0.0, 0.0, 1.0e200, 0.0;
    0.0, 0.0, 0.0, 1.0e200;
};
node determinant: Dimensionless = det(@matrix);
node samples: Dimensionless[Fin(3)] = table[Fin(3)] { 1.0e308; 1.0e-100; -1.0e308; };
node average: Dimensionless = mean(@samples);
node quotient: Complex<Dimensionless> = complex(5.0e-324, 0.0) / complex(0.5, 0.5);
node real_part: Dimensionless = re(@quotient);
node imaginary_part: Dimensionless = im(@quotient);
",
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    assert!((find_value(&result, "determinant") - 1.0).abs() <= 4.0 * f64::EPSILON);
    assert_eq!(
        find_value(&result, "average").to_bits(),
        (1.0e-100_f64 / 3.0).to_bits()
    );
    assert_eq!(find_value(&result, "real_part").to_bits(), 1);
    assert_eq!(
        find_value(&result, "imaginary_part").to_bits(),
        (-f64::from_bits(1)).to_bits()
    );
}

#[test]
fn eval_complex_milestone() {
    let source = include_str!("../../../tests/fixtures/valid/complex.gcl");
    let result = compile_and_eval(source).unwrap();
    let find = |name: &str| {
        result
            .nodes()
            .find(|(candidate, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|error| panic!("value `{name}` has error: {error}"))
    };

    match find("a") {
        Value::Complex {
            si_value,
            dimension,
            display_unit,
        } => {
            assert_eq!((si_value.re(), si_value.im()), (3.0, 4.0));
            assert!(!dimension.is_dimensionless());
            assert!(display_unit.is_none());
        }
        other => panic!("expected a complex value, got {other:?}"),
    }
    match find("complex_product") {
        Value::Complex { si_value, .. } => {
            assert_eq!((si_value.re(), si_value.im()), (-9.0, 38.0));
        }
        other => panic!("expected a complex value, got {other:?}"),
    }
    for name in ["display_cm", "copied_cm"] {
        match find(name) {
            Value::Complex {
                si_value,
                display_unit: Some(display_unit),
                ..
            } => {
                assert_eq!((si_value.re(), si_value.im()), (3.0, 4.0));
                assert_eq!(display_unit.label, "cm");
            }
            other => panic!("expected a converted complex value, got {other:?}"),
        }
    }
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, assertion, _)| *assertion == AssertResult::Pass)
    );
}

#[test]
fn indexed_tolerance_reports_failing_keys_with_detail() {
    // #809: indexed tolerance assertions check per key and report each
    // failing key with its actual/expected/delta detail.
    let result = compile_and_eval(
        "index Case = { A, B };\n\
         node actual: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.0 };\n\
         node expected: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.5 };\n\
         assert close = @actual ~= @expected +/- 0.1;",
    )
    .unwrap();
    match &result.assertions[0].1 {
        crate::eval::types::AssertResult::Fail { message } => {
            assert_eq!(
                message,
                "failed at Case#B (actual 2, expected 2.5 +/- 0.1, off by 0.5)"
            );
        }
        other => panic!("expected Fail, got {other:?}"),
    }
}

#[test]
fn indexed_tolerance_per_key_and_explicit_relative_pass() {
    let result = compile_and_eval(
        "index Case = { A, B };\n\
         node actual: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.0 };\n\
         node expected: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.5 };\n\
         node tol: Dimensionless[Case] = { Case#A: 0.01, Case#B: 0.6 };\n\
         node relative_tol: Dimensionless[Case] = for case: Case { abs(@expected[case]) * 0.25 };\n\
         assert per_key = @actual ~= @expected +/- @tol;\n\
         assert relative = @actual ~= @expected +/- @relative_tol;",
    )
    .unwrap();
    for (name, result, _) in &result.assertions {
        assert_eq!(
            result,
            &crate::eval::types::AssertResult::Pass,
            "assertion `{name}` should pass"
        );
    }
}

#[test]
fn indexed_tolerance_respects_per_variant_expected_fail() {
    // Expected failure occurs → Pass; unexpected pass at the marked key →
    // Fail, exactly as for indexed boolean assertions.
    let source = |tol: &str| {
        format!(
            "index Case = {{ A, B }};\n\
             node actual: Dimensionless[Case] = {{ Case#A: 1.0, Case#B: 2.0 }};\n\
             node expected: Dimensionless[Case] = {{ Case#A: 1.0, Case#B: 2.5 }};\n\
             #[expected_fail(Case#B)]\n\
             assert known = @actual ~= @expected +/- {tol};"
        )
    };

    let result = compile_and_eval(&source("0.1")).unwrap();
    assert_eq!(
        result.assertions[0].1,
        crate::eval::types::AssertResult::Pass
    );

    let result = compile_and_eval(&source("1.0")).unwrap();
    match &result.assertions[0].1 {
        crate::eval::types::AssertResult::Fail { message } => {
            assert!(
                message.contains("unexpected pass at Case#B"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected Fail, got {other:?}"),
    }
}

#[test]
fn explicit_for_compares_indexed_values_element_wise() {
    let result = compile_and_eval(
        "index Case = { A, B };\n\
         node actual: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.0 };\n\
         node expected: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.5 };\n\
         node same: Bool[Case] = for case: Case { @actual[case] == @expected[case] };\n\
         node below: Bool[Case] = for case: Case { @actual[case] < 3.0 };\n\
         assert per_key_report = for case: Case { @actual[case] == @expected[case] };",
    )
    .unwrap();
    let node = |name: &str| {
        result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap()
            .clone()
    };
    let entries = |value: &crate::eval::types::Value| match value {
        crate::eval::types::Value::Indexed { entries, .. } => entries
            .iter()
            .map(|(k, v)| {
                let crate::eval::types::Value::Bool(b) = v else {
                    panic!("expected Bool entry, got {v:?}")
                };
                (k.to_string(), *b)
            })
            .collect::<Vec<_>>(),
        other => panic!("expected indexed value, got {other:?}"),
    };
    assert_eq!(
        entries(&node("same")),
        vec![("A".to_string(), true), ("B".to_string(), false)]
    );
    assert_eq!(
        entries(&node("below")),
        vec![("A".to_string(), true), ("B".to_string(), true)]
    );
    match &result.assertions[0].1 {
        crate::eval::types::AssertResult::Fail { message } => {
            assert_eq!(message, "failed at Case#B");
        }
        other => panic!("expected per-key Fail, got {other:?}"),
    }
}

#[test]
fn expected_fail_finite_position_passes_when_element_fails() {
    // #816: `#[expected_fail(#N)]` keys bind to finite structural axes positionally.
    let result = compile_and_eval(
        "#[expected_fail(#1)]\n\
         assert r = for i: Fin(2) { to_int(i) == 0 };",
    )
    .unwrap();
    assert_eq!(
        result.assertions[0].1,
        crate::eval::types::AssertResult::Pass
    );
}

#[test]
fn expected_fail_finite_position_unexpected_pass_fails() {
    // The "unexpected pass" tripwire works for Fin positions: #0 passes but was
    // marked expected_fail, and #1 fails without being marked.
    let result = compile_and_eval(
        "#[expected_fail(#0)]\n\
         assert r = for i: Fin(2) { to_int(i) == 0 };",
    )
    .unwrap();
    match &result.assertions[0].1 {
        crate::eval::types::AssertResult::Fail { message } => {
            assert!(
                message.contains("unexpected pass") && message.contains("#0"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected Fail, got {other:?}"),
    }
}

#[test]
fn expected_fail_mixed_named_and_finite_tuple_key() {
    let result = compile_and_eval(
        "index Mode = { Boost, Cruise };\n\
         #[expected_fail((Mode#Boost, #1))]\n\
         assert m = for mode: Mode {\n\
             for i: Fin(2) {\n\
                 match mode {\n\
                     Mode#Boost => if to_int(i) == 1 { false } else { true },\n\
                     Mode#Cruise => true,\n\
                 }\n\
             }\n\
         };",
    )
    .unwrap();
    assert_eq!(
        result.assertions[0].1,
        crate::eval::types::AssertResult::Pass
    );
}

#[test]
fn assert_negative_runtime_tolerance_errors() {
    // #815: a tolerance computed at runtime must be non-negative; a negative
    // value is an assertion ERROR, not a silent constant-false FAIL.
    let result = compile_and_eval(
        "param x: Dimensionless = 1.0;\n\
         param tol: Dimensionless = -0.1;\n\
         assert exact = @x ~= 1.0 +/- @tol;",
    )
    .unwrap();
    match &result.assertions[0].1 {
        crate::eval::types::AssertResult::Error { message } => {
            assert!(
                message.contains("tolerance must be non-negative"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected assertion error, got {other:?}"),
    }
}

#[test]
fn assert_negative_runtime_tolerance_expression_errors() {
    let result = compile_and_eval(
        "param x: Dimensionless = 1.0;\n\
         param rate: Dimensionless = -0.05;\n\
         assert exact = @x ~= 1.0 +/- abs(1.0) * @rate;",
    )
    .unwrap();
    match &result.assertions[0].1 {
        crate::eval::types::AssertResult::Error { message } => {
            assert!(
                message.contains("tolerance must be non-negative"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected assertion error, got {other:?}"),
    }
}

#[test]
fn assert_on_failed_dependency_reports_dependency_failure() {
    // #814: an assertion referencing a failed node reports the dependency
    // failure with its root cause and the source-level leaf name — not
    // "undefined graph reference `@file.e`".
    let result = compile_and_eval(
        "param zero: Dimensionless = 0.0;\n\
         node e: Dimensionless = 1.0 / @zero;\n\
         node fine: Dimensionless = 2.0;\n\
         assert uses_e = @e > 0.0;\n\
         assert uses_fine = @fine > 0.0;",
    )
    .unwrap();
    let assert_result = |name: &str| {
        result
            .assertions
            .iter()
            .find(|(assert_name, _, _)| assert_name.to_string() == name)
            .unwrap_or_else(|| panic!("assertion `{name}` not found"))
            .1
            .clone()
    };
    assert_eq!(
        assert_result("uses_e"),
        crate::eval::types::AssertResult::Error {
            message: "dependency failed: e (division by zero)".to_string()
        }
    );
    assert_eq!(
        assert_result("uses_fine"),
        crate::eval::types::AssertResult::Pass
    );
}

#[test]
fn assert_on_transitively_failed_dependency_reports_dependency_name() {
    // A dependency that itself failed only because of an upstream failure is
    // listed by name; the root cause is reported on the failing declaration.
    let result = compile_and_eval(
        "param zero: Dimensionless = 0.0;\n\
         node e: Dimensionless = 1.0 / @zero;\n\
         node f: Dimensionless = @e + 1.0;\n\
         assert uses_f = @f > 0.0;",
    )
    .unwrap();
    assert_eq!(
        result.assertions[0].1,
        crate::eval::types::AssertResult::Error {
            message: "dependency failed: f".to_string()
        }
    );
}

#[test]
fn assert_runtime_negative_zero_tolerance_is_zero() {
    let result = compile_and_eval(
        "param x: Dimensionless = 1.0;\n\
         param tolerance: Dimensionless = -0.0;\n\
         assert exact = @x ~= 1.0 +/- @tolerance;",
    )
    .unwrap();
    assert_eq!(
        result.assertions[0].1,
        crate::eval::types::AssertResult::Pass
    );
}

#[test]
fn assert_zero_tolerance_exact_match_passes() {
    // #815: zero tolerance stays legal — exact-match semantics.
    let result = compile_and_eval(
        "param x: Dimensionless = 1.0;\n\
         assert exact = @x ~= 1.0 +/- 0.0;",
    )
    .unwrap();
    assert_eq!(
        result.assertions[0].1,
        crate::eval::types::AssertResult::Pass
    );
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
    assert_eq!(result.params().next().unwrap().0.to_string(), "b");
    assert_eq!(result.params().nth(1).unwrap().0.to_string(), "a");
    assert_eq!(result.nodes().next().unwrap().0.to_string(), "z");
    assert_eq!(result.nodes().nth(1).unwrap().0.to_string(), "y");
}

#[test]
fn eval_result_all_field_source_order() {
    let source = include_str!("../../../tests/fixtures/valid/rocket.gcl");
    let result = compile_and_eval(source).unwrap();
    let names: Vec<String> = result
        .entries
        .iter()
        .map(|(n, _, _)| n.to_string())
        .collect();
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
    assert_eq!(
        result.entries[0].2,
        graphcal_compiler::declaration_category::ValueDeclCategory::Param
    );
    assert_eq!(
        result.entries[3].2,
        graphcal_compiler::declaration_category::ValueDeclCategory::Const
    );
    assert_eq!(
        result.entries[4].2,
        graphcal_compiler::declaration_category::ValueDeclCategory::Node
    );
}

#[test]
fn eval_orbital_milestone() {
    let source = include_str!("../../../tests/fixtures/valid/orbital.gcl");
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
        .nodes()
        .find(|(n, _)| n.to_string() == "speed_kmh")
        .unwrap();
    let speed_kmh_val = speed_kmh.1.as_ref().unwrap();
    assert_eq!(
        speed_kmh_val.display_label(&result.render),
        Some("km/h".to_string())
    );
    let display_kmh = speed_kmh_val.display_value().unwrap();
    let expected_kmh = expected_speed / (1000.0 / 3600.0);
    assert!(
        (display_kmh - expected_kmh).abs() < 0.01,
        "speed_kmh display = {display_kmh}"
    );
}

#[test]
fn eval_generics_milestone() {
    let source = include_str!("../../../tests/fixtures/valid/generics.gcl");
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

#[test]
fn datetime_timezone_literals_are_typed_and_canonicalized_before_evaluation() {
    let source = r#"
node meeting: Datetime = datetime("2024-11-05T10:00", "asia/tokyo");
node displayed: Datetime = @meeting -> "america/new_york";
"#;
    let tir = compile_to_tir(source, "test.gcl").unwrap();
    let graphcal_compiler::hir::expr::ExprKind::FnCall { args, .. } = tir
        .root()
        .body_for_test()
        .nodes()
        .next()
        .unwrap()
        .definition
        .formula()
        .unwrap()
        .kind()
    else {
        panic!("expected datetime function call");
    };
    let graphcal_compiler::hir::expr::ExprKind::ZonedDateTimeLiteral(datetime) = args[0].kind()
    else {
        panic!(
            "expected a resolved zoned datetime literal, got {:?}",
            args[0]
        );
    };
    let graphcal_compiler::hir::expr::ExprKind::IanaTimeZoneLiteral(time_zone_id) = args[1].kind()
    else {
        panic!("expected a typed IANA timezone literal, got {:?}", args[1]);
    };
    assert_eq!(time_zone_id.as_str(), "Asia/Tokyo");
    assert_eq!(datetime.time_zone(), time_zone_id);
    assert_eq!(
        datetime.timestamp(),
        "2024-11-05T01:00:00Z".parse::<jiff::Timestamp>().unwrap()
    );

    let result = compile_and_eval(source).unwrap();
    let Value::Datetime {
        display_tz: Some(display_tz),
        ..
    } = find_entry(&result, "displayed")
    else {
        panic!("expected displayed datetime value");
    };
    assert_eq!(display_tz.as_str(), "America/New_York");
}

#[test]
fn every_civil_datetime_constructor_has_matching_static_and_runtime_scales() {
    let result = compile_and_eval(
        r#"
node offset: Datetime = datetime("2024-11-05T21:00:00.123456789+09:00");
node utc: Datetime = datetime("2024-11-05T12:00:00.123456789Z");
node zoned: Datetime = datetime("2024-11-05T21:00:00.123456789", "Asia/Tokyo");
node offset_delta: Time = @offset - @utc;
node zoned_delta: Time = @zoned - @utc;
"#,
    )
    .unwrap();
    assert!((find_value(&result, "offset_delta")).abs() < f64::EPSILON);
    assert!((find_value(&result, "zoned_delta")).abs() < f64::EPSILON);

    for name in ["offset", "utc", "zoned"] {
        let Value::Datetime {
            epoch, time_scale, ..
        } = find_entry(&result, name)
        else {
            panic!("expected datetime value for {name}");
        };
        let expected = graphcal_compiler::semantic::time_scale::TimeScale::UTC;
        assert_eq!(time_scale, expected);
        assert_eq!(epoch.time_scale, expected.to_hifitime());
    }
}

#[test]
fn every_datetime_extractor_uses_each_supported_declared_scale() {
    use std::fmt::Write as _;

    let scales = graphcal_compiler::semantic::time_scale::TimeScale::ALL;
    let source = scales.iter().fold(String::new(), |mut source, scale| {
        let id = scale.to_string().to_ascii_lowercase();
        write!(
            source,
            "node {id}: Datetime<{scale}> = epoch<{scale}>(\"2024-01-01T00:00:00\");\n\
             node {id}_year: Int = year(@{id});\n\
             node {id}_month: Int = month(@{id});\n\
             node {id}_day: Int = day(@{id});\n\
             node {id}_hour: Int = hour(@{id});\n\
             node {id}_minute: Int = minute(@{id});\n\
             node {id}_second: Int = second(@{id});\n\
             node {id}_weekday: Int = weekday(@{id});\n\
             node {id}_day_of_year: Int = day_of_year(@{id});\n"
        )
        .unwrap();
        source
    });
    let result = compile_and_eval(&source).unwrap();

    for scale in scales {
        let id = scale.to_string().to_ascii_lowercase();
        assert_eq!(find_int_value(&result, &format!("{id}_year")), 2024);
        assert_eq!(find_int_value(&result, &format!("{id}_month")), 1);
        assert_eq!(find_int_value(&result, &format!("{id}_day")), 1);
        assert_eq!(find_int_value(&result, &format!("{id}_hour")), 0);
        assert_eq!(find_int_value(&result, &format!("{id}_minute")), 0);
        assert_eq!(find_int_value(&result, &format!("{id}_second")), 0);
        assert_eq!(find_int_value(&result, &format!("{id}_weekday")), 1);
        assert_eq!(find_int_value(&result, &format!("{id}_day_of_year")), 1);
    }
}

#[test]
fn declared_scale_and_utc_extract_different_fields_near_year_boundary() {
    let result = compile_and_eval(
        r#"
node tt: Datetime<TT> = epoch<TT>("2024-01-01T00:00:30");
node utc: Datetime = to_utc(@tt);
node tt_year: Int = year(@tt);
node tt_day: Int = day(@tt);
node tt_hour: Int = hour(@tt);
node tt_weekday: Int = weekday(@tt);
node tt_day_of_year: Int = day_of_year(@tt);
node utc_year: Int = year(@utc);
node utc_day: Int = day(@utc);
node utc_hour: Int = hour(@utc);
node utc_weekday: Int = weekday(@utc);
node utc_day_of_year: Int = day_of_year(@utc);
"#,
    )
    .unwrap();

    assert_eq!(find_int_value(&result, "tt_year"), 2024);
    assert_eq!(find_int_value(&result, "tt_day"), 1);
    assert_eq!(find_int_value(&result, "tt_hour"), 0);
    assert_eq!(find_int_value(&result, "tt_weekday"), 1);
    assert_eq!(find_int_value(&result, "tt_day_of_year"), 1);
    assert_eq!(find_int_value(&result, "utc_year"), 2023);
    assert_eq!(find_int_value(&result, "utc_day"), 31);
    assert_eq!(find_int_value(&result, "utc_hour"), 23);
    assert_eq!(find_int_value(&result, "utc_weekday"), 7);
    assert_eq!(find_int_value(&result, "utc_day_of_year"), 365);
}

#[test]
fn display_timezone_metadata_does_not_change_datetime_extractors() {
    let result = compile_and_eval(
        r#"
node utc: Datetime = datetime("2024-11-05T23:30:00Z");
node displayed: Datetime = @utc -> "Asia/Tokyo";
node utc_day: Int = day(@utc);
node displayed_day: Int = day(@displayed);
node utc_hour: Int = hour(@utc);
node displayed_hour: Int = hour(@displayed);
"#,
    )
    .unwrap();

    assert_eq!(find_int_value(&result, "utc_day"), 5);
    assert_eq!(find_int_value(&result, "displayed_day"), 5);
    assert_eq!(find_int_value(&result, "utc_hour"), 23);
    assert_eq!(find_int_value(&result, "displayed_hour"), 23);
}

#[test]
fn every_epoch_constructor_has_matching_static_and_runtime_scales() {
    let source = r#"
node utc: Datetime<UTC> = epoch<UTC>("2024-11-05T12:00:00");
node tai: Datetime<TAI> = epoch<TAI>("2024-11-05T12:00:00");
node tt: Datetime<TT> = epoch<TT>("2024-11-05T12:00:00");
node tdb: Datetime<TDB> = epoch<TDB>("2024-11-05T12:00:00");
node et: Datetime<ET> = epoch<ET>("2024-11-05T12:00:00");
node gpst: Datetime<GPST> = epoch<GPST>("2024-11-05T12:00:00");
node gst: Datetime<GST> = epoch<GST>("2024-11-05T12:00:00");
node bdt: Datetime<BDT> = epoch<BDT>("2024-11-05T12:00:00");
node qzsst: Datetime<QZSST> = epoch<QZSST>("2024-11-05T12:00:00");
"#;
    let result = compile_and_eval(source).unwrap();
    for expected_scale in graphcal_compiler::semantic::time_scale::TimeScale::ALL {
        let name = expected_scale.to_string().to_ascii_lowercase();
        let Value::Datetime {
            epoch, time_scale, ..
        } = find_entry(&result, &name)
        else {
            panic!("expected datetime value for {name}");
        };
        assert_eq!(time_scale, expected_scale);
        assert_eq!(epoch.time_scale, expected_scale.to_hifitime());
    }
}

#[test]
fn epoch_constructor_applies_explicit_scale_without_suffix_concat() {
    let result =
        compile_and_eval(r#"node tt: Datetime<TT> = epoch<TT>("2000-01-01T12:00:00");"#).unwrap();
    let Value::Datetime { epoch, .. } = find_entry(&result, "tt") else {
        panic!("expected datetime value");
    };
    let expected =
        hifitime::Epoch::maybe_from_gregorian(2000, 1, 1, 12, 0, 0, 0, hifitime::TimeScale::TT)
            .unwrap();
    assert_eq!(epoch, expected);
}

#[test]
fn eval_indexed_milestone() {
    let source = include_str!("../../../tests/fixtures/valid/indexed.gcl");
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
    assert_eq!(find_int_value(&result, "n_maneuvers"), 3);

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
fn count_returns_int_for_non_quantity_indexed_values() {
    let source = r"
index Case = { First, Second, Third };
node flags: Bool[Case] = for case: Case { true };
node n: Int = count(@flags);
";
    let result = compile_and_eval(source).unwrap();
    assert_eq!(find_int_value(&result, "n"), 3);
}

#[test]
fn eval_scan_uses_index_order_for_map_literals() {
    let source = r"
index Phase = { A, B };
node x: Dimensionless[Phase] = { Phase#B: 10.0, Phase#A: 1.0 };
node y: Dimensionless[Phase] = scan(@x, 0.0, |acc, val| acc + val);
";
    let result = compile_and_eval(source).unwrap();
    let x = find_entry(&result, "x");
    let y = find_entry(&result, "y");
    let x_vals = indexed_si_values(&x);
    let y_vals = indexed_si_values(&y);
    assert_eq!(x_vals, [("A", 1.0), ("B", 10.0)]);
    assert_eq!(y_vals, [("A", 1.0), ("B", 11.0)]);
}

#[test]
fn eval_scan_order_follows_label_declaration_order() {
    let source = r"
index Phase = { B, A };
node x: Dimensionless[Phase] = { Phase#A: 1.0, Phase#B: 10.0 };
node y: Dimensionless[Phase] = scan(@x, 0.0, |acc, val| acc + val);
";
    let result = compile_and_eval(source).unwrap();
    let x = find_entry(&result, "x");
    let y = find_entry(&result, "y");
    let x_vals = indexed_si_values(&x);
    let y_vals = indexed_si_values(&y);
    assert_eq!(x_vals, [("B", 10.0), ("A", 1.0)]);
    assert_eq!(y_vals, [("B", 10.0), ("A", 11.0)]);
}

#[test]
fn eval_unfold_passes_previous_state_and_coordinates() {
    let source = r"
index Step = range(0.0 s, 2.0 s, step: 1.0 s);
node distance: Length[Step] = unfold(
    Step,
    1.0 m,
    |prev_distance, prev_t, t| prev_distance + (2.0 m/s) * (coord(t) - coord(prev_t))
);
";
    let result = compile_and_eval(source).unwrap();
    let Value::Indexed { entries, .. } = find_entry(&result, "distance") else {
        panic!("expected indexed unfold result");
    };
    let distances = entries
        .values()
        .map(|value| value.si_value().unwrap().get())
        .collect::<Vec<_>>();
    assert_eq!(distances, [1.0, 3.0, 5.0]);
}

#[test]
fn eval_indexed_unfold_and_scan_state_preserve_complete_previous_snapshot() {
    let source = include_str!("../../../tests/fixtures/valid/indexed_state_recurrence.gcl");
    let result = compile_and_eval(source).unwrap();

    let state_rows = |name| {
        let Value::Indexed { entries, .. } = find_entry(&result, name) else {
            panic!("expected indexed recurrence result for `{name}`");
        };
        entries
            .values()
            .map(|state| {
                indexed_si_values(state)
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };

    // The asymmetric coupling catches in-place updates: B at step 1 must read
    // the previous A=1, not the newly computed A=3.
    assert_eq!(
        state_rows("trajectory"),
        [vec![1.0, 2.0], vec![3.0, 1.0], vec![4.0, 3.0]]
    );
    assert_eq!(
        state_rows("scanned_state"),
        [vec![1.0, 2.0], vec![2.0, 3.0], vec![4.0, 5.0]]
    );
}

#[test]
fn eval_nested_unfold_uses_its_expression_axis_under_scalar_owner() {
    let source = r"
index Step = range(0.0 s, 2.0 s, step: 1.0 s);
node total: Dimensionless = sum(unfold(
    Step,
    1.0,
    |prev_value, prev_t, t| prev_value + 1.0
));
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "total") - 6.0).abs() < f64::EPSILON);
}

#[test]
fn eval_scan_supports_heterogeneous_accumulator() {
    let source = r"
index Flag = { A, B, C };
node flags: Bool[Flag] = {
    Flag#A: true,
    Flag#B: false,
    Flag#C: true,
};
node count_true: Int[Flag] = scan(
    @flags,
    0,
    |tally, flag| if flag { tally + 1 } else { tally }
);
";
    let result = compile_and_eval(source).unwrap();
    let Value::Indexed { entries, .. } = find_entry(&result, "count_true") else {
        panic!("expected indexed scan result");
    };
    let counts = entries
        .values()
        .map(|value| match value {
            Value::Int(count) => *count,
            other => panic!("expected integer scan entry, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(counts, [1, 1, 2]);
}

#[test]
fn eval_table_literal_finite_index_1d() {
    let source = r"
param v: Dimensionless[Fin(3)] = table[Fin(3)] {
    1.0;
    2.0;
    3.0;
};
node total: Dimensionless = sum(for i: Fin(3) { @v[i] });
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "total") - 6.0).abs() < f64::EPSILON);
}

#[test]
fn eval_table_literal_finite_index_2d() {
    let source = r"
param m: Dimensionless[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
    1.0, 2.0, 3.0;
    4.0, 5.0, 6.0;
};
node row_sums: Dimensionless[Fin(2)] = for i: Fin(2) {
    sum(for j: Fin(3) { @m[i, j] })
};
node total: Dimensionless = sum(for i: Fin(2) { @row_sums[i] });
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "total") - 21.0).abs() < f64::EPSILON);
}

#[test]
fn eval_table_literal() {
    let source = include_str!("../../../tests/fixtures/valid/table_literal.gcl");
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
            .nodes()
            .find(|(n, _)| n.to_string() == "bad")
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
        .nodes()
        .find(|(n, _)| n.to_string() == "bad")
        .unwrap()
        .1;
    assert!(matches!(
        bad_result,
        Err(NodeUnavailable::EvalFailed { .. })
    ));
    // downstream fails with DependencyFailed
    let ds_result = &result
        .nodes()
        .find(|(n, _)| n.to_string() == "downstream")
        .unwrap()
        .1;
    assert!(matches!(
        ds_result,
        Err(NodeUnavailable::DependencyFailed { .. })
    ));
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

#[test]
fn eval_integers_milestone() {
    let source = include_str!("../../../tests/fixtures/valid/integers.gcl");
    let result = compile_and_eval(source).unwrap();

    assert_eq!(find_int_value(&result, "a"), 10);
    assert_eq!(find_int_value(&result, "b"), 3);
    assert_eq!(find_int_value(&result, "int_sum"), 13);
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
    // Exact conversion accepts an integer-valued quantity.
    assert_eq!(find_int_value(&result, "back_to_int"), 3);
    // A rounding policy must be explicit before conversion.
    assert_eq!(find_int_value(&result, "truncated_to_int"), 3);
}

#[test]
fn eval_zero_operand_does_not_report_underflow() {
    let result = compile_and_eval(
        "param zero: Length = 0.0 m;\nparam tiny: Length = 1.0e-300 m;\nnode area: Area = @zero * @tiny;",
    )
    .unwrap();
    assert!(find_value(&result, "area").abs() < f64::EPSILON);
}

#[test]
fn eval_minimum_integer_literal_modulo_negative_one() {
    let result = compile_and_eval(
        "param min: Int = -9223372036854775808;\nnode remainder: Int = @min % -1;",
    )
    .unwrap();

    assert_eq!(find_int_value(&result, "min"), i64::MIN);
    assert_eq!(find_int_value(&result, "remainder"), 0);
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
    // Int + Quantity should be a type error
    let err = compile_and_eval("param x: Int = 10;\nnode y: Dimensionless = @x + 1.0;");
    assert!(err.is_err());
}

#[test]
fn plot_only_finite_axes_do_not_require_unrelated_declarations() {
    for prefix in [
        "",
        "node unrelated: Dimensionless[Fin(2)] = table[Fin(2)] { 1.0; 1.0; };\n",
    ] {
        let source = format!(
            "{prefix}{}",
            r"
param divisor: Dimensionless = 1.0;
plot curve = {
    mark: line,
    encode: {
        x: for i: Fin(2) { 1.0 },
        y: for i: Fin(2) { 1.0 / @divisor },
    },
};
"
        );
        let result = compile_and_eval(&source).unwrap();
        assert!(!result.has_errors(), "{result:?}");
        assert_eq!(result.plots.len(), 1);
        assert_eq!(result.plots[0].encodings.len(), 2);
        for (_, values) in &result.plots[0].encodings {
            match values {
                PlotFieldValue::Numbers(values) => assert_eq!(values.as_slice(), [1.0, 1.0]),
                other => panic!("expected numeric plot data, got {other:?}"),
            }
        }
    }
}

#[test]
fn plot_properties_evaluate_to_their_checked_value_types() {
    let result = compile_and_eval(
        r##"
plot dots = {
    mark: point { filled: true, size: 30, color: "#0891b2" },
    encode: { x: 1.0, y: 2.0 },
    title: "Dots",
    width: 480,
};
layer overlay = { plots: [dots], width: 200.0 };
"##,
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    let [plot] = result.plots.as_slice() else {
        panic!("expected one plot, got {:?}", result.plots);
    };
    // A boolean property stays a boolean, never a string spelling of one.
    assert_eq!(
        plot.mark_properties,
        [
            (MarkProperty::Filled, PropertyValue::Bool(true)),
            (MarkProperty::Size, PropertyValue::Number(30.0)),
            (
                MarkProperty::Color,
                PropertyValue::String("#0891b2".to_string())
            ),
        ]
    );
    assert_eq!(
        plot.properties,
        [
            (
                PlotProperty::Title,
                PropertyValue::String("Dots".to_string())
            ),
            (PlotProperty::Width, PropertyValue::Number(480.0)),
        ]
    );
    assert_eq!(
        result.layers[0].properties,
        [(CompositionProperty::Width, PropertyValue::Number(200.0))]
    );
}

#[test]
fn plot_axis_metadata_names_the_unit_of_the_plotted_numbers() {
    use graphcal_compiler::syntax::ast::EncodingChannel;

    fn meta(
        plot: &crate::eval::PlotSpec,
        channel: EncodingChannel,
    ) -> Option<(Option<&str>, Option<&str>)> {
        plot.encoding_meta
            .iter()
            .find(|(candidate, _)| *candidate == channel)
            .map(|(_, meta)| (meta.dimension_label.as_deref(), meta.unit_label.as_deref()))
    }

    let result = compile_and_eval(
        r"
index Epoch = linspace(0.0 min, 10.0 min, points: 3);
node elapsed: Time[Epoch] = for t: Epoch { coord(t) };
node speed: Velocity[Epoch] = for t: Epoch { (2.0 m/s) * coord(t) / (1.0 s) };
node ratio: Dimensionless[Epoch] = for t: Epoch { coord(t) / (1.0 s) };
plot unconverted = { mark: line, encode: { x: @elapsed, y: @speed } };
plot converted = {
    mark: line,
    encode: {
        x: for t: Epoch { t },
        y: for t: Epoch { @speed[t] -> km/s },
        color: @ratio,
    },
};
",
    )
    .unwrap();
    assert!(!result.has_errors(), "{result:?}");
    let [unconverted, converted] = result.plots.as_slice() else {
        panic!("expected two plots, got {:?}", result.plots);
    };
    // Unconverted quantities are plotted in SI and named by the canonical unit.
    assert_eq!(
        meta(unconverted, EncodingChannel::X),
        Some((Some("Time"), Some("s")))
    );
    assert_eq!(
        meta(unconverted, EncodingChannel::Y),
        Some((Some("Velocity"), Some("m/s")))
    );
    // A coordinate key is plotted as its SI coordinate, although its index
    // displays minutes.
    assert_eq!(
        meta(converted, EncodingChannel::X),
        Some((Some("Time"), Some("s")))
    );
    let x = converted
        .encodings
        .iter()
        .find(|(channel, _)| *channel == EncodingChannel::X)
        .map(|(_, values)| values);
    assert!(
        matches!(x, Some(PlotFieldValue::Numbers(values)) if values == &[0.0, 300.0, 600.0]),
        "{x:?}"
    );
    // An explicit conversion names its display unit.
    assert_eq!(
        meta(converted, EncodingChannel::Y),
        Some((Some("Velocity"), Some("km/s")))
    );
    // A dimensionless channel names no unit.
    assert_eq!(meta(converted, EncodingChannel::Color), Some((None, None)));
}

#[test]
fn datetime_domain_constraints_accept_inclusive_same_scale_bounds() {
    let result = compile_and_eval(
        r#"
const node TT_START: Datetime<TT> = epoch<TT>("2024-01-01T00:00:00");
const node TT_END: Datetime<TT> = epoch<TT>("2024-12-31T23:59:59");
param utc: Datetime(
    min: datetime("2024-01-01T00:00:00Z"),
    max: datetime("2024-12-31T23:59:59Z"),
) = datetime("2024-06-01T00:00:00Z") -> "Asia/Tokyo";
param tt_at_min: Datetime<TT>(min: @TT_START, max: @TT_END) = @TT_START;
param tt_at_max: Datetime<TT>(min: @TT_START, max: @TT_END) = @TT_END;
param converted_bound: Datetime<TT>(
    min: to_tt(datetime("2024-01-01T00:00:00Z")),
) = epoch<TT>("2024-06-01T00:00:00");
"#,
    )
    .unwrap();

    for name in ["utc", "tt_at_min", "tt_at_max", "converted_bound"] {
        let (_, value, _) = result
            .entries
            .iter()
            .find(|(candidate, _, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("{name} not found"));
        assert!(value.is_ok(), "{name} failed: {value:?}");
    }
}

#[test]
fn every_supported_datetime_scale_accepts_matching_domain_bounds() {
    use std::fmt::Write as _;

    let source = graphcal_compiler::semantic::time_scale::TimeScale::ALL
        .iter()
        .fold(String::new(), |mut source, scale| {
            let name = scale.to_string().to_ascii_lowercase();
            writeln!(
                source,
                "param {name}: Datetime<{scale}>(\
                 min: epoch<{scale}>(\"2024-01-01T00:00:00\"), \
                 max: epoch<{scale}>(\"2024-12-31T23:59:59\")) = \
                 epoch<{scale}>(\"2024-06-01T00:00:00\");"
            )
            .unwrap();
            source
        });
    let result = compile_and_eval(&source).unwrap();
    for scale in graphcal_compiler::semantic::time_scale::TimeScale::ALL {
        let name = scale.to_string().to_ascii_lowercase();
        let (_, value, _) = result
            .entries
            .iter()
            .find(|(candidate, _, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("{name} not found"));
        assert!(value.is_ok(), "{name} failed: {value:?}");
    }
}

#[test]
fn indexed_datetime_domain_constraint_reports_the_first_violating_entry() {
    let result = compile_and_eval(
        r#"
pub(bind) index Event = { Early, OnTime };
param schedule: Datetime(
    min: datetime("2024-01-01T00:00:00Z"),
    max: datetime("2024-12-31T23:59:59Z"),
)[Event] = {
    Event#Early: datetime("2023-12-31T23:59:59Z"),
    Event#OnTime: datetime("2024-06-01T00:00:00Z"),
};
"#,
    )
    .unwrap();
    let (_, schedule, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "schedule")
        .expect("schedule not found");
    let NodeUnavailable::EvalFailed { message } = schedule.as_ref().unwrap_err() else {
        panic!("expected EvalFailed, got {schedule:?}");
    };
    assert!(
        message.contains("Early") && message.contains("below minimum"),
        "message = {message}"
    );
}

#[test]
fn datetime_struct_field_constraint_is_enforced_at_construction() {
    let result = compile_and_eval(
        r#"
type EventSpec {
    EventSpec(at: Datetime<TT>(
        min: epoch<TT>("2024-01-01T00:00:00"),
        max: epoch<TT>("2024-12-31T23:59:59"),
    )),
}
node BAD: EventSpec = EventSpec(at: epoch<TT>("2025-01-01T00:00:00"));
"#,
    )
    .unwrap();
    let (_, bad, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "BAD")
        .expect("BAD not found");
    let NodeUnavailable::EvalFailed { message } = bad.as_ref().unwrap_err() else {
        panic!("expected EvalFailed, got {bad:?}");
    };
    assert!(
        message.contains("EventSpec.at") && message.contains("above maximum"),
        "message = {message}"
    );
}

#[test]
fn indexed_datetime_struct_field_constraint_is_element_wise() {
    let result = compile_and_eval(
        r#"
index Slot = { First, Second };
type Schedule {
    Schedule(events: Datetime(
        min: datetime("2024-01-01T00:00:00Z"),
        max: datetime("2024-12-31T23:59:59Z"),
    )[Slot]),
}
node BAD: Schedule = Schedule(events: {
    Slot#First: datetime("2024-06-01T00:00:00Z"),
    Slot#Second: datetime("2025-01-01T00:00:00Z"),
});
"#,
    )
    .unwrap();
    let (_, bad, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "BAD")
        .expect("BAD not found");
    let NodeUnavailable::EvalFailed { message } = bad.as_ref().unwrap_err() else {
        panic!("expected EvalFailed, got {bad:?}");
    };
    assert!(
        message.contains("Schedule.events")
            && message.contains("Second")
            && message.contains("above maximum"),
        "message = {message}"
    );
}

#[test]
fn int_domain_constraints_preserve_i64_extremes() {
    let result = compile_and_eval(
        "param min_value: Int(\
         min: -9223372036854775807 - 1, \
         max: 9223372036854775807) = -9223372036854775807 - 1;\n\
         param max_value: Int(\
         min: -9223372036854775807 - 1, \
         max: 9223372036854775807) = 9223372036854775807;",
    )
    .unwrap();
    assert_eq!(find_int_value(&result, "min_value"), i64::MIN);
    assert_eq!(find_int_value(&result, "max_value"), i64::MAX);
}

#[test]
fn struct_field_within_bounds_passes() {
    let source = include_str!("../../../tests/fixtures/valid/domain_field_within_bounds.gcl");
    let result = compile_and_eval(source).unwrap();
    let (_, val) = result
        .consts()
        .find(|(n, _)| n.to_string() == "SAT")
        .expect("SAT not found");
    matches!(val.as_ref().unwrap(), Value::Struct { .. });
}

#[test]
fn struct_field_runtime_violation_is_per_node_error() {
    let source = "
type Spec { Spec(mass: Mass(min: 100.0 kg, max: 2000.0 kg)) }
param x: Mass = 5000.0 kg;
node SAT: Spec = Spec(mass: @x);
";
    let result = compile_and_eval(source).unwrap();
    let (_, sat_result, _) = result
        .entries
        .iter()
        .find(|(n, _, _)| n.to_string() == "SAT")
        .expect("SAT not found");
    let err = sat_result.as_ref().unwrap_err();
    let NodeUnavailable::EvalFailed { message } = err else {
        panic!("expected EvalFailed, got {err:?}");
    };
    assert!(
        message.contains("Spec.mass") && message.contains("above maximum"),
        "message = {message}"
    );
}

#[test]
fn union_member_field_violation() {
    let source = "
pub type Result {
    Burn(dv: Velocity(max: 10.0 km/s)),
    Coast,
}
node R: Result = Burn(dv: 50.0 km/s);
";
    let result = compile_and_eval(source).unwrap();
    let (_, r_result, _) = result
        .entries
        .iter()
        .find(|(n, _, _)| n.to_string() == "R")
        .expect("R not found");
    let err = r_result.as_ref().unwrap_err();
    let NodeUnavailable::EvalFailed { message } = err else {
        panic!("expected EvalFailed, got {err:?}");
    };
    assert!(
        message.contains("Burn.dv") && message.contains("above maximum"),
        "message = {message}"
    );
}

#[test]
fn temporary_generic_constructor_uses_its_concrete_field_constraint() {
    let result = compile_and_eval(
        r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node bad: Length = Box<Length>(x: 0.1 m).x;
",
    )
    .unwrap();
    let (_, bad, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "bad")
        .expect("bad not found");
    let NodeUnavailable::EvalFailed { message } = bad.as_ref().unwrap_err() else {
        panic!("expected EvalFailed, got {bad:?}");
    };
    assert!(message.contains("below minimum"), "message = {message}");
}

#[test]
fn matching_generic_field_constraint_remains_evaluable() {
    let result = compile_and_eval(
        r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node good: Box<Length> = Box<Length>(x: 1.0 m);
",
    )
    .unwrap();
    let (_, good, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "good")
        .expect("good not found");
    assert!(good.is_ok(), "good failed: {good:?}");
}

#[test]
fn valid_generic_static_fin_key_constraint_remains_evaluable() {
    let result = compile_and_eval(
        r"
type T<N: Nat> { T(x: Int(min: to_int(key(Fin(N), 1)))) }
node good: T<2> = T<2>(x: 1);
",
    )
    .unwrap();
    let (_, good, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "good")
        .expect("good not found");
    assert!(good.is_ok(), "good failed: {good:?}");
}

#[test]
fn generic_nat_field_constraints_are_keyed_by_concrete_application() {
    let result = compile_and_eval(
        r"
type AtLeastCardinality<N: Nat> {
    AtLeastCardinality(x: Int(min: count(for i: Fin(N) { i })))
}
node two: AtLeastCardinality<2> = AtLeastCardinality<2>(x: 2);
node bad_three: AtLeastCardinality<3> = AtLeastCardinality<3>(x: 2);
",
    )
    .unwrap();

    let (_, two, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "two")
        .expect("two not found");
    assert!(two.is_ok(), "two failed: {two:?}");

    let (_, bad_three, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "bad_three")
        .expect("bad_three not found");
    let NodeUnavailable::EvalFailed { message } = bad_three.as_ref().unwrap_err() else {
        panic!("expected EvalFailed, got {bad_three:?}");
    };
    assert!(message.contains("below minimum"), "message = {message}");
}

#[test]
fn valid_generic_constant_index_constraint_remains_evaluable() {
    let result = compile_and_eval(
        r"
type T<N: Nat> {
    T(x: Int(min: to_int((for i: Fin(N) { i })[1])))
}
node good: T<2> = T<2>(x: 1);
",
    )
    .unwrap();
    let (_, good, _) = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "good")
        .expect("good not found");
    assert!(good.is_ok(), "good failed: {good:?}");
}

#[test]
fn public_value_equality_distinguishes_constructor_and_generic_args() {
    let result = compile_and_eval(
        "type Mode { Coast, Burn }\n\
         type Eci { Eci }\n\
         type Ecef { Ecef }\n\
         type Framed<F: Type> { Framed(v: Dimensionless) }\n\
         node a: Mode = Coast;\n\
         node b: Mode = Burn;\n\
         node c: Framed<Eci> = Framed<Eci>(v: 1.0);\n\
         node d: Framed<Ecef> = Framed<Ecef>(v: 1.0);",
    )
    .unwrap();
    let node = |name: &str| {
        result
            .nodes()
            .find(|(n, _)| n.to_string() == name)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .clone()
    };
    assert_eq!(node("a"), node("a"));
    assert_ne!(node("a"), node("b"));
    assert_ne!(node("c"), node("d"));
    assert!(
        matches!(node("a"), Value::Struct { ref constructor, .. } if constructor.as_str() == "Coast")
    );
}

#[test]
fn index_axis_resolves_concrete_definitions_from_checked_tir() {
    use crate::runtime_value::IndexAxis;
    use graphcal_compiler::semantic::checked_type::IndexTypeRef;
    use graphcal_compiler::syntax::index_name::IndexName;

    let source = "index Mode = { Idle, Run }; index Step = range(0.0 s, 2.0 s, step: 1.0 s);";
    let tir = compile_to_tir(source, "axis.gcl").unwrap();
    let index =
        |name| IndexTypeRef::with_owner(tir.root_dag_id().clone(), IndexName::expect_valid(name));

    let mode = index("Mode");
    let axis = IndexAxis::resolve(&tir, &mode).unwrap();
    assert_eq!(axis.index(), &mode);
    assert_eq!(axis.len(), 2);
    assert!(matches!(
        axis.kind(),
        graphcal_compiler::semantic::index_def::ConcreteIndexKind::Named { .. }
    ));

    let axis = IndexAxis::resolve(&tir, &index("Step")).unwrap();
    assert_eq!(axis.len(), 3);
    assert!(axis.coordinate_data().is_some());

    assert!(IndexAxis::resolve(&tir, &index("Missing")).is_none());
}

#[test]
fn ordinary_plot_and_composition_property_failures_remain_contained() {
    let result = compile_and_eval("node value: Length = 1.0 m; plot good = { mark: line, encode: { x: 1.0, y: 2.0 } }; plot broken = { mark: line { stroke_width: 1.0 / 0.0 }, encode: { x: 1.0, y: 2.0 } }; plot bad_width = { mark: line, encode: { x: 1.0, y: 2.0 }, width: -1.0 }; figure comparison = { plots: [good], title: \"Valid\" }; layer overlay = { plots: [good], width: 1.0 / 0.0 };").unwrap();
    assert!(result.nodes().next().unwrap().1.is_ok());
    assert_eq!(result.plots.len(), 1);
    assert_eq!(result.figures.len(), 1);
    assert_eq!(result.plot_errors.len(), 3);
    assert_eq!(result.presentation_diagnostics.as_slice(), []);
}

#[test]
fn indexed_graph_ref_borrows_collection_before_selecting_entry() {
    crate::eval_expr::reset_cloned_runtime_node_count();
    let direct_copy = r"
node source: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    to_int(row) * 4 + to_int(column)
};
node copied: Int[Fin(4), Fin(4)] = @source;
";
    compile_and_eval(direct_copy).unwrap();
    assert_eq!(
        crate::eval_expr::take_cloned_runtime_node_count(),
        21,
        "the clone observer must count the root, four rows, and 16 leaves"
    );

    let elementwise_copy = r"
node source: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    to_int(row) * 4 + to_int(column)
};
node copied: Int[Fin(4), Fin(4)] = for row: Fin(4), column: Fin(4) {
    @source[row, column]
};
";
    compile_and_eval(elementwise_copy).unwrap();
    assert_eq!(
        crate::eval_expr::take_cloned_runtime_node_count(),
        16,
        "index traversal must clone only the 16 selected leaves"
    );
}

/// The shared execution frame of DAG bodies.
#[cfg(test)]
mod execution_frames {
    use graphcal_compiler::node_unavailable::NodeUnavailable;
    use graphcal_compiler::outcome::Outcome;
    use graphcal_compiler::semantic_error::SemanticError;
    use graphcal_compiler::semantic_error::evaluation::{EvaluationError, EvaluatorFailure};

    use crate::execution_frame::{ExecutionFrame, FailurePolicy};

    /// An ordinary (non-fatal) evaluation failure of a test declaration.
    #[derive(Debug, thiserror::Error)]
    #[error("ordinary sentinel")]
    struct OrdinarySentinel;

    #[test]
    fn shared_frames_cancel_before_interpretation() {
        let (tir, src, sources) =
            crate::test_tir::checked_tir_from_source("node value: Dimensionless = 1.0;").unwrap();
        let prepared = crate::exec_plan::compile(&tir, src, &sources).unwrap();
        let plan = prepared.plan();
        for policy in [FailurePolicy::Contain, FailurePolicy::Propagate] {
            let mut frame = ExecutionFrame::new(plan, plan.root(), policy);
            let cancellation = graphcal_compiler::cancellation::CancellationSource::new();
            cancellation.cancel();
            let outcome = frame.run(&cancellation.token(), |_, _| {
                panic!("cancelled frame must not invoke its expression adapter")
            });
            assert!(matches!(outcome, Err(Outcome::Cancelled)));
        }
    }

    #[test]
    fn frame_arguments_are_domain_checked_and_keep_presentation_only_when_bound() {
        use crate::presentation_evidence::{PendingLeaf, PendingQuantityDisplay, QuantityDisplay};
        use crate::runtime_presentation::EvaluatedRuntimeValue;
        use crate::runtime_value::RuntimeValue;
        let source = "param p: Dimensionless(min: 0.0) = 1.0; node n: Dimensionless = @p;";
        let (tir, src, sources) = crate::test_tir::checked_tir_from_source(source).unwrap();
        let prepared = crate::exec_plan::compile(&tir, src, &sources).unwrap();
        let plan = prepared.plan();
        let key = tir
            .root()
            .body_for_test()
            .params()
            .next()
            .map(graphcal_compiler::tir::typed::TypedParamEntry::identity)
            .unwrap();
        let labelled = |value: f64| {
            EvaluatedRuntimeValue::with_leaf(
                RuntimeValue::quantity(value).unwrap(),
                PendingLeaf::Quantity(PendingQuantityDisplay::Ready(QuantityDisplay::Unit {
                    label: "percent".to_owned(),
                    scale: graphcal_compiler::semantic::unit_scale::PositiveFiniteScale::new(0.01)
                        .unwrap(),
                })),
            )
            .unwrap()
        };
        let span = graphcal_compiler::syntax::span::Span::new(0, 0);

        let mut frame = ExecutionFrame::new(plan, plan.root(), FailurePolicy::Contain);
        frame.bind_argument(&key, labelled(2.0), src, span).unwrap();
        assert!(frame.values().contains_key(&key));
        assert!(frame.presentations().contains_key(&key));
        assert!(frame.errors().is_empty());

        let mut frame = ExecutionFrame::new(plan, plan.root(), FailurePolicy::Contain);
        frame
            .bind_argument(&key, labelled(-1.0), src, span)
            .unwrap();
        assert!(!frame.values().contains_key(&key));
        assert!(!frame.presentations().contains_key(&key));
        assert!(matches!(
            frame.errors().get(&key),
            Some(NodeUnavailable::EvalFailed { .. })
        ));
        let outcome = frame.finish();
        assert!(outcome.values.is_empty() && outcome.presented.is_empty());
        assert_eq!(outcome.errors.len(), 1);

        let mut frame = ExecutionFrame::new(plan, plan.root(), FailurePolicy::Propagate);
        assert!(
            frame
                .bind_argument(&key, labelled(-1.0), src, span)
                .is_err()
        );
    }

    #[test]
    fn shared_frame_dependency_and_fatal_error_policies_are_explicit() {
        let source = "node a: Dimensionless = 1.0; node dependent: Dimensionless = @a; node independent: Dimensionless = 2.0;";
        let (tir, src, sources) = crate::test_tir::checked_tir_from_source(source).unwrap();
        let prepared = crate::exec_plan::compile(&tir, src, &sources).unwrap();
        let plan = prepared.plan();
        let token = graphcal_compiler::cancellation::CancellationToken::unbounded();
        for policy in [FailurePolicy::Contain, FailurePolicy::Propagate] {
            for fatal in [false, true] {
                let mut frame = ExecutionFrame::new(plan, plan.root(), policy);
                let outcome = frame.run(&token, |entry, _| {
                    if entry.key().as_str() == "a" {
                        return Err(if fatal {
                            SemanticError::internal_error(
                                "fatal sentinel",
                                src,
                                graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::WholeFile,
                            )
                        } else {
                            SemanticError::located(
                                src,
                                entry.body().root().span(),
                                EvaluationError::Runtime(EvaluatorFailure::new(OrdinarySentinel)),
                            )
                        }
                        .into());
                    }
                    assert_ne!(
                        entry.key().as_str(),
                        "dependent",
                        "failed dependencies must never be interpreted"
                    );
                    Ok(crate::runtime_presentation::EvaluatedRuntimeValue::plain(
                        crate::runtime_value::RuntimeValue::quantity(2.0).unwrap(),
                    ))
                });
                if fatal || matches!(policy, FailurePolicy::Propagate) {
                    assert!(outcome.is_err());
                } else {
                    outcome.unwrap();
                    assert!(
                        frame
                            .values()
                            .keys()
                            .any(|key| key.as_str() == "independent")
                    );
                    assert!(
                        frame
                            .errors()
                            .iter()
                            .any(|(key, error)| key.as_str() == "dependent"
                                && matches!(error, NodeUnavailable::DependencyFailed { .. }))
                    );
                }
            }
        }
    }

    // Compile-time negative API assertion: implementing DerefMut would make the
    // inferred marker ambiguous and fail compilation, allowing unchecked replacement
    // of the selected environment through its read-only field interface.
    const _: fn() = || {
        trait ReadOnlyUnlessMutable<Marker> {
            fn probe() {}
        }
        impl<T: ?Sized> ReadOnlyUnlessMutable<()> for T {}
        struct Mutable;
        impl<T: ?Sized + std::ops::DerefMut> ReadOnlyUnlessMutable<Mutable> for T {}
        let _ = <crate::eval_expr::EvalSession<'static> as ReadOnlyUnlessMutable<_>>::probe;
    };
}

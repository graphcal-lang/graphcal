use super::*;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_io::RealFileSystem;

fn fs() -> RealFileSystem {
    RealFileSystem::default()
}

/// Find the SI value of a named quantity declaration.
fn find_value(result: &EvalResult, name: &str) -> f64 {
    // Check consts first
    if let Some((_, val)) = result.consts.iter().find(|(n, _)| n.to_string() == name) {
        return val.as_ref().unwrap().si_value().unwrap();
    }
    // Check params and nodes (wrapped in Result)
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

#[test]
fn cancellation_stops_an_in_flight_evaluation() {
    use std::{
        collections::HashMap,
        sync::{Arc, Barrier, mpsc},
        thread,
        time::Duration,
    };

    let source = "node values: Dimensionless[Fin(1000000)] = \
                  for i: Fin(1000000) { 1.0 };";
    let project = crate::loader::LoadedProject::from_source(source, "cancel.gcl").unwrap();
    let cancellation_source = graphcal_compiler::cancellation::CancellationSource::new();
    let cancellation = cancellation_source.token();
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        worker_barrier.wait();
        let result = compile_and_eval_from_project_with_cancellation(
            &project,
            &HashMap::new(),
            &cancellation,
        );
        sender.send(result).unwrap();
    });

    barrier.wait();
    // Let the worker enter the compile/eval pipeline before requesting
    // cancellation. The exact stage is deliberately irrelevant: every stage
    // shares the same token and must unwind to this boundary.
    thread::sleep(Duration::from_millis(20));
    cancellation_source.cancel();

    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("cancelled evaluation should stop promptly");
    let error = result.expect_err("the long-running evaluation should be cancelled");
    assert!(error.is_cancelled(), "unexpected outcome: {error:?}");
}

fn run_mutated_tir_values(
    tir: &graphcal_compiler::tir::typed::TIR,
    source: &str,
) -> crate::eval_expr::RuntimeValueMap {
    let src = miette::NamedSource::new("test.gcl", std::sync::Arc::new(source.to_string()));
    let plan = crate::exec_plan::compile(tir, &src).unwrap();
    let builtin_consts = graphcal_compiler::registry::builtins::builtin_constants();
    let builtin_fns = graphcal_compiler::registry::builtins::builtin_functions();
    super::runtime::run_eval_loop(
        &plan,
        tir,
        &src,
        builtin_consts,
        builtin_fns,
        &crate::host_fns::demo_registry(),
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .unwrap()
    .values
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "clearer to express expected math directly"
)]
fn eval_rocket_milestone() {
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
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
fn eval_complex_milestone() {
    let source = include_str!("../../../../tests/fixtures/valid/complex.gcl");
    let result = compile_and_eval(source).unwrap();
    let find = |name: &str| {
        result
            .nodes
            .iter()
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
    match find("product") {
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
fn eval_uses_hir_builtin_dispatch_after_syntax_mutation() {
    let source = "node y: Dimensionless = sqrt(4.0);";
    let mut tir = compile_to_tir(source, "test.gcl").unwrap();
    assert!(!tir.root().semantic.expressions.nodes.is_empty());
    tir.root_mut().nodes[0].expr.kind =
        graphcal_compiler::hir::ExprKind::StringLiteral("mutated entry".to_string());

    let values = run_mutated_tir_values(&tir, source);
    let key = crate::decl_key::RuntimeDeclKey::for_local_decl(
        tir.root(),
        &graphcal_compiler::syntax::module_name::ScopedName::local("y"),
    );
    let value = values[&key].expect_quantity("y").unwrap();
    assert!((value - 2.0).abs() < f64::EPSILON);
}

#[test]
fn eval_uses_hir_lexical_locals_after_syntax_mutation() {
    let source = "index Phase = { Burn };\nnode y: Dimensionless[Phase] = for p: Phase { match p { Phase.Burn => 1.0 } };";
    let mut tir = compile_to_tir(source, "test.gcl").unwrap();
    assert!(!tir.root().semantic.expressions.nodes.is_empty());
    tir.root_mut().nodes[0].expr.kind =
        graphcal_compiler::hir::ExprKind::StringLiteral("mutated entry".to_string());

    let values = run_mutated_tir_values(&tir, source);
    let key = crate::decl_key::RuntimeDeclKey::for_local_decl(
        tir.root(),
        &graphcal_compiler::syntax::module_name::ScopedName::local("y"),
    );
    let crate::eval_expr::RuntimeValue::Indexed { entries, .. } = &values[&key] else {
        panic!("expected indexed value, got {:?}", values[&key]);
    };
    let burn = graphcal_compiler::syntax::index_name::IndexEntryKey::named(
        graphcal_compiler::syntax::index_name::IndexVariantName::expect_valid("Burn"),
    );
    let value = entries[&burn].expect_quantity("Burn entry").unwrap();
    assert!((value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn eval_assertions_use_hir_body_after_syntax_mutation() {
    let source = "assert ok = sqrt(4.0) == 2.0;";
    let mut tir = compile_to_tir(source, "test.gcl").unwrap();
    assert!(!tir.root().semantic.expressions.asserts.is_empty());
    let span = tir.root().asserts[0].span;
    tir.root_mut().asserts[0].body =
        graphcal_compiler::hir::AssertBody::Expr(graphcal_compiler::hir::Expr::new(
            graphcal_compiler::hir::ExprKind::StringLiteral("mutated entry".to_string()),
            span,
        ));

    let src = miette::NamedSource::new("test.gcl", std::sync::Arc::new(source.to_string()));
    let plan = crate::exec_plan::compile(&tir, &src).unwrap();
    let declared_types = tir.build_declared_types(&src).unwrap();
    let result = super::runtime::evaluate_plan(
        &tir,
        &plan,
        &declared_types,
        &src,
        &crate::host_fns::demo_registry(),
    )
    .unwrap();

    assert!(matches!(
        result.assertions.as_slice(),
        [(_, super::types::AssertResult::Pass, _)]
    ));
}

#[test]
fn indexed_tolerance_reports_failing_keys_with_detail() {
    // #809: indexed tolerance assertions check per key and report each
    // failing key with its actual/expected/delta detail.
    let result = compile_and_eval(
        "index Case = { A, B };\n\
         node actual: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.0 };\n\
         node expected: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.5 };\n\
         assert close = @actual ~= @expected +/- 0.1;",
    )
    .unwrap();
    match &result.assertions[0].1 {
        super::types::AssertResult::Fail { message } => {
            assert_eq!(
                message,
                "failed at Case.B (actual 2, expected 2.5 +/- 0.1, off by 0.5)"
            );
        }
        other => panic!("expected Fail, got {other:?}"),
    }
}

#[test]
fn indexed_tolerance_per_key_and_explicit_relative_pass() {
    let result = compile_and_eval(
        "index Case = { A, B };\n\
         node actual: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.0 };\n\
         node expected: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.5 };\n\
         node tol: Dimensionless[Case] = { Case.A: 0.01, Case.B: 0.6 };\n\
         node relative_tol: Dimensionless[Case] = for case: Case { abs(@expected[case]) * 0.25 };\n\
         assert per_key = @actual ~= @expected +/- @tol;\n\
         assert relative = @actual ~= @expected +/- @relative_tol;",
    )
    .unwrap();
    for (name, result, _) in &result.assertions {
        assert_eq!(
            result,
            &super::types::AssertResult::Pass,
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
             node actual: Dimensionless[Case] = {{ Case.A: 1.0, Case.B: 2.0 }};\n\
             node expected: Dimensionless[Case] = {{ Case.A: 1.0, Case.B: 2.5 }};\n\
             #[expected_fail(Case.B)]\n\
             assert known = @actual ~= @expected +/- {tol};"
        )
    };

    let result = compile_and_eval(&source("0.1")).unwrap();
    assert_eq!(result.assertions[0].1, super::types::AssertResult::Pass);

    let result = compile_and_eval(&source("1.0")).unwrap();
    match &result.assertions[0].1 {
        super::types::AssertResult::Fail { message } => {
            assert!(
                message.contains("unexpected pass at Case.B"),
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
         node actual: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.0 };\n\
         node expected: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.5 };\n\
         node same: Bool[Case] = for case: Case { @actual[case] == @expected[case] };\n\
         node below: Bool[Case] = for case: Case { @actual[case] < 3.0 };\n\
         assert per_key_report = for case: Case { @actual[case] == @expected[case] };",
    )
    .unwrap();
    let node = |name: &str| {
        result
            .nodes
            .iter()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap()
            .clone()
    };
    let entries = |value: &super::types::Value| match value {
        super::types::Value::Indexed { entries, .. } => entries
            .iter()
            .map(|(k, v)| {
                let super::types::Value::Bool(b) = v else {
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
        super::types::AssertResult::Fail { message } => {
            assert_eq!(message, "failed at Case.B");
        }
        other => panic!("expected per-key Fail, got {other:?}"),
    }
}

#[test]
fn expected_fail_finite_position_passes_when_element_fails() {
    // #816: `#[expected_fail(#N)]` keys bind to finite structural axes positionally.
    let result = compile_and_eval(
        "#[expected_fail(#1)]\n\
         assert r = for i: Fin(2) { i == 0 };",
    )
    .unwrap();
    assert_eq!(result.assertions[0].1, super::types::AssertResult::Pass);
}

#[test]
fn expected_fail_finite_position_unexpected_pass_fails() {
    // The "unexpected pass" tripwire works for Fin positions: #0 passes but was
    // marked expected_fail, and #1 fails without being marked.
    let result = compile_and_eval(
        "#[expected_fail(#0)]\n\
         assert r = for i: Fin(2) { i == 0 };",
    )
    .unwrap();
    match &result.assertions[0].1 {
        super::types::AssertResult::Fail { message } => {
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
         #[expected_fail((Mode.Boost, #1))]\n\
         assert m = for m: Mode {\n\
             for i: Fin(2) {\n\
                 match m {\n\
                     Mode.Boost => if i == 1 { false } else { true },\n\
                     Mode.Cruise => true,\n\
                 }\n\
             }\n\
         };",
    )
    .unwrap();
    assert_eq!(result.assertions[0].1, super::types::AssertResult::Pass);
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
         node y: Dimensionless = @checked(v: -3.0).out;\n\
         node independent: Dimensionless = 1.0;",
    )
    .unwrap();
    let node_result = |name: &str| {
        result
            .nodes
            .iter()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .clone()
    };
    match node_result("y") {
        Err(NodeError::EvalFailed { message }) => {
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
         node y: Dimensionless = @checked(v: 3.0).out;",
    )
    .unwrap();
    assert!((find_value(&result, "y") - 6.0).abs() < f64::EPSILON);
    // The dag's assert is internal to the instantiation — no spurious
    // top-level assertion report.
    assert!(result.assertions.is_empty());
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
             node y: Dimensionless = @checked(v: {v}).out;"
        )
    };

    let result = compile_and_eval(&source_template("3.0")).unwrap();
    assert!(
        result.nodes[0].1.is_ok(),
        "expected failure occurred → no error: {:?}",
        result.nodes[0].1
    );

    let result = compile_and_eval(&source_template("-3.0")).unwrap();
    match &result.nodes[0].1 {
        Err(NodeError::EvalFailed { message }) => {
            assert!(
                message.contains("assertion passed but was marked #[expected_fail]"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected eval failure for unexpected pass, got {other:?}"),
    }
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
        super::types::AssertResult::Error { message } => {
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
        super::types::AssertResult::Error { message } => {
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
        super::types::AssertResult::Error {
            message: "dependency failed: e (division by zero)".to_string()
        }
    );
    assert_eq!(assert_result("uses_fine"), super::types::AssertResult::Pass);
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
        super::types::AssertResult::Error {
            message: "dependency failed: f".to_string()
        }
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
    assert_eq!(result.assertions[0].1, super::types::AssertResult::Pass);
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
    assert_eq!(result.params[0].0.to_string(), "b");
    assert_eq!(result.params[1].0.to_string(), "a");
    assert_eq!(result.nodes[0].0.to_string(), "z");
    assert_eq!(result.nodes[1].0.to_string(), "y");
}

#[test]
fn eval_result_all_field_source_order() {
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    let result = compile_and_eval(source).unwrap();
    let names: Vec<String> = result.all.iter().map(|(n, _, _)| n.to_string()).collect();
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
    let source = include_str!("../../../../tests/fixtures/valid/orbital.gcl");
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
        .find(|(n, _)| n.to_string() == "speed_kmh")
        .unwrap();
    let speed_kmh_val = speed_kmh.1.as_ref().unwrap();
    assert_eq!(
        speed_kmh_val.display_label(&result.base_dim_symbols),
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
    let source = include_str!("../../../../tests/fixtures/valid/generics.gcl");
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
        .find(|(n, _, _)| n.to_string() == name)
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
            .map(|(k, v)| {
                (
                    k.as_named()
                        .expect("helper is only used with named indexes")
                        .as_str(),
                    v.si_value().unwrap(),
                )
            })
            .collect(),
        _ => panic!("expected indexed value, got {value:?}"),
    }
}

#[test]
fn datetime_timezone_literals_are_typed_and_canonicalized_before_evaluation() {
    let source = r#"
node meeting: Datetime = datetime("2024-11-05T10:00", "asia/tokyo");
node displayed: Datetime = @meeting -> "america/new_york";
"#;
    let tir = compile_to_tir(source, "test.gcl").unwrap();
    let graphcal_compiler::hir::ExprKind::FnCall { args, .. } = &tir.root().nodes[0].expr.kind
    else {
        panic!("expected datetime function call");
    };
    let graphcal_compiler::hir::ExprKind::ZonedDateTimeLiteral(datetime) = &args[0].kind else {
        panic!(
            "expected a resolved zoned datetime literal, got {:?}",
            args[0]
        );
    };
    let graphcal_compiler::hir::ExprKind::IanaTimeZoneLiteral(time_zone_id) = &args[1].kind else {
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
        CompileError::Eval(GraphcalError::NonexistentCivilDateTime { .. })
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
        CompileError::Eval(GraphcalError::RepeatedCivilDateTime { .. })
    ));
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
        let expected = graphcal_compiler::registry::time_scale::TimeScale::UTC;
        assert_eq!(time_scale, expected);
        assert_eq!(epoch.time_scale, expected.to_hifitime());
    }
}

#[test]
fn every_datetime_extractor_uses_each_supported_declared_scale() {
    use std::fmt::Write as _;

    let scales = graphcal_compiler::registry::time_scale::TimeScale::ALL;
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
    for expected_scale in graphcal_compiler::registry::time_scale::TimeScale::ALL {
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
fn eval_indexed_milestone() {
    let source = include_str!("../../../../tests/fixtures/valid/indexed.gcl");
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
node x: Dimensionless[Phase] = { Phase.B: 10.0, Phase.A: 1.0 };
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
node x: Dimensionless[Phase] = { Phase.A: 1.0, Phase.B: 10.0 };
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
    |prev_distance, prev_t, t| prev_distance + (2.0 m/s) * (t - prev_t)
);
";
    let result = compile_and_eval(source).unwrap();
    let Value::Indexed { entries, .. } = find_entry(&result, "distance") else {
        panic!("expected indexed unfold result");
    };
    let distances = entries
        .values()
        .map(|value| value.si_value().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(distances, [1.0, 3.0, 5.0]);
}

#[test]
fn eval_indexed_unfold_and_scan_state_preserve_complete_previous_snapshot() {
    let source = include_str!("../../../../tests/fixtures/valid/indexed_state_recurrence.gcl");
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
    Flag.A: true,
    Flag.B: false,
    Flag.C: true,
};
node count_true: Int[Flag] = scan(
    @flags,
    0,
    |count, flag| if flag { count + 1 } else { count }
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
    let source = include_str!("../../../../tests/fixtures/valid/table_literal.gcl");
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

fn parse_expr(s: &str) -> graphcal_compiler::desugar::desugared_ast::Expr {
    let raw = graphcal_compiler::syntax::parser::Parser::new(s)
        .parse_single_expr()
        .unwrap();
    raw.into()
}

#[test]
fn override_param_changes_result() {
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    // Default isp=320 s, override to 450 s => higher delta_v
    let default = compile_and_eval_named(source, "test.gcl").unwrap();
    let default_dv = find_value(&default, "delta_v");

    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("isp"), parse_expr("450.0 s"));
    let overridden = compile_and_eval_with_overrides(source, "test.gcl", &overrides).unwrap();
    let new_dv = find_value(&overridden, "delta_v");

    assert!(new_dv > default_dv, "higher isp should give higher delta_v");
}

#[test]
fn override_with_wrong_dimension_errors() {
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    // isp expects Time, not Mass
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("isp"), parse_expr("450.0 kg"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
    assert!(result.is_err());
}

#[test]
fn override_node_errors() {
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("delta_v"), parse_expr("100.0 m/s"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
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
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("g0"), parse_expr("10.0 m/s^2"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
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
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("nonexistent"), parse_expr("100"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
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
    let result = compile_and_eval_with_overrides(source, "test.gcl", &HashMap::new());
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
    overrides.insert(DeclName::expect_valid("x"), parse_expr("42.0"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 43.0).abs() < f64::EPSILON, "y = {y}, expected 43.0");
}
// --- Module import tests ---#[test]#[test]// --- Runtime arithmetic error tests ---

/// Helper: assert that a specific node in the result has a `NodeError::EvalFailed`
/// whose message contains `needle`.
fn assert_node_error(source: &str, node_name: &str, needle: &str) {
    let result = compile_and_eval(source).unwrap();
    let (_, node_result, _) = result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == node_name)
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
        .nodes
        .iter()
        .find(|(n, _)| n.to_string() == "bad")
        .unwrap()
        .1;
    assert!(matches!(bad_result, Err(NodeError::EvalFailed { .. })));
    // downstream fails with DependencyFailed
    let ds_result = &result
        .nodes
        .iter()
        .find(|(n, _)| n.to_string() == "downstream")
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

/// Helper: find a named Bool value.
fn find_bool_value(result: &EvalResult, name: &str) -> bool {
    let val = result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == name)
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
    let source = include_str!("../../../../tests/fixtures/valid/integers.gcl");
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
    // Exact conversion accepts an integer-valued quantity.
    assert_eq!(find_int_value(&result, "back_to_int"), 3);
    // A rounding policy must be explicit before conversion.
    assert_eq!(find_int_value(&result, "truncated_to_int"), 3);
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
    // Int + Quantity should be a type error
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
        .join("../../tests/fixtures/valid/multi/instantiated_import/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
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
fn project_instantiated_import_graph_ref() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_graph_ref/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // my_mass = 800 kg, passed as dry_mass binding via @my_mass
    // delta_v = 320 * 9.80665 * ln(3600/800)
    let expected_delta_v = 320.0 * 9.80665 * (3600.0_f64 / 800.0).ln();
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - expected_delta_v).abs() < 0.01,
        "result = {result_val}, expected = {expected_delta_v}"
    );
}

#[test]
fn project_import_preserves_structural_finite_index_identity() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("src/app");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"app\"\n",
    )
    .unwrap();
    std::fs::write(
        source_dir.join("lib.gcl"),
        "pub const node values: Dimensionless[Fin(2)] = table[Fin(2)] { 1.0; 2.0; };\n",
    )
    .unwrap();
    let root = source_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import app.lib.{ values };\nconst node copied: Dimensionless[Fin(2)] = values;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let copied = result
        .consts
        .iter()
        .find(|(name, _)| name.to_string() == "copied")
        .and_then(|(_, value)| value.as_ref().ok())
        .expect("copied imported finite-indexed value");
    assert!(matches!(copied, Value::Indexed { entries, .. } if entries.len() == 2));
}

#[test]
fn project_selective_import_item_rejects_unknown_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/attr");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"attr\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub node x: Dimensionless = 1.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import attr.lib.{ #[bogus] x };\nnode y: Dimensionless = @x;\n",
    )
    .unwrap();

    match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
        Err(CompileError::Eval(GraphcalError::UnknownAttribute { name, .. })) => {
            assert_eq!(name, "bogus");
        }
        other => panic!("expected UnknownAttribute, got {other:?}"),
    }
}

#[test]
fn project_qualified_index_type_annotation_and_variant_arg() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/mission");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"mission\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub index Phase = { Burn, Coast };\n\
         pub dim GravityAccel = Length / Time^2;\n\
         pub node thrust: Dimensionless[Phase] = { Phase.Burn: 3.0, Phase.Coast: 5.0 };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import mission.lib as lib;\n\
         node thrust: Dimensionless[lib.Phase] = { lib.Phase.Burn: 3.0, lib.Phase.Coast: 5.0 };\n\
         node burn: Dimensionless = @thrust[lib.Phase.Burn];\n\
         node accel: lib.GravityAccel = 9.80665 m/s^2;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    assert!((find_value(&result, "burn") - 3.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "accel") - 9.80665).abs() < f64::EPSILON);
}

fn write_same_leaf_index_project(main_source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Warm, Cold };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_same_variant_index_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_range_index_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Step = range(0.0 s, 1.0 s, step: 1.0 s);\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Step = range(0.0 s, 2.0 s, step: 1.0 s);\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_constructor_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Action { Pick(distance: Length), Idle }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Command { Pick(duration: Time), Idle }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_struct_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Pick(distance: Length), Idle }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Pick(duration: Time), Idle }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(distance: Length) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(duration: Time) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(distance: Length(min: 1.0 m)) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(duration: Time(min: 1.0 s)) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn write_same_leaf_same_field_constrained_record_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub type Item { Item(value: Length(min: 1.0 m)) }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub type Item { Item(value: Length(min: 10.0 m)) }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();
    (dir, root)
}

fn loaded_file_dag_id(
    project: &crate::loader::LoadedProject,
    file_name: &str,
) -> graphcal_compiler::dag_id::DagId {
    project
        .files
        .values()
        .find(|file| file.path.file_name().and_then(|name| name.to_str()) == Some(file_name))
        .map_or_else(
            || panic!("loaded file `{file_name}` not found"),
            |file| file.dag_id.clone(),
        )
}

#[test]
fn project_constructor_call_uses_resolved_owner_with_same_leaf_constructors() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a.Action = a.Pick(distance: 2.0 m);\n\
         node command: b.Command = b.Pick(duration: 3.0 s);\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_match_pattern_uses_resolved_constructor_and_binding() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a.Action = a.Pick(distance: 2.0 m);\n\
         node distance: Length = match @action {\n\
             a.Pick(distance: d) => d,\n\
             a.Idle => 0.0 m,\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_struct_type_uses_resolved_owner_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_struct_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a.Item = a.Pick(distance: 2.0 m);\n\
         node command: b.Item = b.Pick(duration: 3.0 s);\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_struct_type_rejects_same_leaf_wrong_owner_constructor() {
    let (_dir, root) = write_same_leaf_struct_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node bad: a.Item = b.Pick(duration: 3.0 s);\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(GraphcalError::DimensionMismatchInAnnotation { .. })) => {}
        other => panic!("expected DimensionMismatchInAnnotation, got {other:?}"),
    }
}

#[test]
fn project_field_access_uses_resolved_struct_type_def_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a.Item = a.Item(distance: 2.0 m);\n\
         node distance: Length = @item.distance;\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn eval_constructor_calls_preserve_same_leaf_struct_owners() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a.Action = a.Pick(distance: 2.0 m);\n\
         node command: b.Command = b.Pick(duration: 3.0 s);\n",
    );

    let (_tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let owner_of_struct = |name: &str| {
        let value = result
            .nodes
            .iter()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("node `{name}` failed: {e}"));
        let Value::Struct { type_name, .. } = value else {
            panic!("expected struct value for `{name}`, got {value:?}");
        };
        assert_eq!(type_name.name().as_str(), "Pick");
        type_name.resolved().clone()
    };

    assert_eq!(owner_of_struct("action").owner(), &a_id);
    assert_eq!(owner_of_struct("command").owner(), &b_id);
}

#[test]
fn eval_constructor_match_uses_resolved_owner_and_binding() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a.Action = a.Pick(distance: 2.0 m);\n\
         node command: b.Command = b.Pick(duration: 3.0 s);\n\
         node distance: Length = match @action {\n\
             a.Pick(distance: d) => d,\n\
             a.Idle => 0.0 m,\n\
         };\n\
         node duration: Time = match @command {\n\
             b.Pick(duration: t) => t,\n\
             b.Idle => 0.0 s,\n\
         };\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "distance") - 2.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "duration") - 3.0).abs() < f64::EPSILON);
}

#[test]
fn eval_field_access_uses_resolved_struct_type_def_with_same_leaf_types() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a.Item = a.Item(distance: 2.0 m);\n\
         node other: b.Item = b.Item(duration: 3.0 s);\n\
         node distance: Length = @item.distance;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "distance") - 2.0).abs() < f64::EPSILON);
}

#[test]
fn eval_constructor_match_rejects_runtime_owner_mismatch_with_same_leaf_constructor() {
    let (_dir, root) = write_same_leaf_constructor_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node action: a.Action = a.Pick(distance: 2.0 m);\n\
         node distance: Length = match @action {\n\
             a.Pick(distance: d) => d,\n\
             a.Idle => 0.0 m,\n\
         };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .resolved_decl_key_for_local(&graphcal_compiler::syntax::module_name::ScopedName::local(
            "distance",
        ))
        .expect("distance decl key");
    let expr = &tir.root().semantic.expressions.nodes[&expr_key];
    let b_owner = graphcal_compiler::syntax::names::ResolvedName::from_def(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Command"),
    );
    let mut fields = indexmap::IndexMap::new();
    fields.insert(
        graphcal_compiler::syntax::type_name::FieldName::expect_valid("distance"),
        crate::eval_expr::RuntimeValue::Quantity(9.0),
    );
    let values = HashMap::from([(
        crate::decl_key::RuntimeDeclKey::for_local_decl(
            tir.root(),
            &graphcal_compiler::syntax::module_name::ScopedName::local("action"),
        ),
        crate::eval_expr::RuntimeValue::Struct {
            type_name: graphcal_compiler::registry::declared_type::StructTypeRef::with_display_leaf(
                graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Pick"),
                b_owner,
            ),
            fields,
        },
    )]);
    let empty_locals = crate::eval_expr::HirLocalValueMap::root();
    let builtin_consts = graphcal_compiler::registry::builtins::builtin_constants();
    let builtin_fns = graphcal_compiler::registry::builtins::builtin_functions();
    let src = &project.files[&project.root].named_source;
    let ctx = crate::eval_expr::EvalContext {
        cancellation: graphcal_compiler::cancellation::CancellationToken::unbounded(),
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        tir: &tir,
        current_dag: Some(tir.root()),
        root_values: Some(&values),
        struct_field_constraints: None,
        host_fns: None,
    };

    let err = crate::eval_expr::eval_hir_expr(expr, &values, &empty_locals, &ctx).unwrap_err();
    match err {
        GraphcalError::EvalError { message, .. } => {
            assert!(message.contains("no match arm for variant"), "{message}");
        }
        other => panic!("expected EvalError, got {other:?}"),
    }
}

#[test]
fn eval_field_access_rejects_runtime_owner_mismatch_with_same_leaf_type() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a.Item = a.Item(distance: 2.0 m);\n\
         node other: b.Item = b.Item(duration: 3.0 s);\n\
         node distance: Length = @item.distance;\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .resolved_decl_key_for_local(&graphcal_compiler::syntax::module_name::ScopedName::local(
            "distance",
        ))
        .expect("distance decl key");
    let expr = &tir.root().semantic.expressions.nodes[&expr_key];
    let b_owner = graphcal_compiler::syntax::names::ResolvedName::from_def(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Item"),
    );
    let mut fields = indexmap::IndexMap::new();
    fields.insert(
        graphcal_compiler::syntax::type_name::FieldName::expect_valid("distance"),
        crate::eval_expr::RuntimeValue::Quantity(99.0),
    );
    let values = HashMap::from([(
        crate::decl_key::RuntimeDeclKey::for_local_decl(
            tir.root(),
            &graphcal_compiler::syntax::module_name::ScopedName::local("item"),
        ),
        crate::eval_expr::RuntimeValue::Struct {
            type_name: graphcal_compiler::registry::declared_type::StructTypeRef::with_display_leaf(
                graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Item"),
                b_owner,
            ),
            fields,
        },
    )]);
    let empty_locals = crate::eval_expr::HirLocalValueMap::root();
    let builtin_consts = graphcal_compiler::registry::builtins::builtin_constants();
    let builtin_fns = graphcal_compiler::registry::builtins::builtin_functions();
    let src = &project.files[&project.root].named_source;
    let ctx = crate::eval_expr::EvalContext {
        cancellation: graphcal_compiler::cancellation::CancellationToken::unbounded(),
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        tir: &tir,
        current_dag: Some(tir.root()),
        root_values: Some(&values),
        struct_field_constraints: None,
        host_fns: None,
    };

    let err = crate::eval_expr::eval_hir_expr(expr, &values, &empty_locals, &ctx).unwrap_err();
    match err {
        GraphcalError::EvalError { message, .. } => {
            assert!(message.contains("no field `distance`"), "{message}");
        }
        other => panic!("expected EvalError, got {other:?}"),
    }
}

#[test]
fn eval_struct_field_constraints_use_resolved_owner_with_same_leaf_types_and_fields() {
    let (_dir, root) = write_same_leaf_same_field_constrained_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_ok: a.Item = a.Item(value: 2.0 m);\n\
         node b_bad: b.Item = b.Item(value: 2.0 m);\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let a_ok = result
        .nodes
        .iter()
        .find(|(n, _)| n.to_string() == "a_ok")
        .expect("node a_ok")
        .1
        .as_ref();
    assert!(
        a_ok.is_ok(),
        "a_ok should satisfy a.Item's constraint: {a_ok:?}"
    );
    let b_bad = result
        .nodes
        .iter()
        .find(|(n, _)| n.to_string() == "b_bad")
        .expect("node b_bad")
        .1
        .as_ref();
    match b_bad {
        Err(NodeError::EvalFailed { message }) => {
            assert!(message.contains("minimum"), "{message}");
        }
        other => panic!("expected b_bad constraint failure, got {other:?}"),
    }
}

#[test]
fn project_declared_type_preserves_same_leaf_index_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = for p: a.Phase { 1.0 };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let src = &project.files[&project.root].named_source;
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let declared = tir.root().build_declared_types(src).unwrap();

    let graphcal_compiler::registry::declared_type::DeclaredType::Indexed { index, .. } =
        &declared[&graphcal_compiler::syntax::module_name::ScopedName::local("series")]
    else {
        panic!("expected indexed declared type for `series`");
    };
    assert_eq!(index.display_name().as_str(), "Phase");
    assert_eq!(
        index
            .declared_resolved()
            .map(graphcal_compiler::syntax::names::ResolvedName::owner,),
        Some(&a_id)
    );
}

#[test]
fn project_declared_type_preserves_same_leaf_struct_owner() {
    let (_dir, root) = write_same_leaf_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a.Item = a.Item(distance: 2.0 m);\n\
         node other: b.Item = b.Item(duration: 3.0 s);\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let src = &project.files[&project.root].named_source;
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");
    let declared = tir.root().build_declared_types(src).unwrap();

    let graphcal_compiler::registry::declared_type::DeclaredType::Struct(item, _) =
        &declared[&graphcal_compiler::syntax::module_name::ScopedName::local("item")]
    else {
        panic!("expected struct declared type for `item`");
    };
    let graphcal_compiler::registry::declared_type::DeclaredType::Struct(other, _) =
        &declared[&graphcal_compiler::syntax::module_name::ScopedName::local("other")]
    else {
        panic!("expected struct declared type for `other`");
    };
    assert_eq!(item.name().as_str(), "Item");
    assert_eq!(other.name().as_str(), "Item");
    assert_eq!(item.resolved().owner(), &a_id);
    assert_eq!(other.resolved().owner(), &b_id);
}

#[test]
fn project_struct_field_constraints_preserve_same_leaf_struct_owner() {
    let (_dir, root) = write_same_leaf_constrained_record_type_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node item: a.Item = a.Item(distance: 2.0 m);\n\
         node other: b.Item = b.Item(duration: 3.0 s);\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let src = &project.files[&project.root].named_source;
    let constraints =
        crate::exec_plan::resolve_struct_field_constraints(&tir, &HashMap::new(), src).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");

    assert!(constraints.keys().any(|key| {
        key.owning_type.resolved().owner() == &a_id
            && key.owning_type.name().as_str() == "Item"
            && key.constructor.as_str() == "Item"
            && key.field.as_str() == "distance"
    }));
    assert!(constraints.keys().any(|key| {
        key.owning_type.resolved().owner() == &b_id
            && key.owning_type.name().as_str() == "Item"
            && key.constructor.as_str() == "Item"
            && key.field.as_str() == "duration"
    }));
    assert!(
        constraints
            .keys()
            .all(|key| !key.owning_type.resolved().owner().segments().is_empty())
    );
}

#[test]
fn nat_generic_arguments_work_in_types_constructors_and_nested_expressions() {
    let source = r"
pub type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

pub type Matrix<M: Nat, N: Nat> {
    Matrix(value: Fixed<M * N + 1>),
}

param value: Matrix<2, 3> = Matrix<2, 3>(
    value: Fixed<7>(value: 1.0),
);
";

    let result = compile_and_eval(source).unwrap();
    let value = result
        .params
        .iter()
        .find(|(name, _)| name.to_string() == "value")
        .expect("value param")
        .1
        .as_ref();
    assert!(value.is_ok(), "Nat-generic value failed: {value:?}");
}

#[test]
fn sorted_generic_defaults_are_substituted_in_type_and_constructor_positions() {
    let source = r"
pub index Component = { X, Y, Z };
pub type Marker { Marker }
pub type Fixed<
    D: Dim,
    I: Index = Component,
    F: Type = Marker,
    N: Nat = 3,
> {
    Fixed(value: D),
}

pub type AllDefaults<
    D: Dim = Dimensionless,
    I: Index = Component,
    F: Type = Marker,
    N: Nat = 3,
> {
    AllDefaults(value: D),
}

param value: Fixed<Dimensionless> = Fixed<Dimensionless>(value: 1.0);
param all_defaults: AllDefaults = AllDefaults(value: 2.0);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn dependent_defaults_compose_across_all_generic_sorts() {
    let source = r"
pub index Phase = { A };
pub type Marker { Marker }
pub type Fixed<N: Nat> { Fixed(value: Dimensionless) }
pub type Bundle<
    D: Dim,
    I: Index,
    T: Type,
    N: Nat,
    E: Dim = D * D,
    J: Index = I,
    U: Type = T,
    M: Nat = N + 1,
> {
    Bundle(scalar: E, payload: U, series: E[J], fixed: Fixed<M>),
}

param value: Bundle<Length, Phase, Marker, 2> = Bundle<Length, Phase, Marker, 2>(
    scalar: 1.0 m^2,
    payload: Marker,
    series: { Phase.A: 2.0 m^2 },
    fixed: Fixed<3>(value: 1.0),
);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn dependent_nat_defaults_compose_through_nested_type_defaults() {
    let source = r"
pub type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

pub type Holder<N: Nat, M: Nat = N + 1, F: Type = Fixed<M>> {
    Holder(value: F),
}

param value: Holder<2> = Holder<2>(value: Fixed<3>(value: 1.0));
";

    compile_and_eval(source).unwrap();
}

#[test]
fn generic_defaults_may_reference_only_earlier_parameters() {
    let error = compile_and_eval(
        "pub type Invalid<N: Nat = M, M: Nat = 3> { Invalid(value: Dimensionless) }",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("default for generic parameter `N` may reference only earlier generic parameters; `M` is not earlier"),
        "unexpected error: {error}"
    );
}

#[test]
fn generic_defaults_must_form_a_trailing_parameter_suffix() {
    let error =
        compile_and_eval("pub type Invalid<N: Nat = 3, M: Nat> { Invalid(value: Dimensionless) }")
            .unwrap_err();
    assert!(
        error.to_string().contains(
            "generic parameter `M` without a default cannot follow defaulted parameter `N`"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn nested_type_generic_arguments_apply_inner_sorted_defaults() {
    let source = r"
pub type Fixed<D: Dim, N: Nat = 3> {
    Fixed(value: D),
}

pub type Holder<F: Type> {
    Holder(value: F),
}

param value: Holder<Fixed<Dimensionless>> = Holder<Fixed<Dimensionless>>(
    value: Fixed<Dimensionless>(value: 1.0),
);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn multi_declaration_supports_finite_shared_axes() {
    let source = r"
param x: Dimensionless[Fin(2)],
param y: Dimensionless[Fin(2)]
  = table[Fin(2), (_, _)] {
      : _, _;
      1.0, 2.0;
      3.0, 4.0;
  };
";
    let result = compile_and_eval(source).unwrap();
    for name in ["x", "y"] {
        let value = result
            .params
            .iter()
            .find(|(candidate, _)| candidate.to_string() == name)
            .and_then(|(_, value)| value.as_ref().ok())
            .expect("finite multi-decl param");
        assert!(matches!(value, Value::Indexed { entries, .. } if entries.len() == 2));
    }
}

#[test]
fn finite_indexes_remain_distinct_from_nat_generic_arguments() {
    let source = r"
pub type Vector<N: Nat, D: Dim> {
    Vector(values: D[Fin(N)]),
}
pub type IndexedVector<I: Index, D: Dim> {
    IndexedVector(values: D[I]),
}
pub type DefaultIndexed<I: Index = Fin(2)> {
    DefaultIndexed(values: Dimensionless[I]),
}

param defaulted: DefaultIndexed = DefaultIndexed(
    values: table[Fin(2)] { 7.0; 8.0; },
);
param vector: Vector<3, Dimensionless> = Vector<3, Dimensionless>(
    values: table[Fin(3)] { 1.0; 2.0; 3.0; },
);
param indexed: IndexedVector<Fin(3), Dimensionless> =
    IndexedVector<Fin(3), Dimensionless>(
        values: table[Fin(3)] { 4.0; 5.0; 6.0; },
    );
";

    compile_and_eval(source).unwrap();
}

#[test]
fn generic_arguments_reject_sort_crossing() {
    let cases = [
        (
            "pub type ByIndex<I: Index> { ByIndex(value: Dimensionless) }\nparam value: ByIndex<3>;",
            "expected Index, found Nat `3`",
        ),
        (
            "pub type ByNat<N: Nat> { ByNat(value: Dimensionless) }\nparam value: ByNat<Fin(3)>;",
            "sort `Nat`",
        ),
        (
            "pub index Phase = { A };\npub type ByNat<N: Nat> { ByNat(value: Dimensionless) }\nparam value: ByNat<Phase>;",
            "sort `Nat`",
        ),
        (
            "pub index Phase = { A };\npub type ByType<T: Type> { ByType(value: T) }\nparam value: ByType<Dimensionless[Phase]>;",
            "sort `Type`",
        ),
    ];

    for (source, expected) in cases {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn bare_nat_index_positions_are_rejected() {
    for source in [
        "param value: Dimensionless[3];",
        "pub type Sized<N: Nat> { Sized(values: Dimensionless[N]) }",
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains("expected Index, found Nat"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn finite_index_cardinality_is_validated_before_allocation() {
    for (source, expected) in [
        (
            "param value: Dimensionless[Fin(0)];",
            "Fin(0) is not allowed",
        ),
        (
            "param value: Dimensionless[Fin(1000001)];",
            "exceeds the practical limit",
        ),
        (
            "pub type Vector<N: Nat> { Vector(values: Dimensionless[Fin(N)]) }\nparam value: Vector<0>;",
            "Fin(0) is not allowed",
        ),
        (
            "pub type Vector<N: Nat> { Vector(values: Dimensionless[Fin(N)]) }\nparam value: Vector<1000001>;",
            "exceeds the practical limit",
        ),
        (
            "pub type InvalidDefault<I: Index = Fin(0)> { InvalidDefault(value: Dimensionless) }",
            "Fin(0) is not allowed",
        ),
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn non_empty_function_generic_arguments_are_rejected() {
    let error = compile_and_eval("node value: Dimensionless = sqrt<3>(4.0);").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("function `sqrt` does not accept generic arguments"),
        "unexpected error: {error}"
    );
}

#[test]
fn project_generic_struct_defaults_preserve_same_leaf_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    let module_source = "pub type Marker { Marker }\n\
         pub type Wrap<D: Dim, F: Type = Marker> { Wrap(value: D) }\n";
    std::fs::write(root_dir.join("a.gcl"), module_source).unwrap();
    std::fs::write(root_dir.join("b.gcl"), module_source).unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_wrap: a.Wrap<Length> = a.Wrap<Length>(value: 1.0 m);\n\
         node b_wrap: b.Wrap<Time> = b.Wrap<Time>(value: 1.0 s);\n",
    )
    .unwrap();

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");
    let marker_owner = |decl: &str| {
        let key = graphcal_compiler::syntax::module_name::ScopedName::local(decl);
        let graphcal_compiler::tir::typed::ResolvedTypeExpr::GenericStruct {
            name: wrap,
            generic_args,
            ..
        } = &tir.root().resolved_decl_types[&key]
        else {
            panic!("expected generic struct annotation for `{decl}`");
        };
        assert_eq!(wrap.as_str(), "Wrap");
        let graphcal_compiler::tir::typed::ResolvedGenericArg::Type(
            graphcal_compiler::tir::typed::ResolvedTypeExpr::Struct(marker_resolved, _),
        ) = &generic_args[1]
        else {
            panic!(
                "expected default marker type arg for `{decl}`, got {:?}",
                generic_args[1]
            );
        };
        assert_eq!(marker_resolved.as_str(), "Marker");
        marker_resolved.owner().clone()
    };

    assert_eq!(marker_owner("a_wrap"), a_id);
    assert_eq!(marker_owner("b_wrap"), b_id);
}

#[test]
fn project_index_access_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = for p: a.Phase { 1.0 };\n\
         node burn: Dimensionless = @series[a.Phase.Burn];\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_index_access_rejects_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = for p: a.Phase { 1.0 };\n\
         node bad: Dimensionless = @series[b.Phase.Warm];\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(GraphcalError::IndexMismatch { .. })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_for_comp_rejects_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = for p: b.Phase { 1.0 };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(GraphcalError::DimensionMismatchInAnnotation { .. })) => {}
        other => panic!("expected DimensionMismatchInAnnotation, got {other:?}"),
    }
}

#[test]
fn project_map_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = {\n\
             a.Phase.Burn: 1.0,\n\
             a.Phase.Coast: 2.0,\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_map_literal_rejects_same_leaf_wrong_owner_key() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = {\n\
             a.Phase.Burn: 1.0,\n\
             b.Phase.Warm: 2.0,\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(GraphcalError::IndexMismatch { .. })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_map_literal_missing_variants_uses_resolved_owner() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = {\n\
             a.Phase.Burn: 1.0,\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(GraphcalError::MissingVariants { missing, .. })) => {
            assert_eq!(missing.len(), 1);
            assert_eq!(
                missing[0]
                    .as_named()
                    .expect("Phase is a named index")
                    .as_str(),
                "Coast"
            );
        }
        other => panic!("expected MissingVariants, got {other:?}"),
    }
}

#[test]
fn project_table_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = table[a.Phase] {\n\
             Burn: 1.0;\n\
             Coast: 2.0;\n\
         };\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_variant_literal_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = {\n\
             a.Phase.Burn: 1.0,\n\
             a.Phase.Coast: 2.0,\n\
         };\n\
         node burn: Dimensionless = @series[a.Phase.Burn];\n",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn project_label_match_uses_resolved_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a.Phase] = for p: a.Phase {\n\
             match p {\n\
                 a.Phase.Burn => 1.0,\n\
                 a.Phase.Coast => 2.0,\n\
             }\n\
         };\n\
         node burn_code: Dimensionless = @code[a.Phase.Burn];\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "burn_code") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn project_label_match_rejects_same_leaf_wrong_owner_pattern() {
    let (_dir, root) = write_same_leaf_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a.Phase] = for p: a.Phase {\n\
             match p {\n\
                 a.Phase.Burn => 1.0,\n\
                 b.Phase.Warm => 2.0,\n\
             }\n\
         };\n",
    );

    match compile_to_tir_project(&root, None, &fs()) {
        Err(CompileError::Eval(GraphcalError::IndexMismatch { .. })) => {}
        other => panic!("expected IndexMismatch, got {other:?}"),
    }
}

#[test]
fn project_expected_fail_keys_accept_resolved_index_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_checks: Bool[a.Phase] = {\n\
             a.Phase.Burn: false,\n\
             a.Phase.Coast: true,\n\
         };\n\
         #[expected_fail(a.Phase.Burn)]\n\
         assert a_expected = @a_checks;\n\
         node b_checks: Bool[b.Phase] = {\n\
             b.Phase.Burn: false,\n\
             b.Phase.Coast: true,\n\
         };\n\
         #[expected_fail(b.Phase.Burn)]\n\
         assert b_expected = @b_checks;\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let assert_result = |name: &str| {
        result
            .assertions
            .iter()
            .find(|(assert_name, _, _)| assert_name.to_string() == name)
            .unwrap_or_else(|| panic!("assertion `{name}` not found"))
            .1
            .clone()
    };
    assert_eq!(assert_result("a_expected"), AssertResult::Pass);
    assert_eq!(assert_result("b_expected"), AssertResult::Pass);
}

#[test]
fn project_expected_fail_keys_reject_same_leaf_wrong_owner() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node b_checks: Bool[b.Phase] = {\n\
             b.Phase.Burn: false,\n\
             b.Phase.Coast: true,\n\
         };\n\
         #[expected_fail(a.Phase.Burn)]\n\
         assert b_expected = @b_checks;\n",
    );

    match compile_and_eval_project(&root, &HashMap::new(), None, &fs()) {
        Err(CompileError::Eval(GraphcalError::ExpectedFailKeyIndexMismatch { .. })) => {}
        other => {
            panic!(
                "expected ExpectedFailKeyIndexMismatch for foreign expected_fail key, got {other:?}"
            )
        }
    }
}

#[test]
fn eval_index_collections_preserve_same_leaf_owners_across_runtime_boundaries() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a.{ Phase };\n\
         import collide.a as a;\n\
         import collide.b as b;\n\
         dag pick_a {\n\
             import collide.a.{ Phase };\n\
             param series: Dimensionless[Phase];\n\
             pub node burn: Dimensionless = @series[Phase.Burn];\n\
             pub node echoed: Dimensionless[Phase] = for p: Phase { @series[p] };\n\
         }\n\
         node map_a: Dimensionless[a.Phase] = {\n\
             a.Phase.Burn: 10.0,\n\
             a.Phase.Coast: 20.0,\n\
         };\n\
         node map_b: Dimensionless[b.Phase] = {\n\
             b.Phase.Burn: 1.0,\n\
             b.Phase.Coast: 2.0,\n\
         };\n\
         node table_a: Dimensionless[Phase] = table[Phase] {\n\
             Burn: 3.0;\n\
             Coast: 4.0;\n\
         };\n\
         node for_a: Dimensionless[a.Phase] = for p: a.Phase {\n\
             match p {\n\
                 a.Phase.Burn => @map_a[p],\n\
                 a.Phase.Coast => @pick_a(series: @map_a).echoed[p],\n\
             }\n\
         };\n\
         node scan_a: Dimensionless[a.Phase] = scan(@map_a, 0.0, |acc, val| acc + val);\n\
         node total: Dimensionless = @pick_a(series: @map_a).burn\n\
             + @table_a[Phase.Burn]\n\
             + @for_a[a.Phase.Coast]\n\
             + @scan_a[a.Phase.Coast];\n",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "total") - 63.0).abs() < f64::EPSILON);

    let owner_of_indexed = |name: &str| {
        let value = result
            .nodes
            .iter()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("node `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("node `{name}` failed: {e}"));
        let Value::Indexed { index_name, .. } = value else {
            panic!("expected indexed value for `{name}`, got {value:?}");
        };
        index_name
            .declared_resolved()
            .cloned()
            .unwrap_or_else(|| panic!("expected declared index for `{name}`"))
    };

    let a_owner = owner_of_indexed("map_a");
    let b_owner = owner_of_indexed("map_b");
    assert_ne!(a_owner, b_owner);
    assert_eq!(owner_of_indexed("table_a"), a_owner);
    assert_eq!(owner_of_indexed("for_a"), a_owner);
    assert_eq!(owner_of_indexed("scan_a"), a_owner);
}

#[test]
fn eval_unfold_uses_resolved_explicit_range_index_owner_with_same_leaf_indexes() {
    let (_dir, root) = write_same_leaf_range_index_project(
        "import collide.b as b;\n\
         import collide.a as a;\n\
         node y: Dimensionless[a.Step] = unfold(a.Step, 0.0, |prev_y, prev_t, t| prev_y + 1.0);\n",
    );

    let (_tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_owner = loaded_file_dag_id(&project, "a.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let value = result
        .nodes
        .iter()
        .find(|(name, _)| name.to_string() == "y")
        .expect("node y")
        .1
        .as_ref()
        .expect("node y value");
    let Value::Indexed {
        index_name,
        entries,
        ..
    } = value
    else {
        panic!("expected indexed value for `y`, got {value:?}");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(
        index_name
            .declared_resolved()
            .map(graphcal_compiler::syntax::names::ResolvedName::owner),
        Some(&a_owner)
    );
}

#[test]
fn eval_index_access_rejects_runtime_owner_mismatch_with_same_leaf_variant() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series: Dimensionless[a.Phase] = {\n\
             a.Phase.Burn: 1.0,\n\
             a.Phase.Coast: 2.0,\n\
         };\n\
         node burn: Dimensionless = @series[a.Phase.Burn];\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .resolved_decl_key_for_local(&graphcal_compiler::syntax::module_name::ScopedName::local(
            "burn",
        ))
        .expect("burn decl key");
    let expr = &tir.root().semantic.expressions.nodes[&expr_key];
    let b_owner = graphcal_compiler::syntax::names::ResolvedName::from_def(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Phase"),
    );
    let mut entries = indexmap::IndexMap::new();
    entries.insert(
        graphcal_compiler::syntax::index_name::IndexEntryKey::named(
            graphcal_compiler::syntax::index_name::IndexVariantName::expect_valid("Burn"),
        ),
        crate::eval_expr::RuntimeValue::Quantity(99.0),
    );
    entries.insert(
        graphcal_compiler::syntax::index_name::IndexEntryKey::named(
            graphcal_compiler::syntax::index_name::IndexVariantName::expect_valid("Coast"),
        ),
        crate::eval_expr::RuntimeValue::Quantity(100.0),
    );
    let values = HashMap::from([(
        crate::decl_key::RuntimeDeclKey::for_local_decl(
            tir.root(),
            &graphcal_compiler::syntax::module_name::ScopedName::local("series"),
        ),
        crate::eval_expr::RuntimeValue::Indexed {
            index_name: graphcal_compiler::registry::declared_type::IndexTypeRef::from_resolved(
                b_owner,
            ),
            entries,
        },
    )]);
    let empty_locals = crate::eval_expr::HirLocalValueMap::root();
    let builtin_consts = graphcal_compiler::registry::builtins::builtin_constants();
    let builtin_fns = graphcal_compiler::registry::builtins::builtin_functions();
    let src = &project.files[&project.root].named_source;
    let ctx = crate::eval_expr::EvalContext {
        cancellation: graphcal_compiler::cancellation::CancellationToken::unbounded(),
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        tir: &tir,
        current_dag: Some(tir.root()),
        root_values: Some(&values),
        struct_field_constraints: None,
        host_fns: None,
    };

    let err = crate::eval_expr::eval_hir_expr(expr, &values, &empty_locals, &ctx).unwrap_err();
    match err {
        GraphcalError::EvalError { message, .. } => {
            assert!(message.contains("index argument belongs to"), "{message}");
        }
        other => panic!("expected EvalError, got {other:?}"),
    }
}

#[test]
fn eval_label_match_rejects_runtime_owner_mismatch_with_same_leaf_variant() {
    let (_dir, root) = write_same_leaf_same_variant_index_project(
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node code: Dimensionless[a.Phase] = for p: a.Phase {\n\
             match p {\n\
                 a.Phase.Burn => 1.0,\n\
                 a.Phase.Coast => 2.0,\n\
             }\n\
         };\n",
    );

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let expr_key = tir
        .root()
        .resolved_decl_key_for_local(&graphcal_compiler::syntax::module_name::ScopedName::local(
            "code",
        ))
        .expect("code decl key");
    let expr = &tir.root().semantic.expressions.nodes[&expr_key];
    let graphcal_compiler::hir::ExprKind::ForComp { bindings, body } = &expr.kind else {
        panic!("expected `code` to be a for-comprehension, got {expr:?}");
    };
    let [binding] = bindings.as_slice() else {
        panic!("expected one for-comprehension binding, got {bindings:?}");
    };
    let match_expr = body;
    let b_owner = graphcal_compiler::syntax::names::ResolvedName::from_def(
        loaded_file_dag_id(&project, "b.gcl"),
        graphcal_compiler::syntax::index_name::IndexName::expect_valid("Phase"),
    );
    let values = HashMap::new();
    let local_values = crate::eval_expr::HirLocalValueMap::from_bindings(vec![(
        binding.local.id,
        crate::eval_expr::RuntimeValue::Label {
            index_name: graphcal_compiler::registry::declared_type::IndexTypeRef::from_resolved(
                b_owner,
            ),
            variant: graphcal_compiler::syntax::index_name::IndexVariantName::expect_valid("Burn"),
        },
    )]);
    let builtin_consts = graphcal_compiler::registry::builtins::builtin_constants();
    let builtin_fns = graphcal_compiler::registry::builtins::builtin_functions();
    let src = &project.files[&project.root].named_source;
    let ctx = crate::eval_expr::EvalContext {
        cancellation: graphcal_compiler::cancellation::CancellationToken::unbounded(),
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        tir: &tir,
        current_dag: Some(tir.root()),
        root_values: Some(&values),
        struct_field_constraints: None,
        host_fns: None,
    };

    let err =
        crate::eval_expr::eval_hir_expr(match_expr, &values, &local_values, &ctx).unwrap_err();
    match err {
        GraphcalError::EvalError { message, .. } => {
            assert!(message.contains("no match arm for label"), "{message}");
        }
        other => panic!("expected EvalError, got {other:?}"),
    }
}

// ---- Bare module path eval tests ----
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
                .find(|(n, _, _)| n.to_string() == "z")
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

// --- Partial overrides / partial bindings tests ---

#[test]
fn cli_partial_override_uses_defaults() {
    // When overrides are provided for some params, the rest fall back to defaults.
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("isp"), parse_expr("450.0 s"));
    let result = compile_and_eval_with_overrides(source, "test.gcl", &overrides);
    assert!(
        result.is_ok(),
        "partial overrides should fall back to defaults: {result:?}"
    );
}

#[test]
fn import_partial_binding_uses_defaults() {
    // Parameterized import with partial binding falls back to defaults.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        result.is_ok(),
        "partial import binding should fall back to defaults: {result:?}"
    );
}

// --- Required param (no default) import tests ---

#[test]
fn project_required_param_import() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/required_param_import/src/library/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
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
        .join("../../tests/fixtures/invalid/multi/injectable_index_kind_mismatch/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
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
fn project_injectable_index_basic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/injectable_index_basic/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
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
        .join("../../tests/fixtures/valid/multi/instantiated_import_type_binding/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // origin_size = 1.0 m (the lib's `Widget { size: 1.0 m }` rewritten to
    // `MyWidget { size: 1.0 m }` after type substitution)
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 1.0).abs() < 1e-10,
        "result = {result_val}, expected 1.0"
    );
}

#[test]
fn project_instantiated_import_dim_binding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_dim_binding/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // result = 10.0 m/s; the lib's `v: Speed = 10.0 m/s` has its type_ann
    // rewritten Speed -> Velocity so main's Velocity dimension resolves.
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 10.0).abs() < 1e-10,
        "result = {result_val}, expected 10.0"
    );
}

#[test]
fn project_pub_import_reexport_selective() {
    // Issue #452: selective `import "X" { pub item }` re-exports the
    // item at the importer's visible surface, so a transitive importer
    // can reach it via the intermediate file.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/pub_import_reexport_selective/src/middle/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // result = 9.80665 m/s^2 (in the base unit, value 9.80665).
    let result_val = find_value(&result, "result");
    assert!(
        (result_val - 9.806_65).abs() < 1e-10,
        "result = {result_val}, expected 9.80665"
    );
}

#[test]
fn project_include_overrides_index_no_param_binding_v005() {
    // V005: overriding `Phase` orphans the `cost` default (which mentions
    // `Phase.Design` / `Phase.Build`) because the importer forgot to
    // re-bind `cost` in the same include statement.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/include_overrides_index_no_param_binding/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(GraphcalError::IncludeMustReconcileOverride {
            overridden,
            overridden_kind,
            orphan_decl,
            ..
        })) => {
            assert_eq!(overridden, "Phase");
            assert_eq!(overridden_kind, "index");
            assert_eq!(orphan_decl, "cost");
        }
        other => panic!("expected IncludeMustReconcileOverride, got {other:?}"),
    }
}

#[test]
fn project_include_overrides_index_with_param_binding_ok() {
    // Positive companion to project_include_overrides_index_no_param_binding_v005:
    // supplying a fresh `cost` binding satisfies A8.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/multi/include_overrides_index_with_param_binding/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let result_val = find_value(&result, "result");
    // total = 10 + 20 = 30
    assert!(
        (result_val - 30.0).abs() < 1e-10,
        "result = {result_val}, expected 30.0"
    );
}

#[test]
fn merged_dependency_body_error_renders_against_dependency_source() {
    // #868: a dimension mismatch introduced by an instantiated include's
    // dimension rebinding lives in the *dependency* body, which keeps the
    // dependency file's byte offsets. The D002 diagnostic must render against
    // the dependency source (`lib.gcl`) with an in-bounds span — before the
    // fix it was rendered against the importer (`main.gcl`), yielding an
    // out-of-bounds label.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/invalid/multi/merged_dep_body_dim_mismatch/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(GraphcalError::DimensionMismatchInAnnotation {
            declared,
            inferred,
            src,
            span,
        })) => {
            assert_eq!(declared, "Mass");
            assert_eq!(inferred, "Velocity");
            assert!(
                src.name().ends_with("lib.gcl"),
                "diagnostic must name the dependency file, got `{}`",
                src.name()
            );
            // The span must index into the source it renders against.
            let len = src.inner().len();
            assert!(
                span.offset() + span.len() <= len,
                "span (offset {}, len {}) is out of bounds for `{}` of length {len}",
                span.offset(),
                span.len(),
                src.name(),
            );
        }
        other => panic!("expected D002 against the dependency source, got {other:?}"),
    }
}

#[test]
fn project_pub_include_leaks_private_type_v006() {
    // V006: `pub include` re-exports container's `origin` decl whose
    // signature (post-substitution) names `PrivateInner`, which is a
    // private-local type at the importer.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/pub_include_leaks_private_type/src/container/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(GraphcalError::GenericsLeakage {
            reexport_name,
            leaked_name,
            leaked_kind,
            ..
        })) => {
            assert_eq!(reexport_name, "origin");
            assert_eq!(leaked_name, "PrivateInner");
            assert_eq!(leaked_kind, "type");
        }
        other => panic!("expected GenericsLeakage, got {other:?}"),
    }
}

#[test]
fn project_pub_include_with_public_type_binding_ok() {
    // Positive companion to project_pub_include_leaks_private_type_v006:
    // binding `Element` to a `pub` importer-local type satisfies A9.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/pub_include_with_public_type_binding/src/container/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        result.is_ok(),
        "`pub include` re-exporting a `pub` type binding should compile: {result:?}"
    );
}

#[test]
fn project_injectable_index_expected_fail() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/injectable_index_expected_fail/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    // The within_limit assertion should pass overall because Overdrive is marked expected_fail.
    let assert_result = result
        .assertions
        .iter()
        .find(|(name, _, _)| name.to_string().contains("within_limit"))
        .expect("within_limit assertion not found");
    assert!(
        matches!(assert_result.1, AssertResult::Pass),
        "expected Pass, got {:?}",
        assert_result.1
    );
}

// ---- Inline DAG tests (Phase 5) ----

fn setup_inline_semantics_project(
    files: &[(&str, &str)],
    root_rel: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"sem\"\n",
    )
    .expect("write manifest");
    for (rel, source) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture parent");
        }
        std::fs::write(path, source).expect("write fixture source");
    }
    let root = dir.path().join(root_rel);
    (dir, root)
}

#[test]
fn inline_dag_required_coordinate_index_rejects_dimension_mismatch() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include pass_through(
    Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(GraphcalError::IndexBindingDimensionMismatch {
            dep_index,
            expected_dim,
            bound_index,
            found_dim,
            ..
        })) => {
            assert_eq!(dep_index, "Step");
            assert_eq!(expected_dim, "Time");
            assert_eq!(bound_index, "DistanceStep");
            assert_eq!(found_dim, "Length");
        }
        other => panic!("expected IndexBindingDimensionMismatch, got {other:?}"),
    }
}

#[test]
fn inline_dag_required_coordinate_index_rejects_named_index() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

pub index Phase = { Start, End };
include pass_through(
    Step: Phase,
    samples: {
        Phase.Start: 1.0 m,
        Phase.End: 2.0 m,
    },
) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(GraphcalError::IndexKindMismatch {
            dep_index,
            dep_kind,
            bound_index,
            bound_kind,
            ..
        })) => {
            assert_eq!(dep_index, "Step");
            assert_eq!(dep_kind, "coordinate");
            assert_eq!(bound_index, "Phase");
            assert_eq!(bound_kind, "named");
        }
        other => panic!("expected IndexKindMismatch, got {other:?}"),
    }
}

#[test]
fn inline_dag_required_index_missing_binding_uses_index_diagnostic() {
    let source = r"
dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}

include pass_through(samples: 1.0 m) as output;
";

    let result = compile_and_eval(source);
    match result {
        Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound { name, .. })) => {
            assert_eq!(name, "Step");
        }
        other => panic!("expected RequiredIndexNotBound, got {other:?}"),
    }
}

#[test]
fn inline_dag_coordinate_contract_applies_simultaneous_dimension_binding() {
    let source = r"
dag pass_through {
    pub(bind) dim AxisDim;
    pub(bind) index Step: AxisDim;
    param samples: AxisDim[Step];
    pub node result: AxisDim[Step] = @samples;
}

pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include pass_through(
    AxisDim: Time,
    Step: TimeStep,
    samples: for time: TimeStep { time },
) as output;
";

    compile_and_eval(source).unwrap();
}

#[test]
fn nested_dag_can_forward_compatible_required_coordinate_index() {
    let source = r"
dag inner {
    pub(bind) index InnerStep: Time;
    pub node n: Int = count(for step: InnerStep { step });
}

dag outer {
    pub(bind) index OuterStep: Time;
    include inner(InnerStep: OuterStep) as inner_run;
    pub node n: Int = 1;
}

pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include outer(OuterStep: TimeStep) as output;
";

    compile_and_eval(source).unwrap();
}

#[test]
fn file_dag_coordinate_contract_applies_simultaneous_dimension_binding() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                r"
pub(bind) dim AxisDim;
pub(bind) index Step: AxisDim;
param samples: AxisDim[Step];
pub node result: AxisDim[Step] = @samples;
",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
include sem.library(
    AxisDim: Time,
    Step: TimeStep,
    samples: for time: TimeStep { time },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
}

#[test]
fn cross_file_inline_dag_rejects_coordinate_dimension_mismatch() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                r"
pub dag pass_through {
    pub(bind) index Step: Time;
    param samples: Length[Step];
    pub node result: Length[Step] = @samples;
}
",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include sem.library.pass_through(
    Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        matches!(
            result,
            Err(CompileError::Eval(
                GraphcalError::IndexBindingDimensionMismatch { .. }
            ))
        ),
        "expected coordinate dimension mismatch, got {result:?}"
    );
}

#[test]
fn file_dag_index_dimension_diagnostic_uses_importer_source_span() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/library.gcl",
                "pub(bind) index Step: Time;\nparam samples: Length[Step];\n",
            ),
            (
                "src/sem/main.gcl",
                r"
pub index DistanceStep = range(0.0 m, 2.0 m, step: 1.0 m);
include sem.library(
    Step: DistanceStep,
    samples: for distance: DistanceStep { distance },
) as output;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    let diagnostic: &dyn miette::Diagnostic = &error;
    let mut rendered = String::new();
    miette::NarratableReportHandler::new()
        .render_report(&mut rendered, diagnostic)
        .unwrap();

    assert!(rendered.contains("Step: DistanceStep"), "{rendered}");
    assert!(!rendered.contains("OutOfBounds"), "{rendered}");
}

#[test]
fn inline_dag_rejects_parent_type_without_import() {
    let source = "\
pub dim Speed = Length / Time;

// `Speed` is deliberately not imported by the DAG body.
dag analyze {
    param v: Speed;
    pub node out: Speed = @v;
}

param input: Speed = 1.0 m/s;
node result: Speed = @analyze(v: @input).out;
";

    let result = compile_and_eval_named(source, "test.gcl");
    assert!(
        result.is_err(),
        "inline DAG body should not inherit parent type-system names: {result:?}"
    );
}

#[test]
fn inline_dag_body_import_drives_dependency_loading() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 3.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
dag scaled {
    import sem.lib.{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}
node result: Dimensionless = @scaled(x: 2.0).out;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let project = crate::loader::load_project(&root, None, &fs()).unwrap();
    assert_eq!(
        project.files.len(),
        2,
        "inline DAG body import should load its dependency"
    );
}

#[test]
fn inline_dag_body_value_import_is_available_to_call() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 3.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
import sem.lib as lib;

dag scaled {
    import sem.lib.{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}
node result: Dimensionless = @scaled(x: 2.0).out;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 6.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn include_inside_inline_dag_body_is_available_to_call() {
    let source = "\
dag inner {
    param x: Dimensionless;
    pub node y: Dimensionless = @x + 1.0;
}

dag outer {
    param z: Dimensionless;
    include inner(x: @z).{ y };
    pub node out: Dimensionless = @y + 1.0;
}

node result: Dimensionless = @outer(z: 2.0).out;
";

    let result = compile_and_eval(source).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 4.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn nested_inline_dag_is_compilable_from_parent_body() {
    let source = "\
dag outer {
    dag inner {
        pub node result: Dimensionless = 4.0;
    }
    pub node out: Dimensionless = @inner().result + 1.0;
}

node result: Dimensionless = @outer().out;
";

    let result = compile_and_eval(source).unwrap();
    let value = find_value(&result, "result");
    assert!((value - 5.0).abs() < 1e-10, "result = {value}");
}

#[test]
fn inline_dag_include_and_call_share_body_import_semantics() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "pub const node scale: Dimensionless = 2.0;\n",
            ),
            (
                "src/sem/main.gcl",
                "\
dag scaled {
    import sem.lib.{ scale };
    param x: Dimensionless;
    pub node out: Dimensionless = @x * @scale;
}

include scaled(x: 3.0).{ out as included };
node called: Dimensionless = @scaled(x: 3.0).out;
node difference: Dimensionless = @included - @called;
",
            ),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let included = find_value(&result, "included");
    let called = find_value(&result, "called");
    let difference = find_value(&result, "difference");
    assert!((included - 6.0).abs() < 1e-10, "included = {included}");
    assert!((called - 6.0).abs() < 1e-10, "called = {called}");
    assert!(difference.abs() < 1e-10, "difference = {difference}");
}

#[test]
fn inline_dag_include_selected_adt_output_uses_producer_scope() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}

pub dag make_payload {
    import sem.lib.{ type Payload, Payload };

    pub node out: Payload = Payload(x: 1.0);
}
",
            ),
            (
                "src/sem/main.gcl",
                "include sem.lib.make_payload().{ out };\n",
            ),
        ],
        "src/sem/main.gcl",
    );

    compile_to_tir_project(&root, None, &fs()).unwrap();
}

#[test]
fn inline_dag_include_adt_param_default_uses_producer_scope() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}

pub dag use_default {
    import sem.lib.{ type Payload, Payload };

    param p: Payload = Payload(x: 1.0);
    pub node y: Dimensionless = @p.x;
}
",
            ),
            ("src/sem/main.gcl", "include sem.lib.use_default().{ y };\n"),
        ],
        "src/sem/main.gcl",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let y = find_value(&result, "y");
    assert!((y - 1.0).abs() < 1e-10, "y = {y}");
}

#[test]
fn consumer_still_needs_import_for_explicit_adt_names() {
    let (_dir, root) = setup_inline_semantics_project(
        &[
            (
                "src/sem/lib.gcl",
                "\
pub type Payload {
    Payload(x: Dimensionless),
}
",
            ),
            (
                "src/sem/main.gcl",
                "\
import sem.lib;
node p: Payload = Payload(x: 1.0);
",
            ),
        ],
        "src/sem/main.gcl",
    );

    assert!(
        compile_to_tir_project(&root, None, &fs()).is_err(),
        "consumer-written Payload names must still require an import"
    );
}

#[test]
fn inline_dag_basic_selective() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/inline_dag_basic/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let val = find_value(&result, "final_result");
    assert!((val - 20.0).abs() < 1e-10, "expected 20.0, got {val}");
}

#[test]
fn inline_dag_include_selects_effective_param_value() {
    let source = "\
dag default_dag {
    param x: Dimensionless = 1.0;
}

dag bound_dag {
    param x: Dimensionless = 1.0;
}

include default_dag().{ x as default_x };
include bound_dag(x: 2.0).{ x as bound_x };
node sum: Dimensionless = @default_x + @bound_x;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "default_x") - 1.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_x") - 2.0).abs() < 1e-10);
    assert!((find_value(&result, "sum") - 3.0).abs() < 1e-10);
}

#[test]
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "Graphcal source uses `{result}` as a brace-list selector, not a format arg"
)]
fn inline_dag_recursive_error() {
    // Direct recursion: dag includes itself.
    let source = r"
dag recursive {
    param x: Dimensionless;
    include recursive(x: 1.0).{result};
    node result: Dimensionless = @x;
}
include recursive(x: 1.0).{result};
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
dag add_velocities {
    param a: Velocity;
    param b: Velocity;
    node sum: Velocity = @a + @b;
}

param v1: Velocity = 10.0 m/s;
param v2: Velocity = 5.0 m/s;
include add_velocities(a: @v1, b: @v2).{sum as total};
node result: Velocity = @total;
";
    let result = compile_and_eval(source).unwrap();
    let val = find_value(&result, "result");
    assert!((val - 15.0).abs() < 1e-10, "expected 15.0, got {val}");
}

// ---- Cross-file DAG tests (Phase 6.10) ----
// ---- Cross-file qualified inline dag calls (issue #467) ----#[test]#[test]// ---- Bare module path DAG reference tests ----
// ---- Inline DAG invocation (issue #451) ----

#[test]
fn eval_inline_dag_call_basic() {
    let source = "\
dag scale {
    param factor: Dimensionless;
    param v: Length;
    pub node result: Length = @v * @factor;
}

param src: Length = 10.0 m;
node doubled: Length = @scale(factor: 2.0, v: @src).result;
";
    let result = compile_and_eval(source).unwrap();
    let doubled = find_value(&result, "doubled");
    assert!(
        (doubled - 20.0).abs() < 1e-10,
        "expected 20.0, got {doubled}"
    );
}

#[test]
fn eval_inline_dag_call_projects_effective_param_value() {
    let source = "\
dag config {
    param factor: Dimensionless = 2.0;
}

node default_factor: Dimensionless = @config().factor;
node bound_factor: Dimensionless = @config(factor: 3.0).factor;
";
    let result = compile_and_eval(source).unwrap();
    assert!((find_value(&result, "default_factor") - 2.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_factor") - 3.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_chains_through_body_nodes() {
    // An inline call where the dag body has an intermediate node; tests that
    // earlier nodes are evaluated and visible to later ones.
    let source = "\
dag two_step {
    param v: Length;
    node mid: Length = @v * 2.0;
    pub node result: Length = @mid + 1.0 m;
}

param src: Length = 3.0 m;
node out: Length = @two_step(v: @src).result;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    // (3 * 2) + 1 = 7
    assert!((out - 7.0).abs() < 1e-10, "expected 7.0, got {out}");
}

#[test]
fn eval_inline_dag_call_imports_parent_const_with_alias() {
    let source = "\
pub const node seed_len: Length = 3.0 m;

dag scaled {
    import test.{seed_len as imported_seed};

    param factor: Dimensionless;
    pub node result: Length = @imported_seed * @factor;
}

node out: Length = @scaled(factor: 4.0).result;
";
    let result = compile_and_eval_named(source, "test.gcl").unwrap();
    let out = find_value(&result, "out");
    assert!((out - 12.0).abs() < 1e-10, "expected 12.0, got {out}");
}

#[test]
fn eval_qualified_inline_dag_call_imports_parent_const_with_alias() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/inline_dag_call_cross_file_parent_const/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let earth_half = find_value(&result, "earth_half");
    assert!(
        (earth_half - 3_185_500.0).abs() < 1e-10,
        "expected 3185500.0, got {earth_half}"
    );
    assert!((find_value(&result, "default_factor") - 1.0).abs() < 1e-10);
    assert!((find_value(&result, "bound_factor") - 0.5).abs() < 1e-10);
}

#[test]
fn imported_file_and_inline_dag_aliases_are_both_directly_callable() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/callable_module_aliases/src/callable/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    assert!((find_value(&result, "file_result") - 4.0).abs() < 1e-10);
    assert!((find_value(&result, "file_param_result") - 7.0).abs() < 1e-10);
    assert!((find_value(&result, "inline_result") - 9.0).abs() < 1e-10);
    assert!((find_value(&result, "selected_result") - 12.0).abs() < 1e-10);
    assert!((find_value(&result, "unaliased_file_result") - 10.0).abs() < 1e-10);
    assert!((find_value(&result, "unaliased_inline_result") - 18.0).abs() < 1e-10);
    assert!((find_value(&result, "nested_scope_result") - 40.0).abs() < 1e-10);
}

#[test]
fn absolute_inline_call_path_reports_imported_name_diagnostic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/callable_module_alias_absolute_path/src/callable/main.gcl",
    );
    let err = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap_err();
    assert!(matches!(
        err,
        CompileError::Eval(GraphcalError::UnknownModule { name, .. }) if name == "callable"
    ));
}

#[test]
fn eval_inline_dag_namespace_alias_at_field() {
    // Issue #518: `include foo() as bar; @bar.member` was N002.
    // Two instances confirm distinct namespaces.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/inline_dag_namespace/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let doubled = find_value(&result, "doubled_result");
    let tripled = find_value(&result, "tripled_result");
    assert!((doubled - 20.0).abs() < 1e-10, "doubled = {doubled}");
    assert!((tripled - 30.0).abs() < 1e-10, "tripled = {tripled}");
}

#[test]
fn eval_cross_file_include_namespace_alias_at_field() {
    // Issue #518: `include path(...) as alias; @alias.member` across files.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/instantiated_import_module/src/rocket/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let dv = find_value(&result, "dv");
    assert!(dv > 0.0, "dv should be positive, got {dv}");
}

#[test]
fn eval_import_namespace_alias_at_field() {
    // Issue #518: `import path as alias; @alias.const_member` across files.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/module_import_alias/src/constants/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let g = find_value(&result, "g");
    assert!((g - 9.806_65).abs() < 1e-10, "g = {g}");
}

#[test]
fn eval_qualified_const_refs_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub const node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub const node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         const node combined: Dimensionless = @a.shared + @b.shared;\n\
         const node shared: Dimensionless = @combined + 1.0;\n\
         node out: Dimensionless = @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 6.0).abs() < 1e-10, "out = {out}");
}

#[test]
fn eval_included_struct_array_field_access_uses_imported_type_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/repro");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"repro\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("types.gcl"),
        "pub type Item {\n    Item(mass: Mass),\n}\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("data.gcl"),
        "import repro.types.{ type Item, Item };\n\
         pub index ItemId = { Only };\n\
         pub node items: Item[ItemId] = {\n\
             ItemId.Only: Item(mass: 1.0 kg),\n\
         };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import repro.types.{ type Item };\n\
         import repro.data.{ ItemId };\n\
         include repro.data().{ items };\n\
         node first_mass: Mass = @items[ItemId.Only].mass;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let first_mass = find_value(&result, "first_mass");
    assert!(
        (first_mass - 1.0).abs() < 1e-10,
        "first_mass = {first_mass}"
    );
}

#[test]
fn eval_qualified_runtime_refs_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a() as a;\n\
         include collide.b() as b;\n\
         node total: Dimensionless = @a.shared + @b.shared;\n\
         node shared: Dimensionless = @total + 1.0;\n\
         node out: Dimensionless = @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 6.0).abs() < 1e-10, "out = {out}");
}

#[test]
fn eval_qualified_params_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "param shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "param shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a() as a;\n\
         include collide.b() as b;\n\
         param shared: Dimensionless = 100.0;\n\
         node total: Dimensionless = @a.shared + @b.shared + @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 105.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_selective_import_aliases_with_colliding_leaf_names() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub const node shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub const node shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a.{ shared as a_shared };\n\
         import collide.b.{ shared as b_shared };\n\
         const node shared: Dimensionless = 100.0;\n\
         node total: Dimensionless = @a_shared + @b_shared + @shared;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 105.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_overrides_route_selective_same_leaf_params_by_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "param shared: Dimensionless = 2.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "param shared: Dimensionless = 3.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.a().{ shared as a_shared };\n\
         include collide.b().{ shared as b_shared };\n\
         node total: Dimensionless = @a_shared + @b_shared;\n",
    )
    .unwrap();

    let mut overrides = HashMap::new();
    overrides.insert(DeclName::expect_valid("a_shared"), parse_expr("20.0"));
    overrides.insert(DeclName::expect_valid("b_shared"), parse_expr("30.0"));
    let result = compile_and_eval_project(&root, &overrides, None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 50.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_include_dep_with_aliased_module_import() {
    // End-to-end coverage for including a dep that itself imports another
    // module under an alias (`import lib as mission;` + `@mission.C`).
    // The merge must keep the dep's qualified imported-value keys intact
    // (see `merge_dependency_keeps_qualified_imported_value_keys` in
    // graphcal-compiler for the unit-level regression test).
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("lib.gcl"),
        "pub const node C: Dimensionless = 7.0;\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("dep.gcl"),
        "import collide.lib as mission;\n\
         pub node out: Dimensionless = @mission.C * 2.0;\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include collide.dep().{ out as dep_out };\n\
         node total: Dimensionless = @dep_out + 1.0;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let total = find_value(&result, "total");
    assert!((total - 15.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_inline_dag_include_cross_file_self_import() {
    // Cross-file `include` of a DAG whose body has `import <self>.{...}`
    // (resolved against the dag's parent file). The parent's value must
    // flow through `merge_dependency` into the importer's IR for eval.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/valid/inline_dag_include_cross_file_self_import/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out = find_value(&result, "out");
    assert!(
        (out - 3_185_500.0).abs() < 1e-10,
        "expected 3185500.0, got {out}"
    );
}

#[test]
fn eval_inline_dag_call_in_for_comp_with_loop_var() {
    // Motivating shape: inline call inside a `for` whose arg references the
    // loop variable via an indexed graph ref.
    let source = "\
pub index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param dist: Length[Region] = { Region.A: 1.0 m, Region.B: 2.0 m };
node distances: Length[Region] = for r: Region { @id_len(v: @dist[r]).result };
";
    let result = compile_and_eval(source).unwrap();
    // distances is indexed, look it up by cell.
    let distances_entry = result
        .nodes
        .iter()
        .find(|(n, _)| n.to_string() == "distances")
        .expect("distances node")
        .1
        .as_ref()
        .expect("distances value");
    match distances_entry {
        crate::eval::types::Value::Indexed { entries, .. } => {
            let mut seen: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
            for (variant, value) in entries {
                seen.insert(variant.to_string(), value.si_value().unwrap());
            }
            assert!((seen["A"] - 1.0).abs() < 1e-10);
            assert!((seen["B"] - 2.0).abs() < 1e-10);
        }
        other => panic!("expected Indexed, got {other:?}"),
    }
}

#[test]
fn eval_inline_dag_call_in_match_arm_fresh_instances_per_syntactic_site() {
    // Each arm has a distinct syntactic call site; the eval-time selection
    // picks one arm's value but every call site semantically is a fresh
    // instantiation. This test exercises the motivating for/match shape.
    let source = "\
pub index Source = { Primary, Secondary };
pub index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

param dist_primary: Length[Region] = { Region.A: 1.0 m, Region.B: 2.0 m };
param dist_secondary: Length[Region] = { Region.A: 10.0 m, Region.B: 20.0 m };

node effective: Length[Source, Region] = for s: Source, r: Region {
    match s {
        Source.Primary   => @id_len(v: @dist_primary[r]).result,
        Source.Secondary => @id_len(v: @dist_secondary[r]).result,
    }
};
";
    let result = compile_and_eval(source).unwrap();
    let entry = result
        .nodes
        .iter()
        .find(|(n, _)| n.to_string() == "effective")
        .expect("effective node")
        .1
        .as_ref()
        .expect("effective value");
    // Nested Indexed: outer Source, inner Region.
    let crate::eval::types::Value::Indexed { entries: outer, .. } = entry else {
        panic!("expected Indexed, got {entry:?}");
    };
    let mut cells: std::collections::HashMap<(String, String), f64> =
        std::collections::HashMap::new();
    for (svar, sval) in outer {
        let crate::eval::types::Value::Indexed { entries: inner, .. } = sval else {
            panic!("expected inner Indexed, got {sval:?}");
        };
        for (rvar, rval) in inner {
            cells.insert(
                (svar.to_string(), rvar.to_string()),
                rval.si_value().unwrap(),
            );
        }
    }
    assert!((cells[&("Primary".into(), "A".into())] - 1.0).abs() < 1e-10);
    assert!((cells[&("Primary".into(), "B".into())] - 2.0).abs() < 1e-10);
    assert!((cells[&("Secondary".into(), "A".into())] - 10.0).abs() < 1e-10);
    assert!((cells[&("Secondary".into(), "B".into())] - 20.0).abs() < 1e-10);
}

#[test]
fn eval_inline_dag_call_composition_fixture() {
    let source =
        include_str!("../../../../tests/fixtures/valid/inline_dag_call_composition/main.gcl");
    let result = compile_and_eval(source).unwrap();
    // ((3 * 2) + 1) m = 7 m
    let y = find_value(&result, "y");
    assert!((y - 7.0).abs() < 1e-10, "expected 7.0, got {y}");
}

#[test]
fn eval_inline_dag_call_in_for_fixture() {
    let source = include_str!("../../../../tests/fixtures/valid/inline_dag_call_in_for/main.gcl");
    let _result = compile_and_eval(source).unwrap();
}

#[test]
fn eval_inline_dag_call_in_match_fixture() {
    let source = include_str!("../../../../tests/fixtures/valid/inline_dag_call_in_match/main.gcl");
    let _result = compile_and_eval(source).unwrap();
}

#[test]
fn eval_inline_dag_call_forward_reference_within_body() {
    // MVP walked the dag body in source order, which made this fail at eval
    // because `b` was evaluated before `a` was bound. The compile-pipeline
    // refactor runs the body in topological order.
    let source = "\
dag forward {
    param v: Length;
    pub node b: Length = @a * 2.0;
    node a: Length = @v + 1.0 m;
}

param src: Length = 3.0 m;
node out: Length = @forward(v: @src).b;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    // (3 + 1) * 2 = 8
    assert!((out - 8.0).abs() < 1e-10, "expected 8.0, got {out}");
}

#[test]
fn eval_cross_file_inline_dag_nested_call_uses_canonical_target_with_same_leaf_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub dag helper {\n\
             pub node result: Dimensionless = 2.0;\n\
         }\n\
         pub dag outer {\n\
             pub node result: Dimensionless = @helper().result + 10.0;\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub dag helper {\n\
             pub node result: Dimensionless = 100.0;\n\
         }\n\
         pub dag outer {\n\
             pub node result: Dimensionless = @helper().result + 1000.0;\n\
         }\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         dag helper {\n\
             pub node result: Dimensionless = 10000.0;\n\
         }\n\
         node out_a: Dimensionless = @a.outer().result;\n\
         node out_b: Dimensionless = @b.outer().result;\n\
         node out_local: Dimensionless = @helper().result;\n\
         node total: Dimensionless = @out_a + @out_b + @out_local;\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();
    let out_a = find_value(&result, "out_a");
    let out_b = find_value(&result, "out_b");
    let out_local = find_value(&result, "out_local");
    let total = find_value(&result, "total");
    assert!((out_a - 12.0).abs() < 1e-10, "out_a = {out_a}");
    assert!((out_b - 1100.0).abs() < 1e-10, "out_b = {out_b}");
    assert!(
        (out_local - 10000.0).abs() < 1e-10,
        "out_local = {out_local}"
    );
    assert!((total - 11112.0).abs() < 1e-10, "total = {total}");
}

#[test]
fn eval_public_values_preserve_same_leaf_imported_index_owners() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("a.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    std::fs::write(
        root_dir.join("b.gcl"),
        "pub index Phase = { Burn, Coast };\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node series_a: Dimensionless[a.Phase] = for p: a.Phase { 1.0 };\n\
         node series_b: Dimensionless[b.Phase] = for p: b.Phase { 2.0 };\n",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs()).unwrap();

    let indexed_owner = |name: &str| {
        let value = result
            .nodes
            .iter()
            .find(|(n, _)| n.to_string() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
        let Value::Indexed {
            index_name,
            entries,
            ..
        } = value
        else {
            panic!("expected indexed value for `{name}`, got {value:?}");
        };
        assert_eq!(index_name.display_name().as_str(), "Phase");
        assert_eq!(entries.len(), 2);
        index_name
            .declared_resolved()
            .cloned()
            .unwrap_or_else(|| panic!("expected declared index for `{name}`"))
    };

    assert_ne!(indexed_owner("series_a"), indexed_owner("series_b"));
}

#[test]
fn eval_inline_dag_call_indexed_output_projection() {
    // Projected output is itself indexed; the call site reads one cell.
    let source = "\
pub index Region = { A, B };

dag doubler {
    import input.{ Region };

    param v: Length[Region];
    pub node result: Length[Region] = for r: Region { @v[r] * 2.0 };
}

param dist: Length[Region] = { Region.A: 1.0 m, Region.B: 3.0 m };
node out_a: Length = @doubler(v: @dist).result[Region.A];
node out_b: Length = @doubler(v: @dist).result[Region.B];
";
    let result = compile_and_eval(source).unwrap();
    let a = find_value(&result, "out_a");
    let b = find_value(&result, "out_b");
    assert!((a - 2.0).abs() < 1e-10, "expected 2.0, got {a}");
    assert!((b - 6.0).abs() < 1e-10, "expected 6.0, got {b}");
}

// ---- Typed Int and Datetime domain constraints (#958) ----

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
            .all
            .iter()
            .find(|(candidate, _, _)| candidate.to_string() == name)
            .unwrap_or_else(|| panic!("{name} not found"));
        assert!(value.is_ok(), "{name} failed: {value:?}");
    }
}

#[test]
fn every_supported_datetime_scale_accepts_matching_domain_bounds() {
    use std::fmt::Write as _;

    let source = graphcal_compiler::registry::time_scale::TimeScale::ALL
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
    for scale in graphcal_compiler::registry::time_scale::TimeScale::ALL {
        let name = scale.to_string().to_ascii_lowercase();
        let (_, value, _) = result
            .all
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
    Event.Early: datetime("2023-12-31T23:59:59Z"),
    Event.OnTime: datetime("2024-06-01T00:00:00Z"),
};
"#,
    )
    .unwrap();
    let (_, schedule, _) = result
        .all
        .iter()
        .find(|(name, _, _)| name.to_string() == "schedule")
        .expect("schedule not found");
    let NodeError::EvalFailed { message } = schedule.as_ref().unwrap_err() else {
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
        .all
        .iter()
        .find(|(name, _, _)| name.to_string() == "BAD")
        .expect("BAD not found");
    let NodeError::EvalFailed { message } = bad.as_ref().unwrap_err() else {
        panic!("expected EvalFailed, got {bad:?}");
    };
    assert!(
        message.contains("EventSpec.at") && message.contains("above maximum"),
        "message = {message}"
    );
}

#[test]
fn datetime_struct_field_bound_requires_the_field_scale() {
    let error = compile_and_eval(
        r#"
type EventSpec {
    EventSpec(at: Datetime<TT>(min: datetime("2024-01-01T00:00:00Z"))),
}
node event: EventSpec = EventSpec(at: epoch<TT>("2024-06-01T00:00:00"));
"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::DatetimeDomainBoundTypeMismatch { .. })
    ));
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
    Slot.First: datetime("2024-06-01T00:00:00Z"),
    Slot.Second: datetime("2025-01-01T00:00:00Z"),
});
"#,
    )
    .unwrap();
    let (_, bad, _) = result
        .all
        .iter()
        .find(|(name, _, _)| name.to_string() == "BAD")
        .expect("BAD not found");
    let NodeError::EvalFailed { message } = bad.as_ref().unwrap_err() else {
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
fn domain_bounds_reject_runtime_graph_references() {
    let error = compile_to_tir(
        r#"
param start: Datetime = datetime("2024-01-01T00:00:00Z");
param event: Datetime(min: @start) = datetime("2024-06-01T00:00:00Z");
"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::GraphRefInConst { .. })
    ));
}

#[test]
fn int_domain_constraints_preserve_i64_extremes() {
    let result = compile_and_eval(
        "param minimum: Int(\
         min: -9223372036854775807 - 1, \
         max: 9223372036854775807) = -9223372036854775807 - 1;\n\
         param maximum: Int(\
         min: -9223372036854775807 - 1, \
         max: 9223372036854775807) = 9223372036854775807;",
    )
    .unwrap();
    assert_eq!(find_int_value(&result, "minimum"), i64::MIN);
    assert_eq!(find_int_value(&result, "maximum"), i64::MAX);
}

// ---- Domain constraints on struct/union member fields (#450 Pos 1+2) ----

#[test]
fn struct_field_within_bounds_passes() {
    let source = include_str!("../../../../tests/fixtures/valid/domain_field_within_bounds.gcl");
    let result = compile_and_eval(source).unwrap();
    let (_, val) = result
        .consts
        .iter()
        .find(|(n, _)| n.to_string() == "SAT")
        .expect("SAT not found");
    matches!(val.as_ref().unwrap(), Value::Struct { .. });
}

#[test]
fn struct_field_const_violation_is_compile_time() {
    let source = "
type Spec { Spec(mass: Mass(min: 100.0 kg, max: 2000.0 kg)) }
const node SAT: Spec = Spec(mass: 5000.0 kg);
";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(GraphcalError::DomainViolation {
        name, violation, ..
    }) = err
    else {
        panic!("expected DomainViolation, got {err:?}");
    };
    assert_eq!(name, "SAT.mass");
    assert!(
        violation.contains("above maximum"),
        "violation = {violation}"
    );
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
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == "SAT")
        .expect("SAT not found");
    let err = sat_result.as_ref().unwrap_err();
    let NodeError::EvalFailed { message } = err else {
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
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == "R")
        .expect("R not found");
    let err = r_result.as_ref().unwrap_err();
    let NodeError::EvalFailed { message } = err else {
        panic!("expected EvalFailed, got {err:?}");
    };
    assert!(
        message.contains("Burn.dv") && message.contains("above maximum"),
        "message = {message}"
    );
}

#[test]
fn struct_field_min_exceeds_max_at_compile_time() {
    let source = "type Foo { Foo(x: Mass(min: 100.0 kg, max: 50.0 kg)) }";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(GraphcalError::DomainMinExceedsMax { name, .. }) = err else {
        panic!("expected DomainMinExceedsMax, got {err:?}");
    };
    assert_eq!(name, "Foo.x");
}

#[test]
fn struct_field_invalid_target_at_compile_time() {
    let source = "type Foo { Foo(x: Bool(min: 0.0)) }";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(GraphcalError::InvalidDomainTarget { .. })
        ),
        "expected InvalidDomainTarget, got {err:?}"
    );
}

#[test]
fn struct_field_dim_mismatch_at_compile_time() {
    let source = "type Foo { Foo(x: Length(min: 1.0 s)) }";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(GraphcalError::DomainDimensionMismatch { name, .. }) = err else {
        panic!("expected DomainDimensionMismatch, got {err:?}");
    };
    assert_eq!(name, "Foo.x");
}

// ---- Position 4: domain constraint on a generic type argument ----

#[test]
fn generic_type_arg_constraint_rejected() {
    let source = "
pub type Eci { Eci }
pub type Vec3<D: Dim, F: Type> { Vec3(x: D, y: D, z: D) }
param p: Vec3<Length(min: 0.0 m), Eci> = Vec3<Length, Eci>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(GraphcalError::GenericTypeArgDomainConstraint { .. })
        ),
        "expected GenericTypeArgDomainConstraint, got {err:?}"
    );
}

// ---- Position 3: regression — include'd DAG already validates ----

#[test]
fn included_dag_param_constraint_runtime_violation() {
    let source = "
dag bumper {
    param v: Velocity(max: 100.0 m/s);
    pub node out: Velocity = @v * 2.0;
}
param speed: Velocity = 1000.0 m/s;
include bumper(v: @speed).{ out as doubled };
";
    let result = compile_and_eval(source).unwrap();
    let (_, v_result, _) = result
        .all
        .iter()
        .find(|(n, _, _)| n.to_string() == "bumper.v")
        .expect("bumper.v not found");
    assert!(v_result.is_err(), "v should violate domain constraint");
}

#[test]
fn included_dag_param_constraint_dim_mismatch() {
    let source = "
dag bumper {
    param v: Velocity(min: 1.0 kg);
    pub node out: Velocity = @v;
}
include bumper(v: 5.0 m/s).{ out };
";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(GraphcalError::DomainDimensionMismatch { .. })
        ),
        "expected DomainDimensionMismatch, got {err:?}"
    );
}

#[test]
fn eval_inline_dag_call_const_node_in_body() {
    // `const node` inside a dag body should participate in the same
    // topological evaluation as runtime nodes.
    let source = "\
dag with_const {
    param v: Length;
    const node multiplier: Dimensionless = 3.0;
    pub node result: Length = @v * @multiplier;
}

param src: Length = 4.0 m;
node out: Length = @with_const(v: @src).result;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 12.0).abs() < 1e-10, "expected 12.0, got {out}");
}

#[test]
fn product_and_rss_support_linear_algebra_ergonomics() {
    let source = include_str!("../../../../tests/fixtures/valid/linear_algebra_ergonomics.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "box_volume") - 24.0).abs() < 1e-12);
    assert!((find_value(&result, "budget_sigma") - 5.0).abs() < 1e-12);
    assert!((find_value(&result, "chained_product") - 720.0).abs() < 1e-12);
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, result, _)| matches!(result, super::types::AssertResult::Pass))
    );
}

#[test]
fn dag_local_required_types_enable_reusable_linear_algebra() {
    let source = include_str!("../../../../tests/fixtures/valid/linear_algebra_reusable_dag.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "displacement_magnitude.result") - 13.0).abs() < 1e-12);
    assert!((find_value(&result, "duration_magnitude.result") - 13.0).abs() < 1e-12);
    assert!(
        result
            .assertions
            .iter()
            .all(|(_, result, _)| matches!(result, super::types::AssertResult::Pass))
    );
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
        result
            .assertions
            .iter()
            .all(|(_, r, _)| matches!(r, super::types::AssertResult::Pass)),
        "all fixture assertions must pass: {:?}",
        result.assertions
    );
}

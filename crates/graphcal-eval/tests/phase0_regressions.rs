//! Phase 0 regression tests from `.local/2026-05-31_code-review-report.md`.
//!
//! These tests assert the desired safety behavior. Regressions that are not fixed
//! yet remain marked `#[should_panic]`; active fixes should remove that attribute
//! and make the assertion pass normally.
#![cfg(test)]

use std::collections::HashMap;
use std::panic;

use graphcal_eval::eval::{EvalResult, Value, compile_and_eval, compile_and_eval_project};
use graphcal_io::RealFileSystem;

fn has_decl_error(result: &EvalResult, name: &str) -> bool {
    result
        .all
        .iter()
        .any(|(decl_name, value, _)| decl_name.to_string() == name && value.is_err())
}

fn value_for<'a>(result: &'a EvalResult, name: &str) -> &'a Value {
    result
        .all
        .iter()
        .find(|(decl_name, _, _)| decl_name.to_string() == name)
        .unwrap_or_else(|| panic!("declaration `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|err| panic!("declaration `{name}` has error: {err}"))
}

fn assert_rejected_or_decl_error(source: &str, decl_name: &str, bug: &str) {
    match compile_and_eval(source) {
        Err(_) => {}
        Ok(result) if has_decl_error(&result, decl_name) => {}
        Ok(result) => panic!("{bug}: accepted invalid program successfully: {result:?}"),
    }
}

#[test]
fn non_finite_numeric_literal_is_rejected() {
    assert_rejected_or_decl_error(
        "node x: Dimensionless = 1e999;",
        "x",
        "BUG: non-finite numeric literal accepted",
    );
}

#[test]
fn overflowing_unit_literal_is_rejected() {
    assert_rejected_or_decl_error(
        "node x: Length = 1e308 km;",
        "x",
        "BUG: overflowing unit literal accepted",
    );
}

#[test]
fn zero_static_unit_scale_is_rejected() {
    assert_rejected_or_decl_error(
        "const unit z: Length = 0.0 m;\nnode x: Length = 1.0 z;",
        "x",
        "BUG: zero static unit scale accepted",
    );
}

#[test]
fn negative_static_unit_scale_is_rejected() {
    assert_rejected_or_decl_error(
        "const unit neg_m: Length = (-1.0) m;\nnode x: Length = 1.0 neg_m;",
        "x",
        "BUG: negative static unit scale accepted",
    );
}

#[test]
fn non_finite_static_unit_scale_is_rejected() {
    assert_rejected_or_decl_error(
        "const unit huge_m: Length = 1e999 m;\nnode x: Length = 1.0 huge_m;",
        "x",
        "BUG: non-finite static unit scale accepted",
    );
}

#[test]
fn zero_dynamic_unit_scale_is_rejected() {
    assert_rejected_or_decl_error(
        r"
base dim Money;
base unit USD: Money;
param usd_per_eur: Dimensionless = 0.0;
unit EUR: Money = (@usd_per_eur) USD;
node price: Money = 1.0 EUR;
",
        "price",
        "BUG: zero dynamic unit scale accepted",
    );
}

#[test]
fn negative_dynamic_unit_scale_is_rejected() {
    assert_rejected_or_decl_error(
        r"
base dim Money;
base unit USD: Money;
param usd_per_eur: Dimensionless = -1.0;
unit EUR: Money = (@usd_per_eur) USD;
node price: Money = 1.0 EUR;
",
        "price",
        "BUG: negative dynamic unit scale accepted",
    );
}

#[test]
fn non_finite_dynamic_unit_scale_is_rejected() {
    assert_rejected_or_decl_error(
        r"
base dim Money;
base unit USD: Money;
param usd_per_eur: Dimensionless = 1e999;
unit EUR: Money = (@usd_per_eur) USD;
node price: Money = 1.0 EUR;
",
        "price",
        "BUG: non-finite dynamic unit scale accepted",
    );
}

#[test]
fn linspace_step_count_does_not_overshoot_end() {
    let source = r"
pub index T = linspace(0.0 s, 1.0 s, step: 0.6 s);
node x: Dimensionless[T] = for t: T { t / 1.0 s };
";
    let result = compile_and_eval(source).unwrap_or_else(|err| {
        panic!("BUG: linspace step count produced entries beyond end: unexpected error: {err}")
    });
    match value_for(&result, "x") {
        Value::Indexed { entries, .. } => assert_eq!(
            entries.len(),
            2,
            "BUG: linspace step count produced entries beyond end: expected 0.0 and 0.6 only, got {entries:?}",
        ),
        other => panic!(
            "BUG: linspace step count produced entries beyond end: expected indexed value, got {other:?}"
        ),
    }
}

#[test]
fn infinite_range_bounds_are_diagnostic_not_panic() {
    let source = r"
index T = linspace(0.0, 1e999, step: 1.0);
node x: Dimensionless[T] = for t: T { 1.0 };
";
    let result = panic::catch_unwind(|| compile_and_eval(source).is_err());
    match result {
        Err(payload) => {
            drop(payload);
            panic!("BUG: infinite range bounds must produce a diagnostic instead of panicking")
        }
        Ok(true) => {}
        Ok(false) => panic!(
            "BUG: infinite range bounds must produce a diagnostic instead of panicking: accepted invalid range",
        ),
    }
}

#[test]
fn range_zero_is_rejected() {
    let source = r"
node s: Dimensionless = sum(for i: range(0) { 1.0 });
node m: Dimensionless = min(for i: range(0) { 1.0 });
";
    let result = compile_and_eval(source);
    assert!(
        result.is_err(),
        "BUG: range(0) accepted despite the no-empty-index invariant: {result:?}",
    );
}

fn write_required_runtime_input_type_project(
    main_source: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap_or_else(|err| {
        panic!("BUG: type-only import required an evaluated dependency: tempdir failed: {err}")
    });
    let package_dir = dir.path().join("src/pkg");
    std::fs::create_dir_all(&package_dir).unwrap_or_else(|err| {
        panic!("BUG: type-only import required an evaluated dependency: mkdir failed: {err}")
    });
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap_or_else(|err| {
        panic!(
            "BUG: type-only import required an evaluated dependency: manifest write failed: {err}"
        )
    });
    std::fs::write(
        package_dir.join("lib.gcl"),
        r"
pub type Foo { Foo(x: Dimensionless) }
pub(bind) index Phase;
param cost: Dimensionless[Phase];
",
    )
    .unwrap_or_else(|err| {
        panic!("BUG: type-only import required an evaluated dependency: lib write failed: {err}")
    });
    let root = package_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap_or_else(|err| {
        panic!("BUG: type-only import required an evaluated dependency: root write failed: {err}")
    });

    (dir, root)
}

#[test]
fn type_only_import_from_library_with_required_runtime_inputs_compiles() {
    let (_dir, root) = write_required_runtime_input_type_project(
        r"
import pkg.lib.{ type Foo };
pub type Bar { Bar(inner: Foo) }
",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default());
    assert!(
        result.is_ok(),
        "BUG: type-only import required an evaluated dependency: {result:?}",
    );
}

#[test]
fn module_type_import_from_library_with_required_runtime_inputs_compiles() {
    let (_dir, root) = write_required_runtime_input_type_project(
        r"
import pkg.lib as lib;
pub type Bar { Bar(inner: lib.Foo) }
",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default());
    assert!(
        result.is_ok(),
        "BUG: module type resolution required an evaluated dependency: {result:?}",
    );
}

#[test]
fn runtime_include_from_library_with_required_runtime_inputs_is_user_error() {
    let (_dir, root) = write_required_runtime_input_type_project(
        r"
include pkg.lib().{ cost };
",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default());
    let message = format!("{result:?}");
    assert!(
        result.is_err()
            && message.contains("cannot include runtime item `cost`")
            && !message.contains("internal"),
        "BUG: runtime import from unevaluated library should be a user-facing diagnostic: {message}",
    );
}

#[test]
fn to_int_rejects_upper_out_of_range_boundary() {
    let source = "node x: Int = to_int(9.223372036854776e18);";
    assert_rejected_or_decl_error(
        source,
        "x",
        "BUG: to_int accepted the upper out-of-range boundary",
    );
}

#[test]
fn long_operator_chain_compiles_and_evaluates() {
    // Regression: every recursive walker (lowering, dim-check, eval) used to
    // recurse once per chain term with no stack-growth guard; a few hundred
    // terms aborted the process with a stack overflow in debug builds.
    let terms = 2_000;
    let source = format!(
        "node x: Dimensionless = {};",
        vec!["1.0"; terms].join(" + ")
    );
    let result = compile_and_eval(&source).unwrap();
    let x = value_for(&result, "x");
    #[expect(clippy::cast_precision_loss, reason = "small test constant")]
    let expected = terms as f64;
    assert!((x.si_value().unwrap() - expected).abs() < 1e-9);
}

#[test]
fn dynamic_unit_dep_augmentation_orders_late_param_before_unit_use() {
    // Exercises the typed dynamic-unit dependency map (unit name → scoped
    // refs): the param backing the dynamic unit is declared *after* the
    // node using the unit, so evaluation order relies on the augmented
    // runtime deps extracted from the scale expression.
    let source = r"
base dim Money;
base unit USD: Money;
unit EUR: Money = (@usd_per_eur) USD;
node price: Money = 3.0 EUR;
param usd_per_eur: Dimensionless = 2.0;
";
    let result = compile_and_eval(source).unwrap();
    let price = value_for(&result, "price");
    assert!((price.si_value().unwrap() - 6.0).abs() < 1e-9);
}

#[test]
fn nested_unfold_self_reference_is_not_a_cycle() {
    // Regression: the unfold self-edge was only removed when `unfold` was
    // the top-level expression of the declaration; a nested form (e.g.
    // inside `if`) was rejected with a spurious cyclic-dependency error.
    let source = r"
index Step = linspace(0.0 s, 2.0 s, step: 1.0 s);
node y: Dimensionless[Step] =
    if 1.0 > 0.0 { unfold(0.0, |p, t| @y[p] + 1.0) } else { unfold(0.0, |p, t| @y[p] + 2.0) };
";
    let result = compile_and_eval(source).unwrap();
    let y = value_for(&result, "y");
    match y {
        Value::Indexed { entries, .. } => assert_eq!(entries.len(), 3),
        other => panic!("expected indexed value, got {other:?}"),
    }
}

#[test]
fn self_reference_outside_unfold_is_still_a_cycle() {
    let source = "node a: Dimensionless = @a + 1.0;";
    assert!(compile_and_eval(source).is_err());
}

#[test]
fn fully_qualified_self_import_is_not_a_circular_import() {
    // Regression: the top-level import loop lacked the self-path guard the
    // inline-dag loop has, so `import nasa.main.velocity.{v};` inside
    // main.gcl recursed into itself and reported the misleading
    // `circular import detected: main.gcl -> main.gcl` (and, once guarded,
    // panicked on the not-yet-registered self DagId). Self-file imports are
    // not supported, but they must fail with a structured diagnostic, not a
    // bogus cycle or a panic.
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/nasa");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"nasa\"\n",
    )
    .unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "dag velocity {\n\
             pub node v: Dimensionless = 2.0;\n\
         }\n\
         import nasa.main.velocity.{v};\n\
         node out: Dimensionless = @v + 1.0;\n",
    )
    .unwrap();

    let err = compile_and_eval_project(
        &root,
        &std::collections::HashMap::new(),
        None,
        &RealFileSystem::default(),
    )
    .unwrap_err();
    let message = format!("{err:?}");
    assert!(
        !message.contains("circular import"),
        "self-import must not be reported as a circular import: {message}"
    );
}

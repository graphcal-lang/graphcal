//! Phase 0 regression tests from `.local/2026-05-31_code-review-report.md`.
//!
//! These tests assert the desired safety behavior. Regressions that are not fixed
//! yet remain marked `#[should_panic]`; active fixes should remove that attribute
//! and make the assertion pass normally.
#![cfg(test)]

use std::collections::HashMap;
use std::panic;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{
    CompileError, EvalResult, Value, compile_and_eval, compile_and_eval_project,
};
use graphcal_io::RealFileSystem;
use miette::Diagnostic;

fn compile_graphcal_error(source: &str) -> GraphcalError {
    match compile_and_eval(source).unwrap_err() {
        CompileError::Eval(err) => err,
        other => panic!("expected semantic error, got {other:?}"),
    }
}

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
fn overflowing_quantity_literal_is_rejected() {
    assert_rejected_or_decl_error(
        "node x: Length = 1e308 km;",
        "x",
        "BUG: overflowing quantity literal accepted",
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
fn cross_dimension_static_unit_definition_is_rejected() {
    let result =
        compile_and_eval("const unit wrong: Length = 1.0 h;\nparam x: Length = 2.0 wrong;");
    let message = format!("{result:?}");
    assert!(
        result.is_err() && message.contains("UnitDefinitionDimensionMismatch"),
        "BUG: a time scale was registered as a length unit: {message}",
    );
}

#[test]
fn cross_dimension_dynamic_unit_definition_is_rejected() {
    let result =
        compile_and_eval("param factor: Dimensionless = 2.0;\nunit wrong: Length = (@factor) h;");
    let message = format!("{result:?}");
    assert!(
        result.is_err() && message.contains("UnitDefinitionDimensionMismatch"),
        "BUG: a dynamic time scale was registered as a length unit: {message}",
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
fn unused_dynamic_unit_scale_rejects_unknown_reference() {
    let err = compile_graphcal_error(
        "base dim Money;\nbase unit USD: Money;\nunit EUR: Money = (@missing) USD;\n",
    );
    assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
}

#[test]
fn unused_dynamic_unit_scale_rejects_assertion_reference() {
    let err = compile_graphcal_error(
        "base dim Money;\nbase unit USD: Money;\nassert factor = true;\nunit EUR: Money = (@factor) USD;\n",
    );
    assert!(matches!(err, GraphcalError::GraphRefToAssert { .. }));
}

#[test]
fn dynamic_unit_scale_must_be_scalar_dimensionless_quantity() {
    let invalid_factors = [
        "param factor: Length = 2.0 m;",
        "param factor: Bool = true;",
        "param factor: Int = 2;",
        "pub index Case = { A, B };\nparam factor: Dimensionless[Case] = { Case.A: 1.0, Case.B: 2.0 };",
        "pub type Config { Config(value: Dimensionless) }\nparam factor: Config = Config(value: 2.0);",
    ];
    for declarations in invalid_factors {
        let source = format!(
            "base dim Money;\nbase unit USD: Money;\n{declarations}\nunit EUR: Money = (@factor) USD;\n"
        );
        let err = compile_graphcal_error(&source);
        assert!(
            matches!(err, GraphcalError::DynamicUnitScaleTypeMismatch { .. }),
            "expected D032 for invalid dynamic scale, got {err:?}"
        );
    }
}

#[test]
fn range_step_must_land_on_endpoint() {
    let source = r"
pub index T = range(0.0 s, 1.0 s, step: 0.6 s);
node x: Dimensionless[T] = for t: T { t / 1.0 s };
";
    let error = compile_and_eval(source).unwrap_err();
    assert!(
        error.to_string().contains("does not land on endpoint"),
        "unexpected error: {error}"
    );
}

#[test]
fn infinite_range_bounds_are_diagnostic_not_panic() {
    let source = r"
index T = range(0.0, 1e999, step: 1.0);
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
fn finite_zero_is_rejected() {
    let source = r"
node s: Dimensionless = sum(for i: Fin(0) { 1.0 });
node m: Dimensionless = minimum(for i: Fin(0) { 1.0 });
";
    let result = compile_and_eval(source);
    assert!(
        result.is_err(),
        "BUG: Fin(0) accepted despite the no-empty-index invariant: {result:?}",
    );
}

fn write_imported_binding_collision_project(
    libraries: &[(&str, f64)],
    qualified_import: bool,
    nested: bool,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/collision");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collision\"\n",
    )
    .unwrap();

    for (name, imported_value) in libraries {
        std::fs::write(
            package_dir.join(format!("config_{name}.gcl")),
            format!("pub const node C: Dimensionless = {imported_value:.1};\n"),
        )
        .unwrap();
        let import = if qualified_import {
            format!("import collision.config_{name} as config;\n")
        } else {
            format!("import collision.config_{name}.{{ C }};\n")
        };
        let reference = if qualified_import { "@config.C" } else { "@C" };
        std::fs::write(
            package_dir.join(format!("leaf_{name}.gcl")),
            format!(
                "{import}param p: Dimensionless;\npub node out: Dimensionless = @p + {reference};\n"
            ),
        )
        .unwrap();
        if nested {
            std::fs::write(
                package_dir.join(format!("lib_{name}.gcl")),
                format!(
                    "param p: Dimensionless;\ninclude collision.leaf_{name}(p: @p) as leaf;\npub node out: Dimensionless = @leaf.out;\n"
                ),
            )
            .unwrap();
        }
    }

    let include_file = if nested { "lib" } else { "leaf" };
    let includes = libraries
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            format!(
                "include collision.{include_file}_{name}(p: {}.0) as {name};",
                index + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let total = libraries
        .iter()
        .map(|(name, _)| format!("@{name}.out"))
        .collect::<Vec<_>>()
        .join(" + ");
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        format!("{includes}\npub node total: Dimensionless = {total};\n"),
    )
    .unwrap();

    (dir, root)
}

fn assert_imported_binding_collision_values(
    libraries: &[(&str, f64)],
    qualified_import: bool,
    nested: bool,
) {
    let (_dir, root) =
        write_imported_binding_collision_project(libraries, qualified_import, nested);
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|err| panic!("collision project must compile: {err:?}"));

    let mut expected_total = 0.0;
    for (index, (name, imported_value)) in libraries.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "small test fixture index")]
        let expected = imported_value + (index + 1) as f64;
        let actual = value_for(&result, &format!("{name}.out"))
            .si_value()
            .unwrap();
        assert!(
            (actual - expected).abs() < 1e-9,
            "include `{name}` read another instance's imported value: expected {expected}, got {actual}"
        );
        expected_total += expected;
    }
    let total = value_for(&result, "total").si_value().unwrap();
    assert!((total - expected_total).abs() < 1e-9);
}

#[test]
fn selective_same_leaf_imports_are_isolated_for_all_include_orders() {
    let permutations = [
        [("a", 1.0), ("b", 10.0), ("c", 100.0)],
        [("a", 1.0), ("c", 100.0), ("b", 10.0)],
        [("b", 10.0), ("a", 1.0), ("c", 100.0)],
        [("b", 10.0), ("c", 100.0), ("a", 1.0)],
        [("c", 100.0), ("a", 1.0), ("b", 10.0)],
        [("c", 100.0), ("b", 10.0), ("a", 1.0)],
    ];
    for permutation in permutations {
        assert_imported_binding_collision_values(&permutation, false, false);
    }
}

#[test]
fn equal_qualified_aliases_in_included_files_keep_canonical_targets() {
    assert_imported_binding_collision_values(&[("a", 1.0), ("b", 10.0)], true, false);
}

#[test]
fn nested_includes_keep_hidden_imported_bindings_instance_scoped() {
    assert_imported_binding_collision_values(&[("a", 1.0), ("b", 10.0)], false, true);
}

#[test]
fn unused_dynamic_unit_scale_rejects_external_function_call() {
    let err = compile_graphcal_error(
        r#"
import plugin "graphcal:demo" as demo {
    fn factor(x: Dimensionless) -> Dimensionless;
}
param trigger: Dimensionless = 2.0;
unit Bad: Length = (demo.factor(@trigger)) m;
"#,
    );
    assert!(matches!(err, GraphcalError::ExternCallNotAllowed { .. }));
}

#[test]
fn include_rejects_runtime_dependent_unit_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/dynamic_include");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"dynamic_include\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("lib.gcl"),
        "pub base dim Money;\npub base unit USD: Money;\nparam factor: Dimensionless;\npub unit EUR: Money = (@factor) USD;\npub node out: Money = 3.0 EUR;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include dynamic_include.lib(factor: 2.0) as priced;\n",
    )
    .unwrap();

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::IncludeRuntimeUnit { ref name, .. })
            if name.name().as_str() == "EUR"
    ));
}

#[test]
fn repeated_includes_reject_runtime_dependent_unit_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/dynamic_instances");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"dynamic_instances\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("lib.gcl"),
        "pub base dim Money;\npub base unit USD: Money;\nparam factor: Dimensionless;\npub unit EUR: Money = (@factor) USD;\npub node out: Money = 3.0 EUR;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include dynamic_instances.lib(factor: 2.0) as cheap;\ninclude dynamic_instances.lib(factor: 4.0) as expensive;\n",
    )
    .unwrap();

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::IncludeRuntimeUnit { ref name, .. })
            if name.name().as_str() == "EUR"
    ));
}

#[test]
fn nested_include_rejects_runtime_dependent_unit_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/nested_dynamic_instances");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"nested_dynamic_instances\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("lib.gcl"),
        "pub base dim Money;\npub base unit USD: Money;\nparam factor: Dimensionless;\npub unit EUR: Money = (@factor) USD;\npub node out: Money = 3.0 EUR;\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("wrapper.gcl"),
        "import nested_dynamic_instances.lib as units;\nparam first_factor: Dimensionless;\nparam second_factor: Dimensionless;\ninclude nested_dynamic_instances.lib(factor: @first_factor) as first;\ninclude nested_dynamic_instances.lib(factor: @second_factor) as second;\npub node out: units.Money = @first.out + @second.out;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        "include nested_dynamic_instances.wrapper(first_factor: 2.0, second_factor: 3.0) as low;\ninclude nested_dynamic_instances.wrapper(first_factor: 4.0, second_factor: 5.0) as high;\n",
    )
    .unwrap();

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_err();
    let CompileError::Eval(GraphcalError::IncludeRuntimeUnit { name, src, .. }) = error else {
        panic!("expected M026 include-boundary error, got {error:?}");
    };
    assert_eq!(name.name().as_str(), "EUR");
    assert!(src.name().ends_with("wrapper.gcl"), "wrong source: {src:?}");
}

#[test]
fn included_dynamic_unit_error_uses_producer_source() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/dynamic_source");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"dynamic_source\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("lib.gcl"),
        "pub base dim Money;\npub base unit USD: Money;\npub unit EUR: Money = (@missing) USD;\npub node out: Money = 1.0 EUR;\n",
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(&root, "include dynamic_source.lib() as bad;\n").unwrap();

    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_err();
    let CompileError::Eval(error) = error else {
        panic!("expected semantic error, got {error:?}");
    };
    assert!(matches!(error, GraphcalError::UnknownGraphRef { .. }));
    let source = error.named_source().expect("error must carry source");
    assert!(
        source.name().ends_with("lib.gcl"),
        "wrong source: {source:?}"
    );
    let label = error.labels().unwrap().next().unwrap();
    let span = label.inner();
    assert_eq!(
        &source.inner()[span.offset()..span.offset() + span.len()],
        "missing"
    );
}

#[test]
fn include_binding_lowering_error_uses_importer_source() {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/provenance");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"provenance\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("lib.gcl"),
        "param input: Dimensionless;\npub node out: Dimensionless = @input;\n",
    )
    .unwrap();
    let main_source = "// deliberately different importer bytes\ninclude provenance.lib(input: @missing) as inst;\n";
    let root = package_dir.join("main.gcl");
    std::fs::write(&root, main_source).unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default());
    match result {
        Err(CompileError::Eval(GraphcalError::UnknownGraphRef { name, src, span })) => {
            assert_eq!(name.member().as_str(), "missing");
            assert!(
                src.name().ends_with("main.gcl"),
                "wrong source: {}",
                src.name()
            );
            assert!(span.offset() + span.len() <= src.inner().len());
            assert_eq!(
                &src.inner()[span.offset()..span.offset() + span.len()],
                "missing"
            );
        }
        other => panic!("expected importer-owned N002 diagnostic, got {other:?}"),
    }
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
fn explicit_include_reports_its_missing_required_index() {
    let (_dir, root) = write_required_runtime_input_type_project(
        r"
include pkg.lib().{ cost };
",
    );

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default());
    assert!(
        matches!(
            &result,
            Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound { name, .. }))
                if name == "Phase"
        ),
        "explicit instance should report its unsatisfied index: {result:?}",
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
fn nested_unfold_evaluates() {
    let source = r"
index Step = range(0.0 s, 2.0 s, step: 1.0 s);
node y: Dimensionless[Step] =
    if 1.0 > 0.0 { unfold(Step, 0.0, |prev_y, p, t| prev_y + 1.0) } else { unfold(Step, 0.0, |prev_y, p, t| prev_y + 2.0) };
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

#[test]
fn cross_file_inline_dag_call_using_extern_functions_evaluates() {
    // #943: a dep file's dag body may call the dep's own extern (plugin)
    // functions. The importer's merged TIR must carry the dep's resolved
    // extern signatures so a qualified inline call evaluates end-to-end.
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src/pkg");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pkg\"\n",
    )
    .unwrap();
    std::fs::write(
        package_dir.join("lib.gcl"),
        r#"
import plugin "graphcal:demo" as demo {
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
}

pub dag mid {
    param a: Length;
    param b: Length;
    pub node m: Length = demo.lerp(@a, @b, 0.5);
}
"#,
    )
    .unwrap();
    let root = package_dir.join("main.gcl");
    std::fs::write(
        &root,
        r"
import pkg.lib as lib;
node midpoint: Length = @lib.mid(a: 1.0 m, b: 3.0 m).m;
",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|err| panic!("cross-file extern call failed to compile/eval: {err:?}"));
    let value = value_for(&result, "midpoint");
    assert!(
        (value.si_value().unwrap() - 2.0).abs() < 1e-12,
        "expected lerp midpoint 2 m, got {value:?}"
    );
}

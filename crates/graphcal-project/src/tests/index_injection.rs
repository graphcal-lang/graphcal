//! Injectable (required) indexes and include binding rules.

use super::*;

#[test]
fn include_required_index_accepts_structural_finite_axis() {
    // Regression for #1077: structural axes need no fabricated named-index alias.
    let source = include_str!("../../../../tests/fixtures/valid/finite_index_dag_binding.gcl");
    let result = compile_and_eval(source).unwrap();
    let Value::Indexed { entries, .. } = find_entry(&result, "out") else {
        panic!("expected finite-indexed output");
    };
    let values = entries
        .values()
        .map(|value| value.si_value().unwrap().get())
        .collect::<Vec<_>>();
    assert_eq!(values, vec![2.0, 4.0, 6.0]);
}

#[test]
fn project_injectable_index_kind_mismatch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/invalid/multi/injectable_index_kind_mismatch/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Module(ModuleError::IndexKindMismatch {
                            dep_index,
                            bound_index,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(dep_index.to_string(), "Phase");
            assert_eq!(bound_index.to_string(), "TimeStep");
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
    // `Phase#Design` / `Phase#Build`) because the importer forgot to
    // re-bind `cost` in the same include statement.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/include_overrides_index_no_param_binding/src/lib/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Visibility(VisibilityError::IncludeMustReconcileOverride {
                            overridden,
                            overridden_kind,
                            orphan_decl,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(overridden.as_str(), "Phase");
            assert_eq!(overridden_kind.to_string(), "index");
            assert_eq!(orphan_decl.as_str(), "cost");
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
fn template_closure_error_renders_against_dependency_source() {
    // Option A checks reusable bodies with bindable Static ports rigid. The
    // V007 diagnostic belongs to the dependency body and must retain that
    // file's source and an in-bounds span.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/invalid/multi/merged_dep_body_dim_mismatch/src/lib/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(rendered)) => {
            let SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind:
                    SemanticErrorKind::Visibility(VisibilityError::TemplateBodyDependsOnStaticDefault {
                        body_name,
                        port_name,
                        ..
                    }),
                primary: span,
                ..
            }) = &rendered.error
            else {
                panic!("expected V007, got {:?}", rendered.error);
            };
            let src = rendered.registered_source();
            assert_eq!(body_name.as_str(), "v");
            assert_eq!(port_name.as_str(), "Speed");
            assert!(
                src.name().ends_with("lib.gcl"),
                "diagnostic must name the dependency file, got `{}`",
                src.name()
            );
            // The span must index into the source it renders against.
            let len = src.text().len();
            assert!(
                span.offset() + span.len() <= len,
                "span (offset {}, len {}) is out of bounds for `{}` of length {len}",
                span.offset(),
                span.len(),
                src.name(),
            );
        }
        other => panic!("expected V007 against the dependency source, got {other:?}"),
    }
}

#[test]
fn project_selective_include_leaks_private_type_v006() {
    // V006: selective `{ pub origin }` re-exports a declaration whose effective
    // signature names importer-private `PrivateInner` after substitution.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/pub_include_leaks_private_type/src/container/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Visibility(VisibilityError::GenericsLeakage {
                            reexport_name,
                            leaked_name,
                            leaked_kind,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(reexport_name.as_str(), "origin");
            assert_eq!(leaked_name.as_str(), "PrivateInner");
            assert_eq!(leaked_kind.noun(), "type");
        }
        other => panic!("expected GenericsLeakage, got {other:?}"),
    }
}

#[test]
fn project_selective_include_rejects_private_generic_default_binding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/invalid/multi/pub_include_leaks_private_generic_default/src/generic_default/main.gcl",
    );
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    match result {
        Err(CompileError::Eval(RenderedSemanticError {
            error:
                SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind:
                        SemanticErrorKind::Visibility(VisibilityError::GenericsLeakage {
                            reexport_name,
                            leaked_name,
                            leaked_kind,
                            ..
                        }),
                    ..
                }),
            ..
        })) => {
            assert_eq!(reexport_name.as_str(), "Wrapper");
            assert_eq!(leaked_name.as_str(), "PrivateElement");
            assert_eq!(leaked_kind.noun(), "type");
        }
        other => panic!("expected generic-default GenericsLeakage, got {other:?}"),
    }
}

#[test]
fn project_selective_include_with_public_type_binding_ok() {
    // Positive companion to project_selective_include_leaks_private_type_v006:
    // binding `Element` to a `pub` importer-local type satisfies A9.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/valid/multi/pub_include_with_public_type_binding/src/container/main.gcl");
    let result = compile_and_eval_project(&root, &HashMap::new(), None, &fs());
    assert!(
        result.is_ok(),
        "selectively re-exporting an output with a `pub` type binding should compile: {result:?}"
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
        .find(|assertion| assertion.name.to_string().contains("within_limit"))
        .expect("within_limit assertion not found");
    assert!(
        matches!(assert_result.outcome, AssertResult::Pass),
        "expected Pass, got {:?}",
        assert_result.outcome
    );
}

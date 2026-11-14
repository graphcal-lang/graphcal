//! Parameter overrides and required parameters.

use super::*;

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
        Err(CompileError::Binding(BindingError::NotAParam { name, actual_kind })) => {
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
        Err(CompileError::Binding(BindingError::NotAParam { name, actual_kind })) => {
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
        Err(CompileError::Binding(BindingError::UnknownParam { name })) => {
            assert_eq!(name.as_str(), "nonexistent");
        }
        other => panic!("expected OverrideUnknownParam, got {other:?}"),
    }
}

#[test]
fn recursive_types_are_rejected_before_any_model_schema() {
    let cases = [
        (
            "direct",
            "pub type List { Nil, Cons(head: Int, tail: List) }\nparam list: List;",
            "type `List` refers to itself through `Cons.tail`",
        ),
        (
            "through an include binding",
            "dag holder {\n\
             \x20   pub(bind) type T;\n\
             \x20   pub type Node { Node(next: T) }\n\
             }\n\
             pub type Outer { Outer(inner: Node) }\n\
             include holder(type T: Outer)::{ type Node };",
            "type `Outer` refers to itself through `Outer.inner` -> `Node` -> `Node.next`",
        ),
    ];
    for (case, source, message) in cases {
        let project = crate::loader::LoadedProject::from_source(source, "recursive.gcl").unwrap();
        match prepare_from_project(&project) {
            Err(CompileError::Eval(RenderedSemanticError {
                error:
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind: SemanticErrorKind::Struct(error @ StructError::RecursiveType { .. }),
                        ..
                    }),
                ..
            })) => assert_eq!(error.to_string(), message, "{case}"),
            other => panic!("{case}: expected a recursive type error, got {other:?}"),
        }
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

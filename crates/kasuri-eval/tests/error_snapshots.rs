//! Tests for error rendering snapshots.
#![allow(clippy::unwrap_used)]

use kasuri_eval::eval::compile_and_eval_named;
use miette::{Diagnostic, NarratableReportHandler};

/// Compile the given source and return the rendered error string.
/// Uses miette's `NarratableReportHandler` for deterministic output.
fn render_error(source: &str, name: &str) -> String {
    let err = compile_and_eval_named(source, name).unwrap_err();
    let diagnostic: &dyn Diagnostic = &err;
    let mut buf = String::new();
    NarratableReportHandler::new()
        .render_report(&mut buf, diagnostic)
        .unwrap();
    buf
}

#[test]
fn error_duplicate_name() {
    let source = include_str!("../../../tests/fixtures/errors/duplicate.ksr");
    let rendered = render_error(source, "duplicate.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_graph_ref() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_ref.ksr");
    let rendered = render_error(source, "unknown_ref.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_const_ref() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_const_ref.ksr");
    let rendered = render_error(source, "unknown_const_ref.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_at_in_const() {
    let source = include_str!("../../../tests/fixtures/errors/at_in_const.ksr");
    let rendered = render_error(source, "at_in_const.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_bad_const_casing() {
    let source = include_str!("../../../tests/fixtures/errors/bad_const_casing.ksr");
    let rendered = render_error(source, "bad_const_casing.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_bad_param_casing() {
    let source = include_str!("../../../tests/fixtures/errors/bad_param_casing.ksr");
    let rendered = render_error(source, "bad_param_casing.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_runtime_cycle() {
    let source = include_str!("../../../tests/fixtures/errors/cycle.ksr");
    let rendered = render_error(source, "cycle.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_const_cycle() {
    let source = include_str!("../../../tests/fixtures/errors/const_cycle.ksr");
    let rendered = render_error(source, "const_cycle.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_function() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_function.ksr");
    let rendered = render_error(source, "unknown_function.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_wrong_arity() {
    let source = include_str!("../../../tests/fixtures/errors/wrong_arity.ksr");
    let rendered = render_error(source, "wrong_arity.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_dim_mismatch_add() {
    let source = include_str!("../../../tests/fixtures/errors/dim_mismatch_add.ksr");
    let rendered = render_error(source, "dim_mismatch_add.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_dim_mismatch_annotation() {
    let source = include_str!("../../../tests/fixtures/errors/dim_mismatch_annotation.ksr");
    let rendered = render_error(source, "dim_mismatch_annotation.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_exp_requires_dimensionless() {
    let source = include_str!("../../../tests/fixtures/errors/exp_requires_dimensionless.ksr");
    let rendered = render_error(source, "exp_requires_dimensionless.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_unit() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_unit.ksr");
    let rendered = render_error(source, "unknown_unit.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_conversion_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/conversion_dim_mismatch.ksr");
    let rendered = render_error(source, "conversion_dim_mismatch.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_struct_field() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_struct_field.ksr");
    let rendered = render_error(source, "unknown_struct_field.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_struct_field_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/struct_field_dim_mismatch.ksr");
    let rendered = render_error(source, "struct_field_dim_mismatch.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_duplicate_let() {
    let source = include_str!("../../../tests/fixtures/errors/duplicate_let.ksr");
    let rendered = render_error(source, "duplicate_let.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_missing_struct_field() {
    let source = include_str!("../../../tests/fixtures/errors/missing_struct_field.ksr");
    let rendered = render_error(source, "missing_struct_field.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_at_in_fn() {
    let source = include_str!("../../../tests/fixtures/errors/at_in_fn.ksr");
    let rendered = render_error(source, "at_in_fn.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_recursive_fn() {
    let source = include_str!("../../../tests/fixtures/errors/recursive_fn.ksr");
    let rendered = render_error(source, "recursive_fn.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_fn_generic_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/fn_generic_dim_mismatch.ksr");
    let rendered = render_error(source, "fn_generic_dim_mismatch.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_dimension() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_dimension.ksr");
    let rendered = render_error(source, "unknown_dimension.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_extra_struct_fields() {
    let source = include_str!("../../../tests/fixtures/errors/extra_struct_fields.ksr");
    let rendered = render_error(source, "extra_struct_fields.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_not_a_struct() {
    let source = include_str!("../../../tests/fixtures/errors/not_a_struct.ksr");
    let rendered = render_error(source, "not_a_struct.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_index() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_index.ksr");
    let rendered = render_error(source, "unknown_index.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_variant() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_variant.ksr");
    let rendered = render_error(source, "unknown_variant.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_missing_variants() {
    let source = include_str!("../../../tests/fixtures/errors/missing_variants.ksr");
    let rendered = render_error(source, "missing_variants.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_extra_variants() {
    let source = include_str!("../../../tests/fixtures/errors/extra_variants.ksr");
    let rendered = render_error(source, "extra_variants.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_index_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/index_mismatch.ksr");
    let rendered = render_error(source, "index_mismatch.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_non_literal_exponent() {
    let source = include_str!("../../../tests/fixtures/errors/non_literal_exponent.ksr");
    let rendered = render_error(source, "non_literal_exponent.ksr");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_boolean_dim_error() {
    let source = include_str!("../../../tests/fixtures/errors/boolean_dim_error.ksr");
    let rendered = render_error(source, "boolean_dim_error.ksr");
    insta::assert_snapshot!(rendered);
}

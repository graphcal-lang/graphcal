//! Tests for error rendering snapshots.
#![allow(clippy::unwrap_used, reason = "test code")]

use graphcal_eval::eval::{NodeError, compile_and_eval_named};
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

/// Compile the given source and return the per-node error message for a specific node.
/// Used for runtime errors that are now contained per-node.
fn render_node_error(source: &str, name: &str, node_name: &str) -> String {
    let result = compile_and_eval_named(source, name).unwrap();
    let (_, node_result, _) = result
        .all
        .iter()
        .find(|(n, _, _)| n.as_str() == node_name)
        .unwrap_or_else(|| panic!("node `{node_name}` not found"));
    match node_result {
        Err(NodeError::EvalFailed { message }) => message.clone(),
        Err(other) => panic!("expected EvalFailed, got {other}"),
        Ok(val) => panic!("expected error for `{node_name}`, got {val:?}"),
    }
}

#[test]
fn error_duplicate_name() {
    let source = include_str!("../../../tests/fixtures/errors/duplicate.gcl");
    let rendered = render_error(source, "duplicate.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_graph_ref() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_ref.gcl");
    let rendered = render_error(source, "unknown_ref.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_const_ref() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_const_ref.gcl");
    let rendered = render_error(source, "unknown_const_ref.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_at_in_const() {
    let source = include_str!("../../../tests/fixtures/errors/at_in_const.gcl");
    let rendered = render_error(source, "at_in_const.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_bad_const_casing() {
    let source = include_str!("../../../tests/fixtures/errors/bad_const_casing.gcl");
    let rendered = render_error(source, "bad_const_casing.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_bad_param_casing() {
    let source = include_str!("../../../tests/fixtures/errors/bad_param_casing.gcl");
    let rendered = render_error(source, "bad_param_casing.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_runtime_cycle() {
    let source = include_str!("../../../tests/fixtures/errors/cycle.gcl");
    let rendered = render_error(source, "cycle.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_const_cycle() {
    let source = include_str!("../../../tests/fixtures/errors/const_cycle.gcl");
    let rendered = render_error(source, "const_cycle.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_function() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_function.gcl");
    let rendered = render_error(source, "unknown_function.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_wrong_arity() {
    let source = include_str!("../../../tests/fixtures/errors/wrong_arity.gcl");
    let rendered = render_error(source, "wrong_arity.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_dim_mismatch_add() {
    let source = include_str!("../../../tests/fixtures/errors/dim_mismatch_add.gcl");
    let rendered = render_error(source, "dim_mismatch_add.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_dim_mismatch_annotation() {
    let source = include_str!("../../../tests/fixtures/errors/dim_mismatch_annotation.gcl");
    let rendered = render_error(source, "dim_mismatch_annotation.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_exp_requires_dimensionless() {
    let source = include_str!("../../../tests/fixtures/errors/exp_requires_dimensionless.gcl");
    let rendered = render_error(source, "exp_requires_dimensionless.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_unit() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_unit.gcl");
    let rendered = render_error(source, "unknown_unit.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_conversion_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/conversion_dim_mismatch.gcl");
    let rendered = render_error(source, "conversion_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_struct_field() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_struct_field.gcl");
    let rendered = render_error(source, "unknown_struct_field.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_struct_field_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/struct_field_dim_mismatch.gcl");
    let rendered = render_error(source, "struct_field_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_duplicate_let() {
    let source = include_str!("../../../tests/fixtures/errors/duplicate_let.gcl");
    let rendered = render_error(source, "duplicate_let.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_missing_struct_field() {
    let source = include_str!("../../../tests/fixtures/errors/missing_struct_field.gcl");
    let rendered = render_error(source, "missing_struct_field.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_at_in_fn() {
    let source = include_str!("../../../tests/fixtures/errors/at_in_fn.gcl");
    let rendered = render_error(source, "at_in_fn.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_recursive_fn() {
    let source = include_str!("../../../tests/fixtures/errors/recursive_fn.gcl");
    let rendered = render_error(source, "recursive_fn.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_fn_generic_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/fn_generic_dim_mismatch.gcl");
    let rendered = render_error(source, "fn_generic_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_dimension() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_dimension.gcl");
    let rendered = render_error(source, "unknown_dimension.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_extra_struct_fields() {
    let source = include_str!("../../../tests/fixtures/errors/extra_struct_fields.gcl");
    let rendered = render_error(source, "extra_struct_fields.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_not_a_struct() {
    let source = include_str!("../../../tests/fixtures/errors/not_a_struct.gcl");
    let rendered = render_error(source, "not_a_struct.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_index() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_index.gcl");
    let rendered = render_error(source, "unknown_index.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_variant() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_variant.gcl");
    let rendered = render_error(source, "unknown_variant.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_missing_variants() {
    let source = include_str!("../../../tests/fixtures/errors/missing_variants.gcl");
    let rendered = render_error(source, "missing_variants.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_extra_variants() {
    let source = include_str!("../../../tests/fixtures/errors/extra_variants.gcl");
    let rendered = render_error(source, "extra_variants.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_index_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/index_mismatch.gcl");
    let rendered = render_error(source, "index_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_non_literal_exponent() {
    let source = include_str!("../../../tests/fixtures/errors/non_literal_exponent.gcl");
    let rendered = render_error(source, "non_literal_exponent.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_boolean_dim_error() {
    let source = include_str!("../../../tests/fixtures/errors/boolean_dim_error.gcl");
    let rendered = render_error(source, "boolean_dim_error.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_division_by_zero() {
    let source = include_str!("../../../tests/fixtures/errors/division_by_zero.gcl");
    let rendered = render_node_error(source, "division_by_zero.gcl", "y");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_sqrt_negative() {
    let source = include_str!("../../../tests/fixtures/errors/sqrt_negative.gcl");
    let rendered = render_node_error(source, "sqrt_negative.gcl", "y");
    insta::assert_snapshot!(rendered);
}

// --- Tagged union error tests ---

#[test]
fn error_non_exhaustive_match() {
    let source = include_str!("../../../tests/fixtures/errors/non_exhaustive_match.gcl");
    let rendered = render_error(source, "non_exhaustive_match.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_duplicate_match_arm() {
    let source = include_str!("../../../tests/fixtures/errors/duplicate_match_arm.gcl");
    let rendered = render_error(source, "duplicate_match_arm.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_match_arm_type_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/match_arm_type_mismatch.gcl");
    let rendered = render_error(source, "match_arm_type_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_field_access_multi_variant() {
    let source = include_str!("../../../tests/fixtures/errors/field_access_multi_variant.gcl");
    let rendered = render_error(source, "field_access_multi_variant.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Range index error tests ---

#[test]
fn error_range_index_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/errors/range_index_dim_mismatch.gcl");
    let rendered = render_error(source, "range_index_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_range_index_invalid() {
    let source = include_str!("../../../tests/fixtures/errors/range_index_invalid.gcl");
    let rendered = render_error(source, "range_index_invalid.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Assertion error tests ---

#[test]
fn error_at_assert() {
    let source = include_str!("../../../tests/fixtures/errors/at_assert.gcl");
    let rendered = render_error(source, "at_assert.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_assert_not_bool() {
    let source = include_str!("../../../tests/fixtures/errors/assert_not_bool.gcl");
    let rendered = render_error(source, "assert_not_bool.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_assumes_unknown_assert() {
    let source = include_str!("../../../tests/fixtures/errors/assumes_unknown_assert.gcl");
    let rendered = render_error(source, "assumes_unknown_assert.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_assumes_on_const() {
    let source = include_str!("../../../tests/fixtures/errors/assumes_on_const.gcl");
    let rendered = render_error(source, "assumes_on_const.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_attribute() {
    let source = include_str!("../../../tests/fixtures/errors/unknown_attribute.gcl");
    let rendered = render_error(source, "unknown_attribute.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Index variant match error tests ---

#[test]
fn error_non_exhaustive_index_match() {
    let source = include_str!("../../../tests/fixtures/errors/non_exhaustive_index_match.gcl");
    let rendered = render_error(source, "non_exhaustive_index_match.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_index_match_with_bindings() {
    let source = include_str!("../../../tests/fixtures/errors/index_match_with_bindings.gcl");
    let rendered = render_error(source, "index_match_with_bindings.gcl");
    insta::assert_snapshot!(rendered);
}

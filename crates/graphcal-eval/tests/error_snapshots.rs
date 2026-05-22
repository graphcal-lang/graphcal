//! Tests for error rendering snapshots.
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
    let source = include_str!("../../../tests/fixtures/invalid/duplicate.gcl");
    let rendered = render_error(source, "duplicate.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_graph_ref() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_ref.gcl");
    let rendered = render_error(source, "unknown_ref.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_const_ref() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_const_ref.gcl");
    let rendered = render_error(source, "unknown_const_ref.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_at_in_const() {
    let source = include_str!("../../../tests/fixtures/invalid/at_in_const.gcl");
    let rendered = render_error(source, "at_in_const.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_runtime_cycle() {
    let source = include_str!("../../../tests/fixtures/invalid/cycle.gcl");
    let rendered = render_error(source, "cycle.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_const_cycle() {
    let source = include_str!("../../../tests/fixtures/invalid/const_cycle.gcl");
    let rendered = render_error(source, "const_cycle.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_function() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_function.gcl");
    let rendered = render_error(source, "unknown_function.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_wrong_arity() {
    let source = include_str!("../../../tests/fixtures/invalid/wrong_arity.gcl");
    let rendered = render_error(source, "wrong_arity.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_dim_mismatch_add() {
    let source = include_str!("../../../tests/fixtures/invalid/dim_mismatch_add.gcl");
    let rendered = render_error(source, "dim_mismatch_add.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_dim_mismatch_annotation() {
    let source = include_str!("../../../tests/fixtures/invalid/dim_mismatch_annotation.gcl");
    let rendered = render_error(source, "dim_mismatch_annotation.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_exp_requires_dimensionless() {
    let source = include_str!("../../../tests/fixtures/invalid/exp_requires_dimensionless.gcl");
    let rendered = render_error(source, "exp_requires_dimensionless.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_unit() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_unit.gcl");
    let rendered = render_error(source, "unknown_unit.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_conversion_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/conversion_dim_mismatch.gcl");
    let rendered = render_error(source, "conversion_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_struct_field() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_struct_field.gcl");
    let rendered = render_error(source, "unknown_struct_field.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_struct_field_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/struct_field_dim_mismatch.gcl");
    let rendered = render_error(source, "struct_field_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_missing_struct_field() {
    let source = include_str!("../../../tests/fixtures/invalid/missing_struct_field.gcl");
    let rendered = render_error(source, "missing_struct_field.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_recursive_fn() {
    let source = include_str!("../../../tests/fixtures/invalid/recursive_fn.gcl");
    let rendered = render_error(source, "recursive_fn.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_dimension() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_dimension.gcl");
    let rendered = render_error(source, "unknown_dimension.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_extra_struct_fields() {
    let source = include_str!("../../../tests/fixtures/invalid/extra_struct_fields.gcl");
    let rendered = render_error(source, "extra_struct_fields.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_not_a_struct() {
    let source = include_str!("../../../tests/fixtures/invalid/not_a_struct.gcl");
    let rendered = render_error(source, "not_a_struct.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_index() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_index.gcl");
    let rendered = render_error(source, "unknown_index.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_variant() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_variant.gcl");
    let rendered = render_error(source, "unknown_variant.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_missing_variants() {
    let source = include_str!("../../../tests/fixtures/invalid/missing_variants.gcl");
    let rendered = render_error(source, "missing_variants.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_extra_variants() {
    let source = include_str!("../../../tests/fixtures/invalid/extra_variants.gcl");
    let rendered = render_error(source, "extra_variants.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_index_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/index_mismatch.gcl");
    let rendered = render_error(source, "index_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_non_literal_exponent() {
    let source = include_str!("../../../tests/fixtures/invalid/non_literal_exponent.gcl");
    let rendered = render_error(source, "non_literal_exponent.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_boolean_dim_error() {
    let source = include_str!("../../../tests/fixtures/invalid/boolean_dim_error.gcl");
    let rendered = render_error(source, "boolean_dim_error.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_division_by_zero() {
    let source = include_str!("../../../tests/fixtures/runtime_error/division_by_zero.gcl");
    let rendered = render_node_error(source, "division_by_zero.gcl", "y");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_sqrt_negative() {
    let source = include_str!("../../../tests/fixtures/runtime_error/sqrt_negative.gcl");
    let rendered = render_node_error(source, "sqrt_negative.gcl", "y");
    insta::assert_snapshot!(rendered);
}

// --- Tagged union error tests ---

#[test]
fn error_non_exhaustive_match() {
    let source = include_str!("../../../tests/fixtures/invalid/non_exhaustive_match.gcl");
    let rendered = render_error(source, "non_exhaustive_match.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_duplicate_match_arm() {
    let source = include_str!("../../../tests/fixtures/invalid/duplicate_match_arm.gcl");
    let rendered = render_error(source, "duplicate_match_arm.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_match_arm_type_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/match_arm_type_mismatch.gcl");
    let rendered = render_error(source, "match_arm_type_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_field_access_multi_variant() {
    let source = include_str!("../../../tests/fixtures/invalid/field_access_multi_variant.gcl");
    let rendered = render_error(source, "field_access_multi_variant.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Range index error tests ---

#[test]
fn error_range_index_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/range_index_dim_mismatch.gcl");
    let rendered = render_error(source, "range_index_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_range_index_invalid() {
    let source = include_str!("../../../tests/fixtures/invalid/range_index_invalid.gcl");
    let rendered = render_error(source, "range_index_invalid.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Assertion error tests ---

#[test]
fn error_at_assert() {
    let source = include_str!("../../../tests/fixtures/invalid/at_assert.gcl");
    let rendered = render_error(source, "at_assert.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_assert_not_bool() {
    let source = include_str!("../../../tests/fixtures/invalid/assert_not_bool.gcl");
    let rendered = render_error(source, "assert_not_bool.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_assumes_unknown_assert() {
    let source = include_str!("../../../tests/fixtures/invalid/assumes_unknown_assert.gcl");
    let rendered = render_error(source, "assumes_unknown_assert.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_assumes_on_const() {
    let source = include_str!("../../../tests/fixtures/invalid/assumes_on_const.gcl");
    let rendered = render_error(source, "assumes_on_const.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_unknown_attribute() {
    let source = include_str!("../../../tests/fixtures/invalid/unknown_attribute.gcl");
    let rendered = render_error(source, "unknown_attribute.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Expected-fail error tests ---

#[test]
fn error_expected_fail_on_node() {
    let source = include_str!("../../../tests/fixtures/invalid/expected_fail_on_node.gcl");
    let rendered = render_error(source, "expected_fail_on_node.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_expected_fail_all_on_indexed() {
    let source = include_str!("../../../tests/fixtures/invalid/expected_fail_all_on_indexed.gcl");
    let rendered = render_error(source, "expected_fail_all_on_indexed.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Index variant match error tests ---

#[test]
fn error_non_exhaustive_index_match() {
    let source = include_str!("../../../tests/fixtures/invalid/non_exhaustive_index_match.gcl");
    let rendered = render_error(source, "non_exhaustive_index_match.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_index_match_with_bindings() {
    let source = include_str!("../../../tests/fixtures/invalid/index_match_with_bindings.gcl");
    let rendered = render_error(source, "index_match_with_bindings.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_inline_dag_call_missing_projection() {
    let source = include_str!(
        "../../../tests/fixtures/invalid/inline_dag_call_errors/missing_projection.gcl"
    );
    let rendered = render_error(source, "missing_projection.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_inline_dag_call_qualified_missing_projection() {
    let source = include_str!(
        "../../../tests/fixtures/invalid/inline_dag_call_errors/qualified_missing_projection.gcl"
    );
    let rendered = render_error(source, "qualified_missing_projection.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_nested_indexed_map() {
    let source = include_str!("../../../tests/fixtures/invalid/nested_indexed_map.gcl");
    let rendered = render_error(source, "nested_indexed_map.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_table_row_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/table_row_mismatch.gcl");
    let rendered = render_error(source, "table_row_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_datetime_scale_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/datetime_scale_mismatch.gcl");
    let rendered = render_error(source, "datetime_scale_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_datetime_conversion_non_datetime() {
    let source =
        include_str!("../../../tests/fixtures/invalid/datetime_conversion_non_datetime.gcl");
    let rendered = render_error(source, "datetime_conversion_non_datetime.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_datetime_extract_non_datetime() {
    let source = include_str!("../../../tests/fixtures/invalid/datetime_extract_non_datetime.gcl");
    let rendered = render_error(source, "datetime_extract_non_datetime.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_datetime_timezone_non_datetime() {
    let source = include_str!("../../../tests/fixtures/invalid/datetime_timezone_non_datetime.gcl");
    let rendered = render_error(source, "datetime_timezone_non_datetime.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_invalid_timezone() {
    let source = include_str!("../../../tests/fixtures/invalid/invalid_timezone.gcl");
    let rendered = render_error(source, "invalid_timezone.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Domain constraint error tests ---

#[test]
fn error_domain_violation() {
    let source = include_str!("../../../tests/fixtures/runtime_error/domain_violation.gcl");
    let rendered = render_node_error(source, "domain_violation.gcl", "mass");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_min_exceeds_max() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_min_exceeds_max.gcl");
    let rendered = render_error(source, "domain_min_exceeds_max.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_on_bool() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_on_bool.gcl");
    let rendered = render_error(source, "domain_on_bool.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_invalid_key() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_invalid_key.gcl");
    let rendered = render_error(source, "domain_invalid_key.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Struct/union member field domain constraint errors (#450 Pos 1+2) ---

#[test]
fn error_domain_field_on_bool() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_field_on_bool.gcl");
    let rendered = render_error(source, "domain_field_on_bool.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_field_min_exceeds_max() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_field_min_exceeds_max.gcl");
    let rendered = render_error(source, "domain_field_min_exceeds_max.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_field_dim_mismatch() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_field_dim_mismatch.gcl");
    let rendered = render_error(source, "domain_field_dim_mismatch.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_field_const_violation() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_field_const_violation.gcl");
    let rendered = render_error(source, "domain_field_const_violation.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_field_runtime_violation() {
    let source =
        include_str!("../../../tests/fixtures/runtime_error/domain_field_runtime_violation.gcl");
    let rendered = render_node_error(source, "domain_field_runtime_violation.gcl", "SAT");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_domain_union_member_violation() {
    let source =
        include_str!("../../../tests/fixtures/runtime_error/domain_union_member_violation.gcl");
    let rendered = render_node_error(source, "domain_union_member_violation.gcl", "R");
    insta::assert_snapshot!(rendered);
}

// --- Position 4: domain constraint on a generic type argument ---

#[test]
fn error_domain_generic_arg_constraint() {
    let source = include_str!("../../../tests/fixtures/invalid/domain_generic_arg_constraint.gcl");
    let rendered = render_error(source, "domain_generic_arg_constraint.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_variant_literal_in_node() {
    let source = include_str!("../../../tests/fixtures/invalid/variant_literal_in_node.gcl");
    let rendered = render_error(source, "variant_literal_in_node.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_variant_literal_in_const() {
    let source = include_str!("../../../tests/fixtures/invalid/variant_literal_in_const.gcl");
    let rendered = render_error(source, "variant_literal_in_const.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_pub_assert_variant_literal() {
    let source = include_str!("../../../tests/fixtures/invalid/pub_assert_variant_literal.gcl");
    let rendered = render_error(source, "pub_assert_variant_literal.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_required_index_standalone() {
    let source = include_str!("../../../tests/fixtures/invalid/required_index_standalone.gcl");
    let rendered = render_error(source, "required_index_standalone.gcl");
    insta::assert_snapshot!(rendered);
}

// --- Visibility errors ---

#[test]
fn error_required_param_not_pub() {
    let source = include_str!("../../../tests/fixtures/runtime_error/required_param_not_pub.gcl");
    let rendered = render_error(source, "required_param_not_pub.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_required_index_not_pub() {
    let source = include_str!("../../../tests/fixtures/invalid/required_index_not_pub.gcl");
    let rendered = render_error(source, "required_index_not_pub.gcl");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_private_in_public() {
    let source = include_str!("../../../tests/fixtures/invalid/private_in_public.gcl");
    let rendered = render_error(source, "private_in_public.gcl");
    insta::assert_snapshot!(rendered);
}

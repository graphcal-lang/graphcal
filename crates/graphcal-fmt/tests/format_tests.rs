#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]

use graphcal_fmt::format_source;

// ---------------------------------------------------------------------------
// Idempotency: format(format(x)) == format(x)
// ---------------------------------------------------------------------------

macro_rules! idempotency_test {
    ($name:ident, $fixture:expr) => {
        #[test]
        fn $name() {
            let source = include_str!(concat!("../../../tests/fixtures/", $fixture));
            let formatted = format_source(source).expect("format_source should succeed");
            let reformatted = format_source(&formatted)
                .expect("format_source on formatted output should succeed");
            assert_eq!(
                formatted, reformatted,
                "Formatter is not idempotent for {}",
                $fixture
            );
        }
    };
}

idempotency_test!(idempotent_constants, "constants.gcl");
idempotency_test!(idempotent_functions, "functions.gcl");
idempotency_test!(idempotent_generics, "generics.gcl");
idempotency_test!(idempotent_hohmann, "hohmann.gcl");
idempotency_test!(idempotent_indexed, "indexed.gcl");
idempotency_test!(idempotent_integers, "integers.gcl");
idempotency_test!(idempotent_orbital, "orbital.gcl");
idempotency_test!(idempotent_range_index, "range_index.gcl");
idempotency_test!(idempotent_rocket, "rocket.gcl");
idempotency_test!(idempotent_tagged_union, "tagged_union.gcl");
idempotency_test!(idempotent_tagged_union_param, "tagged_union_param.gcl");
idempotency_test!(idempotent_table_literal, "table_literal.gcl");
idempotency_test!(idempotent_time_scan, "time_scan.gcl");
idempotency_test!(idempotent_user_dimensions, "user_dimensions.gcl");
idempotency_test!(idempotent_assertions, "assertions.gcl");
idempotency_test!(idempotent_assertions_fail, "assertions_fail.gcl");
idempotency_test!(
    idempotent_assertions_tolerance_fail,
    "assertions_tolerance_fail.gcl"
);
idempotency_test!(idempotent_assertions_assumes, "assertions_assumes.gcl");
idempotency_test!(idempotent_assertions_indexed, "assertions_indexed.gcl");
idempotency_test!(idempotent_plot_basic, "plot_basic.gcl");
idempotency_test!(idempotent_variant_comparison, "variant_comparison.gcl");
idempotency_test!(idempotent_variant_match, "variant_match.gcl");
idempotency_test!(idempotent_power_budget, "power_budget.gcl");
idempotency_test!(idempotent_thermal_analysis, "thermal_analysis.gcl");
idempotency_test!(idempotent_parenthesized_exprs, "parenthesized_exprs.gcl");
idempotency_test!(idempotent_expected_fail_pass, "expected_fail_pass.gcl");
idempotency_test!(
    idempotent_expected_fail_unexpected_pass,
    "expected_fail_unexpected_pass.gcl"
);
idempotency_test!(
    idempotent_expected_fail_indexed,
    "expected_fail_indexed.gcl"
);
idempotency_test!(
    idempotent_expected_fail_multi_indexed,
    "expected_fail_multi_indexed.gcl"
);
idempotency_test!(
    idempotent_expected_fail_indexed_partial,
    "expected_fail_indexed_partial.gcl"
);
idempotency_test!(
    idempotent_expected_fail_indexed_unexpected_pass,
    "expected_fail_indexed_unexpected_pass.gcl"
);
idempotency_test!(
    idempotent_expected_fail_multi_indexed_partial,
    "expected_fail_multi_indexed_partial.gcl"
);

// ---------------------------------------------------------------------------
// Round-trip: parse(format(x)) succeeds for all fixtures
// ---------------------------------------------------------------------------

macro_rules! roundtrip_test {
    ($name:ident, $fixture:expr) => {
        #[test]
        fn $name() {
            let source = include_str!(concat!("../../../tests/fixtures/", $fixture));
            let formatted = format_source(source).expect("format_source should succeed");
            let parse_result = graphcal_syntax::parser::Parser::new(&formatted).parse_file();
            assert!(
                parse_result.is_ok(),
                "Formatted output of {} failed to parse: {:?}",
                $fixture,
                parse_result.err()
            );
        }
    };
}

roundtrip_test!(roundtrip_constants, "constants.gcl");
roundtrip_test!(roundtrip_functions, "functions.gcl");
roundtrip_test!(roundtrip_generics, "generics.gcl");
roundtrip_test!(roundtrip_hohmann, "hohmann.gcl");
roundtrip_test!(roundtrip_indexed, "indexed.gcl");
roundtrip_test!(roundtrip_integers, "integers.gcl");
roundtrip_test!(roundtrip_orbital, "orbital.gcl");
roundtrip_test!(roundtrip_range_index, "range_index.gcl");
roundtrip_test!(roundtrip_rocket, "rocket.gcl");
roundtrip_test!(roundtrip_tagged_union, "tagged_union.gcl");
roundtrip_test!(roundtrip_tagged_union_param, "tagged_union_param.gcl");
roundtrip_test!(roundtrip_table_literal, "table_literal.gcl");
roundtrip_test!(roundtrip_time_scan, "time_scan.gcl");
roundtrip_test!(roundtrip_user_dimensions, "user_dimensions.gcl");
roundtrip_test!(roundtrip_assertions, "assertions.gcl");
roundtrip_test!(roundtrip_assertions_fail, "assertions_fail.gcl");
roundtrip_test!(
    roundtrip_assertions_tolerance_fail,
    "assertions_tolerance_fail.gcl"
);
roundtrip_test!(roundtrip_assertions_assumes, "assertions_assumes.gcl");
roundtrip_test!(roundtrip_assertions_indexed, "assertions_indexed.gcl");
roundtrip_test!(roundtrip_plot_basic, "plot_basic.gcl");
roundtrip_test!(roundtrip_variant_comparison, "variant_comparison.gcl");
roundtrip_test!(roundtrip_variant_match, "variant_match.gcl");
roundtrip_test!(roundtrip_power_budget, "power_budget.gcl");
roundtrip_test!(roundtrip_thermal_analysis, "thermal_analysis.gcl");
roundtrip_test!(roundtrip_parenthesized_exprs, "parenthesized_exprs.gcl");
roundtrip_test!(roundtrip_expected_fail_pass, "expected_fail_pass.gcl");
roundtrip_test!(
    roundtrip_expected_fail_unexpected_pass,
    "expected_fail_unexpected_pass.gcl"
);
roundtrip_test!(roundtrip_expected_fail_indexed, "expected_fail_indexed.gcl");
roundtrip_test!(
    roundtrip_expected_fail_multi_indexed,
    "expected_fail_multi_indexed.gcl"
);
roundtrip_test!(
    roundtrip_expected_fail_indexed_partial,
    "expected_fail_indexed_partial.gcl"
);
roundtrip_test!(
    roundtrip_expected_fail_indexed_unexpected_pass,
    "expected_fail_indexed_unexpected_pass.gcl"
);
roundtrip_test!(
    roundtrip_expected_fail_multi_indexed_partial,
    "expected_fail_multi_indexed_partial.gcl"
);

// ---------------------------------------------------------------------------
// Comment preservation
// ---------------------------------------------------------------------------

#[test]
fn preserves_leading_comment() {
    let source = "// This is a comment\nparam x: Dimensionless = 1.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// This is a comment"),
        "Leading comment was lost: {formatted}"
    );
}

#[test]
fn preserves_inline_comment() {
    let source = "param x: Dimensionless = 1.0; // inline\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// inline"),
        "Inline comment was lost: {formatted}"
    );
}

#[test]
fn preserves_doc_comment() {
    let source = "/// Doc comment\nparam x: Dimensionless = 1.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("/// Doc comment"),
        "Doc comment was lost: {formatted}"
    );
}

#[test]
fn preserves_multiple_comments() {
    let source = "// First\n// Second\nparam x: Dimensionless = 1.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// First"),
        "First comment lost: {formatted}"
    );
    assert!(
        formatted.contains("// Second"),
        "Second comment lost: {formatted}"
    );
}

#[test]
fn preserves_blank_line_between_declarations() {
    let source = "param x: Dimensionless = 1.0;\n\nparam y: Dimensionless = 2.0;\n";
    let formatted = format_source(source).unwrap();
    // Should have a blank line between declarations
    assert!(
        formatted.contains(";\n\nparam y"),
        "Blank line between declarations was lost: {formatted}"
    );
}

// ---------------------------------------------------------------------------
// Specific formatting rules
// ---------------------------------------------------------------------------

#[test]
fn trailing_newline() {
    let source = "param x: Dimensionless = 1.0;";
    let formatted = format_source(source).unwrap();
    assert!(formatted.ends_with('\n'), "Missing trailing newline");
}

#[test]
fn parse_error_returns_none() {
    let source = "this is not valid gcl }{}{";
    assert!(format_source(source).is_none());
}

#[test]
fn format_dimension_decl() {
    let source = "dimension Velocity = Length / Time;\n";
    let formatted = format_source(source).unwrap();
    assert_eq!(formatted, "dimension Velocity = Length / Time;\n");
}

#[test]
fn format_base_dimension() {
    let source = "dimension Length;\n";
    let formatted = format_source(source).unwrap();
    assert_eq!(formatted, "dimension Length;\n");
}

#[test]
fn format_unit_decl() {
    let source = "unit km: Length = 1000 m;\n";
    let formatted = format_source(source).unwrap();
    assert_eq!(formatted, "unit km: Length = 1000 m;\n");
}

#[test]
fn format_binary_op_precedence_preserved() {
    let source = "node x: Dimensionless = (1.0 + 2.0) * 3.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("(1.0 + 2.0) * 3.0"),
        "Parentheses for precedence were lost: {formatted}"
    );
}

#[test]
fn format_no_unnecessary_parens() {
    let source = "node x: Dimensionless = 1.0 + 2.0 * 3.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("1.0 + 2.0 * 3.0"),
        "Unnecessary parens added: {formatted}"
    );
}

#[test]
fn format_attribute_no_args() {
    let source = "#[lazy]\nnode x: Dimensionless = 1.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("#[lazy]\nnode x"),
        "Attribute not preserved: {formatted}"
    );
}

#[test]
fn format_attribute_with_args() {
    let source = "#[assumes(pressure_safe, temp_bounded)]\nnode x: Dimensionless = 1.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("#[assumes(pressure_safe, temp_bounded)]"),
        "Attribute args not preserved: {formatted}"
    );
}

#[test]
fn format_multiple_attributes() {
    let source = "#[lazy]\n#[assumes(x)]\nnode y: Dimensionless = 1.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("#[lazy]\n#[assumes(x)]\nnode y"),
        "Multiple attributes not preserved: {formatted}"
    );
}

#[test]
fn format_assert_bool() {
    let source = "param x: Dimensionless = 1.0;\nassert check = @x > 0.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("assert check = @x > 0.0;"),
        "Assert formatting incorrect: {formatted}"
    );
}

#[test]
fn format_assert_tolerance() {
    let source = "param x: Dimensionless = 1.0;\nassert check = @x ~= 1.0 +/- 0.1;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("assert check = @x ~= 1.0 +/- 0.1;"),
        "Assert tolerance formatting incorrect: {formatted}"
    );
}

#[test]
fn format_assert_tolerance_relative() {
    let source = "param x: Dimensionless = 1.0;\nassert check = @x ~= 1.0 +/- 5 %;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("assert check = @x ~= 1.0 +/- 5%;"),
        "Assert relative tolerance formatting incorrect: {formatted}"
    );
}

// ---------------------------------------------------------------------------
// Table literal formatting
// ---------------------------------------------------------------------------

#[test]
fn format_table_1d_preserves_syntax() {
    let source = r"
cat Maneuver { Departure, Correction, Insertion }
param dv: Dimensionless[Maneuver] = table[Maneuver] {
    Departure: 2.46;
    Correction: 0.12;
    Insertion: 1.83;
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("table[Maneuver]"),
        "1D table syntax not preserved: {formatted}"
    );
    assert!(
        !formatted.contains("Maneuver::"),
        "1D table should not use qualified syntax: {formatted}"
    );
    assert!(
        formatted.contains("Departure:"),
        "1D table row labels missing: {formatted}"
    );
}

#[test]
fn format_table_1d_aligns_values() {
    let source = r"
cat Maneuver { Departure, Correction, Insertion }
param dv: Dimensionless[Maneuver] = table[Maneuver] {
    Departure: 2.46;
    Correction: 0.12;
    Insertion: 1.83;
};
";
    let formatted = format_source(source).unwrap();
    // Values should be right-aligned (semicolons at the same column)
    let lines: Vec<&str> = formatted.lines().collect();
    let semicolon_positions: Vec<usize> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with(|c: char| c.is_uppercase()) && l.ends_with(';'))
        .filter_map(|l| l.rfind(';'))
        .collect();
    assert!(
        !semicolon_positions.is_empty(),
        "No table rows found: {formatted}"
    );
    assert!(
        semicolon_positions.windows(2).all(|w| w[0] == w[1]),
        "Values not aligned in 1D table: positions={semicolon_positions:?}\n{formatted}"
    );
}

#[test]
fn format_table_2d_preserves_syntax() {
    let source = r"
cat Phase { Launch, Cruise }
cat Maneuver { Departure, Correction }
param m: Dimensionless[Phase, Maneuver] = table[Phase, Maneuver] {
    Departure, Correction;
    Launch: 5000.0, 0.0;
    Cruise: 0.0, 4500.0;
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("table[Phase, Maneuver]"),
        "2D table syntax not preserved: {formatted}"
    );
    assert!(
        formatted.contains("Departure,"),
        "2D table header row missing: {formatted}"
    );
    assert!(
        !formatted.contains("Phase::"),
        "2D table should not use qualified syntax: {formatted}"
    );
}

#[test]
fn format_map_literal_not_converted_to_table() {
    let source = r"
cat Maneuver { Departure, Correction }
param dv: Dimensionless[Maneuver] = {
    Maneuver::Departure: 2.46,
    Maneuver::Correction: 0.12,
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        !formatted.contains("table["),
        "Map literal should not be converted to table: {formatted}"
    );
    assert!(
        formatted.contains("Maneuver::Departure"),
        "Map literal should use qualified syntax: {formatted}"
    );
}

// ---------------------------------------------------------------------------
// Snapshots: capture exact formatted output for each fixture
// ---------------------------------------------------------------------------

macro_rules! snapshot_test {
    ($name:ident, $fixture:expr) => {
        #[test]
        fn $name() {
            let source = include_str!(concat!("../../../tests/fixtures/", $fixture));
            let formatted = format_source(source).expect("format_source should succeed");
            insta::assert_snapshot!(formatted);
        }
    };
}

snapshot_test!(snapshot_constants, "constants.gcl");
snapshot_test!(snapshot_functions, "functions.gcl");
snapshot_test!(snapshot_generics, "generics.gcl");
snapshot_test!(snapshot_hohmann, "hohmann.gcl");
snapshot_test!(snapshot_indexed, "indexed.gcl");
snapshot_test!(snapshot_integers, "integers.gcl");
snapshot_test!(snapshot_orbital, "orbital.gcl");
snapshot_test!(snapshot_range_index, "range_index.gcl");
snapshot_test!(snapshot_rocket, "rocket.gcl");
snapshot_test!(snapshot_tagged_union, "tagged_union.gcl");
snapshot_test!(snapshot_tagged_union_param, "tagged_union_param.gcl");
snapshot_test!(snapshot_table_literal, "table_literal.gcl");
snapshot_test!(snapshot_time_scan, "time_scan.gcl");
snapshot_test!(snapshot_user_dimensions, "user_dimensions.gcl");
snapshot_test!(snapshot_assertions, "assertions.gcl");
snapshot_test!(snapshot_assertions_fail, "assertions_fail.gcl");
snapshot_test!(
    snapshot_assertions_tolerance_fail,
    "assertions_tolerance_fail.gcl"
);
snapshot_test!(snapshot_assertions_assumes, "assertions_assumes.gcl");
snapshot_test!(snapshot_assertions_indexed, "assertions_indexed.gcl");
snapshot_test!(snapshot_plot_basic, "plot_basic.gcl");
snapshot_test!(snapshot_variant_comparison, "variant_comparison.gcl");
snapshot_test!(snapshot_variant_match, "variant_match.gcl");
snapshot_test!(snapshot_power_budget, "power_budget.gcl");
snapshot_test!(snapshot_thermal_analysis, "thermal_analysis.gcl");
snapshot_test!(snapshot_parenthesized_exprs, "parenthesized_exprs.gcl");
snapshot_test!(snapshot_expected_fail_pass, "expected_fail_pass.gcl");
snapshot_test!(
    snapshot_expected_fail_unexpected_pass,
    "expected_fail_unexpected_pass.gcl"
);
snapshot_test!(snapshot_expected_fail_indexed, "expected_fail_indexed.gcl");
snapshot_test!(
    snapshot_expected_fail_multi_indexed,
    "expected_fail_multi_indexed.gcl"
);
snapshot_test!(
    snapshot_expected_fail_indexed_partial,
    "expected_fail_indexed_partial.gcl"
);
snapshot_test!(
    snapshot_expected_fail_indexed_unexpected_pass,
    "expected_fail_indexed_unexpected_pass.gcl"
);
snapshot_test!(
    snapshot_expected_fail_multi_indexed_partial,
    "expected_fail_multi_indexed_partial.gcl"
);
snapshot_test!(
    snapshot_comments_in_expressions,
    "comments_in_expressions.gcl"
);

// ---------------------------------------------------------------------------
// Fixture: comments_in_expressions.gcl
// ---------------------------------------------------------------------------

idempotency_test!(
    idempotent_comments_in_expressions,
    "comments_in_expressions.gcl"
);
roundtrip_test!(
    roundtrip_comments_in_expressions,
    "comments_in_expressions.gcl"
);

// ---------------------------------------------------------------------------
// Multi-file fixtures: import syntax, module imports, qualified references
// ---------------------------------------------------------------------------

// alias: selective import with renaming
idempotency_test!(idempotent_multi_alias_main, "multi/alias/main.gcl");
idempotency_test!(idempotent_multi_alias_helper, "multi/alias/helper.gcl");
roundtrip_test!(roundtrip_multi_alias_main, "multi/alias/main.gcl");
roundtrip_test!(roundtrip_multi_alias_helper, "multi/alias/helper.gcl");
snapshot_test!(snapshot_multi_alias_main, "multi/alias/main.gcl");
snapshot_test!(snapshot_multi_alias_helper, "multi/alias/helper.gcl");

// alias_conflict: multiple imports with renaming
idempotency_test!(
    idempotent_multi_alias_conflict_main,
    "multi/alias_conflict/main.gcl"
);
idempotency_test!(
    idempotent_multi_alias_conflict_a,
    "multi/alias_conflict/a.gcl"
);
idempotency_test!(
    idempotent_multi_alias_conflict_b,
    "multi/alias_conflict/b.gcl"
);
roundtrip_test!(
    roundtrip_multi_alias_conflict_main,
    "multi/alias_conflict/main.gcl"
);
snapshot_test!(
    snapshot_multi_alias_conflict_main,
    "multi/alias_conflict/main.gcl"
);

// module_import: whole-module import
idempotency_test!(
    idempotent_multi_module_import_main,
    "multi/module_import/main.gcl"
);
idempotency_test!(
    idempotent_multi_module_import_constants,
    "multi/module_import/constants.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_main,
    "multi/module_import/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_main,
    "multi/module_import/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_constants,
    "multi/module_import/constants.gcl"
);

// module_import_alias: import with alias
idempotency_test!(
    idempotent_multi_module_import_alias_main,
    "multi/module_import_alias/main.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_alias_main,
    "multi/module_import_alias/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_alias_main,
    "multi/module_import_alias/main.gcl"
);

// module_import_fn: qualified function call
idempotency_test!(
    idempotent_multi_module_import_fn_main,
    "multi/module_import_fn/main.gcl"
);
idempotency_test!(
    idempotent_multi_module_import_fn_lib,
    "multi/module_import_fn/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_fn_main,
    "multi/module_import_fn/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_fn_main,
    "multi/module_import_fn/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_fn_lib,
    "multi/module_import_fn/lib.gcl"
);

// module_import_graph_ref: qualified @-references
idempotency_test!(
    idempotent_multi_module_import_graph_ref_main,
    "multi/module_import_graph_ref/main.gcl"
);
idempotency_test!(
    idempotent_multi_module_import_graph_ref_params,
    "multi/module_import_graph_ref/params.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_graph_ref_main,
    "multi/module_import_graph_ref/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_graph_ref_main,
    "multi/module_import_graph_ref/main.gcl"
);

// module_import_mixed: selective + module imports in same file
idempotency_test!(
    idempotent_multi_module_import_mixed_main,
    "multi/module_import_mixed/main.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_mixed_main,
    "multi/module_import_mixed/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_mixed_main,
    "multi/module_import_mixed/main.gcl"
);

// mission_plan: complex multi-file project with subdirectories
idempotency_test!(
    idempotent_multi_mission_plan_main,
    "multi/mission_plan/main.gcl"
);
idempotency_test!(
    idempotent_multi_mission_plan_constants,
    "multi/mission_plan/shared/constants.gcl"
);
idempotency_test!(
    idempotent_multi_mission_plan_indexes,
    "multi/mission_plan/shared/indexes.gcl"
);
idempotency_test!(
    idempotent_multi_mission_plan_power,
    "multi/mission_plan/subsystems/power.gcl"
);
idempotency_test!(
    idempotent_multi_mission_plan_propulsion,
    "multi/mission_plan/subsystems/propulsion.gcl"
);
roundtrip_test!(
    roundtrip_multi_mission_plan_main,
    "multi/mission_plan/main.gcl"
);
snapshot_test!(
    snapshot_multi_mission_plan_main,
    "multi/mission_plan/main.gcl"
);
snapshot_test!(
    snapshot_multi_mission_plan_constants,
    "multi/mission_plan/shared/constants.gcl"
);
snapshot_test!(
    snapshot_multi_mission_plan_indexes,
    "multi/mission_plan/shared/indexes.gcl"
);
snapshot_test!(
    snapshot_multi_mission_plan_power,
    "multi/mission_plan/subsystems/power.gcl"
);
snapshot_test!(
    snapshot_multi_mission_plan_propulsion,
    "multi/mission_plan/subsystems/propulsion.gcl"
);

// rocket_split: selective import with many identifiers
idempotency_test!(
    idempotent_multi_rocket_split_main,
    "multi/rocket_split/main.gcl"
);
idempotency_test!(
    idempotent_multi_rocket_split_constants,
    "multi/rocket_split/constants.gcl"
);
idempotency_test!(
    idempotent_multi_rocket_split_params,
    "multi/rocket_split/params.gcl"
);
roundtrip_test!(
    roundtrip_multi_rocket_split_main,
    "multi/rocket_split/main.gcl"
);
snapshot_test!(
    snapshot_multi_rocket_split_main,
    "multi/rocket_split/main.gcl"
);
snapshot_test!(
    snapshot_multi_rocket_split_constants,
    "multi/rocket_split/constants.gcl"
);
snapshot_test!(
    snapshot_multi_rocket_split_params,
    "multi/rocket_split/params.gcl"
);

// parent_import: importing from parent directory
idempotency_test!(
    idempotent_multi_parent_import_child_main,
    "multi/parent_import/child/main.gcl"
);
idempotency_test!(
    idempotent_multi_parent_import_lib,
    "multi/parent_import/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_parent_import_child_main,
    "multi/parent_import/child/main.gcl"
);
snapshot_test!(
    snapshot_multi_parent_import_child_main,
    "multi/parent_import/child/main.gcl"
);
snapshot_test!(
    snapshot_multi_parent_import_lib,
    "multi/parent_import/lib.gcl"
);

// parent_import_with_manifest
idempotency_test!(
    idempotent_multi_parent_import_manifest_child_main,
    "multi/parent_import_with_manifest/child/main.gcl"
);
idempotency_test!(
    idempotent_multi_parent_import_manifest_lib,
    "multi/parent_import_with_manifest/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_parent_import_manifest_child_main,
    "multi/parent_import_with_manifest/child/main.gcl"
);
snapshot_test!(
    snapshot_multi_parent_import_manifest_child_main,
    "multi/parent_import_with_manifest/child/main.gcl"
);

// explicit_index: import of index types
idempotency_test!(
    idempotent_multi_explicit_index_main,
    "multi/explicit_index/main.gcl"
);
idempotency_test!(
    idempotent_multi_explicit_index_lib,
    "multi/explicit_index/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_explicit_index_main,
    "multi/explicit_index/main.gcl"
);
snapshot_test!(
    snapshot_multi_explicit_index_main,
    "multi/explicit_index/main.gcl"
);

// diamond_assert: diamond-shaped import graph
idempotency_test!(
    idempotent_multi_diamond_assert_main,
    "multi/diamond_assert/main.gcl"
);
idempotency_test!(
    idempotent_multi_diamond_assert_shared,
    "multi/diamond_assert/shared.gcl"
);
idempotency_test!(
    idempotent_multi_diamond_assert_left,
    "multi/diamond_assert/left.gcl"
);
idempotency_test!(
    idempotent_multi_diamond_assert_right,
    "multi/diamond_assert/right.gcl"
);
roundtrip_test!(
    roundtrip_multi_diamond_assert_main,
    "multi/diamond_assert/main.gcl"
);
snapshot_test!(
    snapshot_multi_diamond_assert_main,
    "multi/diamond_assert/main.gcl"
);

// assertions: cross-file assertions with #[assumes]
idempotency_test!(
    idempotent_multi_assertions_main,
    "multi/assertions/main.gcl"
);
idempotency_test!(
    idempotent_multi_assertions_checks,
    "multi/assertions/checks.gcl"
);
roundtrip_test!(roundtrip_multi_assertions_main, "multi/assertions/main.gcl");
snapshot_test!(snapshot_multi_assertions_main, "multi/assertions/main.gcl");
snapshot_test!(
    snapshot_multi_assertions_checks,
    "multi/assertions/checks.gcl"
);

// auto_assert: auto-evaluated assertions from imports
idempotency_test!(
    idempotent_multi_auto_assert_main,
    "multi/auto_assert/main.gcl"
);
idempotency_test!(
    idempotent_multi_auto_assert_lib,
    "multi/auto_assert/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_auto_assert_main,
    "multi/auto_assert/main.gcl"
);
snapshot_test!(
    snapshot_multi_auto_assert_main,
    "multi/auto_assert/main.gcl"
);
snapshot_test!(snapshot_multi_auto_assert_lib, "multi/auto_assert/lib.gcl");

// auto_assert_module: module import with auto assertions
idempotency_test!(
    idempotent_multi_auto_assert_module_main,
    "multi/auto_assert_module/main.gcl"
);
idempotency_test!(
    idempotent_multi_auto_assert_module_lib,
    "multi/auto_assert_module/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_auto_assert_module_main,
    "multi/auto_assert_module/main.gcl"
);
snapshot_test!(
    snapshot_multi_auto_assert_module_main,
    "multi/auto_assert_module/main.gcl"
);

// imported_deps: import with internal graph dependencies
idempotency_test!(
    idempotent_multi_imported_deps_main,
    "multi/imported_deps/main.gcl"
);
idempotency_test!(
    idempotent_multi_imported_deps_lib,
    "multi/imported_deps/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_imported_deps_main,
    "multi/imported_deps/main.gcl"
);
snapshot_test!(
    snapshot_multi_imported_deps_main,
    "multi/imported_deps/main.gcl"
);

// imported_assert_fail: import with failing assertion
idempotency_test!(
    idempotent_multi_imported_assert_fail_main,
    "multi/imported_assert_fail/main.gcl"
);
idempotency_test!(
    idempotent_multi_imported_assert_fail_lib,
    "multi/imported_assert_fail/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_imported_assert_fail_main,
    "multi/imported_assert_fail/main.gcl"
);
snapshot_test!(
    snapshot_multi_imported_assert_fail_main,
    "multi/imported_assert_fail/main.gcl"
);

// bad_name_import, missing_import, circular: error-case import syntax (still parseable)
idempotency_test!(
    idempotent_multi_bad_name_import,
    "multi/bad_name_import.gcl"
);
idempotency_test!(idempotent_multi_missing_import, "multi/missing_import.gcl");
idempotency_test!(idempotent_multi_circular_a, "multi/circular_a.gcl");
idempotency_test!(idempotent_multi_circular_b, "multi/circular_b.gcl");
idempotency_test!(idempotent_multi_helper, "multi/helper.gcl");
roundtrip_test!(roundtrip_multi_bad_name_import, "multi/bad_name_import.gcl");
roundtrip_test!(roundtrip_multi_missing_import, "multi/missing_import.gcl");
roundtrip_test!(roundtrip_multi_circular_a, "multi/circular_a.gcl");
roundtrip_test!(roundtrip_multi_circular_b, "multi/circular_b.gcl");
snapshot_test!(snapshot_multi_bad_name_import, "multi/bad_name_import.gcl");
snapshot_test!(snapshot_multi_missing_import, "multi/missing_import.gcl");
snapshot_test!(snapshot_multi_circular_a, "multi/circular_a.gcl");
snapshot_test!(snapshot_multi_circular_b, "multi/circular_b.gcl");

// ---------------------------------------------------------------------------
// Edge-case fixture: long lines, deep nesting, complex expressions
// ---------------------------------------------------------------------------

idempotency_test!(idempotent_format_edge_cases, "format_edge_cases.gcl");
roundtrip_test!(roundtrip_format_edge_cases, "format_edge_cases.gcl");
snapshot_test!(snapshot_format_edge_cases, "format_edge_cases.gcl");

// ---------------------------------------------------------------------------
// Comment-in-expression preservation tests
// ---------------------------------------------------------------------------

#[test]
fn preserves_trailing_comment_in_1d_table() {
    let source = r"
cat Maneuver { Departure, Correction, Insertion }
param dv: Dimensionless[Maneuver] = table[Maneuver] {
    Departure:  2.46; // departure burn
    Correction: 0.12; // midcourse
    Insertion:  1.83; // insertion
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// departure burn"),
        "Trailing comment on 1D table row lost: {formatted}"
    );
    assert!(
        formatted.contains("// midcourse"),
        "Trailing comment on 1D table row lost: {formatted}"
    );
    // Comments should be on the same line as the row
    for line in formatted.lines() {
        if line.contains("// departure burn") {
            assert!(
                line.contains("Departure"),
                "Trailing comment not on same line as row: {formatted}"
            );
        }
    }
}

#[test]
fn preserves_leading_comment_between_table_rows() {
    let source = r"
cat Maneuver { Departure, Correction, Insertion }
param dv: Dimensionless[Maneuver] = table[Maneuver] {
    // first
    Departure:  2.46;
    // second
    Correction: 0.12;
    // third
    Insertion:  1.83;
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// first"),
        "Leading comment between table rows lost: {formatted}"
    );
    assert!(
        formatted.contains("// second"),
        "Leading comment between table rows lost: {formatted}"
    );
    // Ensure the comment appears before the row, not after the declaration
    let first_pos = formatted.find("// first").unwrap();
    let departure_pos = formatted.find("Departure:").unwrap();
    assert!(
        first_pos < departure_pos,
        "Leading comment should appear before row label: {formatted}"
    );
}

#[test]
fn preserves_comment_in_block_let() {
    let source = r"
fn f(x: Dimensionless) -> Dimensionless {
    // before a
    let a = x * 2.0;
    // before b
    let b = a + 1.0;
    // before result
    b
}
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// before a"),
        "Comment before let binding lost: {formatted}"
    );
    assert!(
        formatted.contains("// before b"),
        "Comment before second let binding lost: {formatted}"
    );
    assert!(
        formatted.contains("// before result"),
        "Comment before tail expression lost: {formatted}"
    );
    // Order check
    let a_comment = formatted.find("// before a").unwrap();
    let a_let = formatted.find("let a").unwrap();
    assert!(
        a_comment < a_let,
        "Comment should appear before its let binding: {formatted}"
    );
}

#[test]
fn preserves_comment_in_match_arms() {
    let source = r"
cat Phase { Coast, Burn }
node x: Dimensionless[Phase] = for p: Phase {
    match p {
        // coasting
        Phase::Coast => 0.0,
        // burning
        Phase::Burn => 1.0,
    }
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// coasting"),
        "Comment before match arm lost: {formatted}"
    );
    assert!(
        formatted.contains("// burning"),
        "Comment before match arm lost: {formatted}"
    );
    let coast_comment = formatted.find("// coasting").unwrap();
    let coast_arm = formatted.find("Phase::Coast =>").unwrap();
    assert!(
        coast_comment < coast_arm,
        "Comment should appear before its match arm: {formatted}"
    );
}

#[test]
fn preserves_trailing_comment_in_map_literal() {
    let source = r"
cat Maneuver { Departure, Correction }
param dv: Dimensionless[Maneuver] = {
    Maneuver::Departure: 2.46, // departure
    Maneuver::Correction: 0.12, // correction
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// departure"),
        "Trailing comment on map entry lost: {formatted}"
    );
    assert!(
        formatted.contains("// correction"),
        "Trailing comment on map entry lost: {formatted}"
    );
    // Comment should be on the same line as the entry
    for line in formatted.lines() {
        if line.contains("// departure") {
            assert!(
                line.contains("Departure"),
                "Trailing comment not on same line as map entry: {formatted}"
            );
        }
    }
}

#[test]
fn preserves_leading_comment_in_map_literal() {
    let source = r"
cat Maneuver { Departure, Correction }
param dv: Dimensionless[Maneuver] = {
    // departure entry
    Maneuver::Departure: 2.46,
    // correction entry
    Maneuver::Correction: 0.12,
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// departure entry"),
        "Leading comment on map entry lost: {formatted}"
    );
    let comment_pos = formatted.find("// departure entry").unwrap();
    let entry_pos = formatted.find("Maneuver::Departure").unwrap();
    assert!(
        comment_pos < entry_pos,
        "Leading comment should appear before map entry: {formatted}"
    );
}

#[test]
fn preserves_trailing_comment_on_3d_table_slice_header() {
    let source = r"
cat Scenario { Nominal, Contingency }
cat Phase { Launch, Cruise, Arrival }
cat Maneuver { Departure, Correction, Insertion }
param mass_3d: Dimensionless[Scenario, Phase, Maneuver] = table[Scenario, Phase, Maneuver] {
    [Scenario::Nominal] // nominal scenario
             Departure, Correction, Insertion;
    Launch:  5000.0,        0.0,       0.0;
    Cruise:     0.0,     4500.0,       0.0;
    Arrival:    0.0,        0.0,    4000.0;

    [Scenario::Contingency] // contingency scenario
             Departure, Correction, Insertion;
    Launch:  4800.0,        0.0,       0.0;
    Cruise:     0.0,     4200.0,       0.0;
    Arrival:    0.0,        0.0,    3800.0;
};
";
    let formatted = format_source(source).unwrap();
    // Both trailing comments must survive
    assert!(
        formatted.contains("// nominal scenario"),
        "Trailing comment on slice header lost: {formatted}"
    );
    assert!(
        formatted.contains("// contingency scenario"),
        "Trailing comment on slice header lost: {formatted}"
    );
    // Each comment must be on the same line as its slice header
    for line in formatted.lines() {
        if line.contains("// nominal scenario") {
            assert!(
                line.contains("[Scenario::Nominal]"),
                "Comment not on same line as slice header: {formatted}"
            );
        }
        if line.contains("// contingency scenario") {
            assert!(
                line.contains("[Scenario::Contingency]"),
                "Comment not on same line as slice header: {formatted}"
            );
        }
    }
}

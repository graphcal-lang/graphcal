use graphcal_fmt::format_source;

// ---------------------------------------------------------------------------
// Idempotency: format(format(x)) == format(x)
//
// Only `invalid/` (parseable-but-rejected) fixtures are exercised here. For
// well-formed fixtures the stronger invariant `format(x) == x` is enforced by
// `well_formed_fixtures_are_formatted` in the CLI test suite, which makes
// idempotency on them trivially true.
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

idempotency_test!(idempotent_functions, "invalid/functions.gcl");

// ---------------------------------------------------------------------------
// Round-trip: parse(format(x)) succeeds
//
// As with idempotency, only `invalid/` fixtures are exercised here; well-formed
// fixtures parse-round-trip trivially under the `format(x) == x` invariant.
// ---------------------------------------------------------------------------

macro_rules! roundtrip_test {
    ($name:ident, $fixture:expr) => {
        #[test]
        fn $name() {
            let source = include_str!(concat!("../../../tests/fixtures/", $fixture));
            let formatted = format_source(source).expect("format_source should succeed");
            let parse_result =
                graphcal_compiler::syntax::parser::Parser::new(&formatted).parse_file();
            assert!(
                parse_result.is_ok(),
                "Formatted output of {} failed to parse: {:?}",
                $fixture,
                parse_result.err()
            );
        }
    };
}

roundtrip_test!(roundtrip_functions, "invalid/functions.gcl");

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
fn does_not_insert_blank_lines_between_dag_declarations() {
    let source = "dag sample {\n    param x: Dimensionless;\n    param y: Dimensionless;\n}\n";
    let formatted = format_source(source).unwrap();

    for line in formatted.lines() {
        assert!(
            !line.ends_with([' ', '\t']),
            "formatted line has trailing whitespace: {line:?}\n{formatted}"
        );
    }
    assert!(
        formatted.contains(";\n    param y"),
        "formatter should not insert a blank line between DAG declarations: {formatted}"
    );
    assert!(
        !formatted.contains(";\n\n    param y"),
        "formatter inserted a blank line between DAG declarations: {formatted}"
    );
}

#[test]
fn preserves_existing_blank_lines_between_dag_declarations() {
    let source = "dag sample {\n    param x: Dimensionless;\n\n    param y: Dimensionless;\n}\n";
    let formatted = format_source(source).unwrap();

    assert!(
        formatted.contains(";\n\n    param y"),
        "formatter should preserve an existing blank line between DAG declarations: {formatted}"
    );
}

#[test]
fn parse_error_returns_err() {
    let source = "this is not valid gcl }{}{";
    let err = format_source(source).expect_err("expected parse error");
    assert!(matches!(err, graphcal_fmt::FormatError::Parse(_)));
}

#[test]
fn format_dimension_decl() {
    let source = "dim Velocity = Length / Time;\n";
    let formatted = format_source(source).unwrap();
    assert_eq!(formatted, "dim Velocity = Length / Time;\n");
}

#[test]
fn format_type_import_item() {
    let source = "import school.records.{pub type Student as Pupil, Student};";
    let formatted = format_source(source).unwrap();
    assert_eq!(
        formatted,
        "import school.records.{ pub type Student as Pupil, Student };\n"
    );
}

#[test]
fn format_base_dimension() {
    let source = "base dim Length;\n";
    let formatted = format_source(source).unwrap();
    assert_eq!(formatted, "base dim Length;\n");
}

#[test]
fn format_unit_decl() {
    let source = "unit EUR: Money = (@rate) USD;\n";
    let formatted = format_source(source).unwrap();
    assert_eq!(formatted, "unit EUR: Money = (@rate) USD;\n");
}

#[test]
fn format_const_unit_decl() {
    let source = "const unit km: Length = 1000 m;\n";
    let formatted = format_source(source).unwrap();
    assert_eq!(formatted, "const unit km: Length = 1000 m;\n");
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

// Issue #575: load-bearing parens around a binary-op operand of unary `!` or
// around the lhs of `^` must survive the formatter.

#[test]
fn format_keeps_parens_around_not_of_and() {
    let source = "param a: Bool = true;\nparam b: Bool = false;\nnode x: Bool = !(@a && @b);\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("!(@a && @b)"),
        "load-bearing parens around `&&` operand of `!` were stripped: {formatted}"
    );
}

#[test]
fn format_keeps_parens_around_not_of_or() {
    let source = "param a: Bool = true;\nparam b: Bool = false;\nnode x: Bool = !(@a || @b);\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("!(@a || @b)"),
        "load-bearing parens around `||` operand of `!` were stripped: {formatted}"
    );
}

#[test]
fn format_keeps_parens_around_neg_in_pow_lhs() {
    let source = "param n: Int = 3;\nnode y: Int = (-@n) ^ 2;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("(-@n) ^ 2"),
        "load-bearing parens around `-` lhs of `^` were stripped: {formatted}"
    );
}

#[test]
fn format_pow_with_signed_literal_rhs_no_parens() {
    // `x ^ -2` is unambiguous because `^` is right-assoc; no parens needed.
    let source = "param x: Dimensionless = 2.0;\nnode y: Dimensionless = @x ^ -2.0;\n";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("@x ^ -2.0"),
        "rhs of `^` should not gain parens around a unary literal: {formatted}"
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
index Maneuver = { Departure, Correction, Insertion };
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
index Maneuver = { Departure, Correction, Insertion };
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
index Phase = { Launch, Cruise };
index Maneuver = { Departure, Correction };
param m: Dimensionless[Phase, Maneuver] = table[Phase, Maneuver] {
    : Departure, Correction;
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
index Maneuver = { Departure, Correction };
param dv: Dimensionless[Maneuver] = {
    Maneuver.Departure: 2.46,
    Maneuver.Correction: 0.12,
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        !formatted.contains("table["),
        "Map literal should not be converted to table: {formatted}"
    );
    assert!(
        formatted.contains("Maneuver.Departure"),
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

snapshot_test!(snapshot_constants, "valid/constants.gcl");
snapshot_test!(snapshot_functions, "invalid/functions.gcl");
snapshot_test!(snapshot_generics, "valid/generics.gcl");
snapshot_test!(snapshot_hohmann, "valid/hohmann.gcl");
snapshot_test!(snapshot_indexed, "valid/indexed.gcl");
snapshot_test!(snapshot_integers, "valid/integers.gcl");
snapshot_test!(snapshot_orbital, "valid/orbital.gcl");
snapshot_test!(snapshot_range_index, "valid/range_index.gcl");
snapshot_test!(snapshot_rocket, "valid/rocket.gcl");
snapshot_test!(snapshot_tagged_union, "valid/tagged_union.gcl");
snapshot_test!(snapshot_tagged_union_param, "valid/tagged_union_param.gcl");
snapshot_test!(snapshot_table_literal, "valid/table_literal.gcl");
snapshot_test!(snapshot_multi_decl_1d, "valid/multi_decl_1d.gcl");
snapshot_test!(snapshot_multi_decl_2d, "valid/multi_decl_2d.gcl");
snapshot_test!(snapshot_multi_decl_sliced, "valid/multi_decl_sliced.gcl");
snapshot_test!(snapshot_time_scan, "valid/time_scan.gcl");
snapshot_test!(snapshot_user_dimensions, "valid/user_dimensions.gcl");
snapshot_test!(snapshot_assertions, "valid/assertions.gcl");
snapshot_test!(
    snapshot_assertions_fail,
    "runtime_error/assertions_fail.gcl"
);
snapshot_test!(
    snapshot_assertions_tolerance_fail,
    "runtime_error/assertions_tolerance_fail.gcl"
);
snapshot_test!(
    snapshot_assertions_assumes,
    "runtime_error/assertions_assumes.gcl"
);
snapshot_test!(
    snapshot_assertions_indexed,
    "runtime_error/assertions_indexed.gcl"
);
snapshot_test!(snapshot_plot_basic, "valid/plot_basic.gcl");
snapshot_test!(snapshot_variant_comparison, "valid/variant_comparison.gcl");
snapshot_test!(snapshot_variant_match, "valid/variant_match.gcl");
snapshot_test!(snapshot_power_budget, "valid/power_budget.gcl");
snapshot_test!(snapshot_thermal_analysis, "valid/thermal_analysis.gcl");
snapshot_test!(
    snapshot_parenthesized_exprs,
    "valid/parenthesized_exprs.gcl"
);
snapshot_test!(snapshot_expected_fail_pass, "valid/expected_fail_pass.gcl");
snapshot_test!(
    snapshot_expected_fail_unexpected_pass,
    "runtime_error/expected_fail_unexpected_pass.gcl"
);
snapshot_test!(
    snapshot_expected_fail_indexed,
    "valid/expected_fail_indexed.gcl"
);
snapshot_test!(
    snapshot_expected_fail_multi_indexed,
    "valid/expected_fail_multi_indexed.gcl"
);
snapshot_test!(
    snapshot_expected_fail_indexed_partial,
    "runtime_error/expected_fail_indexed_partial.gcl"
);
snapshot_test!(
    snapshot_expected_fail_indexed_unexpected_pass,
    "runtime_error/expected_fail_indexed_unexpected_pass.gcl"
);
snapshot_test!(
    snapshot_expected_fail_multi_indexed_partial,
    "runtime_error/expected_fail_multi_indexed_partial.gcl"
);
snapshot_test!(
    snapshot_comments_in_expressions,
    "valid/comments_in_expressions.gcl"
);
snapshot_test!(
    snapshot_required_indexes,
    "valid_library/required_indexes.gcl"
);
snapshot_test!(snapshot_domain_scalar, "valid/domain_scalar.gcl");
snapshot_test!(snapshot_domain_indexed, "valid/domain_indexed.gcl");

// ---------------------------------------------------------------------------
// Multi-file fixtures: import syntax, module imports, qualified references
// ---------------------------------------------------------------------------

// alias: selective import with renaming
snapshot_test!(
    snapshot_multi_alias_main,
    "valid/multi/alias/src/helper/main.gcl"
);
snapshot_test!(
    snapshot_multi_alias_helper,
    "valid/multi/alias/src/helper/lib.gcl"
);

// alias_conflict: multiple imports with renaming
snapshot_test!(
    snapshot_multi_alias_conflict_main,
    "valid/multi/alias_conflict/src/lib/main.gcl"
);

// module_import: whole-module import
idempotency_test!(
    idempotent_multi_module_import_main,
    "invalid/multi/module_import/src/constants/main.gcl"
);
idempotency_test!(
    idempotent_multi_module_import_constants,
    "invalid/multi/module_import/src/constants/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_main,
    "invalid/multi/module_import/src/constants/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_main,
    "invalid/multi/module_import/src/constants/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_constants,
    "invalid/multi/module_import/src/constants/lib.gcl"
);

// module_import_alias: import with alias
snapshot_test!(
    snapshot_multi_module_import_alias_main,
    "valid/multi/module_import_alias/src/constants/main.gcl"
);

// module_import_fn: qualified function call
snapshot_test!(
    snapshot_multi_module_import_fn_main,
    "valid/multi/module_import_fn/src/lib/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_fn_lib,
    "valid/multi/module_import_fn/src/lib/lib.gcl"
);

// cross_file_dag: cross-file DAG paths
idempotency_test!(
    idempotent_multi_cross_file_dag_main,
    "invalid/multi/cross_file_dag/src/lib/main.gcl"
);
idempotency_test!(
    idempotent_multi_cross_file_dag_lib,
    "invalid/multi/cross_file_dag/src/lib/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_cross_file_dag_main,
    "invalid/multi/cross_file_dag/src/lib/main.gcl"
);
snapshot_test!(
    snapshot_multi_cross_file_dag_main,
    "invalid/multi/cross_file_dag/src/lib/main.gcl"
);
snapshot_test!(
    snapshot_multi_cross_file_dag_lib,
    "invalid/multi/cross_file_dag/src/lib/lib.gcl"
);

// bare_dag_ref: bare module path DAG references
snapshot_test!(
    snapshot_multi_bare_dag_ref_main,
    "valid/multi/bare_dag_ref/src/bare_dag_ref/main.gcl"
);
snapshot_test!(
    snapshot_multi_bare_dag_ref_lib,
    "valid/multi/bare_dag_ref/src/bare_dag_ref/lib.gcl"
);

// module_import_graph_ref: qualified @-references
idempotency_test!(
    idempotent_multi_module_import_graph_ref_main,
    "invalid/multi/module_import_graph_ref/src/params/main.gcl"
);
idempotency_test!(
    idempotent_multi_module_import_graph_ref_params,
    "invalid/multi/module_import_graph_ref/src/params/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_graph_ref_main,
    "invalid/multi/module_import_graph_ref/src/params/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_graph_ref_main,
    "invalid/multi/module_import_graph_ref/src/params/main.gcl"
);

// module_import_mixed: selective + module imports in same file
idempotency_test!(
    idempotent_multi_module_import_mixed_main,
    "invalid/multi/module_import_mixed/src/lib/main.gcl"
);
roundtrip_test!(
    roundtrip_multi_module_import_mixed_main,
    "invalid/multi/module_import_mixed/src/lib/main.gcl"
);
snapshot_test!(
    snapshot_multi_module_import_mixed_main,
    "invalid/multi/module_import_mixed/src/lib/main.gcl"
);

// rocket_split: selective import with many identifiers
snapshot_test!(
    snapshot_multi_rocket_split_main,
    "valid/multi/rocket_split/src/lib/main.gcl"
);
snapshot_test!(
    snapshot_multi_rocket_split_constants,
    "valid/multi/rocket_split/src/lib/constants.gcl"
);
snapshot_test!(
    snapshot_multi_rocket_split_params,
    "valid/multi/rocket_split/src/lib/params.gcl"
);

// explicit_index: import of index types
snapshot_test!(
    snapshot_multi_explicit_index_main,
    "valid/multi/explicit_index/src/lib/main.gcl"
);

// diamond_assert: diamond-shaped import graph
snapshot_test!(
    snapshot_multi_diamond_assert_main,
    "valid/multi/diamond_assert/src/graph/main.gcl"
);

// assertions: cross-file assertions with #[assumes]
snapshot_test!(
    snapshot_multi_assertions_main,
    "valid/multi/assertions/src/checks/main.gcl"
);
snapshot_test!(
    snapshot_multi_assertions_checks,
    "valid/multi/assertions/src/checks/lib.gcl"
);

// auto_assert: auto-evaluated assertions from imports
snapshot_test!(
    snapshot_multi_auto_assert_main,
    "valid/multi/auto_assert/src/lib/main.gcl"
);
snapshot_test!(
    snapshot_multi_auto_assert_lib,
    "valid/multi/auto_assert/src/lib/lib.gcl"
);

// auto_assert_module: module import with auto assertions
idempotency_test!(
    idempotent_multi_auto_assert_module_main,
    "invalid/multi/auto_assert_module/src/lib/main.gcl"
);
idempotency_test!(
    idempotent_multi_auto_assert_module_lib,
    "invalid/multi/auto_assert_module/src/lib/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_auto_assert_module_main,
    "invalid/multi/auto_assert_module/src/lib/main.gcl"
);
snapshot_test!(
    snapshot_multi_auto_assert_module_main,
    "invalid/multi/auto_assert_module/src/lib/main.gcl"
);

// imported_deps: import with internal graph dependencies
snapshot_test!(
    snapshot_multi_imported_deps_main,
    "valid/multi/imported_deps/src/lib/main.gcl"
);

// imported_assert_fail: import with failing assertion
snapshot_test!(
    snapshot_multi_imported_assert_fail_main,
    "runtime_error/multi/imported_assert_fail/src/lib/main.gcl"
);

// bad_name_import, missing_module, circular_imports: error-case import syntax (still parseable)
idempotency_test!(
    idempotent_multi_bad_name_import_main,
    "invalid/multi/bad_name_import/src/bad_name_import/main.gcl"
);
idempotency_test!(
    idempotent_multi_bad_name_import_helper,
    "invalid/multi/bad_name_import/src/bad_name_import/helper.gcl"
);
idempotency_test!(
    idempotent_multi_missing_module_main,
    "invalid/multi/missing_module/src/missing_module/main.gcl"
);
idempotency_test!(
    idempotent_multi_circular_imports_main,
    "invalid/multi/circular_imports/src/circ/main.gcl"
);
idempotency_test!(
    idempotent_multi_circular_imports_circ,
    "invalid/multi/circular_imports/src/circ/lib.gcl"
);
idempotency_test!(
    idempotent_multi_circular_imports_back,
    "invalid/multi/circular_imports/src/circ/back.gcl"
);
roundtrip_test!(
    roundtrip_multi_bad_name_import_main,
    "invalid/multi/bad_name_import/src/bad_name_import/main.gcl"
);
roundtrip_test!(
    roundtrip_multi_bad_name_import_helper,
    "invalid/multi/bad_name_import/src/bad_name_import/helper.gcl"
);
roundtrip_test!(
    roundtrip_multi_missing_module_main,
    "invalid/multi/missing_module/src/missing_module/main.gcl"
);
roundtrip_test!(
    roundtrip_multi_circular_imports_main,
    "invalid/multi/circular_imports/src/circ/main.gcl"
);
roundtrip_test!(
    roundtrip_multi_circular_imports_circ,
    "invalid/multi/circular_imports/src/circ/lib.gcl"
);
roundtrip_test!(
    roundtrip_multi_circular_imports_back,
    "invalid/multi/circular_imports/src/circ/back.gcl"
);
snapshot_test!(
    snapshot_multi_bad_name_import_main,
    "invalid/multi/bad_name_import/src/bad_name_import/main.gcl"
);
snapshot_test!(
    snapshot_multi_bad_name_import_helper,
    "invalid/multi/bad_name_import/src/bad_name_import/helper.gcl"
);
snapshot_test!(
    snapshot_multi_missing_module_main,
    "invalid/multi/missing_module/src/missing_module/main.gcl"
);
snapshot_test!(
    snapshot_multi_circular_imports_main,
    "invalid/multi/circular_imports/src/circ/main.gcl"
);
snapshot_test!(
    snapshot_multi_circular_imports_circ,
    "invalid/multi/circular_imports/src/circ/lib.gcl"
);
snapshot_test!(
    snapshot_multi_circular_imports_back,
    "invalid/multi/circular_imports/src/circ/back.gcl"
);

// ---------------------------------------------------------------------------
// Edge-case fixture: long lines, deep nesting, complex expressions
// ---------------------------------------------------------------------------

idempotency_test!(
    idempotent_format_edge_cases,
    "invalid/format_edge_cases.gcl"
);
roundtrip_test!(roundtrip_format_edge_cases, "invalid/format_edge_cases.gcl");
snapshot_test!(snapshot_format_edge_cases, "invalid/format_edge_cases.gcl");

// ---------------------------------------------------------------------------
// Comment-in-expression preservation tests
// ---------------------------------------------------------------------------

#[test]
fn preserves_trailing_comment_in_1d_table() {
    let source = r"
index Maneuver = { Departure, Correction, Insertion };
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
index Maneuver = { Departure, Correction, Insertion };
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
fn preserves_comment_in_match_arms() {
    let source = r"
index Phase = { Coast, Burn };
node x: Dimensionless[Phase] = for p: Phase {
    match p {
        // coasting
        Phase.Coast => 0.0,
        // burning
        Phase.Burn => 1.0,
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
    let coast_arm = formatted.find("Phase.Coast =>").unwrap();
    assert!(
        coast_comment < coast_arm,
        "Comment should appear before its match arm: {formatted}"
    );
}

#[test]
fn preserves_trailing_comment_in_map_literal() {
    let source = r"
index Maneuver = { Departure, Correction };
param dv: Dimensionless[Maneuver] = {
    Maneuver.Departure: 2.46, // departure
    Maneuver.Correction: 0.12, // correction
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
index Maneuver = { Departure, Correction };
param dv: Dimensionless[Maneuver] = {
    // departure entry
    Maneuver.Departure: 2.46,
    // correction entry
    Maneuver.Correction: 0.12,
};
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// departure entry"),
        "Leading comment on map entry lost: {formatted}"
    );
    let comment_pos = formatted.find("// departure entry").unwrap();
    let entry_pos = formatted.find("Maneuver.Departure").unwrap();
    assert!(
        comment_pos < entry_pos,
        "Leading comment should appear before map entry: {formatted}"
    );
}

#[test]
fn preserves_trailing_comment_on_3d_table_slice_header() {
    let source = r"
index Scenario = { Nominal, Contingency };
index Phase = { Launch, Cruise, Arrival };
index Maneuver = { Departure, Correction, Insertion };
param mass_3d: Dimensionless[Scenario, Phase, Maneuver] = table[Scenario, Phase, Maneuver] {
    [Scenario.Nominal] // nominal scenario
           : Departure, Correction, Insertion;
    Launch:  5000.0,        0.0,       0.0;
    Cruise:     0.0,     4500.0,       0.0;
    Arrival:    0.0,        0.0,    4000.0;

    [Scenario.Contingency] // contingency scenario
           : Departure, Correction, Insertion;
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
                line.contains("[Scenario.Nominal]"),
                "Comment not on same line as slice header: {formatted}"
            );
        }
        if line.contains("// contingency scenario") {
            assert!(
                line.contains("[Scenario.Contingency]"),
                "Comment not on same line as slice header: {formatted}"
            );
        }
    }
}

#[test]
fn long_operator_chain_formats_without_stack_overflow() {
    // Regression: the pretty-printer document for a long operator chain is
    // as deep as the chain, and dropping it recursed in `Rc` drop glue with
    // no stack-growth guard — a few thousand terms aborted the process.
    let source = format!(
        "node x: Dimensionless = {};\n",
        vec!["1.0"; 2_000].join(" + ")
    );
    let formatted = format_source(&source).unwrap();
    assert!(formatted.contains("1.0 + 1.0"));
}

#[test]
fn multi_decl_with_internal_comments_is_preserved_verbatim() {
    // Regression: comments inside a multi-decl body were consumed without
    // being emitted — `graphcal format` permanently destroyed user content.
    // Declarations whose internal comments the formatter cannot anchor are
    // now emitted verbatim instead.
    let source = "\
pub index Component = { A, B };

param      power: Power[Component],
param      duty:  Dimensionless[Component]
    = table[Component, (_, _)] {
         : _, _;
        // explains row A
        A: 10.0 W, 0.5;
        B: 20.0 W, 0.9; // explains row B
    };
";
    let formatted = format_source(source).unwrap();
    assert!(
        formatted.contains("// explains row A") && formatted.contains("// explains row B"),
        "comments inside multi-decl bodies must survive formatting:\n{formatted}"
    );
}

#[test]
fn comment_inside_if_branch_does_not_migrate() {
    // Regression: comments in undrained expression positions (e.g. inside
    // an `if` branch) stayed queued and were emitted as the *next*
    // declaration's leading comment, relocating them out of context.
    let source = "\
node x: Dimensionless = if 1.0 > 0.0 {
    // chosen when positive
    1.0
} else {
    2.0
};
node y: Dimensionless = 3.0;
";
    let formatted = format_source(source).unwrap();
    let comment_pos = formatted.find("// chosen when positive").unwrap();
    let y_pos = formatted.find("node y").unwrap();
    assert!(
        comment_pos < y_pos,
        "comment must stay with `node x`, not migrate below:\n{formatted}"
    );
    let x_pos = formatted.find("node x").unwrap();
    assert!(comment_pos > x_pos);
}

#[test]
fn comment_count_is_preserved_across_formatting() {
    let source = "\
// file header
node a: Dimensionless = 1.0; // trailing
node b: Dimensionless = if 1.0 > 0.0 {
    // branch comment
    1.0
} else { 2.0 };
// footer
";
    let formatted = format_source(source).unwrap();
    let count = |s: &str| s.matches("//").count();
    assert_eq!(
        count(source),
        count(&formatted),
        "comment count in == out:\n{formatted}"
    );
}

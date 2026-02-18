#![allow(clippy::unwrap_used, reason = "test code")]

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
idempotency_test!(idempotent_time_scan, "time_scan.gcl");
idempotency_test!(idempotent_user_dimensions, "user_dimensions.gcl");

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
roundtrip_test!(roundtrip_time_scan, "time_scan.gcl");
roundtrip_test!(roundtrip_user_dimensions, "user_dimensions.gcl");

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

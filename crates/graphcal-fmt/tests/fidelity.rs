//! Formatter fidelity regressions: formatting may change layout only, never
//! which declaration a comment documents or where a comment sits.
#![cfg(test)]

use graphcal_fmt::format_source;

/// Format `source`, check the output is a fixpoint, and return it.
fn format_stable(source: &str) -> String {
    let formatted = format_source(source)
        .unwrap_or_else(|error| panic!("failed to format {source:?}: {error}"));
    assert_eq!(
        format_source(&formatted).unwrap(),
        formatted,
        "formatting {source:?} is not idempotent"
    );
    formatted
}

// ---------------------------------------------------------------------------
// FE-4: `///` blocks keep documenting the same declaration.
// ---------------------------------------------------------------------------

#[test]
fn detached_doc_block_stays_detached_from_the_next_doc_block() {
    let source = "param a: Dimensionless = 1.0;\n\n\
                  /// Section: derived quantities (not documentation of `b`).\n\n\
                  /// Doubled input.\n\
                  param b: Dimensionless = 2.0;\n";
    assert_eq!(format_stable(source), source);
}

#[test]
fn detached_doc_block_stays_detached_from_the_declaration() {
    let source =
        "param a: Dimensionless = 1.0;\n\n/// Section note.\n\nparam b: Dimensionless = 2.0;\n";
    assert_eq!(format_stable(source), source);
}

#[test]
fn file_header_doc_block_stays_detached_from_the_first_declaration() {
    let source = "/// File header.\n\nparam a: Dimensionless = 1.0;\n";
    assert_eq!(format_stable(source), source);
}

#[test]
fn detached_doc_block_in_dag_body_stays_detached() {
    let source =
        "dag d {\n    param a: Dimensionless;\n\n    /// note\n\n    param b: Dimensionless;\n}\n";
    assert_eq!(format_stable(source), source);
}

#[test]
fn detached_doc_block_between_multi_decl_slots_stays_detached() {
    let source = "index I = { X, Y };\n\n\
                  /// doc a\n\
                  param a: Dimensionless[I],\n\n\
                  /// detached note\n\n\
                  /// doc b\n\
                  param b: Dimensionless[I]\n    = table[I, (_, _)] {\n         :   _,   _;\n        X: 1.0, 2.0;\n        Y: 3.0, 4.0;\n    };\n";
    let formatted = format_stable(source);
    assert!(
        formatted.contains("/// detached note\n\n/// doc b\nparam b"),
        "detached note merged into the doc of `b`: {formatted}"
    );
}

#[test]
fn own_line_doc_comment_after_lone_cr_documents_the_next_declaration() {
    let source = "node a: Dimensionless = 1.0;\r/// doc for b\rnode b: Dimensionless = 2.0;\r";
    assert_eq!(
        format_stable(source),
        "node a: Dimensionless = 1.0;\n\n/// doc for b\nnode b: Dimensionless = 2.0;\n"
    );
}

// ---------------------------------------------------------------------------
// FE-5: an end-of-line comment after a construct stays after it.
// ---------------------------------------------------------------------------

#[test]
fn comment_after_call_stays_after_the_declaration() {
    let source = "param gm: Dimensionless = 2.0;\n\
                  param r: Dimensionless = 1.0;\n\
                  node v: Dimensionless = sqrt(@gm / @r); // circular orbit speed\n\
                  node w: Dimensionless = @v * 2.0; // doubled\n";
    assert_eq!(format_stable(source), source);
}

#[test]
fn comment_after_map_literal_stays_after_the_declaration() {
    let source = "pub index I = { A, B };\n\
                  param v: Dimensionless[I] = { I#A: 1.0, I#B: 2.0 }; // calibration from 2024 test\n";
    assert_eq!(
        format_stable(source),
        "pub index I = { A, B };\n\
         param v: Dimensionless[I] = {\n    I#A: 1.0,\n    I#B: 2.0,\n}; // calibration from 2024 test\n"
    );
}

#[test]
fn comment_after_match_stays_after_the_declaration() {
    let source = "index I = { A, B };\n\
                  param k: Key<I> = I#A;\n\
                  node x: Dimensionless = match @k { I#A => 1.0, I#B => 2.0 }; // pick\n";
    let formatted = format_stable(source);
    assert!(
        formatted.contains("    I#B => 2.0,\n}; // pick\n"),
        "comment moved into the match: {formatted}"
    );
}

#[test]
fn comment_after_dag_body_stays_after_the_dag() {
    let source = "dag d { param a: Dimensionless; } // the dag\n";
    assert_eq!(
        format_stable(source),
        "dag d {\n    param a: Dimensionless;\n} // the dag\n"
    );
}

#[test]
fn comment_inside_index_brackets_stays_inside() {
    let source = "index I = { A, B };\n\
                  param v: Dimensionless[I] = {\n    I#A: 1.0,\n    I#B: 2.0,\n};\n\
                  node x: Dimensionless = @v[ // which\n I#A] + 1.0;\n";
    assert_eq!(format_stable(source), source);
}

// ---------------------------------------------------------------------------
// FE-8: a bare built-in type former keeps its spelling.
// ---------------------------------------------------------------------------

#[test]
fn bare_complex_and_key_keep_their_spelling() {
    for source in [
        "node z: Complex = 1.0;\n",
        "param k: Key;\n",
        "param z: Complex<Length>;\n",
        "index I = { A };\nparam k: Key<I>;\n",
    ] {
        assert_eq!(format_stable(source), source);
    }
}

#[test]
fn bare_complex_fixture_formats() {
    let source = include_str!("../../../tests/fixtures/invalid/complex_requires_dimension_arg.gcl");
    assert!(format_stable(source).contains(": Complex "));
}

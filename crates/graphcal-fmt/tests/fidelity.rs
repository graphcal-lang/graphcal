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

//! Phase 1 regression tests from `.local/2026-08-05_code-review-chunk-3.md`.
#![cfg(test)]

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{CompileError, compile_and_eval};

fn compile_graphcal_error(source: &str) -> GraphcalError {
    match compile_and_eval(source).unwrap_err() {
        CompileError::Eval(error) => error,
        other => panic!("expected semantic error, got {other:?}"),
    }
}

#[test]
fn sink_graph_references_are_lowered_strictly() {
    let sources = [
        r#"
plot p = {
    mark: line,
    encode: { x: @missing },
};
"#,
        r#"
plot p = {
    mark: line { stroke_width: @missing },
    encode: { x: 1.0 },
};
"#,
        r#"
plot p = {
    mark: line,
    encode: { x: 1.0 },
    width: @missing,
};
"#,
        r#"
plot p = { mark: line, encode: { x: 1.0 } };
figure f = { plots: [p], title: @missing };
"#,
        r#"
plot p = { mark: line, encode: { x: 1.0 } };
layer l = { plots: [p], title: @missing };
"#,
    ];

    for source in sources {
        assert!(
            matches!(compile_graphcal_error(source), GraphcalError::UnknownGraphRef { .. }),
            "sink accepted an unresolved graph reference:\n{source}"
        );
    }
}

#[test]
fn sink_constructor_function_and_unit_names_are_lowered_strictly() {
    for source in [
        "plot p = { mark: line, encode: { x: Missing(value: 1.0) } };",
        "plot p = { mark: line, encode: { x: missing(1.0) } };",
        "plot p = { mark: line, encode: { x: 1.0 missing_unit } };",
    ] {
        assert!(
            compile_and_eval(source).is_err(),
            "sink accepted an unresolved semantic name: {source}"
        );
    }
}

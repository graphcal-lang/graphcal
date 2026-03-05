//! Property-based tests: declaration order must not affect evaluation results.
//!
//! Graphcal's reactive evaluation model builds a dependency DAG and
//! topologically sorts it, so the source order of top-level declarations
//! should never influence the computed values.
//!
//! These tests randomly shuffle declarations and verify that the evaluation
//! results remain identical.
//!
//! See: <https://github.com/shunichironomura/graphcal/issues/247>
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]

use graphcal_eval::eval::{EvalResult, compile_and_eval};
use graphcal_syntax::parser::Parser;
use proptest::prelude::*;
use rand::SeedableRng;
use rand::seq::SliceRandom;

// ============================================================================
// Helpers
// ============================================================================

/// Parse the source, shuffle the top-level declarations using a seeded RNG,
/// and reassemble the source text.
///
/// Each `Declaration` carries a `span` (byte offset + length) that covers the
/// full text from keyword/attribute to closing semicolon/brace. We extract
/// each declaration's text slice, shuffle those slices, and join them with
/// blank lines so the parser can re-parse the shuffled source.
fn shuffle_source(source: &str, seed: u64) -> String {
    let mut parser = Parser::new(source);
    let file = parser.parse_file().expect("fixture must parse");

    let mut slices: Vec<&str> = file
        .declarations
        .iter()
        .map(|decl| {
            let start = decl.span.offset();
            let mut end = start + decl.span.len();
            // Some declaration spans (e.g., `range`) exclude the trailing
            // semicolon. Extend the slice to include it when present.
            if end < source.len() && source.as_bytes()[end] == b';' {
                end += 1;
            }
            &source[start..end]
        })
        .collect();

    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    slices.shuffle(&mut rng);

    slices.join("\n\n")
}

/// Assert that two `EvalResult`s are equivalent by comparing every declaration
/// by name. The source order of `consts`, `params`, and `nodes` may differ, so
/// we look up each name in both results and compare values with `PartialEq`.
fn assert_results_equal(original: &EvalResult, shuffled: &EvalResult) {
    // Consts
    assert_eq!(
        original.consts.len(),
        shuffled.consts.len(),
        "const count mismatch"
    );
    for (name, val) in &original.consts {
        let shuffled_val = &shuffled
            .consts
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("const `{name}` missing in shuffled result"))
            .1;
        assert!(
            val == shuffled_val,
            "const `{name}` differs: {val:?} vs {shuffled_val:?}"
        );
    }

    // Params
    assert_eq!(
        original.params.len(),
        shuffled.params.len(),
        "param count mismatch"
    );
    for (name, res) in &original.params {
        let shuffled_res = &shuffled
            .params
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("param `{name}` missing in shuffled result"))
            .1;
        assert!(
            res == shuffled_res,
            "param `{name}` differs: {res:?} vs {shuffled_res:?}"
        );
    }

    // Nodes
    assert_eq!(
        original.nodes.len(),
        shuffled.nodes.len(),
        "node count mismatch"
    );
    for (name, res) in &original.nodes {
        let shuffled_res = &shuffled
            .nodes
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("node `{name}` missing in shuffled result"))
            .1;
        assert!(
            res == shuffled_res,
            "node `{name}` differs: {res:?} vs {shuffled_res:?}"
        );
    }
}

// ============================================================================
// Property-based tests
// ============================================================================

proptest! {
    #[test]
    fn rocket_order_independent(seed in 0u64..10000) {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let shuffled = shuffle_source(source, seed);
        let original_result = compile_and_eval(source)
            .expect("original source must evaluate");
        let shuffled_result = compile_and_eval(&shuffled)
            .unwrap_or_else(|e| panic!("shuffled source (seed={seed}) failed to evaluate: {e}"));
        assert_results_equal(&original_result, &shuffled_result);
    }

    #[test]
    fn indexed_order_independent(seed in 0u64..10000) {
        let source = include_str!("../../../tests/fixtures/indexed.gcl");
        let shuffled = shuffle_source(source, seed);
        let original_result = compile_and_eval(source)
            .expect("original source must evaluate");
        let shuffled_result = compile_and_eval(&shuffled)
            .unwrap_or_else(|e| panic!("shuffled source (seed={seed}) failed to evaluate: {e}"));
        assert_results_equal(&original_result, &shuffled_result);
    }

    #[test]
    fn range_index_order_independent(seed in 0u64..10000) {
        let source = include_str!("../../../tests/fixtures/range_index.gcl");
        let shuffled = shuffle_source(source, seed);
        let original_result = compile_and_eval(source)
            .expect("original source must evaluate");
        let shuffled_result = compile_and_eval(&shuffled)
            .unwrap_or_else(|e| panic!("shuffled source (seed={seed}) failed to evaluate: {e}"));
        assert_results_equal(&original_result, &shuffled_result);
    }
}

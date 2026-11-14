//! Regression tests for diagnostic wording, codes and spans (lane L27, #2268).
//!
//! Each test renders the diagnostic a small program reports, so the snapshot
//! pins the code, the message, the span and the help text the user sees.
#![allow(
    clippy::unwrap_used,
    reason = "a program expected to fail must fail; panicking is the clearest test failure"
)]

use graphcal_project::prepare::compile_and_eval_named;
use miette::{Diagnostic, NarratableReportHandler};

/// Compile `source` and render its error deterministically.
fn render_error(source: &str, name: &str) -> String {
    let err = compile_and_eval_named(source, name).unwrap_err();
    let diagnostic: &dyn Diagnostic = &err;
    let mut buf = String::new();
    NarratableReportHandler::new()
        .render_report(&mut buf, diagnostic)
        .unwrap();
    buf
}

/// MDL-19: an input port and an explicit export of the same name are a
/// duplicate declaration (N001), not an internal assertion failure.
#[test]
fn param_and_pub_node_of_one_name_report_duplicate() {
    let source = "param x: Length = 1.0 m;\npub node x: Length = @x * 2.0;\n";
    insta::assert_snapshot!(render_error(source, "collide.gcl"));
}

/// RES-13: two includes with one alias are a duplicate name reported at the
/// second alias, not a duplicate internal module over the whole file.
#[test]
fn duplicate_include_alias_is_reported_at_the_alias() {
    let source = "dag d { pub node y: Length = 1.0 m; }\ninclude d() as x;\ninclude d() as x;\nnode z: Length = @x::y;\n";
    insta::assert_snapshot!(render_error(source, "dupinc.gcl"));
}

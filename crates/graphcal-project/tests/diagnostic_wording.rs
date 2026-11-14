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

/// RES-17: a generic parameter that shadows a private declaration is the
/// shadowing error (N027), not a visibility error whose "add `pub`" advice
/// cannot work.
#[test]
fn generic_parameter_shadowing_a_private_static_reports_shadowing() {
    let source = "type T { T(x: Length) }\npub type Box<T: Type> { Box(value: T) }\nnode b: Box<Dimensionless> = Box<Dimensionless>(value: 1.0);\n";
    insta::assert_snapshot!(render_error(source, "shadow.gcl"));
}

/// TYC-10: an exponent overflow found while specializing an include is
/// reported at the include (or the selected dimension), not over the file.
#[test]
fn instance_dimension_overflow_is_reported_at_the_include() {
    let projected_value = "dim Vol = Length^3;\ndag lib {\n    pub(bind) dim D;\n    param x: D;\n    pub node y: D^1000000000 = @x ^ 1000000000;\n}\ninclude lib(dim D: Vol, x: 1.0 m^3)::{ y };\n";
    insta::assert_snapshot!(
        "instance_dimension_overflow_value",
        render_error(projected_value, "ovf_value.gcl")
    );
    let projected_dimension = "dim Vol = Length^3;\ndag lib {\n    pub(bind) dim D;\n    pub dim Q = D^1000000000;\n    param x: D;\n}\ninclude lib(dim D: Vol, x: 1.0 m^3)::{ dim Q };\nnode n: Q = todo;\n";
    insta::assert_snapshot!(
        "instance_dimension_overflow_dimension",
        render_error(projected_dimension, "ovf_dimension.gcl")
    );
}

/// TYC-15: map entries of different types are a map-literal rule, not an
/// annotation mismatch against a type nobody declared.
#[test]
fn map_entries_of_different_types_report_the_map_rule() {
    let source = "index P = { A, B };\nnode x: Length[P] = { P#A: 1.0 m, P#B: 2.0 s };\n";
    insta::assert_snapshot!(render_error(source, "map_entries.gcl"));
}

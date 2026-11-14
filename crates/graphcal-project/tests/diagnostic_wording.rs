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

/// FE-15: parse diagnostics name what was actually found, where it was found.
#[test]
fn parse_diagnostics_report_what_was_found() {
    for (name, source) in [
        ("unit_missing_eq", "unit kfoot: Length 304.8 m;\n"),
        (
            "exponent_out_of_range",
            "node x: Length = 1.0 m^99999999999;\n",
        ),
        ("misplaced_pub", "pub param x: Length = 1.0 m;\n"),
    ] {
        insta::assert_snapshot!(
            format!("parse_diagnostic_{name}"),
            render_error(source, &format!("{name}.gcl"))
        );
    }
}

/// TIR-13: diagnostics spell names as written and quote each name once,
/// never showing a local slot or an owner-qualified internal identity.
#[test]
fn diagnostics_do_not_show_internal_representations() {
    for (name, source) in [
        (
            "invalid_index_variable",
            "type Wrap {\n    Wrap(n: Int),\n}\n\nparam v: Dimensionless[Fin(4)] = for i: Fin(4) { 1.0 };\nnode w: Wrap = Wrap(n: 2);\nnode bad: Dimensionless = match @w {\n    Wrap(n: k) => @v[k],\n};\n",
        ),
        (
            "plot_channel_axes",
            "pub index Mode = { Normal, Boost };\npub index Phase = { Launch, Cruise };\nparam a: Dimensionless[Mode] = { Mode#Normal: 1.0, Mode#Boost: 2.0 };\nparam b: Dimensionless[Phase] = { Phase#Launch: 1.0, Phase#Cruise: 2.0 };\nplot p = {\n    mark: point,\n    encode: {\n        x: @a,\n        y: @b,\n    },\n};\n",
        ),
        (
            "domain_on_struct",
            "type P {\n    P(x: Length),\n}\n\nnode p: P(min: 1.0 m) = P(x: 2.0 m);\n",
        ),
        (
            "field_of_union",
            "type M {\n    A(x: Length),\n    B,\n}\nnode m: M = B;\nnode y: Length = @m.x;\n",
        ),
        (
            "positional_constructor",
            "type P {\n    P(x: Length),\n}\nnode p: P = P(2.0 m);\n",
        ),
        (
            "non_exhaustive_map",
            "index Mode = { Normal, Boost };\nindex Phase = { Launch, Cruise };\nnode x: Dimensionless[Mode, Phase] = {\n    (Mode#Normal, Phase#Launch): 1.0,\n    (Mode#Normal, Phase#Cruise): 1.0,\n    (Mode#Boost, Phase#Launch): 1.0,\n};\n",
        ),
        (
            "unreconciled_override",
            "pub index Other = { X, Y };\n\ndag lib {\n    pub(bind) index I = { A, B };\n    param w: Dimensionless[I] = { I#A: 1.0, I#B: 2.0 };\n    pub node out: Dimensionless = sum(@w);\n}\n\ninclude lib(index I: Other) as l;\nnode r: Dimensionless = @l::out;\n",
        ),
    ] {
        insta::assert_snapshot!(
            format!("internal_representation_{name}"),
            render_error(source, &format!("{name}.gcl"))
        );
    }
}

/// TIR-14: help texts describe the language as it is.
#[test]
fn help_texts_match_the_language() {
    for (name, source) in [
        (
            "unknown_local",
            "param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };\nnode xs: Dimensionless[Fin(3)] = for i: Fin(3) { @v[j] };\n",
        ),
        ("builtin_function_name", "node sum: Length = 1.0 m;\n"),
        ("time_scale_name", "pub index UTC = { A, B };\n"),
    ] {
        insta::assert_snapshot!(
            format!("help_text_{name}"),
            render_error(source, &format!("{name}.gcl"))
        );
    }
    // N001's help says names are unique within a file or `dag` body: an
    // inline DAG may reuse a name of its enclosing file.
    let inline_reuse = "node x: Length = 1.0 m;\ndag d {\n    pub node x: Length = 2.0 m;\n}\nnode y: Length = @d()::x;\n";
    assert!(compile_and_eval_named(inline_reuse, "inline_reuse.gcl").is_ok());
}

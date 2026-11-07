//! Regression tests for selective includes that rebind Static ports and
//! project template types (#2243).
#![cfg(test)]

use std::collections::HashMap;

use graphcal_compiler::semantic_error::visibility::VisibilityError;
use graphcal_compiler::semantic_error::{SemanticError, SemanticErrorKind};
use graphcal_compiler::static_interface::StaticInputKind;
use graphcal_eval::eval::EvalResult;
use graphcal_io::RealFileSystem;
use graphcal_project::compile_error::CompileError;
use graphcal_project::prepare::compile_and_eval_project;

/// Write a package `p` whose files are given relative to its source
/// directory, returning the path of `main.gcl`.
fn write_project(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src").join("p");
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"p\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(&package_dir).unwrap();
    for (name, source) in files {
        std::fs::write(package_dir.join(name), source).unwrap();
    }
    (dir, package_dir.join("main.gcl"))
}

fn eval(root: &std::path::Path) -> EvalResult {
    compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|error| panic!("project must evaluate: {error:?}"))
}

fn semantic_error(root: &std::path::Path) -> SemanticError {
    match compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default()) {
        Err(CompileError::Eval(error)) => error.error,
        Err(other) => panic!("expected a semantic error, got {other:?}"),
        Ok(_) => panic!("project must be rejected"),
    }
}

fn entry<'a>(result: &'a EvalResult, name: &str) -> &'a graphcal_eval::eval::EvalEntry {
    result
        .entries
        .iter()
        .find(|entry| entry.name.to_string() == name)
        .unwrap_or_else(|| panic!("declaration `{name}` not found"))
}

fn si_value(result: &EvalResult, name: &str) -> f64 {
    entry(result, name)
        .outcome
        .as_ref()
        .unwrap_or_else(|error| panic!("declaration `{name}` has error: {error}"))
        .si_value()
        .unwrap()
        .get()
}

/// The selected item, and each missing type with its port, of a V008.
fn missing_rebound_types(
    error: &SemanticError,
) -> (String, Vec<(String, StaticInputKind, String)>) {
    match error {
        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
            kind:
                SemanticErrorKind::Visibility(VisibilityError::IncludeMustSelectReboundType {
                    item,
                    types,
                }),
            ..
        }) => (
            item.to_string(),
            types
                .iter()
                .map(|dependency| {
                    (
                        dependency.nominal.to_string(),
                        dependency.port_kind,
                        dependency.port.to_string(),
                    )
                })
                .collect(),
        ),
        other => panic!("expected V008, got {other:?}"),
    }
}

const INDEXED_LIBRARY: &str = "pub(bind) index I = { a, b };\n\
     pub type T { T(x: Length[I]) }\n\
     param v: Length[I] = { I#a: 1.0 m, I#b: 2.0 m };\n\
     pub node out: T = T(x: @v);\n";

/// RES-1: an output whose type mentions a rebound index used to keep the
/// template's type, so `@out.x` checked as `Length[I]` (two entries) while
/// holding the three entries of `J`, and `count`/`sum` silently used them.
#[test]
fn output_whose_type_depends_on_a_rebound_index_requires_the_type() {
    let (_dir, root) = write_project(&[
        ("lib.gcl", INDEXED_LIBRARY),
        (
            "main.gcl",
            "import p.lib::{ type T as LT, index I as LI };\n\
             pub index J = { c, d, e };\n\
             include p.lib(index I: J, v: { J#c: 1.0 m, J#d: 2.0 m, J#e: 3.0 m })::{ out };\n\
             node y: Length[LI] = @out.x;\n\
             node n: Int = count(@y);\n",
        ),
    ]);
    assert_eq!(
        missing_rebound_types(&semantic_error(&root)),
        (
            "out".to_owned(),
            vec![("T".to_owned(), StaticInputKind::Index, "I".to_owned())]
        )
    );
}

/// Selecting the type projects its specialization, so the output follows
/// the rebound index.
#[test]
fn selected_type_follows_the_rebound_index() {
    let (_dir, root) = write_project(&[
        ("lib.gcl", INDEXED_LIBRARY),
        (
            "main.gcl",
            "pub index J = { c, d, e };\n\
             include p.lib(index I: J, v: { J#c: 1.0 m, J#d: 2.0 m, J#e: 3.0 m })::{ type T, out };\n\
             node y: Length[J] = @out.x;\n\
             node total: Length = sum(@y);\n",
        ),
    ]);
    let result = eval(&root);
    assert!((si_value(&result, "total") - 6.0).abs() < 1e-12);
}

/// An include that rebinds no Static port keeps every template type.
#[test]
fn include_without_static_bindings_needs_no_type_selection() {
    let (_dir, root) = write_project(&[
        ("lib.gcl", INDEXED_LIBRARY),
        (
            "main.gcl",
            "import p.lib::{ index I as LI };\n\
             include p.lib(v: { LI#a: 3.0 m, LI#b: 4.0 m })::{ out };\n\
             node total: Length = sum(@out.x);\n",
        ),
    ]);
    let result = eval(&root);
    assert!((si_value(&result, "total") - 7.0).abs() < 1e-12);
}

const NESTED_LIBRARY: &str = "pub(bind) index I = { a, b };\n\
     pub type T { T(x: Length[I]) }\n\
     pub type U { U(t: T) }\n\
     param v: Length[I] = { I#a: 1.0 m, I#b: 2.0 m };\n\
     pub node out: U = U(t: T(x: @v));\n";

/// Every type the output reaches that depends on the port is listed, also
/// through a selected type: projecting `U` alone kept its field's `T`.
#[test]
fn every_reached_type_depending_on_a_rebound_port_is_listed() {
    for (selection, expected) in [
        ("out", vec!["U", "T"]),
        ("type U, out", vec!["T"]),
        ("type T, out", vec!["U"]),
    ] {
        let (_dir, root) = write_project(&[
            ("lib.gcl", NESTED_LIBRARY),
            (
                "main.gcl",
                &format!(
                    "pub index J = {{ c, d, e }};\n\
                     include p.lib(index I: J, v: {{ J#c: 1.0 m, J#d: 2.0 m, J#e: 3.0 m }})::{{ {selection} }};\n"
                ),
            ),
        ]);
        let (_, types) = missing_rebound_types(&semantic_error(&root));
        assert_eq!(
            types
                .iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>(),
            expected,
            "selection `{selection}`"
        );
    }

    let (_dir, root) = write_project(&[
        ("lib.gcl", NESTED_LIBRARY),
        (
            "main.gcl",
            "pub index J = { c, d, e };\n\
             include p.lib(index I: J, v: { J#c: 1.0 m, J#d: 2.0 m, J#e: 3.0 m })::{ type U, type T, out };\n\
             node n: Int = count(@out.t.x);\n",
        ),
    ]);
    let result = eval(&root);
    assert!(matches!(
        entry(&result, "n").outcome,
        Ok(graphcal_eval::eval::Value::Int(3))
    ));
}

const DIMENSION_LIBRARY: &str = "pub(bind) dim Q;\n\
     pub type R { R(v: Q) }\n\
     param x: Q;\n\
     pub node out: R = R(v: @x);\n";

/// A rebound dimension port is covered like an index port.
#[test]
fn output_whose_type_depends_on_a_rebound_dimension_requires_the_type() {
    let (_dir, root) = write_project(&[
        ("lib.gcl", DIMENSION_LIBRARY),
        (
            "main.gcl",
            "include p.lib(dim Q: Mass, x: 5.0 kg)::{ out };\n\
             node m: Mass = @out.v;\n",
        ),
    ]);
    assert_eq!(
        missing_rebound_types(&semantic_error(&root)),
        (
            "out".to_owned(),
            vec![("R".to_owned(), StaticInputKind::Dimension, "Q".to_owned())]
        )
    );

    let (_dir, root) = write_project(&[
        ("lib.gcl", DIMENSION_LIBRARY),
        (
            "main.gcl",
            "include p.lib(dim Q: Mass, x: 5.0 kg)::{ out, type R };\n\
             node m: Mass = @out.v;\n",
        ),
    ]);
    let result = eval(&root);
    assert!((si_value(&result, "m") - 5.0).abs() < 1e-12);
}

const ALIASED_LIBRARY: &str = "pub(bind) index I = { a, b };\n\
     pub type T {\n\
         A(x: Length),\n\
     }\n\
     pub node out: Length = 1.0 m;\n";

/// RES-4: a type projected under Static bindings used to be owned by
/// `(importer, template leaf)`, so projecting it beside the importer's own
/// `T` or `A`, or projecting it twice, was an internal error (X001).
#[test]
fn projected_type_coexists_with_the_importers_own_same_named_type() {
    let (_dir, root) = write_project(&[
        ("lib.gcl", ALIASED_LIBRARY),
        (
            "main.gcl",
            "index J = { a, b };\n\
             type T {\n\
                 A(y: Mass),\n\
             }\n\
             include p.lib(index I: J)::{ type T as U, A as B, out };\n\
             node n: T = A(y: 1.0 kg);\n\
             node m: Mass = match @n { A(y: v) => v };\n\
             node k: U = B(x: 2.0 m);\n\
             node kx: Length = match @k { B(x: v) => v };\n",
        ),
    ]);
    let result = eval(&root);
    assert!((si_value(&result, "m") - 1.0).abs() < 1e-12);
    assert!((si_value(&result, "kx") - 2.0).abs() < 1e-12);
}

/// RES-4: two includes projecting the same template type get two distinct
/// types.
#[test]
fn two_projections_of_one_template_type_are_distinct_types() {
    let includes = "index J = { a, b };\n\
         index K = { c, d, e };\n\
         include p.lib(index I: J)::{ type T as U1, A as A1 };\n\
         include p.lib(index I: K)::{ type T as U2, A as A2 };\n";
    let (_dir, root) = write_project(&[
        ("lib.gcl", ALIASED_LIBRARY),
        (
            "main.gcl",
            &format!(
                "{includes}\
                 node n1: U1 = A1(x: 1.0 m);\n\
                 node n2: U2 = A2(x: 2.0 m);\n\
                 node x2: Length = match @n2 {{ A2(x: v) => v }};\n"
            ),
        ),
    ]);
    let result = eval(&root);
    assert!((si_value(&result, "x2") - 2.0).abs() < 1e-12);

    let (_dir, root) = write_project(&[
        ("lib.gcl", ALIASED_LIBRARY),
        (
            "main.gcl",
            &format!("{includes}node n1: U1 = A2(x: 1.0 m);\n"),
        ),
    ]);
    let error = semantic_error(&root);
    assert!(
        matches!(
            &error,
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Dimension(_),
                ..
            })
        ),
        "a value of one projection must not check as the other: {error:?}"
    );
}

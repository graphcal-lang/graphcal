//! Regression tests for items a dependency re-exports from its own includes,
//! and for the namespaces of selective-include items (#2259).
#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use graphcal_eval::eval::{AssertResult, EvalOutputView, EvalResult};
use graphcal_io::RealFileSystem;
use graphcal_project::compile_error::CompileError;
use graphcal_project::prepare::compile_and_eval_project;
use miette::Diagnostic;

/// A package `p` whose files are written below `src/p/`, returning the
/// directory and the path of `main.gcl`.
fn package(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"p\"\n",
    )
    .unwrap();
    let package_dir = dir.path().join("src").join("p");
    std::fs::create_dir_all(&package_dir).unwrap();
    for (name, source) in files {
        std::fs::write(package_dir.join(name), source).unwrap();
    }
    (dir, package_dir.join("main.gcl"))
}

fn eval(root: &Path) -> EvalResult {
    compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|error| panic!("project must evaluate: {error:?}"))
}

fn eval_error(root: &Path) -> CompileError {
    match compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default()) {
        Err(error) => error,
        Ok(_) => panic!("project must be rejected"),
    }
}

fn code(error: &CompileError) -> Option<String> {
    error.code().map(|code| code.to_string())
}

fn si_value(result: &EvalResult, name: &str) -> f64 {
    result
        .entries
        .iter()
        .find(|entry| entry.name.to_string() == name)
        .unwrap_or_else(|| panic!("declaration `{name}` not found"))
        .outcome
        .as_ref()
        .unwrap_or_else(|error| panic!("declaration `{name}` has error: {error}"))
        .si_value()
        .unwrap()
        .get()
}

const STATIC_LIB: &str = "\
pub dim Foo = Mass;
pub type W { W(x: Length) }
pub index Q = { a, b };
pub const unit ft: Length = 0.3048 m;
pub node out: Length = 1.0 m;
";

const STATIC_MID: &str =
    "include p.lib()::{ pub dim Foo, pub type W, pub W, pub index Q, pub unit ft, pub out };\n";

#[test]
fn static_items_reexported_by_an_include_are_selectable_in_their_namespace() {
    let (_dir, main) = package(&[
        ("lib.gcl", STATIC_LIB),
        ("mid.gcl", STATIC_MID),
        (
            "main.gcl",
            "include p.mid()::{ dim Foo, type W, W, index Q, unit ft, out };\n\
             node mass: Foo = 2.0 kg;\n\
             node w: W = W(x: 1.0 m);\n\
             node by_q: Length[Q] = { Q#a: 1.0 m, Q#b: 2.0 m };\n\
             node feet: Length = 1.0 ft;\n\
             node o: Length = @out;\n",
        ),
    ]);
    let result = eval(&main);
    assert_eq!(si_value(&result, "mass"), 2.0);
    assert!((si_value(&result, "feet") - 0.3048).abs() < 1e-12);
    assert_eq!(si_value(&result, "o"), 1.0);
}

#[test]
fn static_item_reexported_by_an_include_is_not_a_term() {
    let (_dir, main) = package(&[
        ("lib.gcl", STATIC_LIB),
        ("mid.gcl", STATIC_MID),
        ("main.gcl", "include p.mid()::{ Foo };\n"),
    ]);
    let error = eval_error(&main);
    assert_eq!(code(&error).as_deref(), Some("graphcal::M022"));
    // The diagnostic names the module the source names, never the include's
    // internal instance.
    let message = error.to_string();
    assert!(message.contains("src.p.mid"), "{message}");
    assert!(!message.contains("<include@"), "{message}");
}

/// A leaf with a plot, an assertion, and a value, and modules that re-export
/// them through selective includes: a file root, an inline DAG, and a file
/// whose own inline DAG declares the plot (PRJ-3).
const FACADE_PACKAGE: &[(&str, &str)] = &[
    (
        "leaf.gcl",
        "param x: Dimensionless;\n\
         pub plot chart = { mark: point, encode: { x: @x } };\n\
         pub assert positive = @x > 0.0;\n\
         pub node y: Dimensionless = @x;\n\
         pub const node k: Dimensionless = 4.0;\n",
    ),
    (
        "mid.gcl",
        "include p.leaf(x: -1.0)::{ pub chart, pub positive, pub y, pub k };\n",
    ),
    (
        "facades.gcl",
        "pub dag facade {\n\
             include p.leaf(x: 2.0)::{ pub chart, pub y };\n\
         }\n",
    ),
    (
        "own.gcl",
        "pub dag inner {\n\
             param x: Dimensionless;\n\
             pub plot chart = { mark: point, encode: { x: @x } };\n\
         }\n\
         include inner(x: 3.0)::{ pub chart };\n",
    ),
    ("imported.gcl", "import p.leaf::{ pub k };\n"),
];

fn facade_package(main: &str) -> (tempfile::TempDir, PathBuf) {
    let mut files = FACADE_PACKAGE.to_vec();
    files.push(("main.gcl", main));
    package(&files)
}

fn surface_names(result: &EvalResult) -> Vec<String> {
    result
        .output_values(EvalOutputView::Surface)
        .map(|entry| entry.name.to_string())
        .collect()
}

fn plot_names(result: &EvalResult) -> Vec<String> {
    result
        .plots
        .iter()
        .map(|plot| plot.name.to_string())
        .collect()
}

#[test]
fn reexported_value_is_an_output_of_a_selective_include() {
    for main in [
        "include p.mid()::{ y };\n",
        "include p.facades.facade()::{ y };\n",
    ] {
        let (_dir, root) = facade_package(main);
        let result = eval(&root);
        assert_eq!(surface_names(&result), ["y"], "{main}");
    }
}

#[test]
fn reexported_value_is_an_output_of_a_module_include() {
    let (_dir, root) = facade_package("include p.mid() as m;\nnode z: Dimensionless = @m::y;\n");
    let result = eval(&root);
    let names = surface_names(&result);
    assert!(names.contains(&"m::y".to_owned()), "{names:?}");
    assert_eq!(si_value(&result, "z"), -1.0);
}

#[test]
fn reexported_plot_is_requested_through_every_template_form() {
    for main in [
        "include p.mid()::{ chart, y };\n",
        "include p.facades.facade()::{ chart, y };\n",
        "include p.own()::{ chart };\n",
    ] {
        let (_dir, root) = facade_package(main);
        let result = eval(&root);
        assert_eq!(plot_names(&result), ["chart"], "{main}");
    }
    // A figure can name a plot an inline DAG re-exports.
    let (_dir, root) =
        facade_package("include p.facades.facade()::{ chart };\nfigure f = { plots: [chart] };\n");
    assert_eq!(eval(&root).figures.len(), 1);
}

#[test]
fn reexported_const_is_a_constant_of_the_importer() {
    for main in [
        "include p.mid()::{ k };\n",
        "include p.mid()::{ k };\nconst node k2: Dimensionless = @k * 2.0;\n",
        "include p.leaf(x: 1.0)::{ k };\nconst node k2: Dimensionless = @k * 2.0;\n",
        "include p.imported()::{ k };\nconst node k2: Dimensionless = @k * 2.0;\n",
    ] {
        let (_dir, root) = facade_package(main);
        let result = eval(&root);
        if main.contains("k2") {
            assert_eq!(si_value(&result, "k2"), 8.0, "{main}");
        } else {
            assert_eq!(si_value(&result, "k"), 4.0, "{main}");
        }
    }
}

#[test]
fn reexported_assertion_is_reported_once_with_its_include_site_expectation() {
    let (_dir, root) = facade_package("include p.mid()::{ positive, y };\n");
    let result = eval(&root);
    let reported = result
        .assertions
        .iter()
        .map(|assertion| assertion.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(reported, ["positive"]);
    assert!(matches!(
        result.assertions[0].outcome,
        AssertResult::Fail { .. }
    ));

    let (_dir, root) = facade_package("include p.mid()::{ #[expected_fail] positive, y };\n");
    let result = eval(&root);
    assert_eq!(result.assertions.len(), 1);
    assert!(
        !matches!(result.assertions[0].outcome, AssertResult::Fail { .. }),
        "{:?}",
        result.assertions[0].outcome
    );
}

//! Regression tests: assertions and plots selected from an include report in
//! the order the selection list names them, on every run (PRJ-8, #2003).
//!
//! The selections used to live in `HashMap`s, whose per-instance random seed
//! changed the order on every evaluation, so each project is evaluated
//! repeatedly.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_eval::eval::EvalResult;
use graphcal_io::RealFileSystem;
use graphcal_project::prepare::compile_and_eval_project;

const LIBRARY: &str = "\
param x: Dimensionless = 1.0;
pub assert a1 = @x > 0.0;
pub assert a2 = @x > 0.0;
pub assert a3 = @x > 0.0;
pub assert a4 = @x > 0.0;
pub assert a5 = @x > 0.0;
pub plot c1 = { mark: point, encode: { x: 1.0 } };
pub plot c2 = { mark: point, encode: { x: 1.0 } };
pub plot c3 = { mark: point, encode: { x: 1.0 } };
pub plot c4 = { mark: point, encode: { x: 1.0 } };
pub plot c5 = { mark: point, encode: { x: 1.0 } };
";

/// Write a package `p` with the library above and `main`, returning the path
/// of `main.gcl`.
fn write_project(main: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src").join("p");
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"p\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("lib.gcl"), LIBRARY).unwrap();
    std::fs::write(package_dir.join("main.gcl"), main).unwrap();
    (dir, package_dir.join("main.gcl"))
}

fn eval(root: &std::path::Path) -> EvalResult {
    compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|error| panic!("project must evaluate: {error:?}"))
}

fn assertion_names(result: &EvalResult) -> Vec<String> {
    result
        .assertions
        .iter()
        .map(|assertion| assertion.name.to_string())
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
fn selected_assertions_report_in_selection_order() {
    let (_dir, root) = write_project("include p.lib()::{ a3, a1, a5 as last, a2, a4 };\n");
    for _ in 0..16 {
        assert_eq!(
            assertion_names(&eval(&root)),
            ["a3", "a1", "last", "a2", "a4"]
        );
    }
}

#[test]
fn selected_plots_present_in_selection_order() {
    let (_dir, root) = write_project("include p.lib()::{ c3, c1, c5 as last, c2, c4 };\n");
    for _ in 0..16 {
        assert_eq!(plot_names(&eval(&root)), ["c3", "c1", "last", "c2", "c4"]);
    }
}

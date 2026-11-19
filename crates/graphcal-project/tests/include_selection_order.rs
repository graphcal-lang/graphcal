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

/// Assertions that all fail for `x = 1.0`.
const FAILING_LIBRARY: &str = "\
param x: Dimensionless;
pub assert f1 = @x > 2.0;
pub assert f2 = @x > 2.0;
pub assert f3 = @x > 2.0;
pub assert f4 = @x > 2.0;
pub assert f5 = @x > 2.0;
";

/// Write a package `p` with the libraries above and `main`, returning the path
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
    std::fs::write(package_dir.join("failing.gcl"), FAILING_LIBRARY).unwrap();
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

/// `model serve` reports a row's first failing assertion; with several
/// selected assertions failing, that is the first one selected.
#[test]
fn model_serve_reports_the_first_selected_failing_assertion() {
    use graphcal_model_protocol::{
        FiniteF64, InputColumn, PortName, RequestBatch, RowFailure, RowOutcome, SessionSpec,
    };

    let (_dir, root) = write_project(
        "param x: Dimensionless = 1.0;\n\
         include p.failing(x: @x)::{ f3, f1, f5 as last, f2, f4 };\n\
         pub node y: Dimensionless = @x;\n",
    );
    // The order is fixed when the project compiles, so compile on each run.
    for _ in 0..16 {
        let project =
            graphcal_project::loader::load_project(&root, None, &RealFileSystem::default())
                .unwrap();
        let prepared = graphcal_project::project_compiler::ProjectCompiler::new(&project)
            .prepare()
            .unwrap();
        let schema = std::sync::Arc::new(
            prepared
                .session_schema(&SessionSpec::new(
                    vec![PortName::new("x").unwrap()],
                    vec![PortName::new("y").unwrap()],
                ))
                .unwrap(),
        );
        let session = prepared
            .model_session(std::sync::Arc::clone(&schema))
            .unwrap();
        let request = RequestBatch::new(
            std::sync::Arc::clone(&schema),
            vec![InputColumn::Quantity(vec![FiniteF64::new(1.0).unwrap()])],
        )
        .unwrap();
        let result = prepared.evaluate_request(&session, &request).unwrap();
        let Some(RowOutcome::Failure(RowFailure::AssertionFailed { assertion, .. })) =
            result.row(0)
        else {
            panic!("the row must fail an assertion");
        };
        assert_eq!(assertion, "f3");
    }
}

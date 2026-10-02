//! Regression tests for unresolvable imports inside inline DAG bodies (L-6).
//!
//! A body import or include whose module path does not resolve used to be
//! dropped silently: unused, the project compiled; used, the reference failed
//! later as an unknown graph reference. It is now rejected at its path with
//! the same diagnostic a file-root import of that path gets, in standalone
//! (manifest-less) and package projects alike.
#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use graphcal_io::RealFileSystem;
use graphcal_project::prepare::compile_and_eval_project;

const UNUSED: &str = "\
dag lib {
    import nosuchmod::{ foo };
    param a: Dimensionless;
    pub node b: Dimensionless = @a * 2.0;
}
param x: Dimensionless = 1.0;
include lib(a: @x)::{ b as bb };
node r: Dimensionless = @bb;
";

const USED: &str = "\
dag lib {
    import nosuchmod::{ foo };
    param a: Dimensionless;
    pub node b: Dimensionless = @a * @foo;
}
param x: Dimensionless = 1.0;
include lib(a: @x)::{ b as bb };
node r: Dimensionless = @bb;
";

/// A standalone file outside any package.
fn write_standalone(source: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main.gcl");
    std::fs::write(&main, source).unwrap();
    (dir, main)
}

/// The entry file `src/pk/main.gcl` of a package named `pk`.
fn write_package(source: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"pk\"\n",
    )
    .unwrap();
    let package_dir = dir.path().join("src").join("pk");
    std::fs::create_dir_all(&package_dir).unwrap();
    let main = package_dir.join("main.gcl");
    std::fs::write(&main, source).unwrap();
    (dir, main)
}

/// Diagnostic code of the error rejecting `root`.
fn rejection_code(root: &Path) -> String {
    let error = compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default())
        .map(|_| ())
        .expect_err("project must be rejected");
    miette::Diagnostic::code(&error)
        .expect("error must have a code")
        .to_string()
}

#[test]
fn standalone_dag_body_import_of_another_file_is_rejected() {
    for source in [UNUSED, USED] {
        let (_dir, main) = write_standalone(source);
        assert_eq!(rejection_code(&main), "graphcal::M017");
    }
}

#[test]
fn package_dag_body_import_with_a_foreign_first_segment_is_rejected() {
    for source in [UNUSED, USED] {
        let (_dir, main) = write_package(source);
        assert_eq!(rejection_code(&main), "graphcal::M013");
    }
}

#[test]
fn package_dag_body_import_of_a_missing_module_is_rejected() {
    for source in [UNUSED, USED] {
        let (_dir, main) = write_package(&source.replace("nosuchmod", "pk.nosuch"));
        assert_eq!(rejection_code(&main), "graphcal::M002");
    }
}

#[test]
fn package_dag_body_import_of_a_sibling_file_still_resolves() {
    let (dir, main) = write_package(
        "\
dag lib {
    import pk.helper::{ two };
    param a: Dimensionless;
    pub node b: Dimensionless = @a * @two;
}
param x: Dimensionless = 1.5;
include lib(a: @x)::{ b as bb };
node r: Dimensionless = @bb;
",
    );
    std::fs::write(
        dir.path().join("src/pk/helper.gcl"),
        "pub const node two: Dimensionless = 2.0;",
    )
    .unwrap();
    let result = compile_and_eval_project(&main, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|error| panic!("project must evaluate: {error}"));
    let r = result
        .entries
        .iter()
        .find(|(name, _, _)| name.to_string() == "r")
        .and_then(|(_, value, _)| value.as_ref().ok())
        .and_then(|value| value.si_value().ok())
        .expect("`r` must evaluate")
        .get();
    assert!((r - 3.0).abs() < 1e-12, "r = {r}");
}

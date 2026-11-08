//! Regression tests for items a dependency re-exports from its own includes,
//! and for the namespaces of selective-include items (#2259).
#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use graphcal_eval::eval::EvalResult;
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

//! Regression tests: when several bindings are bad, the reported one is the
//! same on every run (PRJ-13).
//!
//! The bindings used to be visited in `HashMap` order, whose per-instance
//! random seed changed the reported binding between runs, so each project is
//! checked repeatedly.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_io::RealFileSystem;
use graphcal_project::compile_error::CompileError;
use graphcal_project::prepare::{ExternalValue, compile_and_eval_project};

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

fn error(root: &std::path::Path, overrides: &HashMap<DeclName, ExternalValue>) -> CompileError {
    compile_and_eval_project(root, overrides, None, &RealFileSystem::default())
        .expect_err("project must be rejected")
}

#[test]
fn several_bad_include_index_bindings_report_the_first_in_source_order() {
    let (_dir, root) = write_project(&[
        (
            "lib.gcl",
            "pub(bind) index A;\npub(bind) index B;\npub(bind) index C;\npub(bind) index D;\n\
             pub node out: Dimensionless = 1.0;\n",
        ),
        (
            "main.gcl",
            "include p.lib(index C: Nope3, index A: Nope1, index D: Nope4, index B: Nope2)::{ out };\n",
        ),
    ]);
    for _ in 0..16 {
        let message = error(&root, &HashMap::new()).to_string();
        assert!(
            message.contains("`C: Nope3`"),
            "expected the first authored binding, got {message}"
        );
    }
}

#[test]
fn several_bad_parameter_values_report_the_first_parameter_port() {
    let (_dir, root) = write_project(&[(
        "main.gcl",
        "param a: Length;\nparam b: Length;\nparam c: Length;\nparam d: Length;\n\
         node s: Length = @a + @b + @c + @d;\n",
    )]);
    let overrides = ["d", "c", "b", "a"]
        .into_iter()
        .map(|name| {
            (
                DeclName::expect_valid(name),
                ExternalValue::parse(format!("<value {name}>"), "3").unwrap(),
            )
        })
        .collect::<HashMap<_, _>>();
    for _ in 0..16 {
        let error = format!("{:?}", error(&root, &overrides));
        assert!(
            error.contains("<value a>") && !error.contains("<value b>"),
            "expected the binding of the first parameter `a`, got {error}"
        );
    }
}

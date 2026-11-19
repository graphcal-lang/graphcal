//! Which module a one-segment `import` / `include` path names (#2242).
//!
//! A file-root path names a top-level `dag` of its own file, and otherwise a
//! module found by package resolution; it is never dropped.
#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use graphcal_eval::eval::{AssertResult, EvalResult, Value};
use graphcal_io::RealFileSystem;
use graphcal_project::compile_error::CompileError;
use graphcal_project::load_error::LoadError;
use graphcal_project::prepare::compile_and_eval_project;
use miette::Diagnostic;

/// A package `name` whose files are written below `src/`, by path relative
/// to `src/`.
fn package(name: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        format!("[package]\nname = \"{name}\"\n"),
    )
    .unwrap();
    for (path, source) in files {
        let path = dir.path().join("src").join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    dir
}

fn source_path(dir: &tempfile::TempDir, path: &str) -> PathBuf {
    dir.path().join("src").join(path)
}

fn eval(root: &Path) -> EvalResult {
    compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default()).unwrap()
}

fn eval_error(root: &Path) -> CompileError {
    compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default()).unwrap_err()
}

fn assertion_failed(result: &EvalResult, name: &str) -> bool {
    result.assertions.iter().any(|assertion| {
        assertion.name.to_string() == name && matches!(assertion.outcome, AssertResult::Fail { .. })
    })
}

const PIPELINE_ROOT: &str = "\
param x: Dimensionless;
pub assert positive = @x > 0.0;
pub node y: Dimensionless = @x;
";

#[test]
fn file_root_include_is_not_shadowed_by_a_nested_inline_dag() {
    // The include names the package root `src/pipeline.gcl`. A nested DAG of
    // the same name is not visible from the file root, so it must neither
    // shadow nor drop the include and its assertion.
    let dir = package(
        "pipeline",
        &[
            ("pipeline.gcl", PIPELINE_ROOT),
            (
                "pipeline/main.gcl",
                "include pipeline(x: -1.0) as p;\n\
                 dag engine {\n\
                     dag pipeline {\n\
                         pub node z: Dimensionless = 1.0;\n\
                     }\n\
                 }\n\
                 node a: Dimensionless = 1.0;\n",
            ),
        ],
    );
    let result = eval(&source_path(&dir, "pipeline/main.gcl"));
    assert!(
        assertion_failed(&result, "p::positive"),
        "{:?}",
        result.assertions
    );
}

#[test]
fn file_root_brace_include_is_not_shadowed_by_a_nested_inline_dag() {
    let dir = package(
        "pipeline",
        &[
            ("pipeline.gcl", PIPELINE_ROOT),
            (
                "pipeline/main.gcl",
                "include pipeline(x: -1.0)::{y};\n\
                 dag engine { dag pipeline { } }\n\
                 node a: Bool = @y < 0.0;\n",
            ),
        ],
    );
    let result = eval(&source_path(&dir, "pipeline/main.gcl"));
    let a = result
        .entries
        .iter()
        .find(|entry| entry.name.to_string() == "a")
        .unwrap();
    assert!(
        matches!(a.outcome, Ok(Value::Bool(true))),
        "{:?}",
        a.outcome
    );
}

#[test]
fn file_root_include_of_a_top_level_dag_is_instantiated() {
    let dir = package(
        "pipeline",
        &[(
            "pipeline/main.gcl",
            "dag check {\n\
                 param x: Dimensionless;\n\
                 pub assert positive = @x > 0.0;\n\
             }\n\
             include check(x: -1.0) as c;\n\
             node a: Dimensionless = 1.0;\n",
        )],
    );
    let result = eval(&source_path(&dir, "pipeline/main.gcl"));
    assert!(
        assertion_failed(&result, "c::positive"),
        "{:?}",
        result.assertions
    );
}

#[test]
fn standalone_file_root_include_of_its_own_stem_is_rejected() {
    // `solo` names the file itself; including it would expand forever.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("solo.gcl");
    std::fs::write(
        &root,
        "param x: Dimensionless = 1.0;\n\
         assert positive = @x > 0.0;\n\
         include solo(x: -1.0) as again;\n\
         node a: Dimensionless = @x;\n",
    )
    .unwrap();
    let error = eval_error(&root);
    assert!(
        format!("{error:?}").contains("Recursive"),
        "expected a recursive include error, got {error:?}"
    );
}

#[test]
fn file_root_import_of_a_top_level_dag_binds_its_items() {
    // Previously dropped: `@c` was unknown, or silently bound a same-named
    // file-root declaration.
    let dir = package(
        "pipeline",
        &[(
            "pipeline/main.gcl",
            "pub dag d {\n\
                 pub const node c: Dimensionless = 2.0;\n\
             }\n\
             import d::{c};\n\
             node a: Bool = @c == 2.0;\n",
        )],
    );
    let result = eval(&source_path(&dir, "pipeline/main.gcl"));
    let a = result
        .entries
        .iter()
        .find(|entry| entry.name.to_string() == "a")
        .unwrap();
    assert!(
        matches!(a.outcome, Ok(Value::Bool(true))),
        "{:?}",
        a.outcome
    );
}

#[test]
fn file_root_import_of_a_top_level_dag_conflicts_with_a_same_named_declaration() {
    let dir = package(
        "pipeline",
        &[(
            "pipeline/main.gcl",
            "pub dag d {\n\
                 pub const node c: Dimensionless = 2.0;\n\
             }\n\
             import d::{c};\n\
             const node c: Dimensionless = 5.0;\n",
        )],
    );
    let error = eval_error(&source_path(&dir, "pipeline/main.gcl"));
    assert!(
        format!("{error:?}").contains("Duplicate"),
        "expected a duplicate-name error, got {error:?}"
    );
}

const NASA_ROOT: &str = "pub const node g0: Dimensionless = 1.0;\n";

const NASA_INLINE_IMPORT: &str = "\
pub const node g0: Dimensionless = 2.0;
pub dag inner {
    import nasa::{g0};
    pub node got: Dimensionless = @g0;
}
include inner() as i;
pub node result: Bool = @i::got == 1.0;
";

fn entry_is_true(result: &EvalResult, name: &str) -> bool {
    result.entries.iter().any(|entry| {
        entry.name.to_string() == name && matches!(entry.outcome, Ok(Value::Bool(true)))
    })
}

#[test]
fn package_dag_body_path_spelled_as_the_file_stem_is_rejected() {
    // Earlier releases bound the file's own `g0` here; the package root is the
    // documented meaning, so the ambiguous spelling fails loudly instead of
    // silently changing which module it names.
    let dir = package(
        "nasa",
        &[
            ("nasa.gcl", NASA_ROOT),
            ("nasa/nasa.gcl", NASA_INLINE_IMPORT),
        ],
    );
    let error = eval_error(&source_path(&dir, "nasa/nasa.gcl"));
    let CompileError::Load(LoadError::PackageFileStemInDagBody { stem, help, .. }) = &error else {
        panic!("expected M037, got {error:?}");
    };
    assert_eq!(stem.as_str(), "nasa");
    assert!(help.contains("`nasa.nasa`"), "{help}");
    assert_eq!(
        error.code().map(|code| code.to_string()).as_deref(),
        Some("graphcal::M037")
    );
}

#[test]
fn package_dag_body_path_naming_the_package_resolves_from_another_file() {
    let dir = package(
        "nasa",
        &[
            ("nasa.gcl", NASA_ROOT),
            ("nasa/other.gcl", NASA_INLINE_IMPORT),
        ],
    );
    let result = eval(&source_path(&dir, "nasa/other.gcl"));
    assert!(entry_is_true(&result, "result"), "{:?}", result.entries);
}

#[test]
fn package_file_root_import_spelled_as_the_file_stem_names_the_package() {
    // Not a self-import (M033): `nasa` is the package root `src/nasa.gcl`.
    let dir = package(
        "nasa",
        &[
            ("nasa.gcl", NASA_ROOT),
            (
                "nasa/nasa.gcl",
                "import nasa::{g0};\nnode result: Bool = @g0 == 1.0;\n",
            ),
        ],
    );
    let result = eval(&source_path(&dir, "nasa/nasa.gcl"));
    assert!(entry_is_true(&result, "result"), "{:?}", result.entries);
}

#[test]
fn package_file_root_include_spelled_as_the_file_stem_names_the_package() {
    let dir = package(
        "pipeline",
        &[
            ("pipeline.gcl", PIPELINE_ROOT),
            (
                "pipeline/pipeline.gcl",
                "include pipeline(x: -1.0) as p;\nnode a: Dimensionless = 1.0;\n",
            ),
        ],
    );
    let result = eval(&source_path(&dir, "pipeline/pipeline.gcl"));
    assert!(
        assertion_failed(&result, "p::positive"),
        "{:?}",
        result.assertions
    );
}

#[test]
fn standalone_dag_body_stem_is_a_self_reference() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("solo.gcl");
    std::fs::write(
        &root,
        "pub const node g0: Dimensionless = 2.0;\n\
         dag inner {\n\
             import solo::{g0};\n\
             pub node got: Dimensionless = @g0;\n\
         }\n\
         node result: Bool = @inner()::got == 2.0;\n",
    )
    .unwrap();
    let result = eval(&root);
    assert!(entry_is_true(&result, "result"), "{:?}", result.entries);
}

//! Regression tests for the Phase 4 resolver refactor
//! (`.local/2026-09-26_invariant-complexity-implementation-plan.md` §7).
#![cfg(test)]

use std::collections::HashMap;

use graphcal_eval::eval::EvalResult;
use graphcal_io::RealFileSystem;
use graphcal_project::prepare::compile_and_eval_project;

/// Write a single-package project whose files are given relative to the
/// package source directory (subdirectories allowed).
fn write_project(
    package: &str,
    files: &[(&str, &str)],
    entry: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let package_dir = dir.path().join("src").join(package);
    std::fs::write(
        dir.path().join("graphcal.toml"),
        format!("[package]\nname = \"{package}\"\n"),
    )
    .unwrap();
    for (name, source) in files {
        let path = package_dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    (dir, package_dir.join(entry))
}

fn eval(root: &std::path::Path) -> EvalResult {
    compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default())
        .unwrap_or_else(|error| panic!("project must evaluate: {error}"))
}

fn eval_error(root: &std::path::Path) -> String {
    compile_and_eval_project(root, &HashMap::new(), None, &RealFileSystem::default())
        .map(|_| ())
        .expect_err("project must be rejected")
        .to_string()
}

fn si_value(result: &EvalResult, name: &str) -> f64 {
    result
        .entries
        .iter()
        .find(|(decl_name, _, _)| decl_name.to_string() == name)
        .unwrap_or_else(|| panic!("declaration `{name}` not found"))
        .1
        .as_ref()
        .unwrap_or_else(|error| panic!("declaration `{name}` has error: {error}"))
        .si_value()
        .unwrap()
        .get()
}

/// P4-1b (breaking): a file submodule `lib/x.gcl` and an inline `dag x` in
/// `lib.gcl` share the module path `p.lib.x`. Neither is preferred; loading
/// both is an explicit ambiguity error.
#[test]
fn file_submodule_and_inline_dag_sharing_a_module_path_are_ambiguous() {
    let (_dir, root) = write_project(
        "p",
        &[
            (
                "lib.gcl",
                "pub dag x {\n    pub const node a: Dimensionless = 1.0;\n}\n",
            ),
            ("lib/x.gcl", "pub const node a: Dimensionless = 2.0;\n"),
            (
                "main.gcl",
                "import p.lib.x::{ a };\nimport p.lib::{ x };\nconst node out: Dimensionless = a;\n",
            ),
        ],
        "main.gcl",
    );
    let error = eval_error(&root);
    assert!(
        error.contains("module path `src.p.lib.x` is ambiguous"),
        "unexpected error: {error}"
    );
}

/// P4-1b (breaking): an alias of a file module qualifies only that file's
/// inline DAGs; `l.x` no longer reaches the separate file `lib/x.gcl`.
#[test]
fn alias_qualifier_names_inline_dags_not_file_submodules() {
    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", "pub const node b: Dimensionless = 1.0;\n"),
            ("lib/x.gcl", "pub const node a: Dimensionless = 2.0;\n"),
            (
                "main.gcl",
                "import p.lib as l;\nimport p.lib.x as lx;\nconst node out: Dimensionless = l.x::a;\n",
            ),
        ],
        "main.gcl",
    );
    let error = eval_error(&root);
    assert!(
        error.contains("unknown module `src.p.lib.x`"),
        "unexpected error: {error}"
    );

    // The file submodule stays reachable through its own module path.
    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", "pub const node b: Dimensionless = 1.0;\n"),
            ("lib/x.gcl", "pub const node a: Dimensionless = 2.0;\n"),
            (
                "main.gcl",
                "import p.lib as l;\nimport p.lib.x as lx;\nconst node out: Dimensionless = lx::a + l::b;\n",
            ),
        ],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "out") - 3.0).abs() < 1e-9);
}

/// P4-3 (known bug from P3-6): selecting a `const node` through a selective
/// include of a same-file inline DAG reported "unknown module
/// `main.<include@N>`", because the include's instance was never registered
/// with the module resolver.
#[test]
fn selective_include_of_a_local_dag_can_select_a_const_node() {
    let result = graphcal_project::prepare::compile_and_eval(
        "dag lib {\n\
         \x20   pub const node k: Dimensionless = 3.0;\n\
         \x20   param v: Dimensionless;\n\
         \x20   pub node w: Dimensionless = @v * 2.0;\n\
         }\n\
         param x: Dimensionless = 1.0;\n\
         include lib(v: @x)::{ k as kk, w };\n\
         node out: Dimensionless = @kk + @w;\n",
    )
    .unwrap_or_else(|error| panic!("local const selection must compile: {error}"));
    assert!((si_value(&result, "kk") - 3.0).abs() < 1e-9);
    assert!((si_value(&result, "out") - 5.0).abs() < 1e-9);
}

/// P4-3: the names a selective include of a same-file inline DAG introduces
/// share the collision unit with imports, like every other include.
#[test]
fn local_dag_include_selection_collides_with_an_import() {
    let (_dir, root) = write_project(
        "p",
        &[
            ("dep.gcl", "pub const node k: Dimensionless = 1.0;\n"),
            (
                "main.gcl",
                "import p.dep::{ k };\n\
                 dag lib {\n\
                 \x20   pub const node k: Dimensionless = 3.0;\n\
                 }\n\
                 include lib()::{ k };\n\
                 const node out: Dimensionless = k;\n",
            ),
        ],
        "main.gcl",
    );
    let error = eval_error(&root);
    assert!(
        error.contains("duplicate name `k`"),
        "unexpected error: {error}"
    );
}

/// A runtime unit used through an instance projection (`1.0 inst::qu`)
/// reads the instance's params: the reader is scheduled after them, so the
/// scale, evaluated in the instance's scope, finds their values.
#[test]
fn instance_runtime_unit_used_by_the_including_dag_reads_instance_values() {
    let library = r"
param k: Dimensionless = 2.0;
pub unit qu: Length = (@k) m;
pub node out: Length = 3.0 qu;
";
    let inline = format!(
        "dag lib {{{library}}}\n\
         include lib(k: 5.0) as inst;\n\
         node o: Length = @inst::out;\n\
         node o4: Length = 1.0 inst::qu;\n"
    );
    let (_dir, root) = write_project("unitinline", &[("main.gcl", &inline)], "main.gcl");
    let result = eval(&root);
    assert!((si_value(&result, "o4") - 5.0).abs() < 1e-12);
    assert!((si_value(&result, "o") - 15.0).abs() < 1e-12);

    let (_dir, root) = write_project(
        "unitfile",
        &[
            ("lib.gcl", library),
            (
                "main.gcl",
                "include unitfile.lib(k: 5.0) as inst;\n\
                 node o4: Length = 1.0 inst::qu;\n",
            ),
        ],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "o4") - 5.0).abs() < 1e-12);
}

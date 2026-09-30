//! Regression tests for canonical dimension, unit, and index definitions
//! (P5-7, `.local/2026-09-26_invariant-complexity-implementation-plan.md` §8).
//!
//! Every name in a `dim` / `unit` / `index` declaration resolves through the
//! module resolver of its declaring module, exactly like HIR type lowering.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_eval::eval::EvalResult;
use graphcal_io::RealFileSystem;
use graphcal_project::prepare::compile_and_eval_project;

/// Write a single-package project whose files are given relative to the
/// package source directory.
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

const BINDABLE_LIBRARY: &str = "pub(bind) dim Q;\n\
     pub dim QR = Q / Time;\n\
     param q: Q;\n\
     pub node r: QR = @q / 2.0 s;\n";

/// Bug fix: a projected dimension defined over a dimension port bound to one
/// of the importer's own dimensions specializes through that dimension. It
/// used to stay opaque, so `R` mismatched the instance's output type.
#[test]
fn projected_dimension_bound_to_a_local_dimension_is_specialized() {
    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", BINDABLE_LIBRARY),
            (
                "main.gcl",
                "dim Mine = Length * Mass;\n\
                 include p.lib(dim Q: Mine, q: 4.0 m * kg)::{dim QR as R, r};\n\
                 node check: R = @r;\n",
            ),
        ],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "check") - 2.0).abs() < 1e-12);
}

/// A projected dimension bound to a prelude dimension keeps working.
#[test]
fn projected_dimension_bound_to_a_prelude_dimension_is_specialized() {
    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", BINDABLE_LIBRARY),
            (
                "main.gcl",
                "include p.lib(dim Q: Length, q: 4.0 m)::{dim QR as R, r};\n\
                 const unit mps: R = 1.0 m / s;\n\
                 node check: R = @r;\n",
            ),
        ],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "check") - 2.0).abs() < 1e-12);
}

/// Bug fix: the type-system declarations of an included inline DAG resolve
/// its `import <self>::{...}` items. They used to be unknown when the DAG was
/// first lowered as an include template.
#[test]
fn included_inline_dag_type_system_self_imports_resolve() {
    let (_dir, root) = write_project(
        "p",
        &[(
            "main.gcl",
            "pub dim Speed = Length / Time;\n\
             pub const unit furl: Length = 201.168 m;\n\
             dag d {\n\
               import p.main::{dim Speed, unit furl};\n\
               pub dim S2 = Speed^2;\n\
               const unit f2: Length = 2.0 furl;\n\
               param a: Speed = 1.0 m/s;\n\
               pub node x: S2 = @a * 1.0 m/s;\n\
               pub node w: Length = 1.0 f2;\n\
             }\n\
             include d(a: 1.0 m/s)::{x, w};\n",
        )],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "x") - 1.0).abs() < 1e-12);
    assert!((si_value(&result, "w") - 402.336).abs() < 1e-9);
}

/// Corner-case breaking fix: an inline DAG is isolated from its enclosing
/// file (Concept 9) in its unit declarations too, whether or not it is
/// included. An unused DAG used to see the parent's bare units.
#[test]
fn inline_dag_unit_declarations_cannot_name_parent_units() {
    let (_dir, root) = write_project(
        "p",
        &[(
            "main.gcl",
            "const unit furl: Length = 201.168 m;\n\
             dag d {\n\
               const unit f2: Length = 2.0 furl;\n\
               param a: Length = 1.0 m;\n\
               pub node x: Length = @a;\n\
             }\n\
             node y: Length = 1.0 m;\n",
        )],
        "main.gcl",
    );
    let error = eval_error(&root);
    assert!(error.contains("unknown unit `furl`"), "{error}");
}

/// Corner-case breaking fix: an include alias names the instance's outputs,
/// not a Static or Unit namespace. Dimension and unit declarations used to
/// accept `inst::Name` while every other position rejected it.
#[test]
fn include_alias_is_not_a_static_or_unit_namespace() {
    let library = "pub const unit furl: Length = 201.168 m;\n\
                   pub dim Speed = Length / Time;\n\
                   param x: Length = 1.0 m;\n\
                   pub node y: Length = @x * 2.0;\n";
    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", library),
            (
                "main.gcl",
                "include p.lib() as inst;\nconst unit two: Length = 2.0 inst::furl;\n",
            ),
        ],
        "main.gcl",
    );
    let error = eval_error(&root);
    assert!(error.contains("unknown unit `inst::furl`"), "{error}");

    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", library),
            (
                "main.gcl",
                "include p.lib() as inst;\ndim S3 = inst::Speed;\n",
            ),
        ],
        "main.gcl",
    );
    let error = eval_error(&root);
    assert!(error.contains("unknown dimension `inst::Speed`"), "{error}");
}

/// Pure imports keep exposing a module's static dimensions and units to the
/// importer's own declarations under the module alias.
#[test]
fn module_import_alias_reaches_static_dimensions_and_units() {
    let (_dir, root) = write_project(
        "p",
        &[
            (
                "lib.gcl",
                "pub const unit furl: Length = 201.168 m;\npub dim Speed = Length / Time;\n",
            ),
            (
                "main.gcl",
                "import p.lib as l;\n\
                 const unit two: Length = 2.0 l::furl;\n\
                 dim S3 = l::Speed;\n\
                 node v: S3 = 2.0 m/s;\n\
                 node z: Length = 1.0 two;\n",
            ),
        ],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "v") - 2.0).abs() < 1e-12);
    assert!((si_value(&result, "z") - 402.336).abs() < 1e-9);
}

/// A runtime unit still never crosses a pure import, even inside another
/// unit's definition.
#[test]
fn runtime_unit_does_not_cross_a_pure_import_in_unit_definitions() {
    let (_dir, root) = write_project(
        "p",
        &[
            (
                "lib.gcl",
                "pub base dim Money;\n\
                 pub base unit USD: Money;\n\
                 param rate: Dimensionless = 2.0;\n\
                 pub unit EUR: Money = (@rate) USD;\n",
            ),
            (
                "main.gcl",
                "import p.lib as l;\nunit twice: l::Money = 2.0 l::EUR;\n",
            ),
        ],
        "main.gcl",
    );
    let error = eval_error(&root);
    assert!(error.contains("EUR"), "{error}");
}

const SPECIALIZED_TYPE_LIBRARY: &str = "pub(bind) dim Q;\n\
     pub dim QR = Q / Time;\n\
     pub type Inner { Inner(x: Q) }\n\
     pub type Box { Box(v: Q, i: Inner) }\n\
     pub type Rated { Rated(r: QR) }\n\
     param q: Q;\n\
     pub node b: Box = Box(v: @q, i: Inner(x: @q));\n";

/// Bug fix: a type projected under Static bindings is specialized from the
/// template's canonical definition, so its fields may name other template
/// types. The template signature used to be re-read in the importer's scope,
/// where `Inner` is unknown.
#[test]
fn specialized_type_projection_keeps_template_field_types() {
    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", SPECIALIZED_TYPE_LIBRARY),
            (
                "main.gcl",
                "include p.lib(dim Q: Length, q: 2.0 m)::{type Box, Box, b};\n\
                 node v: Length = match @b { Box(v: value, i: _) => value };\n",
            ),
        ],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "v") - 2.0).abs() < 1e-12);
}

/// A specialized type whose field names a derived dimension of a bound port
/// follows the include binding when the same include projects that dimension.
#[test]
fn specialized_type_projection_follows_the_projected_dimension() {
    let (_dir, root) = write_project(
        "p",
        &[
            ("lib.gcl", SPECIALIZED_TYPE_LIBRARY),
            (
                "main.gcl",
                "include p.lib(dim Q: Length, q: 2.0 m)::{type Rated, Rated, dim QR};\n\
                 node r: Rated = Rated(r: 3.0 m/s);\n\
                 node speed: QR = match @r { Rated(r: value) => value };\n",
            ),
        ],
        "main.gcl",
    );
    let result = eval(&root);
    assert!((si_value(&result, "speed") - 3.0).abs() < 1e-12);
}

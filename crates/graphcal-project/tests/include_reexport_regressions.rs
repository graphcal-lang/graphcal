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

const TYPED_LIB: &str = "\
pub type Reading { Reading(value: Dimensionless) }
pub node r: Reading = Reading(value: 1.0);
";

const SHARED_SPELLING_LIB: &str = "\
pub type mode { A, B }
pub node mode: Dimensionless = 1.0;
pub node pick: mode = A;
";

/// Selecting only the constructor `Reading` must not project the template's
/// type onto the importer's unrelated type of the same spelling (PRJ-4).
#[test]
fn selected_constructor_does_not_rebind_a_same_named_importer_type() {
    let (_dir, main) = package(&[
        ("lib.gcl", TYPED_LIB),
        (
            "main.gcl",
            "type Reading { Other(v: Length) }\n\
             include p.lib()::{ r, Reading };\n\
             node mine: Reading = Other(v: 2.0 m);\n\
             node theirs: Dimensionless = match @r { Reading(value: x) => x };\n",
        ),
    ]);
    let result = eval(&main);
    assert_eq!(si_value(&result, "theirs"), 1.0);

    // A value of the template's type is not a value of the importer's.
    let (_dir, main) = package(&[
        ("lib.gcl", TYPED_LIB),
        (
            "main.gcl",
            "type Reading { Other(v: Length) }\n\
             include p.lib()::{ r, Reading };\n\
             node mine: Reading = @r;\n\
             node len: Length = match @mine { Other(v: x) => x };\n",
        ),
    ]);
    assert_eq!(code(&eval_error(&main)).as_deref(), Some("graphcal::D002"));
}

/// A `type` item selects only the type, never a value of the same spelling
/// (PRJ-4).
#[test]
fn selected_type_does_not_bind_a_same_named_value() {
    let (_dir, main) = package(&[
        ("lib2.gcl", SHARED_SPELLING_LIB),
        (
            "main.gcl",
            "include p.lib2()::{ type mode, pick };\n\
             node x: Dimensionless = @mode;\n",
        ),
    ]);
    assert_eq!(code(&eval_error(&main)).as_deref(), Some("graphcal::N002"));

    // The importer may declare its own value of that spelling.
    let (_dir, main) = package(&[
        ("lib2.gcl", SHARED_SPELLING_LIB),
        (
            "main.gcl",
            "include p.lib2()::{ type mode, pick };\n\
             node mode: Dimensionless = 2.0;\n",
        ),
    ]);
    assert_eq!(si_value(&eval(&main), "mode"), 2.0);
}

/// The V006 leakage check of a `{ pub type mode }` re-export inspects the
/// type, not a value that shares its spelling (PRJ-4).
#[test]
fn reexport_leakage_check_follows_the_selected_namespace() {
    let lib = "pub(bind) dim Q;\n\
               param v: Q;\n\
               pub node mode: Q = @v;\n\
               pub type mode { A, B }\n";
    let (_dir, main) = package(&[
        ("lib.gcl", lib),
        (
            "main.gcl",
            "dim Private = Length;\n\
             include p.lib(dim Q: Private, v: 1.0 m)::{ pub type mode, A };\n\
             node choice: mode = A;\n",
        ),
    ]);
    eval(&main);

    // Re-exporting the value itself still leaks the private dimension.
    let (_dir, main) = package(&[
        ("lib.gcl", lib),
        (
            "main.gcl",
            "dim Private = Length;\n\
             include p.lib(dim Q: Private, v: 1.0 m)::{ pub mode };\n",
        ),
    ]);
    assert_eq!(code(&eval_error(&main)).as_deref(), Some("graphcal::V006"));
}

const BOUNDED_LIB: &str = "\
const node LO: Length = 5.0 m;
param x: Length = 10.0 m;
pub node y: Length(min: @LO) = @x;
";

/// The domain bound of a selected value keeps the template's `@LO`, whatever
/// the importer itself declares under that name (RES-2).
#[test]
fn selected_value_bound_resolves_in_the_template() {
    for importer_lo in [
        "const node LO: Length = 100.0 m;\n",
        "param LO: Length = 100.0 m;\n",
        "const node LO: Mass = 1.0 kg;\n",
    ] {
        let (_dir, main) = package(&[
            ("lib.gcl", BOUNDED_LIB),
            (
                "main.gcl",
                &format!("{importer_lo}include p.lib(x: 7.0 m)::{{ y }};\nnode z: Length = @y;\n"),
            ),
        ]);
        let result = eval(&main);
        assert_eq!(si_value(&result, "z"), 7.0, "{importer_lo}");
    }

    // The template's own bound still applies.
    let (_dir, main) = package(&[
        ("lib.gcl", BOUNDED_LIB),
        ("main.gcl", "include p.lib(x: 1.0 m)::{ y };\n"),
    ]);
    let result = eval(&main);
    let y = result
        .entries
        .iter()
        .find(|entry| entry.name.to_string() == "y")
        .expect("y is an output");
    assert!(y.outcome.is_err(), "{:?}", y.outcome);
}

const DIMENSION_PORT_DAG: &str = "\
dag lib {
    pub(bind) dim Q;
    param v: Q;
    pub node w: Q = @v;
}
";

/// A dimension port bound to a prelude dimension can be selected under an
/// alias (TYC-9).
#[test]
fn dimension_port_bound_to_a_prelude_dimension_is_selectable() {
    for (dimension, value) in [("Length", "300.0 m"), ("Temperature", "300.0 K")] {
        let (_dir, main) = package(&[(
            "main.gcl",
            &format!(
                "{DIMENSION_PORT_DAG}\
                 include lib(dim Q: {dimension}, v: {value})::{{ w, dim Q as QQ }};\n\
                 node t5: QQ = @w;\n"
            ),
        )]);
        assert_eq!(si_value(&eval(&main), "t5"), 300.0, "{dimension}");
    }
}

/// A binding target that names no dimension is reported at the include,
/// not across the whole file (TYC-9).
#[test]
fn unknown_dimension_binding_target_is_reported_at_the_include() {
    for selection in ["w, dim Q as QQ", "w"] {
        let include = format!("include lib(dim Q: Nope, v: 300.0 m)::{{ {selection} }};");
        let source = format!("{DIMENSION_PORT_DAG}{include}\nnode t5: Length = @w;\n");
        let (_dir, main) = package(&[("main.gcl", &source)]);
        let CompileError::Eval(error) = eval_error(&main) else {
            panic!("expected a semantic error");
        };
        let graphcal_compiler::semantic_error::SemanticError::Located(diagnostic) = &error.error
        else {
            panic!("expected a located error, got {:?}", error.error);
        };
        assert_eq!(
            diagnostic.primary.offset(),
            source.find(&include).unwrap(),
            "{selection}"
        );
    }
}

const RECORD_LIB: &str = "\
pub(bind) index I = { a, b };
pub type T { T(x: Length[I]) }
param v: Length[I] = { I#a: 1.0 m, I#b: 2.0 m };
pub node out: T = T(x: @v);
pub type U { A(a: Length), B(b: Length) }
pub node u: U = A(a: 1.0 m);
";

/// Field access and `match` on a value read from an included instance do
/// not depend on the reading module naming or constructing its type
/// (RES-20, #1942).
#[test]
fn nominal_value_of_an_include_needs_no_local_mention_of_its_type() {
    let (_dir, main) = package(&[
        ("lib.gcl", RECORD_LIB),
        (
            "main.gcl",
            "include p.lib() as l;\n\
             node z: Length = sum(@l::out.x);\n",
        ),
    ]);
    assert_eq!(si_value(&eval(&main), "z"), 3.0);

    let (_dir, main) = package(&[
        ("lib.gcl", RECORD_LIB),
        (
            "main.gcl",
            "import p.lib::{ A, B };\n\
             include p.lib() as l;\n\
             node z: Length = match @l::u { A(a: x) => x, B(b: x) => x };\n",
        ),
    ]);
    assert_eq!(si_value(&eval(&main), "z"), 1.0);

    // The same holds in a DAG that is itself included.
    let (_dir, main) = package(&[
        ("lib.gcl", RECORD_LIB),
        (
            "middle.gcl",
            "include p.lib() as a;\n\
             pub node total: Length = sum(@a::out.x);\n",
        ),
        (
            "main.gcl",
            "include p.middle()::{ total };\n\
             node z: Length = @total;\n",
        ),
    ]);
    assert_eq!(si_value(&eval(&main), "z"), 3.0);
}

/// Another file's private inline DAG is rejected whether a file root or a
/// DAG body includes it.
#[test]
fn private_inline_dag_of_another_file_is_not_includable() {
    let other = "dag hidden {\n    pub node x: Dimensionless = 1.0;\n}\n";
    for main in [
        "include p.other.hidden()::{ x };\n",
        "pub dag wrapper {\n    include p.other.hidden()::{ x };\n    pub node y: Dimensionless = @x;\n}\ninclude wrapper()::{ y };\n",
    ] {
        let (_dir, root) = package(&[("other.gcl", other), ("main.gcl", main)]);
        let error = eval_error(&root);
        assert!(
            matches!(
                &error,
                CompileError::Eval(error) if matches!(
                    &error.error,
                    graphcal_compiler::semantic_error::SemanticError::Located(diagnostic)
                        if matches!(
                            diagnostic.kind,
                            graphcal_compiler::semantic_error::SemanticErrorKind::Visibility(
                                graphcal_compiler::semantic_error::visibility::VisibilityError::ImportPrivateItem { .. }
                            )
                        )
                )
            ),
            "{main}: {error:?}"
        );
    }
}

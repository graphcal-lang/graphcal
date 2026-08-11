//! Regressions from the Chunk 6 loader/orchestration review.
//!
//! Each ignored test describes the intended behavior before its implementing
//! phase lands. Later stack branches remove the corresponding `#[ignore]`.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{
    CompileError, compile_and_eval, compile_and_eval_named, compile_and_eval_project,
};
use graphcal_eval::loader::load_project;
use graphcal_io::RealFileSystem;

#[test]
fn recursive_algebraic_values_evaluate_after_checking() {
    let result = compile_and_eval(
        r"
type List { Nil, Cons(head: Int, tail: List) }
node empty: List = Nil;
node one: List = Cons(head: 1, tail: @empty);
",
    );

    assert!(
        result.is_ok(),
        "recursive algebraic values should evaluate: {result:?}"
    );
}

#[test]
fn nested_inline_module_paths_preserve_every_segment() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/mission");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"mission\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("lib.gcl"),
        r"
pub dag left {
    pub dag shared {
        pub const node value: Dimensionless = 1.0;
    }
}
pub dag right {
    pub dag shared {
        pub const node value: Dimensionless = 2.0;
    }
}
",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(
        &root,
        r"
import mission.lib.left.shared.{ value as left };
import mission.lib.right.shared.{ value as right };
node total: Dimensionless = @left + @right;
",
    )
    .unwrap();

    let result = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default());
    let result = result.unwrap_or_else(|error| panic!("nested module import failed: {error:?}"));
    let total = result
        .nodes
        .iter()
        .find(|(name, _)| name.to_string() == "total")
        .expect("total output")
        .1
        .as_ref()
        .expect("total value")
        .si_value()
        .expect("dimensionless total");
    assert!((total - 3.0).abs() < f64::EPSILON);
}

#[test]
fn module_resolver_rejects_recursive_include_expansion() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("main.gcl");
    std::fs::write(
        &root,
        r"
dag recurse {
    include recurse() as again;
}
",
    )
    .unwrap();

    let project = load_project(&root, None, &RealFileSystem::default()).unwrap();
    let error = project
        .build_module_resolver()
        .expect_err("recursive includes must not produce a module resolver");
    assert!(
        error.to_string().contains("recursive") || error.to_string().contains("cycle"),
        "unexpected recursion diagnostic: {error:?}"
    );
}

#[test]
fn module_resolver_rejects_mutually_recursive_includes() {
    let project = graphcal_eval::loader::LoadedProject::from_source(
        r"
dag first { include second() as next; }
dag second { include first() as next; }
",
        "main.gcl",
    )
    .unwrap();

    assert!(matches!(
        project.build_module_resolver(),
        Err(graphcal_eval::loader::ModuleResolverBuildError::RecursiveIncludeExpansion { .. })
    ));
}

#[test]
fn module_resolver_builds_deep_acyclic_include_chains_iteratively() {
    const DEPTH: usize = 128;

    let source = (0..DEPTH)
        .map(|index| {
            if index + 1 == DEPTH {
                format!("dag d{index} {{ pub node value: Dimensionless = 1.0; }}\n")
            } else {
                format!("dag d{index} {{ include d{}() as next; }}\n", index + 1)
            }
        })
        .collect::<String>();
    let project = graphcal_eval::loader::LoadedProject::from_source(&source, "main.gcl").unwrap();

    project
        .build_module_resolver()
        .expect("deep acyclic include expansion should be stack-safe");
}

#[test]
fn inline_self_import_rejects_assertions_like_cross_file_imports() {
    let error = compile_and_eval_named(
        r"
pub assert okay = true;
dag calculation {
    import self.{ okay };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
        "self.gcl",
    )
    .expect_err("assertions are not pure module import items");

    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::ImportAssertionItem { .. })
    ));
}

#[test]
fn inline_self_import_rejects_plots_like_cross_file_imports() {
    let error = compile_and_eval_named(
        r"
pub plot chart = { mark: point, encode: { x: 1.0 } };
dag calculation {
    import self.{ chart };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
        "self.gcl",
    )
    .expect_err("plots are not pure module import items");

    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::ImportPlotItem { .. })
    ));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PureImportOutcome {
    Success,
    RuntimeRejected,
    AssertionRejected,
    VisualizationRejected,
}

struct PureImportParityCase {
    role: &'static str,
    self_source: &'static str,
    library_source: &'static str,
    importer_source: &'static str,
    expected: PureImportOutcome,
}

fn cross_file_import_outcome(library_source: &str, importer_source: &str) -> PureImportOutcome {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/parity");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"parity\"\n",
    )
    .unwrap();
    std::fs::write(package.join("lib.gcl"), library_source).unwrap();
    let root = package.join("main.gcl");
    std::fs::write(&root, importer_source).unwrap();
    pure_import_outcome(compile_and_eval_project(
        &root,
        &HashMap::new(),
        None,
        &RealFileSystem::default(),
    ))
}

fn pure_import_outcome(
    result: Result<graphcal_eval::eval::EvalResult, CompileError>,
) -> PureImportOutcome {
    match result {
        Ok(_) => PureImportOutcome::Success,
        Err(CompileError::Eval(GraphcalError::ImportRuntimeItem { .. })) => {
            PureImportOutcome::RuntimeRejected
        }
        Err(CompileError::Eval(GraphcalError::ImportAssertionItem { .. })) => {
            PureImportOutcome::AssertionRejected
        }
        Err(CompileError::Eval(GraphcalError::ImportPlotItem { .. })) => {
            PureImportOutcome::VisualizationRejected
        }
        Err(other) => panic!("unexpected pure-import result: {other:?}"),
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "table-driven parity covers seven roles with complete source programs"
)]
fn inline_self_imports_match_cross_file_pure_import_policy() {
    let cases = [
        PureImportParityCase {
            role: "constant",
            self_source: r"
pub const node scale: Dimensionless = 2.0;
dag calculation {
    import self.{ scale };
    pub node out: Dimensionless = @scale;
}
node result: Dimensionless = @calculation().out;
",
            library_source: "pub const node scale: Dimensionless = 2.0;\n",
            importer_source: r"
import parity.lib.{ scale };
node result: Dimensionless = @scale;
",
            expected: PureImportOutcome::Success,
        },
        PureImportParityCase {
            role: "param",
            self_source: r"
param source: Dimensionless = 2.0;
dag calculation {
    import self.{ source };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
            library_source: "param source: Dimensionless = 2.0;\n",
            importer_source: r"
import parity.lib.{ source };
node result: Dimensionless = 1.0;
",
            expected: PureImportOutcome::RuntimeRejected,
        },
        PureImportParityCase {
            role: "node",
            self_source: r"
pub node source: Dimensionless = 2.0;
dag calculation {
    import self.{ source };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
            library_source: "pub node source: Dimensionless = 2.0;\n",
            importer_source: r"
import parity.lib.{ source };
node result: Dimensionless = 1.0;
",
            expected: PureImportOutcome::RuntimeRejected,
        },
        PureImportParityCase {
            role: "assertion",
            self_source: r"
pub assert okay = true;
dag calculation {
    import self.{ okay };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
            library_source: "pub assert okay = true;\n",
            importer_source: r"
import parity.lib.{ okay };
node result: Dimensionless = 1.0;
",
            expected: PureImportOutcome::AssertionRejected,
        },
        PureImportParityCase {
            role: "plot",
            self_source: r"
pub plot chart = { mark: point, encode: { x: 1.0 } };
dag calculation {
    import self.{ chart };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
            library_source: "pub plot chart = { mark: point, encode: { x: 1.0 } };\n",
            importer_source: r"
import parity.lib.{ chart };
node result: Dimensionless = 1.0;
",
            expected: PureImportOutcome::VisualizationRejected,
        },
        PureImportParityCase {
            role: "constructor",
            self_source: r"
pub type Choice { Pick }
dag calculation {
    import self.{ type Choice, Pick };
    pub node out: Choice = Pick;
}
node result: Choice = @calculation().out;
",
            library_source: "pub type Choice { Pick }\n",
            importer_source: r"
import parity.lib.{ type Choice, Pick };
node result: Choice = Pick;
",
            expected: PureImportOutcome::Success,
        },
        PureImportParityCase {
            role: "DAG",
            self_source: r"
pub dag helper { pub node out: Dimensionless = 2.0; }
dag calculation {
    import self.{ helper };
    pub node out: Dimensionless = @helper().out;
}
node result: Dimensionless = @calculation().out;
",
            library_source: "pub dag helper { pub node out: Dimensionless = 2.0; }\n",
            importer_source: r"
import parity.lib.{ helper };
node result: Dimensionless = @helper().out;
",
            expected: PureImportOutcome::Success,
        },
    ];

    for case in cases {
        let self_outcome =
            pure_import_outcome(compile_and_eval_named(case.self_source, "self.gcl"));
        let cross_outcome = cross_file_import_outcome(case.library_source, case.importer_source);
        assert_eq!(self_outcome, case.expected, "self-import {}", case.role);
        assert_eq!(
            cross_outcome, case.expected,
            "cross-file import {}",
            case.role
        );
        assert_eq!(
            self_outcome, cross_outcome,
            "policy drift for {}",
            case.role
        );
    }
}

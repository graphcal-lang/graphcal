//! Regressions from the Chunk 6 loader/orchestration review.
//!
//! Each ignored test describes the intended behavior before its implementing
//! phase lands. Later stack branches remove the corresponding `#[ignore]`.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{CompileError, compile_and_eval, compile_and_eval_project};
use graphcal_eval::loader::load_project;
use graphcal_io::RealFileSystem;

#[test]
#[ignore = "fixed by #1257 in Phase 3"]
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
#[ignore = "fixed by #1256 in Phase 1"]
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
#[ignore = "fixed by #1255 in Phase 1"]
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
#[ignore = "fixed by #1264 in Phase 4"]
fn inline_self_import_rejects_assertions_like_cross_file_imports() {
    let error = compile_and_eval(
        r"
pub assert okay = true;
dag calculation {
    import self.{ okay };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
    )
    .expect_err("assertions are not pure module import items");

    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::ImportAssertionItem { .. })
    ));
}

#[test]
#[ignore = "fixed by #1264 in Phase 4"]
fn inline_self_import_rejects_plots_like_cross_file_imports() {
    let error = compile_and_eval(
        r"
pub plot chart = { mark: point, encode: { x: 1.0 } };
dag calculation {
    import self.{ chart };
    pub node out: Dimensionless = 1.0;
}
node result: Dimensionless = @calculation().out;
",
    )
    .expect_err("plots are not pure module import items");

    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::ImportPlotItem { .. })
    ));
}

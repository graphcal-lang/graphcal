//! Phase 1 regression tests from `.local/2026-08-05_code-review-chunk-3.md`.
#![cfg(test)]

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{
    CompileError, compile_and_eval, compile_and_eval_project,
};
use graphcal_io::RealFileSystem;

fn compile_graphcal_error(source: &str) -> GraphcalError {
    match compile_and_eval(source).unwrap_err() {
        CompileError::Eval(error) => error,
        other => panic!("expected semantic error, got {other:?}"),
    }
}

#[test]
fn sink_graph_references_are_lowered_strictly() {
    let sources = [
        r#"
plot p = {
    mark: line,
    encode: { x: @missing },
};
"#,
        r#"
plot p = {
    mark: line { stroke_width: @missing },
    encode: { x: 1.0 },
};
"#,
        r#"
plot p = {
    mark: line,
    encode: { x: 1.0 },
    width: @missing,
};
"#,
        r#"
plot p = { mark: line, encode: { x: 1.0 } };
figure f = { plots: [p], title: @missing };
"#,
        r#"
plot p = { mark: line, encode: { x: 1.0 } };
layer l = { plots: [p], title: @missing };
"#,
    ];

    for source in sources {
        assert!(
            matches!(compile_graphcal_error(source), GraphcalError::UnknownGraphRef { .. }),
            "sink accepted an unresolved graph reference:\n{source}"
        );
    }
}

#[test]
fn sink_constructor_function_and_unit_names_are_lowered_strictly() {
    for source in [
        "plot p = { mark: line, encode: { x: Missing(value: 1.0) } };",
        "plot p = { mark: line, encode: { x: missing(1.0) } };",
        "plot p = { mark: line, encode: { x: 1.0 missing_unit } };",
    ] {
        assert!(
            compile_and_eval(source).is_err(),
            "sink accepted an unresolved semantic name: {source}"
        );
    }
}

#[test]
fn same_span_dag_calls_from_distinct_files_keep_their_hir_targets() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/calls");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"calls\"\n",
    )
    .unwrap();

    let library = r#"
dag helper {
    param x: Dimensionless;
    pub node out: Dimensionless = @x + 1.0;
}
param input: Dimensionless;
pub node result: Dimensionless = @helper(x: @input).out;
"#;
    std::fs::write(package.join("a.gcl"), library).unwrap();
    std::fs::write(package.join("b.gcl"), library).unwrap();
    let root = package.join("main.gcl");
    std::fs::write(
        &root,
        r#"
include calls.a(input: 1.0) as a;
include calls.b(input: 2.0) as b;
node total: Dimensionless = @a.result + @b.result;
"#,
    )
    .unwrap();

    let result = compile_and_eval_project(
        &root,
        &HashMap::new(),
        None,
        &RealFileSystem::default(),
    )
    .unwrap_or_else(|error| panic!("same-layout DAG calls must compile: {error:?}"));
    let total = result
        .nodes
        .iter()
        .find(|(name, _)| name.to_string() == "total")
        .unwrap()
        .1
        .as_ref()
        .unwrap()
        .si_value()
        .unwrap();
    assert!((total - 5.0).abs() < f64::EPSILON);
}

#[test]
fn dag_call_in_plot_property_uses_hir_directly() {
    let result = compile_and_eval(
        r#"
dag chart_config {
    pub node width: Dimensionless = 320.0;
}
plot p = {
    mark: line,
    encode: { x: 1.0 },
    width: @chart_config().width,
};
"#,
    );
    assert!(result.is_ok(), "plot DAG call failed: {result:?}");
}

#[test]
fn source_nested_dag_calls_are_runtime_includes_in_compile_time_contexts() {
    for source in [
        r#"
dag helper {
    pub const node fixed: Dimensionless = 1.0;
    pub node out: Dimensionless = @fixed;
}
const node value: Dimensionless = @helper().out;
"#,
        r#"
dag helper {
    param value: Dimensionless = 1.0;
}
const node value: Dimensionless = @helper().value;
"#,
        r#"
dag helper {
    pub node min_value: Dimensionless = 0.0;
}
param value: Dimensionless(min: @helper().min_value) = 1.0;
"#,
        r#"
dag helper {
    pub node min_value: Dimensionless = 0.0;
}
type Box { Box(value: Dimensionless(min: @helper().min_value)) }
"#,
    ] {
        assert!(
            matches!(
                compile_graphcal_error(source),
                GraphcalError::DagCallInCompileTime { .. }
            ),
            "compile-time DAG call was accepted:\n{source}"
        );
    }
}

#[test]
fn file_root_dag_call_is_also_runtime_only() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/call_phase");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"call_phase\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("library.gcl"),
        "pub node out: Dimensionless = 1.0;\n",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(
        &root,
        "import call_phase.library as library;\nconst node value: Dimensionless = @library().out;\n",
    )
    .unwrap();

    let error = compile_and_eval_project(
        &root,
        &HashMap::new(),
        None,
        &RealFileSystem::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::DagCallInCompileTime { .. })
    ));
}

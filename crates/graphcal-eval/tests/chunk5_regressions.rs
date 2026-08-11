//! Regression tests for `.local/2026-08-08_chunk-5-code-review.md`.

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_eval::eval::{CompileError, EvalResult, compile_and_eval, compile_and_eval_project};
use graphcal_io::RealFileSystem;

fn assert_node_failed(result: &EvalResult, name: &str) {
    let value = result
        .nodes
        .iter()
        .find(|(candidate, _)| candidate.to_string() == name);
    let Some((_, value)) = value else {
        assert!(value.is_some(), "node `{name}` is missing");
        return;
    };
    assert!(value.is_err(), "node `{name}` unexpectedly succeeded");
}

#[test]
fn callable_and_include_reject_bound_param_constraint_violations() {
    let result = compile_and_eval(
        r"
dag bounded {
    param x: Dimensionless(max: 1.0);
    pub node result: Dimensionless = @x;
}
include bounded(x: 2.0).{ result as included };
node called: Dimensionless = @bounded(x: 2.0).result;
",
    )
    .unwrap();

    assert_node_failed(&result, "included");
    assert_node_failed(&result, "called");
}

#[test]
fn callable_and_include_reject_defaulted_param_constraint_violations() {
    let result = compile_and_eval(
        r"
dag bounded {
    param x: Dimensionless(max: 1.0) = 2.0;
    pub node result: Dimensionless = @x;
}
include bounded().{ result as included };
node called: Dimensionless = @bounded().result;
",
    )
    .unwrap();

    assert_node_failed(&result, "included");
    assert_node_failed(&result, "called");
}

#[test]
fn callable_and_include_reject_node_constraint_violations() {
    let result = compile_and_eval(
        r"
dag bounded {
    param x: Dimensionless;
    pub node result: Dimensionless(max: 1.0) = @x;
}
include bounded(x: 2.0).{ result as included };
node called: Dimensionless = @bounded(x: 2.0).result;
",
    )
    .unwrap();

    assert_node_failed(&result, "included");
    assert_node_failed(&result, "called");
}

#[test]
fn callable_const_constraints_are_checked_before_runtime() {
    let error = compile_and_eval(
        r"
dag bounded {
    const node LIMIT: Dimensionless(max: 1.0) = 2.0;
    pub node result: Dimensionless = @LIMIT;
}
node called: Dimensionless = @bounded().result;
",
    )
    .unwrap_err();

    assert!(matches!(
        error,
        CompileError::Eval(GraphcalError::DomainViolation { .. })
    ));
}

#[test]
fn callable_and_include_apply_indexed_constraints_element_wise() {
    let result = compile_and_eval(
        r"
dag bounded {
    param xs: Dimensionless(max: 1.0)[Fin(2)];
    pub node result: Dimensionless[Fin(2)] = @xs;
}
include bounded(xs: for i: Fin(2) { 2.0 }).{ result as included };
node called: Dimensionless[Fin(2)] = @bounded(xs: for i: Fin(2) { 2.0 }).result;
",
    )
    .unwrap();

    assert_node_failed(&result, "included");
    assert_node_failed(&result, "called");
}

#[test]
fn selectively_imported_field_constraint_uses_its_defining_const_pool() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/constraints");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"constraints\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("schema.gcl"),
        r"
const node MIN: Length = 1.0 m;
pub const node EXPORTED: Dimensionless = 1.0;
pub type Item { Item(value: Length(min: @MIN)) }
",
    )
    .unwrap();

    let root = package.join("main.gcl");
    std::fs::write(
        &root,
        r"
import constraints.schema.{ type Item, Item };
node good: Item = Item(value: 2.0 m);
",
    )
    .unwrap();
    compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default()).unwrap();

    std::fs::write(
        &root,
        r"
import constraints.schema.{ EXPORTED };
node good: Dimensionless = @EXPORTED;
",
    )
    .unwrap();
    compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default()).unwrap();
}

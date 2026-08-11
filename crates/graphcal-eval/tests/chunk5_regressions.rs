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

#[test]
fn datetime_numeric_constructors_and_arithmetic_reject_saturation() {
    let result = compile_and_eval(
        r#"
node now: Datetime = datetime("2024-01-01T00:00:00Z");
node far_add: Datetime = @now + 1.0e300 s;
node far_subtract: Datetime = @now - 1.0e300 s;
node far_unix: Datetime = from_unix(1.0e300);
node far_jd: Datetime = from_jd(1.0e300);
node far_mjd: Datetime = from_mjd(-1.0e300);
"#,
    )
    .unwrap();

    for name in ["far_add", "far_subtract", "far_unix", "far_jd", "far_mjd"] {
        assert_node_failed(&result, name);
    }
}

#[test]
fn overflowing_display_conversion_is_a_node_and_plot_error() {
    let result = compile_and_eval(
        r"
const unit tiny: Length = 1.0e-300 m;
node huge: Length = 1.0e300 m -> tiny;
plot p = { mark: point, encode: { x: 1.0e300 m -> tiny } };
",
    )
    .unwrap();

    assert_node_failed(&result, "huge");
    assert!(result.plots.is_empty());
    assert_eq!(result.plot_errors.len(), 1);
    assert!(result.plot_errors[0].message.contains("non-finite"));
}

#[test]
fn mean_uses_a_scaled_accumulator() {
    let result = compile_and_eval(
        r"
index Sample = { A, B };
node values: Dimensionless[Sample] = {
    Sample.A: 1.0e308,
    Sample.B: 1.0e308,
};
node average: Dimensionless = mean(@values);
",
    )
    .unwrap();
    let (_, average) = result
        .nodes
        .iter()
        .find(|(name, _)| name.to_string() == "average")
        .unwrap();
    let average = average.as_ref().unwrap().si_value().unwrap();
    assert!((average / 1.0e308 - 1.0).abs() < f64::EPSILON);
}

#[test]
fn plot_rejects_ints_that_binary64_cannot_represent_exactly() {
    let result = compile_and_eval(
        r"
plot p = {
    mark: point,
    encode: { x: 9007199254740993 },
};
",
    )
    .unwrap();

    assert!(result.plots.is_empty());
    assert_eq!(result.plot_errors.len(), 1);
    assert!(result.plot_errors[0].message.contains("to_float"));
}

#[test]
fn oversized_composite_shapes_are_rejected_before_evaluation() {
    let error = compile_and_eval(
        r"
node grid: Dimensionless[Fin(1000000), Fin(1000000)] =
    for row: Fin(1000000), column: Fin(1000000) { 1.0 };
",
    )
    .unwrap_err();

    assert!(matches!(
        error,
        graphcal_eval::eval::CompileError::Eval(
            graphcal_compiler::registry::error::GraphcalError::MaterializedShapeTooLarge {
                maximum: 1_000_000,
                ..
            }
        )
    ));
}

#[test]
fn nested_concrete_generic_field_shapes_obey_the_total_limit() {
    let error = compile_and_eval(
        r"
pub type Matrix<N: Nat> {
    Matrix(values: Dimensionless[Fin(N), Fin(N)])
}
param matrix: Matrix<1000000>;
",
    )
    .unwrap_err();

    assert!(matches!(
        error,
        graphcal_eval::eval::CompileError::Eval(
            graphcal_compiler::registry::error::GraphcalError::MaterializedShapeTooLarge {
                maximum: 1_000_000,
                ..
            }
        )
    ));
}

#[test]
fn instantiated_index_bindings_obey_the_total_shape_limit() {
    let error = compile_and_eval(
        r"
dag grid {
    pub(bind) index Row;
    pub(bind) index Column;
    pub node values: Dimensionless[Row, Column] =
        for row: Row, column: Column { 1.0 };
}
include grid(Row: Fin(1000000), Column: Fin(1000000)) as giant;
node unreachable: Dimensionless = count(@giant.values);
",
    )
    .unwrap_err();

    assert!(matches!(
        error,
        graphcal_eval::eval::CompileError::Eval(
            graphcal_compiler::registry::error::GraphcalError::MaterializedShapeTooLarge {
                maximum: 1_000_000,
                ..
            }
        )
    ));
}

#[test]
fn cubic_kernel_work_is_rejected_before_the_kernel_runs() {
    let result = compile_and_eval(
        r"
node lhs: Dimensionless[Fin(216), Fin(216)] =
    for row: Fin(216), column: Fin(216) { 1.0 };
node rhs: Dimensionless[Fin(216), Fin(216)] =
    for row: Fin(216), column: Fin(216) { 1.0 };
node product: Dimensionless[Fin(216), Fin(216)] = matmul(@lhs, @rhs);
",
    )
    .unwrap();

    let (_, product) = result
        .nodes
        .iter()
        .find(|(name, _)| name.to_string() == "product")
        .unwrap();
    let error = product.as_ref().unwrap_err().to_string();
    assert!(error.contains("evaluation budget"), "{error}");
}

#[test]
fn datetime_plot_data_preserves_nanoseconds() {
    let result = compile_and_eval(
        r#"
node instant: Datetime = datetime("2026-01-01T00:00:00.000000001Z");
plot p = {
    mark: point,
    encode: { x: @instant },
};
"#,
    )
    .unwrap();

    let plot = result.plots.first().unwrap();
    let (_, values) = plot.encodings.first().unwrap();
    let graphcal_eval::eval::PlotFieldValue::Datetimes(values) = values else {
        panic!("expected datetime plot data, got {values:?}");
    };
    assert_eq!(values, &["2026-01-01T00:00:00.000000001Z"]);
}

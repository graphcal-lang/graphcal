//! Regression tests for `.local/2026-08-08_chunk-5-code-review.md`.

use std::collections::HashMap;
use std::fmt::Write as _;

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

fn successful_node<'a>(
    result: &'a EvalResult,
    name: &str,
) -> Result<&'a graphcal_eval::eval::Value, String> {
    result
        .nodes
        .iter()
        .find(|(candidate, _)| candidate.to_string() == name)
        .ok_or_else(|| format!("node `{name}` is missing"))?
        .1
        .as_ref()
        .map_err(|error| format!("node `{name}` failed: {error}"))
}

fn every_quantity_uses_km(value: &graphcal_eval::eval::Value) -> Result<(), String> {
    match value {
        graphcal_eval::eval::Value::Quantity { display_unit, .. } => {
            let label = display_unit.as_ref().map(|unit| unit.label.as_str());
            (label == Some("km"))
                .then_some(())
                .ok_or_else(|| format!("expected km presentation, got {label:?}"))
        }
        graphcal_eval::eval::Value::Indexed { entries, .. } => {
            entries.values().try_for_each(every_quantity_uses_km)
        }
        other => Err(format!(
            "expected quantity presentation tree, got {other:?}"
        )),
    }
}

#[test]
fn presentation_provenance_has_no_read_chain_depth_limit() {
    let mut source = "node n0: Length = 1000.0 m -> km;\n".to_string();
    for index in 1..=100 {
        writeln!(source, "node n{index}: Length = @n{};", index - 1).unwrap();
    }

    let result = compile_and_eval(&source).unwrap();
    every_quantity_uses_km(successful_node(&result, "n100").unwrap()).unwrap();
}

#[test]
fn dag_call_output_keeps_definition_owned_presentation() {
    let result = compile_and_eval(
        r"
dag units {
    pub node distance: Length = 1000.0 m -> km;
}
node called: Length = @units().distance;
",
    )
    .unwrap();

    every_quantity_uses_km(successful_node(&result, "called").unwrap()).unwrap();
}

fn dynamic_call_display_values(value: &graphcal_eval::eval::Value) -> Result<Vec<f64>, String> {
    let graphcal_eval::eval::Value::Indexed { entries, .. } = value else {
        return Err(format!(
            "expected indexed dynamic-call output, got {value:?}"
        ));
    };
    entries
        .values()
        .map(|entry| {
            let graphcal_eval::eval::Value::Quantity {
                si_value,
                display_unit,
                ..
            } = entry
            else {
                return Err(format!("expected quantity call output, got {entry:?}"));
            };
            let label = display_unit.as_ref().map(|unit| unit.label.as_str());
            if label != Some("DynamicM") {
                return Err(format!("expected DynamicM presentation, got {label:?}"));
            }
            graphcal_eval::eval::quantity_display_value(*si_value, display_unit.as_ref())
                .map_err(|error| error.to_string())
        })
        .collect()
}

#[test]
fn repeated_dag_calls_resolve_dynamic_units_in_each_call_environment() {
    let result = compile_and_eval(
        r"
index Rate = { Low, High };
dag units {
    param rate: Dimensionless;
    unit DynamicM: Length = (@rate) m;
    pub node distance: Length = 1.0 DynamicM;
}
node values: Length[Rate] = for rate: Rate {
    @units(rate: match rate {
        Rate.Low => 2.0,
        Rate.High => 3.0,
    }).distance
};
",
    )
    .unwrap();

    let displayed =
        dynamic_call_display_values(successful_node(&result, "values").unwrap()).unwrap();
    assert_eq!(displayed, [1.0, 1.0]);
}

#[test]
fn multi_axis_comprehension_keeps_nested_leaf_presentation() {
    let result = compile_and_eval(
        r"
index Row = { A, B };
index Column = { X, Y };
node grid: Length[Row, Column] =
    for row: Row, column: Column { 1.0 km };
",
    )
    .unwrap();

    every_quantity_uses_km(successful_node(&result, "grid").unwrap()).unwrap();
}

#[test]
fn equivalent_plot_operands_use_the_same_checked_dimension() {
    let result = compile_and_eval(
        r"
node distance: Length = 1.0 m;
plot left_first = {
    mark: point,
    encode: { x: @distance * 2.0 },
};
plot right_first = {
    mark: point,
    encode: { x: 2.0 * @distance },
};
",
    )
    .unwrap();

    assert_eq!(result.plots.len(), 2);
    let dimensions = result
        .plots
        .iter()
        .map(|plot| {
            plot.encoding_meta
                .first()
                .and_then(|(_, metadata)| metadata.dimension_label.as_deref())
        })
        .collect::<Vec<_>>();
    assert_eq!(dimensions[0], dimensions[1]);
    assert_eq!(dimensions[0], Some("Length"));
}

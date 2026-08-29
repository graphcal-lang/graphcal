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
include bounded(x: 2.0)::{ result as included };
node called: Dimensionless = @bounded(x: 2.0)::result;
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
include bounded()::{ result as included };
node called: Dimensionless = @bounded()::result;
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
include bounded(x: 2.0)::{ result as included };
node called: Dimensionless = @bounded(x: 2.0)::result;
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
node called: Dimensionless = @bounded()::result;
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
include bounded(xs: for i: Fin(2) { 2.0 })::{ result as included };
node called: Dimensionless[Fin(2)] = @bounded(xs: for i: Fin(2) { 2.0 })::result;
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
import constraints.schema::{ type Item, Item };
node good: Item = Item(value: 2.0 m);
",
    )
    .unwrap();
    compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default()).unwrap();

    std::fs::write(
        &root,
        r"
import constraints.schema::{ EXPORTED };
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
    Sample#A: 1.0e308,
    Sample#B: 1.0e308,
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
include grid(index Row: Fin(1000000), index Column: Fin(1000000)) as giant;
node unreachable: Dimensionless = count(@giant::values);
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
node matrix_product: Dimensionless[Fin(216), Fin(216)] = matmul(@lhs, @rhs);
",
    )
    .unwrap();

    let (_, matrix_product) = result
        .nodes
        .iter()
        .find(|(name, _)| name.to_string() == "matrix_product")
        .unwrap();
    let error = matrix_product.as_ref().unwrap_err().to_string();
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

fn quantity_display_labels(value: &graphcal_eval::eval::Value) -> Result<Vec<String>, String> {
    match value {
        graphcal_eval::eval::Value::Quantity { display_unit, .. } => display_unit
            .as_ref()
            .map(|unit| vec![unit.label.clone()])
            .ok_or_else(|| "quantity has no display presentation".to_string()),
        graphcal_eval::eval::Value::Indexed { entries, .. } => entries
            .values()
            .map(quantity_display_labels)
            .collect::<Result<Vec<_>, _>>()
            .map(|nested| nested.into_iter().flatten().collect()),
        other => Err(format!(
            "expected quantity presentation tree, got {other:?}"
        )),
    }
}

#[test]
fn semantic_struct_equality_covers_constructors_indexed_and_coordinate_fields() {
    let result = compile_and_eval(
        r"
index Sample = { A, B };
index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);
type Choice { A, B }
type IndexedHolder { IndexedHolder(values: Dimensionless[Sample]) }
type KeyHolder { KeyHolder(key: Key<TimeStep>) }
node constructors_differ: Bool = A != B;
node indexed_equal: Bool =
    IndexedHolder(values: { Sample#A: 1.0, Sample#B: 2.0 }) ==
    IndexedHolder(values: { Sample#A: 1.0, Sample#B: 2.0 });
node coordinate_equal: Bool =
    KeyHolder(key: nearest_key(TimeStep, 1.0 s)) ==
    KeyHolder(key: nearest_key(TimeStep, 1.0 s));
",
    )
    .unwrap();

    for name in ["constructors_differ", "indexed_equal", "coordinate_equal"] {
        assert!(matches!(
            successful_node(&result, name).unwrap(),
            graphcal_eval::eval::Value::Bool(true)
        ));
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
node called: Length = @units()::distance;
",
    )
    .unwrap();

    every_quantity_uses_km(successful_node(&result, "called").unwrap()).unwrap();
}

fn dynamic_call_display_values(value: &graphcal_eval::eval::Value) -> Result<Vec<f64>, String> {
    match value {
        graphcal_eval::eval::Value::Quantity {
            si_value,
            display_unit,
            ..
        } => {
            let label = display_unit.as_ref().map(|unit| unit.label.as_str());
            if label != Some("DynamicM") {
                return Err(format!("expected DynamicM presentation, got {label:?}"));
            }
            graphcal_eval::eval::quantity_display_value(*si_value, display_unit.as_ref())
                .map(|value| vec![value])
                .map_err(|error| error.to_string())
        }
        graphcal_eval::eval::Value::Struct { fields, .. } => fields
            .values()
            .map(dynamic_call_display_values)
            .collect::<Result<Vec<_>, _>>()
            .map(|nested| nested.into_iter().flatten().collect()),
        graphcal_eval::eval::Value::Indexed { entries, .. } => entries
            .values()
            .map(dynamic_call_display_values)
            .collect::<Result<Vec<_>, _>>()
            .map(|nested| nested.into_iter().flatten().collect()),
        other => Err(format!(
            "expected dynamic-call presentation tree, got {other:?}"
        )),
    }
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
        Rate#Low => 2.0,
        Rate#High => 3.0,
    })::distance
};
",
    )
    .unwrap();

    let displayed =
        dynamic_call_display_values(successful_node(&result, "values").unwrap()).unwrap();
    assert_eq!(displayed, [1.0, 1.0]);
}

#[test]
fn reused_dag_call_result_keeps_one_presentation_invocation() {
    let result = compile_and_eval(
        r"
dag units {
    param rate: Dimensionless;
    unit DynamicM: Length = (@rate) m;
    pub node distance: Length = 1.0 DynamicM;
}
node original: Length = @units(rate: 2.0)::distance;
node copies: Length[Fin(2)] = for i: Fin(2) { @original };
",
    )
    .unwrap();

    let displayed =
        dynamic_call_display_values(successful_node(&result, "copies").unwrap()).unwrap();
    assert_eq!(displayed, [1.0, 1.0]);
}

#[test]
fn reordered_dag_call_results_keep_their_presentation_invocations() {
    let result = compile_and_eval(
        r"
index Rate = { Two, Three };
index Order = { First, Second };
dag units {
    param rate: Dimensionless;
    unit DynamicM: Length = (@rate) m;
    pub node distance: Length = 1.0 DynamicM;
}
node rates: Dimensionless[Rate] = {
    Rate#Two: 2.0,
    Rate#Three: 3.0,
};
node values: Length[Rate] = for rate: Rate {
    @units(rate: @rates[rate])::distance
};
node reversed: Length[Order] = {
    Order#First: @values[Rate#Three],
    Order#Second: @values[Rate#Two],
};
node authored_reverse: Length[Order] = {
    Order#Second: @values[Rate#Two],
    Order#First: @values[Rate#Three],
};
node selected: Length[Order] = for order: Order {
    match order {
        Order#First => @values[Rate#Three],
        Order#Second => @values[Rate#Two],
    }
};
plot reordered = {
    mark: point,
    encode: { x: @selected },
};
",
    )
    .unwrap();

    for node in ["reversed", "authored_reverse", "selected"] {
        let displayed =
            dynamic_call_display_values(successful_node(&result, node).unwrap()).unwrap();
        assert_eq!(displayed, [1.0, 1.0], "node `{node}`");
    }

    let plot = result.plots.first().expect("reordered plot must render");
    let (_, values) = plot.encodings.first().expect("plot must contain x data");
    let graphcal_eval::eval::PlotFieldValue::Numbers(values) = values else {
        panic!("expected numeric plot data, got {values:?}");
    };
    assert_eq!(values, &[1.0, 1.0]);
    assert_eq!(
        plot.encoding_meta
            .first()
            .and_then(|(_, metadata)| metadata.unit_label.as_deref()),
        Some("DynamicM")
    );
}

#[test]
fn repeated_and_structured_projections_keep_presentation_invocations() {
    let result = compile_and_eval(
        r"
index Rate = { Two, Three };
index Selection = { First, Second, Third };
type Pair { Pair(left: Length, right: Length) }
dag units {
    param rate: Dimensionless;
    unit DynamicM: Length = (@rate) m;
    pub node distance: Length = 1.0 DynamicM;
}
node rates: Dimensionless[Rate] = {
    Rate#Two: 2.0,
    Rate#Three: 3.0,
};
node values: Length[Rate] = for rate: Rate {
    @units(rate: @rates[rate])::distance
};
node selected: Length[Selection] = {
    Selection#First: @values[Rate#Three],
    Selection#Second: @values[Rate#Three],
    Selection#Third: @values[Rate#Two],
};
node packed: Pair = Pair(
    right: @values[Rate#Two],
    left: @values[Rate#Three],
);
node only_three: Length = @values[Rate#Three];
",
    )
    .unwrap();

    let selected =
        dynamic_call_display_values(successful_node(&result, "selected").unwrap()).unwrap();
    assert_eq!(selected, [1.0, 1.0, 1.0]);
    let packed = dynamic_call_display_values(successful_node(&result, "packed").unwrap()).unwrap();
    assert_eq!(packed, [1.0, 1.0]);
    let only_three =
        dynamic_call_display_values(successful_node(&result, "only_three").unwrap()).unwrap();
    assert_eq!(only_three, [1.0]);
}

#[test]
fn equal_si_values_keep_distinct_presentation_invocations() {
    let result = compile_and_eval(
        r"
index Rate = { Two, Three };
index Order = { First, Second };
dag units {
    param rate: Dimensionless;
    unit DynamicM: Length = (@rate) m;
    pub node fixed: Length = 6.0 m -> DynamicM;
}
node rates: Dimensionless[Rate] = {
    Rate#Two: 2.0,
    Rate#Three: 3.0,
};
node values: Length[Rate] = for rate: Rate {
    @units(rate: @rates[rate])::fixed
};
node reversed: Length[Order] = {
    Order#First: @values[Rate#Three],
    Order#Second: @values[Rate#Two],
};
",
    )
    .unwrap();

    let graphcal_eval::eval::Value::Indexed { entries, .. } =
        successful_node(&result, "values").unwrap()
    else {
        panic!("values must be indexed");
    };
    assert!(entries.values().all(|value| match value {
        graphcal_eval::eval::Value::Quantity { si_value, .. } => {
            (*si_value - 6.0).abs() < f64::EPSILON
        }
        _ => false,
    }));

    let displayed =
        dynamic_call_display_values(successful_node(&result, "reversed").unwrap()).unwrap();
    assert_eq!(displayed, [2.0, 3.0]);
}

#[test]
fn nested_dag_calls_keep_each_presentation_invocation() {
    let result = compile_and_eval(
        r"
index Rate = { Two, Three };
index Order = { First, Second };
dag inner {
    param rate: Dimensionless;
    unit DynamicM: Length = (@rate) m;
    pub node distance: Length = 1.0 DynamicM;
}
dag wrapper {
    param rate: Dimensionless;
    pub node distance: Length = @inner(rate: @rate)::distance;
}
node rates: Dimensionless[Rate] = {
    Rate#Two: 2.0,
    Rate#Three: 3.0,
};
node values: Length[Rate] = for rate: Rate {
    @wrapper(rate: @rates[rate])::distance
};
node reversed: Length[Order] = {
    Order#First: @values[Rate#Three],
    Order#Second: @values[Rate#Two],
};
",
    )
    .unwrap();

    let displayed =
        dynamic_call_display_values(successful_node(&result, "reversed").unwrap()).unwrap();
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
fn projected_index_binders_keep_definition_owned_presentation_selectors() {
    let result = compile_and_eval(
        r"
index Scenario = { Baseline };
index Model = { A, B };
index Distance = { Near, Far };
index Series = { First, Second };

node source: Length[Scenario, Model, Distance] =
    for scenario: Scenario, model: Model, distance: Distance {
        match model {
            Model#A => 1.0 m,
            Model#B => 0.001 km,
        }
    };

node consumer: Length[Series, Distance] =
    for series: Series, distance: Distance {
        match series {
            Series#First => @source[Scenario#Baseline, Model#A, distance],
            Series#Second => @source[Scenario#Baseline, Model#B, distance],
        }
    };
",
    )
    .unwrap();

    let labels = quantity_display_labels(successful_node(&result, "consumer").unwrap()).unwrap();
    assert_eq!(
        labels.iter().map(String::as_str).collect::<Vec<_>>(),
        ["m", "m", "km", "km"]
    );
}

#[test]
fn projected_index_expression_arguments_use_the_access_owner_locals() {
    let result = compile_and_eval(
        r"
index Scenario = { Baseline };
index Model = { A, B };
index Distance = { Near, Far };
index Series = { First, Second };

node source: Length[Scenario, Model, Distance] =
    for scenario: Scenario, model: Model, distance: Distance {
        match model {
            Model#A => 1.0 m,
            Model#B => 0.001 km,
        }
    };

node consumer: Length[Series, Distance] =
    for series: Series, distance: Distance {
        @source[
            Scenario#Baseline,
            match series {
                Series#First => Model#A,
                Series#Second => Model#B,
            },
            distance,
        ]
    };
",
    )
    .unwrap();

    let labels = quantity_display_labels(successful_node(&result, "consumer").unwrap()).unwrap();
    assert_eq!(
        labels.iter().map(String::as_str).collect::<Vec<_>>(),
        ["m", "m", "km", "km"]
    );
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

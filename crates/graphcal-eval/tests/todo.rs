#![cfg(test)]

use graphcal_eval::eval::{EvalResult, NodeUnavailable, ProjectCompiler, compile_and_eval};
use graphcal_eval::loader::LoadedProject;

fn result<'a>(
    evaluation: &'a EvalResult,
    name: &str,
) -> &'a Result<graphcal_eval::eval::Value, NodeUnavailable> {
    &evaluation
        .all
        .iter()
        .find(|(candidate, _, _)| candidate.to_string() == name)
        .unwrap_or_else(|| panic!("missing {name}; results: {:?}", evaluation.all))
        .1
}

fn check(source: &str) -> bool {
    let loaded = LoadedProject::from_source(source, "todo.gcl").unwrap();
    ProjectCompiler::new(&loaded).check().is_ok()
}

#[test]
fn blocked_assertions_are_not_expected_failures() {
    use graphcal_eval::eval::AssertResult;
    let evaluation = compile_and_eval(
        r"
node unfinished: Length = todo {};
#[expected_fail]
assert missing = if true { true } else { @unfinished > 0.0 m };
assert independent = true;
",
    )
    .unwrap();
    assert!(evaluation.is_incomplete());
    assert!(!evaluation.has_errors());
    assert!(matches!(
        evaluation.assertions[0].1,
        AssertResult::Blocked { .. }
    ));
    assert!(matches!(evaluation.assertions[1].1, AssertResult::Pass));
}

#[test]
fn successful_call_projection_still_reports_private_unfinished_nodes() {
    let evaluation = compile_and_eval(
        r"
dag model {
    node unfinished: Length = todo {};
    pub node known: Length = 1.0 m;
}
node known: Length = @model()::known;
",
    )
    .unwrap();
    assert!(result(&evaluation, "known").is_ok());
    assert!(evaluation.is_incomplete());
    assert!(!evaluation.has_errors());
    assert_eq!(evaluation.unfinished_calls.len(), 1);
}

#[test]
fn genuine_call_failures_remain_errors_when_later_incomplete_calls_succeed() {
    let evaluation = compile_and_eval(
        r"
dag model {
    param divisor: Dimensionless = 1.0;
    node unfinished: Length = todo {};
    node bad: Length = 1.0 m / @divisor;
    pub node known: Length = 2.0 m;
}
node known_from_failure: Length = @model(divisor: 0.0)::known;
node known_from_success: Length = @model(divisor: 1.0)::known;
",
    )
    .unwrap();
    assert!(result(&evaluation, "known_from_failure").is_err());
    assert!(result(&evaluation, "known_from_success").is_ok());
    assert!(evaluation.is_incomplete());
    assert!(evaluation.has_errors());
    assert_eq!(evaluation.unfinished_calls.len(), 1);
}

#[test]
fn private_call_only_blocking_does_not_block_an_independent_projection() {
    let evaluation = compile_and_eval(
        r"
        dag source { pub node unfinished: Length = todo {}; }
        dag consumer {
            node private_result: Length = @source()::unfinished;
            pub node available: Length = 2.0 m;
        }
        node available: Length = @consumer()::available;
    ",
    )
    .unwrap();
    assert!(result(&evaluation, "available").is_ok());
    assert!(evaluation.is_incomplete());
    assert!(!evaluation.has_errors());
    assert_eq!(evaluation.unfinished_calls.len(), 1);
}

#[test]
fn todo_remains_available_in_all_three_namespaces_and_as_a_dag_name() {
    let evaluation = compile_and_eval(
        r"
index todo = { First };
unit todo: Length = 1.0 m;
param todo: Length = 2.0 todo;
node missing: Length[todo] = todo { @todo };
dag library {
    dag todo { pub node known: Length = 1.0 m; }
}
node known: Length = @library.todo()::known;
",
    )
    .unwrap();
    assert!(result(&evaluation, "known").is_ok());
    assert!(matches!(
        result(&evaluation, "missing"),
        Err(NodeUnavailable::Todo { .. })
    ));
}

#[test]
fn static_call_blocking_respects_parameter_overrides_and_nested_calls() {
    let evaluation = compile_and_eval(
        r"
dag source { pub node missing: Length = todo {}; }
dag stage {
    param value: Length = @source()::missing;
    pub node output: Length = @value;
}
dag wrapper {
    pub node output: Length = if true { 1.0 m } else { @stage()::output };
}
node supplied: Length = @stage(value: 2.0 m)::output;
node blocked: Length = @wrapper()::output;
assert pending = if true { true } else { @source()::missing > 0.0 m };
",
    )
    .unwrap();
    assert!(
        result(&evaluation, "supplied").is_ok(),
        "{:?}",
        result(&evaluation, "supplied")
    );
    assert!(
        matches!(
            result(&evaluation, "blocked"),
            Err(NodeUnavailable::Blocked { .. })
        ),
        "{:?}",
        result(&evaluation, "blocked")
    );
    assert!(matches!(
        evaluation.assertions[0].1,
        graphcal_eval::eval::AssertResult::Blocked { .. }
    ));
    assert!(!evaluation.has_errors());
}

#[test]
fn blocking_includes_unselected_call_outputs() {
    let evaluation = compile_and_eval(
        r"
dag model { pub node missing: Length = todo {}; }
node blocked: Length = if true { 1.0 m } else { @model()::missing };
",
    )
    .unwrap();
    assert!(
        matches!(
            result(&evaluation, "blocked"),
            Err(NodeUnavailable::Blocked { .. })
        ),
        "{:?}",
        result(&evaluation, "blocked")
    );
}

#[test]
fn plots_and_compositions_are_blocked_without_becoming_internal_errors() {
    let evaluation = compile_and_eval(
        r"
node missing: Dimensionless = todo {};
plot curve = { mark: point, encode: { y: @missing } };
figure combined = { plots: [curve] };
layer overlay = { plots: [curve] };
",
    )
    .unwrap();
    assert!(evaluation.is_incomplete());
    assert!(!evaluation.has_errors());
    assert_eq!(evaluation.plot_errors.len(), 3);
    assert!(evaluation.plots.is_empty());
    assert!(evaluation.figures.is_empty());
    assert!(evaluation.layers.is_empty());
}

#[test]
fn graph_export_keeps_marker_edges_including_constants_without_inventing_a_value() {
    use graphcal_eval::graph_ir::{
        dot::{GraphView, render},
        project_tir,
    };
    let project = LoadedProject::from_source("param input: Length = 1.0 m; const node scale: Dimensionless = 2.0; node missing: Length = todo { @input, @input, @scale };", "todo.gcl").unwrap();
    let checked = ProjectCompiler::new(&project).check().unwrap();
    let dot = render(&project_tir(checked.tir()).unwrap(), GraphView::Flat);
    assert!(dot.contains("TODO"));
    assert_eq!(dot.matches(" -> ").count(), 2);
}

#[test]
fn rocket_checks_and_evaluates_independent_nodes() {
    let source = r"
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
const node g0: Acceleration = 9.80665 m/s^2;
node v_exhaust: Velocity = todo { @isp, @g0 };
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
";
    assert!(check(source));
    let evaluation = compile_and_eval(source).unwrap();
    assert!(matches!(
        result(&evaluation, "v_exhaust"),
        Err(NodeUnavailable::Todo { .. })
    ));
    assert!(matches!(
        result(&evaluation, "delta_v"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert!(
        (result(&evaluation, "mass_ratio")
            .as_ref()
            .unwrap()
            .si_value()
            .unwrap()
            - 10.0 / 3.0)
            .abs()
            < 1e-12
    );
}

#[test]
fn empty_interface_and_contextual_identifier_remain_distinct() {
    let evaluation = compile_and_eval(
        r"
param todo: Length = 1.0 m;
node unfinished: Length = todo {};
node copied: Length = @todo;
",
    )
    .unwrap();
    assert!(matches!(
        result(&evaluation, "unfinished"),
        Err(NodeUnavailable::Todo { .. })
    ));
    assert_eq!(
        result(&evaluation, "copied")
            .as_ref()
            .unwrap()
            .si_value()
            .unwrap()
            .to_bits(),
        1.0_f64.to_bits()
    );
    compile_and_eval("type State { todo(value: Length) } node x: State = todo(value: 1.0 m);")
        .unwrap();
}

#[test]
fn unfinished_nodes_keep_all_static_checks() {
    for source in [
        "node x: Length = todo { @missing };",
        "node x: Length = todo {}; node y: Length = @x + 1.0 s;",
        "node x: Length = todo { @x };",
        "node x: Length = todo { @y }; node y: Length = @x;",
        "assert a = true; node x: Length = todo { @a };",
    ] {
        assert!(!check(source), "unexpectedly accepted: {source}");
    }
}

#[test]
fn marker_is_not_an_expression_or_a_constant_or_a_default() {
    for source in [
        "node x = todo {};",
        "node x: Length = todo;",
        "node x: Length = todo();",
        "param x: Length = todo {};",
        "const node x: Length = todo {};",
        "node x: Length = (todo {});",
        "node x: Length = todo {} + 1.0 m;",
        "node x: Length = if true { todo {} } else { 1.0 m };",
        "node x: Length = todo { 1.0 m };",
        "node x: Length = todo { @y + @z };",
        "node x: Length = todo { @y[0] };",
        "node x: Length = todo { @y.field };",
        "node x: Length = todo { @y @z };",
        "node x: Length = todo { m };",
        "node x: Length = todo { @m };",
        "node x: Length = todo { @d()::x };",
    ] {
        assert!(
            compile_and_eval(source).is_err(),
            "unexpectedly accepted: {source}"
        );
    }
}

#[test]
fn blocking_is_static_and_retains_all_origins_and_real_failures() {
    let evaluation = compile_and_eval(
        r"
node a: Length = todo {};
node b: Length = todo {};
node c: Length = @a + @b;
node d: Length = if true { 1.0 m } else { @c };
node error: Length = 1.0 m / 0.0;
node mixed: Length = @d + @error;
",
    )
    .unwrap();
    let reason = result(&evaluation, "d").as_ref().unwrap_err();
    assert_eq!(reason.unfinished().len(), 2);
    assert!(!reason.has_failure());
    let reason = result(&evaluation, "mixed").as_ref().unwrap_err();
    assert_eq!(reason.unfinished().len(), 2);
    assert!(reason.has_failure());
}

#[test]
fn records_and_indexed_nodes_keep_their_declared_types() {
    let evaluation = compile_and_eval(
        r"
type Pair { Pair(length: Length, time: Time) }
node pair: Pair = todo {};
node speed: Velocity = @pair.length / @pair.time;
node array: Length[Fin(2)] = todo {};
node sum_length: Length = sum(@array);
",
    )
    .unwrap();
    assert!(matches!(
        result(&evaluation, "speed"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert!(matches!(
        result(&evaluation, "sum_length"),
        Err(NodeUnavailable::Blocked { .. })
    ));
}

#[test]
fn include_and_inline_call_preserve_unfinished_and_independent_outputs() {
    let evaluation = compile_and_eval(
        r"
dag partial {
    pub node missing: Length = todo {};
    pub node known: Length = 2.0 m;
}
include partial() as included;
node missing: Length = @partial()::missing;
node known: Length = @partial()::known;
",
    )
    .unwrap();
    assert!(matches!(
        result(&evaluation, "missing"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert_eq!(
        result(&evaluation, "known")
            .as_ref()
            .unwrap()
            .si_value()
            .unwrap()
            .to_bits(),
        2.0_f64.to_bits()
    );
    assert!(matches!(
        result(&evaluation, "included::missing"),
        Err(NodeUnavailable::Todo { .. })
    ));
}

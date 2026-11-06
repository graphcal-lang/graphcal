#![cfg(test)]

use graphcal_eval::eval::{EvalResult, NodeUnavailable, OutputUnavailable};
use graphcal_project::loader::LoadedProject;
use graphcal_project::prepare::compile_and_eval;
use graphcal_project::project_compiler::ProjectCompiler;

fn result<'a>(
    evaluation: &'a EvalResult,
    name: &str,
) -> &'a Result<graphcal_eval::eval::Value, OutputUnavailable> {
    &evaluation
        .entries
        .iter()
        .find(
            |graphcal_eval::eval::EvalEntry {
                 name: candidate, ..
             }| candidate.to_string() == name,
        )
        .unwrap_or_else(|| panic!("missing {name}; results: {:?}", evaluation.entries))
        .outcome
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
node unfinished: Length = todo<Length> {};
#[expected_fail]
assert missing = if true { true } else { @unfinished > 0.0 m };
assert independent = true;
",
    )
    .unwrap();
    assert!(evaluation.is_incomplete());
    assert!(!evaluation.has_errors());
    assert!(matches!(
        evaluation.assertions[0].outcome,
        AssertResult::Blocked { .. }
    ));
    assert!(matches!(
        evaluation.assertions[1].outcome,
        AssertResult::Pass
    ));
}

#[test]
fn successful_call_projection_still_reports_private_unfinished_nodes() {
    let evaluation = compile_and_eval(
        r"
dag model {
    node unfinished: Length = todo<Length> {};
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
    node unfinished: Length = todo<Length> {};
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
        dag source { pub node unfinished: Length = todo<Length> {}; }
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
node missing: Length[todo] = todo<Length[todo]> { @todo };
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
dag source { pub node missing: Length = todo<Length> {}; }
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
        evaluation.assertions[0].outcome,
        graphcal_eval::eval::AssertResult::Blocked { .. }
    ));
    assert!(!evaluation.has_errors());
}

#[test]
fn blocking_includes_unselected_call_outputs() {
    let evaluation = compile_and_eval(
        r"
dag model { pub node missing: Length = todo<Length> {}; }
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
node missing: Dimensionless = todo<Dimensionless> {};
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
    use graphcal_project::graph_ir::{
        dot::{GraphView, render},
        project_tir,
    };
    let project = LoadedProject::from_source("param input: Length = 1.0 m; const node scale: Dimensionless = 2.0; node missing: Length = todo<Length> { @input, @input, @scale };", "todo.gcl").unwrap();
    let checked = ProjectCompiler::new(&project).check().unwrap();
    let dot = render(&project_tir(checked.tir()).unwrap(), GraphView::Flat);
    assert!(dot.contains("TODO"));
    assert_eq!(dot.matches(" -> ").count(), 2);
}

#[test]
fn graph_export_draws_an_unfinished_constant_as_a_dashed_constant() {
    use graphcal_project::graph_ir::{
        dot::{GraphView, render},
        project_tir,
    };
    let project = LoadedProject::from_source(
        "const node g0: Acceleration = 9.80665 m/s^2; const node v: Velocity = todo<Velocity> { @g0 };",
        "todo.gcl",
    )
    .unwrap();
    let checked = ProjectCompiler::new(&project).check().unwrap();
    let dot = render(&project_tir(checked.tir()).unwrap(), GraphView::Flat);
    assert!(
        dot.contains("[label=\"v\\nVelocity\\nTODO\", shape=box, style=\"rounded,dashed\"]"),
        "{dot}"
    );
    assert!(dot.contains("[label=\"g0\\nAcceleration\", shape=box, style=rounded]"));
}

#[test]
fn rocket_checks_and_evaluates_independent_nodes() {
    let source = r"
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
const node g0: Acceleration = 9.80665 m/s^2;
node v_exhaust: Velocity = todo<Velocity> { @isp, @g0 };
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
            .get()
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
node unfinished: Length = todo<Length> {};
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
            .get()
            .to_bits(),
        1.0_f64.to_bits()
    );
    compile_and_eval("type State { todo(value: Length) } node x: State = todo(value: 1.0 m);")
        .unwrap();
}

#[test]
fn unfinished_nodes_keep_all_static_checks() {
    for source in [
        "node x: Length = todo<Length> { @missing };",
        "node x: Length = todo<Length> {}; node y: Length = @x + 1.0 s;",
        "node x: Length = todo<Length> { @x };",
        "node x: Length = todo<Length> { @y }; node y: Length = @x;",
        "assert a = true; node x: Length = todo<Length> { @a };",
    ] {
        assert!(!check(source), "unexpectedly accepted: {source}");
    }
}

#[test]
fn holes_are_expressions_anywhere_in_a_node_formula() {
    let evaluation = compile_and_eval(
        r"
param k: Dimensionless = 2.0;
node parenthesized: Length = (todo<Length> {});
node operand: Length = todo<Length> { @k } + 1.0 m;
node unselected: Length = if true { 1.0 m } else { todo<Length> {} };
node argument: Area = sqrt(todo<Area^2> {});
node element: Length[Fin(2)] = for i: Fin(2) { todo<Length> { @k } };
node done: Length = 1.0 m;
",
    )
    .unwrap();
    for name in [
        "parenthesized",
        "operand",
        "unselected",
        "argument",
        "element",
    ] {
        assert!(
            matches!(result(&evaluation, name), Err(NodeUnavailable::Todo { .. })),
            "{name}: {:?}",
            result(&evaluation, name)
        );
    }
    assert!(result(&evaluation, "done").is_ok());
}

#[test]
fn holes_keep_the_surrounding_formula_type_checked() {
    for source in [
        "node x: Length = todo<Time> {};",
        "node x: Length = todo<Length> {} + 1.0 s;",
        "node x: Length = sqrt(todo<Length> {});",
        "node x: Length[Fin(2)] = for i: Fin(2) { todo<Length[Fin(2)]> {} };",
    ] {
        assert!(!check(source), "unexpectedly accepted: {source}");
    }
}

#[test]
fn holes_are_rejected_outside_formulas_and_in_malformed_shapes() {
    for source in [
        "node x = todo<Length> {};",
        "node x: Length = todo {};",
        "node x: Length = todo;",
        "node x: Length = todo();",
        "node x: Length = todo<Length>;",
        "node x: Length = todo<Length(min: 0.0 m)> {};",
        "param x: Length = todo<Length> {};",
        "const unit u: Length = (todo<Dimensionless> {}) m;",
        "assert a = todo<Bool> {};",
        "node x: Length = todo<Length> { 1.0 m };",
        "node x: Length = todo<Length> { @y + @z };",
        "node x: Length = todo<Length> { @y[0] };",
        "node x: Length = todo<Length> { @y.field };",
        "node x: Length = todo<Length> { @y @z };",
        "node x: Length = todo<Length> { m };",
        "node x: Length = todo<Length> { @m };",
        "node x: Length = todo<Length> { @d()::x };",
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
node a: Length = todo<Length> {};
node b: Length = todo<Length> {};
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
node pair: Pair = todo<Pair> {};
node speed: Velocity = @pair.length / @pair.time;
node array: Length[Fin(2)] = todo<Length[Fin(2)]> {};
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
    pub node missing: Length = todo<Length> {};
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
            .get()
            .to_bits(),
        2.0_f64.to_bits()
    );
    assert!(matches!(
        result(&evaluation, "included::missing"),
        Err(NodeUnavailable::Todo { .. })
    ));
}

const RECURRENCE: &str = r"
index Step = range(0.0 s, 3.0 s, step: 1.0 s);
param initial_state: Dimensionless = 1.0;
param k: Dimensionless = 0.5;
";

#[test]
fn keyed_interfaces_sketch_a_recurrence_before_its_formulas() {
    let source = format!(
        "{RECURRENCE}
node action: Dimensionless[Step] = for t: Step {{ todo<Dimensionless> {{ @k, @state[t] }} }};
node state: Dimensionless[Step] = unfold(Step, todo<Dimensionless> {{ @initial_state }}, |s, prev_t, t| todo<Dimensionless> {{ s, @action[prev_t] }});
node total: Dimensionless = sum(@state);
node free: Dimensionless = @k * 2.0;
"
    );
    assert!(check(&source));
    let evaluation = compile_and_eval(&source).unwrap();
    for name in ["action", "state"] {
        assert!(matches!(
            result(&evaluation, name),
            Err(NodeUnavailable::Todo { .. })
        ));
    }
    assert!(matches!(
        result(&evaluation, "total"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert!(result(&evaluation, "free").is_ok());
    assert!(!evaluation.has_errors());
}

#[test]
fn finished_members_of_a_group_with_an_unfinished_member_are_blocked() {
    let evaluation = compile_and_eval(&format!(
        "{RECURRENCE}
node action: Dimensionless[Step] = for t: Step {{ @k * @state[t] }};
node state: Dimensionless[Step] = unfold(Step, @initial_state, |s, prev_t, t| todo<Dimensionless> {{ s, @action[prev_t] }});
"
    ))
    .unwrap();
    assert!(matches!(
        result(&evaluation, "state"),
        Err(NodeUnavailable::Todo { .. })
    ));
    let reason = result(&evaluation, "action").as_ref().unwrap_err();
    assert!(matches!(reason, NodeUnavailable::Blocked { .. }));
    assert_eq!(reason.unfinished().len(), 1);
    assert!(!evaluation.has_errors());
}

#[test]
fn keyed_interfaces_are_checked_like_reads() {
    for body in [
        // Same-step cycle through two interfaces (R7).
        "node action: Dimensionless[Step] = for t: Step { todo<Dimensionless> { @state[t] } };
         node state: Dimensionless[Step] = unfold(Step, 1.0, |s, prev_t, t| todo<Dimensionless> { @action[t] });",
        // A whole-value read in a cycle (R6).
        "node action: Dimensionless[Step] = for t: Step { todo<Dimensionless> { @state } };
         node state: Dimensionless[Step] = unfold(Step, 1.0, |s, prev_t, t| s + @action[prev_t]);",
        // A key out of scope.
        "node action: Dimensionless[Step] = for t: Step { todo<Dimensionless> { @k[prev_t] } };",
        // A key of another axis.
        "index Other = { A, B };
         node o: Dimensionless[Other] = for x: Other { 1.0 };
         node bad: Dimensionless[Step] = for t: Step { todo<Dimensionless> { @o[t] } };",
        // A local that is not in scope.
        "node x: Dimensionless = todo<Dimensionless> { s };",
    ] {
        let source = format!("{RECURRENCE}{body}");
        assert!(!check(&source), "unexpectedly accepted: {body}");
    }
}

#[test]
fn reachable_unfinished_names_each_unfinished_declaration_once_by_its_definition() {
    let loaded = LoadedProject::from_source(
        r"
dag partial {
    pub node missing: Length = todo<Length> {};
    pub node known: Length = 2.0 m;
}
dag unused { node pending: Length = todo<Length> {}; }
include partial() as a;
include partial() as b;
node direct: Length = todo<Length> {};
node blocked: Length = @direct + @a::missing;
",
        "todo.gcl",
    )
    .unwrap();
    let checked = ProjectCompiler::new(&loaded).check().unwrap();
    let names = checked
        .tir()
        .reachable_unfinished()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["todo.direct", "todo.partial.missing", "todo.unused.pending"]
    );
}

/// The diagnostic code of the error rejecting `source`.
fn rejection_code(source: &str) -> String {
    let error = compile_and_eval(source)
        .map(|_| ())
        .expect_err("source must be rejected");
    miette::Diagnostic::code(&error)
        .expect("error must have a code")
        .to_string()
}

#[test]
fn unfinished_constants_have_no_value_and_block_what_reads_them() {
    let evaluation = compile_and_eval(
        r"
const node g0: Acceleration = 9.80665 m/s^2;
const node isp: Time = 300.0 s;
const node v_exhaust: Velocity = todo<Velocity> { @isp, @g0 };
const node doubled: Velocity = 2.0 * @v_exhaust;
const node bounded: Velocity(min: 0.0 m/s) = @doubled;
const node static_rule: Length = if true { 1.0 m } else { todo<Length> {} };
node reads_constant: Velocity = 3.0 * @doubled;
node independent: Velocity = @isp * @g0;
unit scaled: Length = (@static_rule / 1.0 m) m;
node uses_unit: Length = 2.0 scaled;
assert positive = @doubled > 0.0 m/s;
plot chart = { mark: point, encode: { x: @doubled, y: 1.0 m } };
",
    )
    .unwrap();
    assert!(matches!(
        result(&evaluation, "v_exhaust"),
        Err(NodeUnavailable::Todo { .. })
    ));
    assert!(matches!(
        result(&evaluation, "static_rule"),
        Err(NodeUnavailable::Todo { .. })
    ));
    for blocked in ["doubled", "bounded", "reads_constant", "uses_unit"] {
        assert!(
            matches!(
                result(&evaluation, blocked),
                Err(NodeUnavailable::Blocked { failed_deps, .. }) if failed_deps.is_empty()
            ),
            "{blocked}: {:?}",
            result(&evaluation, blocked)
        );
    }
    assert!(result(&evaluation, "independent").is_ok());
    assert!(evaluation.is_incomplete());
    assert!(!evaluation.has_errors());
}

#[test]
fn a_bound_reading_an_unfinished_constant_is_a_compile_error() {
    const UNFINISHED: &str = "const node lo: Length = todo<Length> {};\n";
    for declaration in [
        "param x: Length(min: @lo) = 1.0 m;",
        "node y: Length(max: 2.0 * @lo) = 1.0 m;",
        "const node lo2: Length = @lo; const node z: Length(min: @lo2) = 1.0 m;",
        "type Box { Box(value: Length(min: @lo)) } node b: Box = Box(value: 2.0 m);",
    ] {
        assert_eq!(
            rejection_code(&format!("{UNFINISHED}{declaration}")),
            "graphcal::C010",
            "{declaration}"
        );
    }
}

#[test]
fn a_constant_hole_names_only_constants() {
    for source in [
        "param m: Mass = 1.0 kg; const node c: Mass = todo<Mass> { @m };",
        "node n: Mass = 1.0 kg; const node c: Mass = todo<Mass> { @n };",
    ] {
        assert_eq!(rejection_code(source), "graphcal::N005", "{source}");
    }
    // Interface entries are dependencies, so they close cycles.
    assert_eq!(
        rejection_code("const node a: Length = todo<Length> { @b }; const node b: Length = @a;"),
        "graphcal::G001"
    );
}

#[test]
fn includes_and_calls_see_the_unfinished_constants_of_their_template() {
    let evaluation = compile_and_eval(
        r"
dag inner {
    pub const node k: Length = todo<Length> {};
    pub node out: Length = 2.0 * @k;
    pub node free: Length = 1.0 m;
}
include inner() as i;
node via_free: Length = @inner()::free;
node via_out: Length = @inner()::out;
",
    )
    .unwrap();
    assert!(result(&evaluation, "via_free").is_ok());
    assert!(matches!(
        result(&evaluation, "via_out"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert!(matches!(
        result(&evaluation, "i::k"),
        Err(NodeUnavailable::Todo { .. })
    ));
    assert!(matches!(
        result(&evaluation, "i::out"),
        Err(NodeUnavailable::Blocked { .. })
    ));
}

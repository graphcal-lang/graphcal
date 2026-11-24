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
assert missing = @unfinished > 0.0 m;
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
fn call_blocking_follows_evaluation_through_overrides_and_nested_calls() {
    let evaluation = compile_and_eval(
        r"
dag source { pub node missing: Length = todo<Length> {}; }
dag stage {
    param value: Length = @source()::missing;
    pub node output: Length = @value;
}
dag wrapper {
    pub node output: Length = @stage()::output;
}
node supplied: Length = @stage(value: 2.0 m)::output;
node blocked: Length = @wrapper()::output;
assert pending = @source()::missing > 0.0 m;
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
fn only_call_outputs_that_evaluation_reaches_block() {
    let evaluation = compile_and_eval(
        r"
dag model { pub node missing: Length = todo<Length> {}; }
param reach: Bool = false;
node unselected: Length = if @reach { @model()::missing } else { 1.0 m };
node selected: Length = if @reach { 1.0 m } else { @model()::missing };
",
    )
    .unwrap();
    assert!(
        result(&evaluation, "unselected").is_ok(),
        "{:?}",
        result(&evaluation, "unselected")
    );
    assert!(
        matches!(
            result(&evaluation, "selected"),
            Err(NodeUnavailable::Blocked { .. })
        ),
        "{:?}",
        result(&evaluation, "selected")
    );
    assert!(evaluation.is_incomplete());
}

#[test]
fn a_model_whose_holes_are_not_reached_is_still_incomplete() {
    let evaluation = compile_and_eval(
        r"
param reach: Bool = false;
node value: Length = if @reach { todo<Length> {} } else { 1.0 m };
",
    )
    .unwrap();
    assert!(result(&evaluation, "value").is_ok());
    assert!(!evaluation.reached_unfinished());
    assert!(evaluation.is_incomplete());
    assert_eq!(evaluation.unfinished_declarations.len(), 1);
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
node zero_times: Length = 0.0 * todo<Length> {};
node argument: Area = sqrt(todo<Area^2> {});
node element: Length[Fin(2)] = for i: Fin(2) { todo<Length> { @k } };
node done: Length = 1.0 m;
",
    )
    .unwrap();
    for name in [
        "parenthesized",
        "operand",
        "zero_times",
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
    // A hole that evaluation does not reach has no effect on the result.
    assert!(result(&evaluation, "unselected").is_ok());
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
fn blocking_follows_evaluation_and_keeps_transitive_origins_and_real_failures() {
    let evaluation = compile_and_eval(
        r"
node a: Length = todo<Length> {};
node b: Length = @a * 2.0;
node c: Length = @b + 1.0 m;
node d: Length = if true { 1.0 m } else { @c };
node error: Length = 1.0 m / 0.0;
node mixed: Length = @error + @c;
",
    )
    .unwrap();
    let reason = result(&evaluation, "c").as_ref().unwrap_err();
    assert!(matches!(reason, NodeUnavailable::Blocked { .. }));
    assert_eq!(
        reason
            .unfinished()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert!(!reason.has_failure());
    assert!(result(&evaluation, "d").is_ok());
    // Evaluation stops at the first input with no value; the failure is
    // reported, and `error` fails by itself.
    let reason = result(&evaluation, "mixed").as_ref().unwrap_err();
    assert!(reason.has_failure());
    assert!(evaluation.has_errors());
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
    // The init of `state` reaches its hole first. Every element of `action`
    // reads, through its interface, the element of `state` at the same step,
    // so `action` is blocked by `state` (`internals/recurrence-groups.md` §5).
    assert!(matches!(
        result(&evaluation, "state"),
        Err(NodeUnavailable::Todo { .. })
    ));
    assert!(matches!(
        result(&evaluation, "action"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert!(matches!(
        result(&evaluation, "total"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert!(result(&evaluation, "free").is_ok());
    assert!(!evaluation.has_errors());
}

#[test]
fn a_member_reaching_its_own_hole_with_other_unfinished_inputs_is_todo() {
    let evaluation = compile_and_eval(&format!(
        "{RECURRENCE}
dag helper {{ pub node missing: Dimensionless = todo<Dimensionless> {{}}; }}
node action: Dimensionless[Step] = for t: Step {{ @k * @state[t] }};
node state: Dimensionless[Step] = unfold(Step, todo<Dimensionless> {{}} + @helper()::missing, |s, prev_t, t| s + @action[prev_t]);
"
    ))
    .unwrap();
    assert!(
        matches!(
            result(&evaluation, "state"),
            Err(NodeUnavailable::Todo { .. })
        ),
        "{:?}",
        result(&evaluation, "state")
    );
}

#[test]
fn a_failure_is_not_hidden_by_an_unfinished_datetime_operand() {
    let evaluation = compile_and_eval(
        r"
param zero: Dimensionless = 0.0;
node later: Datetime<TAI> = todo<Datetime<TAI>> {} + 1.0 s / @zero;
node earlier: Datetime<TAI> = 1.0 s / @zero + todo<Datetime<TAI>> {};
",
    )
    .unwrap();
    for name in ["later", "earlier"] {
        let reason = result(&evaluation, name).as_ref().unwrap_err();
        assert!(reason.has_failure(), "{name}: {reason:?}");
    }
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
const node unreached: Length = if true { 1.0 m } else { todo<Length> {} };
const node reached: Length = if false { 1.0 m } else { todo<Length> {} };
node reads_constant: Velocity = 3.0 * @doubled;
node independent: Velocity = @isp * @g0;
unit scaled: Length = (@reached / 1.0 m) m;
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
    // A constant is TODO only when its evaluation reaches the hole.
    assert!(matches!(
        result(&evaluation, "reached"),
        Err(NodeUnavailable::Todo { .. })
    ));
    assert!(result(&evaluation, "unreached").is_ok());
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
fn a_bound_is_an_error_only_when_it_reads_a_missing_value() {
    let source = r"
index Part = { Wheel, Bracket };
const node lo: Length = todo<Length> {};
const node limits: Length[Part] = table[Part] { Wheel: 1.0 m; Bracket: todo<Length> {}; };
param unselected: Length(min: if true { 0.0 m } else { @lo }) = 1.0 m;
param known_cell: Length(max: @limits[Part#Wheel]) = 0.5 m;
";
    assert!(check(source));
    assert_eq!(
        rejection_code(&format!(
            "{source}param missing_cell: Length(max: @limits[Part#Bracket]) = 0.5 m;"
        )),
        "graphcal::C010"
    );
}

#[test]
fn a_constant_table_with_an_unfinished_cell_keeps_its_other_cells() {
    let evaluation = compile_and_eval(
        r"
index Part = { Wheel, Bracket, Panel };
const node qty: Int[Part] = table[Part] { Wheel: 4; Bracket: todo<Int> {}; Panel: 2; };
const node wheels: Int = @qty[Part#Wheel];
const node bracket: Int = @qty[Part#Bracket];
node doubled: Int[Part] = for p: Part { @qty[p] * 2 };
",
    )
    .unwrap();
    assert!(matches!(
        result(&evaluation, "qty"),
        Err(NodeUnavailable::Todo { .. })
    ));
    let qty = cells(&evaluation, "qty");
    assert_eq!(int_of(cell(qty, "Wheel")), 4);
    assert!(result(&evaluation, "wheels").is_ok());
    assert!(matches!(
        result(&evaluation, "bracket"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    assert_eq!(int_of(cell(cells(&evaluation, "doubled"), "Panel")), 4);
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

/// A package `review` whose entry `src/review/main.gcl` (`main`) includes
/// `review.lib`, binding its port to an expression over `review.consts`.
fn write_binding_package(main: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"review\"\n",
    )
    .unwrap();
    let package = dir.path().join("src").join("review");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("consts.gcl"),
        "pub const node c_done: Dimensionless = 3.0;\n\
         pub const node c_todo: Dimensionless = todo<Dimensionless> {};\n",
    )
    .unwrap();
    std::fs::write(
        package.join("lib.gcl"),
        "param x: Dimensionless = 1.0;\npub node y: Dimensionless = @x * 2.0;\n",
    )
    .unwrap();
    let entry = package.join("main.gcl");
    std::fs::write(&entry, main).unwrap();
    (dir, entry)
}

fn check_package(entry: &std::path::Path) -> graphcal_project::project_compiler::CheckedProject {
    let project = graphcal_project::loader::load_project(
        entry,
        None,
        &graphcal_io::RealFileSystem::default(),
    )
    .unwrap();
    ProjectCompiler::new(&project).check().unwrap()
}

#[test]
fn an_include_binding_reading_an_unfinished_constant_is_reachable() {
    // `check --deny-todo` counts `reachable_unfinished`; it must agree with
    // evaluation, which blocks everything downstream of the binding.
    let (_dir, entry) = write_binding_package(
        "import review.consts as c;\n\
         include review.lib(x: c::c_todo) as inst;\n\
         node out: Dimensionless = @inst::y;\n",
    );
    let checked = check_package(&entry);
    let names = checked
        .tir()
        .reachable_unfinished()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(
        matches!(names.as_slice(), [name] if name.ends_with("consts.c_todo")),
        "{names:?}"
    );
}

#[test]
fn graph_export_draws_the_constant_reads_of_an_include_binding() {
    use graphcal_project::graph_ir::{
        dot::{GraphView, render},
        project_tir,
    };
    let (_dir, entry) = write_binding_package(
        "import review.consts as c;\n\
         param k: Dimensionless = 1.0;\n\
         include review.lib(x: @k + c::c_done) as inst;\n\
         node out: Dimensionless = @inst::y;\n",
    );
    let checked = check_package(&entry);
    let dot = render(&project_tir(checked.tir()).unwrap(), GraphView::Flat);
    let constant = dot
        .lines()
        .find(|line| line.contains("consts.c_done\""))
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or_else(|| panic!("no c_done node: {dot}"));
    let port = dot
        .lines()
        .find(|line| line.contains("[label=\"x\\nDimensionless\""))
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or_else(|| panic!("no port node: {dot}"));
    assert!(dot.contains(&format!("{constant} -> {port};")), "{dot}");
}

/// The cells of `name`, which has no value as a whole but has cells.
fn cells<'a>(evaluation: &'a EvalResult, name: &str) -> &'a graphcal_eval::eval::ValueCells {
    evaluation
        .entries
        .iter()
        .find(|entry| entry.name.to_string() == name)
        .and_then(|entry| entry.cells.as_ref())
        .unwrap_or_else(|| panic!("{name} has no cells"))
}

/// The cell of `cells` at the named entry `label`.
fn cell<'a>(
    cells: &'a graphcal_eval::eval::ValueCells,
    label: &str,
) -> &'a graphcal_eval::eval::ValueCell {
    cells
        .entries
        .iter()
        .find_map(|(key, cell)| (key.to_string() == label).then_some(cell))
        .unwrap_or_else(|| panic!("no cell {label}"))
}

fn int_of(value: &graphcal_eval::eval::ValueCell) -> i64 {
    match value {
        graphcal_eval::eval::ValueCell::Value(graphcal_eval::eval::Value::Int(value)) => *value,
        other => panic!("not an Int cell: {other:?}"),
    }
}

const PARTS: &str = r"
index Part = { Wheel, Bracket, Panel };
node qty: Int[Part] = table[Part] {
    Wheel: 4;
    Bracket: todo<Int> {};
    Panel: 2;
};
";

#[test]
fn a_hole_in_one_cell_leaves_the_other_cells_and_their_readers_available() {
    let evaluation = compile_and_eval(&format!(
        "{PARTS}
node wheel_count: Int = @qty[Part#Wheel];
node bracket_twice: Int = @qty[Part#Bracket] * 2;
node doubled: Int[Part] = for p: Part {{ @qty[p] * 2 }};
node total: Int = @qty[Part#Wheel] + @qty[Part#Bracket];
node picked: Int = @qty[argmax(for p: Part {{ if p == Part#Panel {{ 1.0 }} else {{ 0.0 }} }})];
"
    ))
    .unwrap();
    assert!(matches!(
        result(&evaluation, "qty"),
        Err(NodeUnavailable::Todo { .. })
    ));
    let qty = cells(&evaluation, "qty");
    assert_eq!(int_of(cell(qty, "Wheel")), 4);
    assert_eq!(int_of(cell(qty, "Panel")), 2);
    assert!(matches!(
        cell(qty, "Bracket"),
        graphcal_eval::eval::ValueCell::Unavailable(NodeUnavailable::Todo { .. })
    ));
    assert!(result(&evaluation, "wheel_count").is_ok());
    // A runtime key reads the one cell it selects.
    assert!(result(&evaluation, "picked").is_ok());
    for blocked in ["bracket_twice", "total", "doubled"] {
        assert!(
            matches!(
                result(&evaluation, blocked),
                Err(NodeUnavailable::Blocked { .. })
            ),
            "{blocked}: {:?}",
            result(&evaluation, blocked)
        );
    }
    let doubled = cells(&evaluation, "doubled");
    assert_eq!(int_of(cell(doubled, "Wheel")), 8);
    assert!(matches!(
        cell(doubled, "Bracket"),
        graphcal_eval::eval::ValueCell::Unavailable(NodeUnavailable::Blocked { .. })
    ));
    assert!(evaluation.is_incomplete());
    assert!(!evaluation.has_errors());
}

#[test]
fn cells_pass_through_references_slices_and_selected_arms() {
    let evaluation = compile_and_eval(
        r"
index Phase = { Launch, Cruise };
index Part = { Wheel, Bracket };
param pick_launch: Bool = true;
node m: Int[Phase, Part] = table[Phase, Part] {
    : Wheel, Bracket;
    Launch: 1, todo<Int> {};
    Cruise: 3, 4;
};
node alias: Int[Phase, Part] = @m;
node launch: Int[Part] = @m[Phase#Launch];
node cruise: Int[Part] = @m[Phase#Cruise];
node chosen: Int[Part] = if @pick_launch { @m[Phase#Launch] } else { @m[Phase#Cruise] };
node launch_wheel: Int = @m[Phase#Launch, Part#Wheel];
",
    )
    .unwrap();
    // A slice whose cells all have values is a value.
    assert!(result(&evaluation, "cruise").is_ok());
    assert!(result(&evaluation, "launch_wheel").is_ok());
    for name in ["alias", "launch", "chosen"] {
        assert!(
            matches!(
                result(&evaluation, name),
                Err(NodeUnavailable::Blocked { .. })
            ),
            "{name}: {:?}",
            result(&evaluation, name)
        );
    }
    assert_eq!(int_of(cell(cells(&evaluation, "launch"), "Wheel")), 1);
    assert_eq!(int_of(cell(cells(&evaluation, "chosen"), "Wheel")), 1);
    let alias = cells(&evaluation, "alias");
    let graphcal_eval::eval::ValueCell::Cells(cruise) = cell(alias, "Cruise") else {
        panic!("a row of cells");
    };
    assert_eq!(int_of(cell(cruise, "Bracket")), 4);
}

#[test]
fn a_failed_or_out_of_domain_cell_fails_alone() {
    let evaluation = compile_and_eval(
        r"
index Part = { Wheel, Bracket, Panel };
param divisor: Int = 0;
node ratio: Int[Part] = for p: Part { if p == Part#Panel { div_trunc(1, @divisor) } else { 1 } };
node bounded: Int(min: 0)[Part] = table[Part] { Wheel: 1; Bracket: -1; Panel: todo<Int> {}; };
node wheel_ratio: Int = @ratio[Part#Wheel];
node panel_ratio: Int = @ratio[Part#Panel];
",
    )
    .unwrap();
    assert!(evaluation.has_errors());
    assert!(result(&evaluation, "wheel_ratio").is_ok());
    let reason = result(&evaluation, "panel_ratio").as_ref().unwrap_err();
    assert!(reason.has_failure(), "{reason:?}");
    let ratio = cells(&evaluation, "ratio");
    assert_eq!(int_of(cell(ratio, "Bracket")), 1);
    let bounded = cells(&evaluation, "bounded");
    assert_eq!(int_of(cell(bounded, "Wheel")), 1);
    assert!(matches!(
        cell(bounded, "Bracket"),
        graphcal_eval::eval::ValueCell::Unavailable(NodeUnavailable::EvalFailed { .. })
    ));
    assert!(matches!(
        cell(bounded, "Panel"),
        graphcal_eval::eval::ValueCell::Unavailable(NodeUnavailable::Todo { .. })
    ));
}

#[test]
fn a_struct_with_a_hole_in_one_field_is_unfinished_as_a_whole() {
    let evaluation = compile_and_eval(
        r"
type Pair { Pair(length: Length, time: Time) }
node pair: Pair = Pair(length: 1.0 m, time: todo<Time> {});
node length: Length = @pair.length;
",
    )
    .unwrap();
    assert!(matches!(
        result(&evaluation, "pair"),
        Err(NodeUnavailable::Todo { .. })
    ));
    assert!(matches!(
        result(&evaluation, "length"),
        Err(NodeUnavailable::Blocked { .. })
    ));
}

#[test]
fn a_cell_outside_the_domain_fails_alone_also_when_every_cell_has_a_value() {
    let evaluation = compile_and_eval(
        r"
index Part = { Wheel, Bracket, Panel };
node bounded: Int(min: 0)[Part] = table[Part] { Wheel: 1; Bracket: -1; Panel: 2; };
node wheel: Int = @bounded[Part#Wheel];
",
    )
    .unwrap();
    assert!(evaluation.has_errors());
    assert!(result(&evaluation, "wheel").is_ok());
    let bounded = cells(&evaluation, "bounded");
    assert_eq!(int_of(cell(bounded, "Panel")), 2);
    assert!(matches!(
        cell(bounded, "Bracket"),
        graphcal_eval::eval::ValueCell::Unavailable(NodeUnavailable::EvalFailed { .. })
    ));
}

#[test]
fn an_indexed_row_outside_the_domain_fails_only_in_its_bad_entry() {
    let evaluation = compile_and_eval(
        r"
index Row = { A, B };
index Col = { X, Y };
node raw: Int[Col] = table[Col] { X: 1; Y: -1; };
node bounded: Int(min: 0)[Row, Col] = for r: Row { @raw };
node good: Int = @bounded[Row#A, Col#X];
",
    )
    .unwrap();
    assert!(
        result(&evaluation, "good").is_ok(),
        "{:?}",
        result(&evaluation, "good")
    );
    assert!(evaluation.has_errors());
}

#[test]
fn a_mixed_value_keeps_the_failed_declarations_its_cells_read() {
    let evaluation = compile_and_eval(
        r"
index Part = { Wheel, Bracket };
param divisor: Int = 0;
node bad: Int = div_trunc(1, @divisor);
node mixed: Int[Part] = table[Part] { Wheel: todo<Int> {} + @bad; Bracket: div_trunc(2, @divisor); };
",
    )
    .unwrap();
    let Err(NodeUnavailable::Blocked { failed_deps, .. }) = result(&evaluation, "mixed") else {
        panic!("{:?}", result(&evaluation, "mixed"));
    };
    let failed = failed_deps
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(failed.contains(&"bad".to_owned()), "{failed:?}");
    assert!(failed.contains(&"mixed".to_owned()), "{failed:?}");
}

#[test]
fn a_failed_cell_does_not_hide_an_unfinished_one_in_a_nested_row() {
    let evaluation = compile_and_eval(
        r"
index Row = { A, B };
index Col = { X, Y };
param divisor: Int = 0;
node mixed: Int[Row, Col] = table[Row, Col] { : X, Y; A: todo<Int> {}, div_trunc(1, @divisor); B: 1, 2; };
",
    )
    .unwrap();
    let reason = result(&evaluation, "mixed").as_ref().unwrap_err();
    assert!(reason.has_failure(), "{reason:?}");
    assert!(reason.is_incomplete(), "{reason:?}");
}

#[test]
fn a_failed_cell_does_not_hide_an_unfinished_one() {
    let evaluation = compile_and_eval(
        r"
index Part = { Wheel, Bracket, Panel };
param divisor: Int = 0;
node mixed: Int[Part] = table[Part] { Wheel: 1; Bracket: todo<Int> {}; Panel: div_trunc(1, @divisor); };
",
    )
    .unwrap();
    let reason = result(&evaluation, "mixed").as_ref().unwrap_err();
    assert!(reason.has_failure(), "{reason:?}");
    assert!(reason.is_incomplete(), "{reason:?}");
    assert!(evaluation.has_errors());
}

#[test]
fn cells_pass_through_inline_dag_call_arguments_and_outputs() {
    let evaluation = compile_and_eval(
        r"
node qty: Int[Fin(3)] = table[Fin(3)] { 4; todo<Int> {}; 2; };
dag twice {
    param input: Int[Fin(3)];
    pub node out: Int[Fin(3)] = for i: Fin(3) { @input[i] * 2 };
    pub node first: Int = @input[0];
}
node doubled: Int[Fin(3)] = @twice(input: @qty)::out;
node first: Int = @twice(input: @qty)::first;
node last: Int = @twice(input: @qty)::out[2];
node middle: Int = @twice(input: @qty)::out[1];
",
    )
    .unwrap();
    let doubled = cells(&evaluation, "doubled");
    assert_eq!(int_of(cell(doubled, "#0")), 8);
    assert!(matches!(
        cell(doubled, "#1"),
        graphcal_eval::eval::ValueCell::Unavailable(NodeUnavailable::Blocked { .. })
    ));
    assert!(result(&evaluation, "first").is_ok());
    assert!(result(&evaluation, "last").is_ok());
    assert!(matches!(
        result(&evaluation, "middle"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    // The hole is the caller's own: no invoked DAG is unfinished.
    assert_eq!(evaluation.unfinished_calls, Vec::new());
}

#[test]
fn an_unfold_keeps_the_steps_before_the_first_one_without_a_value() {
    let evaluation = compile_and_eval(
        r"
index Step = range(0.0 s, 4.0 s, step: 1.0 s);
param zero: Dimensionless = 0.0;
node state: Dimensionless[Step] = unfold(Step, 1.0, |x, prev_t, t| if coord(t) < 1.5 s { x * 2.0 } else { todo<Dimensionless> { x } });
node total: Dimensionless[Step] = scan(@state, 0.0, |acc, v| acc + v);
node ratio: Dimensionless[Step] = scan(for t: Step { 1.0 }, 0.0, |acc, v| if acc < 1.5 { acc + v } else { acc / @zero });
",
    )
    .unwrap();
    let state = cells(&evaluation, "state");
    let shown = state
        .entries
        .values()
        .map(|cell| match cell {
            graphcal_eval::eval::ValueCell::Value(value) => {
                value.si_value().unwrap().get().to_string()
            }
            graphcal_eval::eval::ValueCell::Unavailable(reason) => reason.status_label().to_owned(),
            graphcal_eval::eval::ValueCell::Cells(_) => panic!("nested cells"),
        })
        .collect::<Vec<_>>();
    assert_eq!(shown, ["1", "2", "TODO", "TODO", "TODO"]);
    // A scan over a value with missing cells reads it as a whole.
    assert!(matches!(
        result(&evaluation, "total"),
        Err(NodeUnavailable::Blocked { .. })
    ));
    // The steps before the failing one keep their values; the failing step
    // and every later one, which reads it, have none.
    let ratio = cells(&evaluation, "ratio")
        .entries
        .values()
        .map(|cell| match cell {
            graphcal_eval::eval::ValueCell::Value(value) => {
                value.si_value().unwrap().get().to_string()
            }
            graphcal_eval::eval::ValueCell::Unavailable(reason) => reason.status_label().to_owned(),
            graphcal_eval::eval::ValueCell::Cells(_) => panic!("nested cells"),
        })
        .collect::<Vec<_>>();
    assert_eq!(ratio, ["1", "2", "ERROR", "ERROR", "ERROR"]);
    assert!(evaluation.has_errors());
}

#[test]
fn references_arguments_and_call_slices_keep_their_cells() {
    let evaluation = compile_and_eval(
        r"
index Row = { A, B };
index Col = { X, Y };
const node limits: Int[Col] = table[Col] { X: 1; Y: todo<Int> {}; };
node alias: Int[Col] = @limits;
node grid: Int[Fin(2), Fin(2)] = for i: Fin(2), j: Fin(2) { if to_int(i) == 0 && to_int(j) == 1 { todo<Int> {} } else { 1 } };
dag pass {
    param unused: Int;
    param used: Int;
    param m: Int[Fin(2), Fin(2)];
    pub node out: Int = @used * 2;
    pub node same: Int[Fin(2), Fin(2)] = @m;
}
node unread_hole: Int = @pass(unused: todo<Int> {}, used: 3, m: @grid)::out;
node row: Int[Fin(2)] = @pass(unused: 1, used: 1, m: @grid)::same[0];
",
    )
    .unwrap();
    assert_eq!(int_of(cell(cells(&evaluation, "alias"), "X")), 1);
    assert!(
        result(&evaluation, "unread_hole").is_ok(),
        "{:?}",
        result(&evaluation, "unread_hole")
    );
    let row = cells(&evaluation, "row");
    assert_eq!(int_of(cell(row, "#0")), 1);
    assert!(matches!(
        cell(row, "#1"),
        graphcal_eval::eval::ValueCell::Unavailable(NodeUnavailable::Blocked { .. })
    ));
}

#[test]
fn the_available_cells_of_a_constant_are_checked_against_its_domain() {
    let code = rejection_code(
        "index Part = { Wheel, Bracket }; \
         const node c: Int(min: 0)[Part] = table[Part] { Wheel: -1; Bracket: todo<Int> {}; };",
    );
    assert!(code.starts_with("graphcal::"), "{code}");
    assert!(check(
        "index Part = { Wheel, Bracket }; \
         const node c: Int(min: 0)[Part] = table[Part] { Wheel: 1; Bracket: todo<Int> {}; };"
    ));
}

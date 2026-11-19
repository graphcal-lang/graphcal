//! Cancellation checkpoints and work that must be done only once.

use super::*;

/// A file whose every DAG includes the previous one twice, so the root's
/// include of `d{depth}` materializes `2^(depth + 1) - 2` instances.
fn exponential_include_source(stem: &str, depth: usize) -> String {
    let levels = (1..=depth)
        .enumerate()
        .fold(String::new(), |mut levels, (previous, level)| {
            std::fmt::Write::write_fmt(
                &mut levels,
                format_args!(
                    "pub dag d{level} {{\n\
                     import {stem}::{{d{previous}}};\n\
                     include d{previous}(x: 1.0) as a;\n\
                     include d{previous}(x: 2.0) as b;\n\
                     param x: Dimensionless = 1.0;\n\
                     pub node y: Dimensionless = @a::y + @b::y;\n\
                     }}\n"
                ),
            )
            .expect("writing to a String cannot fail");
            levels
        });
    format!(
        "pub dag d0 {{\n\
         param x: Dimensionless = 1.0;\n\
         pub node y: Dimensionless = @x;\n\
         }}\n\
         {levels}\
         include d{depth}() as top;\n\
         node r: Dimensionless = @top::y;\n"
    )
}

/// Check the exponential include tree of `depth` with a token that cancels
/// once `phase` has made a few steps, and return the outcome with the steps
/// `phase` made in all (the whole tree takes thousands).
fn cancel_during(
    phase: graphcal_compiler::cancellation::ProgressPhase,
    depth: usize,
) -> (Result<(), Outcome<CompileError>>, usize) {
    const STEPS_BEFORE_CANCELLING: usize = 10;
    let project = crate::loader::LoadedProject::from_source(
        &exponential_include_source("exponential", depth),
        "exponential.gcl",
    )
    .unwrap();
    // The check runs on this thread, whose progress the token watches.
    let before = phase.steps_on_this_thread();
    let cancellation = graphcal_compiler::cancellation::CancellationToken::cancel_after_progress(
        phase,
        STEPS_BEFORE_CANCELLING,
    );
    let result = ProjectCompiler::new(&project)
        .cancellation(&cancellation)
        .check()
        .map(|_| ());
    (result, phase.steps_on_this_thread().saturating_sub(before))
}

#[test]
fn cancellation_stops_exponential_include_expansion() {
    // The token cancels only once the resolver has expanded some instances,
    // so the check stops promptly only if the expansion itself observes it.
    let (result, steps) = cancel_during(
        graphcal_compiler::cancellation::ProgressPhase::IncludeExpansion,
        14,
    );
    assert!(
        matches!(result, Err(Outcome::Cancelled)),
        "unexpected outcome: {result:?}"
    );
    assert!(
        steps <= 11,
        "expanded {steps} instances, not 10, before stopping"
    );
}

#[test]
fn cancellation_stops_exponential_instantiation() {
    // The token cancels only once instantiation has materialized some
    // instances, so the check stops promptly only if instantiation itself
    // observes it.
    let (result, steps) = cancel_during(
        graphcal_compiler::cancellation::ProgressPhase::Instantiation,
        10,
    );
    assert!(
        matches!(result, Err(Outcome::Cancelled)),
        "unexpected outcome: {result:?}"
    );
    assert!(
        steps <= 11,
        "instantiated {steps} edges, not 10, before stopping"
    );
}

#[test]
fn instances_share_one_type_definition_table() {
    // Each instance used to copy the type definitions of every DAG present,
    // so materializing N instances cost O(N * #DAGs).
    let source = "pub type Pair { Pair(left: Dimensionless, right: Dimensionless) }\n\
                  pub dag leaf {\n\
                  import shared::{type Pair, Pair};\n\
                  param x: Dimensionless = 1.0;\n\
                  pub node pair: Pair = Pair(left: @x, right: @x);\n\
                  }\n\
                  pub dag middle {\n\
                  import shared::{leaf};\n\
                  include leaf(x: 1.0) as a;\n\
                  include leaf(x: 2.0) as b;\n\
                  pub node y: Dimensionless = 1.0;\n\
                  }\n\
                  include middle() as first;\n\
                  include middle() as second;\n\
                  node r: Dimensionless = @first::y + @second::y;\n";
    let tir = compile_to_tir(source, "shared.gcl").unwrap();
    let instances = tir
        .local_dags()
        .filter(|(id, _)| id.leaf().is_instance())
        .map(|(_, dag)| &dag.body_for_test().semantic().type_defs)
        .collect::<Vec<_>>();
    assert!(
        instances.len() >= 6,
        "two middles, each with two leaves: {}",
        instances.len()
    );
    assert!(
        instances
            .iter()
            .all(|type_defs| type_defs.shares_table_with(instances[0])),
        "every instance must share the project-wide type-definition table"
    );
}

#[test]
fn rigid_templates_are_built_once_however_many_includes_rebind_them() {
    // Each rebinding include used to rebuild its template's rigid view from
    // a clone of the whole TIR, twice (to instantiate it and to infer its
    // port-generic trees), so N rebinding includes cost O(N^2). The rigid
    // views are now built once per template and port set.
    let rigid_view_builds = |includes: usize| {
        let includes = (0..includes)
            .map(|index| format!("include lib(dim Out: Time, value: {index}.0 s) as i{index};\n"))
            .collect::<Vec<_>>()
            .concat();
        let source = format!(
            "dag lib {{\n\
             pub(bind) dim Out = Length;\n\
             param value: Out;\n\
             pub node v: Out = @value;\n\
             }}\n\
             {includes}"
        );
        // The check runs on this thread, so the thread's count is its own.
        let before = graphcal_compiler::tir::typed::rigid_view_builds_for_test();
        compile_to_tir(&source, "rigid.gcl").unwrap();
        graphcal_compiler::tir::typed::rigid_view_builds_for_test() - before
    };
    let one = rigid_view_builds(1);
    assert!(one > 0, "a rebinding include must build a rigid view");
    assert_eq!(
        rigid_view_builds(50),
        one,
        "rigid views must be built once per template and port set, not per include"
    );
}

#[test]
fn cancellation_stops_an_in_flight_evaluation() {
    use std::{
        collections::HashMap,
        sync::{Arc, Barrier, mpsc},
        thread,
        time::Duration,
    };

    let source = "node values: Dimensionless[Fin(1000000)] = \
                  for i: Fin(1000000) { 1.0 };";
    let project = crate::loader::LoadedProject::from_source(source, "cancel.gcl").unwrap();
    let cancellation_source = graphcal_compiler::cancellation::CancellationSource::new();
    let cancellation = cancellation_source.token();
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        worker_barrier.wait();
        let result = ProjectCompiler::new(&project)
            .cancellation(&cancellation)
            .eval(&HashMap::new());
        sender.send(result).unwrap();
    });

    barrier.wait();
    // Let the worker enter the compile/eval pipeline before requesting
    // cancellation. The exact stage is deliberately irrelevant: every stage
    // shares the same token and must unwind to this boundary.
    thread::sleep(Duration::from_millis(20));
    cancellation_source.cancel();

    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("cancelled evaluation should stop promptly");
    let error = result.expect_err("the long-running evaluation should be cancelled");
    assert!(
        matches!(error, Outcome::Cancelled),
        "unexpected outcome: {error:?}"
    );
}

/// Projecting a large indexed output observes the request's cancellation per
/// entry, and a deadline that passes during projection cancels the row
/// rather than returning it as a success (#2251).
#[test]
fn model_requests_observe_cancellation_while_projecting_outputs() {
    use graphcal_model_protocol::{FiniteF64, InputColumn, RequestBatch};

    const ENTRIES: usize = 20_000;
    // The smallest number of successful checkpoints that lets one request of
    // `output` over `ENTRIES` values finish.
    let checkpoints_to_finish = |output: &str| {
        let source = format!(
            "param scale: Dimensionless = 1.0;\n\
             node values: Dimensionless[Fin({ENTRIES})] = for i: Fin({ENTRIES}) {{ @scale }};\n\
             pub node indexed: Dimensionless[Fin({ENTRIES})] = @values;\n\
             pub node total: Int = count(@values);\n"
        );
        let project = crate::loader::LoadedProject::from_source(&source, "model.gcl").unwrap();
        let prepared = prepare_from_project(&project).unwrap();
        let schema = session_over(&prepared, &["scale"], &[output]);
        let session = prepared
            .model_session(std::sync::Arc::clone(&schema))
            .unwrap();
        let request = RequestBatch::new(
            std::sync::Arc::clone(&schema),
            vec![InputColumn::Quantity(vec![FiniteF64::new(2.0).unwrap()])],
        )
        .unwrap();
        let finishes = |checkpoints: usize| {
            let cancellation =
                graphcal_compiler::cancellation::CancellationToken::cancel_after_successful_checkpoints(
                    checkpoints,
                );
            match prepared.evaluate_request_with_cancellation(&session, &request, &cancellation) {
                Ok(_) => true,
                Err(Outcome::Cancelled) => false,
                Err(Outcome::Failed(error)) => panic!("unexpected failure: {error}"),
            }
        };
        let (mut low, mut high) = (0, 1_usize);
        while !finishes(high) {
            low = high;
            high *= 2;
        }
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if finishes(middle) {
                high = middle;
            } else {
                low = middle;
            }
        }
        high
    };
    let scalar = checkpoints_to_finish("total");
    let indexed = checkpoints_to_finish("indexed");
    assert!(
        indexed >= scalar + ENTRIES,
        "projecting {ENTRIES} entries must observe cancellation per entry: \
         scalar output needs {scalar} checkpoints, indexed output {indexed}"
    );
}

/// Static TODO reachability is value-independent, so one prepared plan
/// answers each query once, however many expressions and evaluations ask
/// it (#2251).
#[test]
fn todo_reachability_is_computed_once_per_plan() {
    use std::fmt::Write as _;

    const CALLERS: usize = 64;
    let mut source = String::from(
        "dag helper { node pending: Dimensionless = todo<Dimensionless> {}; pub node out: Dimensionless = 1.0; }\n",
    );
    for caller in 0..CALLERS {
        writeln!(
            source,
            "node caller{caller}: Dimensionless = @helper()::out + {caller}.0;"
        )
        .unwrap();
    }
    let project = crate::loader::LoadedProject::from_source(&source, "reach.gcl").unwrap();
    let prepared = ProjectCompiler::new(&project).prepare().unwrap();
    let row = prepared.binding_builder().finish().unwrap();
    let (_, first) = graphcal_eval::pipeline_metrics::measure(|| prepared.evaluate(&row));
    let (_, second) = graphcal_eval::pipeline_metrics::measure(|| prepared.evaluate(&row));
    assert!(
        first.unfinished_reach_analyses > 0,
        "the program has a todo, so reachability must be analysed: {first:?}"
    );
    assert!(
        first.unfinished_reach_analyses < 8,
        "each query must be answered once, not once per caller: {first:?}"
    );
    assert_eq!(
        second.unfinished_reach_analyses, 0,
        "a later evaluation of the same plan must reuse every answer: {second:?}"
    );
}

/// A chain of DAGs each calling the previous one twice does `2^depth` work
/// from linear source (#2251). Evaluation has no operation budget, so the
/// chain must stop promptly on cancellation instead.
#[test]
fn cancellation_stops_an_exponential_dag_call_chain() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;
    use std::{fmt::Write as _, sync::mpsc, thread, time::Duration};

    const DEPTH: usize = 48;
    // The leaf DAG, reached only through every level of the chain, reports
    // each evaluation through a test host function.
    let mut source = String::from(
        "import plugin \"graphcal:chain-test\" as test {\n    fn leaf(x: Dimensionless) -> Dimensionless;\n}\n\
         dag d0 { pub node out: Dimensionless = test::leaf(1.0); }\n",
    );
    for level in 1..=DEPTH {
        let previous = level - 1;
        writeln!(
            source,
            "dag d{level} {{ pub node out: Dimensionless = @d{previous}()::out + @d{previous}()::out; }}"
        )
        .unwrap();
    }
    writeln!(source, "node result: Dimensionless = @d{DEPTH}()::out;").unwrap();
    let project = crate::loader::LoadedProject::from_source(&source, "calls.gcl").unwrap();
    let leaves = Arc::new(AtomicUsize::new(0));
    let observed_leaves = Arc::clone(&leaves);
    let mut host_fns = graphcal_eval::host_fns::HostFunctionRegistry::new();
    register_test_host_fn(
        &mut host_fns,
        "graphcal:chain-test",
        "leaf",
        &["x"],
        graphcal_compiler::function_signature::NamedParamKind::dimensionless(),
        move |arguments| {
            observed_leaves.fetch_add(1, Ordering::SeqCst);
            Ok(graphcal_eval::host_fns::HostFnValue::from_argument(
                &arguments[0],
            ))
        },
    );
    let cancellation_source = graphcal_compiler::cancellation::CancellationSource::new();
    let cancellation = cancellation_source.token();
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        // Compile without the token so that cancellation can only stop the
        // evaluation itself.
        let prepared = ProjectCompiler::new(&project)
            .host_fns(&host_fns)
            .prepare()
            .unwrap();
        let row = prepared.binding_builder().finish().unwrap();
        sender
            .send(prepared.evaluate_with_cancellation(&row, &cancellation))
            .unwrap();
    });

    // Cancel only once evaluation has descended through the whole chain:
    // the leaf has been evaluated inside `DEPTH` nested calls.
    let deadline = Instant::now() + Duration::from_secs(60);
    while leaves.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "evaluation never reached the leaf"
        );
        thread::sleep(Duration::from_millis(1));
    }
    cancellation_source.cancel();

    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("a cancelled call chain should stop promptly");
    let error = result.expect_err("the call chain should be cancelled");
    assert!(
        matches!(error, Outcome::Cancelled),
        "unexpected outcome: {error:?}"
    );
    let evaluated = leaves.load(Ordering::SeqCst);
    assert!(
        u64::try_from(evaluated).unwrap() < 1_u64 << DEPTH,
        "the chain stopped inside its 2^{DEPTH} leaf evaluations, after {evaluated}"
    );
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "clearer to express expected math directly"
)]
fn eval_rocket_milestone() {
    let source = include_str!("../../../../tests/fixtures/valid/rocket.gcl");
    let result = compile_and_eval(source).unwrap();

    assert!((find_value(&result, "dry_mass") - 1200.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "fuel_mass") - 2800.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "isp") - 320.0).abs() < f64::EPSILON);
    assert!((find_value(&result, "g0") - 9.80665).abs() < 1e-10);

    let v_exhaust = find_value(&result, "v_exhaust");
    assert!(
        (v_exhaust - 320.0 * 9.80665).abs() < 0.001,
        "v_exhaust = {v_exhaust}"
    );

    let mass_ratio = find_value(&result, "mass_ratio");
    assert!(
        (mass_ratio - (4000.0 / 1200.0)).abs() < 1e-6,
        "mass_ratio = {mass_ratio}"
    );

    let delta_v = find_value(&result, "delta_v");
    let expected_delta_v = 320.0 * 9.80665 * (4000.0_f64 / 1200.0).ln();
    assert!(
        (delta_v - expected_delta_v).abs() < 0.001,
        "delta_v = {delta_v}, expected = {expected_delta_v}"
    );
}

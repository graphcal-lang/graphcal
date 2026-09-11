//! The experiments' suggested edits must produce the advertised behavior through
//! the same prepared-project parameter binding API used by the playground.
#![cfg(not(target_arch = "wasm32"))]
#![expect(
    clippy::panic,
    reason = "test failures include the complete browser compilation/evaluation outcome"
)]

use std::error::Error;
use std::path::Path;

use graphcal_wasm::{
    AssertionOutcomeView, BindingRequest, EvaluateOutcome, PlaygroundFile, PlaygroundRequest,
    PrepareOutcome, prepare,
};

fn experiment(
    filename: &str,
    bindings: &[(&str, &str)],
    assertions: &str,
) -> Result<(), Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/playground/examples")
        .join(filename);
    let source = std::fs::read_to_string(path)?;
    let project = match prepare(PlaygroundRequest {
        entry: filename.to_owned(),
        files: vec![PlaygroundFile {
            path: filename.to_owned(),
            content: format!("{source}\n{assertions}\n"),
        }],
    }) {
        PrepareOutcome::Prepared(project) => project,
        PrepareOutcome::Rejected { error } => panic!("{filename}: {error:?}"),
        PrepareOutcome::CompileError { diagnostics } => panic!("{filename}: {diagnostics:?}"),
    };
    let bindings = bindings
        .iter()
        .map(|(name, expr)| BindingRequest {
            name: (*name).to_owned(),
            expr: (*expr).to_owned(),
        })
        .collect::<Vec<_>>();
    let evaluation = match project.evaluate(&bindings) {
        EvaluateOutcome::Evaluated { evaluation } => evaluation,
        outcome => panic!("{filename} {bindings:?}: {outcome:?}"),
    };
    assert!(
        !evaluation.has_errors && evaluation.notices.is_empty(),
        "{filename} {bindings:?}: {evaluation:?}"
    );
    assert_eq!(evaluation.figures.len(), 2, "{filename}");
    assert!(!evaluation.assertions.is_empty(), "{filename}");
    assert!(
        evaluation
            .assertions
            .iter()
            .all(|assertion| matches!(assertion.outcome, AssertionOutcomeView::Pass)),
        "{filename} {bindings:?}: {:?}",
        evaluation.assertions
    );
    Ok(())
}

#[test]
fn added_mass_can_help_or_hurt_at_different_frequencies() -> Result<(), Box<dyn Error>> {
    experiment(
        "resonance.gcl",
        &[("mass", "15.0 kg")],
        "assert heavier_helps = @your_amplitude < @reference_amplitude / 5.0;",
    )?;
    experiment(
        "resonance.gcl",
        &[("mass", "15.0 kg"), ("driving_frequency", "2.6 Hz")],
        "assert heavier_hurts = @your_amplitude > @reference_amplitude * 5.0;",
    )
}

#[test]
fn opposite_phase_cancels_at_the_speaker_centerline() -> Result<(), Box<dyn Error>> {
    experiment(
        "speaker_interference.gcl",
        &[],
        "assert constructive = abs(@louder_than_one_speaker - 4.0) < 1.0e-10;",
    )?;
    experiment(
        "speaker_interference.gcl",
        &[("relative_phase", "180.0 deg")],
        "assert destructive = @louder_than_one_speaker < 1.0e-20;",
    )
}

#[test]
fn aliased_signals_share_samples_and_fold_backwards() -> Result<(), Box<dyn Error>> {
    [
        ("1.0 Hz", "10.0 Hz", "1.0 Hz"),
        ("6.0 Hz", "10.0 Hz", "4.0 Hz"),
        ("9.0 Hz", "10.0 Hz", "1.0 Hz"),
        ("10.0 Hz", "10.0 Hz", "0.0 Hz"),
        ("9.0 Hz", "25.0 Hz", "9.0 Hz"),
    ]
    .into_iter()
    .try_for_each(|(signal, sampling, alias)| {
        experiment(
            "aliasing.gcl",
            &[("signal_frequency", signal), ("sampling_rate", sampling)],
            &format!("assert expected_alias = abs(@alias_frequency - ({alias})) < 1.0e-10 Hz;"),
        )
    })
}

#[test]
fn logistic_growth_has_cycles_chaos_and_a_periodic_window() -> Result<(), Box<dyn Error>> {
    [("2.8", 1), ("3.2", 2), ("3.5", 4), ("3.83", 3)]
        .into_iter()
        .try_for_each(|(growth, period)| {
            experiment(
                "logistic_chaos.gcl",
                &[("growth", growth)],
                &format!(
                    "assert periodic_tail = for n: Generation {{
                        if coord(n) >= 100.0 {{
                            abs(@population[n, Start#InitialA] -
                                @population[nearest_key(Generation, coord(n) - {period}.0), Start#InitialA]) < 1.0e-5
                        }} else {{ true }}
                    }};"
                ),
            )
        })?;
    experiment(
        "logistic_chaos.gcl",
        &[("growth", "3.9")],
        "assert sensitive_to_initial_conditions = @largest_separation > 0.5;",
    )
}

#[test]
fn queue_wait_explodes_and_has_no_finite_mean_at_capacity() -> Result<(), Box<dyn Error>> {
    [("48.0 1/h", "4.0 min"), ("57.0 1/h", "19.0 min")]
        .into_iter()
        .try_for_each(|(rate, wait)| {
            experiment(
                "queue_cliff.gcl",
                &[("arrival_rate", rate)],
                &format!(
                    "assert expected_wait = match @current_wait {{
                        FiniteMean(wait: wait) => abs(wait - ({wait})) < 1.0e-8 s,
                        NoFiniteSteadyStateMean => false,
                    }};"
                ),
            )
        })?;
    experiment(
        "queue_cliff.gcl",
        &[("arrival_rate", "57.0 1/h"), ("service_time", "0.9 min")],
        "assert small_speedup_helps = match @current_wait {
            FiniteMean(wait: wait) => wait < 6.0 min,
            NoFiniteSteadyStateMean => false,
        };",
    )?;
    ["60.0 1/h", "70.0 1/h"].into_iter().try_for_each(|rate| {
        experiment(
            "queue_cliff.gcl",
            &[("arrival_rate", rate)],
            "assert no_finite_wait = match @current_wait {
                FiniteMean(wait: wait) => false,
                NoFiniteSteadyStateMean => true,
            };
            assert no_steady_state = !@steady_state_exists;",
        )
    })
}

#[test]
fn feedback_delay_destabilizes_an_otherwise_monotonic_controller() -> Result<(), Box<dyn Error>> {
    ["1.0 Hz", "4.0 Hz"].into_iter().try_for_each(|gain| {
        experiment(
            "delayed_control.gcl",
            &[("gain", gain)],
            "assert no_overshoot = @maximum_overshoot == 0.0 m;
            assert matches_analytic_response = for t: Moment {
                abs(@position[t, Controller#NoDelay] - @position[t, Controller#YourDelay]) < 1.0e-12 m
            };",
        )
    })?;
    experiment(
        "delayed_control.gcl",
        &[("gain", "4.0 Hz"), ("delay_steps", "10")],
        "assert growing_oscillations = @maximum_overshoot > 10.0 m;",
    )?;
    experiment(
        "delayed_control.gcl",
        &[("gain", "2.0 Hz"), ("delay_steps", "10")],
        "assert bounded_overshoot = @maximum_overshoot > 0.0 m && @maximum_overshoot < 1.0 m;
        assert settled = abs(@history[nearest_key(Moment, 20.0 s), 0] - @target) < 0.01 m;",
    )
}

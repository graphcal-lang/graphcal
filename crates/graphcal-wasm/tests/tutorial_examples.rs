// Native-only integration tests load canonical examples from the documentation tree.
#![cfg(not(target_arch = "wasm32"))]

use std::error::Error;
use std::path::{Path, PathBuf};

use graphcal_wasm::{
    EvaluationView, PlaygroundFile, PlaygroundOutcome, PlaygroundRequest, evaluate,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ExampleDescriptor {
    entry: String,
    files: Vec<String>,
}

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/assets/playground/examples")
}

fn load_example(name: &str) -> Result<PlaygroundRequest, Box<dyn Error>> {
    let directory = examples_root().join(name);
    let descriptor_text = std::fs::read_to_string(directory.join("project.json"))?;
    let descriptor: ExampleDescriptor = serde_json::from_str(&descriptor_text)?;
    let files = descriptor
        .files
        .into_iter()
        .map(|path| {
            std::fs::read_to_string(directory.join(&path))
                .map(|content| PlaygroundFile { path, content })
        })
        .collect::<Result<_, _>>()?;
    Ok(PlaygroundRequest {
        entry: descriptor.entry,
        files,
    })
}

fn evaluate_example(name: &str) -> Result<EvaluationView, Box<dyn Error>> {
    match evaluate(load_example(name)?) {
        PlaygroundOutcome::Evaluated { evaluation } => Ok(evaluation),
        outcome => {
            Err(std::io::Error::other(format!("{name} did not evaluate: {outcome:?}")).into())
        }
    }
}

#[test]
fn every_tutorial_example_evaluates_without_errors() -> Result<(), Box<dyn Error>> {
    [
        ("step-1", &["mass_with_margin", "mass_ratio"][..]),
        ("step-2", &["delta_v"][..]),
        ("step-3", &["transfer", "total_dv", "tof_hours"][..]),
        (
            "step-4",
            &["v_parking", "transfer_dv", "departure_dv", "total"][..],
        ),
        ("step-5", &["delta_v"][..]),
        ("step-6", &["total_dv", "cumulative_dv"][..]),
    ]
    .into_iter()
    .try_for_each(|(name, expected_names)| {
        let evaluation = evaluate_example(name)?;
        assert!(!evaluation.has_errors, "{name} has runtime errors");
        assert!(!evaluation.values.is_empty(), "{name} produced no values");
        assert!(
            expected_names.iter().all(|expected| evaluation
                .values
                .iter()
                .any(|declaration| declaration.name == *expected)),
            "{name} did not produce every expected value: {expected_names:?}"
        );
        Ok::<_, Box<dyn Error>>(())
    })
}

#[test]
fn multi_file_example_exposes_expected_surface() -> Result<(), Box<dyn Error>> {
    let evaluation = evaluate_example("step-5")?;
    let names: Vec<&str> = evaluation
        .values
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "g0",
            "dry_mass",
            "fuel_mass",
            "isp",
            "v_exhaust",
            "mass_ratio",
            "delta_v",
        ]
    );
    Ok(())
}

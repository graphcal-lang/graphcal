//! The published single-file catalog is also the native regression manifest.
#![cfg(not(target_arch = "wasm32"))]

use std::error::Error;
use std::path::Path;

use graphcal_wasm::{
    AssertionOutcomeView, PlaygroundFile, PlaygroundOutcome, PlaygroundRequest, evaluate,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Example {
    id: String,
    filename: String,
    source_path: String,
    expected_values: Vec<String>,
    expected_figures: usize,
}

#[test]
fn every_published_single_file_example_evaluates() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let catalog: Vec<Example> = serde_json::from_str(&std::fs::read_to_string(
        root.join("web/playground/examples/catalog.json"),
    )?)?;
    assert!(catalog.len() >= 7);
    catalog.into_iter().try_for_each(|example| {
        let source = std::fs::read_to_string(root.join(&example.source_path))?;
        let outcome = evaluate(PlaygroundRequest {
            entry: example.filename.clone(),
            files: vec![PlaygroundFile {
                path: example.filename,
                content: source,
            }],
        });
        let PlaygroundOutcome::Evaluated { evaluation } = outcome else {
            panic!("{}: {outcome:?}", example.id);
        };
        assert!(!evaluation.has_errors, "{}: {evaluation:?}", example.id);
        assert!(
            evaluation.notices.is_empty(),
            "{}: {:?}",
            example.id,
            evaluation.notices
        );
        assert_eq!(
            evaluation.figures.len(),
            example.expected_figures,
            "{}",
            example.id
        );
        assert!(
            evaluation
                .assertions
                .iter()
                .all(|assertion| matches!(assertion.outcome, AssertionOutcomeView::Pass))
        );
        assert!(
            example
                .expected_values
                .iter()
                .all(|name| evaluation.values.iter().any(|value| value.name == *name)),
            "{}: missing expected values",
            example.id
        );
        Ok::<_, Box<dyn Error>>(())
    })
}

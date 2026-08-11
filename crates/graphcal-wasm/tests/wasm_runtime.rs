// Bare-Wasm integration tests run through wasm-bindgen-test's Node runner.
#![cfg(target_arch = "wasm32")]

use graphcal_wasm::{PlaygroundFile, PlaygroundOutcome, PlaygroundRequest, evaluate};
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn in_memory_project_evaluates_on_bare_wasm() {
    let outcome = evaluate(PlaygroundRequest {
        entry: "main.gcl".to_string(),
        files: vec![PlaygroundFile {
            path: "main.gcl".to_string(),
            content: "param x: Dimensionless = 2.0;\nnode doubled: Dimensionless = @x * 2.0;"
                .to_string(),
        }],
    });

    let PlaygroundOutcome::Evaluated { evaluation } = outcome else {
        panic!("expected evaluation, got {outcome:?}");
    };
    assert_eq!(evaluation.values.len(), 2);
    assert!(!evaluation.has_errors);
}

#[wasm_bindgen_test]
fn table_cardinality_diagnostic_preserves_u64_count_on_wasm() {
    let outcome = evaluate(PlaygroundRequest {
        entry: "main.gcl".to_string(),
        files: vec![PlaygroundFile {
            path: "main.gcl".to_string(),
            content: "param m: Dimensionless[Fin(1), Fin(18446744073709551615)] = table[Fin(1), Fin(18446744073709551615)] { 1.0; };".to_string(),
        }],
    });

    let PlaygroundOutcome::CompileError { diagnostics } = outcome else {
        panic!("expected compile error, got {outcome:?}");
    };
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0].message.contains("18446744073709551615"),
        "{}",
        diagnostics[0].message
    );
}

#[wasm_bindgen_test]
fn multi_file_project_evaluates_on_bare_wasm() {
    let outcome = evaluate(PlaygroundRequest {
        entry: "src/demo/main.gcl".to_string(),
        files: vec![
            PlaygroundFile {
                path: "graphcal.toml".to_string(),
                content: "[package]\nname = \"demo\"\n".to_string(),
            },
            PlaygroundFile {
                path: "src/demo/constants.gcl".to_string(),
                content: "pub const node factor: Dimensionless = 2.0;".to_string(),
            },
            PlaygroundFile {
                path: "src/demo/main.gcl".to_string(),
                content:
                    "import demo.constants.{factor};\nnode doubled: Dimensionless = @factor * 2.0;"
                        .to_string(),
            },
        ],
    });

    let PlaygroundOutcome::Evaluated { evaluation } = outcome else {
        panic!("expected evaluation, got {outcome:?}");
    };
    assert_eq!(evaluation.values.len(), 2);
    assert!(!evaluation.has_errors);
}

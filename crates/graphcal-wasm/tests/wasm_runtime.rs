// Bare-Wasm integration tests run through wasm-bindgen-test's Node runner.
#![cfg(target_arch = "wasm32")]
#![expect(
    clippy::panic,
    clippy::unwrap_used,
    reason = "Wasm integration tests fail immediately when JavaScript fixtures or outcomes are malformed"
)]

use graphcal_wasm::{
    MAX_PLAYGROUND_FILE_BYTES, MAX_PLAYGROUND_FILES, MAX_PLAYGROUND_PATH_BYTES, PlaygroundFile,
    PlaygroundOutcome, PlaygroundRequest, evaluate, evaluate_project_js,
};
use js_sys::{Array, Function, JsString, Object, Proxy, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;

fn set(target: &JsValue, property: &str, value: &JsValue) {
    assert!(
        Reflect::set(target, &JsValue::from_str(property), value).unwrap(),
        "test fixture property should be writable"
    );
}

fn js_file(path: &JsValue, content: &JsValue) -> JsValue {
    let file = Object::new();
    set(file.as_ref(), "path", path);
    set(file.as_ref(), "content", content);
    file.into()
}

fn js_request(entry: &JsValue, files: &Array) -> JsValue {
    let request = Object::new();
    set(request.as_ref(), "entry", entry);
    set(request.as_ref(), "files", files.as_ref());
    request.into()
}

fn rejected_kind(outcome: &JsValue) -> String {
    let status = Reflect::get(outcome, &JsValue::from_str("status"))
        .unwrap()
        .as_string()
        .unwrap();
    assert_eq!(status, "rejected");
    let error = Reflect::get(outcome, &JsValue::from_str("error")).unwrap();
    Reflect::get(&error, &JsValue::from_str("kind"))
        .unwrap()
        .as_string()
        .unwrap()
}

fn repeated_js_string(value: &str, count: usize) -> JsValue {
    JsString::from(value)
        .repeat(i32::try_from(count).unwrap())
        .into()
}

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
fn javascript_boundary_evaluates_a_valid_bounded_request() {
    let files = Array::new();
    files.push(&js_file(
        &JsValue::from_str("main.gcl"),
        &JsValue::from_str("node x: Dimensionless = 1.0;"),
    ));
    let outcome = evaluate_project_js(js_request(&JsValue::from_str("main.gcl"), &files)).unwrap();
    let status = Reflect::get(&outcome, &JsValue::from_str("status"))
        .unwrap()
        .as_string()
        .unwrap();
    assert_eq!(status, "evaluated");
}

fn property(value: &JsValue, name: &str) -> JsValue {
    Reflect::get(value, &JsValue::from_str(name)).unwrap()
}

fn declaration_outcome(evaluation: &JsValue, name: &str) -> JsValue {
    let values: Array = property(evaluation, "values").dyn_into().unwrap();
    let declaration = (0..values.length())
        .map(|index| values.get(index))
        .find(|value| property(value, "name").as_string().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("missing declaration {name}"));
    property(&declaration, "outcome")
}

fn quantity_si(evaluation: &JsValue, name: &str) -> f64 {
    let outcome = declaration_outcome(evaluation, name);
    assert_eq!(
        property(&outcome, "status").as_string().as_deref(),
        Some("value")
    );
    let value = property(&outcome, "value");
    assert_eq!(
        property(&value, "kind").as_string().as_deref(),
        Some("quantity")
    );
    property(&value, "si_value").as_f64().unwrap()
}

#[wasm_bindgen_test]
fn javascript_boundary_consumes_checked_generic_bounds_and_nominal_roots() {
    for cardinality in [
        "N * 18446744073709551615 * 0 + 1",
        "0 * N * 18446744073709551615 + 1",
        "N * 0 * 18446744073709551615 + 1",
    ] {
        let source = format!(
            r"
type Marker {{ Marker(value: Dimensionless), }}
type Gate<N: Nat> {{
    Gate(value: Dimensionless(min: sum(for i: Fin({cardinality}) {{ Marker(value: 1.0).value }}))),
}}
node accepted: Gate<2> = Gate<2>(value: 1.0);
node too_small: Gate<2> = Gate<2>(value: 0.5);
node result: Dimensionless = @accepted.value;
"
        );
        let files = Array::new();
        files.push(&js_file(
            &JsValue::from_str("main.gcl"),
            &JsValue::from_str(&source),
        ));
        let outcome =
            evaluate_project_js(js_request(&JsValue::from_str("main.gcl"), &files)).unwrap();
        assert_eq!(
            property(&outcome, "status").as_string().as_deref(),
            Some("evaluated"),
            "checked bound failed for {cardinality}: {outcome:?}"
        );
        let evaluation = property(&outcome, "evaluation");
        assert_eq!(
            quantity_si(&evaluation, "result").to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(
            property(&declaration_outcome(&evaluation, "too_small"), "status")
                .as_string()
                .as_deref(),
            Some("error"),
            "the retained field contract must still reject the actual value"
        );
    }
}

#[wasm_bindgen_test]
fn javascript_boundary_preserves_numeric_edges_and_contains_non_finite_results() {
    let source = r"
node matrix: Dimensionless[Fin(4), Fin(4)] = table[Fin(4), Fin(4)] {
    1.0e-200, 0.0, 0.0, 0.0;
    0.0, 1.0e-200, 0.0, 0.0;
    0.0, 0.0, 1.0e200, 0.0;
    0.0, 0.0, 0.0, 1.0e200;
};
node determinant: Dimensionless = det(@matrix);
node samples: Dimensionless[Fin(3)] = table[Fin(3)] { 1.0e308; 1.0e-100; -1.0e308; };
node average: Dimensionless = mean(@samples);
node quotient: Complex<Dimensionless> = complex(5.0e-324, 0.0) / complex(0.5, 0.5);
node real_part: Dimensionless = re(@quotient);
node imaginary_part: Dimensionless = im(@quotient);
node signed_zero: Dimensionless = -0.0;
node overflow: Dimensionless = 1.0e308 * 2.0;
";
    let files = Array::new();
    files.push(&js_file(
        &JsValue::from_str("main.gcl"),
        &JsValue::from_str(source),
    ));
    let outcome = evaluate_project_js(js_request(&JsValue::from_str("main.gcl"), &files)).unwrap();
    assert_eq!(
        property(&outcome, "status").as_string().as_deref(),
        Some("evaluated")
    );
    let evaluation = property(&outcome, "evaluation");
    assert!((quantity_si(&evaluation, "determinant") - 1.0).abs() <= 4.0 * f64::EPSILON);
    assert_eq!(
        quantity_si(&evaluation, "average").to_bits(),
        (1.0e-100_f64 / 3.0).to_bits()
    );
    assert_eq!(quantity_si(&evaluation, "real_part").to_bits(), 1);
    assert_eq!(
        quantity_si(&evaluation, "imaginary_part").to_bits(),
        (-f64::from_bits(1)).to_bits()
    );
    assert_eq!(
        quantity_si(&evaluation, "signed_zero").to_bits(),
        (-0.0_f64).to_bits()
    );
    assert_eq!(
        property(&declaration_outcome(&evaluation, "overflow"), "status")
            .as_string()
            .as_deref(),
        Some("error")
    );
    assert_eq!(property(&evaluation, "has_errors").as_bool(), Some(true));
}

#[wasm_bindgen_test]
fn javascript_boundary_rejects_file_count_before_reading_elements() {
    let files = Array::new_with_length(u32::try_from(MAX_PLAYGROUND_FILES + 1).unwrap());
    let outcome = evaluate_project_js(js_request(&JsValue::from_str("main.gcl"), &files)).unwrap();
    assert_eq!(rejected_kind(&outcome), "too_many_files");
}

#[wasm_bindgen_test]
fn javascript_boundary_rejects_oversized_content_before_copying_it_to_wasm() {
    let files = Array::new();
    files.push(&js_file(
        &JsValue::from_str("main.gcl"),
        &repeated_js_string("x", MAX_PLAYGROUND_FILE_BYTES + 1),
    ));
    let outcome = evaluate_project_js(js_request(&JsValue::from_str("main.gcl"), &files)).unwrap();
    assert_eq!(rejected_kind(&outcome), "file_too_large");
}

#[wasm_bindgen_test]
fn javascript_boundary_rejects_entry_and_file_paths_before_copying_them() {
    let valid_files = Array::new();
    valid_files.push(&js_file(
        &JsValue::from_str("main.gcl"),
        &JsValue::from_str(""),
    ));
    let oversized_entry = repeated_js_string("x", MAX_PLAYGROUND_PATH_BYTES + 1);
    let outcome = evaluate_project_js(js_request(&oversized_entry, &valid_files)).unwrap();
    assert_eq!(rejected_kind(&outcome), "path_too_long");

    let oversized_files = Array::new();
    oversized_files.push(&js_file(
        &repeated_js_string("x", MAX_PLAYGROUND_PATH_BYTES + 1),
        &JsValue::from_str(""),
    ));
    let outcome =
        evaluate_project_js(js_request(&JsValue::from_str("main.gcl"), &oversized_files)).unwrap();
    assert_eq!(rejected_kind(&outcome), "path_too_long");
}

#[wasm_bindgen_test]
fn javascript_boundary_counts_utf8_bytes_without_copying_strings() {
    let exact_path = format!("{}.gcl", "é".repeat((MAX_PLAYGROUND_PATH_BYTES - 4) / 2));
    let files = Array::new();
    files.push(&js_file(
        &JsValue::from_str(&exact_path),
        &JsValue::from_str("node x: Dimensionless = 1.0;"),
    ));
    let outcome = evaluate_project_js(js_request(&JsValue::from_str(&exact_path), &files)).unwrap();
    let status = Reflect::get(&outcome, &JsValue::from_str("status"))
        .unwrap()
        .as_string()
        .unwrap();
    assert_eq!(status, "evaluated");

    let oversized_path = format!("é{exact_path}");
    let files = Array::new();
    files.push(&js_file(
        &JsValue::from_str(&oversized_path),
        &JsValue::from_str(""),
    ));
    let outcome = evaluate_project_js(js_request(&JsValue::from_str("main.gcl"), &files)).unwrap();
    assert_eq!(rejected_kind(&outcome), "path_too_long");
}

#[wasm_bindgen_test]
fn javascript_boundary_rejects_aggregate_path_metadata() {
    let files = Array::new();
    for index in 0..MAX_PLAYGROUND_FILES {
        let path = format!(
            "{index:02}{}.gcl",
            "x".repeat(MAX_PLAYGROUND_PATH_BYTES - 6)
        );
        files.push(&js_file(&JsValue::from_str(&path), &JsValue::from_str("")));
    }
    let first_path = Reflect::get(&files.get(0), &JsValue::from_str("path")).unwrap();
    let outcome = evaluate_project_js(js_request(&first_path, &files)).unwrap();
    assert_eq!(rejected_kind(&outcome), "project_paths_too_large");
}

#[wasm_bindgen_test]
fn javascript_boundary_rejects_an_unknown_cyclic_field_without_recursion() {
    let request = Object::new();
    let files = Array::new();
    files.push(&js_file(
        &JsValue::from_str("main.gcl"),
        &JsValue::from_str(""),
    ));
    set(request.as_ref(), "entry", &JsValue::from_str("main.gcl"));
    set(request.as_ref(), "files", files.as_ref());
    set(request.as_ref(), "cycle", request.as_ref());

    let error = evaluate_project_js(request.into()).unwrap_err();
    assert!(
        error
            .as_string()
            .is_some_and(|message| message.contains("unexpected properties"))
    );
}

#[wasm_bindgen_test]
fn javascript_boundary_contains_revoked_proxy_introspection_errors() {
    let revocable = Proxy::revocable(Array::new().as_ref(), &Object::new());
    let proxy = Reflect::get(revocable.as_ref(), &JsValue::from_str("proxy")).unwrap();
    let revoke: Function = Reflect::get(revocable.as_ref(), &JsValue::from_str("revoke"))
        .unwrap()
        .unchecked_into();
    revoke.call0(&JsValue::UNDEFINED).unwrap();

    let request = Object::new();
    set(request.as_ref(), "entry", &JsValue::from_str("main.gcl"));
    set(request.as_ref(), "files", &proxy);
    let error = evaluate_project_js(request.into()).unwrap_err();
    assert!(
        error
            .as_string()
            .is_some_and(|message| message.contains("could not inspect"))
    );
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
                    "import demo.constants::{factor};\nnode doubled: Dimensionless = @factor * 2.0;"
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

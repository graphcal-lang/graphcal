//! Actual Node/wasm-bindgen transport regressions, not Rust JSON snapshots.
#![cfg(target_arch = "wasm32")]
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "boundary regression assertions"
)]
use js_sys::{Array, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;

fn get(value: &JsValue, name: &str) -> JsValue {
    Reflect::get(value, &JsValue::from_str(name)).unwrap()
}
fn set(value: &JsValue, name: &str, item: &JsValue) {
    assert!(Reflect::set(value, &JsValue::from_str(name), item).unwrap());
}
fn evaluate(source: &str) -> JsValue {
    let file = Object::new();
    set(&file, "path", &JsValue::from_str("main.gcl"));
    set(&file, "content", &JsValue::from_str(source));
    let files = Array::new();
    files.push(&file);
    let request = Object::new();
    set(&request, "entry", &JsValue::from_str("main.gcl"));
    set(&request, "files", &files);
    let outcome = graphcal_wasm::evaluate_project_js(request.into()).unwrap();
    assert_eq!(
        get(&outcome, "status").as_string().as_deref(),
        Some("evaluated"),
        "{outcome:?}"
    );
    get(&outcome, "evaluation")
}
fn declaration(evaluation: &JsValue, name: &str) -> JsValue {
    let values: Array = get(evaluation, "values").dyn_into().unwrap();
    let value = values
        .iter()
        .find(|value| get(value, "name").as_string().as_deref() == Some(name))
        .unwrap();
    get(&value, "outcome")
}

#[wasm_bindgen_test]
fn node_boundary_keeps_si_and_typed_notices_for_scalar_nested_and_plot_failures() {
    let result = evaluate(
        r"
node broken: Dimensionless = 1.0 / 0.0;
unit bad: Length = (@broken) m;
type Reading { Reading(value: Length), }
node chosen: Length = 6.0 m -> bad;
node unchosen: Length = if true { 2.0 m -> cm } else { 1.0 m -> bad };
node nested: Reading[Fin(2)] = for i: Fin(2) { Reading(value: @chosen) };
plot measurement = { mark: point, encode: { x: @chosen } };
",
    );
    assert_eq!(get(&result, "has_errors").as_bool(), Some(true));
    assert_eq!(
        get(&declaration(&result, "broken"), "status")
            .as_string()
            .as_deref(),
        Some("error")
    );
    let chosen = declaration(&result, "chosen");
    assert_eq!(get(&chosen, "status").as_string().as_deref(), Some("value"));
    let value = get(&chosen, "value");
    assert_eq!(get(&value, "si_value").as_f64(), Some(6.0));
    assert_eq!(get(&value, "value").as_f64(), Some(6.0));
    assert_eq!(
        get(&get(&declaration(&result, "unchosen"), "value"), "unit")
            .as_string()
            .as_deref(),
        Some("cm")
    );
    assert_eq!(
        get(&declaration(&result, "nested"), "status")
            .as_string()
            .as_deref(),
        Some("value")
    );
    let notices: Array = get(&result, "notices").dyn_into().unwrap();
    assert!(notices.length() >= 4);
    assert!(notices.iter().all(|notice| get(&notice, "kind").as_string().as_deref() == Some("presentation_error")));
    assert!(notices.iter().any(|notice| {
        get(&notice, "message")
            .as_string()
            .unwrap()
            .contains("channel x")
    }));
    let figures: Array = get(&result, "figures").dyn_into().unwrap();
    assert_eq!(figures.length(), 1);
}

#[wasm_bindgen_test]
fn node_boundary_preserves_literal_and_constant_si_on_label_overflow() {
    let literal = include_str!("../../graphcal-eval/src/eval/tests/display-label-overflow.gcl");
    let constant =
        include_str!("../../graphcal-eval/src/eval/tests/constant-display-label-overflow.gcl");
    let converted = literal.replace("= 1.0 identity_scale", "= 1.0 -> identity_scale");
    for source in [literal, constant, &converted] {
        let result = evaluate(source);
        let outcome = declaration(&result, "value");
        assert_eq!(
            get(&outcome, "status").as_string().as_deref(),
            Some("value")
        );
        let value = get(&outcome, "value");
        assert_eq!(
            get(&value, "si_value").as_f64().unwrap().to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(
            get(&value, "value").as_f64().unwrap().to_bits(),
            1.0_f64.to_bits()
        );
        assert_eq!(get(&result, "has_errors").as_bool(), Some(true));
        let notices: Array = get(&result, "notices").dyn_into().unwrap();
        assert_eq!(notices.length(), 1);
        assert_eq!(
            get(&notices.get(0), "kind").as_string().as_deref(),
            Some("presentation_error")
        );
        assert!(
            get(&notices.get(0), "message")
                .as_string()
                .unwrap()
                .contains("canonical unit exponent accumulation overflowed")
        );
    }
    let valid = evaluate(
        "const unit identity_scale: Length/Length = 1.0 m/m; node value: Dimensionless = 1.0 identity_scale^(1/2147483647);",
    );
    assert_eq!(get(&valid, "has_errors").as_bool(), Some(false));
    assert_eq!(
        get(&get(&declaration(&valid, "value"), "value"), "unit")
            .as_string()
            .as_deref(),
        Some("identity_scale^(1/2147483647)")
    );
}

#[wasm_bindgen_test]
fn node_boundary_distinguishes_repeated_call_evidence_through_local_projection() {
    let result = evaluate(
        r"
dag worker { param large: Bool; pub node out: Length = if @large { 2000.0 m -> km } else { 3.0 m -> cm }; }
type Reading { Reading(value: Length), }
node values: Reading[Fin(2)] = for i: Fin(2) { Reading(value: @worker(large: to_int(i) == 1)::out) };
node small: Length = match @values[0] { Reading(value: local) => local };
node large: Length = match @values[1] { Reading(value: local) => local };
",
    );
    assert_eq!(get(&result, "has_errors").as_bool(), Some(false));
    for (name, unit, displayed, si) in [("small", "cm", 300.0, 3.0), ("large", "km", 2.0, 2000.0)] {
        let value = get(&declaration(&result, name), "value");
        assert_eq!(get(&value, "unit").as_string().as_deref(), Some(unit));
        assert_eq!(get(&value, "value").as_f64(), Some(displayed));
        assert_eq!(get(&value, "si_value").as_f64(), Some(si));
    }
}
